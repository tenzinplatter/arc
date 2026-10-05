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
    app::{
        model::{LoadingState, StoryListState},
        pane::story_list::group_stories_by_iteration,
    },
};

use super::{headed_rows, story_item_builder::StoryItemWidget};

const SPINNER_CHARS: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

pub struct StoryListView<'a> {
    stories: &'a [Story],
    iterations: &'a [Iteration],
    state: &'a StoryListState,
    active_story: Option<&'a Story>,
    loading: LoadingState,
    tick: usize,
}

impl<'a> StoryListView<'a> {
    pub fn new(
        stories: &'a [Story],
        iterations: &'a [Iteration],
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

        let sections = group_stories_by_iteration(self.stories, self.iterations, self.state.show_finished);

        let rows = headed_rows(sections.iter().map(|section| {
            let header = match section.iteration {
                Some(it) => it.name.clone(),
                None => "No Iteration".to_string(),
            };
            (header, section.stories.iter().copied())
        }));

        let list_block = Block::bordered()
            .border_set(border::THICK)
            .padding(Padding::vertical(1));
        let stories_area = list_block.inner(area);
        list_block.render(area, buf);

        let selected_pos = self
            .state
            .selected_story_id
            .and_then(|id| rows.iter().position(|(_, story)| story.id == id));

        let active_story = self.active_story;
        let count = rows.len();

        let builder = ListBuilder::new(move |context| {
            let (header, story) = &rows[context.index];
            let is_active = active_story.is_some_and(|a| a.id == story.id);
            let widget =
                StoryItemWidget::new(story, is_active, context.is_selected, header.clone());
            let height = widget.height();
            (widget, height)
        });

        let list = ListView::new(builder, count);
        let mut list_state = ListState::default();
        list_state.select(selected_pos);
        StatefulWidget::render(list, stories_area, buf, &mut list_state);
    }
}
