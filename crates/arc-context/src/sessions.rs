use std::fmt;
use std::str::FromStr;

use anyhow::bail;
use arc_notes::BranchName;
use chrono::{DateTime, FixedOffset, NaiveDateTime, Timelike};

use crate::section::{SectionHeading, append_to_section, classify, section};

const ENTRY_PREFIX: &str = "### ";
const BRANCH_SEPARATOR: &str = " · ";
const TIME_FORMAT: &str = "%Y-%m-%d %H:%M %:z";
const OFFSET_FORMATS: [&str; 2] = [TIME_FORMAT, "%Y-%m-%d %H:%M %z"];
const LOCAL_FORMAT: &str = "%Y-%m-%d %H:%M";
const CONTINUATION_INDENT: &str = "    ";
const AUTO_TAG: &str = "[auto]";
const SHOWN_COMMITS: usize = 10;

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

fn is_auto(entry: &str) -> bool {
    entry
        .lines()
        .next()
        .and_then(EntryHeader::parse)
        .is_some_and(|h| h.kind == EntryKind::Auto)
}

pub fn context_sessions(note: &str, count: usize) -> Vec<String> {
    let entries = entries(note);
    let mut shown: Vec<usize> = (entries.len().saturating_sub(count)..entries.len()).collect();
    let last_handoff = entries.iter().rposition(|entry| !is_auto(entry));
    if let (Some(handoff), Some(oldest)) = (last_handoff, shown.first_mut())
        && handoff < *oldest
    {
        *oldest = handoff;
    }
    shown.into_iter().map(|i| entries[i].clone()).collect()
}

fn headers(note: &str) -> Vec<EntryHeader> {
    entries(note)
        .iter()
        .filter_map(|entry| EntryHeader::parse(entry.lines().next()?))
        .collect()
}

pub fn latest_session_at(note: &str) -> Option<SessionTime> {
    headers(note).into_iter().map(|h| h.at).max()
}

pub fn latest_handoff_at(note: &str) -> Option<SessionTime> {
    headers(note)
        .into_iter()
        .filter(|h| h.kind == EntryKind::Handoff)
        .map(|h| h.at)
        .max()
}

pub fn activity_since(latest: SessionTime, activity: DateTime<FixedOffset>) -> bool {
    SessionTime::new(activity) > latest
}

pub fn append_session(note: &str, entry: &SessionEntry) -> String {
    append_to_section(note, &SectionHeading::sessions(), &entry.to_string())
}

