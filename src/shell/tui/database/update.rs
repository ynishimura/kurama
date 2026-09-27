//! Database explorer state and its pure transitions.
//!
//! One request runs at a time: a key that would start another while one
//! runs says so and does nothing, and `Esc` stops the running one. The table
//! list is fetched a page at a time as the selection reaches its end, and
//! the filter reads only what was fetched. No I/O, no clock: the runtime
//! runs the effects and tells the update how long a request has taken.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::Value;

use super::effects::{DbAsk, DbEffect};
use super::messages::{Answer, Answered, DbMessage, Failure};
use super::model::{CellDetail, DbModal, DbModel, Focus, Grid, Running, Tab, TableEntry};
use crate::domain::types::database::DbEncoding;
use crate::shell::tui::components::line_input::typed;
use crate::shell::tui::components::list_navigation::moved_selection;

/// What the screen says when a key would start a second request.
pub const BUSY: &str = "a request is running; Esc stops it";

pub fn update(model: &mut DbModel, message: DbMessage) -> Vec<DbEffect> {
    match message {
        DbMessage::Key(key) => on_key(model, key),
        DbMessage::Resize => Vec::new(),
        DbMessage::Elapsed(seconds) => {
            if let Some(running) = &mut model.running {
                running.elapsed_secs = seconds;
            }
            Vec::new()
        }
        DbMessage::Answered(answer) => on_answer(model, answer),
        DbMessage::Copied(result) => {
            model.notice = Some(match result {
                Ok(()) => "copied to the clipboard".into(),
                Err(message) => format!("copy failed: {message}"),
            });
            Vec::new()
        }
        DbMessage::SqlEdited(result) => {
            match result {
                Ok(text) => {
                    model.sql = text.trim_end().to_owned().into();
                    model.tab = Tab::Sql;
                    model.focus = Focus::Detail;
                }
                Err(message) => model.modal = Some(DbModal::Error(message)),
            }
            Vec::new()
        }
    }
}

/// The first request of the screen: the first page of the table list.
pub fn start(model: &mut DbModel) -> Vec<DbEffect> {
    ask(model, DbAsk::Tables { after: None })
}

fn on_answer(model: &mut DbModel, answer: Answer) -> Vec<DbEffect> {
    model.running = None;
    let Answer {
        ask,
        outcome,
        elapsed_ms,
    } = answer;
    match outcome {
        Err(Failure { message, stopped }) => {
            if stopped {
                // The SQL and the last result stay; the stop is the answer.
                model.last_stop = Some(message);
            } else {
                model.modal = Some(DbModal::Error(message));
            }
            Vec::new()
        }
        Ok(Answered::Listing { result, next }) => {
            model.tables_loaded = true;
            model.more_tables = next;
            model.tables.extend(result.rows.iter().map(|row| {
                let text = |index: usize| {
                    row.get(index)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned()
                };
                TableEntry {
                    schema: text(0),
                    name: text(1),
                    kind: text(2),
                }
            }));
            model.apply_search();
            more_tables(model)
        }
        Ok(Answered::Rows(result)) => {
            let (tab, source) = match &ask {
                DbAsk::Describe { table, .. } => (Tab::Columns, table.clone()),
                DbAsk::Preview { table, .. } => (Tab::Preview, table.clone()),
                DbAsk::Query { .. } | DbAsk::Tables { .. } => (Tab::Result, "query".to_owned()),
            };
            let grid = Some(Grid {
                source,
                result,
                elapsed_ms,
                row: 0,
                column: 0,
            });
            match tab {
                Tab::Columns => model.columns = grid,
                Tab::Preview => model.preview = grid,
                Tab::Result | Tab::Sql => model.result = grid,
            }
            model.tab = tab;
            model.focus = Focus::Detail;
            Vec::new()
        }
    }
}

/// Ask for a request, unless one is running.
fn ask(model: &mut DbModel, ask: DbAsk) -> Vec<DbEffect> {
    if model.running.is_some() {
        model.notice = Some(BUSY.into());
        return Vec::new();
    }
    model.last_stop = None;
    model.running = Some(Running {
        ask: ask.clone(),
        elapsed_secs: 0,
        stopping: false,
    });
    vec![DbEffect::Ask(ask)]
}

