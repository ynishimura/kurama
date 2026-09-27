//! Pure TUI rendering for the TEA model.
//!
//! The home screen is header, a row of tabs (AWS / Auth / API / DB / Data),
//! the tab's table, its detail pane (on terminals at least 100 columns wide)
//! and one row of key hints; every other screen is a modal drawn on top of
//! it. Regions come from `layout`, widgets from
//! `components`, colors from `theme`: this file only decides what to show.

use ratatui::Frame;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::sources::{SourceList, Tab};
use super::update::{MFA_CODE_LEN, Screen, TuiModel};
use crate::domain::functions::profile_status::describe_time_left;
use crate::shell::tui::components::{
    Badge, DetailPane, Header, KeyHints, KeyValues, Modal, PROFILE_COLUMNS, ProfileTable, TableRow,
    Tabs, fit,
};
use crate::shell::tui::layout::{HomeLayout, home_tabs_layout};
use crate::shell::tui::tea::messages::ProfileRow;
use crate::shell::tui::theme::{self, Tone};

pub const APP_TITLE: &str = "鞍馬 kurama";
pub const APP_SUBTITLE: &str = "AWS credential switcher";
const PROFILES_TITLE: &str = "Profiles";
const DETAILS_TITLE: &str = "Details";

const SOURCE_COLUMNS: [&str; 2] = ["NAME", "STATE"];
/// Width of the detail pane's labels on the source tabs (`AWS profile`).
const SOURCE_LABEL_WIDTH: usize = 12;

// Priority order: a narrow terminal drops the last hints first.
const HOME_HINTS: [(&str, &str); 9] = [
    ("↑↓", "move"),
    ("Enter", "assume"),
    ("q", "quit"),
    ("/", "search"),
    ("^K", "go to"),
    ("?", "help"),
    ("r", "readonly"),
    ("c", "console"),
    ("Tab", "next tab"),
];
const AUTH_HINTS: [(&str, &str); 5] = source_hints("login");
const API_HINTS: [(&str, &str); 5] = source_hints("explore");
const DB_HINTS: [(&str, &str); 5] = source_hints("open");
const DATA_HINTS: [(&str, &str); 5] = source_hints("copy");
const S3_HINTS: [(&str, &str); 5] = source_hints("open");

const fn source_hints(enter: &'static str) -> [(&'static str, &'static str); 5] {
    [
        ("↑↓", "move"),
        ("Enter", enter),
        ("q", "quit"),
        ("Tab", "next tab"),
        ("?", "help"),
    ]
}
const PALETTE_HINTS: [(&str, &str); 4] = [
    ("type", "search"),
    ("Enter", "go"),
    ("Esc", "close"),
    ("↑↓", "select"),
];
const SEARCH_HINTS: [(&str, &str); 4] = [
    ("type", "filter"),
    ("Enter", "keep"),
    ("Esc", "clear"),
    ("↑↓", "move"),
];
const MFA_HINTS: [(&str, &str); 3] = [("digits", "code"), ("Enter", "submit"), ("Esc", "cancel")];
const HELP_HINTS: [(&str, &str); 1] = [("Esc", "close")];
const ERROR_HINTS: [(&str, &str); 1] = [("Enter", "back")];
const SUCCESS_HINTS: [(&str, &str); 2] = [("Enter", "exit"), ("Esc", "back")];
const PROCESSING_HINTS: [(&str, &str); 0] = [];

/// Render the current model without reading external state or changing it.
pub fn render(frame: &mut Frame, model: &TuiModel) {
    let (tabs, layout) = home_tabs_layout(frame.area());
    frame.render_widget(header(model), layout.header);
    let labels = Tab::ALL.map(Tab::label);
    let selected = Tab::ALL.iter().position(|tab| *tab == model.tab);
    frame.render_widget(Tabs::new(&labels, selected.unwrap_or(0)), tabs);
    match model.sources.list(model.tab) {
        None => render_profiles(frame, model, &layout),
        Some(list) => render_sources(frame, model, list, &layout),
    }
    frame.render_widget(KeyHints::new(hints(model)), layout.footer);
    if let Some(modal) = modal(model) {
        modal.render(frame, frame.area());
    }
    if let Some(palette) = &model.palette {
        super::palette_view::modal(palette, frame.area()).render(frame, frame.area());
    }
}

