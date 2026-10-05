use std::fmt;
use std::str::FromStr;

use anyhow::bail;
use arc_notes::BranchName;
use chrono::{NaiveDateTime, Timelike};

use crate::section::{SectionHeading, append_to_section, classify, section};

const ENTRY_PREFIX: &str = "### ";
const BRANCH_SEPARATOR: &str = " · ";
const TIME_FORMAT: &str = "%Y-%m-%d %H:%M";
const CONTINUATION_INDENT: &str = "    ";

fn starts_entry(line: &str) -> bool {
    line.starts_with(ENTRY_PREFIX)
}

fn entries(note: &str) -> Vec<String> {
    let scanned = classify(note);
    let Some(section) = section(&scanned, &SectionHeading::sessions()) else {
        return Vec::new();
    };
    let mut lines = section[1..]
        .iter()
        .map(|s| s.line)
        .skip_while(|line| !line.is_heading(starts_entry))
        .peekable();
    let mut entries = Vec::new();
    while let Some(heading) = lines.next() {
        let mut entry = vec![heading.text()];
        while let Some(line) = lines.next_if(|line| !line.is_heading(starts_entry)) {
            entry.push(line.text());
        }
        let len = entry
            .iter()
            .rposition(|line| !line.trim().is_empty())
            .map_or(0, |i| i + 1);
        entries.push(entry[..len].join("\n"));
    }
    entries
}

pub fn latest_sessions(note: &str, count: usize) -> Vec<String> {
    let mut entries = entries(note);
    let skip = entries.len().saturating_sub(count);
    entries.split_off(skip)
}

pub fn latest_session_at(note: &str) -> Option<NaiveDateTime> {
    entries(note)
        .iter()
        .filter_map(|entry| SessionTime::from_header(entry.lines().next()?))
        .map(SessionTime::get)
        .max()
}

