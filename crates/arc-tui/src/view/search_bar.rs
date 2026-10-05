use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    widgets::{Block, Paragraph, Widget},
};

use crate::app::pane::searchable_list::SearchListState;

pub fn render_search_bar<Id>(state: &SearchListState<Id>, area: Rect, buf: &mut Buffer) {
    let query = &state.search_query;
    let display = if query.is_empty() && !state.search_active {
        "/ to search".to_string()
    } else {
        format!("{}_", query)
    };
    let bar_style = if state.search_active {
        Style::new().yellow()
    } else {
        Style::new().dark_gray()
    };
    Paragraph::new(display)
        .block(Block::bordered().title(" Search "))
        .style(bar_style)
        .render(area, buf);
}
