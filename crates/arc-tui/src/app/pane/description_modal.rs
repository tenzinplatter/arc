use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::{
    app::{cmd::Cmd, model::Overlay, msg::DescriptionModalMsg},
    navkey,
};

pub fn update(overlay: &mut Overlay, msg: DescriptionModalMsg) -> Vec<Cmd> {
    let Overlay::Description { scroll, .. } = overlay else {
        return vec![Cmd::None];
    };
    match msg {
        DescriptionModalMsg::Close => *overlay = Overlay::None,
        DescriptionModalMsg::ScrollUp => scroll.scroll_up(),
        DescriptionModalMsg::ScrollDown => scroll.scroll_down(),
        DescriptionModalMsg::ScrollPageUp => scroll.scroll_page_up(),
        DescriptionModalMsg::ScrollPageDown => scroll.scroll_page_down(),
        DescriptionModalMsg::ScrollToTop => scroll.scroll_to_top(),
        DescriptionModalMsg::ScrollToBottom => scroll.scroll_to_bottom(),
    }
    vec![Cmd::None]
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
    use crate::api::story::Story;
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
    fn scrolling_keeps_description_open() {
        let story = create_test_story();
        let mut overlay = Overlay::description(story.clone());

        update(&mut overlay, DescriptionModalMsg::ScrollDown);

        assert!(matches!(overlay, Overlay::Description { story: ref s, .. } if s.id == story.id));
    }

    #[test]
    fn close_dismisses_overlay() {
        let mut overlay = Overlay::description(create_test_story());

        update(&mut overlay, DescriptionModalMsg::Close);

        assert!(matches!(overlay, Overlay::None));
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
