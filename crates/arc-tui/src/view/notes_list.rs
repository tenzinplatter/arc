use arc_notes::{NoteDir, NoteRelPath};
use chrono::NaiveDate;
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Layout, Rect},
    style::Style,
    symbols::border,
    text::Line,
    widgets::{Block, Padding, Paragraph, Widget, WidgetRef},
};

use crate::app::model::NotesListState;

pub struct NotesListView<'a> {
    state: &'a NotesListState,
}

impl<'a> NotesListView<'a> {
    pub fn new(state: &'a NotesListState) -> Self {
        Self { state }
    }
}

fn format_daily_name(stem: &str) -> Option<String> {
    let date = NaiveDate::parse_from_str(stem, "%Y-%m-%d").ok()?;
    Some(date.format("%a, %b %-d %Y").to_string())
}

fn format_slug(stem: &str) -> String {
    stem.split('-')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(c) => {
                    let upper: String = c.to_uppercase().collect();
                    format!("{}{}", upper, chars.as_str())
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn section_title(dir: NoteDir) -> &'static str {
    match dir {
        NoteDir::Daily => "Daily Notes",
        NoteDir::Stories => "Story Notes",
        NoteDir::Iterations => "Iteration Notes",
        NoteDir::Epics => "Epic Notes",
        NoteDir::Scratch => "Scratch Notes",
    }
}

fn display_name(path: &NoteRelPath, dir: NoteDir) -> String {
    let stem = match path.file_stem() {
        Some(s) => s,
        None => return "???".to_string(),
    };

    if dir == NoteDir::Daily
        && let Some(formatted) = format_daily_name(stem)
    {
        return formatted;
    }

    format_slug(stem)
}

impl<'a> WidgetRef for NotesListView<'a> {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        if self.state.is_empty() {
            let block = Block::bordered().border_set(border::THICK);
            let inner = block.inner(area);
            block.render(area, buf);

            let message = "No notes found.";
            let style = Style::default().gray();
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

        let sections: Vec<(NoteDir, &[NoteRelPath])> = self
            .state
            .sections()
            .filter(|(_, notes)| !notes.is_empty())
            .collect();

        let n = sections.len() as u32;
        let outer_constraints: Vec<Constraint> = (0..n).map(|_| Constraint::Fill(1)).collect();
        let section_chunks = Layout::vertical(outer_constraints).split(area);

        for (idx, (dir, notes)) in sections.iter().enumerate() {
            let section_area = section_chunks[idx];

            let inner_chunks = Layout::vertical([
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .split(section_area);

            let header_style = Style::default().dark_gray();
            let display = format!(" ── {} ──", section_title(*dir));
            let title_line = Line::from(display).style(header_style);
            buf.set_line(inner_chunks[0].x, inner_chunks[0].y, &title_line, inner_chunks[0].width);

            let list_block = Block::bordered()
                .border_set(border::THICK)
                .padding(Padding::vertical(1));
            let items_area = list_block.inner(inner_chunks[1]);
            list_block.render(inner_chunks[1], buf);

            let visible_items = (items_area.height / 2) as usize;
            let sel_idx = self.state.selected_path.as_ref()
                .and_then(|sel| notes.iter().position(|p| p == sel));
            let scroll = if let Some(sel) = sel_idx {
                let max_scroll = notes.len().saturating_sub(visible_items.max(1));
                sel.saturating_sub(visible_items.saturating_sub(1)).min(max_scroll)
            } else {
                0
            };

            let mut y = items_area.y;
            for note_path in notes.iter().skip(scroll) {
                if y >= items_area.y + items_area.height {
                    break;
                }

                let is_selected = self.state.selected_path.as_ref() == Some(note_path);
                let name = display_name(note_path, *dir);

                let name_style = if is_selected {
                    Style::default().bold()
                } else {
                    Style::default()
                };
                let name_line = Line::from(format!("  {}", name)).style(name_style);
                buf.set_line(items_area.x, y, &name_line, items_area.width);
                y += 1;

                if y < items_area.y + items_area.height {
                    let divider_style = if is_selected {
                        Style::default().yellow()
                    } else {
                        Style::default().dark_gray()
                    };
                    let divider = "─".repeat(items_area.width as usize);
                    let divider_line = Line::from(divider).style(divider_style);
                    buf.set_line(items_area.x, y, &divider_line, items_area.width);
                    y += 1;
                }
            }
        }
    }
}
