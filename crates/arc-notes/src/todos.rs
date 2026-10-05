use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::checkbox::{Checkbox, split_checkbox};
use crate::path::NoteRelPath;

use anyhow::Context;
use chrono::NaiveDate;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Todo {
    pub id: Uuid,
    pub text: String,
    pub date: Option<NaiveDate>,
    pub completed: bool,
    #[serde(default)]
    pub source: TodoSource,
}

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum TodoSource {
    #[default]
    Manual,
    NoteParsed {
        file: NoteRelPath,
        line: u32,
        fingerprint: u64,
    },
}

impl Todo {
    pub fn new_manual(text: String, date: NaiveDate) -> Self {
        Self {
            id: Uuid::new_v4(),
            text,
            date: Some(date),
            completed: false,
            source: TodoSource::Manual,
        }
    }
}

const TODOS_FILE: &str = "todos.json";
const TODOS_LOCK_FILE: &str = "todos.json.lock";

fn lock_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join(TODOS_LOCK_FILE)
}

fn todos_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join(TODOS_FILE)
}

fn open_lock(cache_dir: &Path) -> anyhow::Result<std::fs::File> {
    if !cache_dir.exists() {
        std::fs::create_dir_all(cache_dir)?;
    }
    let path = lock_path(cache_dir);
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)?;
    Ok(file)
}

fn read_todos_inner(cache_dir: &Path) -> Vec<Todo> {
    let path = todos_path(cache_dir);
    let Ok(mut f) = std::fs::File::open(&path) else {
        return Vec::new();
    };
    let mut buf = String::new();
    if f.read_to_string(&mut buf).is_err() {
        return Vec::new();
    }
    serde_json::from_str(&buf).unwrap_or_default()
}

fn write_todos_inner(cache_dir: &Path, todos: &[Todo]) -> anyhow::Result<()> {
    std::fs::create_dir_all(cache_dir)?;
    let path = todos_path(cache_dir);
    let mut tmp = tempfile::NamedTempFile::new_in(cache_dir)?;
    let content = serde_json::to_string_pretty(todos)?;
    tmp.write_all(content.as_bytes())?;
    tmp.persist(&path)?;
    Ok(())
}

pub async fn load_todos(cache_dir: &Path) -> Vec<Todo> {
    let cache_dir = cache_dir.to_path_buf();
    tokio::task::spawn_blocking(move || read_todos_inner(&cache_dir))
        .await
        .unwrap_or_default()
}

pub async fn save_todos(cache_dir: &Path, todos: &[Todo]) -> anyhow::Result<()> {
    let cache_dir = cache_dir.to_path_buf();
    let todos = todos.to_vec();
    tokio::task::spawn_blocking(move || {
        let lock = open_lock(&cache_dir)?;
        FileExt::lock_exclusive(&lock)?;
        let result = write_todos_inner(&cache_dir, &todos);
        let _ = FileExt::unlock(&lock);
        result
    })
    .await?
}

pub async fn modify_with_lock<F>(cache_dir: &Path, f: F) -> anyhow::Result<Vec<Todo>>
where
    F: FnOnce(&mut Vec<Todo>) + Send + 'static,
{
    let cache_dir = cache_dir.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let lock = open_lock(&cache_dir)?;
        FileExt::lock_exclusive(&lock)?;
        let result = (|| {
            let mut list = read_todos_inner(&cache_dir);
            f(&mut list);
            write_todos_inner(&cache_dir, &list)?;
            Ok::<_, anyhow::Error>(list)
        })();
        let _ = FileExt::unlock(&lock);
        result
    })
    .await?
}

fn rewrite_checkbox_line(content: &str, line_1based: u32, state: Checkbox) -> Option<String> {
    if line_1based == 0 {
        return None;
    }
    let target_idx = (line_1based - 1) as usize;
    let mut out = String::with_capacity(content.len());
    let mut found = false;
    for (idx, segment) in content.split_inclusive('\n').enumerate() {
        if idx == target_idx {
            let rewritten = rewrite_checkbox_in_segment(segment, state)?;
            out.push_str(&rewritten);
            found = true;
        } else {
            out.push_str(segment);
        }
    }
    found.then_some(out)
}

fn rewrite_checkbox_in_segment(segment: &str, state: Checkbox) -> Option<String> {
    let rest = segment.trim_start();
    let leading = &segment[..segment.len() - rest.len()];
    let (_, body) = split_checkbox(rest)?;
    Some(format!("{leading}{}{body}", state.marker()))
}

