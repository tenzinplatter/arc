use std::collections::BTreeMap;

use chrono::NaiveDate;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use uuid::Uuid;

use arc_notes::Checkbox;

use crate::{
    app::{cmd::Cmd, msg::TodosListMsg, pane::nav},
    todos::Todo,
};

pub use crate::app::model::TodosListState;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SectionKind {
    Overdue,
    Today,
    Tomorrow,
    Future(NaiveDate),
    NoDate,
}

impl SectionKind {
    pub fn header(&self) -> String {
        match self {
            SectionKind::Overdue => "Overdue".to_string(),
            SectionKind::Tomorrow => "Tomorrow".to_string(),
            SectionKind::Today => "Today".to_string(),
            SectionKind::Future(d) => d.format("%a, %b %-d %Y").to_string(),
            SectionKind::NoDate => "No date".to_string(),
        }
    }
}

pub struct DaySection<'a> {
    pub kind: SectionKind,
    pub todos: Vec<&'a Todo>,
}

pub fn group_todos_by_section<'a>(
    todos: impl IntoIterator<Item = &'a Todo>,
    today: NaiveDate,
) -> Vec<DaySection<'a>> {
    let tomorrow = today.succ_opt().expect("today + 1 day fits in NaiveDate");
    let (mut overdue, mut on_today, mut on_tomorrow, mut no_date) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut future: BTreeMap<NaiveDate, Vec<&Todo>> = BTreeMap::new();

    for todo in todos {
        let bucket = match todo.date {
            None => &mut no_date,
            Some(d) if d < today => &mut overdue,
            Some(d) if d == today => &mut on_today,
            Some(d) if d == tomorrow => &mut on_tomorrow,
            Some(d) => future.entry(d).or_default(),
        };
        bucket.push(todo);
    }

    [
        (SectionKind::Overdue, overdue),
        (SectionKind::Today, on_today),
        (SectionKind::Tomorrow, on_tomorrow),
    ]
    .into_iter()
    .chain(future.into_iter().map(|(d, todos)| (SectionKind::Future(d), todos)))
    .chain([(SectionKind::NoDate, no_date)])
    .filter(|(_, todos)| !todos.is_empty())
    .map(|(kind, todos)| DaySection { kind, todos })
    .collect()
}

fn section_of(todo_id: Uuid, sections: &[DaySection]) -> Option<usize> {
    sections
        .iter()
        .position(|section| section.todos.iter().any(|t| t.id == todo_id))
}

fn ordered_ids(sections: &[DaySection]) -> Vec<Uuid> {
    sections
        .iter()
        .flat_map(|s| s.todos.iter().map(|t| t.id))
        .collect()
}

fn next_todo_id(current_id: Uuid, sections: &[DaySection]) -> Option<Uuid> {
    nav::step_wrapping(&ordered_ids(sections), current_id, true)
}

fn prev_todo_id(current_id: Uuid, sections: &[DaySection]) -> Option<Uuid> {
    nav::step_wrapping(&ordered_ids(sections), current_id, false)
}

pub fn visible_sections<'a>(state: &TodosListState, todos: &'a [Todo]) -> Vec<DaySection<'a>> {
    group_todos_by_section(
        todos.iter().filter(|t| state.is_visible(t)),
        arc_core::time::today(),
    )
}

