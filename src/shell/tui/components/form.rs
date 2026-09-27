//! The operation form of the explorer: one row per parameter with its
//! value, the request body below, and the last validation error.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::{LineInput, fit, panel};
use crate::shell::tui::theme::{self, Tone};

/// Columns between the label, the hint and the value.
const GAP: &str = "  ";

pub struct FormField<'a> {
    pub label: &'a str,
    /// Where the parameter goes and its type, `path  string`.
    pub hint: String,
    pub value: &'a LineInput,
    pub required: bool,
    pub focused: bool,
}

pub struct FormBody<'a> {
    pub media_type: &'a str,
    pub required: bool,
    pub text: &'a str,
    pub focused: bool,
}

pub struct Form<'a> {
    pub title: &'a str,
    pub fields: Vec<FormField<'a>>,
    pub body: Option<FormBody<'a>>,
    pub error: Option<&'a str>,
    /// Shown when there is neither a field nor a body.
    pub empty_message: &'a str,
}

impl Form<'_> {
    pub fn render(self, frame: &mut Frame, area: Rect) {
        let block = panel(self.title);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let width = inner.width as usize;
        let marker_width = theme::SELECTION_MARKER.width();
        let mut label_width = self
            .fields
            .iter()
            .map(|field| field.label.width() + usize::from(field.required))
            .chain(std::iter::once("body*".width()))
            .max()
            .unwrap_or(0);
        let mut hint_width = self
            .fields
            .iter()
            .map(|field| field.hint.width())
            .max()
            .unwrap_or(0);
        // Long parameter names or enums must leave room for the value and cursor.
        let columns = width.saturating_sub(marker_width + 2 * GAP.len());
        let metadata_budget = columns.saturating_sub((columns / 3).max(1));
        if label_width + hint_width > metadata_budget {
            label_width = label_width.min(metadata_budget / 2);
            hint_width = hint_width.min(metadata_budget.saturating_sub(label_width));
        }
        let mut lines: Vec<Line> = Vec::new();
        // First, so it stays in view above a long body.
        if let Some(error) = self.error {
            lines.push(Line::from(Span::styled(
                format!("! {}", fit(error, width.saturating_sub(2))),
                Tone::Danger.style(),
            )));
            lines.push(Line::default());
        }
        for field in &self.fields {
            let (marker, label_style) = if field.focused {
                (theme::SELECTION_MARKER, theme::selection())
            } else {
                ("  ", theme::label())
            };
            let label = format!(
                "{}{}",
                fit(
                    field.label,
                    label_width.saturating_sub(usize::from(field.required))
                ),
                if field.required { "*" } else { "" }
            );
            let mut spans = vec![
                Span::styled(marker, label_style),
                Span::styled(pad(&label, label_width), label_style),
                Span::raw(GAP),
                Span::styled(pad(&field.hint, hint_width), theme::hint()),
                Span::raw(GAP),
            ];
            let used = marker_width + label_width + GAP.len() + hint_width + GAP.len();
            let value_width = width.saturating_sub(used);
            if field.focused {
                spans.extend(field.value.line(value_width).spans);
            } else {
                spans.push(Span::raw(fit(
                    field.value.as_str(),
                    value_width.saturating_sub(1),
                )));
            }
            lines.push(Line::from(spans));
        }
        if let Some(body) = &self.body {
            let (marker, label_style) = if body.focused {
                (theme::SELECTION_MARKER, theme::selection())
            } else {
                ("  ", theme::label())
            };
            let label = format!("body{}", if body.required { "*" } else { "" });
            lines.push(Line::from(vec![
                Span::styled(marker, label_style),
                Span::styled(pad(&label, label_width), label_style),
                Span::raw(GAP),
                Span::styled(body.media_type.to_string(), theme::hint()),
                Span::raw(GAP),
                Span::styled("Ctrl-E edits", theme::hint()),
            ]));
            let indent = " ".repeat(marker_width + 2);
            let text_width = width.saturating_sub(indent.len()).max(1);
            for line in body.text.lines() {
                lines.push(Line::from(vec![
                    Span::raw(indent.clone()),
                    Span::raw(fit(line, text_width)),
                ]));
            }
        }
        if self.fields.is_empty() && self.body.is_none() {
            lines.push(Line::from(Span::styled(self.empty_message, theme::hint())));
        }
        frame.render_widget(Paragraph::new(lines), inner);
    }
}

