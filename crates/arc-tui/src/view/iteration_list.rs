use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::Style,
    widgets::{Paragraph, Widget, WidgetRef},
};

use crate::{
    api::iteration::Iteration,
    app::{model::IterationListState, pane::iteration_list::visible_split},
    custom_list::LinearList,
    view::search_bar::render_search_bar,
};

pub struct IterationListView<'a> {
    pub active_iterations: &'a [Iteration],
    pub all_iterations: &'a [Iteration],
    pub state: &'a IterationListState,
}

impl<'a> IterationListView<'a> {
    pub fn new(
        active_iterations: &'a [Iteration],
        all_iterations: &'a [Iteration],
        state: &'a IterationListState,
    ) -> IterationListView<'a> {
        IterationListView {
            active_iterations,
            all_iterations,
            state,
        }
    }
}

impl WidgetRef for IterationListView<'_> {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).split(area);

        render_search_bar(self.state, chunks[0], buf);
        let query = &self.state.search_query;

        let content_area = chunks[1];

        let (active_visible, rest_visible) =
            visible_split(self.active_iterations, self.all_iterations, query);

        let active_list_height = (active_visible.len() as u16 * 2 + 4)
            .min(content_area.height.saturating_sub(2) / 2)
            .max(4);

        let section_chunks = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(active_list_height),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(content_area);

        Paragraph::new(" ── Active ──")
            .style(Style::new().dark_gray())
            .render(section_chunks[0], buf);

        let active_empty_msg = if query.is_empty() {
            "No active iterations."
        } else {
            "No results."
        };
        LinearList::new(&active_visible, self.state.selected_id, active_empty_msg)
            .render_ref(section_chunks[1], buf);

        Paragraph::new(" ── All ──")
            .style(Style::new().dark_gray())
            .render(section_chunks[2], buf);

        let all_empty_msg = if query.is_empty() {
            "No other iterations."
        } else {
            "No results."
        };
        LinearList::new(&rest_visible, self.state.selected_id, all_empty_msg)
            .render_ref(section_chunks[3], buf);
    }
}
