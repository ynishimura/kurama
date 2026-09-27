//! Pure rendering of the explorer.
//!
//! The list screen is the operation table with the detail of the selected
//! operation beside it (on terminals at least 100 columns wide); the form
//! and the result take the whole width. Modals are drawn on top. Regions
//! come from `layout`, widgets from `components`, colors from `theme`.

use crate::domain::functions::profile_status::describe_time_left;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::json_tree::{RowKind, tree_rows};
use super::model::{
    ExplorerModal, ExplorerModel, FormModel, ResultModel, Screen, result_lines, status_text,
};
use crate::domain::functions::spec_output::first_paragraph;
use crate::domain::types::api_spec::{Operation, Parameter, RequestBody, Response};
use crate::shell::tui::components::{
    Badge, Form, FormBody, FormField, Header, KeyHints, KeyValues, Modal, OperationRow,
    OperationTable, TextView, fit,
};
use crate::shell::tui::layout::{self, HomeLayout, home_layout};
use crate::shell::tui::theme::{self, Tone};

pub const APP_TITLE: &str = "鞍馬 kurama api";
const OPERATIONS_TITLE: &str = "Operations";
const DETAIL_TITLE: &str = "Operation";
const LABEL_WIDTH: usize = 10;

// Priority order: a narrow terminal drops the last hints first.
const LIST_HINTS: [(&str, &str); 8] = [
    ("↑↓", "move"),
    ("Enter", "open"),
    ("/", "search"),
    ("q", "quit"),
    ("h", "history"),
    ("?", "help"),
    ("c", "copy"),
    ("o", "docs"),
];
const SEARCH_HINTS: [(&str, &str); 4] = [
    ("type", "filter"),
    ("Enter", "keep"),
    ("Esc", "clear"),
    ("↑↓", "move"),
];
// Letters are typed into the fields, so the form's own keys carry a
// modifier and help is F1.
const FORM_HINTS: [(&str, &str); 7] = [
    ("Tab", "next"),
    ("type", "value"),
    ("Enter", "send"),
    ("^E", "body"),
    ("^Y", "copy"),
    ("Esc", "back"),
    ("F1", "help"),
];
const FORM_HINTS_WITHOUT_BODY: [(&str, &str); 6] = [
    ("Tab", "next"),
    ("type", "value"),
    ("Enter", "send"),
    ("^Y", "copy"),
    ("Esc", "back"),
    ("F1", "help"),
];
const RESULT_HINTS: [(&str, &str); 7] = [
    ("↑↓", "scroll"),
    ("j", "jq"),
    ("t", "tree"),
    ("h", "headers"),
    ("c", "copy"),
    ("Esc", "back"),
    ("q", "quit"),
];
const TREE_HINTS: [(&str, &str); 7] = [
    ("↑↓", "move"),
    ("←→", "fold"),
    ("y", "copy path"),
    ("j", "jq path"),
    ("t", "text"),
    ("Esc", "back"),
    ("q", "quit"),
];
const HELP_HINTS: [(&str, &str); 1] = [("Esc", "close")];
const HISTORY_HINTS: [(&str, &str); 4] = [
    ("↑↓", "select"),
    ("Enter", "open"),
    ("s", "save"),
    ("Esc", "close"),
];
const NAMING_HINTS: [(&str, &str); 2] = [("Enter", "save"), ("Esc", "cancel")];
const ERROR_HINTS: [(&str, &str); 1] = [("Enter", "back")];
const PROCESSING_HINTS: [(&str, &str); 0] = [];
const JQ_HINTS: [(&str, &str); 5] = [
    ("Tab", "complete"),
    ("Enter", "apply"),
    ("F1", "examples"),
    ("Esc", "cancel"),
    ("↑↓", "history"),
];
const JQ_CHOICE_HINTS: [(&str, &str); 4] = [
    ("↑↓", "select"),
    ("Enter", "insert"),
    ("Esc", "back"),
    ("Tab", "complete"),
];

