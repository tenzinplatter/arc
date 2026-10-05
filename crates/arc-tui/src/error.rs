use chrono::{Duration, NaiveDateTime};
use ratatui::{
    buffer::Buffer,
    layout::{HorizontalAlignment, Rect},
    style::Style,
    widgets::{Block, BorderType, Padding, Paragraph, Widget, WidgetRef, Wrap},
};
use unicode_ellipsis::truncate_str;

use crate::text_utils::{count_wrapped_lines, truncate_to_lines};
use arc_core::dbg_file;

const BORDERS: u16 = 2;
const HORIZONTAL_PADDING: u16 = 2;
const HORIZONTAL_CHROME: u16 = BORDERS + HORIZONTAL_PADDING;
const ERROR_NOTIFICATION_WINDOW_MAX_WIDTH: u16 = 50;
const ERROR_NOTIFICATION_MAX_TEXT_HEIGHT: u16 = 3;
const ERROR_TITLE_MAX_WIDTH: u16 = ERROR_NOTIFICATION_WINDOW_MAX_WIDTH - HORIZONTAL_CHROME;
pub const ERROR_NOTIFICATION_MAX_HEIGHT: u16 = ERROR_NOTIFICATION_MAX_TEXT_HEIGHT + BORDERS;

#[derive(Clone, Debug)]
pub struct ErrorInfo {
    short: String,
    long: String,
    created_at: NaiveDateTime,
}

impl ErrorInfo {
    pub fn get_required_width(&self) -> u16 {
        let text_len = u16::max(self.short.len() as u16, self.long.len() as u16);
        u16::min(text_len + HORIZONTAL_CHROME, ERROR_NOTIFICATION_WINDOW_MAX_WIDTH)
    }

    pub fn get_required_height(&self, available_width: u16) -> u16 {
        let inner_width = available_width.saturating_sub(HORIZONTAL_CHROME) as usize;
        if inner_width == 0 {
            return ERROR_NOTIFICATION_MAX_HEIGHT;
        }

        let line_count = count_wrapped_lines(&self.long, inner_width);
        let text_height = (line_count as u16).min(ERROR_NOTIFICATION_MAX_TEXT_HEIGHT);

        text_height + BORDERS
    }

    pub fn is_expired(&self) -> bool {
        arc_core::time::now_naive() > self.created_at + Duration::seconds(3)
    }

    pub fn new<A, B>(short: A, long: B) -> ErrorInfo
    where
        A: ToString,
        B: ToString,
    {
        ErrorInfo {
            short: short.to_string(),
            long: long.to_string(),
            created_at: arc_core::time::now_naive(),
        }
    }
}

impl WidgetRef for ErrorInfo {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        let truncated_title = truncate_str(&self.short, ERROR_TITLE_MAX_WIDTH as usize);
        let padded_title = format!(" {} ", truncated_title);
        dbg_file!("{}", truncated_title);
        let inner_width = area.width.saturating_sub(HORIZONTAL_CHROME) as usize;
        let display_text = truncate_to_lines(
            &self.long,
            inner_width,
            ERROR_NOTIFICATION_MAX_TEXT_HEIGHT as usize,
        );

        let block = Block::bordered()
            .title_top(padded_title)
            .title_alignment(HorizontalAlignment::Center)
            .border_style(Style::default().red())
            .border_type(BorderType::Rounded)
            .padding(Padding::horizontal(1));

        Paragraph::new(display_text)
            .wrap(Wrap { trim: true })
            .block(block)
            .render(area, buf);
    }
}
