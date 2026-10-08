use std::fmt;
use std::path::Path;

use anyhow::anyhow;
use arc_notes::todos::{load_todos, toggle_note_checkbox, transform_with_lock};
use arc_notes::{Checkbox, NoteRelPath, Todo, TodoSource};
use chrono::NaiveDate;
use serde::Serialize;
use uuid::Uuid;

use crate::blocking;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ListedTodo {
    pub id: Uuid,
    pub text: String,
    pub date: Option<NaiveDate>,
    pub note: Option<NoteRelPath>,
}

impl From<Todo> for ListedTodo {
    fn from(todo: Todo) -> Self {
        let note = match todo.source {
            TodoSource::NoteParsed { file, .. } => Some(file),
            TodoSource::Manual => None,
        };
        Self {
            id: todo.id,
            text: todo.text,
            date: todo.date,
            note,
        }
    }
}

impl fmt::Display for ListedTodo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}  {}", self.id, self.text)?;
        if let Some(date) = self.date {
            write!(f, " (due {date})")?;
        }
        if let Some(note) = &self.note {
            write!(f, " · {note}")?;
        }
        Ok(())
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

pub async fn add_todo(cache_dir: &Path, text: String, date: Option<NaiveDate>) -> anyhow::Result<Todo> {
    let todo = Todo {
        id: Uuid::new_v4(),
        text,
        date,
        completed: false,
        source: TodoSource::Manual,
    };
    let added = todo.clone();
    let updated = transform_with_lock(cache_dir, move |list| list.into_iter().chain([added]).collect()).await?;
    mirror(cache_dir, updated).await?;
    Ok(todo)
}

pub async fn complete_todo(cache_dir: &Path, notes_dir: &Path, id: Uuid) -> anyhow::Result<Todo> {
    let todo = load_todos(cache_dir)
        .await
        .into_iter()
        .find(|todo| todo.id == id)
        .ok_or_else(|| anyhow!("no todo with id {id}"))?;
    if let TodoSource::NoteParsed { file, .. } = &todo.source {
        toggle_note_checkbox(notes_dir, file, &todo.text, Checkbox::Done).await?;
    }
    let updated = transform_with_lock(cache_dir, move |list| {
        list.into_iter()
            .map(|todo| if todo.id == id { Todo { completed: true, ..todo } } else { todo })
            .collect()
    })
    .await?;
    mirror(cache_dir, updated).await?;
    Ok(Todo { completed: true, ..todo })
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

    fn manual(text: &str) -> Todo {
        Todo {
            id: Uuid::new_v4(),
            text: text.to_string(),
            date: None,
            completed: false,
            source: TodoSource::Manual,
        }
    }

    #[tokio::test]
    async fn added_todos_are_listed_until_completed() {
        let dir = tempfile::tempdir().unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 10, 9);
        let added = add_todo(dir.path(), "book flights".to_string(), date).await.unwrap();

        let listed = open_todos(dir.path()).await;
        assert_eq!(listed, [ListedTodo { id: added.id, text: "book flights".to_string(), date, note: None }]);

        let completed = complete_todo(dir.path(), dir.path(), added.id).await.unwrap();
        assert!(completed.completed);
        assert!(open_todos(dir.path()).await.is_empty());
    }

    #[tokio::test]
    async fn completing_a_note_todo_ticks_its_checkbox() {
        let cache = tempfile::tempdir().unwrap();
        let notes = tempfile::tempdir().unwrap();
        std::fs::write(notes.path().join("alpha.md"), "# Alpha\n\n- [ ] ship it\n- [ ] other\n").unwrap();
        let todo = Todo {
            source: TodoSource::NoteParsed {
                file: NoteRelPath::parse("alpha.md").unwrap(),
                line: 3,
                fingerprint: 1,
            },
            ..manual("ship it")
        };
        arc_notes::todos::save_todos(cache.path(), &[todo.clone()]).await.unwrap();

        complete_todo(cache.path(), notes.path(), todo.id).await.unwrap();

        let note = std::fs::read_to_string(notes.path().join("alpha.md")).unwrap();
        assert_eq!(note, "# Alpha\n\n- [x] ship it\n- [ ] other\n");
        assert!(open_todos(cache.path()).await.is_empty());
    }

    #[tokio::test]
    async fn completing_an_unknown_id_fails_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let kept = manual("keep");
        arc_notes::todos::save_todos(dir.path(), &[kept.clone()]).await.unwrap();

        assert!(complete_todo(dir.path(), dir.path(), Uuid::new_v4()).await.is_err());
        assert_eq!(open_todos(dir.path()).await, [ListedTodo::from(kept)]);
    }
}