pub fn append_breadcrumb(note: &str, crumb: &Breadcrumb) -> String {
    append_to_section(note, &SectionHeading::sessions(), &crumb.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Handoff,
    Auto,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntryHeader {
    pub at: SessionTime,
    pub kind: EntryKind,
}

impl EntryHeader {
    pub fn parse(line: &str) -> Option<Self> {
        let mut parts = line.strip_prefix(ENTRY_PREFIX)?.split(BRANCH_SEPARATOR);
        let at = SessionTime::parse_stamp(parts.next()?.trim())?;
        let kind = match parts.last().map(str::trim) {
            Some(AUTO_TAG) => EntryKind::Auto,
            _ => EntryKind::Handoff,
        };
        Some(Self { at, kind })
    }
}

fn header(at: SessionTime, branch: Option<&BranchName>, kind: EntryKind) -> String {
    let tag = match kind {
        EntryKind::Handoff => String::new(),
        EntryKind::Auto => format!("{BRANCH_SEPARATOR}{AUTO_TAG}"),
    };
    match branch {
        Some(branch) => format!("{ENTRY_PREFIX}{at}{BRANCH_SEPARATOR}{branch}{tag}"),
        None => format!("{ENTRY_PREFIX}{at}{tag}"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SessionTime(DateTime<FixedOffset>);

impl SessionTime {
    pub fn new(at: DateTime<FixedOffset>) -> Self {
        Self(
            at.with_second(0)
                .and_then(|at| at.with_nanosecond(0))
                .unwrap_or(at),
        )
    }

    pub fn now() -> Self {
        Self::new(arc_core::time::now())
    }

    pub fn get(self) -> DateTime<FixedOffset> {
        self.0
    }

    fn parse_stamp(stamp: &str) -> Option<Self> {
        OFFSET_FORMATS
            .iter()
            .find_map(|format| DateTime::parse_from_str(stamp, format).ok())
            .or_else(|| {
                NaiveDateTime::parse_from_str(stamp, LOCAL_FORMAT)
                    .ok()
                    .map(arc_core::time::assume_local)
            })
            .map(Self::new)
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
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let lines: Vec<&str> = text
            .split('\n')
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
        header(self.at, self.branch.as_ref(), EntryKind::Handoff)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OneLine(String);

impl OneLine {
    pub fn parse(text: &str) -> Option<Self> {
        let joined = text
            .split(|c: char| c.is_control())
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        (!joined.is_empty()).then_some(Self(joined))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for OneLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Breadcrumb {
    Commits {
        at: SessionTime,
        branch: Option<BranchName>,
        subjects: Vec<OneLine>,
        uncommitted: Option<OneLine>,
    },
    Uncommitted {
        at: SessionTime,
        branch: Option<BranchName>,
        summary: OneLine,
    },
}

impl Breadcrumb {
    pub fn new(
        at: SessionTime,
        branch: Option<BranchName>,
        subjects: Vec<OneLine>,
        uncommitted: Option<OneLine>,
    ) -> Option<Self> {
        match (subjects.is_empty(), uncommitted) {
            (false, uncommitted) => Some(Breadcrumb::Commits {
                at,
                branch,
                subjects,
                uncommitted,
            }),
            (true, Some(summary)) => Some(Breadcrumb::Uncommitted {
                at,
                branch,
                summary,
            }),
            (true, None) => None,
        }
    }

    pub fn header(&self) -> String {
        match self {
            Breadcrumb::Commits { at, branch, .. } | Breadcrumb::Uncommitted { at, branch, .. } => {
                header(*at, branch.as_ref(), EntryKind::Auto)
            }
        }
    }
}

impl fmt::Display for Breadcrumb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.header())?;
        let uncommitted = match self {
            Breadcrumb::Commits {
                subjects,
                uncommitted,
                ..
            } => {
                f.write_str("\n**Commits:**")?;
                subjects
                    .iter()
                    .take(SHOWN_COMMITS)
                    .try_for_each(|s| write!(f, "\n- {s}"))?;
                if let more @ 1.. = subjects.len().saturating_sub(SHOWN_COMMITS) {
                    write!(f, "\n- and {more} more")?;
                }
                uncommitted.as_ref()
            }
            Breadcrumb::Uncommitted { summary, .. } => Some(summary),
        };
        uncommitted
            .iter()
            .try_for_each(|u| write!(f, "\n**Uncommitted:** {u}"))
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
        SessionTime::new(
            DateTime::parse_from_str(&format!("{s} +10:00"), "%Y-%m-%d %H:%M:%S %:z").unwrap(),
        )
    }

    fn local(s: &str) -> SessionTime {
        SessionTime::new(arc_core::time::assume_local(
            NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap(),
        ))
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

    const FULL: &str = "### 2026-10-05 14:32 +10:00 · tenzin/sc-16727/optical-flow\n**Done:** wired the flow\n**State:** tests green\n**Next:** tune the threshold\n**Open questions:** which camera?";

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
            "### 2026-10-05 14:32 +10:00 · tenzin/sc-16727/optical-flow"
        );
        assert_eq!(
            entry("2026-10-05 09:05:00", None, None).to_string(),
            "### 2026-10-05 09:05 +10:00\n**Done:** wired the flow\n**State:** tests green\n**Next:** tune the threshold"
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
            "### 2026-10-05 14:32 +10:00\n**Done:** first\n    ## Injected\n    ### 2026-01-01 00:00 · fake\n    ```\n    # x\n      - item\n**State:** tests green\n**Next:** one line"
        );
        let note = append_session("# S\n## Sessions\n", &e);
        let note = append_session(&note, &entry("2026-10-05 15:00:00", None, None));
        let note = append_to_section(&note, &SectionHeading::parse("After").unwrap(), "x");
        let sessions = latest_sessions(&note, 5);
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0], e.to_string());
        assert_eq!(latest_session_at(&note), Some(at("2026-10-05 15:00:00")));
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
        assert_eq!(latest_session_at(&note), Some(at("2026-10-05 11:30:00")));
    }

    #[test]
    fn session_times_skip_unparseable_headers() {
        assert_eq!(latest_session_at(NOTE), None);
        assert_eq!(latest_session_at("# S\n"), None);
        let note = "## Sessions\n### 2026-10-05 14:32 · b\n### garbage\n### 2026-10-04\n### 2026-13-01 10:00\n```\n### 2026-12-01 10:00\n```\n## Later\n### 2026-12-02 10:00\n";
        assert_eq!(latest_session_at(note), Some(local("2026-10-05 14:32")));
        assert_eq!(
            latest_session_at("## Sessions\n###  2026-10-05 08:01  \n"),
            Some(local("2026-10-05 08:01"))
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
    fn context_sessions_keep_the_latest_handoff() {
        let note = "## Sessions\n### 2026-10-01 09:00 · a\n**Done:** first\n### 2026-10-02 09:00 · b\n**Done:** handoff\n### 2026-10-03 09:00 · b · [auto]\n**Commits:**\n- x\n### 2026-10-04 09:00 · [auto]\n**Uncommitted:** 1 file\n### 2026-10-05 09:00 · b · [auto]\n**Commits:**\n- y\n";
        let heads = |sessions: Vec<String>| -> Vec<String> {
            sessions
                .iter()
                .map(|s| s.lines().next().unwrap().to_owned())
                .collect()
        };
        assert_eq!(
            heads(context_sessions(note, 3)),
            [
                "### 2026-10-02 09:00 · b",
                "### 2026-10-04 09:00 · [auto]",
                "### 2026-10-05 09:00 · b · [auto]",
            ]
        );
        assert_eq!(
            heads(context_sessions(note, 1)),
            ["### 2026-10-02 09:00 · b"]
        );
        assert!(context_sessions(note, 0).is_empty());
        assert_eq!(context_sessions(note, 5), latest_sessions(note, 5));
        assert_eq!(context_sessions(NOTE, 3), latest_sessions(NOTE, 3));
        let only_auto = "## Sessions\n### 2026-10-03 09:00 · [auto]\n### 2026-10-04 09:00 · [auto]\n### 2026-10-05 09:00 · [auto]\n";
        assert_eq!(
            context_sessions(only_auto, 2),
            latest_sessions(only_auto, 2)
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

    #[test]
    fn entries_after_a_stray_fence_read_back() {
        let first = entry("2026-10-05 09:00:00", Some("a"), None);
        let second = entry("2026-10-05 10:00:00", None, Some("q"));
        let stray = "---\nid: s\n---\n# S\n```\nstray\n";
        let note = append_session(&append_session(stray, &first), &second);
        assert!(
            note.starts_with("---\nid: s\n---\n# S\n```\nstray\n```\n\n## Sessions\n\n"),
            "{note}"
        );
        assert_eq!(
            latest_sessions(&note, 5),
            vec![first.to_string(), second.to_string()]
        );

        let yaml = "---\ndesc: |\n  ````\n---\n# S\n\n## Sessions\n";
        let note = append_session(&append_session(yaml, &first), &second);
        assert_eq!(note.matches("## Sessions").count(), 1, "{note}");
        assert_eq!(
            latest_sessions(&note, 5),
            vec![first.to_string(), second.to_string()]
        );
    }

    #[test]
    fn carriage_returns_cannot_fake_entry_headings() {
        let mut e = entry("2026-10-05 14:32:00", None, None);
        e.handoff.done = text("a\r### 2026-01-01 00:00 · fake\r\rb\r\n");
        assert_eq!(e.handoff.done.as_str(), "a\n### 2026-01-01 00:00 · fake\nb");
        let note = append_session("## Sessions\n", &e);
        assert_eq!(latest_sessions(&note, 5), vec![e.to_string()]);
        assert!(!note.contains('\r'));
    }

    #[test]
    fn session_times_compare_across_offsets() {
        let note = "## Sessions\n### 2026-10-05 14:32 +10:00 · a\n### 2026-10-05 04:00 +00:00\n### 2026-10-05 01:00 -03:00\n### 2026-10-05 14:00 +1000\n";
        assert_eq!(latest_session_at(note), Some(at("2026-10-05 14:32:00")));
        let late = "## Sessions\n### 2026-10-05 14:32 +10:00\n### 2026-10-05 04:40 +00:00\n";
        assert_eq!(
            latest_session_at(late).unwrap().to_string(),
            "2026-10-05 04:40 +00:00"
        );
    }

    #[test]
    fn activity_counts_only_from_the_next_minute() {
        let latest = at("2026-10-05 14:32:00");
        let activity = |s: &str| DateTime::parse_from_rfc3339(s).unwrap();
        assert!(!activity_since(
            latest,
            activity("2026-10-05T14:32:59+10:00")
        ));
        assert!(!activity_since(latest, activity("2026-10-05T04:32:30Z")));
        assert!(!activity_since(
            latest,
            activity("2026-10-05T14:31:00+10:00")
        ));
        assert!(activity_since(
            latest,
            activity("2026-10-05T14:33:00+10:00")
        ));
        assert!(activity_since(latest, activity("2026-10-05T04:33:00Z")));
        assert!(!activity_since(
            latest,
            activity("2026-10-05T02:33:00+02:00")
        ));
        assert!(activity_since(
            latest,
            activity("2026-10-04T23:33:00-05:00")
        ));
    }

    #[test]
    fn headers_without_an_offset_are_local_time() {
        let note = "## Sessions\n### 2026-10-05 14:32 · b\n";
        assert_eq!(latest_session_at(note), Some(local("2026-10-05 14:32")));
        assert_eq!(
            latest_session_at(note)
                .unwrap()
                .get()
                .naive_local()
                .to_string(),
            "2026-10-05 14:32:00"
        );
    }

    fn line(s: &str) -> OneLine {
        OneLine::parse(s).unwrap()
    }

    #[test]
    fn one_line_flattens_control_characters() {
        assert_eq!(line(" a\r\n## b\tc ").as_str(), "a ## b c");
        assert_eq!(OneLine::parse(" \n\t"), None);
    }

    #[test]
    fn golden_breadcrumb_rendering() {
        let crumb = Breadcrumb::new(
            at("2026-10-05 17:45:10"),
            Some(BranchName::parse("tenzin/sc-7/x").unwrap()),
            vec![line("add flow"), line("### not a heading")],
            Some(line("2 files changed, 5 insertions(+); 1 untracked")),
        )
        .unwrap();
        assert_eq!(
            crumb.to_string(),
            "### 2026-10-05 17:45 +10:00 · tenzin/sc-7/x · [auto]\n**Commits:**\n- add flow\n- ### not a heading\n**Uncommitted:** 2 files changed, 5 insertions(+); 1 untracked"
        );
        let dirty = Breadcrumb::new(
            at("2026-10-05 17:45:00"),
            None,
            vec![],
            Some(line("1 file changed")),
        )
        .unwrap();
        assert_eq!(
            dirty.to_string(),
            "### 2026-10-05 17:45 +10:00 · [auto]\n**Uncommitted:** 1 file changed"
        );
        assert_eq!(
            Breadcrumb::new(at("2026-10-05 17:45:00"), None, vec![], None),
            None
        );
    }

    #[test]
    fn breadcrumbs_cap_commit_subjects() {
        let subjects = (1..=13).map(|i| line(&format!("c{i}"))).collect();
        let crumb = Breadcrumb::new(at("2026-10-05 17:45:00"), None, subjects, None).unwrap();
        let rendered = crumb.to_string();
        assert!(rendered.contains("- c10\n- and 3 more"), "{rendered}");
        assert!(!rendered.contains("c11"), "{rendered}");
    }

    #[test]
    fn breadcrumbs_read_back_as_auto_entries() {
        let handoff = entry("2026-10-05 09:00:00", Some("auto"), None);
        let crumb = Breadcrumb::new(
            at("2026-10-05 18:00:00"),
            Some(BranchName::parse("b").unwrap()),
            vec![line("- ---"), line("===")],
            None,
        )
        .unwrap();
        let note = append_breadcrumb(&append_session("# S\n", &handoff), &crumb);
        let note = append_to_section(&note, &SectionHeading::parse("After").unwrap(), "x");
        assert_eq!(
            latest_sessions(&note, 5),
            vec![handoff.to_string(), crumb.to_string()]
        );
        assert_eq!(latest_session_at(&note), Some(at("2026-10-05 18:00:00")));
        assert_eq!(latest_handoff_at(&note), Some(at("2026-10-05 09:00:00")));
        assert_eq!(
            EntryHeader::parse(&handoff.header()).map(|h| h.kind),
            Some(EntryKind::Handoff)
        );
        assert_eq!(
            EntryHeader::parse(&crumb.header()).map(|h| h.kind),
            Some(EntryKind::Auto)
        );
        assert_eq!(
            latest_handoff_at("## Sessions\n### 2026-10-05 10:00 · [auto]\n"),
            None
        );
    }
}
