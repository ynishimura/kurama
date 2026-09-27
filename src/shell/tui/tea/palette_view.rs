//! The command palette's modal: the query, the matches (kind, name, where
//! it goes) with the selected one marked, and how many there are.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::palette::{MAX_MATCHES, Palette};
use crate::shell::tui::components::{Modal, fit};
use crate::shell::tui::layout::palette_rows;
use crate::shell::tui::theme::{self, Tone};

pub const TITLE: &str = "Go to";
/// Width of the kind column (`history`).
const KIND_WIDTH: usize = 8;

pub fn modal(palette: &Palette, area: Rect) -> Modal<'static> {
    let width = Modal::content_width(area);
    let rows = palette_rows(area.height);
    let label = "> ";
    let mut query = palette.input.line(width.saturating_sub(label.width()));
    query.spans.insert(0, Span::styled(label, theme::key()));
    let mut body = vec![query, Line::default()];
    if palette.matches.is_empty() {
        body.push(Line::from(Span::styled("Nothing matches.", theme::hint())));
    }
    let start = palette
        .selected
        .saturating_sub(rows / 2)
        .min(palette.matches.len().saturating_sub(rows));
    for (position, index) in palette.matches.iter().enumerate().skip(start).take(rows) {
        let item = &palette.items[*index];
        let selected = position == palette.selected;
        let marker = if selected {
            theme::SELECTION_MARKER
        } else {
            "  "
        };
        let text = fit(
            &format!(
                "{marker}{:<KIND_WIDTH$}{}  {}",
                item.kind, item.name, item.detail
            ),
            width,
        );
        body.push(Line::from(Span::styled(
            text,
            if selected {
                theme::selection()
            } else {
                theme::plain()
            },
        )));
    }
    body.push(Line::default());
    let shown = if palette.matches.len() == MAX_MATCHES {
        format!("{}+", MAX_MATCHES)
    } else {
        palette.matches.len().to_string()
    };
    body.push(Line::from(Span::styled(
        fit(
            &format!("{shown} of {} · Enter goes there", palette.items.len()),
            width,
        ),
        theme::hint(),
    )));
    Modal {
        title: TITLE,
        tone: Tone::Info,
        body,
    }
}
