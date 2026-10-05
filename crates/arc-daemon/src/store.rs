use std::path::Path;

use anyhow::Result;
use uuid::Uuid;

use arc_notes::NoteRelPath;
use arc_notes::scanner::ParsedTodo;
use arc_notes::todos::{Todo, TodoSource, modify_with_lock};

/// Merge note-parsed todos for `file` into the on-disk store.
///
/// - Preserves manual entries.
/// - Preserves NoteParsed entries for other files.
/// - For NoteParsed entries from `file`: matches by fingerprint to preserve
///   id + completed; otherwise generates a new Uuid.
pub async fn merge_file(
    cache_dir: &Path,
    file: &NoteRelPath,
    parsed: Vec<ParsedTodo>,
) -> Result<Vec<Todo>> {
    let file = file.clone();
    modify_with_lock(cache_dir, move |list| {
        // Parsed entries not yet matched to an existing todo. We drain matches
        // out of this as we walk the list, so whatever remains is genuinely new.
        let mut unmatched: Vec<ParsedTodo> = parsed;

        // Walk the existing list in order, preserving the position of every todo
        // so the UI doesn't reshuffle on re-scan. Entries from `file` are updated
        // in place (matched by fingerprint) or dropped if their line is gone.
        let mut next: Vec<Todo> = Vec::with_capacity(list.len() + unmatched.len());
        for todo in list.drain(..) {
            let this_file_fp = match &todo.source {
                TodoSource::NoteParsed { file: f, fingerprint, .. } if f == &file => {
                    Some(*fingerprint)
                }
                _ => None,
            };
            let Some(fp) = this_file_fp else {
                // Manual entry, or NoteParsed from another file: keep untouched.
                next.push(todo);
                continue;
            };
            if let Some(pos) = unmatched.iter().position(|p| p.fingerprint == fp) {
                let p = unmatched.remove(pos);
                next.push(Todo {
                    id: todo.id,
                    text: p.text,
                    date: p.date,
                    completed: p.completed,
                    source: TodoSource::NoteParsed {
                        file: p.file,
                        line: p.line,
                        fingerprint: p.fingerprint,
                    },
                });
            }
            // else: the line no longer exists in the note — drop it.
        }

        // Genuinely new checkboxes are appended at the end.
        for p in unmatched {
            next.push(Todo {
                id: Uuid::new_v4(),
                text: p.text,
                date: p.date,
                completed: p.completed,
                source: TodoSource::NoteParsed {
                    file: p.file,
                    line: p.line,
                    fingerprint: p.fingerprint,
                },
            });
        }

        *list = next;
    })
    .await
}

