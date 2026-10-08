use std::collections::HashMap;
use std::sync::OnceLock;

use chrono::NaiveDate;
use regex::Regex;

use uuid::Uuid;

use crate::checkbox::{Checkbox, split_checkbox};
use crate::path::NoteRelPath;
use crate::todos::Todo;

const TODO_ID_NAMESPACE: Uuid = Uuid::from_u128(0x1243f4f0_6526_4859_a227_e672f1bf19e3);

fn date_token_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"@(\d{2}-\d{2}-\d{4})").expect("date token regex compiles")
    })
}

fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn parse_note(content: &str, file: &NoteRelPath) -> Vec<Todo> {
    let re = date_token_regex();
    let mut occurrences: HashMap<String, u32> = HashMap::new();

    content.lines().enumerate().filter_map(|(idx, raw_line)| {
        let line_no = (idx as u32) + 1;
        let (checkbox, rest) = split_checkbox(raw_line.trim_start())?;

        let (date, text) = match re
            .captures(rest)
            .and_then(|c| {
                let m = c.get(0)?;
                let d = NaiveDate::parse_from_str(&c[1], "%d-%m-%Y").ok()?;
                Some((d, m.range()))
            }) {
            Some((d, range)) => (
                Some(d),
                collapse_whitespace(&format!("{}{}", &rest[..range.start], &rest[range.end..])),
            ),
            None => (None, collapse_whitespace(rest)),
        };

        let normalized = text.to_lowercase();
        let occurrence = occurrences.entry(normalized.clone()).or_default();
        let id = todo_id(file, &normalized, *occurrence);
        *occurrence += 1;

        Some(Todo {
            id,
            text,
            date,
            completed: checkbox.is_done(),
            file: file.clone(),
            line: line_no,
        })
    })
    .collect()
}

pub fn todo_id(file: &NoteRelPath, normalized_text: &str, occurrence: u32) -> Uuid {
    let key = format!("{file}\0{normalized_text}\0{occurrence}");
    Uuid::new_v5(&TODO_ID_NAMESPACE, key.as_bytes())
}

pub fn render_todo_line(state: Checkbox, text: &str, date: Option<NaiveDate>) -> String {
    let text = collapse_whitespace(text);
    match date {
        Some(date) => format!("{}{text} @{}", state.marker(), date.format("%d-%m-%Y")),
        None => format!("{}{text}", state.marker()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f() -> NoteRelPath {
        NoteRelPath::parse("note.md").unwrap()
    }

    #[test]
    fn parses_single_open_checkbox() {
        let t = parse_note("- [ ] write the thing", &f());
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].text, "write the thing");
        assert!(!t[0].completed);
        assert_eq!(t[0].date, None);
        assert_eq!(t[0].line, 1);
    }

    #[test]
    fn parses_completed_checkbox_case_insensitive() {
        let t = parse_note("- [x] done\n- [X] also done", &f());
        assert_eq!(t.len(), 2);
        assert!(t[0].completed);
        assert!(t[1].completed);
    }

    #[test]
    fn extracts_dd_mm_yyyy_date_and_strips_from_text() {
        let t = parse_note("- [ ] follow up @10-06-2026 please", &f());
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].date, Some(NaiveDate::from_ymd_opt(2026, 6, 10).unwrap()));
        assert_eq!(t[0].text, "follow up please");
    }

    #[test]
    fn invalid_date_retained_in_text_no_date_field() {
        let t = parse_note("- [ ] todo @32-01-2026", &f());
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].date, None);
        assert_eq!(t[0].text, "todo @32-01-2026");
    }

    #[test]
    fn ignores_indentation_in_recognition() {
        let t = parse_note("    - [ ] indented", &f());
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].text, "indented");
    }

    #[test]
    fn multiple_todos_in_one_file_with_line_numbers() {
        let content = "# Header\n- [ ] one\nsome prose\n- [x] two\n";
        let t = parse_note(content, &f());
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].line, 2);
        assert_eq!(t[1].line, 4);
    }

    #[test]
    fn no_checkboxes_returns_empty() {
        let t = parse_note("# Title\n\njust prose.", &f());
        assert!(t.is_empty());
    }

    #[test]
    fn id_stable_across_whitespace_case_and_completion() {
        let a = parse_note("- [ ] hello world", &f());
        let b = parse_note("    - [x]   Hello World   ", &f());
        assert_eq!(a[0].id, b[0].id);
    }

    #[test]
    fn id_stable_across_date_edits() {
        let a = parse_note("- [ ] hello", &f());
        let b = parse_note("- [ ] hello @10-06-2026", &f());
        assert_eq!(a[0].id, b[0].id);
    }

    #[test]
    fn id_changes_after_text_edit() {
        let a = parse_note("- [ ] hello", &f());
        let b = parse_note("- [ ] hello world", &f());
        assert_ne!(a[0].id, b[0].id);
    }

    #[test]
    fn id_stable_across_line_moves() {
        let a = parse_note("- [ ] x", &f());
        let b = parse_note("# header\n\n- [ ] x", &f());
        assert_eq!(a[0].id, b[0].id);
        assert_ne!(a[0].line, b[0].line);
    }

    #[test]
    fn id_differs_between_notes() {
        let a = parse_note("- [ ] x", &f());
        let b = parse_note("- [ ] x", &NoteRelPath::parse("other.md").unwrap());
        assert_ne!(a[0].id, b[0].id);
    }

    #[test]
    fn duplicate_checkboxes_get_distinct_ids() {
        let t = parse_note("- [ ] call\n- [x] call\n", &f());
        assert_ne!(t[0].id, t[1].id);
    }

    #[test]
    fn todo_ids_are_pinned_so_every_machine_agrees() {
        let id = todo_id(&NoteRelPath::parse("stories/a.md").unwrap(), "write the thing", 0);
        assert_eq!(id.to_string(), "de2ae85b-25cf-5f26-8927-89596c6df8a6");
    }

    #[test]
    fn rendered_line_parses_back_to_the_same_todo() {
        let date = NaiveDate::from_ymd_opt(2026, 10, 9);
        let line = render_todo_line(Checkbox::Open, "book   flights", date);
        assert_eq!(line, "- [ ] book flights @09-10-2026");
        let t = parse_note(&line, &f()).remove(0);
        assert_eq!((t.text.as_str(), t.date, t.completed), ("book flights", date, false));
        assert_eq!(render_todo_line(Checkbox::Done, "shipped", None), "- [x] shipped");
    }
}
