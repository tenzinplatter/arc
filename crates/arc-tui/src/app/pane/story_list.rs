use std::collections::HashMap;

use crossterm::event::KeyEvent;

use crate::{
    api::{
        iteration::Iteration,
        story::{Story, get_story_associated_iteration},
    },
    app::{cmd::{Cmd, NoteToOpen}, msg::StoryListMsg, pane::nav},
    keybindings::Key,
};
use arc_core::{IterationId, StoryId, dbg_file};

pub use crate::app::model::StoryListState;

/// Represents a group of stories belonging to the same iteration
struct IterationSection<'a> {
    #[allow(dead_code)]
    iteration: Option<&'a Iteration>,
    stories: Vec<&'a Story>,
}

/// Groups stories by their iteration, sorted by iteration start date
fn group_stories_by_iteration<'a>(
    stories: &'a [Story],
    iterations: Option<&[&'a Iteration]>,
    show_finished: bool,
) -> Vec<IterationSection<'a>> {
    // Build a HashMap grouping stories by iteration_id
    let mut grouped: HashMap<Option<IterationId>, Vec<&'a Story>> = HashMap::new();
    for story in stories {
        // Filter out completed stories if show_finished is false
        if !show_finished && story.completed {
            continue;
        }
        grouped.entry(story.iteration_id).or_default().push(story);
    }

    let mut sections = Vec::new();

    // If we have iterations, sort them by start_date and create sections
    if let Some(iterations) = iterations {
        let mut sorted_iterations: Vec<_> = iterations.to_vec();
        sorted_iterations.sort_by_key(|it| it.start_date);

        for iteration in sorted_iterations {
            if let Some(mut stories) = grouped.remove(&Some(iteration.id)) {
                // Sort: unfinished first, then completed
                stories.sort_by_key(|s| s.completed);

                sections.push(IterationSection {
                    iteration: Some(iteration),
                    stories,
                });
            }
        }
    }

    // Add "No Iteration" section at the end if there are stories without an iteration
    if let Some(mut stories) = grouped.remove(&None) {
        stories.sort_by_key(|s| s.completed);

        sections.push(IterationSection {
            iteration: None,
            stories,
        });
    }

    sections
}

/// Ids of all stories across sections, in display order.
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
    current_iterations: Option<Vec<&Iteration>>,
    msg: StoryListMsg,
) -> Vec<Cmd> {
    match msg {
        StoryListMsg::FocusNext => {
            if stories.is_empty() {
                return vec![Cmd::None];
            }

            // Group stories to handle section boundaries
            let sections = group_stories_by_iteration(stories, current_iterations.as_deref(), state.show_finished);

            if let Some(current_id) = state.selected_story_id {
                state.selected_story_id = next_story_id(current_id, &sections);
            } else {
                // No story selected, select first story in first section
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

            // Group stories to handle section boundaries
            let sections = group_stories_by_iteration(stories, current_iterations.as_deref(), state.show_finished);

            if let Some(current_id) = state.selected_story_id {
                state.selected_story_id = prev_story_id(current_id, &sections);
            } else {
                // No story selected, select last story in last section
                state.selected_story_id =
                    sections.last().and_then(|s| s.stories.last()).map(|s| s.id);
            }

            vec![Cmd::None]
        }

        StoryListMsg::OpenNote => {
            if let Some(story) = get_hovered_story(state, stories) {
                let iteration_app_url = current_iterations
                    .and_then(|iterations| {
                        get_story_associated_iteration(story.iteration_id, iterations)
                    })
                    .map(|it| it.app_url.clone());

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

            // If hiding finished stories and selected story is completed,
            // select first unfinished story
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
