use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Widget, WidgetRef},
};

use crate::app::model::{LoadingState, ViewType};

const SPINNER_CHARS: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

pub struct NavBar {
    active_view: ViewType,
    loading: LoadingState,
    has_stories: bool,
    tick: usize,
}

impl NavBar {
    pub fn new(
        active_view: ViewType,
        loading: LoadingState,
        has_stories: bool,
        tick: usize,
    ) -> Self {
        Self {
            active_view,
            loading,
            has_stories,
            tick,
        }
    }

    fn spinner_char(&self) -> char {
        SPINNER_CHARS[self.tick % SPINNER_CHARS.len()]
    }
}

impl WidgetRef for NavBar {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        let block = Block::bordered();
        let inner = block.inner(area);
        block.render(area, buf);

        let mut spans = Vec::new();

        for (i, view_type) in ViewType::ALL.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw(" │ "));
            }

            let label = view_type.label();
            let span = if *view_type == self.active_view {
                Span::styled(
                    label,
                    Style::default()
                        .bg(Color::White)
                        .fg(Color::Black)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::raw(label)
            };

            spans.push(span);
        }

        let line = Line::from(spans);
        let paragraph = Paragraph::new(line);
        paragraph.render(inner, buf);

        if self.loading.is_loading() && self.has_stories {
            let loading_text = format!("{} {}", self.spinner_char(), self.loading.label());
            let loading_span = Span::styled(loading_text.clone(), Style::default().gray());
            // Char count, not byte len: the spinner glyph is multi-byte.
            let loading_width = loading_text.chars().count() as u16;

            if inner.width > loading_width {
                let loading_area = Rect::new(
                    inner.x + inner.width - loading_width,
                    inner.y,
                    loading_width,
                    1,
                );
                let loading_paragraph = Paragraph::new(Line::from(loading_span));
                loading_paragraph.render(loading_area, buf);
            }
        }
    }
}