pub fn append_session(note: &str, entry: &SessionEntry) -> String {
    append_to_section(note, &SectionHeading::sessions(), &entry.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SessionTime(NaiveDateTime);

impl SessionTime {
    pub fn new(at: NaiveDateTime) -> Self {
        Self(
            at.with_second(0)
                .and_then(|at| at.with_nanosecond(0))
                .unwrap_or(at),
        )
    }

    pub fn now() -> Self {
        Self::new(arc_core::time::now_naive())
    }

    pub fn get(self) -> NaiveDateTime {
        self.0
    }

    fn from_header(line: &str) -> Option<Self> {
        let stamp = line
            .strip_prefix(ENTRY_PREFIX)?
            .split(BRANCH_SEPARATOR)
            .next()?;
        NaiveDateTime::parse_from_str(stamp.trim(), TIME_FORMAT)
            .ok()
            .map(Self)
    }
}

impl fmt::Display for SessionTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.format(TIME_FORMAT).fmt(f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryText(String);

impl EntryText {
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim_end)
            .filter(|l| !l.is_empty())
            .collect();
        if lines.is_empty() {
            bail!("empty session field");
        }
        Ok(Self(lines.join("\n").trim_start().to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for EntryText {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        Self::parse(s)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Handoff {
    pub done: EntryText,
    pub state: EntryText,
    pub next: EntryText,
    pub questions: Option<EntryText>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionEntry {
    pub at: SessionTime,
    pub branch: Option<BranchName>,
    pub handoff: Handoff,
}

impl SessionEntry {
    pub fn header(&self) -> String {
        match &self.branch {
            Some(branch) => format!("{ENTRY_PREFIX}{}{BRANCH_SEPARATOR}{branch}", self.at),
            None => format!("{ENTRY_PREFIX}{}", self.at),
        }
    }
}

fn write_field(f: &mut fmt::Formatter<'_>, label: &str, text: &EntryText) -> fmt::Result {
    let mut lines = text.as_str().split('\n');
    write!(f, "\n**{label}:** {}", lines.next().unwrap_or_default())?;
    lines.try_for_each(|line| write!(f, "\n{CONTINUATION_INDENT}{line}"))
}

impl fmt::Display for SessionEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Handoff {
            done,
            state,
            next,
            questions,
        } = &self.handoff;
        f.write_str(&self.header())?;
        write_field(f, "Done", done)?;
        write_field(f, "State", state)?;
        write_field(f, "Next", next)?;
        questions
            .iter()
            .try_for_each(|q| write_field(f, "Open questions", q))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTE: &str = "---\nid: s\n---\n# Story\n\n## Sessions\n\nstray intro\n\n### 2026-10-01 · one\n**Done:** a\n\n### 2026-10-02 · two\n**Done:** b\n**Next:** c\n\n### 2026-10-03 · three\n**Done:** d\n### 2026-10-04 · four\n**Done:** e\n\n\n## Later\n### not a session\n";

    fn text(s: &str) -> EntryText {
        EntryText::parse(s).unwrap()
    }

    fn at(s: &str) -> SessionTime {
        SessionTime::new(NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap())
    }

    fn entry(stamp: &str, branch: Option<&str>, questions: Option<&str>) -> SessionEntry {
        SessionEntry {
            at: at(stamp),
            branch: branch.map(|b| BranchName::parse(b).unwrap()),
            handoff: Handoff {
                done: text("wired the flow"),
                state: text("tests green"),
                next: text("tune the threshold"),
                questions: questions.map(text),
            },
        }
    }

    const FULL: &str = "### 2026-10-05 14:32 · tenzin/sc-16727/optical-flow\n**Done:** wired the flow\n**State:** tests green\n**Next:** tune the threshold\n**Open questions:** which camera?";

    #[test]
    fn golden_entry_rendering() {
        let full = entry(
            "2026-10-05 14:32:59",
            Some("tenzin/sc-16727/optical-flow"),
            Some("which camera?"),
        );
        assert_eq!(full.to_string(), FULL);
        assert_eq!(
            full.header(),
            "### 2026-10-05 14:32 · tenzin/sc-16727/optical-flow"
        );
        assert_eq!(
            entry("2026-10-05 09:05:00", None, None).to_string(),
            "### 2026-10-05 09:05\n**Done:** wired the flow\n**State:** tests green\n**Next:** tune the threshold"
        );
    }

    #[test]
    fn multi_line_fields_cannot_open_headings_or_fences() {
        let mut e = entry("2026-10-05 14:32:00", None, None);
        e.handoff.done = text(
            "\n  first\r\n\n## Injected\n### 2026-01-01 00:00 · fake\n```\n# x\n  - item  \n\n",
        );
        e.handoff.next = text("one line");
        assert_eq!(
            e.to_string(),
            "### 2026-10-05 14:32\n**Done:** first\n    ## Injected\n    ### 2026-01-01 00:00 · fake\n    ```\n    # x\n      - item\n**State:** tests green\n**Next:** one line"
        );
        let note = append_session("# S\n## Sessions\n", &e);
        let note = append_session(&note, &entry("2026-10-05 15:00:00", None, None));
        let note = append_to_section(&note, &SectionHeading::parse("After").unwrap(), "x");
        let sessions = latest_sessions(&note, 5);
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0], e.to_string());
        assert_eq!(
            latest_session_at(&note),
            Some(at("2026-10-05 15:00:00").get())
        );
        assert!(EntryText::parse(" \n\t\n").is_err());
        assert!("".parse::<EntryText>().is_err());
    }

    #[test]
    fn appending_creates_the_sessions_section() {
        let e = entry(
            "2026-10-05 14:32:00",
            Some("tenzin/sc-16727/optical-flow"),
            Some("which camera?"),
        );
        let before = "---\nid: s\n---\n\n# Story\n\nNotes.\n";
        assert_eq!(
            append_session(before, &e),
            format!("{before}\n## Sessions\n\n{FULL}\n")
        );
        assert_eq!(append_session("", &e), format!("## Sessions\n\n{FULL}\n"));
    }

    #[test]
    fn appending_goes_after_the_last_entry() {
        let e = entry("2026-10-05 14:32:00", None, None);
        let at_end = "# S\n## Sessions\n\n### 2026-10-04 10:00\n**Done:** a\n";
        assert_eq!(append_session(at_end, &e), format!("{at_end}\n{e}\n"));
        let followed = "# S\n## Sessions\n### 2026-10-04 10:00\n**Done:** a\n\n## Log\n- x\n";
        assert_eq!(
            append_session(followed, &e),
            format!("# S\n## Sessions\n### 2026-10-04 10:00\n**Done:** a\n\n{e}\n\n## Log\n- x\n")
        );
        let fenced = "# S\n## Sessions\n### 2026-10-04 10:00\n```sh\n## not a section\n# nor this\n```\n## Log\n";
        assert_eq!(
            append_session(fenced, &e),
            format!(
                "# S\n## Sessions\n### 2026-10-04 10:00\n```sh\n## not a section\n# nor this\n```\n\n{e}\n\n## Log\n"
            )
        );
    }

    #[test]
    fn appending_keeps_crlf() {
        let e = entry("2026-10-05 14:32:00", Some("b"), None);
        let note =
            "# S\r\n\r\n## Sessions\r\n\r\n### 2026-10-04 10:00\r\n**Done:** a\r\n\r\n## Log\r\n";
        let crlf = e.to_string().replace('\n', "\r\n");
        assert_eq!(
            append_session(note, &e),
            format!(
                "# S\r\n\r\n## Sessions\r\n\r\n### 2026-10-04 10:00\r\n**Done:** a\r\n\r\n{crlf}\r\n\r\n## Log\r\n"
            )
        );
        assert_eq!(
            latest_sessions(&append_session(note, &e), 1),
            vec![e.to_string()]
        );
    }

    #[test]
    fn written_entries_read_back_in_order() {
        let entries = [
            entry("2026-10-05 09:00:00", Some("a"), None),
            entry("2026-10-05 11:30:00", None, Some("q")),
            entry("2026-10-05 10:15:00", Some("c"), None),
        ];
        let note = entries.iter().fold("# S\n\nbody\n".to_owned(), |note, e| {
            append_session(&note, e)
        });
        assert_eq!(
            latest_sessions(&note, 5),
            entries.iter().map(ToString::to_string).collect::<Vec<_>>()
        );
        assert_eq!(
            latest_session_at(&note),
            Some(at("2026-10-05 11:30:00").get())
        );
    }

    #[test]
    fn session_times_skip_unparseable_headers() {
        assert_eq!(latest_session_at(NOTE), None);
        assert_eq!(latest_session_at("# S\n"), None);
        let note = "## Sessions\n### 2026-10-05 14:32 · b\n### garbage\n### 2026-10-04\n### 2026-13-01 10:00\n```\n### 2026-12-01 10:00\n```\n## Later\n### 2026-12-02 10:00\n";
        assert_eq!(
            latest_session_at(note),
            Some(at("2026-10-05 14:32:00").get())
        );
        assert_eq!(
            latest_session_at("## Sessions\n###  2026-10-05 08:01  \n"),
            Some(at("2026-10-05 08:01:00").get())
        );
    }

    #[test]
    fn latest_entries_in_file_order() {
        assert_eq!(
            latest_sessions(NOTE, 3),
            vec![
                "### 2026-10-02 · two\n**Done:** b\n**Next:** c",
                "### 2026-10-03 · three\n**Done:** d",
                "### 2026-10-04 · four\n**Done:** e",
            ]
        );
    }

    #[test]
    fn fewer_entries_than_requested() {
        let note = "# S\n## Sessions\n### only\nbody\n";
        assert_eq!(latest_sessions(note, 3), vec!["### only\nbody"]);
        assert!(latest_sessions(NOTE, 0).is_empty());
    }

    #[test]
    fn missing_or_empty_section() {
        assert!(latest_sessions("# S\n\n### stray\n", 3).is_empty());
        assert!(latest_sessions("# S\n## Sessions\n\n## Other\n### x\n", 3).is_empty());
        assert!(latest_sessions("", 3).is_empty());
    }

    #[test]
    fn section_ends_at_next_top_or_second_level_heading() {
        let note = "## Sessions\r\n### a\r\nx\r\n# Appendix\r\n### b\r\n";
        assert_eq!(latest_sessions(note, 3), vec!["### a\nx"]);
        let note = "## Sessions\n### a\n#### detail\n## Notes\n";
        assert_eq!(latest_sessions(note, 3), vec!["### a\n#### detail"]);
    }

    #[test]
    fn headings_inside_code_fences_are_entry_text() {
        let note = "## Sessions\n### a\n```bash\n# build\ncargo build\n### not an entry\n```\nafter\n### b\n~~~~\n## still code\n~~~\n~~~~~\n### c\n";
        assert_eq!(
            latest_sessions(note, 3),
            vec![
                "### a\n```bash\n# build\ncargo build\n### not an entry\n```\nafter",
                "### b\n~~~~\n## still code\n~~~\n~~~~~",
                "### c",
            ]
        );
    }

    #[test]
    fn fences_match_marker_and_length() {
        let note = "## Sessions\n### a\n````\n```\n# inside\n~~~\n````\n# Appendix\n### b\n";
        assert_eq!(
            latest_sessions(note, 3),
            vec!["### a\n````\n```\n# inside\n~~~\n````"]
        );
        let note = "## Sessions\n### a\n    ```\n# Appendix\n";
        assert_eq!(latest_sessions(note, 3), vec!["### a\n    ```"]);
        let note = "## Sessions\n### a\n``` not`a fence\n# Appendix\n";
        assert_eq!(latest_sessions(note, 3), vec!["### a\n``` not`a fence"]);
        let note = "```\n## Sessions\n```\n## Sessions\n### real\n";
        assert_eq!(latest_sessions(note, 3), vec!["### real"]);
        let note = "## Sessions\n### a\n```\n# unclosed\n### b\n";
        assert_eq!(
            latest_sessions(note, 3),
            vec!["### a\n```\n# unclosed\n### b"]
        );
    }
}
