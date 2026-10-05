use std::collections::HashMap;

use crossterm::event::KeyEvent;

use crate::{
    api::{
        iteration::Iteration,
        story::{Story, iteration_of},
    },
    app::{cmd::{Cmd, NoteToOpen}, msg::StoryListMsg, pane::nav},
    keybindings::Key,
};
use arc_core::{IterationId, StoryId, dbg_file};

pub use crate::app::model::StoryListState;

pub struct IterationSection<'a> {
    pub iteration: Option<&'a Iteration>,
    pub stories: Vec<&'a Story>,
}

pub fn group_stories_by_iteration<'a>(
    stories: &'a [Story],
    iterations: &'a [Iteration],
    show_finished: bool,
) -> Vec<IterationSection<'a>> {
    let mut grouped: HashMap<Option<IterationId>, Vec<&'a Story>> = HashMap::new();
    for story in stories.iter().filter(|s| show_finished || !s.completed) {
        grouped.entry(story.iteration_id).or_default().push(story);
    }

    let mut sorted_iterations: Vec<&Iteration> = iterations.iter().collect();
    sorted_iterations.sort_by_key(|it| it.start_date);

    let mut sections = Vec::new();
    for iteration in sorted_iterations {
        if let Some(mut stories) = grouped.remove(&Some(iteration.id)) {
            stories.sort_by_key(|s| s.completed);
            sections.push(IterationSection {
                iteration: Some(iteration),
                stories,
            });
        }
    }

    if let Some(mut stories) = grouped.remove(&None) {
        stories.sort_by_key(|s| s.completed);
        sections.push(IterationSection {
            iteration: None,
            stories,
        });
    }

    sections
}

fn ordered_ids(sections: &[IterationSection]) -> Vec<StoryId> {
    sections
        .iter()
        .flat_map(|s| s.stories.iter().map(|s| s.id))
        .collect()
}

fn next_story_id(current_story_id: StoryId, sections: &[IterationSection]) -> Option<StoryId> {
    nav::step_wrapping(&ordered_ids(sections), current_story_id, true)
}

fn prev_story_id(current_story_id: StoryId, sections: &[IterationSection]) -> Option<StoryId> {
    nav::step_wrapping(&ordered_ids(sections), current_story_id, false)
}

pub fn update(
    state: &mut StoryListState,
    stories: &[Story],
    current_iterations: &[Iteration],
    msg: StoryListMsg,
) -> Vec<Cmd> {
    match msg {
        StoryListMsg::FocusNext => {
            if stories.is_empty() {
                return vec![Cmd::None];
            }

            let sections = group_stories_by_iteration(stories, current_iterations, state.show_finished);

            if let Some(current_id) = state.selected_story_id {
                state.selected_story_id = next_story_id(current_id, &sections);
            } else {
                state.selected_story_id = sections
                    .first()
                    .and_then(|s| s.stories.first())
                    .map(|s| s.id);
            }

            vec![Cmd::None]
        }

        StoryListMsg::FocusPrev => {
            if stories.is_empty() {
                return vec![Cmd::None];
            }

            let sections = group_stories_by_iteration(stories, current_iterations, state.show_finished);

            if let Some(current_id) = state.selected_story_id {
                state.selected_story_id = prev_story_id(current_id, &sections);
            } else {
                state.selected_story_id =
                    sections.last().and_then(|s| s.stories.last()).map(|s| s.id);
            }

            vec![Cmd::None]
        }

        StoryListMsg::OpenNote => {
            if let Some(story) = get_hovered_story(state, stories) {
                let iteration_app_url =
                    iteration_of(&story, current_iterations).map(|it| it.app_url.clone());

                return vec![Cmd::OpenNote(NoteToOpen::story(&story, iteration_app_url))];
            }
            vec![Cmd::None]
        }

        StoryListMsg::SelectStory => {
            let story = get_hovered_story(state, stories);
            dbg_file!("Setting story: {:?} to active", story);
            vec![Cmd::SelectStory(story), Cmd::WriteCache]
        }

        StoryListMsg::TmuxEnter => {
            if let Some(story) = get_hovered_story(state, stories) {
                vec![Cmd::OpenTmuxSession {
                    story_name: story.name.clone(),
                }]
            } else {
                vec![Cmd::None]
            }
        }

        StoryListMsg::EditStoryContents => {
            if let Some(story) = get_hovered_story(state, stories) {
                vec![Cmd::EditStoryContent {
                    story_id: story.id,
                    description: story.description.clone(),
                }]
            } else {
                vec![Cmd::None]
            }
        }

        StoryListMsg::OpenInBrowser => {
            if let Some(story) = get_hovered_story(state, stories) {
                vec![Cmd::OpenInBrowser {
                    app_url: story.app_url.clone(),
                }]
            } else {
                vec![Cmd::None]
            }
        }

        StoryListMsg::ToggleFinished => {
            state.show_finished = !state.show_finished;

            if !state.show_finished
                && let Some(selected_id) = state.selected_story_id
                && let Some(selected_story) = stories.iter().find(|s| s.id == selected_id)
                && selected_story.completed
            {
                state.selected_story_id = stories.iter()
                    .find(|s| !s.completed)
                    .map(|s| s.id);
            }

            vec![Cmd::None]
        }
    }
}

fn get_hovered_story(state: &StoryListState, stories: &[Story]) -> Option<Story> {
    let id = state.selected_story_id?;
    stories.iter().find(|s| s.id == id).cloned()
}

pub fn key_to_msg(key: KeyEvent) -> Option<StoryListMsg> {
    match Key::from_key_event(key)? {
        Key::FocusNext => Some(StoryListMsg::FocusNext),
        Key::FocusPrev => Some(StoryListMsg::FocusPrev),
        Key::ToggleFinished => Some(StoryListMsg::ToggleFinished),
        Key::OpenBrowser => Some(StoryListMsg::OpenInBrowser),
        Key::OpenNote => Some(StoryListMsg::OpenNote),
        Key::SelectStory => Some(StoryListMsg::SelectStory),
        Key::EditDescription => Some(StoryListMsg::EditStoryContents),
        Key::Tmux => Some(StoryListMsg::TmuxEnter),
        _ => None,
    }
}
