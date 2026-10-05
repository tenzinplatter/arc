use std::fmt;

use anyhow::bail;

const MAX_FENCE_INDENT: usize = 3;
const MIN_FENCE_LEN: usize = 3;
const SECTION_PREFIX: &str = "## ";
const SESSIONS_TITLE: &str = "Sessions";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fence {
    marker: char,
    len: usize,
}

impl Fence {
    fn parse(line: &str) -> Option<(Self, &str)> {
        let body = line.trim_start_matches(' ');
        if line.len() - body.len() > MAX_FENCE_INDENT {
            return None;
        }
        let marker = body.chars().next().filter(|c| matches!(c, '`' | '~'))?;
        let len = body.chars().take_while(|&c| c == marker).count();
        (len >= MIN_FENCE_LEN).then_some((Self { marker, len }, &body[len..]))
    }

    fn opening(line: &str) -> Option<Self> {
        Self::parse(line)
            .filter(|(fence, info)| fence.marker != '`' || !info.contains('`'))
            .map(|(fence, _)| fence)
    }

    fn closed_by(self, line: &str) -> bool {
        Self::parse(line).is_some_and(|(fence, rest)| {
            fence.marker == self.marker && fence.len >= self.len && rest.trim().is_empty()
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Line<'a> {
    Prose(&'a str),
    Code(&'a str),
}

impl<'a> Line<'a> {
    pub(crate) fn text(self) -> &'a str {
        match self {
            Line::Prose(text) | Line::Code(text) => text,
        }
    }

    pub(crate) fn is_heading(self, pred: impl Fn(&str) -> bool) -> bool {
        match self {
            Line::Prose(text) => pred(text),
            Line::Code(_) => false,
        }
    }

    fn is_blank(self) -> bool {
        self.text().trim().is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Scanned<'a> {
    pub(crate) line: Line<'a>,
    end: usize,
}

fn without_newline(raw: &str) -> &str {
    raw.strip_suffix('\n')
        .map_or(raw, |text| text.strip_suffix('\r').unwrap_or(text))
}

pub(crate) fn classify(note: &str) -> Vec<Scanned<'_>> {
    note.split_inclusive('\n')
        .scan((None::<Fence>, 0), |(open, end), raw| {
            *end += raw.len();
            let text = without_newline(raw);
            let line = match *open {
                Some(fence) => {
                    if fence.closed_by(text) {
                        *open = None;
                    }
                    Line::Code(text)
                }
                None => match Fence::opening(text) {
                    Some(fence) => {
                        *open = Some(fence);
                        Line::Code(text)
                    }
                    None => Line::Prose(text),
                },
            };
            Some(Scanned { line, end: *end })
        })
        .collect()
}

fn ends_section(line: &str) -> bool {
    line.starts_with("# ") || line.starts_with("## ") || matches!(line.trim_end(), "#" | "##")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionHeading(String);

impl SectionHeading {
    pub fn parse(title: &str) -> anyhow::Result<Self> {
        let title = title.trim().trim_start_matches('#').trim();
        if title.is_empty() {
            bail!("empty section heading");
        }
        if title.contains(['\n', '\r']) {
            bail!("a section heading must be a single line");
        }
        Ok(Self(format!("{SECTION_PREFIX}{title}")))
    }

    pub fn sessions() -> Self {
        Self(format!("{SECTION_PREFIX}{SESSIONS_TITLE}"))
    }

    fn matches(&self, line: Line) -> bool {
        line.is_heading(|text| text.trim_end() == self.0)
    }
}

impl fmt::Display for SectionHeading {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Newline {
    Lf,
    CrLf,
}

impl Newline {
    pub fn of(note: &str) -> Self {
        match note.find('\n') {
            Some(i) if note[..i].ends_with('\r') => Newline::CrLf,
            _ => Newline::Lf,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Newline::Lf => "\n",
            Newline::CrLf => "\r\n",
        }
    }
}

impl fmt::Display for Newline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

pub(crate) fn section<'a, 'n>(
    lines: &'a [Scanned<'n>],
    heading: &SectionHeading,
) -> Option<&'a [Scanned<'n>]> {
    let start = lines.iter().position(|s| heading.matches(s.line))?;
    let rest = &lines[start..];
    let len = rest
        .iter()
        .skip(1)
        .position(|s| s.line.is_heading(ends_section))
        .map_or(rest.len(), |i| i + 1);
    Some(&rest[..len])
}

fn separator(before: &str, nl: Newline) -> String {
    if before.is_empty() {
        return String::new();
    }
    if !before.ends_with('\n') {
        return format!("{nl}{nl}");
    }
    match before.lines().next_back() {
        Some(last) if !last.trim().is_empty() => nl.to_string(),
        _ => String::new(),
    }
}

pub fn append_to_section(note: &str, heading: &SectionHeading, block: &str) -> String {
    let nl = Newline::of(note);
    let lines: Vec<&str> = block.lines().map(str::trim_end).collect();
    let start = lines
        .iter()
        .position(|l| !l.is_empty())
        .unwrap_or(lines.len());
    let end = lines
        .iter()
        .rposition(|l| !l.is_empty())
        .map_or(0, |i| i + 1);
    let block = lines[start..end.max(start)].join(nl.as_str());

    let scanned = classify(note);
    let tail = section(&scanned, heading)
        .and_then(|lines| lines.iter().rev().find(|s| !s.line.is_blank()))
        .map(|s| s.end);
    let (at, text) = match tail {
        Some(at) => (at, block),
        None => (note.len(), format!("{heading}{nl}{nl}{block}")),
    };
    let (before, after) = note.split_at(at);
    let trail = match after.lines().next() {
        Some(line) if !line.trim().is_empty() => nl.as_str(),
        _ => "",
    };
    format!("{before}{}{text}{nl}{trail}{after}", separator(before, nl))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log() -> SectionHeading {
        SectionHeading::parse("Log").unwrap()
    }

    #[test]
    fn headings_parse_from_their_text() {
        assert_eq!(log().to_string(), "## Log");
        assert_eq!(SectionHeading::parse("  ## Log ").unwrap(), log());
        assert_eq!(
            SectionHeading::sessions(),
            SectionHeading::parse("Sessions").unwrap()
        );
        assert!(SectionHeading::parse("").is_err());
        assert!(SectionHeading::parse(" ## ").is_err());
        assert!(SectionHeading::parse("a\nb").is_err());
    }

    #[test]
    fn newline_follows_the_first_line_ending() {
        assert_eq!(Newline::of("a\r\nb\n"), Newline::CrLf);
        assert_eq!(Newline::of("a\nb\r\n"), Newline::Lf);
        assert_eq!(Newline::of("no newline"), Newline::Lf);
    }

    #[test]
    fn creates_the_section_at_the_end_when_absent() {
        assert_eq!(append_to_section("", &log(), "x"), "## Log\n\nx\n");
        assert_eq!(
            append_to_section("# T\nbody", &log(), "x"),
            "# T\nbody\n\n## Log\n\nx\n"
        );
        assert_eq!(
            append_to_section("# T\nbody\n", &log(), "x"),
            "# T\nbody\n\n## Log\n\nx\n"
        );
        assert_eq!(
            append_to_section("# T\nbody\n\n", &log(), "x"),
            "# T\nbody\n\n## Log\n\nx\n"
        );
    }

    #[test]
    fn appends_after_the_last_line_of_the_section() {
        assert_eq!(
            append_to_section("## Log\n- a\n", &log(), "- b"),
            "## Log\n- a\n\n- b\n"
        );
        assert_eq!(
            append_to_section("## Log\n", &log(), "- b"),
            "## Log\n\n- b\n"
        );
        assert_eq!(
            append_to_section("## Log", &log(), "- b"),
            "## Log\n\n- b\n"
        );
        assert_eq!(
            append_to_section("## Log\n- a\n\n\n## Next\nz\n", &log(), "- b"),
            "## Log\n- a\n\n- b\n\n\n## Next\nz\n"
        );
        assert_eq!(
            append_to_section("## Log\n- a\n# Top\n", &log(), "- b"),
            "## Log\n- a\n\n- b\n\n# Top\n"
        );
        assert_eq!(
            append_to_section("## Log\n- a\n### sub\nc\n## Next\n", &log(), "\n\n- b\n\n"),
            "## Log\n- a\n### sub\nc\n\n- b\n\n## Next\n"
        );
    }

    #[test]
    fn fenced_headings_neither_match_nor_end_the_section() {
        let note = "```\n## Log\n```\n## Log\na\n```md\n## Next\n```\n";
        assert_eq!(
            append_to_section(note, &log(), "b"),
            "```\n## Log\n```\n## Log\na\n```md\n## Next\n```\n\nb\n"
        );
    }

    #[test]
    fn crlf_notes_stay_crlf() {
        let note = "# T\r\n\r\n## Log\r\na\r\n\r\n## Next\r\nz\r\n";
        assert_eq!(
            append_to_section(note, &log(), "b\nc"),
            "# T\r\n\r\n## Log\r\na\r\n\r\nb\r\nc\r\n\r\n## Next\r\nz\r\n"
        );
        assert_eq!(
            append_to_section("# T\r\n", &log(), "b"),
            "# T\r\n\r\n## Log\r\n\r\nb\r\n"
        );
    }
}