pub fn update(state: &mut TodosListState, todos: &mut Vec<Todo>, msg: TodosListMsg) -> Vec<Cmd> {
    match msg {
        TodosListMsg::FocusNext => {
            let sections = visible_sections(state, todos);
            if sections.is_empty() {
                return vec![Cmd::None];
            }
            state.selected_id = match state.selected_id {
                Some(current_id) => next_todo_id(current_id, &sections),
                None => sections.first().and_then(|s| s.todos.first()).map(|t| t.id),
            };
            vec![Cmd::None]
        }

        TodosListMsg::FocusPrev => {
            let sections = visible_sections(state, todos);
            if sections.is_empty() {
                return vec![Cmd::None];
            }
            state.selected_id = match state.selected_id {
                Some(current_id) => prev_todo_id(current_id, &sections),
                None => sections.last().and_then(|s| s.todos.last()).map(|t| t.id),
            };
            vec![Cmd::None]
        }

        TodosListMsg::ToggleComplete => {
            let Some(id) = state.selected_id else {
                return vec![Cmd::None];
            };
            let Some(todo) = todos.iter_mut().find(|t| t.id == id) else {
                return vec![Cmd::None];
            };
            todo.completed = !todo.completed;
            state.visible.insert(id);
            vec![Cmd::SetTodoCompleted { id, to: Checkbox::from_completed(todo.completed) }]
        }

        TodosListMsg::FocusSectionNext | TodosListMsg::FocusSectionPrev => {
            let sections = visible_sections(state, todos);
            if sections.is_empty() {
                return vec![Cmd::None];
            }

            let cur_section = state
                .selected_id
                .and_then(|id| section_of(id, &sections))
                .unwrap_or(0);

            let target = if matches!(msg, TodosListMsg::FocusSectionNext) {
                (cur_section + 1) % sections.len()
            } else {
                (cur_section + sections.len() - 1) % sections.len()
            };

            state.selected_id = sections[target].todos.first().map(|t| t.id);
            vec![Cmd::None]
        }

        TodosListMsg::DeleteSelected => {
            let Some(id) = state.selected_id else {
                return vec![Cmd::None];
            };
            let in_todos_note = todos
                .iter()
                .find(|t| t.id == id)
                .is_some_and(|t| t.file.is_todo_list());
            if !in_todos_note {
                return vec![Cmd::None];
            }

            let sections = visible_sections(state, todos);
            let next_id = next_todo_id(id, &sections);
            let prev_id = prev_todo_id(id, &sections);

            todos.retain(|t| t.id != id);

            let still_listed = |candidate: &Uuid| todos.iter().any(|t| t.id == *candidate);
            state.selected_id = next_id
                .filter(still_listed)
                .or(prev_id.filter(still_listed))
                .or_else(|| todos.first().map(|t| t.id));
            vec![Cmd::RemoveTodo { id }]
        }
    }
}

