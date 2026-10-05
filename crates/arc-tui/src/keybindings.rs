use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::navkey;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    FocusNext,
    FocusPrev,
    Quit,
    HelpPanel,
    ViewNext,
    ViewPrev,
    DailyNote,
    Description,
    IterationNote,
    OpenNote,
    OpenBrowser,
    EditDescription,
    Tmux,
    SelectStory,
    ToggleFinished,
}

impl Key {
    pub fn from_key_event(key: KeyEvent) -> Option<Key> {
        match key.code {
            navkey!(down) => Some(Key::FocusNext),
            navkey!(up) => Some(Key::FocusPrev),
            KeyCode::Tab | KeyCode::Char('L') => Some(Key::ViewNext),
            KeyCode::BackTab | KeyCode::Char('H') => Some(Key::ViewPrev),
            KeyCode::Char('q') => Some(Key::Quit),
            KeyCode::Char('?') => Some(Key::HelpPanel),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Some(Key::Quit),
            KeyCode::Char('d') => Some(Key::DailyNote),
            KeyCode::Char(' ') => Some(Key::Description),
            KeyCode::Char('i') => Some(Key::IterationNote),
            KeyCode::Char('n') => Some(Key::OpenNote),
            KeyCode::Char('o') => Some(Key::OpenBrowser),
            KeyCode::Char('e') => Some(Key::EditDescription),
            KeyCode::Char('t') => Some(Key::Tmux),
            KeyCode::Char('a') => Some(Key::SelectStory),
            KeyCode::Char('f') => Some(Key::ToggleFinished),
            _ => None,
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Key::FocusNext => "Focus next story",
            Key::FocusPrev => "Focus previous story",
            Key::Quit => "Quit",
            Key::HelpPanel => "Toggle keybinds help",
            Key::ViewNext => "Next view",
            Key::ViewPrev => "Previous view",
            Key::DailyNote => "Open daily note",
            Key::Description => "Show story description",
            Key::IterationNote => "Open iteration note",
            Key::OpenNote => "Open story note",
            Key::OpenBrowser => "Open in browser",
            Key::EditDescription => "Edit story description",
            Key::Tmux => "Open tmux session",
            Key::SelectStory => "Select as active story",
            Key::ToggleFinished => "Toggle show finished",
        }
    }
}