pub fn render(frame: &mut Frame, model: &ExplorerModel) {
    let layout = home_layout(frame.area());
    frame.render_widget(header(model), layout.header);
    match &model.screen {
        Screen::List => {
            let title = operations_title(model, layout.list.width as usize);
            let empty_message = empty_message(model);
            OperationTable {
                title: &title,
                rows: model
                    .filtered
                    .iter()
                    .filter_map(|index| model.operation(*index))
                    .map(|operation| OperationRow {
                        method: &operation.method,
                        id: &operation.id,
                        deprecated: operation.deprecated,
                    })
                    .collect(),
                selected: model.selected,
                loading: model.loading,
                empty_message: &empty_message,
            }
            .render(frame, layout.list);
            if let Some(area) = layout.detail {
                KeyValues {
                    title: DETAIL_TITLE,
                    label_width: LABEL_WIDTH,
                    entries: detail_entries(
                        model.selected_operation(),
                        layout::detail_shape_rows(area.height),
                    ),
                }
                .render(frame, area);
            }
        }
        Screen::Form(form) => {
            if let Some(operation) = model.operation(form.operation) {
                render_form(frame, main_area(&layout), operation, form);
            }
        }
        Screen::Result(result) => {
            if let Some(operation) = model.operation(result.form.operation) {
                render_result(frame, main_area(&layout), operation, result);
            }
        }
    }
    frame.render_widget(KeyHints::new(hints(model)), layout.footer);
    if let Some(modal) = modal(model, frame.area()) {
        modal.render(frame, frame.area());
    }
}

/// The list and the detail region together: the form and the result use
/// the whole width.
fn main_area(layout: &HomeLayout) -> Rect {
    Rect {
        width: layout.list.width + layout.detail.map_or(0, |detail| detail.width),
        ..layout.list
    }
}

/// Key hints for the current screen, most important first.
pub fn hints(model: &ExplorerModel) -> &'static [(&'static str, &'static str)] {
    match &model.modal {
        Some(ExplorerModal::Help) => return &HELP_HINTS,
        Some(ExplorerModal::Error(_)) => return &ERROR_HINTS,
        Some(ExplorerModal::Processing(_)) => return &PROCESSING_HINTS,
        Some(ExplorerModal::History(history)) => {
            return if history.naming.is_some() {
                &NAMING_HINTS
            } else {
                &HISTORY_HINTS
            };
        }
        Some(ExplorerModal::JqInput(input)) => {
            return if input.panel.is_open() {
                &JQ_CHOICE_HINTS
            } else {
                &JQ_HINTS
            };
        }
        None => {}
    }
    match &model.screen {
        Screen::List if model.searching => &SEARCH_HINTS,
        Screen::List => &LIST_HINTS,
        Screen::Form(form) if form.body.is_some() && form.focus == form.values.len() => &FORM_HINTS,
        Screen::Form(_) => &FORM_HINTS_WITHOUT_BODY,
        Screen::Result(result) if result.tree.is_some() => &TREE_HINTS,
        Screen::Result(_) => &RESULT_HINTS,
    }
}

fn header(model: &ExplorerModel) -> Header<'_> {
    let subtitle = model.notice.clone().unwrap_or_else(|| match &model.spec {
        Some(spec) => format!("{}  {} {}", model.api.name, spec.title, spec.version)
            .trim_end()
            .to_string(),
        None if model.loading => format!("{}  loading the description…", model.api.name),
        None => format!("{}  no description", model.api.name),
    });
    let time_left = model
        .token_expires_at
        .map(|expires_at| describe_time_left("token", expires_at, model.now));
    Header::new(APP_TITLE, subtitle)
        .time_left(time_left)
        .badges([
            Badge {
                label: "bearer",
                on: model.api.auth.is_some(),
            },
            Badge {
                label: "sigv4",
                on: model.api.aws_profile.is_some(),
            },
        ])
}

fn operations_title(model: &ExplorerModel, width: usize) -> String {
    let total = model.spec.as_ref().map_or(0, |spec| spec.operations.len());
    if model.loading || model.spec.is_none() {
        OPERATIONS_TITLE.to_string()
    } else if model.searching || !model.search_query.is_empty() {
        let prefix = format!("{OPERATIONS_TITLE} {}/{total}  /", model.filtered.len());
        let available = width.saturating_sub(prefix.width() + 4);
        let query = if model.searching {
            let (before, after) = model.search_query.visible_parts(available);
            format!("{before}_{after}")
        } else {
            fit(model.search_query.as_str(), available)
        };
        format!("{prefix}{query}")
    } else {
        format!("{OPERATIONS_TITLE} ({total})")
    }
}

