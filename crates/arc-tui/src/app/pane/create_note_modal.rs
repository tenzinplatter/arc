use crossterm::event::{KeyCode, KeyEvent};

use crate::app::{
    cmd::{Cmd, NoteToOpen},
    model::Overlay,
    msg::CreateNoteModalMsg,
};

pub fn update(overlay: &mut Overlay, msg: CreateNoteModalMsg) -> Vec<Cmd> {
    match msg {
        CreateNoteModalMsg::Open => {
            *overlay = Overlay::CreateNote {
                input: String::new(),
            }
        }
        CreateNoteModalMsg::Close => *overlay = Overlay::None,
        CreateNoteModalMsg::TextInput(c) => {
            if let Overlay::CreateNote { input } = overlay {
                input.push(c);
            }
        }
        CreateNoteModalMsg::TextBackspace => {
            if let Overlay::CreateNote { input } = overlay {
                input.pop();
            }
        }
        CreateNoteModalMsg::Accept => {
            if let Overlay::CreateNote { input } = overlay
                && !input.is_empty()
            {
                let note = NoteToOpen::scratch(input);
                *overlay = Overlay::None;
                return vec![Cmd::OpenNote(note)];
            }
        }
    }
    vec![Cmd::None]
}

pub fn key_to_msg(key: KeyEvent) -> Option<CreateNoteModalMsg> {
    match key.code {
        KeyCode::Esc => Some(CreateNoteModalMsg::Close),
        KeyCode::Enter => Some(CreateNoteModalMsg::Accept),
        KeyCode::Backspace => Some(CreateNoteModalMsg::TextBackspace),
        KeyCode::Char(c) => Some(CreateNoteModalMsg::TextInput(c)),
        _ => None,
    }
}