fn normalize_for_match(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn toggle_note_checkbox_blocking(
    notes_dir: &Path,
    relative_file: &NoteRelPath,
    todo_text: &str,
    state: Checkbox,
) -> anyhow::Result<()> {
    let path = relative_file.abs(notes_dir);
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("read {}", path.display()))?;

    let parsed = crate::scanner::parse_note(&content, relative_file);
    let needle = normalize_for_match(todo_text);
    let target = parsed
        .iter()
        .find(|p| normalize_for_match(&p.text) == needle)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no checkbox with matching text remains in {}",
                path.display()
            )
        })?;

    let new_content = rewrite_checkbox_line(&content, target.line, state)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "could not rewrite checkbox at line {} of {}",
                target.line,
                path.display()
            )
        })?;

    if new_content == content {
        return Ok(());
    }

    let dir = path.parent().unwrap_or(notes_dir);
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(new_content.as_bytes())?;
    tmp.persist(&path)?;
    Ok(())
}

pub async fn toggle_note_checkbox(
    notes_dir: &Path,
    relative_file: &NoteRelPath,
    todo_text: &str,
    state: Checkbox,
) -> anyhow::Result<()> {
    let notes_dir = notes_dir.to_path_buf();
    let relative_file = relative_file.clone();
    let todo_text = todo_text.to_string();
    tokio::task::spawn_blocking(move || {
        toggle_note_checkbox_blocking(&notes_dir, &relative_file, &todo_text, state)
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_legacy_todo_without_source_field() {
        let json = r#"{
            "id": "00000000-0000-0000-0000-000000000001",
            "text": "legacy",
            "date": "2026-05-31",
            "completed": false
        }"#;
        let todo: Todo = serde_json::from_str(json).unwrap();
        assert_eq!(todo.text, "legacy");
        assert_eq!(todo.date, Some(chrono::NaiveDate::from_ymd_opt(2026, 5, 31).unwrap()));
        assert!(matches!(todo.source, TodoSource::Manual));
    }

    #[test]
    fn deserializes_todo_without_date() {
        let json = r#"{
            "id": "00000000-0000-0000-0000-000000000002",
            "text": "no date",
            "date": null,
            "completed": false
        }"#;
        let todo: Todo = serde_json::from_str(json).unwrap();
        assert_eq!(todo.date, None);
    }

    #[test]
    fn round_trips_note_parsed_source() {
        let todo = Todo {
            id: uuid::Uuid::nil(),
            text: "scan".to_string(),
            date: None,
            completed: false,
            source: TodoSource::NoteParsed {
                file: NoteRelPath::parse("alpha.md").unwrap(),
                line: 12,
                fingerprint: 0xdead_beef,
            },
        };
        let json = serde_json::to_string(&todo).unwrap();
        let back: Todo = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            back.source,
            TodoSource::NoteParsed { line: 12, fingerprint: 0xdead_beef, .. }
        ));
    }

    #[test]
    fn concurrent_writes_do_not_lose_updates() {
        use std::thread;

        let tmp = tempfile::tempdir().unwrap();
        let cache_dir = tmp.path().to_path_buf();
        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(save_todos(&cache_dir, &[])).unwrap();

        let a = cache_dir.clone();
        let b = cache_dir.clone();

        let h1 = thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            for i in 0..20 {
                rt.block_on(modify_with_lock(&a, move |list| {
                    list.push(Todo::new_manual(
                        format!("a{}", i),
                        chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
                    ));
                })).unwrap();
            }
        });
        let h2 = thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            for i in 0..20 {
                rt.block_on(modify_with_lock(&b, move |list| {
                    list.push(Todo::new_manual(
                        format!("b{}", i),
                        chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
                    ));
                })).unwrap();
            }
        });
        h1.join().unwrap();
        h2.join().unwrap();

        let final_todos = rt.block_on(load_todos(&cache_dir));
        assert_eq!(final_todos.len(), 40, "lost updates detected");
    }

    #[test]
    fn rewrite_checkbox_line_marks_open_complete() {
        let content = "# h\n- [ ] todo\nbody\n";
        let out = rewrite_checkbox_line(content, 2, Checkbox::Done).unwrap();
        assert_eq!(out, "# h\n- [x] todo\nbody\n");
    }

    #[test]
    fn rewrite_checkbox_line_marks_complete_open() {
        let content = "- [x] done\n";
        let out = rewrite_checkbox_line(content, 1, Checkbox::Open).unwrap();
        assert_eq!(out, "- [ ] done\n");
    }

    #[test]
    fn rewrite_checkbox_line_preserves_indentation_and_capital_x() {
        let content = "    - [X] indented\n";
        let out = rewrite_checkbox_line(content, 1, Checkbox::Open).unwrap();
        assert_eq!(out, "    - [ ] indented\n");
    }

    #[test]
    fn rewrite_checkbox_line_keeps_trailing_newline_absence() {
        let content = "- [ ] x";
        let out = rewrite_checkbox_line(content, 1, Checkbox::Done).unwrap();
        assert_eq!(out, "- [x] x");
    }

    #[test]
    fn rewrite_checkbox_line_returns_none_when_not_a_checkbox() {
        let content = "just a header\nnot a checkbox\n";
        assert!(rewrite_checkbox_line(content, 2, Checkbox::Done).is_none());
    }

    #[test]
    fn toggle_note_checkbox_finds_line_by_text_even_after_move() {
        let tmp = tempfile::tempdir().unwrap();
        let rel = NoteRelPath::parse("note.md").unwrap();
        let path = tmp.path().join(&rel);

        std::fs::write(&path, "# header\n\n- [ ] ship it\n").unwrap();

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(toggle_note_checkbox(tmp.path(), &rel, "ship it", Checkbox::Done))
            .unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after, "# header\n\n- [x] ship it\n");
    }

    #[test]
    fn toggle_note_checkbox_matches_case_and_whitespace_insensitively() {
        let tmp = tempfile::tempdir().unwrap();
        let rel = NoteRelPath::parse("note.md").unwrap();
        let path = tmp.path().join(&rel);
        std::fs::write(&path, "- [ ] Ship It\n").unwrap();

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(toggle_note_checkbox(tmp.path(), &rel, "  ship   it  ", Checkbox::Done))
            .unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after, "- [x] Ship It\n");
    }

    #[test]
    fn toggle_note_checkbox_noop_when_already_in_state() {
        let tmp = tempfile::tempdir().unwrap();
        let rel = NoteRelPath::parse("note.md").unwrap();
        let path = tmp.path().join(&rel);
        std::fs::write(&path, "- [x] done\n").unwrap();

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(toggle_note_checkbox(tmp.path(), &rel, "done", Checkbox::Done))
            .unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after, "- [x] done\n");
    }

    #[test]
    fn toggle_note_checkbox_errors_when_text_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let rel = NoteRelPath::parse("note.md").unwrap();
        let path = tmp.path().join(&rel);
        std::fs::write(&path, "- [ ] something else\n").unwrap();

        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt
            .block_on(toggle_note_checkbox(tmp.path(), &rel, "nonexistent", Checkbox::Done))
            .unwrap_err();
        assert!(err.to_string().contains("no checkbox"));
    }

    const GOLDEN_TODOS_JSON: &str = r#"[
  {
    "id": "00000000-0000-0000-0000-000000000001",
    "text": "manual todo",
    "date": "2026-05-31",
    "completed": false,
    "source": {
      "kind": "Manual"
    }
  },
  {
    "id": "00000000-0000-0000-0000-000000000002",
    "text": "parsed todo",
    "date": null,
    "completed": true,
    "source": {
      "kind": "NoteParsed",
      "file": "stories/a.md",
      "line": 12,
      "fingerprint": 18446744073709551615
    }
  }
]"#;

    fn golden_todos() -> Vec<Todo> {
        vec![
            Todo {
                id: Uuid::from_u128(1),
                text: "manual todo".into(),
                date: Some(NaiveDate::from_ymd_opt(2026, 5, 31).unwrap()),
                completed: false,
                source: TodoSource::Manual,
            },
            Todo {
                id: Uuid::from_u128(2),
                text: "parsed todo".into(),
                date: None,
                completed: true,
                source: TodoSource::NoteParsed {
                    file: NoteRelPath::parse("stories/a.md").unwrap(),
                    line: 12,
                    fingerprint: u64::MAX,
                },
            },
        ]
    }

    #[test]
    fn golden_todos_json_format_is_stable() {
        assert_eq!(serde_json::to_string_pretty(&golden_todos()).unwrap(), GOLDEN_TODOS_JSON);
    }

    #[test]
    fn golden_todos_json_literal_deserializes() {
        let todos: Vec<Todo> = serde_json::from_str(GOLDEN_TODOS_JSON).unwrap();
        assert_eq!(todos, golden_todos());
    }
}
