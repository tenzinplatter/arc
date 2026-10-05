pub mod action_item_builder;
pub mod add_todo_modal;
pub mod create_note_modal;
pub mod epic_list;
pub mod keybinds_panel;
pub mod description_modal;
pub mod navbar;
pub mod notes_list;
pub mod search_bar;
pub mod story_item_builder;
pub mod story_list;
pub mod todos_list;
pub mod iteration_list;

pub use action_item_builder::ActionItemWidget;
pub use epic_list::EpicListView;
pub use story_item_builder::StoryItemWidget;
pub use iteration_list::IterationListView;

use ratatui::{buffer::Buffer, layout::Rect, style::Style, text::Line};

pub fn headed_rows<T, I: IntoIterator<Item = T>>(
    sections: impl IntoIterator<Item = (String, I)>,
) -> Vec<(Option<String>, T)> {
    sections
        .into_iter()
        .flat_map(|(header, items)| {
            items
                .into_iter()
                .enumerate()
                .map(move |(i, item)| ((i == 0).then(|| header.clone()), item))
        })
        .collect()
}

fn divider(width: u16, is_selected: bool) -> Line<'static> {
    let style = if is_selected {
        Style::default().yellow()
    } else {
        Style::default().dark_gray()
    };
    Line::from("─".repeat(width as usize)).style(style)
}

pub fn render_headed_item(
    area: Rect,
    buf: &mut Buffer,
    header: Option<&str>,
    content: &Line,
    is_selected: bool,
) {
    if area.height < 1 {
        return;
    }
    if let Some(header) = header {
        let line = Line::from(format!(" ── {header} ──")).style(Style::default().dark_gray());
        buf.set_line(area.x, area.y, &line, area.width);
    }
    let content_y = area.y + header.is_some() as u16;
    if content_y >= area.bottom() {
        return;
    }
    buf.set_line(area.x, content_y, content, area.width);
    if content_y + 1 < area.bottom() {
        buf.set_line(area.x, content_y + 1, &divider(area.width, is_selected), area.width);
    }
}

pub fn render_name_rows(
    rows: impl IntoIterator<Item = (String, bool)>,
    area: Rect,
    buf: &mut Buffer,
) {
    let row_tops = (area.y..area.bottom()).step_by(2);
    for ((name, is_selected), y) in rows.into_iter().zip(row_tops) {
        let style = if is_selected {
            Style::default().bold()
        } else {
            Style::default()
        };
        buf.set_line(area.x, y, &Line::from(format!("  {name}")).style(style), area.width);
        if y + 1 < area.bottom() {
            buf.set_line(area.x, y + 1, &divider(area.width, is_selected), area.width);
        }
    }
}
