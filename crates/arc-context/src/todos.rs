use std::fmt;
use std::path::Path;

use arc_notes::todos::{self, load_todos};
use arc_notes::{Checkbox, NoteRelPath, Todo};
use chrono::NaiveDate;
use serde::Serialize;
use uuid::Uuid;

use crate::blocking;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ListedTodo {
    pub id: Uuid,
    pub text: String,
    pub date: Option<NaiveDate>,
    pub note: NoteRelPath,
}

impl From<Todo> for ListedTodo {
    fn from(todo: Todo) -> Self {
        Self {
            id: todo.id,
            text: todo.text,
            date: todo.date,
            note: todo.file,
        }
    }
}

impl fmt::Display for ListedTodo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}  {}", self.id, self.text)?;
        if let Some(date) = self.date {
            write!(f, " (due {date})")?;
        }
        write!(f, " · {}", self.note)
    }
}

pub async fn open_todos(cache_dir: &Path) -> Vec<ListedTodo> {
    load_todos(cache_dir)
        .await
        .into_iter()
        .filter(|todo| !todo.completed)
        .map(ListedTodo::from)
        .collect()
}

pub async fn add_todo(
    notes_dir: &Path,
    cache_dir: &Path,
    text: String,
    date: Option<NaiveDate>,
) -> anyhow::Result<Todo> {
    let added = todos::add_todo(notes_dir, cache_dir, text, date).await?;
    mirror(cache_dir, added.todos).await?;
    Ok(added.todo)
}

pub async fn complete_todo(cache_dir: &Path, notes_dir: &Path, id: Uuid) -> anyhow::Result<Todo> {
    let done = todos::set_completed(notes_dir, cache_dir, id, Checkbox::Done).await?;
    mirror(cache_dir, done.todos).await?;
    Ok(done.todo)
}

async fn mirror(cache_dir: &Path, todos: Vec<Todo>) -> anyhow::Result<()> {
    let cache_dir = cache_dir.to_path_buf();
    blocking(move || {
        let mut conn = arc_index::open(&cache_dir)?;
        arc_index::mirror_todos(&mut conn, &todos)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn added_todos_are_listed_until_completed() {
        let (notes, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let date = NaiveDate::from_ymd_opt(2026, 10, 9);
        let added = add_todo(notes.path(), cache.path(), "book flights".to_string(), date).await.unwrap();

        let listed = open_todos(cache.path()).await;
        assert_eq!(
            listed,
            [ListedTodo { id: added.id, text: "book flights".to_string(), date, note: NoteRelPath::todos() }]
        );

        let completed = complete_todo(cache.path(), notes.path(), added.id).await.unwrap();
        assert!(completed.completed);
        assert!(open_todos(cache.path()).await.is_empty());
        let note = std::fs::read_to_string(notes.path().join("todos.md")).unwrap();
        assert!(note.ends_with("- [x] book flights @09-10-2026\n"), "{note}");
    }

    #[tokio::test]
    async fn completing_a_story_note_todo_ticks_its_checkbox() {
        let (notes, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let rel = NoteRelPath::parse("alpha.md").unwrap();
        let content = "# Alpha\n\n- [ ] ship it\n- [ ] other\n";
        std::fs::write(rel.abs(notes.path()), content).unwrap();
        let cached = arc_notes::todos::refresh_note(cache.path(), &rel, Some(content)).await.unwrap();

        complete_todo(cache.path(), notes.path(), cached[0].id).await.unwrap();

        let note = std::fs::read_to_string(rel.abs(notes.path())).unwrap();
        assert_eq!(note, "# Alpha\n\n- [x] ship it\n- [ ] other\n");
        assert_eq!(open_todos(cache.path()).await, [ListedTodo::from(cached[1].clone())]);
    }

    #[tokio::test]
    async fn completing_an_unknown_id_fails_without_writing() {
        let (notes, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let kept = add_todo(notes.path(), cache.path(), "keep".to_string(), None).await.unwrap();

        assert!(complete_todo(cache.path(), notes.path(), Uuid::new_v4()).await.is_err());
        assert_eq!(open_todos(cache.path()).await, [ListedTodo::from(kept)]);
    }
}
