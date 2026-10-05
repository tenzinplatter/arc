use std::collections::HashSet;

use crossterm::event::KeyEvent;
use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};

use crate::api::iteration::Iteration;
use arc_core::IterationId;
use crate::app::cmd::{Cmd, NoteToOpen};
use crate::app::model::IterationListState;
use crate::app::msg::IterationListMsg;
use crate::app::pane::searchable_list::{self, Outcome};

pub fn filter_items<'a>(items: &'a [Iteration], query: &str) -> Vec<&'a Iteration> {
    if query.is_empty() {
        return items.iter().collect();
    }
    let matcher = SkimMatcherV2::default();
    items
        .iter()
        .filter(|it| matcher.fuzzy_match(&it.name, query).is_some())
        .collect()
}

pub fn visible_split<'a>(
    current: &'a [Iteration],
    all: &'a [Iteration],
    query: &str,
) -> (Vec<&'a Iteration>, Vec<&'a Iteration>) {
    let active_filtered = filter_items(current, query);
    let active_ids: HashSet<IterationId> = active_filtered.iter().map(|it| it.id).collect();
    let rest_filtered = filter_items(all, query)
        .into_iter()
        .filter(|it| !active_ids.contains(&it.id))
        .collect();
    (active_filtered, rest_filtered)
}

fn combined_visible_ids(current: &[Iteration], all: &[Iteration], query: &str) -> Vec<IterationId> {
    let (active, rest) = visible_split(current, all, query);
    active.into_iter().chain(rest).map(|it| it.id).collect()
}

pub fn update(
    state: &mut IterationListState,
    current_iterations: &[Iteration],
    all_iterations: &[Iteration],
    msg: IterationListMsg,
) -> Vec<Cmd> {
    let outcome = searchable_list::update(
        state,
        |query| combined_visible_ids(current_iterations, all_iterations, query),
        msg,
    );
    match outcome {
        Outcome::Open(id) => {
            let selected = current_iterations
                .iter()
                .find(|it| it.id == id)
                .or_else(|| all_iterations.iter().find(|it| it.id == id));
            match selected {
                Some(iteration) => vec![Cmd::OpenNote(NoteToOpen::iteration(iteration))],
                None => vec![Cmd::None],
            }
        }
        Outcome::Idle => vec![Cmd::None],
    }
}

pub fn key_to_msg(key: KeyEvent) -> Option<IterationListMsg> {
    searchable_list::key_to_msg(key)
}
