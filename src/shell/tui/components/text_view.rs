//! A scrollable text in a panel: the response of the explorer. The title
//! ends with the visible range when the text is longer than the pane.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use super::{fit, panel_styled};
use crate::shell::tui::theme;

pub struct TextView<'a, S: AsRef<str>> {
    pub title: &'a str,
    pub title_style: Style,
    pub lines: &'a [S],
    /// First visible line; the model clamps it to the last line, the view
    /// only keeps it inside the text.
    pub scroll: usize,
    /// A line drawn in the selection style; the view scrolls to keep it on
    /// screen and `scroll` is not read.
    pub selected: Option<usize>,
}

impl<S: AsRef<str>> TextView<'_, S> {
    pub fn render(self, frame: &mut Frame, area: Rect) {
        let height = panel_styled(self.title, self.title_style)
            .inner(area)
            .height as usize;
        let scroll = match self.selected {
            Some(selected) => (selected + 1).saturating_sub(height),
            None => self.scroll,
        }
        .min(self.lines.len().saturating_sub(1));
        let title = if self.lines.len() > height {
            format!(
                "{}  {}-{}/{}",
                self.title,
                scroll + 1,
                (scroll + height).min(self.lines.len()),
                self.lines.len()
            )
        } else {
            self.title.to_string()
        };
        let block = panel_styled(&title, self.title_style);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let visible: Vec<Line> = self
            .lines
            .iter()
            .skip(scroll)
            .take(inner.height as usize)
            .enumerate()
            .map(|(index, line)| {
                let text = fit(line.as_ref(), inner.width as usize);
                if self.selected == Some(scroll + index) {
                    Line::styled(text, theme::selection())
                } else {
                    Line::from(text)
                }
            })
            .collect();
        frame.render_widget(Paragraph::new(visible), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::tui::testing::render_lines;

    #[test]
    fn the_title_shows_the_visible_range_and_the_last_line_can_reach_the_top() {
        let lines: Vec<String> = (1..=10).map(|i| format!("line {i}")).collect();
        let rendered = render_lines(30, 5, |frame| {
            TextView {
                title: "Result",
                title_style: Style::default(),
                lines: &lines,
                scroll: 7,
                selected: None,
            }
            .render(frame, frame.area())
        });
        assert_eq!(rendered[0], "┏ Result  8-10/10 ━━━━━━━━━━━┓");
        assert_eq!(rendered[1], "┃ line 8                     ┃");
        assert_eq!(rendered[3], "┃ line 10                    ┃");
        let end = render_lines(30, 5, |frame| {
            TextView {
                title: "Result",
                title_style: Style::default(),
                lines: &lines,
                scroll: 100,
                selected: None,
            }
            .render(frame, frame.area())
        });
        assert_eq!(end[0], "┏ Result  10-10/10 ━━━━━━━━━━┓");
        assert_eq!(end[1], "┃ line 10                    ┃");
        assert_eq!(end[2], "┃                            ┃");
        let short = render_lines(30, 5, |frame| {
            TextView {
                title: "Result",
                title_style: Style::default(),
                lines: &lines[..2],
                scroll: 0,
                selected: None,
            }
            .render(frame, frame.area())
        });
        assert_eq!(short[0], "┏ Result ━━━━━━━━━━━━━━━━━━━━┓");
    }

    #[test]
    fn a_selected_line_is_kept_on_screen_in_the_selection_style() {
        let lines: Vec<String> = (1..=10).map(|i| format!("line {i}")).collect();
        let buffer = crate::shell::tui::testing::render_frame(30, 5, |frame| {
            TextView {
                title: "Tree",
                title_style: Style::default(),
                lines: &lines,
                scroll: 0,
                selected: Some(6),
            }
            .render(frame, frame.area())
        });
        let rendered = crate::shell::tui::testing::buffer_lines(&buffer);
        assert_eq!(rendered[0], "┏ Tree  5-7/10 ━━━━━━━━━━━━━━┓");
        assert_eq!(rendered[3], "┃ line 7                     ┃");
        assert_eq!(buffer[(2, 3)].style().bg, theme::selection().bg);
        assert_ne!(buffer[(2, 2)].style().bg, theme::selection().bg);
    }
}
