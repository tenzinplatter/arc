use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::checkbox::{Checkbox, split_checkbox};
use crate::note::frontmatter::Frontmatter;
use crate::path::NoteRelPath;
use crate::scanner::{parse_note, render_todo_line};
use crate::write::{self, NoteLocks};

use anyhow::{Context, anyhow, bail};
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
    pub file: NoteRelPath,
    pub line: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edited {
    pub todo: Todo,
    pub todos: Vec<Todo>,
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
    std::fs::read_to_string(todos_path(cache_dir))
        .ok()
        .and_then(|buf| serde_json::from_str(&buf).ok())
        .unwrap_or_default()
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

fn transform_blocking<F>(cache_dir: &Path, f: F) -> anyhow::Result<Vec<Todo>>
where
    F: FnOnce(Vec<Todo>) -> Vec<Todo>,
{
    let lock = open_lock(cache_dir)?;
    FileExt::lock_exclusive(&lock)?;
    let result = (|| {
        let list = f(read_todos_inner(cache_dir));
        write_todos_inner(cache_dir, &list)?;
        Ok::<_, anyhow::Error>(list)
    })();
    let _ = FileExt::unlock(&lock);
    result
}

pub async fn load_todos(cache_dir: &Path) -> Vec<Todo> {
    let cache_dir = cache_dir.to_path_buf();
    tokio::task::spawn_blocking(move || read_todos_inner(&cache_dir))
        .await
        .unwrap_or_default()
}

pub async fn save_todos(cache_dir: &Path, todos: &[Todo]) -> anyhow::Result<()> {
    let todos = todos.to_vec();
    transform_with_lock(cache_dir, move |_| todos).await.map(drop)
}

pub async fn transform_with_lock<F>(cache_dir: &Path, f: F) -> anyhow::Result<Vec<Todo>>
where
    F: FnOnce(Vec<Todo>) -> Vec<Todo> + Send + 'static,
{
    let cache_dir = cache_dir.to_path_buf();
    tokio::task::spawn_blocking(move || transform_blocking(&cache_dir, f)).await?
}

fn with_note_replaced(list: Vec<Todo>, file: &NoteRelPath, parsed: Vec<Todo>) -> Vec<Todo> {
    let at = list.iter().position(|t| &t.file == file).unwrap_or(list.len());
    let mut others: Vec<Todo> = list.into_iter().filter(|t| &t.file != file).collect();
    others.splice(at..at, parsed);
    others
}

pub async fn refresh_note(
    cache_dir: &Path,
    file: &NoteRelPath,
    content: Option<&str>,
) -> anyhow::Result<Vec<Todo>> {
    let parsed = content.map(|c| parse_note(c, file)).unwrap_or_default();
    let file = file.clone();
    transform_with_lock(cache_dir, move |list| with_note_replaced(list, &file, parsed)).await
}

struct NoteEdit {
    parsed: Vec<Todo>,
    todos: Vec<Todo>,
}

fn edit_note<F>(notes_dir: &Path, cache_dir: &Path, file: &NoteRelPath, edit: F) -> anyhow::Result<NoteEdit>
where
    F: FnOnce(Option<&str>) -> anyhow::Result<String>,
{
    let _lock = NoteLocks::in_cache(cache_dir).note(file)?;
    let path = file.abs(notes_dir);
    let before = match std::fs::read_to_string(&path) {
        Ok(content) => Some(content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    let after = edit(before.as_deref())?;
    match &before {
        Some(content) if *content == after => {}
        Some(_) => write::replace(&path, &after)?,
        None => write::create(&path, &after)?,
    }
    let parsed = parse_note(&after, file);
    let replacement = parsed.clone();
    let todos = transform_blocking(cache_dir, |list| with_note_replaced(list, file, replacement))?;
    Ok(NoteEdit { parsed, todos })
}

fn appended(content: Option<&str>, lines: &[String]) -> anyhow::Result<String> {
    let base = match content {
        Some(content) => content.to_string(),
        None => format!("{}\n", Frontmatter::todos().to_block()?),
    };
    let separator = if base.is_empty() || base.ends_with('\n') { "" } else { "\n" };
    Ok(format!("{base}{separator}{}\n", lines.join("\n")))
}

fn add_todo_blocking(
    notes_dir: &Path,
    cache_dir: &Path,
    text: &str,
    date: Option<NaiveDate>,
) -> anyhow::Result<Edited> {
    if text.trim().is_empty() {
        bail!("todo text is empty");
    }
    let line = render_todo_line(Checkbox::Open, text, date);
    let file = NoteRelPath::todos();
    let mut appended_at = 0;
    let edit = edit_note(notes_dir, cache_dir, &file, |content| {
        let after = appended(content, std::slice::from_ref(&line))?;
        appended_at = after.lines().count() as u32;
        Ok(after)
    })?;
    let todo = edit
        .parsed
        .into_iter()
        .find(|t| t.line == appended_at)
        .ok_or_else(|| anyhow!("added todo is missing from {file}"))?;
    Ok(Edited { todo, todos: edit.todos })
}

pub async fn add_todo(
    notes_dir: &Path,
    cache_dir: &Path,
    text: String,
    date: Option<NaiveDate>,
) -> anyhow::Result<Edited> {
    let (notes_dir, cache_dir) = (notes_dir.to_path_buf(), cache_dir.to_path_buf());
    tokio::task::spawn_blocking(move || add_todo_blocking(&notes_dir, &cache_dir, &text, date)).await?
}

fn cached(cache_dir: &Path, id: Uuid) -> anyhow::Result<Todo> {
    read_todos_inner(cache_dir)
        .into_iter()
        .find(|t| t.id == id)
        .ok_or_else(|| anyhow!("no todo with id {id}"))
}

fn line_of(content: Option<&str>, file: &NoteRelPath, id: Uuid) -> anyhow::Result<u32> {
    let content = content.ok_or_else(|| anyhow!("{file} no longer exists"))?;
    parse_note(content, file)
        .into_iter()
        .find(|t| t.id == id)
        .map(|t| t.line)
        .ok_or_else(|| anyhow!("todo {id} is no longer in {file}"))
}

fn set_completed_blocking(
    notes_dir: &Path,
    cache_dir: &Path,
    id: Uuid,
    state: Checkbox,
) -> anyhow::Result<Edited> {
    let file = cached(cache_dir, id)?.file;
    let edit = edit_note(notes_dir, cache_dir, &file, |content| {
        let line = line_of(content, &file, id)?;
        rewrite_checkbox_line(content.unwrap_or_default(), line, state)
            .ok_or_else(|| anyhow!("could not rewrite checkbox at line {line} of {file}"))
    })?;
    let todo = edit
        .parsed
        .into_iter()
        .find(|t| t.id == id)
        .ok_or_else(|| anyhow!("todo {id} is no longer in {file}"))?;
    Ok(Edited { todo, todos: edit.todos })
}

pub async fn set_completed(
    notes_dir: &Path,
    cache_dir: &Path,
    id: Uuid,
    state: Checkbox,
) -> anyhow::Result<Edited> {
    let (notes_dir, cache_dir) = (notes_dir.to_path_buf(), cache_dir.to_path_buf());
    tokio::task::spawn_blocking(move || set_completed_blocking(&notes_dir, &cache_dir, id, state)).await?
}

fn remove_todo_blocking(notes_dir: &Path, cache_dir: &Path, id: Uuid) -> anyhow::Result<Edited> {
    let todo = cached(cache_dir, id)?;
    let edit = edit_note(notes_dir, cache_dir, &todo.file, |content| {
        let line = line_of(content, &todo.file, id)?;
        Ok(without_line(content.unwrap_or_default(), line))
    })?;
    Ok(Edited { todo, todos: edit.todos })
}

pub async fn remove_todo(notes_dir: &Path, cache_dir: &Path, id: Uuid) -> anyhow::Result<Edited> {
    let (notes_dir, cache_dir) = (notes_dir.to_path_buf(), cache_dir.to_path_buf());
    tokio::task::spawn_blocking(move || remove_todo_blocking(&notes_dir, &cache_dir, id)).await?
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyTodo {
    #[serde(rename = "id")]
    _id: serde::de::IgnoredAny,
    text: String,
    date: Option<NaiveDate>,
    completed: bool,
    #[serde(default)]
    source: LegacySource,
}

#[derive(Default, Deserialize)]
#[serde(tag = "kind")]
enum LegacySource {
    #[default]
    Manual,
    #[serde(other)]
    NoteParsed,
}

fn adopt_legacy_todos_blocking(notes_dir: &Path, cache_dir: &Path) -> anyhow::Result<usize> {
    let Ok(raw) = std::fs::read_to_string(todos_path(cache_dir)) else {
        return Ok(0);
    };
    let Ok(legacy) = serde_json::from_str::<Vec<LegacyTodo>>(&raw) else {
        return Ok(0);
    };
    let lines: Vec<String> = legacy
        .iter()
        .filter(|t| matches!(t.source, LegacySource::Manual) && !t.text.trim().is_empty())
        .map(|t| render_todo_line(Checkbox::from_completed(t.completed), &t.text, t.date))
        .collect();
    match lines.is_empty() {
        true => transform_blocking(cache_dir, |_| Vec::new()).map(drop)?,
        false => edit_note(notes_dir, cache_dir, &NoteRelPath::todos(), |content| appended(content, &lines)).map(drop)?,
    }
    Ok(lines.len())
}

pub async fn adopt_legacy_todos(notes_dir: &Path, cache_dir: &Path) -> anyhow::Result<usize> {
    let (notes_dir, cache_dir) = (notes_dir.to_path_buf(), cache_dir.to_path_buf());
    tokio::task::spawn_blocking(move || adopt_legacy_todos_blocking(&notes_dir, &cache_dir)).await?
}

fn rewrite_checkbox_line(content: &str, line_1based: u32, state: Checkbox) -> Option<String> {
    let target_idx = line_1based.checked_sub(1)? as usize;
    let segments: Vec<&str> = content.split_inclusive('\n').collect();
    let (before, rest) = segments.split_at_checked(target_idx)?;
    let (target, after) = rest.split_first()?;
    let rewritten = rewrite_checkbox_in_segment(target, state)?;
    Some(format!("{}{rewritten}{}", before.concat(), after.concat()))
}

fn rewrite_checkbox_in_segment(segment: &str, state: Checkbox) -> Option<String> {
    let rest = segment.trim_start();
    let leading = &segment[..segment.len() - rest.len()];
    let (_, body) = split_checkbox(rest)?;
    Some(format!("{leading}{}{body}", state.marker()))
}

fn without_line(content: &str, line_1based: u32) -> String {
    content
        .split_inclusive('\n')
        .enumerate()
        .filter(|(idx, _)| *idx as u32 + 1 != line_1based)
        .map(|(_, segment)| segment)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note_todo(file: &str, text: &str) -> Todo {
        parse_note(&format!("- [ ] {text}"), &NoteRelPath::parse(file).unwrap()).remove(0)
    }

    fn date(d: u32) -> Option<NaiveDate> {
        NaiveDate::from_ymd_opt(2026, 10, d)
    }

    fn dirs() -> (tempfile::TempDir, tempfile::TempDir) {
        (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap())
    }

    fn read(notes: &tempfile::TempDir, rel: &str) -> String {
        std::fs::read_to_string(notes.path().join(rel)).unwrap()
    }

    #[test]
    fn concurrent_writes_do_not_lose_updates() {
        use std::thread;

        let tmp = tempfile::tempdir().unwrap();
        let cache_dir = tmp.path().to_path_buf();
        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(save_todos(&cache_dir, &[])).unwrap();

        let writers: Vec<_> = ["a", "b"]
            .into_iter()
            .map(|prefix| {
                let dir = cache_dir.clone();
                thread::spawn(move || {
                    let rt = tokio::runtime::Runtime::new().unwrap();
                    for i in 0..20 {
                        let todo = note_todo("n.md", &format!("{prefix}{i}"));
                        rt.block_on(transform_with_lock(&dir, move |mut list| {
                            list.push(todo);
                            list
                        }))
                        .unwrap();
                    }
                })
            })
            .collect();
        writers.into_iter().for_each(|h| h.join().unwrap());

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
    fn replacing_a_note_keeps_its_place_in_the_list() {
        let list = vec![note_todo("a.md", "one"), note_todo("b.md", "two"), note_todo("c.md", "three")];
        let replaced = with_note_replaced(list, &NoteRelPath::parse("b.md").unwrap(), vec![
            note_todo("b.md", "x"),
            note_todo("b.md", "y"),
        ]);
        let texts: Vec<&str> = replaced.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(texts, ["one", "x", "y", "three"]);
    }

    #[tokio::test]
    async fn refreshing_a_removed_note_drops_its_todos() {
        let cache = tempfile::tempdir().unwrap();
        save_todos(cache.path(), &[note_todo("a.md", "one"), note_todo("b.md", "two")]).await.unwrap();
        let after = refresh_note(cache.path(), &NoteRelPath::parse("a.md").unwrap(), None).await.unwrap();
        assert_eq!(after, [note_todo("b.md", "two")]);
    }

    #[tokio::test]
    async fn adding_creates_the_todos_note_and_caches_the_todo() {
        let (notes, cache) = dirs();
        let added = add_todo(notes.path(), cache.path(), "book  flights".into(), date(9)).await.unwrap();

        let content = read(&notes, "todos.md");
        assert!(content.starts_with("---\n"), "{content}");
        assert!(content.ends_with("\n- [ ] book flights @09-10-2026\n"), "{content}");
        assert_eq!((added.todo.text.as_str(), added.todo.date), ("book flights", date(9)));
        assert_eq!(load_todos(cache.path()).await, [added.todo]);
    }

    #[tokio::test]
    async fn adding_appends_after_a_note_without_trailing_newline() {
        let (notes, cache) = dirs();
        std::fs::write(notes.path().join("todos.md"), "- [ ] first").unwrap();
        let added = add_todo(notes.path(), cache.path(), "second".into(), None).await.unwrap();

        assert_eq!(read(&notes, "todos.md"), "- [ ] first\n- [ ] second\n");
        assert_eq!(added.todo.line, 2);
        assert_eq!(added.todos.len(), 2);
    }

    #[tokio::test]
    async fn adding_a_duplicate_returns_the_new_line() {
        let (notes, cache) = dirs();
        let first = add_todo(notes.path(), cache.path(), "call".into(), None).await.unwrap();
        let second = add_todo(notes.path(), cache.path(), "call".into(), None).await.unwrap();
        assert_ne!(first.todo.id, second.todo.id);
        assert_eq!(second.todo.line, first.todo.line + 1);
    }

    #[tokio::test]
    async fn adding_empty_text_fails_without_writing() {
        let (notes, cache) = dirs();
        assert!(add_todo(notes.path(), cache.path(), "   ".into(), None).await.is_err());
        assert!(!notes.path().join("todos.md").exists());
    }

    #[tokio::test]
    async fn completing_ticks_the_checkbox_by_id_even_after_it_moved() {
        let (notes, cache) = dirs();
        let rel = NoteRelPath::parse("stories/a.md").unwrap();
        std::fs::create_dir_all(notes.path().join("stories")).unwrap();
        std::fs::write(rel.abs(notes.path()), "- [ ] ship it\n").unwrap();
        refresh_note(cache.path(), &rel, Some("- [ ] ship it\n")).await.unwrap();
        let id = load_todos(cache.path()).await[0].id;

        std::fs::write(rel.abs(notes.path()), "# moved\n\n- [ ] Ship It\n").unwrap();
        let done = set_completed(notes.path(), cache.path(), id, Checkbox::Done).await.unwrap();

        assert_eq!(read(&notes, "stories/a.md"), "# moved\n\n- [x] Ship It\n");
        assert!(done.todo.completed);
        assert_eq!((done.todo.id, done.todo.line), (id, 3));
        assert_eq!(done.todos, [done.todo]);
    }

    #[tokio::test]
    async fn completing_the_second_of_two_duplicates_leaves_the_first() {
        let (notes, cache) = dirs();
        add_todo(notes.path(), cache.path(), "call".into(), None).await.unwrap();
        let second = add_todo(notes.path(), cache.path(), "call".into(), None).await.unwrap();

        set_completed(notes.path(), cache.path(), second.todo.id, Checkbox::Done).await.unwrap();
        assert!(read(&notes, "todos.md").ends_with("- [ ] call\n- [x] call\n"));
    }

    #[tokio::test]
    async fn completing_a_todo_edited_away_fails_without_writing() {
        let (notes, cache) = dirs();
        let added = add_todo(notes.path(), cache.path(), "old".into(), None).await.unwrap();
        let before = read(&notes, "todos.md").replace("old", "new");
        std::fs::write(notes.path().join("todos.md"), &before).unwrap();

        let err = set_completed(notes.path(), cache.path(), added.todo.id, Checkbox::Done).await.unwrap_err();
        assert!(err.to_string().contains("no longer in"), "{err}");
        assert_eq!(read(&notes, "todos.md"), before);
    }

    #[tokio::test]
    async fn completing_an_unknown_id_fails() {
        let (notes, cache) = dirs();
        assert!(set_completed(notes.path(), cache.path(), Uuid::nil(), Checkbox::Done).await.is_err());
    }

    #[tokio::test]
    async fn removing_deletes_only_that_line() {
        let (notes, cache) = dirs();
        std::fs::write(notes.path().join("todos.md"), "# t\n- [ ] keep\n- [ ] drop\nafter\n").unwrap();
        let rel = NoteRelPath::todos();
        refresh_note(cache.path(), &rel, Some(&read(&notes, "todos.md"))).await.unwrap();
        let drop_id = load_todos(cache.path()).await[1].id;

        let removed = remove_todo(notes.path(), cache.path(), drop_id).await.unwrap();
        assert_eq!(removed.todo.text, "drop");
        assert_eq!(read(&notes, "todos.md"), "# t\n- [ ] keep\nafter\n");
        assert_eq!(removed.todos.len(), 1);
    }

    const LEGACY_TODOS_JSON: &str = r#"[
  {
    "id": "00000000-0000-0000-0000-000000000001",
    "text": "manual todo",
    "date": "2026-05-31",
    "completed": false,
    "source": { "kind": "Manual" }
  },
  {
    "id": "00000000-0000-0000-0000-000000000002",
    "text": "older manual todo without a source",
    "date": null,
    "completed": true
  },
  {
    "id": "00000000-0000-0000-0000-000000000003",
    "text": "parsed todo",
    "date": null,
    "completed": false,
    "source": { "kind": "NoteParsed", "file": "stories/a.md", "line": 12, "fingerprint": 1 }
  }
]"#;

    #[tokio::test]
    async fn legacy_manual_todos_move_into_the_todos_note_once() {
        let (notes, cache) = dirs();
        std::fs::write(cache.path().join(TODOS_FILE), LEGACY_TODOS_JSON).unwrap();

        assert_eq!(adopt_legacy_todos(notes.path(), cache.path()).await.unwrap(), 2);
        let content = read(&notes, "todos.md");
        assert!(
            content.ends_with("\n- [ ] manual todo @31-05-2026\n- [x] older manual todo without a source\n"),
            "{content}"
        );
        let texts: Vec<String> = load_todos(cache.path()).await.into_iter().map(|t| t.text).collect();
        assert_eq!(texts, ["manual todo", "older manual todo without a source"]);

        assert_eq!(adopt_legacy_todos(notes.path(), cache.path()).await.unwrap(), 0);
        assert_eq!(read(&notes, "todos.md"), content);
    }

    #[tokio::test]
    async fn adopting_without_a_cache_file_is_a_noop() {
        let (notes, cache) = dirs();
        assert_eq!(adopt_legacy_todos(notes.path(), cache.path()).await.unwrap(), 0);
        assert!(!notes.path().join("todos.md").exists());
    }

    const GOLDEN_TODOS_JSON: &str = r#"[
  {
    "id": "00000000-0000-0000-0000-000000000001",
    "text": "parsed todo",
    "date": "2026-05-31",
    "completed": true,
    "file": "stories/a.md",
    "line": 12
  }
]"#;

    fn golden_todos() -> Vec<Todo> {
        vec![Todo {
            id: Uuid::from_u128(1),
            text: "parsed todo".into(),
            date: NaiveDate::from_ymd_opt(2026, 5, 31),
            completed: true,
            file: NoteRelPath::parse("stories/a.md").unwrap(),
            line: 12,
        }]
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

    #[test]
    fn current_cache_is_not_mistaken_for_legacy() {
        assert!(serde_json::from_str::<Vec<LegacyTodo>>(GOLDEN_TODOS_JSON).is_err());
    }
}
