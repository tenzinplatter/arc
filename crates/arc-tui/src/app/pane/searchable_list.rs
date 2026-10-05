//! Generic fuzzy-searchable list pane, shared by the epic and iteration views.
//! Both are the same interaction: a filterable list with j/k navigation, a
//! typing mode toggled with `/`, and Enter to open the selected item. The only
//! differences (how the visible set is computed, and what opening produces) are
//! supplied by the caller.

use crossterm::event::{KeyCode, KeyEvent};

use crate::navkey;

#[derive(Clone, Debug)]
pub struct SearchListState<Id> {
    pub selected_id: Option<Id>,
    pub search_query: String,
    pub search_active: bool,
}

impl<Id> Default for SearchListState<Id> {
    fn default() -> Self {
        Self {
            selected_id: None,
            search_query: String::new(),
            search_active: false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum SearchListMsg {
    FocusNext,
    FocusPrev,
    OpenNote,
    ActivateSearch,
    DeactivateSearch,
    SearchInput(char),
    SearchBackspace,
    ClearSearch,
}

/// What the caller should do after a generic update.
pub enum Outcome<Id> {
    Idle,
    /// Open the item with this id (the current selection).
    Open(Id),
}

/// Apply a message to the shared list state. `visible_ids` yields the ids of the
/// currently-visible items (in display order) for a given query — the caller
/// captures its own item set and filtering rule.
pub fn update<Id: Copy + PartialEq>(
    state: &mut SearchListState<Id>,
    visible_ids: impl Fn(&str) -> Vec<Id>,
    msg: SearchListMsg,
) -> Outcome<Id> {
    match msg {
        SearchListMsg::FocusNext => {
            move_selection(state, &visible_ids, true);
            Outcome::Idle
        }
        SearchListMsg::FocusPrev => {
            move_selection(state, &visible_ids, false);
            Outcome::Idle
        }
        SearchListMsg::OpenNote => match state.selected_id {
            Some(id) => Outcome::Open(id),
            None => Outcome::Idle,
        },
        SearchListMsg::ActivateSearch => {
            state.search_active = true;
            Outcome::Idle
        }
        SearchListMsg::DeactivateSearch => {
            state.search_active = false;
            Outcome::Idle
        }
        SearchListMsg::SearchInput(c) => {
            state.search_query.push(c);
            let visible = visible_ids(&state.search_query.clone());
            state.selected_id = visible.first().copied();
            Outcome::Idle
        }
        SearchListMsg::SearchBackspace => {
            state.search_query.pop();
            let visible = visible_ids(&state.search_query.clone());
            if state.selected_id.is_none() {
                state.selected_id = visible.first().copied();
            }
            Outcome::Idle
        }
        SearchListMsg::ClearSearch => {
            state.search_query.clear();
            state.search_active = false;
            Outcome::Idle
        }
    }
}

fn move_selection<Id: Copy + PartialEq>(
    state: &mut SearchListState<Id>,
    visible_ids: &impl Fn(&str) -> Vec<Id>,
    forward: bool,
) {
    let visible = visible_ids(&state.search_query.clone());
    if visible.is_empty() {
        return;
    }
    let current = state
        .selected_id
        .and_then(|id| visible.iter().position(|&x| x == id));
    let next = match (current, forward) {
        (Some(i), true) => (i + 1) % visible.len(),
        (Some(0), false) => visible.len() - 1,
        (Some(i), false) => i - 1,
        (None, true) => 0,
        (None, false) => visible.len() - 1,
    };
    state.selected_id = Some(visible[next]);
}

/// How a key should be treated while a searchable list is focused.
pub enum SearchKey {
    /// Turn into this list message.
    Msg(SearchListMsg),
    /// Not a search key — let normal key handling run.
    Passthrough,
    /// Swallow (typing mode eats unhandled keys).
    Consume,
}

/// Interpret a key against the search state machine. `has_query` is whether a
/// filter query is currently set.
pub fn search_key(code: KeyCode, search_active: bool, has_query: bool) -> SearchKey {
    if search_active {
        match code {
            // Enter still opens the selected item.
            KeyCode::Enter => SearchKey::Passthrough,
            // Esc exits typing mode but keeps the query so the list stays filtered.
            KeyCode::Esc => SearchKey::Msg(SearchListMsg::DeactivateSearch),
            KeyCode::Backspace => SearchKey::Msg(SearchListMsg::SearchBackspace),
            KeyCode::Char(c) => SearchKey::Msg(SearchListMsg::SearchInput(c)),
            _ => SearchKey::Consume,
        }
    } else if has_query && code == KeyCode::Esc {
        SearchKey::Msg(SearchListMsg::ClearSearch)
    } else {
        SearchKey::Passthrough
    }
}

pub fn key_to_msg(key: KeyEvent) -> Option<SearchListMsg> {
    match key.code {
        navkey!(down) => Some(SearchListMsg::FocusNext),
        navkey!(up) => Some(SearchListMsg::FocusPrev),
        KeyCode::Enter => Some(SearchListMsg::OpenNote),
        _ => None,
    }
}
