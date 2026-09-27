//! Pure rendering of the activity monitor: the calls the audit log recorded, newest first, colored by how they ended, and the selected call's details with the command that makes it again.
//!
//! Regions come from `layout` (the home screen's: the list, and the detail
//! pane from 100 columns on; a compact terminal shows the side that has the
//! keys), widgets from `components`, colors from `theme`. Every text an
//! entry holds is drawn with its control characters escaped.

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::text::Span;
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState, Wrap};
use unicode_width::UnicodeWidthStr;

use super::update::{ActivityModel, Focus};
use crate::domain::functions::audit::{duration_text, outcome, outcome_text, request_text};
use crate::domain::types::audit::AuditEntry;
use crate::shell::cli::client::safe_text;
use crate::shell::tui::components::{Header, KeyHints, KeyValues, fit, panel};
use crate::shell::tui::layout::home_layout;
use crate::shell::tui::theme;

pub const APP_TITLE: &str = "鞍馬 kurama audit";
pub const WATCHING: &str = "watching ~/.local/state/kurama/audit.jsonl";
const LIST_TITLE: &str = "Calls";
const DETAIL_TITLE: &str = "Call";
const LABEL_WIDTH: usize = 10;

/// Column widths: `HH:MM:SS` (UTC, as `kurama audit` prints it), `exec`,
/// `refused`, `999ms`.
const TIME_WIDTH: u16 = 8;
const COMMAND_WIDTH: u16 = 4;
const STATUS_WIDTH: u16 = 7;
const DURATION_WIDTH: u16 = 6;
/// The target column is as wide as the longest target, at most this and at
/// most half of what the fixed columns leave; the request gets the rest.
const TARGET_MAX_WIDTH: u16 = 16;
const COLUMNS: [&str; 6] = ["UTC", "CMD", "TARGET", "REQUEST", "STATUS", "TOOK"];

const LIST_HINTS: [(&str, &str); 4] = [
    ("↑↓", "move"),
    ("y", "copy command"),
    ("Enter", "details"),
    ("q", "quit"),
];
const DETAIL_HINTS: [(&str, &str); 4] = [
    ("Esc", "list"),
    ("y", "copy command"),
    ("↑↓", "move"),
    ("q", "quit"),
];
const EMPTY_HINTS: [(&str, &str); 1] = [("q", "quit")];

pub fn render(frame: &mut Frame, model: &ActivityModel) {
    let layout = home_layout(frame.area());
    let subtitle = match (&model.notice, &model.read_error) {
        (Some(notice), _) => safe_text(notice),
        (None, Some(error)) => safe_text(&format!("the audit log could not be read: {error}")),
        (None, None) => WATCHING.to_owned(),
    };
    frame.render_widget(Header::new(APP_TITLE, subtitle), layout.header);
    match layout.detail {
        Some(detail) => {
            render_list(frame, layout.list, model);
            render_detail(frame, detail, model);
        }
        None => match model.focus {
            Focus::List => render_list(frame, layout.list, model),
            Focus::Detail => render_detail(frame, layout.list, model),
        },
    }
    frame.render_widget(KeyHints::new(hints(model)), layout.footer);
}

/// Key hints for the current state, most important first.
pub fn hints(model: &ActivityModel) -> &'static [(&'static str, &'static str)] {
    match (model.entries.is_empty(), model.focus) {
        (true, _) => &EMPTY_HINTS,
        (false, Focus::List) => &LIST_HINTS,
        (false, Focus::Detail) => &DETAIL_HINTS,
    }
}

fn render_list(frame: &mut Frame, area: Rect, model: &ActivityModel) {
    let title = format!("{LIST_TITLE} ({})", model.entries.len());
    let block = panel(&title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let message = match (&model.read_error, model.loaded, model.entries.is_empty()) {
        (Some(error), _, true) => Some(format!("The audit log could not be read: {error}")),
        (None, false, _) => Some("Reading the audit log…".to_owned()),
        (None, true, true) => Some(
            "Nothing recorded yet. A run of api, exec, db or data under KURAMA_AGENT is \
             recorded, and every run with [audit] enabled = true; it appears here as it ends."
                .to_owned(),
        ),
        _ => None,
    };
    if let Some(message) = message {
        frame.render_widget(
            Paragraph::new(Span::styled(safe_text(&message), theme::hint()))
                .wrap(Wrap { trim: true }),
            inner,
        );
        return;
    }
    let marker = theme::SELECTION_MARKER.width() as u16;
    let fixed = marker + TIME_WIDTH + COMMAND_WIDTH + STATUS_WIDTH + DURATION_WIDTH + 5;
    let longest_target = model
        .entries
        .iter()
        .map(|entry| safe_text(&entry.target).width() as u16)
        .chain([COLUMNS[2].width() as u16])
        .max()
        .unwrap_or_default();
    let target_width = longest_target
        .min(TARGET_MAX_WIDTH)
        .min(inner.width.saturating_sub(fixed) / 2);
    let request_width = inner.width.saturating_sub(fixed + target_width);
    let rows = model.entries.iter().map(|entry| {
        let cells = [
            entry.time.format("%H:%M:%S").to_string(),
            safe_text(&entry.command),
            fit(&safe_text(&entry.target), target_width as usize),
            fit(&safe_text(&request_text(entry)), request_width as usize),
            outcome_text(entry),
            duration_text(entry.duration_ms),
        ];
        Row::new(cells.map(Cell::from)).style(theme::outcome_style(outcome(entry)))
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(TIME_WIDTH),
            Constraint::Length(COMMAND_WIDTH),
            Constraint::Length(target_width),
            Constraint::Min(request_width),
            Constraint::Length(STATUS_WIDTH),
            Constraint::Length(DURATION_WIDTH),
        ],
    )
    .header(Row::new(COLUMNS).style(theme::table_header()))
    .row_highlight_style(theme::selection())
    .highlight_symbol(theme::SELECTION_MARKER);
    let mut state = TableState::default();
    state.select(Some(model.selected));
    frame.render_stateful_widget(table, inner, &mut state);
}

fn render_detail(frame: &mut Frame, area: Rect, model: &ActivityModel) {
    let Some(entry) = model.selected_entry() else {
        let block = panel(DETAIL_TITLE);
        frame.render_widget(block, area);
        return;
    };
    KeyValues {
        title: DETAIL_TITLE,
        label_width: LABEL_WIDTH,
        entries: detail_entries(model, entry),
    }
    .render(frame, area);
}

fn detail_entries<'a>(model: &ActivityModel, entry: &AuditEntry) -> Vec<(String, Span<'a>)> {
    let optional = |value: Option<String>| match value {
        Some(value) => Span::raw(safe_text(&value)),
        None => Span::styled(theme::NONE_TEXT, theme::absent()),
    };
    let rerun = model.rerun(entry);
    vec![
        (
            "Time".into(),
            Span::raw(entry.time.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()),
        ),
        (
            "By".into(),
            Span::raw(if entry.agent { "agent" } else { "person" }),
        ),
        ("Command".into(), Span::raw(safe_text(&entry.command))),
        ("Target".into(), Span::raw(safe_text(&entry.target))),
        ("Request".into(), Span::raw(safe_text(&request_text(entry)))),
        (
            "Status".into(),
            Span::styled(outcome_text(entry), theme::outcome_style(outcome(entry))),
        ),
        (
            "Exit code".into(),
            optional(entry.exit_code.map(|code| code.to_string())),
        ),
        ("Error".into(), optional(entry.error_code.clone())),
        ("Took".into(), Span::raw(duration_text(entry.duration_ms))),
        ("Run again".into(), optional(rerun)),
    ]
}
