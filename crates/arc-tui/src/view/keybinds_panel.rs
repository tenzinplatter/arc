use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Widget};

pub struct KeybindsPanel;

const LINES: &[&str] = &[
    " Global",
    "  ?              Toggle help",
    "  q / Ctrl+C     Quit",
    "  j/k / ↑↓       Navigate",
    "  Tab / L        Next view",
    "  BackTab / H    Prev view",
    "  d              Open daily note",
    "─────────────────────────────────────",
    " Story List",
    "  Space          Show description",
    "  Enter          Action menu",
    "  n              Open story note",
    "  i              Iteration note",
    "  o              Open in browser",
    "  e              Edit description",
    "  t              Tmux session",
    "  a              Select active story",
    "  f              Toggle finished",
    "─────────────────────────────────────",
    " Notes",
    "  Enter          Open note",
    "─────────────────────────────────────",
    "       ? / Esc / q  close",
];

const PANEL_WIDTH: u16 = 43;
const BORDERS: u16 = 2;

impl Widget for KeybindsPanel {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let panel_height = (LINES.len() as u16 + BORDERS).min(area.height);
        let panel_width = PANEL_WIDTH.min(area.width);

        let x = area.x + (area.width.saturating_sub(panel_width)) / 2;
        let y = area.y + (area.height.saturating_sub(panel_height)) / 2;

        let rect = Rect::new(x, y, panel_width, panel_height);

        Clear.render(rect, buf);

        let lines: Vec<Line> = LINES.iter().map(|s| Line::raw(*s)).collect();

        let paragraph = Paragraph::new(lines).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .title(" Keybindings "),
        );

        paragraph.render(rect, buf);
    }
}
