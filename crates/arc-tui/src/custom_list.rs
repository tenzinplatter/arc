use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::Style,
    symbols::border,
    text::Line,
    widgets::{Block, Padding, Paragraph, Widget, WidgetRef},
};

/// Items that can appear in a `LinearList` must expose an id and a display label.
pub trait LinearListItem {
    type Id: Copy + Eq;
    fn id(&self) -> Self::Id;
    fn label(&self) -> &str;
}

impl LinearListItem for arc_shortcut::api::iteration::Iteration {
    type Id = arc_core::IterationId;
    fn id(&self) -> Self::Id { self.id }
    fn label(&self) -> &str { &self.name }
}

impl LinearListItem for arc_shortcut::api::epic::EpicSlim {
    type Id = arc_core::EpicId;
    fn id(&self) -> Self::Id { self.id }
    fn label(&self) -> &str { &self.name }
}

/// A simple, bordered linear list widget with selection highlighting and dividers.
pub struct LinearList<'a, T: LinearListItem> {
    pub items: &'a [T],
    pub selected_id: Option<T::Id>,
    pub empty_message: &'a str,
}

impl<'a, T: LinearListItem> LinearList<'a, T> {
    pub fn new(items: &'a [T], selected_id: Option<T::Id>, empty_message: &'a str) -> Self {
        LinearList {
            items,
            selected_id,
            empty_message,
        }
    }
}

impl<T: LinearListItem> WidgetRef for LinearList<'_, T> {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        let block = Block::bordered()
            .border_set(border::THICK)
            .padding(Padding::vertical(1));
        let inner = block.inner(area);
        block.render(area, buf);

        if self.items.is_empty() {
            let paragraph = Paragraph::new(self.empty_message)
                .style(Style::default().gray())
                .alignment(Alignment::Center);

            if inner.height > 0 {
                let centered = Rect::new(inner.x, inner.y + inner.height / 2, inner.width, 1);
                paragraph.render(centered, buf);
            }
            return;
        }

        // Each item is 2 rows (label + divider). Scroll so the selected item
        // stays on screen instead of clipping off the bottom.
        const PER_ITEM: u16 = 2;
        let visible = (inner.height / PER_ITEM).max(1) as usize;
        let selected_idx = self
            .selected_id
            .and_then(|id| self.items.iter().position(|it| it.id() == id));
        let offset = match selected_idx {
            Some(i) if i >= visible => i + 1 - visible,
            _ => 0,
        };

        let mut y = inner.y;
        for item in self.items.iter().skip(offset) {
            if y >= inner.y + inner.height {
                break;
            }

            let is_selected = self.selected_id == Some(item.id());
            let name_style = if is_selected {
                Style::default().bold()
            } else {
                Style::default()
            };
            let name_line = Line::from(format!("  {}", item.label())).style(name_style);
            buf.set_line(inner.x, y, &name_line, inner.width);
            y += 1;

            if y < inner.y + inner.height {
                let divider_style = if is_selected {
                    Style::default().yellow()
                } else {
                    Style::default().dark_gray()
                };
                let divider = Line::from("─".repeat(inner.width as usize)).style(divider_style);
                buf.set_line(inner.x, y, &divider, inner.width);
                y += 1;
            }
        }
    }
}
