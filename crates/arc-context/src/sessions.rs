const SESSIONS_HEADING: &str = "## Sessions";
const ENTRY_PREFIX: &str = "### ";
const MAX_FENCE_INDENT: usize = 3;
const MIN_FENCE_LEN: usize = 3;

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
enum Line<'a> {
    Prose(&'a str),
    Code(&'a str),
}

impl<'a> Line<'a> {
    fn text(self) -> &'a str {
        match self {
            Line::Prose(text) | Line::Code(text) => text,
        }
    }

    fn is_heading(self, pred: impl Fn(&str) -> bool) -> bool {
        match self {
            Line::Prose(text) => pred(text),
            Line::Code(_) => false,
        }
    }
}

fn classify(note: &str) -> Vec<Line<'_>> {
    note.lines()
        .scan(None::<Fence>, |open, text| {
            Some(match *open {
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
            })
        })
        .collect()
}

fn ends_section(line: &str) -> bool {
    line.starts_with("# ") || line.starts_with("## ") || matches!(line.trim_end(), "#" | "##")
}

fn starts_entry(line: &str) -> bool {
    line.starts_with(ENTRY_PREFIX)
}

pub fn latest_sessions(note: &str, count: usize) -> Vec<String> {
    let mut lines = classify(note)
        .into_iter()
        .skip_while(|line| !line.is_heading(|text| text.trim_end() == SESSIONS_HEADING))
        .skip(1)
        .take_while(|line| !line.is_heading(ends_section))
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
    let skip = entries.len().saturating_sub(count);
    entries.split_off(skip)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTE: &str = "---\nid: s\n---\n# Story\n\n## Sessions\n\nstray intro\n\n### 2026-10-01 · one\n**Done:** a\n\n### 2026-10-02 · two\n**Done:** b\n**Next:** c\n\n### 2026-10-03 · three\n**Done:** d\n### 2026-10-04 · four\n**Done:** e\n\n\n## Later\n### not a session\n";

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
