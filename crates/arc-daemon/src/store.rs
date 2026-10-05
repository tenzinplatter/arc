use std::path::Path;

use anyhow::Result;
use uuid::Uuid;

use arc_notes::NoteRelPath;
use arc_notes::scanner::ParsedTodo;
use arc_notes::todos::{Todo, TodoSource, transform_with_lock};

pub async fn merge_file(
    cache_dir: &Path,
    file: &NoteRelPath,
    parsed: Vec<ParsedTodo>,
) -> Result<Vec<Todo>> {
    let file = file.clone();
    transform_with_lock(cache_dir, move |list| merge(list, &file, parsed)).await
}

fn merge(list: Vec<Todo>, file: &NoteRelPath, mut unmatched: Vec<ParsedTodo>) -> Vec<Todo> {
    let kept: Vec<Todo> = list
        .into_iter()
        .filter_map(|todo| match fingerprint_from(&todo, file) {
            None => Some(todo),
            Some(fp) => unmatched
                .iter()
                .position(|p| p.fingerprint == fp)
                .map(|pos| unmatched.remove(pos).into_todo(todo.id)),
        })
        .collect();
    kept.into_iter()
        .chain(unmatched.into_iter().map(|p| p.into_todo(Uuid::new_v4())))
        .collect()
}

fn fingerprint_from(todo: &Todo, file: &NoteRelPath) -> Option<u64> {
    match &todo.source {
        TodoSource::NoteParsed { file: f, fingerprint, .. } if f == file => Some(*fingerprint),
        _ => None,
    }
}

pub async fn drop_file(cache_dir: &Path, file: &NoteRelPath) -> Result<Vec<Todo>> {
    let file = file.clone();
    transform_with_lock(cache_dir, move |list| {
        list.into_iter()
            .filter(|t| match &t.source {
                TodoSource::NoteParsed { file: f, .. } => f != &file,
                _ => true,
            })
            .collect()
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
            file: NoteRelPath::parse(file).unwrap(),
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
            &NoteRelPath::parse("a.md").unwrap(),
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

        let first = merge_file(
            cache,
            &NoteRelPath::parse("a.md").unwrap(),
            vec![parsed("a.md", "ship it", 0xabc)],
        )
        .await
        .unwrap();
        let original_id = first[0].id;

        let mut p = parsed("a.md", "ship it", 0xabc);
        p.line = 42;
        let after = merge_file(cache, &NoteRelPath::parse("a.md").unwrap(), vec![p]).await.unwrap();

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

        let mut p = parsed("a.md", "ship it", 0xabc);
        p.completed = true;
        merge_file(cache, &NoteRelPath::parse("a.md").unwrap(), vec![p]).await.unwrap();

        let p = parsed("a.md", "ship it", 0xabc);
        let after = merge_file(cache, &NoteRelPath::parse("a.md").unwrap(), vec![p]).await.unwrap();
        assert_eq!(after.len(), 1);
        assert!(!after[0].completed, "note un-check should clear completion");
    }

    #[tokio::test]
    async fn merge_preserves_global_order_of_existing_todos() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path();

        merge_file(cache, &NoteRelPath::parse("a.md").unwrap(), vec![parsed("a.md", "from a", 1)])
            .await
            .unwrap();
        merge_file(cache, &NoteRelPath::parse("b.md").unwrap(), vec![parsed("b.md", "from b", 2)])
            .await
            .unwrap();

        let after = merge_file(cache, &NoteRelPath::parse("a.md").unwrap(), vec![parsed("a.md", "from a", 1)])
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
            &NoteRelPath::parse("a.md").unwrap(),
            vec![parsed("a.md", "first", 1), parsed("a.md", "second", 2)],
        )
        .await
        .unwrap();

        let after = merge_file(cache, &NoteRelPath::parse("a.md").unwrap(), vec![parsed("a.md", "first", 1)])
            .await
            .unwrap();

        assert_eq!(after.len(), 1);
        assert_eq!(after[0].text, "first");
    }

    #[tokio::test]
    async fn merge_does_not_touch_other_files() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path();

        merge_file(cache, &NoteRelPath::parse("a.md").unwrap(), vec![parsed("a.md", "from a", 1)])
            .await
            .unwrap();
        merge_file(cache, &NoteRelPath::parse("b.md").unwrap(), vec![parsed("b.md", "from b", 2)])
            .await
            .unwrap();

        let after = merge_file(cache, &NoteRelPath::parse("a.md").unwrap(), vec![]).await.unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].text, "from b");
    }

    #[tokio::test]
    async fn drop_file_removes_all_for_path() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path();

        merge_file(
            cache,
            &NoteRelPath::parse("a.md").unwrap(),
            vec![parsed("a.md", "x", 1), parsed("a.md", "y", 2)],
        )
        .await
        .unwrap();

        let after = drop_file(cache, &NoteRelPath::parse("a.md").unwrap()).await.unwrap();
        assert!(after.is_empty());
    }
}