fn render_profiles(frame: &mut Frame, model: &TuiModel, layout: &HomeLayout) {
    let title = profiles_title(model, layout.list.width as usize);
    let empty_message = empty_message(model);
    ProfileTable {
        title: &title,
        columns: PROFILE_COLUMNS,
        rows: model
            .profiles
            .filtered_profiles
            .iter()
            .map(TableRow::from)
            .collect(),
        selected: model.profiles.selected_index,
        loading: model.profiles.loading,
        empty_message: &empty_message,
    }
    .render(frame, layout.list);
    if let Some(area) = layout.detail {
        DetailPane {
            title: DETAILS_TITLE,
            row: selected_row(model),
        }
        .render(frame, area);
    }
}

/// A source tab: its rows with the facts `kurama status` reports, and the
/// selected row's details with what Enter does.
fn render_sources(frame: &mut Frame, model: &TuiModel, list: &SourceList, layout: &HomeLayout) {
    let (title, section) = match model.tab {
        Tab::Auth => ("Auth sources", "[auth.<name>]"),
        Tab::Api => ("APIs", "[api.<name>]"),
        Tab::Db => ("Databases", "[db.<name>]"),
        Tab::S3 => ("S3 connections", "[s3.<name>]"),
        Tab::Aws | Tab::Data => ("Data workspaces", "[data.<name>]"),
    };
    let title = format!("{title} ({})", list.rows.len());
    let empty_message = format!("No {section} section in config.toml.");
    ProfileTable {
        title: &title,
        columns: SOURCE_COLUMNS,
        rows: list
            .rows
            .iter()
            .map(|row| TableRow {
                active: row.active,
                kind: row.kind,
                name: &row.name,
                state: &row.state,
            })
            .collect(),
        selected: list.selected,
        loading: model.profiles.loading,
        empty_message: &empty_message,
    }
    .render(frame, layout.list);
    let Some(area) = layout.detail else {
        return;
    };
    let entries = match list.rows.get(list.selected) {
        Some(row) => row
            .details
            .iter()
            .map(|(label, value)| (label.to_string(), Span::raw(value.as_str())))
            .chain([
                (String::new(), Span::raw("")),
                (
                    "Next".to_string(),
                    Span::styled(row.next.as_str(), theme::hint()),
                ),
            ])
            .collect(),
        None => Vec::new(),
    };
    KeyValues {
        title: DETAILS_TITLE,
        label_width: SOURCE_LABEL_WIDTH,
        entries,
    }
    .render(frame, area);
}

/// Key hints for the current screen, most important first.
pub fn hints(model: &TuiModel) -> &'static [(&'static str, &'static str)] {
    if model.palette.is_some() {
        return &PALETTE_HINTS;
    }
    match model.screen {
        Screen::ProfileList if model.searching => &SEARCH_HINTS,
        Screen::ProfileList => match model.tab {
            Tab::Aws => &HOME_HINTS,
            Tab::Auth => &AUTH_HINTS,
            Tab::Api => &API_HINTS,
            Tab::Db => &DB_HINTS,
            Tab::Data => &DATA_HINTS,
            Tab::S3 => &S3_HINTS,
        },
        Screen::MfaInput => &MFA_HINTS,
        Screen::Processing { .. } => &PROCESSING_HINTS,
        Screen::Success(_) => &SUCCESS_HINTS,
        Screen::Error(_) => &ERROR_HINTS,
        Screen::Help => &HELP_HINTS,
    }
}

