use ratatui::{buffer::Buffer, layout::Rect, style::Style, text::Line, widgets::Widget};

pub struct ActionItemWidget {
    label: String,
    is_selected: bool,
    highlight_style: Style,
}

impl ActionItemWidget {
    pub fn new(label: impl Into<String>, is_selected: bool, highlight_style: Style) -> Self {
        Self {
            label: label.into(),
            is_selected,
            highlight_style,
        }
    }

    pub fn height(&self) -> u16 {
        1
    }
}

impl Widget for ActionItemWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 1 {
            return;
        }

        if self.is_selected {
            for x in area.x..area.x + area.width {
                buf[(x, area.y)].set_style(self.highlight_style);
            }
        }

        let line = Line::from(self.label).centered();
        buf.set_line(area.x, area.y, &line, area.width);
    }
}
