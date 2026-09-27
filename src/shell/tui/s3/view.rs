//! Pure rendering of the S3 explorer.
//!
//! The header names the connection, the profile and where the list is; the
//! list shows the buckets, a level or a search, with one status line under
//! it that says how far the rows go and how they ended; the detail pane
//! shows the preview or the selected row (on terminals at least 100 columns
//! wide; a compact one shows the side that has the keys). The search form,
//! the `kurama data` request, help and errors are modals. Regions come from
//! `layout`, widgets from `components`, colors from `theme`.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use super::model::{
    End, Focus, Row, S3Modal, S3Model, SearchForm, View, listing_status, location_uri,
};
use crate::domain::functions::s3_handoff::handoff_command;
use crate::shell::cli::client::safe_text;
use crate::shell::tui::components::{
    Badge, Header, KeyHints, KeyValues, Modal, TextView, fit, panel,
};
use crate::shell::tui::layout::home_layout;
use crate::shell::tui::theme::{self, Tone};

pub const APP_TITLE: &str = "鞍馬 kurama s3";
pub const READ_ONLY_BADGE: &str = "READ ONLY";
const LABEL_WIDTH: usize = 12;

// Priority order: a narrow terminal drops the last hints first.
const LIST_HINTS: [(&str, &str); 9] = [
    ("↑↓", "move"),
    ("Enter", "open"),
    ("←", "up"),
    ("/", "filter"),
    ("s", "key search"),
    ("g", "content search"),
    ("d", "as data"),
    ("q", "quit"),
    ("?", "help"),
];
const PREVIEW_HINTS: [(&str, &str); 6] = [
    ("↑↓", "scroll"),
    ("n", "next range"),
    ("d", "as data"),
    ("Esc", "list"),
    ("q", "quit"),
    ("?", "help"),
];
const FILTER_HINTS: [(&str, &str); 3] = [("type", "filter"), ("Enter", "keep"), ("Esc", "clear")];
const FORM_HINTS: [(&str, &str); 3] = [("Enter", "start"), ("Tab", "field"), ("Esc", "cancel")];
const RUNNING_HINTS: [(&str, &str); 3] = [("Esc", "stop"), ("↑↓", "move"), ("q", "quit")];
const HANDOFF_HINTS: [(&str, &str); 2] = [("y", "copy command"), ("Esc", "close")];
const MODAL_HINTS: [(&str, &str); 1] = [("Esc", "close")];

pub fn render(frame: &mut Frame, model: &S3Model) {
    let layout = home_layout(frame.area());
    frame.render_widget(header(model), layout.header);
    match layout.detail {
        Some(detail) => {
            render_list(frame, layout.list, model);
            render_detail(frame, detail, model);
        }
        None => match model.focus {
            Focus::List => render_list(frame, layout.list, model),
            Focus::Preview => render_detail(frame, layout.list, model),
        },
    }
    frame.render_widget(KeyHints::new(hints(model)), layout.footer);
    if let Some(modal) = modal(model, frame.area()) {
        modal.render(frame, frame.area());
    }
}

/// Key hints for the current state, most important first.
pub fn hints(model: &S3Model) -> &'static [(&'static str, &'static str)] {
    match &model.modal {
        Some(S3Modal::Handoff(_)) => return &HANDOFF_HINTS,
        Some(_) => return &MODAL_HINTS,
        None => {}
    }
    if model.form.is_some() {
        return &FORM_HINTS;
    }
    if model.running.is_some() {
        return &RUNNING_HINTS;
    }
    if model.filtering {
        return &FILTER_HINTS;
    }
    match model.focus {
        Focus::List => &LIST_HINTS,
        Focus::Preview => &PREVIEW_HINTS,
    }
}

fn header(model: &S3Model) -> Header<'_> {
    let summary = &model.summary;
    let subtitle = model.notice.clone().unwrap_or_else(|| {
        let at = match &model.listing.location {
            Some(location) => safe_text(&location_uri(location)),
            None => "buckets".to_owned(),
        };
        format!(
            "{}  {}  {}  {at}",
            summary.name, summary.aws_profile, summary.region
        )
    });
    Header::new(APP_TITLE, subtitle).badges([Badge {
        label: READ_ONLY_BADGE,
        on: true,
    }])
}