fn header(model: &TuiModel) -> Header<'static> {
    let time_left = selected_row(model)
        .filter(|_| model.tab == Tab::Aws)
        .and_then(|row| row.session_expires_at)
        .map(|expires_at| describe_time_left("MFA", expires_at, model.now));
    let subtitle = model.notice.clone().unwrap_or(APP_SUBTITLE.to_string());
    Header::new(APP_TITLE, subtitle)
        .time_left(time_left)
        .badges([
            Badge {
                label: "readonly",
                on: model.mode.readonly,
            },
            Badge {
                label: "console",
                on: model.mode.console_launch,
            },
        ])
}

fn selected_row(model: &TuiModel) -> Option<&ProfileRow> {
    model
        .profiles
        .filtered_profiles
        .get(model.profiles.selected_index)
}

fn profiles_title(model: &TuiModel, width: usize) -> String {
    let total = model.profiles.all_profiles.len();
    if model.profiles.loading {
        PROFILES_TITLE.to_string()
    } else if model.searching || !model.search_query.is_empty() {
        let prefix = format!(
            "{PROFILES_TITLE} {}/{total}  /",
            model.profiles.filtered_profiles.len()
        );
        let available = width.saturating_sub(prefix.width() + 4);
        let query = if model.searching {
            let (before, after) = model.search_query.visible_parts(available);
            format!("{before}_{after}")
        } else {
            fit(model.search_query.as_str(), available)
        };
        format!("{prefix}{query}")
    } else {
        format!("{PROFILES_TITLE} ({total})")
    }
}

fn empty_message(model: &TuiModel) -> String {
    if model.search_query.is_empty() {
        "No profiles in ~/.aws/config. Add a [profile <name>] section with a role_arn.".to_string()
    } else {
        format!(
            "No profile matches \"{}\". Esc clears the search.",
            model.search_query.as_str()
        )
    }
}

fn modal(model: &TuiModel) -> Option<Modal<'static>> {
    let modal = match &model.screen {
        Screen::ProfileList => return None,
        Screen::MfaInput => {
            let typed = model.mfa_input.value.len().min(MFA_CODE_LEN);
            let code = format!("{}{}", "*".repeat(typed), "_".repeat(MFA_CODE_LEN - typed));
            Modal {
                title: "MFA code",
                tone: Tone::Warning,
                body: vec![
                    entry("Profile", model.mfa_input.profile.clone()),
                    entry("Device", model.mfa_input.serial.clone()),
                    Line::default(),
                    entry("Code", code),
                ],
            }
        }
        Screen::Processing { message } => Modal {
            title: "Processing",
            tone: Tone::Info,
            body: vec![Line::from(message.clone())],
        },
        Screen::Success(success) => Modal {
            title: "Success",
            tone: Tone::Success,
            body: vec![
                entry("Profile", success.profile.clone()),
                entry("Access key", success.access_key_id.clone()),
                Line::default(),
                Line::from("Credentials are ready for the shell."),
            ],
        },
        Screen::Error(error) => {
            let mut body = vec![Line::from(error.message.clone())];
            if error.console_url_on_quit {
                body.extend([
                    Line::default(),
                    Line::from(
                        "Open the console URL in a browser yourself: q quits and prints it.",
                    ),
                    Line::from("Credentials are ready for the shell."),
                ]);
            }
            Modal {
                title: "Error",
                tone: Tone::Danger,
                body,
            }
        }
        Screen::Help => Modal {
            title: "Help",
            tone: Tone::Info,
            body: [
                ("↑↓ or j/k", "move through profiles"),
                ("Enter", "assume the selected role"),
                ("/", "search profiles"),
                ("1-6 or Tab", "switch tabs: AWS Auth API DB Data S3"),
                ("Ctrl-K or :", "go to a profile, API or operation"),
                ("r", "toggle readonly mode"),
                ("c", "toggle console launch"),
                ("? or F1", "show this help"),
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
    };
    Some(modal)
}

fn entry(label: &'static str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<12}"), theme::label()),
        Span::raw(value),
    ])
}

#[cfg(test)]
#[path = "view_snapshot_tests.rs"]
mod snapshot_tests;
