//! Label/value rows in a panel. A value longer than the pane wraps onto
//! indented continuation lines, so a long ARN or path stays readable and
//! the labels stay aligned.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{panel, wrap};
use crate::shell::tui::theme;

pub struct KeyValues<'a> {
    pub title: &'a str,
    pub label_width: usize,
    pub entries: Vec<(String, Span<'a>)>,
}

impl KeyValues<'_> {
    pub fn render(self, frame: &mut Frame, area: Rect) {
        let block = panel(self.title);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(
            Paragraph::new(entry_lines(
                &self.entries,
                self.label_width,
                inner.width as usize,
            )),
            inner,
        );
    }
}

/// One line per wrapped chunk of every value, the label on the first.
pub fn entry_lines(
    entries: &[(String, Span<'_>)],
    label_width: usize,
    width: usize,
) -> Vec<Line<'static>> {
    let value_width = width.saturating_sub(label_width).max(1);
    let mut lines = Vec::new();
    for (label, value) in entries {
        let content = value.content.trim_start_matches(' ');
        let indent = (value.content.len() - content.len()).min(value_width.saturating_sub(1));
        for (index, chunk) in wrap(content, value_width - indent).into_iter().enumerate() {
            let label = if index == 0 {
                format!("{label:<label_width$}")
            } else {
                " ".repeat(label_width)
            };
            lines.push(Line::from(vec![
                Span::styled(label, theme::label()),
                Span::styled(format!("{}{chunk}", " ".repeat(indent)), value.style),
            ]));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::tui::testing::render_lines;

    #[test]
    fn preformatted_values_keep_their_indentation_when_wrapped() {
        let rows = entry_lines(&[("Shape".into(), Span::raw("  a long value"))], 8, 18);
        let text: Vec<String> = rows
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect();
        assert_eq!(text, ["Shape     a long", "          value"]);
    }

    #[test]
    fn values_wrap_under_their_label_inside_the_panel() {
        let lines = render_lines(30, 6, |frame| {
            KeyValues {
                title: "Operation",
                label_width: 8,
                entries: vec![
                    (
                        "Request".into(),
                        Span::raw("GET /repos/{owner}/{repo}/issues"),
                    ),
                    ("Scopes".into(), Span::raw("repo")),
                ],
            }
            .render(frame, frame.area())
        });
        assert_eq!(lines[0], "┏ Operation ━━━━━━━━━━━━━━━━━┓");
        assert_eq!(lines[1], "┃ Request GET                ┃");
        assert_eq!(lines[2], "┃         /repos/{owner}/{re ┃");
        assert_eq!(lines[3], "┃         po}/issues         ┃");
        assert_eq!(lines[4], "┃ Scopes  repo               ┃");
    }
}
