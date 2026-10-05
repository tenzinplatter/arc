use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, BorderType, Paragraph, Widget, WidgetRef},
};

use crate::view::description_modal::centered_rect;

pub struct AddTodoModal<'a> {
    input: &'a str,
}

impl<'a> AddTodoModal<'a> {
    pub fn new(input: &'a str) -> Self {
        Self { input }
    }
}

impl WidgetRef for AddTodoModal<'_> {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        let popup_area = centered_rect(50, 30, area);

        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .title(" New Todo ");

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        let [_, label_area, input_area, _, hint_area] =
            Layout::vertical([Constraint::Length(1); 5]).areas(inner);

        let label = Line::from("Todo:").style(Style::default().add_modifier(Modifier::BOLD));
        buf.set_line(label_area.x, label_area.y, &label, label_area.width);

        let input_line = Line::from(format!("{}_", self.input));
        buf.set_line(input_area.x, input_area.y, &input_line, input_area.width);

        let hint =
            Paragraph::new("Enter to add  Esc to cancel").style(Style::default().dark_gray());
        hint.render(hint_area, buf);
    }
}
