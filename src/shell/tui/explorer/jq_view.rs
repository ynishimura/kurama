//! Bounded jq input, candidate/example choices and preview rendering.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::jq_input::{JqInputModel, JqPanel, Preview};
use crate::shell::tui::components::{Modal, fit};
use crate::shell::tui::layout::{jq_choice_name_width, jq_modal_rows};
use crate::shell::tui::theme::{self, Tone};

pub fn modal(input: &JqInputModel, area: Rect) -> Modal<'static> {
    let width = Modal::content_width(area);
    let rows = jq_modal_rows(area.height);
    let label = format!("{:<8}", "Filter");
    let mut filter = input.input.line(width.saturating_sub(label.width()));
    filter.spans.insert(0, Span::styled(label, theme::label()));
    let mut body = vec![filter, Line::default()];
    let (title, selected, choices): (_, _, Vec<_>) = match &input.panel {
        JqPanel::Candidates { items, selected } => (
            "Complete",
            *selected,
            items
                .iter()
                .map(|candidate| (&candidate.label, &candidate.description))
                .collect(),
        ),
        JqPanel::Examples { items, selected } => (
            "Examples",
            *selected,
            items
                .iter()
                .map(|example| (&example.filter, &example.description))
                .collect(),
        ),
        JqPanel::None => ("", 0, Vec::new()),
    };
    if !choices.is_empty() && rows.choices > 0 {
        body.push(Line::from(Span::styled(
            format!("{title} ({}/{})", selected + 1, choices.len()),
            theme::label(),
        )));
        let start = selected
            .saturating_sub(rows.choices / 2)
            .min(choices.len().saturating_sub(rows.choices));
        for (index, (value, description)) in
            choices.iter().enumerate().skip(start).take(rows.choices)
        {
            let marker = if index == selected {
                theme::SELECTION_MARKER
            } else {
                "  "
            };
            let value = if matches!(input.panel, JqPanel::Candidates { .. }) {
                fit(value, jq_choice_name_width(width))
            } else {
                (*value).clone()
            };
            body.push(Line::from(Span::styled(
                fit(&format!("{marker}{value}  {description}"), width),
                if index == selected {
                    theme::selection()
                } else {
                    theme::plain()
                },
            )));
        }
        body.push(Line::default());
    }
    match &input.preview {
        Preview::Output(Ok(lines)) => {
            let shown = lines.len().min(rows.preview);
            body.push(Line::from(Span::styled(
                format!("preview ({shown} shown)"),
                theme::label(),
            )));
            for line in lines.iter().take(rows.preview) {
                let text = line.replace('\n', "\\n").replace('\r', "\\r");
                body.push(Line::from(fit(&text, width)));
            }
            if lines.is_empty() {
                body.push(Line::from(Span::styled("No output", theme::hint())));
            }
        }
        Preview::Output(Err(error)) => body.push(Line::from(Span::styled(
            fit(&format!("preview: {}", error.replace('\n', " ")), width),
            theme::hint(),
        ))),
        Preview::Pending => body.push(Line::from(Span::styled(
            "preview: calculating…",
            theme::hint(),
        ))),
        Preview::Unavailable(message) => {
            body.push(Line::from(Span::styled(fit(message, width), theme::hint())))
        }
    }
    body.push(Line::default());
    body.push(Line::from(Span::styled(
        fit("Empty filter restores the response body.", width),
        theme::hint(),
    )));
    Modal {
        title: "jq filter",
        tone: Tone::Info,
        body,
    }
}
