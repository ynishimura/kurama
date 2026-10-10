//! Pure rendering of the database explorer.
//!
//! The table list on the left and the tabs on the right (on terminals at
//! least 100 columns wide; a compact one shows the side that has the keys).
//! A result is a [`ResultTable`] with one status line under it. Modals are
//! drawn on top. Regions come from `layout`, widgets from `components`,
//! colors from `theme`.

use strum::VariantArray;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use super::model::{DbModal, DbModel, Focus, Grid, Tab, result_status};
use crate::shell::cli::client::safe_text;
use crate::shell::tui::components::{
    Badge, Header, KeyHints, Modal, ResultTable, fit, panel, wrap,
};
use crate::shell::tui::layout::db_layout;
use crate::shell::tui::theme::{self, Tone};

pub const APP_TITLE: &str = "鞍馬 kurama db";
pub const READ_ONLY_BADGE: &str = "READ ONLY";
const TABLES_TITLE: &str = "Tables";

// Priority order: a narrow terminal drops the last hints first.
const TABLES_HINTS: [(&str, &str); 8] = [
    ("↑↓", "move"),
    ("Enter", "columns"),
    ("p", "preview"),
    ("s", "SQL"),
    ("q", "quit"),
    ("/", "filter"),
    ("Tab", "tabs"),
    ("?", "help"),
];
const GRID_HINTS: [(&str, &str); 8] = [
    ("↑↓←→", "cell"),
    ("Enter", "cell detail"),
    ("y", "copy cell"),
    ("Y", "copy row"),
    ("Tab", "next tab"),
    ("Esc", "tables"),
    ("q", "quit"),
    ("?", "help"),
];
// Letters are typed into the statement, so its own keys carry no letter.
const SQL_HINTS: [(&str, &str); 4] = [
    ("Enter", "run"),
    ("^E", "$EDITOR"),
    ("Tab", "result"),
    ("Esc", "tables"),
];
// Nothing is selected: the keys that act on a table are left out.
const NO_TABLE_HINTS: [(&str, &str); 4] =
    [("s", "SQL"), ("/", "filter"), ("q", "quit"), ("?", "help")];
const SEARCH_HINTS: [(&str, &str); 3] = [("type", "filter"), ("Enter", "keep"), ("Esc", "clear")];
// A tab with nothing to select in it.
const EMPTY_TAB_HINTS: [(&str, &str); 4] = [
    ("Tab", "next tab"),
    ("Esc", "tables"),
    ("q", "quit"),
    ("?", "help"),
];
const RUNNING_HINTS: [(&str, &str); 2] = [("Esc", "stop"), ("q", "quit")];
const MODAL_HINTS: [(&str, &str); 1] = [("Esc", "close")];

pub fn render(frame: &mut Frame, model: &DbModel) {
    let layout = db_layout(frame.area());
    frame.render_widget(header(model), layout.header);
    match layout.detail {
        Some(detail) => {
            render_tables(frame, layout.list, model);
            render_detail(frame, detail, model);
        }
        None => match model.focus {
            Focus::Tables => render_tables(frame, layout.list, model),
            Focus::Detail => render_detail(frame, layout.list, model),
        },
    }
    frame.render_widget(KeyHints::new(hints(model)), layout.footer);
    if let Some(modal) = modal(model) {
        modal.render(frame, frame.area());
    }
}

/// Key hints for the current state, most important first.
pub fn hints(model: &DbModel) -> &'static [(&'static str, &'static str)] {
    if model.modal.is_some() {
        return &MODAL_HINTS;
    }
    if model.running.is_some() {
        return &RUNNING_HINTS;
    }
    if model.searching {
        return &SEARCH_HINTS;
    }
    match (model.focus, model.tab) {
        (Focus::Tables, _) if model.selected_table().is_none() => &NO_TABLE_HINTS,
        (Focus::Tables, _) => &TABLES_HINTS,
        (Focus::Detail, Tab::Sql) => &SQL_HINTS,
        (Focus::Detail, _)
            if model
                .grid()
                .is_some_and(|grid| !grid.result.rows.is_empty()) =>
        {
            &GRID_HINTS
        }
        (Focus::Detail, _) => &EMPTY_TAB_HINTS,
    }
}

