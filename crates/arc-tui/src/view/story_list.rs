use std::collections::HashMap;

use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::Style,
    symbols::border,
    widgets::{Block, Padding, Paragraph, StatefulWidget, Widget, WidgetRef},
};
use tui_widget_list::{ListBuilder, ListState, ListView};

use crate::{
    api::{iteration::Iteration, story::Story},
    app::model::{LoadingState, StoryListState},
};

use super::story_item_builder::StoryItemWidget;

const SPINNER_CHARS: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Represents a group of stories belonging to the same iteration
pub struct IterationSection<'a> {
    pub iteration: Option<&'a Iteration>,
    pub stories: Vec<&'a Story>,
}

/// Groups stories by their iteration, sorted by iteration start date
fn group_stories_by_iteration<'a>(
    stories: &'a [Story],
    iterations: Option<&'a [Iteration]>,
    show_finished: bool,
) -> Vec<IterationSection<'a>> {
    // Build a HashMap grouping stories by iteration_id
    let mut grouped: HashMap<Option<arc_core::IterationId>, Vec<&'a Story>> = HashMap::new();
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
        let mut sorted_iterations: Vec<_> = iterations.iter().collect();
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

pub struct StoryListView<'a> {
    stories: &'a [Story],
    iterations: Option<&'a [Iteration]>,
    state: &'a StoryListState,
    active_story: Option<&'a Story>,
    loading: LoadingState,
    tick: usize,
}

impl<'a> StoryListView<'a> {
    pub fn new(
        stories: &'a [Story],
        iterations: Option<&'a [Iteration]>,
        state: &'a StoryListState,
        active_story: Option<&'a Story>,
        loading: LoadingState,
        tick: usize,
    ) -> Self {
        Self {
            stories,
            iterations,
            state,
            active_story,
            loading,
            tick,
        }
    }

    fn spinner_char(&self) -> char {
        SPINNER_CHARS[self.tick % SPINNER_CHARS.len()]
    }
}

impl<'a> WidgetRef for StoryListView<'a> {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        // Handle loading and empty states with a single bordered block
        if self.stories.is_empty() {
            let block = Block::bordered().border_set(border::THICK);
            let inner = block.inner(area);
            block.render(area, buf);

            let message = if self.loading.is_loading() {
                format!("{} {}", self.spinner_char(), self.loading.label())
            } else {
                "No stories assigned in this iteration.".to_string()
            };

            let style = Style::default().gray();
            let paragraph = Paragraph::new(message)
                .style(style)
                .alignment(Alignment::Center);

            if inner.height > 0 {
                let centered_area = Rect::new(inner.x, inner.y + inner.height / 2, inner.width, 1);
                paragraph.render(centered_area, buf);
            }
            return;
        }

        // Group stories by iteration, then flatten to one list. The first
        // story of each group carries its section header, so a single
        // `ListView` scrolls the whole thing and keeps the selection visible.
        let sections = group_stories_by_iteration(self.stories, self.iterations, self.state.show_finished);

        struct Row<'a> {
            story: &'a Story,
            header: Option<String>,
        }
        let mut rows: Vec<Row> = Vec::new();
        for section in &sections {
            let header = match section.iteration {
                Some(it) => it.name.clone(),
                None => "No Iteration".to_string(),
            };
            for (i, story) in section.stories.iter().enumerate() {
                rows.push(Row {
                    story,
                    header: (i == 0).then(|| header.clone()),
                });
            }
        }

        let list_block = Block::bordered()
            .border_set(border::THICK)
            .padding(Padding::vertical(1));
        let stories_area = list_block.inner(area);
        list_block.render(area, buf);

        let selected_pos = self
            .state
            .selected_story_id
            .and_then(|id| rows.iter().position(|r| r.story.id == id));

        let active_story = self.active_story;
        let width = stories_area.width;
        let count = rows.len();

        let builder = ListBuilder::new(move |context| {
            let row = &rows[context.index];
            let is_active = active_story.is_some_and(|a| a.id == row.story.id);
            let widget = StoryItemWidget::new(
                row.story,
                is_active,
                context.is_selected,
                width,
                row.story.completed,
                row.header.clone(),
            );
            let height = widget.height();
            (widget, height)
        });

        let list = ListView::new(builder, count);
        let mut list_state = ListState::default();
        list_state.select(selected_pos);
        StatefulWidget::render(list, stories_area, buf, &mut list_state);
    }
}
