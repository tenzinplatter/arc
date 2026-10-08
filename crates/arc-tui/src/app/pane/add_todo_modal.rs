use crossterm::event::{KeyCode, KeyEvent};

use crate::app::{cmd::Cmd, model::Overlay, msg::AddTodoModalMsg};

pub fn update(overlay: &mut Overlay, msg: AddTodoModalMsg) -> Vec<Cmd> {
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
                let text = std::mem::take(input);
                *overlay = Overlay::None;
                return vec![Cmd::AddTodo { text }];
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