/// `text` padded with spaces to `width` display columns.
fn pad(text: &str, width: usize) -> String {
    let text = fit(text, width);
    format!("{text}{}", " ".repeat(width.saturating_sub(text.width())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::tui::testing::render_lines;

    #[test]
    fn fields_align_and_the_focused_one_shows_the_cursor() {
        let lines = render_lines(52, 11, |frame| {
            Form {
                title: "pets/create  POST /pets",
                fields: vec![
                    FormField {
                        label: "petId",
                        hint: "path  string".into(),
                        value: &LineInput::from("p-1"),
                        required: true,
                        focused: false,
                    },
                    FormField {
                        label: "X-Trace",
                        hint: "header  string".into(),
                        value: &LineInput::from("t"),
                        required: false,
                        focused: true,
                    },
                ],
                body: Some(FormBody {
                    media_type: "application/json",
                    required: true,
                    text: "{\n  \"name\": \"\"\n}",
                    focused: false,
                }),
                error: Some("parameter petId: \"x\" is not an integer"),
                empty_message: "",
            }
            .render(frame, frame.area())
        });
        assert_eq!(
            lines[1],
            "┃ ! parameter petId: \"x\" is not an integer         ┃"
        );
        assert_eq!(
            lines[2],
            "┃                                                  ┃"
        );
        assert_eq!(
            lines[3],
            "┃   petId*   path  string    p-1                   ┃"
        );
        assert_eq!(
            lines[4],
            "┃ ▸ X-Trace  header  string  t▏                    ┃"
        );
        assert_eq!(
            lines[5],
            "┃   body*    application/json  Ctrl-E edits        ┃"
        );
        assert_eq!(
            lines[6],
            "┃     {                                            ┃"
        );
        assert_eq!(
            lines[7],
            "┃       \"name\": \"\"                                 ┃"
        );
    }

    /// Only the focused row is drawn in the selection: the blank marker of
    /// the others has no background, which a selection with one would paint
    /// as a block in front of every field.
    #[test]
    fn only_the_focused_row_carries_the_selection() {
        let value = LineInput::from("v");
        let field = |label, focused| FormField {
            label,
            hint: "query  string".into(),
            value: &value,
            required: false,
            focused,
        };
        let buffer = crate::shell::tui::testing::render_frame(40, 5, |frame| {
            Form {
                title: "x",
                fields: vec![field("a", true), field("b", false)],
                body: None,
                error: None,
                empty_message: "",
            }
            .render(frame, frame.area())
        });
        // Row 1 is the focused field, row 2 the other; column 2 is the marker.
        assert_eq!(buffer[(2, 1)].bg, theme::selection().bg.unwrap());
        assert_eq!(buffer[(2, 2)].bg, ratatui::style::Color::Reset);
        assert_eq!(buffer[(3, 2)].bg, ratatui::style::Color::Reset);
    }

    #[test]
    fn a_long_focused_value_shows_its_end_and_labels_align_by_display_width() {
        let lines = render_lines(40, 5, |frame| {
            Form {
                title: "x",
                fields: vec![
                    FormField {
                        label: "名前",
                        hint: "query  string".into(),
                        value: &LineInput::from("abcdefghijklmnopqrstuvwxyz"),
                        required: true,
                        focused: true,
                    },
                    FormField {
                        label: "id",
                        hint: "path  string".into(),
                        value: &LineInput::from("abcdefghijklmnopqrstuvwxyz"),
                        required: false,
                        focused: false,
                    },
                ],
                body: None,
                error: None,
                empty_message: "",
            }
            .render(frame, frame.area())
        });
        assert_eq!(lines[1], "┃ ▸ 名前*  query  string  …qrstuvwxyz▏ ┃");
        assert_eq!(lines[2], "┃   id     path  string   abcdefghij…  ┃");
    }

    #[test]
    fn long_labels_and_enum_hints_leave_room_for_the_edited_value() {
        let input = LineInput::from("修正abc");
        let lines = render_lines(80, 5, |frame| {
            Form {
                title: "long parameters",
                fields: vec![FormField {
                    label: "a_parameter_with_a_very_long_descriptive_name_that_needs_clipping",
                    hint: "query enum(available, awaiting_confirmation, ready_for_publication)"
                        .into(),
                    value: &input,
                    required: true,
                    focused: true,
                }],
                body: None,
                error: None,
                empty_message: "",
            }
            .render(frame, frame.area())
        });
        assert!(
            lines.iter().any(|line| line.contains("修正abc▏")),
            "{lines:#?}"
        );
        assert!(lines.iter().any(|line| line.contains("…*")), "{lines:#?}");
    }

    #[test]
    fn a_form_without_inputs_says_so() {
        let lines = render_lines(50, 4, |frame| {
            Form {
                title: "pets/list  GET /pets",
                fields: vec![],
                body: None,
                error: None,
                empty_message: "No parameters; Enter sends the request.",
            }
            .render(frame, frame.area())
        });
        assert!(lines[1].contains("No parameters; Enter sends the request."));
    }
}
