//! One row of numbered tabs, the one on screen in the selection style.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::shell::tui::theme;

pub struct Tabs<'a> {
    labels: &'a [&'a str],
    selected: usize,
}

impl<'a> Tabs<'a> {
    pub fn new(labels: &'a [&'a str], selected: usize) -> Self {
        Self { labels, selected }
    }
}

impl Widget for Tabs<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mut spans = vec![Span::raw(" ")];
        for (index, label) in self.labels.iter().enumerate() {
            let text = format!(" {} {label} ", index + 1);
            spans.push(if index == self.selected {
                Span::styled(text, theme::selection())
            } else {
                Span::styled(text, theme::hint())
            });
            spans.push(Span::raw(" "));
        }
        Paragraph::new(Line::from(spans)).render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::tui::testing::render_frame;

    #[test]
    fn every_tab_is_numbered_and_the_selected_one_is_highlighted() {
        let area = Rect::new(0, 0, 40, 1);
        let buffer = render_frame(40, 1, |frame| {
            frame.render_widget(Tabs::new(&["AWS", "Auth", "API"], 1), area)
        });
        let line: String = (0..40).map(|x| buffer[(x, 0)].symbol()).collect();
        assert_eq!(line, "  1 AWS   2 Auth   3 API                ");
        let selected = |x: usize| {
            crate::shell::tui::testing::has_style(&buffer[(x as u16, 0)], theme::selection())
        };
        assert!(selected(line.find("2 Auth").unwrap()));
        assert!(!selected(line.find("1 AWS").unwrap()));
    }
}
