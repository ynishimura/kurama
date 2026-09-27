//! The history modal: favorites and sent requests, one row each, the
//! selected one marked, and the name being typed for a favorite.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::history::{HistoryEntry, HistoryModal};
use crate::shell::tui::components::{Modal, fit};
use crate::shell::tui::layout::history_modal_rows;
use crate::shell::tui::theme::{self, Tone};

pub const TITLE: &str = "History";

pub fn modal(history: &HistoryModal, area: Rect) -> Modal<'static> {
    let width = Modal::content_width(area);
    let rows = history_modal_rows(area.height);
    let mut body = Vec::new();
    if history.entries.is_empty() {
        body.push(Line::from(Span::styled(
            "Nothing sent yet: a request sent from a form is listed here.",
            theme::hint(),
        )));
        return Modal {
            title: TITLE,
            tone: Tone::Info,
            body,
        };
    }
    let start = history
        .selected
        .saturating_sub(rows / 2)
        .min(history.entries.len().saturating_sub(rows));
    for (index, entry) in history.entries.iter().enumerate().skip(start).take(rows) {
        let marker = if index == history.selected {
            theme::SELECTION_MARKER
        } else {
            "  "
        };
        let text = fit(&format!("{marker}{}", row_text(entry)), width);
        body.push(Line::from(if index == history.selected {
            Span::styled(text, theme::selection())
        } else {
            Span::raw(text)
        }));
    }
    body.push(Line::default());
    body.push(match &history.naming {
        Some(name) => {
            let label = format!("{:<6}", "Name");
            let mut line = name.line(width.saturating_sub(label.width()));
            line.spans.insert(0, Span::styled(label, theme::label()));
            line
        }
        None => Line::from(Span::styled(
            format!(
                "{}/{}  Enter opens the form, s saves as a favorite",
                history.selected + 1,
                history.entries.len()
            ),
            theme::hint(),
        )),
    });
    Modal {
        title: TITLE,
        tone: Tone::Info,
        body,
    }
}

/// `[name] operation  a=1  b=2` for a favorite, the operation and its values
/// alone for a sent request.
fn row_text(entry: &HistoryEntry) -> String {
    let mut text = match &entry.name {
        Some(name) => format!("[{name}] {}", entry.operation),
        None => entry.operation.clone(),
    };
    for (name, value) in &entry.params {
        text.push_str(&format!("  {name}={value}"));
    }
    if entry.body.is_some() {
        text.push_str("  +body");
    }
    text
}
