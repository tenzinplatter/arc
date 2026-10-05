use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::OnceLock;

use chrono::NaiveDate;
use regex::Regex;

use uuid::Uuid;

use crate::checkbox::split_checkbox;
use crate::path::NoteRelPath;
use crate::todos::{Todo, TodoSource};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTodo {
    pub text: String,
    pub completed: bool,
    pub date: Option<NaiveDate>,
    pub file: NoteRelPath,
    pub line: u32,
    pub fingerprint: u64,
}

impl ParsedTodo {
    pub fn into_todo(self, id: Uuid) -> Todo {
        Todo {
            id,
            text: self.text,
            date: self.date,
            completed: self.completed,
            source: TodoSource::NoteParsed {
                file: self.file,
                line: self.line,
                fingerprint: self.fingerprint,
            },
        }
    }
}

fn date_token_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"@(\d{2}-\d{2}-\d{4})").expect("date token regex compiles")
    })
}

fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn parse_note(content: &str, file: &NoteRelPath) -> Vec<ParsedTodo> {
    let re = date_token_regex();

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
        let fp = fingerprint(Path::new(file.as_str()), &normalized);

        Some(ParsedTodo {
            text,
            completed: checkbox.is_done(),
            date,
            file: file.clone(),
            line: line_no,
            fingerprint: fp,
        })
    })
    .collect()
}

/// `DefaultHasher` output may change across Rust toolchains, re-iding every note-parsed todo once.
pub fn fingerprint(file: &Path, normalized_text: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    file.hash(&mut h);
    normalized_text.hash(&mut h);
    h.finish()
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
    fn fingerprint_stable_across_whitespace_in_text() {
        let a = parse_note("- [ ] hello world", &f());
        let b = parse_note("    - [ ]   hello world   ", &f());
        assert_eq!(a[0].fingerprint, b[0].fingerprint);
    }

    #[test]
    fn fingerprint_changes_after_text_edit() {
        let a = parse_note("- [ ] hello", &f());
        let b = parse_note("- [ ] hello world", &f());
        assert_ne!(a[0].fingerprint, b[0].fingerprint);
    }

    #[test]
    fn fingerprint_stable_across_line_moves() {
        let a = parse_note("- [ ] x", &f());
        let b = parse_note("# header\n\n- [ ] x", &f());
        assert_eq!(a[0].fingerprint, b[0].fingerprint);
        assert_ne!(a[0].line, b[0].line);
    }

    #[test]
    fn into_todo_keeps_parsed_fields_as_note_source() {
        let p = parse_note("- [x] ship @10-06-2026", &f()).remove(0);
        let fp = p.fingerprint;
        let todo = p.into_todo(Uuid::nil());
        assert_eq!(
            todo,
            Todo {
                id: Uuid::nil(),
                text: "ship".into(),
                date: NaiveDate::from_ymd_opt(2026, 6, 10),
                completed: true,
                source: TodoSource::NoteParsed { file: f(), line: 1, fingerprint: fp },
            }
        );
    }

    #[test]
    fn todo_ids_stay_stable_across_releases_because_fingerprint_value_is_pinned() {
        assert_eq!(fingerprint(Path::new("stories/a.md"), "write the thing"), 9874501586080003358);
    }
}
