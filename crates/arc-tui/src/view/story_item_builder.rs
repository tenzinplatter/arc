use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Widget,
};

use crate::{api::story::Story, view::render_headed_item};

pub struct StoryItemWidget<'a> {
    story: &'a Story,
    is_active: bool,
    is_selected: bool,
    header: Option<String>,
}

impl<'a> StoryItemWidget<'a> {
    pub fn new(story: &'a Story, is_active: bool, is_selected: bool, header: Option<String>) -> Self {
        Self {
            story,
            is_active,
            is_selected,
            header,
        }
    }

    pub fn height(&self) -> u16 {
        2 + self.header.is_some() as u16
    }

    fn story_line(&self) -> Line<'static> {
        let completed = self.story.completed;

        let active_marker = if self.is_active {
            let color = if completed { Color::DarkGray } else { Color::Green };
            Span::styled("● ", Style::default().fg(color))
        } else {
            Span::raw("  ")
        };

        let id_color = if completed { Color::DarkGray } else { Color::Blue };

        let base_style = if completed {
            Style::default().gray()
        } else {
            Style::default()
        };
        let name_style = if self.is_selected {
            base_style.bold()
        } else {
            base_style
        };

        Line::from(vec![
            active_marker,
            Span::styled(format!("sc-{} ", self.story.id), Style::default().fg(id_color)),
            Span::styled(self.story.name.clone(), name_style),
        ])
    }
}

impl Widget for StoryItemWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        render_headed_item(
            area,
            buf,
            self.header.as_deref(),
            &self.story_line(),
            self.is_selected,
        );
    }
}
