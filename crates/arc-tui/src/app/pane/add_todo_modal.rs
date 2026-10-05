use crossterm::event::{KeyCode, KeyEvent};

use crate::{
    app::{cmd::Cmd, model::Overlay, msg::AddTodoModalMsg},
    todos::Todo,
};

pub fn update(overlay: &mut Overlay, todos: &mut Vec<Todo>, msg: AddTodoModalMsg) -> Vec<Cmd> {
    match msg {
        AddTodoModalMsg::Open => {
            *overlay = Overlay::AddTodo {
                input: String::new(),
            }
        }
        AddTodoModalMsg::Close => *overlay = Overlay::None,
        AddTodoModalMsg::TextInput(c) => {
            if let Overlay::AddTodo { input } = overlay {
                input.push(c);
            }
        }
        AddTodoModalMsg::TextBackspace => {
            if let Overlay::AddTodo { input } = overlay {
                input.pop();
            }
        }
        AddTodoModalMsg::Accept => {
            if let Overlay::AddTodo { input } = overlay
                && !input.is_empty()
            {
                todos.push(Todo::new_manual(input.clone(), arc_core::time::today()));
                *overlay = Overlay::None;
                return vec![Cmd::WriteTodos];
            }
        }
    }
    vec![Cmd::None]
}

pub fn key_to_msg(key: KeyEvent) -> Option<AddTodoModalMsg> {
    match key.code {
        KeyCode::Esc => Some(AddTodoModalMsg::Close),
        KeyCode::Enter => Some(AddTodoModalMsg::Accept),
        KeyCode::Backspace => Some(AddTodoModalMsg::TextBackspace),
        KeyCode::Char(c) => Some(AddTodoModalMsg::TextInput(c)),
        _ => None,
    }
}