/// The list's title: what it is -- a level, a key search, a content search
/// -- and the local filter, each written differently.
fn list_title(model: &S3Model, width: usize) -> String {
    let listing = &model.listing;
    let what = match &listing.view {
        View::Buckets => "Buckets".to_owned(),
        View::Level => "List".to_owned(),
        View::KeySearch(text) => format!("Key search \"{}\"", safe_text(text)),
        View::ContentSearch(text) => format!("Content search \"{}\"", safe_text(text)),
    };
    let total = listing.rows.len();
    let more = if listing.end == Some(End::More) {
        "+"
    } else {
        ""
    };
    if model.filtering || !model.filter.is_empty() {
        let prefix = format!("{what} {}/{total}{more}  filter /", model.filtered.len());
        let available = width.saturating_sub(prefix.width() + 4);
        let query = if model.filtering {
            let (before, after) = model.filter.visible_parts(available);
            format!("{before}_{after}")
        } else {
            fit(&safe_text(model.filter.as_str()), available)
        };
        fit(&format!("{prefix}{query}"), width.saturating_sub(4))
    } else {
        fit(&format!("{what} ({total}{more})"), width.saturating_sub(4))
    }
}

fn render_list(frame: &mut Frame, area: Rect, model: &S3Model) {
    let title = list_title(model, area.width as usize);
    let block = panel(&title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [body, status_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    frame.render_widget(
        Paragraph::new(status_line(model, status_area.width as usize)),
        status_area,
    );
    if model.filtered.is_empty() {
        let message = if model.listing.end.is_none() {
            String::new()
        } else if model.listing.end == Some(End::Failed) {
            "S3 answered with an error. ← goes up a level, q quits.".to_owned()
        } else if !model.filter.is_empty() {
            format!(
                "Nothing here matches \"{}\". Esc clears the filter.",
                safe_text(model.filter.as_str())
            )
        } else {
            match &model.listing.view {
                View::Buckets => "No bucket was listed.".to_owned(),
                View::Level => "Nothing under this prefix.".to_owned(),
                View::KeySearch(_) => "No key under the prefix contains the text.".to_owned(),
                View::ContentSearch(_) => "No line read contains the text.".to_owned(),
            }
        };
        frame.render_widget(
            Paragraph::new(Span::styled(message, theme::hint())).wrap(Wrap { trim: true }),
            body,
        );
        return;
    }
    let height = body.height as usize;
    let first = (model.selected + 1).saturating_sub(height.max(1));
    let marker = theme::SELECTION_MARKER.width();
    let width = (body.width as usize).saturating_sub(marker);
    let prefix = model
        .listing
        .location
        .as_ref()
        .map_or("", |location| location.prefix.as_str());
    let lines: Vec<Line> = model
        .filtered
        .iter()
        .enumerate()
        .skip(first)
        .take(height)
        .filter_map(|(position, index)| {
            let row = model.listing.rows.get(*index)?;
            let text = row_text(row, prefix, width);
            Some(if position == model.selected {
                Line::from(vec![
                    Span::styled(theme::SELECTION_MARKER, theme::selection()),
                    Span::styled(
                        format!("{text}{}", " ".repeat(width.saturating_sub(text.width()))),
                        theme::selection(),
                    ),
                ])
            } else {
                Line::from(vec![Span::raw(" ".repeat(marker)), Span::raw(text)])
            })
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), body);
}

/// One row, cut to `width`: a name relative to the level, and what matters
/// about it on the right.
fn row_text(row: &Row, prefix: &str, width: usize) -> String {
    let relative = |name: &str| safe_text(name.strip_prefix(prefix).unwrap_or(name));
    let (name, right) = match row {
        Row::Bucket(bucket) => (
            safe_text(&bucket.name),
            bucket.region.clone().unwrap_or_default(),
        ),
        Row::Prefix(name) => (relative(name), String::new()),
        Row::Object(object) => (relative(&object.key), format!("{}", object.size)),
        Row::Match(found) => (
            format!("{}:{}", relative(&found.key), found.line),
            safe_text(&found.excerpt),
        ),
    };
    if matches!(row, Row::Match(_)) {
        return fit(&format!("{name}  {right}"), width);
    }
    let right_width = right.width();
    if right_width == 0 || width < right_width + 8 {
        return fit(&name, width);
    }
    let name = fit(&name, width - right_width - 2);
    let gap = width - name.width() - right_width;
    format!("{name}{}{right}", " ".repeat(gap))
}

/// The line under the list: the running request, or how far the rows go
/// and how they ended.
fn status_line(model: &S3Model, width: usize) -> Line<'static> {
    if let Some(running) = &model.running {
        let text = if running.stopping {
            format!("stopping… {} s", running.elapsed_secs)
        } else {
            let so_far = match running.ask {
                super::effects::S3Ask::Preview { .. } => String::new(),
                _ => format!("  {}", listing_status(&model.listing)),
            };
            format!(
                "{}… {} s  Esc stops it{so_far}",
                running.ask.label(),
                running.elapsed_secs
            )
        };
        return Line::from(Span::styled(fit(&text, width), Tone::Info.style()));
    }
    let style = match model.listing.end {
        Some(End::Bound(_) | End::Stopped | End::Failed) => Tone::Warning.style(),
        _ => theme::hint(),
    };
    Line::from(Span::styled(
        fit(&listing_status(&model.listing), width),
        style,
    ))
}

fn render_detail(frame: &mut Frame, area: Rect, model: &S3Model) {
    if let Some(preview) = &model.preview {
        let object = preview.result.object.as_ref();
        let mut lines: Vec<String> = Vec::new();
        let mut field = |label: &str, value: String| {
            lines.push(format!("{label:<LABEL_WIDTH$}{value}"));
        };
        field("Key", safe_text(&preview.key));
        if let Some(object) = object {
            field("Size", format!("{} bytes", object.size));
            field(
                "Type",
                object
                    .content_type
                    .as_deref()
                    .map_or_else(|| theme::NONE_TEXT.to_owned(), safe_text),
            );
            field(
                "ETag",
                object
                    .etag
                    .as_deref()
                    .map_or_else(|| theme::NONE_TEXT.to_owned(), safe_text),
            );
            field("Class", safe_text(&object.storage_class));
        }
        if let Some(read) = &preview.result.preview {
            let mut range = format!(
                "bytes {}-{} ({})",
                read.range_start, read.range_end, read.encoding
            );
            match (read.next_offset, preview.result.stop_reason) {
                (Some(next), _) => range.push_str(&format!("; n reads from {next}")),
                (None, Some(reason)) => {
                    range.push_str(&format!("; stopped at {}", super::model::stop_name(reason)))
                }
                (None, None) => range.push_str("; the whole object"),
            }
            field("Range", range);
        }
        lines.push(String::new());
        lines.extend(preview.lines.iter().cloned());
        let style = if model.focus == Focus::Preview {
            Tone::Info.title()
        } else {
            theme::panel_title()
        };
        TextView {
            title: "Preview",
            title_style: style,
            lines: &lines,
            scroll: preview.scroll,
            selected: None,
        }
        .render(frame, area);
        return;
    }
    let entries: Vec<(String, Span)> = match model.selected_row() {
        None => Vec::new(),
        Some(row) => row_details(row)
            .into_iter()
            .map(|(label, value)| (label.to_owned(), Span::raw(value)))
            .collect(),
    };
    KeyValues {
        title: "Details",
        label_width: LABEL_WIDTH,
        entries,
    }
    .render(frame, area);
}

fn row_details(row: &Row) -> Vec<(&'static str, String)> {
    let or_none = |value: &Option<String>| {
        value
            .as_deref()
            .map_or_else(|| theme::NONE_TEXT.to_owned(), safe_text)
    };
    let mut details = match row {
        Row::Bucket(bucket) => vec![
            ("Bucket", safe_text(&bucket.name)),
            ("Region", or_none(&bucket.region)),
            ("Created", or_none(&bucket.created)),
        ],
        Row::Prefix(prefix) => vec![("Prefix", safe_text(prefix))],
        Row::Object(object) => vec![
            ("Key", safe_text(&object.key)),
            ("Size", format!("{} bytes", object.size)),
            ("Modified", or_none(&object.last_modified)),
            ("ETag", or_none(&object.etag)),
            ("Class", or_none(&object.storage_class)),
        ],
        Row::Match(found) => vec![
            ("Key", safe_text(&found.key)),
            ("Line", found.line.to_string()),
            ("Excerpt", safe_text(&found.excerpt)),
        ],
    };
    details.push(("", String::new()));
    details.push((
        "Next",
        match row {
            Row::Bucket(_) | Row::Prefix(_) => "Enter opens it".to_owned(),
            Row::Object(_) => "Enter previews it; d opens it as data".to_owned(),
            Row::Match(_) => "Enter previews the object".to_owned(),
        },
    ));
    details
}

fn form_body(form: &SearchForm, width: usize) -> Vec<Line<'static>> {
    let field = |label: &str, input: &crate::shell::tui::components::LineInput, on: bool| {
        let label = Span::styled(format!("{label:<13}"), theme::label());
        let available = width.saturating_sub(13);
        if on {
            let mut line = input.line(available);
            line.spans.insert(0, label);
            line
        } else {
            Line::from(vec![
                label,
                Span::raw(fit(&safe_text(input.as_str()), available)),
            ])
        }
    };
    let what = if form.content {
        "Reads each object under the prefix, to the bounds, and lists every line that contains the text."
    } else {
        "Lists every key under the prefix that contains the text; not a pattern, case-sensitive."
    };
    let mut body = vec![
        Line::from(vec![
            Span::styled(format!("{:<13}", "in"), theme::label()),
            Span::raw(safe_text(&location_uri(&form.location))),
        ]),
        field("text", &form.text, form.field == 0),
        field("max objects", &form.max_objects, form.field == 1),
        Line::from(""),
        Line::from(Span::styled(what, theme::hint())),
    ];
    if form.content {
        body.push(Line::from(Span::styled(
            "At most 8 MiB an object, 50 MiB and 1000 lines a search. Enter starts it; Esc stops it.",
            theme::hint(),
        )));
    } else {
        body.push(Line::from(Span::styled(
            "Enter starts it; Esc stops it. Nothing is read before Enter.",
            theme::hint(),
        )));
    }
    if let Some(error) = &form.error {
        body.push(Line::from(""));
        body.push(Line::from(Span::styled(
            safe_text(error),
            Tone::Danger.style(),
        )));
    }
    body
}

