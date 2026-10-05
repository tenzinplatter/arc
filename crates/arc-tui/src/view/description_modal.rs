use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect, Size},
    style::{Modifier, Style},
    text::{Line, Text},
    widgets::{Block, BorderType, Paragraph, StatefulWidget, Widget, Wrap},
};
use tui_scrollview::{ScrollView, ScrollViewState};

use crate::api::story::Story;

pub struct DescriptionModal<'a> {
    story: &'a Story,
}

impl<'a> DescriptionModal<'a> {
    pub fn new(story: &'a Story) -> Self {
        Self { story }
    }
}

impl StatefulWidget for DescriptionModal<'_> {
    type State = ScrollViewState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let [title_area, top_divider_area, content_area, bottom_divider_area, _footer_area] =
            Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .margin(1)
            .areas(area);

        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .title_bottom(" j/k scroll • g/G top/bottom • q close ");

        block.render(area, buf);

        let title = Line::from(self.story.name.as_str())
            .style(Style::default().add_modifier(Modifier::BOLD));
        buf.set_line(title_area.x, title_area.y, &title, title_area.width);

        let divider = "─".repeat(top_divider_area.width as usize);
        buf.set_string(top_divider_area.x, top_divider_area.y, &divider, Style::default());

        let trimmed = self.story.description.trim();
        let description = if trimmed.is_empty() {
            "No description".to_string()
        } else {
            trimmed.to_string()
        };

        let paragraph = Paragraph::new(Text::from(description)).wrap(Wrap { trim: false });

        let content_width = content_area.width;
        let total_lines = paragraph.line_count(content_width) as u16;

        let mut scroll_view = ScrollView::new(Size::new(content_width, total_lines));
        scroll_view.render_widget(paragraph, Rect::new(0, 0, content_width, total_lines));
        scroll_view.render(content_area, buf, state);

        buf.set_string(bottom_divider_area.x, bottom_divider_area.y, &divider, Style::default());
    }
}

pub fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    // u32 math: `area.width * percent_x` overflows u16 past ~820 cols.
    let width = (area.width as u32 * percent_x as u32 / 100) as u16;
    let height = (area.height as u32 * percent_y as u32 / 100) as u16;
    let x = (area.width - width) / 2;
    let y = (area.height - height) / 2;

    Rect::new(x, y, width, height)
}
