use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::Style,
    widgets::{Block, Paragraph, Widget, WidgetRef},
};

use crate::{
    api::iteration::Iteration,
    app::{model::IterationListState, pane::iteration_list::filter_items},
    custom_list::LinearList,
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

        // Search bar
        let query = &self.state.search_query;
        let display = if query.is_empty() && !self.state.search_active {
            "/ to search".to_string()
        } else {
            format!("{}_", query)
        };
        let bar_style = if self.state.search_active {
            Style::new().yellow()
        } else {
            Style::new().dark_gray()
        };
        Paragraph::new(display)
            .block(Block::bordered().title(" Search "))
            .style(bar_style)
            .render(chunks[0], buf);

        let content_area = chunks[1];

        // Build filtered lists for each section
        let active_visible: Vec<Iteration> = filter_items(self.active_iterations, query)
            .into_iter()
            .cloned()
            .collect();

        use std::collections::HashSet;
        let active_ids: HashSet<arc_core::IterationId> = active_visible.iter().map(|it| it.id).collect();
        let rest_visible: Vec<Iteration> = filter_items(self.all_iterations, query)
            .into_iter()
            .filter(|it| !active_ids.contains(&it.id))
            .cloned()
            .collect();

        // Active section height: border (2) + 2 lines per item - 1 (no divider on last item), capped at half the content area
        let active_list_height = (active_visible.len() as u16 * 2 + 4)
            .min(content_area.height.saturating_sub(2) / 2)
            .max(4); // minimum: border + empty message

        let section_chunks = Layout::vertical([
            Constraint::Length(1),                  // "Active" header
            Constraint::Length(active_list_height), // Active LinearList
            Constraint::Length(1),                  // "All" header
            Constraint::Min(0),                     // All LinearList
        ])
        .split(content_area);

        // Section headers
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
