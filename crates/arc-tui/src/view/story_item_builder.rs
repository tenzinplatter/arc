use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Widget,
};

use crate::api::story::Story;

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
}

impl Widget for StoryItemWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 1 {
            return;
        }

        let mut y = area.y;
        if let Some(header) = &self.header {
            let line = Line::from(format!(" ── {header} ──")).style(Style::default().dark_gray());
            buf.set_line(area.x, y, &line, area.width);
            y += 1;
        }
        if y >= area.y + area.height {
            return;
        }

        let content = self.render_story_line();
        buf.set_line(area.x, y, &content, area.width);
        y += 1;

        if y < area.y + area.height {
            let divider_style = if self.is_selected {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().dark_gray()
            };
            let divider = Line::from("─".repeat(area.width as usize)).style(divider_style);
            buf.set_line(area.x, y, &divider, area.width);
        }
    }
}

impl StoryItemWidget<'_> {
    fn render_story_line(&self) -> Line<'static> {
        let completed = self.story.completed;
        let mut spans = Vec::new();

        let base_style = if completed {
            Style::default().gray()
        } else {
            Style::default()
        };

        if self.is_active {
            let color = if completed { Color::DarkGray } else { Color::Green };
            spans.push(Span::styled("● ", Style::default().fg(color)));
        } else {
            spans.push(Span::raw("  "));
        }

        let id_color = if completed { Color::DarkGray } else { Color::Blue };
        spans.push(Span::styled(
            format!("sc-{} ", self.story.id),
            Style::default().fg(id_color),
        ));

        let name_style = if self.is_selected {
            base_style.bold()
        } else {
            base_style
        };
        spans.push(Span::styled(self.story.name.clone(), name_style));

        Line::from(spans)
    }
}
