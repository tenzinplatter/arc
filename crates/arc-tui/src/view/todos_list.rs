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
    app::{
        model::TodosListState,
        pane::todos_list::group_todos_by_section,
    },
    todos::Todo,
};
use arc_core::time::today;

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
        let visible: Vec<Todo> = self
            .todos
            .iter()
            .filter(|t| self.state.is_visible(t))
            .cloned()
            .collect();

        if visible.is_empty() {
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

        let today = today();
        let sections = group_todos_by_section(&visible, today);

        struct Row {
            todo: Todo,
            header: Option<String>,
        }
        let mut rows: Vec<Row> = Vec::new();
        for section in &sections {
            let header = section.kind.header().to_string();
            for (i, todo) in section.todos.iter().enumerate() {
                rows.push(Row {
                    todo: todo.clone(),
                    header: (i == 0).then(|| header.clone()),
                });
            }
        }

        let list_block = Block::bordered()
            .border_set(border::THICK)
            .padding(Padding::vertical(1));
        let todos_area = list_block.inner(area);
        list_block.render(area, buf);

        let selected_pos = self
            .state
            .selected_id
            .and_then(|id| rows.iter().position(|r| r.todo.id == id));
        let count = rows.len();

        let builder = ListBuilder::new(move |context| {
            let row = &rows[context.index];
            let todo = &row.todo;
            let is_selected = context.is_selected;
            let source_label = match &todo.source {
                crate::todos::TodoSource::NoteParsed { file, line, .. } => {
                    let stem = file.file_stem().unwrap_or("?");
                    Some(format!(" [{}:{}]", stem, line))
                }
                crate::todos::TodoSource::Manual => None,
            };
            let widget = TodoItemWidget {
                text: todo.text.clone(),
                completed: todo.completed,
                is_selected,
                source_label,
                header: row.header.clone(),
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

struct TodoItemWidget {
    text: String,
    completed: bool,
    is_selected: bool,
    source_label: Option<String>,
    header: Option<String>,
}

impl TodoItemWidget {
    fn height(&self) -> u16 {
        2 + self.header.is_some() as u16
    }
}

impl Widget for TodoItemWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 1 {
            return;
        }

        let mut y = area.y;
        if let Some(header) = &self.header {
            let line = Line::from(format!(" ── {header} ──")).style(Style::default().fg(Color::DarkGray));
            buf.set_line(area.x, y, &line, area.width);
            y += 1;
        }
        if y >= area.y + area.height {
            return;
        }

        let checkbox = if self.completed { "☑" } else { "☐" };

        let mut spans = Vec::new();

        let base_style = if self.completed {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default()
        };

        let name_style = if self.is_selected && !self.completed {
            base_style.bold()
        } else {
            base_style
        };

        spans.push(Span::styled(format!("{} ", checkbox), base_style));
        spans.push(Span::styled(self.text.clone(), name_style));

        if let Some(label) = &self.source_label {
            spans.push(Span::styled(
                label.clone(),
                Style::default().fg(Color::DarkGray),
            ));
        }

        let content = Line::from(spans);
        buf.set_line(area.x, y, &content, area.width);
        y += 1;

        if y < area.y + area.height {
            let divider_style = if self.is_selected {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let divider = Line::from("─".repeat(area.width as usize)).style(divider_style);
            buf.set_line(area.x, y, &divider, area.width);
        }
    }
}
