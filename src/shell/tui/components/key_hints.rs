//! One row of `key action` pairs. Hints are given in priority order and the
//! row shows as many as fit, so a narrow terminal drops the last ones instead
//! of wrapping the footer onto a second line.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

use crate::shell::tui::theme;

/// Columns between two hints.
const GAP: &str = "  ";

pub struct KeyHints<'a> {
    hints: &'a [(&'a str, &'a str)],
}

impl<'a> KeyHints<'a> {
    pub fn new(hints: &'a [(&'a str, &'a str)]) -> Self {
        Self { hints }
    }

    /// The hints that fit in `width` columns, in order.
    fn fitting(&self, width: usize) -> &'a [(&'a str, &'a str)] {
        let mut used = 1; // leading space
        let mut count = 0;
        for (index, (key, action)) in self.hints.iter().enumerate() {
            let gap = if index == 0 { 0 } else { GAP.len() };
            let needed = gap + key.width() + 1 + action.width();
            if used + needed > width {
                break;
            }
            used += needed;
            count += 1;
        }
        &self.hints[..count]
    }
}

impl Widget for KeyHints<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mut spans = vec![Span::raw(" ")];
        for (index, (key, action)) in self.fitting(area.width as usize).iter().enumerate() {
            if index > 0 {
                spans.push(Span::raw(GAP));
            }
            spans.push(Span::styled(*key, theme::key()));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(*action, theme::hint()));
        }
        Paragraph::new(Line::from(spans)).render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::tui::testing::render_lines;

    const HINTS: [(&str, &str); 4] = [
        ("↑↓", "move"),
        ("Enter", "assume"),
        ("q", "quit"),
        ("? ", "help"),
    ];

    fn text(width: u16) -> String {
        let area = Rect::new(0, 0, width, 1);
        render_lines(width, 1, |frame| {
            frame.render_widget(KeyHints::new(&HINTS), area)
        })
        .remove(0)
    }

    #[test]
    fn every_hint_fits_on_a_wide_row() {
        assert_eq!(text(80), " ↑↓ move  Enter assume  q quit  ?  help");
    }

    #[test]
    fn narrow_rows_drop_trailing_hints_instead_of_wrapping() {
        assert_eq!(text(30), " ↑↓ move  Enter assume  q quit");
        assert_eq!(text(12), " ↑↓ move");
        assert_eq!(text(5), "");
    }
}
