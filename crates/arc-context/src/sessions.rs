const SESSIONS_HEADING: &str = "## Sessions";
const ENTRY_PREFIX: &str = "### ";

fn ends_section(line: &str) -> bool {
    line.starts_with("# ") || line.starts_with("## ") || matches!(line.trim_end(), "#" | "##")
}

pub fn latest_sessions(note: &str, count: usize) -> Vec<String> {
    let mut lines = note
        .lines()
        .skip_while(|line| line.trim_end() != SESSIONS_HEADING)
        .skip(1)
        .take_while(|line| !ends_section(line))
        .skip_while(|line| !line.starts_with(ENTRY_PREFIX))
        .peekable();
    let mut entries = Vec::new();
    while let Some(heading) = lines.next() {
        let mut entry = vec![heading];
        while let Some(line) = lines.next_if(|line| !line.starts_with(ENTRY_PREFIX)) {
            entry.push(line);
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
}