/// The next page of the list, once the selection reaches the last row
/// fetched. A filter reads only what is here, so it fetches nothing.
fn more_tables(model: &mut DbModel) -> Vec<DbEffect> {
    let at_the_end = model.selected + 1 >= model.filtered.len();
    match &model.more_tables {
        Some(after) if at_the_end && model.search.is_empty() && model.running.is_none() => {
            let after = Some(after.clone());
            ask(model, DbAsk::Tables { after })
        }
        _ => Vec::new(),
    }
}

fn on_key(model: &mut DbModel, key: KeyEvent) -> Vec<DbEffect> {
    model.notice = None;
    if key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::CONTROL {
        return quit(model);
    }
    if let Some(modal) = &model.modal {
        let closes = match modal {
            DbModal::Help => matches!(
                key.code,
                KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') | KeyCode::Enter
            ),
            DbModal::Cell(_) | DbModal::Error(_) => {
                matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q'))
            }
        };
        if closes {
            model.modal = None;
        }
        return Vec::new();
    }
    if key.code == KeyCode::Esc
        && let Some(running) = &mut model.running
    {
        if running.stopping {
            return Vec::new();
        }
        running.stopping = true;
        return vec![DbEffect::Stop];
    }
    if model.searching {
        return on_search_key(model, key);
    }
    // While a statement runs the SQL tab is not typed into, so q still quits.
    if model.focus == Focus::Detail && model.tab == Tab::Sql && model.running.is_none() {
        return on_sql_key(model, key);
    }
    // Keys every side shares.
    match key.code {
        KeyCode::Char('q') => return quit(model),
        KeyCode::Char('?') => {
            model.modal = Some(DbModal::Help);
            return Vec::new();
        }
        KeyCode::Char('p') => return preview(model),
        KeyCode::Char('s') => {
            model.tab = Tab::Sql;
            model.focus = Focus::Detail;
            return Vec::new();
        }
        KeyCode::Char('e') => {
            return vec![DbEffect::EditSql {
                text: model.sql.as_str().to_owned(),
            }];
        }
        KeyCode::F(5) => return run_sql(model),
        _ => {}
    }
    match model.focus {
        Focus::Tables => on_tables_key(model, key),
        Focus::Detail => on_grid_key(model, key),
    }
}

fn on_tables_key(model: &mut DbModel, key: KeyEvent) -> Vec<DbEffect> {
    match key.code {
        KeyCode::Enter => describe(model),
        KeyCode::Char('/') => {
            model.searching = true;
            Vec::new()
        }
        KeyCode::Tab | KeyCode::Right => {
            model.focus = Focus::Detail;
            Vec::new()
        }
        code => match moved_selection(code, model.selected, model.filtered.len()) {
            Some(index) => {
                model.selected = index;
                more_tables(model)
            }
            None => Vec::new(),
        },
    }
}

fn on_search_key(model: &mut DbModel, key: KeyEvent) -> Vec<DbEffect> {
    match key.code {
        KeyCode::Esc => {
            model.searching = false;
            model.search.clear();
            model.apply_search();
        }
        KeyCode::Enter => model.searching = false,
        KeyCode::Up | KeyCode::Down => {
            if let Some(index) = moved_selection(key.code, model.selected, model.filtered.len()) {
                model.selected = index;
            }
        }
        _ if (typed(key) || !matches!(key.code, KeyCode::Char(_)))
            && model.search.handle_key(key) =>
        {
            model.selected = 0;
            model.apply_search();
        }
        _ => {}
    }
    Vec::new()
}

/// The SQL tab takes letters as text, so its own keys are Enter, F5, ^E
/// and Esc.
fn on_sql_key(model: &mut DbModel, key: KeyEvent) -> Vec<DbEffect> {
    match (key.code, key.modifiers) {
        (KeyCode::Enter, _) | (KeyCode::F(5), _) => run_sql(model),
        (KeyCode::Char('e'), KeyModifiers::CONTROL) => vec![DbEffect::EditSql {
            text: model.sql.as_str().to_owned(),
        }],
        (KeyCode::Esc, _) => {
            model.focus = Focus::Tables;
            Vec::new()
        }
        (KeyCode::Tab, _) => {
            model.tab = Tab::Result;
            Vec::new()
        }
        (KeyCode::BackTab, _) => {
            model.tab = Tab::Preview;
            Vec::new()
        }
        _ => {
            model.sql.handle_key(key);
            Vec::new()
        }
    }
}

