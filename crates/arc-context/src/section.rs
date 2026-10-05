use std::fmt;

use anyhow::bail;
use arc_notes::SplitNote;

const MAX_INDENT: usize = 3;
const MIN_FENCE_LEN: usize = 3;
const MAX_ATX_LEVEL: usize = 6;
const SECTION_LEVEL: usize = 2;
const SESSIONS_TITLE: &str = "Sessions";
const BOM: char = '\u{feff}';

fn unindent(line: &str) -> Option<&str> {
    let body = line.trim_start_matches(' ');
    (line.len() - body.len() <= MAX_INDENT).then_some(body)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fence {
    marker: char,
    len: usize,
}

impl Fence {
    fn parse(line: &str) -> Option<(Self, &str)> {
        let body = unindent(line)?;
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

impl fmt::Display for Fence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (0..self.len).try_for_each(|_| write!(f, "{}", self.marker))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Atx<'a> {
    level: usize,
    text: &'a str,
}

impl<'a> Atx<'a> {
    fn parse(line: &'a str) -> Option<Self> {
        let body = unindent(line)?;
        let level = body.chars().take_while(|&c| c == '#').count();
        if !(1..=MAX_ATX_LEVEL).contains(&level) {
            return None;
        }
        let rest = &body[level..];
        if !(rest.is_empty() || rest.starts_with([' ', '\t'])) {
            return None;
        }
        Some(Self {
            level,
            text: without_closing_hashes(rest.trim_matches([' ', '\t'])),
        })
    }
}

fn without_closing_hashes(text: &str) -> &str {
    let open = text.trim_end_matches('#');
    match open.is_empty() || open.ends_with([' ', '\t']) {
        true => open.trim_end_matches([' ', '\t']),
        false => text,
    }
}

fn setext_level(line: &str) -> Option<usize> {
    let body = unindent(line)?.trim_end_matches([' ', '\t']);
    let marker = body.chars().next()?;
    let level = match marker {
        '=' => 1,
        '-' => 2,
        _ => return None,
    };
    body.chars().all(|c| c == marker).then_some(level)
}

fn thematic_break(line: &str) -> bool {
    let Some(body) = unindent(line) else {
        return false;
    };
    let marks: Vec<char> = body.chars().filter(|c| !matches!(c, ' ' | '\t')).collect();
    marks.len() >= 3 && matches!(marks[0], '-' | '*' | '_') && marks.iter().all(|&c| c == marks[0])
}

fn opens_block(line: &str) -> bool {
    let body = line.trim_start();
    let ordered = body.trim_start_matches(|c: char| c.is_ascii_digit());
    let bullet = |rest: &str| rest.is_empty() || rest.starts_with([' ', '\t']);
    body.starts_with('>')
        || line.contains('|')
        || body.strip_prefix(['-', '*', '+']).is_some_and(bullet)
        || (ordered.len() < body.len() && ordered.strip_prefix(['.', ')']).is_some_and(bullet))
}

fn starts_paragraph(line: &str) -> bool {
    unindent(line).is_some_and(|body| !body.starts_with(['<', '[']) && !opens_block(body))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Line<'a> {
    Frontmatter(&'a str),
    Prose(&'a str),
    Code(&'a str),
}

impl<'a> Line<'a> {
    pub(crate) fn text(self) -> &'a str {
        match self {
            Line::Frontmatter(text) | Line::Prose(text) | Line::Code(text) => text,
        }
    }

    pub(crate) fn is_heading(self, pred: impl Fn(&str) -> bool) -> bool {
        match self {
            Line::Prose(text) => pred(text),
            Line::Frontmatter(_) | Line::Code(_) => false,
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
    open: Option<Fence>,
}

pub(crate) struct Scan<'a> {
    lines: Vec<Scanned<'a>>,
    open: Option<Fence>,
}

fn without_newline(raw: &str) -> &str {
    raw.strip_suffix('\n')
        .map_or(raw, |text| text.strip_suffix('\r').unwrap_or(text))
}

fn scan(text: &str, body_start: usize) -> Scan<'_> {
    let mut open = None::<Fence>;
    let mut end = 0;
    let mut lines = Vec::new();
    for raw in text.split_inclusive('\n') {
        let start = end;
        end += raw.len();
        let text = without_newline(raw);
        let line = if end <= body_start {
            Line::Frontmatter(text)
        } else {
            let text = match start {
                0 => text.strip_prefix(BOM).unwrap_or(text),
                _ => text,
            };
            match open {
                Some(fence) => {
                    if fence.closed_by(text) {
                        open = None;
                    }
                    Line::Code(text)
                }
                None => match Fence::opening(text) {
                    Some(fence) => {
                        open = Some(fence);
                        Line::Code(text)
                    }
                    None => Line::Prose(text),
                },
            }
        };
        lines.push(Scanned { line, end, open });
    }
    Scan { lines, open }
}

pub(crate) fn classify(note: &str) -> Scan<'_> {
    let body_start = SplitNote::new(note).map_or(0, |split| note.len() - split.body().len());
    scan(note, body_start)
}

fn heading_levels(lines: &[Scanned]) -> Vec<Option<usize>> {
    let mut levels = vec![None; lines.len()];
    let mut paragraph: Option<(usize, bool)> = None;
    for (i, scanned) in lines.iter().enumerate() {
        let Line::Prose(text) = scanned.line else {
            paragraph = None;
            continue;
        };
        if text.trim().is_empty() {
            paragraph = None;
            continue;
        }
        if let Some(atx) = Atx::parse(text) {
            levels[i] = Some(atx.level);
            paragraph = None;
            continue;
        }
        paragraph = match (paragraph, setext_level(text)) {
            (Some((start, true)), Some(level)) => {
                levels[start] = Some(level);
                None
            }
            _ if thematic_break(text) => None,
            (Some((start, setext)), _) => Some((start, setext && !opens_block(text))),
            (None, _) => Some((i, starts_paragraph(text))),
        };
    }
    levels
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionHeading(String);

impl SectionHeading {
    pub fn parse(title: &str) -> anyhow::Result<Self> {
        if title.contains(['\n', '\r']) {
            bail!("a section heading must be a single line");
        }
        let title = without_closing_hashes(title.trim().trim_start_matches('#').trim());
        if title.is_empty() {
            bail!("empty section heading");
        }
        Ok(Self(title.to_owned()))
    }

    pub fn sessions() -> Self {
        Self(SESSIONS_TITLE.to_owned())
    }

    fn matches(&self, line: Line) -> bool {
        line.is_heading(|text| {
            Atx::parse(text).is_some_and(|atx| {
                atx.level == SECTION_LEVEL && atx.text.to_lowercase() == self.0.to_lowercase()
            })
        })
    }
}

impl fmt::Display for SectionHeading {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", "#".repeat(SECTION_LEVEL), self.0)
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
    scan: &'a Scan<'n>,
    heading: &SectionHeading,
) -> Option<&'a [Scanned<'n>]> {
    let start = scan.lines.iter().position(|s| heading.matches(s.line))?;
    let end = heading_levels(&scan.lines)
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, level)| level.is_some_and(|level| level <= SECTION_LEVEL))
        .map_or(scan.lines.len(), |(i, _)| i);
    Some(&scan.lines[start..end])
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

fn closing(before: &str, open: Option<Fence>, nl: Newline) -> String {
    let Some(fence) = open else {
        return before.to_owned();
    };
    match before.is_empty() || before.ends_with('\n') {
        true => format!("{before}{fence}{nl}"),
        false => format!("{before}{nl}{fence}{nl}"),
    }
}

fn closed_block(block: &str, nl: Newline) -> String {
    match scan(block, 0).open {
        Some(fence) => format!("{block}{nl}{fence}"),
        None => block.to_owned(),
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
    let block = closed_block(&lines[start..end.max(start)].join(nl.as_str()), nl);

    let scanned = classify(note);
    let tail = section(&scanned, heading)
        .and_then(|lines| lines.iter().rev().find(|s| !s.line.is_blank()));
    let (at, open, text) = match tail {
        Some(s) => (s.end, s.open, block),
        None => (
            note.len(),
            scanned.open,
            format!("{heading}{nl}{nl}{block}"),
        ),
    };
    let (before, after) = note.split_at(at);
    let before = closing(before, open, nl);
    let trail = match after.lines().next() {
        Some(line) if !line.trim().is_empty() => nl.as_str(),
        _ => "",
    };
    format!("{before}{}{text}{nl}{trail}{after}", separator(&before, nl))
}

pub fn append_to_end(note: &str, block: &str) -> String {
    let nl = Newline::of(note);
    let before = closing(note, classify(note).open, nl);
    let separator = match before.ends_with('\n') {
        true => nl.to_string(),
        false => format!("{nl}{nl}"),
    };
    let block = closed_block(block.trim_end(), nl);
    format!("{before}{separator}{block}{nl}")
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

    #[test]
    fn frontmatter_lines_are_never_fences_or_headings() {
        let note = "---\ndesc: |\n  ````\n## Log\n---\n# T\n## Log\na\n";
        assert_eq!(
            append_to_section(note, &log(), "b"),
            "---\ndesc: |\n  ````\n## Log\n---\n# T\n## Log\na\n\nb\n"
        );
        let unclosed = "---\nid: a\n## Log\n";
        assert_eq!(
            append_to_section(unclosed, &log(), "b"),
            format!("{unclosed}\nb\n")
        );
    }

    #[test]
    fn a_stray_open_fence_is_closed_before_a_new_section() {
        assert_eq!(
            append_to_section("# T\n```\ncode\n", &log(), "x"),
            "# T\n```\ncode\n```\n\n## Log\n\nx\n"
        );
        assert_eq!(
            append_to_section("# T\n````\n```\ncode", &log(), "x"),
            "# T\n````\n```\ncode\n````\n\n## Log\n\nx\n"
        );
        assert_eq!(
            append_to_section("~~~\r\ncode\r\n", &log(), "x"),
            "~~~\r\ncode\r\n~~~\r\n\r\n## Log\r\n\r\nx\r\n"
        );
        let once = append_to_section("# T\n```\ncode\n", &log(), "x");
        assert_eq!(
            append_to_section(&once, &log(), "y"),
            "# T\n```\ncode\n```\n\n## Log\n\nx\n\ny\n"
        );
    }

    #[test]
    fn a_section_ending_in_an_open_fence_is_closed_before_the_block() {
        assert_eq!(
            append_to_section("## Log\n```\nopen\n\n", &log(), "b"),
            "## Log\n```\nopen\n```\n\nb\n\n"
        );
    }

    #[test]
    fn appended_blocks_never_leave_a_fence_open() {
        let note = append_to_section("# T\n", &log(), "```sh\nls\n");
        assert_eq!(note, "# T\n\n## Log\n\n```sh\nls\n```\n");
        assert_eq!(
            append_to_section(&note, &log(), "y"),
            "# T\n\n## Log\n\n```sh\nls\n```\n\ny\n"
        );
        assert_eq!(
            append_to_end("a\n~~~~\nx\n", "```\ny\n\n"),
            "a\n~~~~\nx\n~~~~\n\n```\ny\n```\n"
        );
        assert_eq!(append_to_end("a", "b"), "a\n\nb\n");
        assert_eq!(append_to_end("a\n", "b\n\n"), "a\n\nb\n");
    }

    #[test]
    fn heading_variants_match_the_section() {
        for note in [
            "   ## Log\na\n",
            "## log ##\na\n",
            "##\tLog\na\n",
            "\u{feff}## Log\na\n",
            "## LOG  \na\n",
        ] {
            assert_eq!(append_to_section(note, &log(), "b"), format!("{note}\nb\n"));
        }
        for note in [
            "    ## Log\na\n",
            "##Log\na\n",
            "### Log\na\n",
            "## Log#\na\n",
        ] {
            assert_eq!(
                append_to_section(note, &log(), "b"),
                format!("{note}\n## Log\n\nb\n"),
                "{note:?}"
            );
        }
        assert_eq!(SectionHeading::parse("Log ##").unwrap(), log());
    }

    #[test]
    fn indented_and_closed_headings_end_the_section() {
        for next in ["  # Top", "   ## Next ##", "##", "#\tTop"] {
            let note = format!("## Log\na\n{next}\nz\n");
            assert_eq!(
                append_to_section(&note, &log(), "b"),
                format!("## Log\na\n\nb\n\n{next}\nz\n"),
                "{next:?}"
            );
        }
        let note = "## Log\na\n    ## code\n### sub\n#hashtag\n";
        assert_eq!(append_to_section(note, &log(), "b"), format!("{note}\nb\n"));
    }

    #[test]
    fn setext_headings_end_the_section() {
        assert_eq!(
            append_to_section("## Log\na\n\nTitle\n=====\nz\n", &log(), "b"),
            "## Log\na\n\nb\n\nTitle\n=====\nz\n"
        );
        assert_eq!(
            append_to_section("## Log\na\n\nTwo\nlines\n  ---  \n", &log(), "b"),
            "## Log\na\n\nb\n\nTwo\nlines\n  ---  \n"
        );
        assert_eq!(
            append_to_section("## Log\na\n---\n", &log(), "b"),
            "## Log\n\nb\n\na\n---\n"
        );
    }

    #[test]
    fn breaks_and_containers_are_not_setext_headings() {
        for body in [
            "a\n\n---\nc",
            "- item\n---\nc",
            "> quote\n---\nc",
            "1. item\n---\nc",
            "a\n***\nc",
            "a | b\n---\nc",
            "```\na\n```\n---\nc",
            "### entry\n---\nc",
            "    code\n---\nc",
            "- a\n===\nc",
        ] {
            let note = format!("## Log\n{body}\n");
            assert_eq!(
                append_to_section(&note, &log(), "z"),
                format!("{note}\nz\n"),
                "{body:?}"
            );
        }
    }
}