fn empty_message(model: &ExplorerModel) -> String {
    if model.spec.is_none() {
        "The API description could not be loaded; q quits.".to_string()
    } else if model.search_query.is_empty() {
        "The description lists no operation.".to_string()
    } else {
        format!(
            "No operation matches \"{}\". Esc clears the search.",
            model.search_query.as_str()
        )
    }
}

fn detail_entries(
    operation: Option<&Operation>,
    shape_rows: usize,
) -> Vec<(String, Span<'static>)> {
    let Some(operation) = operation else {
        return vec![(
            String::new(),
            Span::styled("No operation selected.", theme::hint()),
        )];
    };
    let Operation {
        id,
        has_operation_id: _,
        method,
        path,
        summary,
        description,
        tags,
        scopes,
        parameters,
        request_body,
        response,
        deprecated,
        external_docs,
        unsupported,
        graphql: _,
    } = operation;
    let text = |value: String| -> Span<'static> {
        if value.is_empty() {
            Span::styled(theme::NONE_TEXT, theme::absent())
        } else {
            Span::raw(value)
        }
    };
    let parameters = parameters
        .iter()
        .map(|parameter| {
            let Parameter {
                name,
                location,
                required,
                schema,
                description: _,
            } = parameter;
            format!(
                "{} ({}, {}{})",
                name,
                location.as_str(),
                schema.display_type(),
                if *required { ", required" } else { "" }
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    let body = request_body.as_ref().map(|body| {
        let RequestBody {
            content_type,
            other_content_types: _,
            required,
            schema: _,
            description: _,
        } = body;
        format!(
            "{}{}",
            content_type,
            if *required { " (required)" } else { "" }
        )
    });
    let mut entries = vec![
        ("Operation".to_string(), Span::raw(id.clone())),
        ("Request".to_string(), Span::raw(format!("{method} {path}"))),
        (
            "Summary".to_string(),
            text(summary.clone().unwrap_or_default()),
        ),
    ];
    if let Some(description) = description {
        entries.push(("About".to_string(), Span::raw(first_paragraph(description))));
    }
    entries.push(("Tags".to_string(), text(tags.join(", "))));
    entries.push(("Scopes".to_string(), text(scopes.join(", "))));
    entries.push(("Params".to_string(), text(parameters)));
    entries.push(("Body".to_string(), text(body.unwrap_or_default())));
    if let Some(response) = response {
        let Response {
            status,
            content_type,
            other_content_types: _,
            schema: _,
            description,
        } = response;
        entries.push((
            "Response".to_string(),
            Span::raw(format!("{status} {content_type}")),
        ));
        if let Some(description) = description {
            entries.push((String::new(), Span::raw(first_paragraph(description))));
        }
    }
    entries.push((
        "Docs".to_string(),
        text(external_docs.clone().unwrap_or_default()),
    ));
    if *deprecated {
        entries.push((
            "Status".to_string(),
            Span::styled("deprecated", Tone::Warning.style()),
        ));
    }
    if !unsupported.is_empty() {
        entries.push((
            "Note".to_string(),
            Span::styled(
                format!(
                    "needs {} which kurama does not send",
                    unsupported.join(", ")
                ),
                Tone::Warning.style(),
            ),
        ));
    }
    if let Some(response) = response {
        let shape = serde_json::to_string_pretty(&response.schema.shape()).unwrap_or_default();
        let total = shape.lines().count();
        for (index, line) in shape.lines().take(shape_rows).enumerate() {
            entries.push((
                if index == 0 {
                    "Shape".into()
                } else {
                    String::new()
                },
                Span::raw(line.to_string()),
            ));
        }
        if total > shape_rows {
            entries.push((
                String::new(),
                Span::styled(
                    format!(
                        "… {} more lines; --describe shows the whole shape",
                        total - shape_rows
                    ),
                    theme::hint(),
                ),
            ));
        }
    }
    entries.push((String::new(), Span::raw("")));
    entries.push((
        "Next".to_string(),
        Span::styled("Enter opens the form", theme::hint()),
    ));
    entries
}

/// A panel title cut to the panel's width, so a long operation id leaves
/// room for the rest.
fn panel_title(text: String, area: Rect) -> String {
    fit(&text, (area.width as usize).saturating_sub(4))
}

fn render_form(frame: &mut Frame, area: Rect, operation: &Operation, form: &FormModel) {
    let title = panel_title(format!("{}  {}", operation.id, operation.label()), area);
    Form {
        title: &title,
        fields: operation
            .parameters
            .iter()
            .zip(&form.values)
            .enumerate()
            .map(|(index, (parameter, value))| FormField {
                label: &parameter.name,
                hint: format!(
                    "{}  {}",
                    parameter.location.as_str(),
                    parameter.schema.display_type()
                ),
                value,
                required: parameter.required,
                focused: form.focus == index,
            })
            .collect(),
        body: operation
            .request_body
            .as_ref()
            .zip(form.body.as_deref())
            .map(|(body, text)| FormBody {
                media_type: &body.content_type,
                required: body.required,
                text,
                focused: form.focus == form.values.len(),
            }),
        error: form.error.as_deref(),
        empty_message: "No parameters; Enter sends the request.",
    }
    .render(frame, area);
}

fn render_result(frame: &mut Frame, area: Rect, operation: &Operation, result: &ResultModel) {
    let mut title = format!(
        "{}  {}  {} ms",
        operation.id,
        status_text(result.status),
        result.elapsed_ms
    );
    if let Some(filter) = &result.jq {
        title.push_str(&format!("  jq: {filter}"));
    }
    if let (Some(tree), Some(body)) = (&result.tree, &result.parsed_body) {
        title.push_str("  tree");
        let title = panel_title(title, area);
        let lines: Vec<String> = tree_rows(body, &tree.expanded)
            .iter()
            .map(|row| {
                let marker = match row.kind {
                    RowKind::Expanded => theme::EXPANDED_MARKER,
                    RowKind::Collapsed => theme::COLLAPSED_MARKER,
                    RowKind::Leaf | RowKind::Omitted => "  ",
                };
                format!("{}{marker}{}", "  ".repeat(row.depth), row.text)
            })
            .collect();
        TextView {
            title: &title,
            title_style: theme::status_style(result.status),
            lines: &lines,
            scroll: 0,
            selected: Some(tree.selected.min(lines.len().saturating_sub(1))),
        }
        .render(frame, area);
        return;
    }
    let title = panel_title(title, area);
    let lines = result_lines(result);
    TextView {
        title: &title,
        title_style: theme::status_style(result.status),
        lines: &lines,
        scroll: result.scroll,
        selected: None,
    }
    .render(frame, area);
}

fn modal(model: &ExplorerModel, area: Rect) -> Option<Modal<'static>> {
    let modal = match model.modal.as_ref()? {
        ExplorerModal::Help => Modal {
            title: "Help",
            tone: Tone::Info,
            body: [
                ("↑↓ or j/k", "move through operations"),
                ("/", "search by id, path, summary or tag"),
                ("Enter", "open the form; Enter again sends"),
                ("h", "sent requests and favorites (on the list)"),
                ("Tab", "next field; letters go into it"),
                ("Ctrl-A/E", "move to the start/end of input"),
                ("e or Ctrl-E", "edit the focused body row in $EDITOR"),
                ("c or Ctrl-Y", "copy the CLI command (Ctrl-Y in the form)"),
                ("j", "jq filter on the result"),
                ("h", "show the response headers"),
                ("t", "the response as a tree; y copies a path"),
                ("o", "open the documentation link"),
                ("? or F1", "this help (F1 in the form)"),
                ("Esc", "back"),
                ("q or Ctrl-C", "quit (Ctrl-C in the form)"),
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
        ExplorerModal::Error(message) => Modal {
            title: "Error",
            tone: Tone::Danger,
            body: message
                .lines()
                .map(|line| Line::from(line.to_string()))
                .collect(),
        },
        ExplorerModal::Processing(message) => Modal {
            title: "Sending",
            tone: Tone::Info,
            body: vec![Line::from(message.clone())],
        },
        ExplorerModal::JqInput(input) => super::jq_view::modal(input, area),
        ExplorerModal::History(history) => super::history_view::modal(history, area),
    };
    Some(modal)
}

#[cfg(test)]
#[path = "view_snapshot_tests.rs"]
mod snapshot_tests;