fn on_grid_key(model: &mut DbModel, key: KeyEvent) -> Vec<DbEffect> {
    match key.code {
        KeyCode::Esc => {
            model.focus = Focus::Tables;
            return Vec::new();
        }
        KeyCode::Tab => {
            model.tab = model.tab.next();
            return Vec::new();
        }
        KeyCode::BackTab => {
            model.tab = model.tab.previous();
            return Vec::new();
        }
        _ => {}
    }
    let Some(grid) = model.grid_mut() else {
        return Vec::new();
    };
    let columns = grid.result.columns.len();
    match key.code {
        KeyCode::Left | KeyCode::Char('h') => grid.column = grid.column.saturating_sub(1),
        KeyCode::Right | KeyCode::Char('l') => {
            grid.column = (grid.column + 1).min(columns.saturating_sub(1));
        }
        KeyCode::Enter => {
            if let Some(detail) = cell_detail(grid) {
                model.modal = Some(DbModal::Cell(detail));
            }
        }
        KeyCode::Char('y') => {
            if let Some(value) = grid
                .result
                .rows
                .get(grid.row)
                .and_then(|r| r.get(grid.column))
            {
                return vec![DbEffect::CopyToClipboard {
                    text: value.as_str().unwrap_or_default().to_owned(),
                }];
            }
        }
        KeyCode::Char('Y') => {
            if let Some(row) = grid.result.rows.get(grid.row) {
                return vec![DbEffect::CopyToClipboard {
                    text: row_as_tsv(row),
                }];
            }
        }
        code => {
            if let Some(index) = moved_selection(code, grid.row, grid.result.rows.len()) {
                grid.row = index;
            }
        }
    }
    Vec::new()
}

fn describe(model: &mut DbModel) -> Vec<DbEffect> {
    let Some(table) = model.selected_table() else {
        return Vec::new();
    };
    let request = DbAsk::Describe {
        schema: table.schema.clone(),
        table: table.name.clone(),
    };
    ask(model, request)
}

fn preview(model: &mut DbModel) -> Vec<DbEffect> {
    let Some(table) = model.selected_table() else {
        return Vec::new();
    };
    let request = DbAsk::Preview {
        schema: table.schema.clone(),
        table: table.name.clone(),
    };
    ask(model, request)
}

fn run_sql(model: &mut DbModel) -> Vec<DbEffect> {
    if model.sql.as_str().trim().is_empty() {
        model.notice = Some("type a statement first; s opens the SQL tab".into());
        return Vec::new();
    }
    let sql = model.sql.as_str().to_owned();
    ask(model, DbAsk::Query { sql })
}

fn quit(model: &mut DbModel) -> Vec<DbEffect> {
    model.should_exit = true;
    // The runtime stops a running statement and closes the connection on
    // its way out; the update only says the screen is done.
    Vec::new()
}

fn cell_detail(grid: &Grid) -> Option<CellDetail> {
    let column = grid.result.columns.get(grid.column)?;
    let value = grid.result.rows.get(grid.row)?.get(grid.column)?;
    Some(CellDetail {
        column: column.name.clone(),
        data_type: column.data_type.clone(),
        base64: column.encoding == DbEncoding::Base64,
        value: match value {
            Value::Null => None,
            Value::String(text) => Some(text.clone()),
            other => Some(other.to_string()),
        },
    })
}

/// One row as tab-separated text, each cell terminal-safe the way the CLI
/// writes it, so a tab or a newline inside a cell cannot add a column.
fn row_as_tsv(row: &[Value]) -> String {
    row.iter()
        .map(|value| match value {
            Value::Null => String::new(),
            Value::String(text) => crate::shell::cli::client::safe_text(text),
            other => other.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\t")
}

#[cfg(test)]
#[path = "update_tests.rs"]
mod tests;
