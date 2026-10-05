use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tui_scrollview::ScrollViewState;

use crate::{
    api::story::Story,
    app::{cmd::Cmd, model::DescriptionModalState, msg::DescriptionModalMsg},
    navkey,
};

pub fn update(state: &mut DescriptionModalState, msg: DescriptionModalMsg) -> Vec<Cmd> {
    match msg {
        DescriptionModalMsg::Open => {
            vec![Cmd::None]
        }

        DescriptionModalMsg::Close => {
            state.is_showing = false;
            state.scroll_view_state = ScrollViewState::default();
            state.story = None;
            vec![Cmd::None]
        }

        DescriptionModalMsg::ScrollUp => {
            state.scroll_view_state.scroll_up();
            vec![Cmd::None]
        }

        DescriptionModalMsg::ScrollDown => {
            state.scroll_view_state.scroll_down();
            vec![Cmd::None]
        }

        DescriptionModalMsg::ScrollPageUp => {
            state.scroll_view_state.scroll_page_up();
            vec![Cmd::None]
        }

        DescriptionModalMsg::ScrollPageDown => {
            state.scroll_view_state.scroll_page_down();
            vec![Cmd::None]
        }

        DescriptionModalMsg::ScrollToTop => {
            state.scroll_view_state.scroll_to_top();
            vec![Cmd::None]
        }

        DescriptionModalMsg::ScrollToBottom => {
            state.scroll_view_state.scroll_to_bottom();
            vec![Cmd::None]
        }
    }
}

pub fn open(state: &mut DescriptionModalState, story: Story) {
    state.is_showing = true;
    state.scroll_view_state = ScrollViewState::default();
    state.story = Some(story);
}

pub fn key_to_msg(key: KeyEvent) -> Option<DescriptionModalMsg> {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => Some(DescriptionModalMsg::Close),
        navkey!(down) => Some(DescriptionModalMsg::ScrollDown),
        navkey!(up) => Some(DescriptionModalMsg::ScrollUp),
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(DescriptionModalMsg::ScrollPageDown)
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(DescriptionModalMsg::ScrollPageUp)
        }
        KeyCode::PageDown => Some(DescriptionModalMsg::ScrollPageDown),
        KeyCode::PageUp => Some(DescriptionModalMsg::ScrollPageUp),
        KeyCode::Char('g') => Some(DescriptionModalMsg::ScrollToTop),
        KeyCode::Char('G') => Some(DescriptionModalMsg::ScrollToBottom),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState};

    fn create_test_story() -> Story {
        Story {
            id: 1.into(),
            name: "Test Story".to_string(),
            description: "Test description".to_string(),
            completed: false,
            branches: vec![],
            comments: vec![],
            epic_id: None,
            iteration_id: None,
            app_url: "https://example.com".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_open_sets_state() {
        let mut state = DescriptionModalState::default();
        let story = create_test_story();

        open(&mut state, story.clone());

        assert!(state.is_showing);
        assert_eq!(state.story.as_ref().unwrap().id, story.id);
    }

    #[test]
    fn test_close_resets_state() {
        let mut state = DescriptionModalState {
            is_showing: true,
            scroll_view_state: ScrollViewState::default(),
            story: Some(create_test_story()),
        };

        update(&mut state, DescriptionModalMsg::Close);

        assert!(!state.is_showing);
        assert!(state.story.is_none());
    }

    #[test]
    fn test_key_to_msg_mappings() {
        let make_key = |code: KeyCode, modifiers: KeyModifiers| KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        };

        assert!(matches!(
            key_to_msg(make_key(KeyCode::Esc, KeyModifiers::NONE)),
            Some(DescriptionModalMsg::Close)
        ));

        assert!(matches!(
            key_to_msg(make_key(KeyCode::Char('q'), KeyModifiers::NONE)),
            Some(DescriptionModalMsg::Close)
        ));

        assert!(matches!(
            key_to_msg(make_key(KeyCode::Char('j'), KeyModifiers::NONE)),
            Some(DescriptionModalMsg::ScrollDown)
        ));

        assert!(matches!(
            key_to_msg(make_key(KeyCode::Char('k'), KeyModifiers::NONE)),
            Some(DescriptionModalMsg::ScrollUp)
        ));

        assert!(matches!(
            key_to_msg(make_key(KeyCode::Char('d'), KeyModifiers::CONTROL)),
            Some(DescriptionModalMsg::ScrollPageDown)
        ));

        assert!(matches!(
            key_to_msg(make_key(KeyCode::Char('u'), KeyModifiers::CONTROL)),
            Some(DescriptionModalMsg::ScrollPageUp)
        ));

        assert!(matches!(
            key_to_msg(make_key(KeyCode::PageDown, KeyModifiers::NONE)),
            Some(DescriptionModalMsg::ScrollPageDown)
        ));

        assert!(matches!(
            key_to_msg(make_key(KeyCode::PageUp, KeyModifiers::NONE)),
            Some(DescriptionModalMsg::ScrollPageUp)
        ));

        assert!(matches!(
            key_to_msg(make_key(KeyCode::Char('g'), KeyModifiers::NONE)),
            Some(DescriptionModalMsg::ScrollToTop)
        ));

        assert!(matches!(
            key_to_msg(make_key(KeyCode::Char('G'), KeyModifiers::NONE)),
            Some(DescriptionModalMsg::ScrollToBottom)
        ));
    }
}