fn header(model: &DbModel) -> Header<'_> {
    let summary = &model.summary;
    let subtitle = model.notice.clone().unwrap_or_else(|| {
        let mut text = format!("{}  {}  {}", summary.name, summary.engine, summary.database);
        if let Some(tunnel) = &summary.tunnel {
            text.push_str(&format!("  {tunnel}"));
        }
        text
    });
    Header::new(APP_TITLE, subtitle).badges([Badge {
        label: READ_ONLY_BADGE,
        on: true,
    }])
}

fn tables_title(model: &DbModel, width: usize) -> String {
    let more = if model.more_tables.is_some() { "+" } else { "" };
    let total = model.tables.len();
    if model.searching || !model.search.is_empty() {
        let prefix = format!("{TABLES_TITLE} {}/{total}{more}  /", model.filtered.len());
        let available = width.saturating_sub(prefix.width() + 4);
        let query = if model.searching {
            let (before, after) = model.search.visible_parts(available);
            format!("{before}_{after}")
        } else {
            fit(model.search.as_str(), available)
        };
        format!("{prefix}{query}")
    } else {
        format!("{TABLES_TITLE} ({total}{more})")
    }
}

fn render_tables(frame: &mut Frame, area: Rect, model: &DbModel) {
    let title = tables_title(model, area.width as usize);
    let block = panel(&title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if !model.tables_loaded {
        frame.render_widget(
            Paragraph::new(Span::styled("Listing the tables…", theme::hint())),
            inner,
        );
        return;
    }
    if model.filtered.is_empty() {
        let message = if model.search.is_empty() {
            "The database has no table or view this user can read.".to_owned()
        } else {
            format!(
                "No table matches \"{}\". Esc clears the filter.",
                safe_text(model.search.as_str())
            )
        };
        frame.render_widget(
            Paragraph::new(Span::styled(message, theme::hint())).wrap(Wrap { trim: true }),
            inner,
        );
        return;
    }
    let height = inner.height as usize;
    let first = (model.selected + 1).saturating_sub(height.max(1));
    let marker = theme::SELECTION_MARKER.width();
    let width = (inner.width as usize).saturating_sub(marker);
    let lines: Vec<Line> = model
        .filtered
        .iter()
        .enumerate()
        .skip(first)
        .take(height)
        .filter_map(|(position, index)| {
            let table = model.tables.get(*index)?;
            let selected = position == model.selected;
            let name = if table.kind == "view" {
                format!("{} (view)", table.name)
            } else {
                table.name.clone()
            };
            let text = fit(&safe_text(&name), width);
            Some(if selected {
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
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The tab bar, the current tab written in brackets.
fn tab_bar(model: &DbModel) -> String {
    Tab::VARIANTS
        .iter()
        .map(|tab| {
            if *tab == model.tab {
                format!("[{}]", tab.title())
            } else {
                tab.title().to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("  ")
}

fn render_detail(frame: &mut Frame, area: Rect, model: &DbModel) {
    let mut title = tab_bar(model);
    if let Some(grid) = model.grid() {
        title.push_str(&format!("  {}", safe_text(&grid.source)));
    }
    let title = fit(&title, (area.width as usize).saturating_sub(4));
    let block = panel(&title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [body, status_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    let mut cut = false;
    match (model.tab, model.grid()) {
        (Tab::Sql, _) => render_sql(frame, body, model),
        (_, Some(grid)) => cut = render_grid(frame, body, model, grid),
        (tab, None) => {
            let message = match tab {
                Tab::Columns => "Enter on a table shows its columns.",
                Tab::Preview => "p shows the first rows of the selected table.",
                _ => "s opens the SQL tab; Enter there runs the statement.",
            };
            frame.render_widget(
                Paragraph::new(Span::styled(message, theme::hint())).wrap(Wrap { trim: true }),
                body,
            );
        }
    }
    frame.render_widget(
        Paragraph::new(status_line(model, cut, status_area.width as usize)),
        status_area,
    );
}

fn render_grid(frame: &mut Frame, area: Rect, model: &DbModel, grid: &Grid) -> bool {
    if grid.result.columns.is_empty() {
        return false;
    }
    let names: Vec<&str> = grid
        .result
        .columns
        .iter()
        .map(|column| column.name.as_str())
        .collect();
    ResultTable {
        columns: names,
        rows: &grid.result.rows,
        selected_row: grid.row,
        selected_column: grid.column,
        focused: model.focus == Focus::Detail,
    }
    .render(frame, area)
}

fn render_sql(frame: &mut Frame, area: Rect, model: &DbModel) {
    let width = area.width as usize;
    let typing = model.focus == Focus::Detail && model.running.is_none();
    let mut lines: Vec<Line> = Vec::new();
    if model.sql.as_str().contains('\n') {
        // A statement from the editor keeps its lines; typing still edits
        // it, and the cursor is where the last key left it.
        lines.extend(
            model
                .sql
                .as_str()
                .lines()
                .map(|line| Line::from(fit(&safe_text(line), width))),
        );
    } else if typing {
        lines.push(model.sql.line(width));
    } else if model.sql.is_empty() {
        lines.push(Line::from(Span::styled(
            "s, then type one statement",
            theme::hint(),
        )));
    } else {
        lines.push(Line::from(fit(&safe_text(model.sql.as_str()), width)));
    }
    lines.push(Line::from(""));
    for note in [
        "Enter or F5 runs it in a read-only transaction; one statement a run.",
        "^E (or e from the list) opens it in $EDITOR for several lines.",
    ] {
        for piece in wrap(note, width) {
            lines.push(Line::from(Span::styled(piece, theme::hint())));
        }
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// The line under the tabs: the running request, the stop that ended the
/// last one, or what the result on this tab is.
fn status_line(model: &DbModel, cut: bool, width: usize) -> Line<'static> {
    if let Some(running) = &model.running {
        let text = if running.stopping {
            format!("stopping… {} s", running.elapsed_secs)
        } else {
            format!(
                "{}… {} s  Esc stops it",
                running.ask.label(),
                running.elapsed_secs
            )
        };
        return Line::from(Span::styled(fit(&text, width), Tone::Info.style()));
    }
    if let Some(stop) = &model.last_stop {
        return Line::from(Span::styled(fit(stop, width), Tone::Warning.style()));
    }
    let Some(grid) = model.grid() else {
        return Line::from("");
    };
    let mut text = result_status(grid);
    if cut {
        text.push_str("  … cell cut to fit (Enter)");
    }
    let style = if grid.result.truncated() {
        Tone::Warning.style()
    } else {
        theme::hint()
    };
    Line::from(Span::styled(fit(&text, width), style))
}

fn modal(model: &DbModel) -> Option<Modal<'static>> {
    let modal = match model.modal.as_ref()? {
        DbModal::Help => Modal {
            title: "Help",
            tone: Tone::Info,
            body: [
                ("↑↓ or j/k", "move through the tables or the rows"),
                ("/", "filter the tables fetched so far"),
                ("Enter", "columns of a table; a cell in full"),
                ("p", "first rows of the table"),
                ("s", "type one statement; Enter or F5 runs it"),
                ("e or Ctrl-E", "edit the statement in $EDITOR"),
                ("Tab", "the result side, then the next tab"),
                ("←→", "move through the columns of a result"),
                ("y / Y", "copy the cell / the row as TSV"),
                ("Esc", "stop what runs; back to the tables"),
                ("q or Ctrl-C", "quit (Ctrl-C while typing SQL)"),
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
        DbModal::Cell(cell) => {
            let mut body = vec![
                Line::from(vec![
                    Span::styled("column    ", theme::label()),
                    Span::raw(safe_text(&cell.column)),
                ]),
                Line::from(vec![
                    Span::styled("type      ", theme::label()),
                    Span::raw(
                        cell.data_type
                            .as_deref()
                            .map(safe_text)
                            .unwrap_or_else(|| theme::NONE_TEXT.to_owned()),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("encoding  ", theme::label()),
                    Span::raw(if cell.base64 {
                        "base64: the bytes are not text, so they are shown encoded"
                    } else {
                        "text"
                    }),
                ]),
                Line::from(""),
            ];
            match &cell.value {
                None => body.push(Line::from(Span::styled("NULL", theme::absent()))),
                Some(value) if value.is_empty() => {
                    body.push(Line::from(Span::styled("(empty string)", theme::absent())));
                }
                Some(value) => {
                    body.extend(value.lines().map(|line| Line::from(safe_text(line))));
                }
            }
            Modal {
                title: "Cell",
                tone: Tone::Info,
                body,
            }
        }
        DbModal::Error(message) => Modal {
            title: "Error",
            tone: Tone::Danger,
            body: message
                .lines()
                .map(|line| Line::from(safe_text(line)))
                .collect(),
        },
    };
    Some(modal)
}

#[cfg(test)]
#[path = "view_snapshot_tests.rs"]
mod snapshot_tests;