fn modal(model: &S3Model, area: Rect) -> Option<Modal<'static>> {
    if model.modal.is_none()
        && let Some(form) = &model.form
    {
        return Some(Modal {
            title: if form.content {
                "Content search"
            } else {
                "Key search"
            },
            tone: Tone::Info,
            body: form_body(form, Modal::content_width(area)),
        });
    }
    let modal = match model.modal.as_ref()? {
        S3Modal::Help => Modal {
            title: "Help",
            tone: Tone::Info,
            body: [
                ("↑↓ or j/k", "move; the next page loads at the end"),
                ("Enter", "open a bucket or prefix; preview an object"),
                ("← or Bksp", "up one level; back from a search"),
                ("/", "filter the rows already listed (no request)"),
                ("s", "key search: every key under the prefix"),
                ("g", "content search: read the objects' lines"),
                ("n", "the next range of the preview"),
                ("d", "the kurama data request for an object"),
                ("Esc", "stop what runs; the rows so far stay"),
                ("q or Ctrl-C", "quit"),
            ]
            .into_iter()
            .map(|(key, action)| {
                Line::from(vec![
                    Span::styled(format!("{key:<13}"), theme::key()),
                    Span::raw(action),
                ])
            })
            .collect(),
        },
        S3Modal::Error(message) => Modal {
            title: "Error",
            tone: Tone::Danger,
            body: message
                .lines()
                .map(|line| Line::from(safe_text(line)))
                .collect(),
        },
        S3Modal::Handoff(action) => {
            let row = |label: &str, value: String| {
                Line::from(vec![
                    Span::styled(format!("{label:<10}"), theme::label()),
                    Span::raw(value),
                ])
            };
            let provenance = &action.provenance;
            Modal {
                title: "Open as data",
                tone: Tone::Info,
                body: vec![
                    Line::from(Span::styled(
                        "kurama data describes its columns. Nothing runs until you do; data reads the object again.",
                        theme::hint(),
                    )),
                    Line::from(""),
                    row("from", safe_text(&action.args.from)),
                    row("s3_source", safe_text(&action.args.s3_source)),
                    row(
                        "etag",
                        provenance
                            .etag
                            .as_deref()
                            .map_or_else(|| theme::NONE_TEXT.to_owned(), safe_text),
                    ),
                    row("size", format!("{} bytes", provenance.size)),
                    Line::from(""),
                    Line::from(safe_text(&handoff_command(action))),
                    Line::from(""),
                    Line::from(Span::styled(
                        "y copies the command; Esc closes this.",
                        theme::hint(),
                    )),
                ],
            }
        }
    };
    Some(modal)
}

#[cfg(test)]
#[path = "view_snapshot_tests.rs"]
mod snapshot_tests;