pub fn key_to_msg(key: KeyEvent) -> Option<TodosListMsg> {
    match key.code {
        KeyCode::Char('j') if key.modifiers == KeyModifiers::CONTROL => {
            Some(TodosListMsg::FocusSectionNext)
        }
        KeyCode::Char('k') if key.modifiers == KeyModifiers::CONTROL => {
            Some(TodosListMsg::FocusSectionPrev)
        }
        KeyCode::Char('j') | KeyCode::Down => Some(TodosListMsg::FocusNext),
        KeyCode::Char('k') | KeyCode::Up => Some(TodosListMsg::FocusPrev),
        KeyCode::Char(' ') | KeyCode::Enter => Some(TodosListMsg::ToggleComplete),
        KeyCode::Char('d') => Some(TodosListMsg::DeleteSelected),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_notes::NoteRelPath;

    fn manual_dated(text: &str, date: NaiveDate) -> Todo {
        Todo {
            id: uuid::Uuid::new_v4(),
            text: text.to_string(),
            date: Some(date),
            completed: false,
            file: NoteRelPath::todos(),
            line: 1,
            scope: Default::default(),
        }
    }

    fn manual_undated(text: &str) -> Todo {
        Todo {
            id: uuid::Uuid::new_v4(),
            text: text.to_string(),
            date: None,
            completed: false,
            file: NoteRelPath::todos(),
            line: 1,
            scope: Default::default(),
        }
    }

    fn completed(text: &str) -> Todo {
        Todo {
            id: uuid::Uuid::new_v4(),
            text: text.to_string(),
            date: None,
            completed: true,
            file: NoteRelPath::todos(),
            line: 1,
            scope: Default::default(),
        }
    }

    fn selected(todos: &[Todo], id: Uuid) -> TodosListState {
        TodosListState {
            selected_id: Some(id),
            ..TodosListState::new(todos)
        }
    }

    #[test]
    fn toggling_asks_for_the_checkbox_to_change() {
        let todo = manual_undated("tick");
        let mut todos = vec![todo.clone()];
        let cmds = update(&mut selected(&todos, todo.id), &mut todos, TodosListMsg::ToggleComplete);
        assert!(matches!(cmds[..], [Cmd::SetTodoCompleted { id, to: Checkbox::Done }] if id == todo.id));
    }

    #[test]
    fn deleting_only_removes_todos_from_the_todos_note() {
        let added = manual_undated("added by hand");
        let in_story = Todo {
            file: NoteRelPath::parse("stories/a.md").unwrap(),
            ..manual_undated("lives in a story")
        };
        let mut todos = vec![added.clone(), in_story.clone()];

        let cmds = update(&mut selected(&todos, in_story.id), &mut todos, TodosListMsg::DeleteSelected);
        assert!(matches!(cmds[..], [Cmd::None]));
        assert_eq!(todos.len(), 2);

        let cmds = update(&mut selected(&todos, added.id), &mut todos, TodosListMsg::DeleteSelected);
        assert!(matches!(cmds[..], [Cmd::RemoveTodo { id }] if id == added.id));
        assert_eq!(todos, [in_story]);
    }

    #[test]
    fn deleting_removes_todos_from_a_repo_todos_note() {
        let in_repo = Todo {
            file: NoteRelPath::repo_todos("o-arc"),
            ..manual_undated("repo scoped")
        };
        let mut todos = vec![in_repo.clone()];

        let cmds = update(&mut selected(&todos, in_repo.id), &mut todos, TodosListMsg::DeleteSelected);
        assert!(matches!(cmds[..], [Cmd::RemoveTodo { id }] if id == in_repo.id));
    }

    #[test]
    fn completed_todo_loaded_at_open_is_hidden() {
        let incomplete = manual_undated("keep me");
        let done = completed("hide me");
        let todos = vec![incomplete.clone(), done.clone()];

        let state = TodosListState::new(&todos);

        assert!(state.is_visible(&incomplete));
        assert!(!state.is_visible(&done), "pre-completed todo stays hidden");

        let sections = visible_sections(&state, &todos);
        let ids: Vec<_> = sections.iter().flat_map(|s| s.todos.iter().map(|t| t.id)).collect();
        assert_eq!(ids, vec![incomplete.id]);
    }

    #[test]
    fn toggling_complete_keeps_todo_visible() {
        let todo = manual_undated("oops");
        let mut todos = vec![todo.clone()];
        let mut state = TodosListState {
            selected_id: Some(todo.id),
            ..TodosListState::new(&todos)
        };

        update(&mut state, &mut todos, TodosListMsg::ToggleComplete);

        assert!(todos[0].completed);
        assert!(
            state.is_visible(&todos[0]),
            "a todo completed this session must remain on screen"
        );
        let sections = visible_sections(&state, &todos);
        assert_eq!(sections.iter().flat_map(|s| &s.todos).count(), 1);
    }

    #[test]
    fn refresh_visible_drops_ids_that_disappear() {
        let todo = manual_undated("gone soon");
        let mut state = TodosListState::new(std::slice::from_ref(&todo));
        assert!(state.visible.contains(&todo.id));

        state.refresh_visible(&[]);
        assert!(state.visible.is_empty());
    }

    #[test]
    fn groups_overdue_today_tomorrow_future_nodate_in_order() {
        let today = NaiveDate::from_ymd_opt(2026, 5, 31).unwrap();
        let tomorrow = today.succ_opt().unwrap();
        let yesterday = today.pred_opt().unwrap();
        let next_week = today + chrono::Duration::days(7);

        let todos = vec![
            manual_dated("future", next_week),
            manual_dated("yesterday", yesterday),
            manual_undated("no date"),
            manual_dated("today", today),
            manual_dated("tomorrow", tomorrow),
        ];

        let kinds: Vec<_> = group_todos_by_section(&todos, today)
            .into_iter()
            .map(|s| s.kind)
            .collect();
        assert_eq!(
            kinds,
            vec![
                SectionKind::Overdue,
                SectionKind::Today,
                SectionKind::Tomorrow,
                SectionKind::Future(next_week),
                SectionKind::NoDate,
            ]
        );
    }
}
