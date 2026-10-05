use crossterm::event::KeyEvent;
use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};

use crate::api::epic::EpicSlim;
use crate::app::cmd::{Cmd, NoteToOpen};
use crate::app::model::EpicListState;
use crate::app::msg::EpicListMsg;
use crate::app::pane::searchable_list::{self, Outcome};

pub fn filter_items<'a>(items: &'a [EpicSlim], query: &str) -> Vec<&'a EpicSlim> {
    if query.is_empty() {
        return items.iter().collect();
    }
    let matcher = SkimMatcherV2::default();
    items
        .iter()
        .filter(|e| matcher.fuzzy_match(&e.name, query).is_some())
        .collect()
}

pub fn update(state: &mut EpicListState, epics: &[EpicSlim], msg: EpicListMsg) -> Vec<Cmd> {
    let outcome = searchable_list::update(
        state,
        |query| filter_items(epics, query).iter().map(|e| e.id).collect(),
        msg,
    );
    match outcome {
        Outcome::Open(id) => match epics.iter().find(|e| e.id == id) {
            Some(epic) => vec![Cmd::OpenNote(NoteToOpen::epic(epic))],
            None => vec![Cmd::None],
        },
        Outcome::Idle => vec![Cmd::None],
    }
}

pub fn key_to_msg(key: KeyEvent) -> Option<EpicListMsg> {
    searchable_list::key_to_msg(key)
}
