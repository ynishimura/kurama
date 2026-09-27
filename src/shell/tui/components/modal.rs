//! A centered message box drawn over the screen: help, the MFA prompt,
//! progress, success and error. It sizes itself to its content and never
//! leaves the viewport.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Padding, Paragraph, Wrap};

use crate::shell::tui::layout::modal_area;
use crate::shell::tui::theme::{self, Tone};

/// Default width; narrower terminals get a narrower box.
pub const DEFAULT_WIDTH: u16 = 60;
const HORIZONTAL_PADDING: u16 = 2;
const VERTICAL_PADDING: u16 = 1;
/// Columns the terminal keeps free on each side of the box.
const MARGIN: u16 = 2;

pub struct Modal<'a> {
    pub title: &'a str,
    pub tone: Tone,
    pub body: Vec<Line<'a>>,
}

impl Modal<'_> {
    /// Width available to body content at this terminal size.
    pub fn content_width(area: Rect) -> usize {
        let width = DEFAULT_WIDTH.min(area.width.saturating_sub(2 * MARGIN));
        width.saturating_sub(2 + 2 * HORIZONTAL_PADDING).max(1) as usize
    }

    pub fn render(self, frame: &mut Frame, area: Rect) {
        let width = DEFAULT_WIDTH.min(area.width.saturating_sub(2 * MARGIN));
        let inner_width = Self::content_width(area) as u16;
        // The paragraph wraps the body; asking it for the row count keeps the
        // box exactly as tall as the wrapped text.
        let paragraph = Paragraph::new(self.body).wrap(Wrap { trim: false });
        let content_rows = paragraph.line_count(inner_width) as u16;
        let height = content_rows + 2 + 2 * VERTICAL_PADDING;
        let rect = modal_area(area, width, height);

        frame.render_widget(Clear, rect);
        let block = Block::bordered()
            .border_set(theme::BORDER)
            .border_style(self.tone.style())
            .title(Line::from(Span::styled(
                format!(" {} ", self.title),
                self.tone.title(),
            )))
            .padding(Padding::new(
                HORIZONTAL_PADDING,
                HORIZONTAL_PADDING,
                VERTICAL_PADDING,
                VERTICAL_PADDING,
            ));
        frame.render_widget(paragraph.block(block), rect);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::tui::testing::render_lines;

    fn render(modal: Modal, width: u16, height: u16) -> Vec<String> {
        render_lines(width, height, |frame| {
            // Something underneath, so the test sees the box clearing it.
            let fill: Vec<Line> = (0..height)
                .map(|_| Line::from("x".repeat(width as usize)))
                .collect();
            frame.render_widget(Paragraph::new(fill), frame.area());
            modal.render(frame, frame.area());
        })
    }

    #[test]
    fn modal_is_centered_sized_to_its_body_and_clears_what_is_under_it() {
        let lines = render(
            Modal {
                title: "Error",
                tone: Tone::Danger,
                body: vec![Line::from("Access denied"), Line::from("Enter back")],
            },
            80,
            12,
        );
        // 2 lines + 2 padding rows + 2 borders = 6 rows, centered in 12: rows 3..=8.
        assert!(lines[3].starts_with("xxxxxxxxxx┏ Error ━"), "{}", lines[3]);
        let text = lines[5].trim_matches('x');
        assert!(text.starts_with("┃  Access denied "), "{text}");
        assert!(text.ends_with("  ┃"), "{text}");
        assert_eq!(text.chars().count(), 60);
        assert!(lines[6].contains("┃  Enter back "), "{}", lines[6]);
        assert!(lines[8].starts_with("xxxxxxxxxx┗"), "{}", lines[8]);
        assert_eq!(lines[2].trim_matches('x'), "");
        assert_eq!(lines[9].trim_matches('x'), "");
    }

    #[test]
    fn a_narrow_terminal_gets_a_narrower_box_that_wraps_the_text() {
        let lines = render(
            Modal {
                title: "Help",
                tone: Tone::Info,
                body: vec![Line::from(
                    "A message that is longer than the box so it must wrap onto lines",
                )],
            },
            40,
            10,
        );
        // width 36, inner 30: 65 columns of text need 3 rows, 7 with padding
        // and borders, centered in 10: rows 1..=7.
        assert!(lines[1].starts_with("xx┏ Help "), "{}", lines[1]);
        assert!(lines[7].starts_with("xx┗"), "{:#?}", lines);
    }
}