/// Drop all NoteParsed entries for `file` (used on file delete/rename-out).
pub async fn drop_file(cache_dir: &Path, file: &NoteRelPath) -> Result<Vec<Todo>> {
    let file = file.clone();
    modify_with_lock(cache_dir, move |list| {
        list.retain(|t| match &t.source {
            TodoSource::NoteParsed { file: f, .. } => f != &file,
            _ => true,
        });
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn parsed(file: &str, text: &str, fingerprint: u64) -> ParsedTodo {
        ParsedTodo {
            text: text.to_string(),
            completed: false,
            date: None,
            file: NoteRelPath::from_trusted(file.into()),
            line: 1,
            fingerprint,
        }
    }

    #[tokio::test]
    async fn merge_preserves_manual_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path();

        arc_notes::todos::save_todos(
            cache,
            &[Todo::new_manual(
                "manual one".to_string(),
                NaiveDate::from_ymd_opt(2026, 5, 31).unwrap(),
            )],
        )
        .await
        .unwrap();

        let final_list = merge_file(
            cache,
            &NoteRelPath::from_trusted("a.md".into()),
            vec![parsed("a.md", "from note", 1)],
        )
        .await
        .unwrap();

        assert_eq!(final_list.len(), 2);
        assert!(final_list.iter().any(|t| t.text == "manual one"
            && matches!(t.source, TodoSource::Manual)));
        assert!(final_list.iter().any(|t| t.text == "from note"
            && matches!(t.source, TodoSource::NoteParsed { .. })));
    }

    #[tokio::test]
    async fn merge_preserves_uuid_via_fingerprint() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path();

        // First pass.
        let first = merge_file(
            cache,
            &NoteRelPath::from_trusted("a.md".into()),
            vec![parsed("a.md", "ship it", 0xabc)],
        )
        .await
        .unwrap();
        let original_id = first[0].id;

        // Daemon rescans, same fingerprint, line moved.
        let mut p = parsed("a.md", "ship it", 0xabc);
        p.line = 42;
        let after = merge_file(cache, &NoteRelPath::from_trusted("a.md".into()), vec![p]).await.unwrap();

        assert_eq!(after.len(), 1);
        assert_eq!(after[0].id, original_id, "id preserved across rescan");
        if let TodoSource::NoteParsed { line, .. } = &after[0].source {
            assert_eq!(*line, 42, "line position updated");
        } else {
            panic!("expected NoteParsed");
        }
    }

    #[tokio::test]
    async fn merge_takes_note_as_source_of_truth_for_completion() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path();

        // First scan: note says checked.
        let mut p = parsed("a.md", "ship it", 0xabc);
        p.completed = true;
        merge_file(cache, &NoteRelPath::from_trusted("a.md".into()), vec![p]).await.unwrap();

        // Second scan: note now says unchecked. Should propagate.
        let p = parsed("a.md", "ship it", 0xabc); // completed = false by default
        let after = merge_file(cache, &NoteRelPath::from_trusted("a.md".into()), vec![p]).await.unwrap();
        assert_eq!(after.len(), 1);
        assert!(!after[0].completed, "note un-check should clear completion");
    }

    #[tokio::test]
    async fn merge_preserves_global_order_of_existing_todos() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path();

        // Two notes; list ends up [from a, from b].
        merge_file(cache, &NoteRelPath::from_trusted("a.md".into()), vec![parsed("a.md", "from a", 1)])
            .await
            .unwrap();
        merge_file(cache, &NoteRelPath::from_trusted("b.md".into()), vec![parsed("b.md", "from b", 2)])
            .await
            .unwrap();

        // Re-scan a.md (e.g. user ticked it off in the UI, daemon re-parses).
        // This must NOT reorder the list — otherwise the TUI jumps.
        let after = merge_file(cache, &NoteRelPath::from_trusted("a.md".into()), vec![parsed("a.md", "from a", 1)])
            .await
            .unwrap();

        let texts: Vec<&str> = after.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(
            texts,
            vec!["from a", "from b"],
            "re-scanning a note must preserve the position of existing todos"
        );
    }

    #[tokio::test]
    async fn merge_removes_deleted_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path();

        merge_file(
            cache,
            &NoteRelPath::from_trusted("a.md".into()),
            vec![parsed("a.md", "first", 1), parsed("a.md", "second", 2)],
        )
        .await
        .unwrap();

        let after = merge_file(cache, &NoteRelPath::from_trusted("a.md".into()), vec![parsed("a.md", "first", 1)])
            .await
            .unwrap();

        assert_eq!(after.len(), 1);
        assert_eq!(after[0].text, "first");
    }

    #[tokio::test]
    async fn merge_does_not_touch_other_files() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path();

        merge_file(cache, &NoteRelPath::from_trusted("a.md".into()), vec![parsed("a.md", "from a", 1)])
            .await
            .unwrap();
        merge_file(cache, &NoteRelPath::from_trusted("b.md".into()), vec![parsed("b.md", "from b", 2)])
            .await
            .unwrap();

        // Rescan a.md with empty parsed -> should remove a's entries only.
        let after = merge_file(cache, &NoteRelPath::from_trusted("a.md".into()), vec![]).await.unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].text, "from b");
    }

    #[tokio::test]
    async fn drop_file_removes_all_for_path() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path();

        merge_file(
            cache,
            &NoteRelPath::from_trusted("a.md".into()),
            vec![parsed("a.md", "x", 1), parsed("a.md", "y", 2)],
        )
        .await
        .unwrap();

        let after = drop_file(cache, &NoteRelPath::from_trusted("a.md".into())).await.unwrap();
        assert!(after.is_empty());
    }
}
