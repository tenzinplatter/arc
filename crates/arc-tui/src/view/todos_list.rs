use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Color, Style},
    symbols::border,
    text::{Line, Span},
    widgets::{Block, Padding, Paragraph, StatefulWidget, Widget, WidgetRef},
};
use tui_widget_list::{ListBuilder, ListState, ListView};

use crate::{
    app::{model::TodosListState, pane::todos_list::visible_sections},
    todos::{Todo, TodoSource},
    view::{headed_rows, render_headed_item},
};

pub struct TodosListView<'a> {
    todos: &'a [Todo],
    state: &'a TodosListState,
}

impl<'a> TodosListView<'a> {
    pub fn new(todos: &'a [Todo], state: &'a TodosListState) -> Self {
        Self { todos, state }
    }
}

impl WidgetRef for TodosListView<'_> {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        let sections = visible_sections(self.state, self.todos);

        if sections.is_empty() {
            let block = Block::bordered().border_set(border::THICK);
            let inner = block.inner(area);
            block.render(area, buf);

            let message = "No todos yet. Press 'n' to add one.";
            let style = Style::default().fg(Color::DarkGray);
            let paragraph = Paragraph::new(message)
                .style(style)
                .alignment(Alignment::Center);

            if inner.height > 0 {
                let centered_area =
                    Rect::new(inner.x, inner.y + inner.height / 2, inner.width, 1);
                paragraph.render(centered_area, buf);
            }
            return;
        }

        let rows = headed_rows(
            sections
                .iter()
                .map(|section| (section.kind.header(), section.todos.iter().copied())),
        );

        let list_block = Block::bordered()
            .border_set(border::THICK)
            .padding(Padding::vertical(1));
        let todos_area = list_block.inner(area);
        list_block.render(area, buf);

        let selected_pos = self
            .state
            .selected_id
            .and_then(|id| rows.iter().position(|(_, todo)| todo.id == id));
        let count = rows.len();

        let builder = ListBuilder::new(move |context| {
            let (header, todo) = &rows[context.index];
            let widget = TodoItemWidget {
                todo,
                is_selected: context.is_selected,
                header: header.clone(),
            };
            let height = widget.height();
            (widget, height)
        });

        let list = ListView::new(builder, count);
        let mut list_state = ListState::default();
        list_state.select(selected_pos);
        StatefulWidget::render(list, todos_area, buf, &mut list_state);
    }
}

struct TodoItemWidget<'a> {
    todo: &'a Todo,
    is_selected: bool,
    header: Option<String>,
}

impl TodoItemWidget<'_> {
    fn height(&self) -> u16 {
        2 + self.header.is_some() as u16
    }

    fn content_line(&self) -> Line<'static> {
        let completed = self.todo.completed;
        let checkbox = if completed { "☑" } else { "☐" };
        let base_style = if completed {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default()
        };
        let name_style = if self.is_selected && !completed {
            base_style.bold()
        } else {
            base_style
        };
        let source_label = match &self.todo.source {
            TodoSource::NoteParsed { file, line, .. } => {
                let stem = file.file_stem().unwrap_or("?");
                Some(Span::styled(
                    format!(" [{}:{}]", stem, line),
                    Style::default().fg(Color::DarkGray),
                ))
            }
            TodoSource::Manual => None,
        };

        [
            Span::styled(format!("{} ", checkbox), base_style),
            Span::styled(self.todo.text.clone(), name_style),
        ]
        .into_iter()
        .chain(source_label)
        .collect()
    }
}

impl Widget for TodoItemWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        render_headed_item(
            area,
            buf,
            self.header.as_deref(),
            &self.content_line(),
            self.is_selected,
        );
    }
}
