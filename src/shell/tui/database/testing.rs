//! Fixtures for every database explorer state and the explorer's UI
//! contract, for the render regression (`view_snapshot_tests.rs`) and the
//! update tests.

use ratatui::buffer::Buffer;
use serde_json::{Value, json};

use super::effects::DbAsk;
use super::model::{
    CellDetail, DbModal, DbModel, DbSummary, Focus, Grid, Running, Tab, TableEntry,
};
use super::view::{APP_TITLE, READ_ONLY_BADGE, hints, render};
use crate::domain::types::database::{DbColumn, DbEncoding, DbResult, DbStopReason};
use crate::shell::tui::testing::{
    buffer_lines, cell, column_of, has_style, modal_violations, render_frame,
};
use crate::shell::tui::theme::{self, Tone};

pub fn render_buffer(model: &DbModel, width: u16, height: u16) -> Buffer {
    render_frame(width, height, |frame| render(frame, model))
}

/// Violations of the database explorer's contract: the title and the
/// read-only badge in the header, the primary hint in the footer, a panel
/// ending above the footer, the selected table marked in the selection
/// style, a modal inside the viewport in its tone.
pub fn check_contract(model: &DbModel, buffer: &Buffer) -> Vec<String> {
    let lines = buffer_lines(buffer);
    let height = lines.len();
    let mut violations = Vec::new();
    if !lines[0].contains(APP_TITLE) {
        violations.push(format!(
            "header row does not show the title: {:?}",
            lines[0]
        ));
    }
    match column_of(&lines[0], READ_ONLY_BADGE) {
        Some(x) => {
            if !has_style(cell(buffer, x, 0), theme::badge_on()) {
                violations.push("the READ ONLY badge is not drawn in its on style".into());
            }
        }
        None => violations.push(format!("header lacks the READ ONLY badge: {:?}", lines[0])),
    }
    let footer = &lines[height - 1];
    if let Some((key, action)) = hints(model).first()
        && !footer.contains(&format!("{key} {action}"))
    {
        violations.push(format!(
            "footer lacks the primary hint {key} {action}: {footer:?}"
        ));
    }
    if height >= 4 && !lines[height - 2].contains(theme::BORDER.bottom_left) {
        violations.push(format!(
            "row above the footer is not a panel bottom border: {:?}",
            lines[height - 2]
        ));
    }
    let modal = match &model.modal {
        Some(DbModal::Help) => Some(("Help", Tone::Info)),
        Some(DbModal::Cell(_)) => Some(("Cell", Tone::Info)),
        Some(DbModal::Error(_)) => Some(("Error", Tone::Danger)),
        None => None,
    };
    if let Some((title, tone)) = modal {
        violations.extend(modal_violations(buffer, &lines, title, tone));
    } else if model.focus == Focus::Tables
        && let Some(table) = model.selected_table()
    {
        let marked = format!("{}{}", theme::SELECTION_MARKER, table.name);
        match lines
            .iter()
            .enumerate()
            .find_map(|(y, line)| column_of(line, &marked).map(|x| (y, x)))
        {
            None => violations.push(format!("the selected table {} is not marked", table.name)),
            Some((y, x)) => {
                if !has_style(cell(buffer, x, y), theme::selection()) {
                    violations.push("the selection marker is not in the selection style".into());
                }
            }
        }
    }
    violations
}

pub mod fixtures {
    use super::*;

    pub fn text(name: &str, data_type: &str) -> DbColumn {
        DbColumn {
            name: name.into(),
            data_type: Some(data_type.into()),
            encoding: DbEncoding::Text,
        }
    }

    fn table(schema: &str, name: &str, kind: &str) -> TableEntry {
        TableEntry {
            schema: schema.into(),
            name: name.into(),
            kind: kind.into(),
        }
    }

    /// A page of the table list as the catalog answers it.
    pub fn listing(names: &[&str]) -> DbResult {
        DbResult::new(
            vec![
                text("schema", "text"),
                text("name", "text"),
                text("type", "text"),
            ],
            names
                .iter()
                .map(|name| vec![json!("main"), json!(name), json!("table")])
                .collect(),
            None,
            0,
        )
    }

    pub fn rows(columns: &[&str], rows: Vec<Vec<Value>>) -> DbResult {
        DbResult::new(
            columns.iter().map(|name| text(name, "TEXT")).collect(),
            rows,
            None,
            0,
        )
    }

    pub fn loading() -> DbModel {
        DbModel::new(DbSummary {
            name: "app".into(),
            engine: "sqlite 3.46.0".into(),
            database: "/srv/app.sqlite3".into(),
            tunnel: None,
        })
    }

    pub fn loaded() -> DbModel {
        let mut model = loading();
        model.summary = DbSummary {
            name: "cmp-dev".into(),
            engine: "mysql 8.4.3".into(),
            database: "portnoy".into(),
            tunnel: Some("via ssm i-0abc1234def567890".into()),
        };
        model.tables = vec![
            table("portnoy", "aws_accounts", "table"),
            table("portnoy", "aws_account_addresses", "table"),
            table("portnoy", "projects", "table"),
            table("portnoy", "open_projects", "view"),
        ];
        model.more_tables = Some(("portnoy".into(), "open_projects".into()));
        model.tables_loaded = true;
        model.apply_search();
        model
    }

    pub fn columns() -> DbModel {
        let mut model = loaded();
        let row = |cells: [Option<&str>; 6]| {
            cells
                .into_iter()
                .map(|cell| cell.map_or(Value::Null, |text| json!(text)))
                .collect()
        };
        model.columns = Some(Grid {
            source: "aws_accounts".into(),
            result: rows(
                &[
                    "column",
                    "declared_type",
                    "nullable",
                    "has_default",
                    "primary_key",
                    "references",
                ],
                vec![
                    row([
                        Some("account_id"),
                        Some("varchar(12)"),
                        Some("false"),
                        Some("false"),
                        Some("true"),
                        None,
                    ]),
                    row([
                        Some("project_id"),
                        Some("bigint"),
                        Some("false"),
                        Some("false"),
                        Some("false"),
                        Some("projects.id"),
                    ]),
                    row([
                        Some("enabled"),
                        Some("tinyint(1)"),
                        Some("false"),
                        Some("true"),
                        Some("false"),
                        None,
                    ]),
                ],
            ),
            elapsed_ms: 4,
            row: 0,
            column: 0,
        });
        model.tab = Tab::Columns;
        model
    }

    /// Same-named columns, NULL and an empty string, a long cell, full-width
    /// text, a control character, a DECIMAL and a big integer, a base64
    /// column and JSON, cut at max_rows.
    pub fn preview() -> DbModel {
        let mut model = columns();
        let mut binary = text("payload", "blob");
        binary.encoding = DbEncoding::Base64;
        model.preview = Some(Grid {
            source: "aws_accounts".into(),
            result: DbResult::new(
                vec![
                    text("id", "bigint"),
                    text("id", "bigint"),
                    text("note", "text"),
                    text("name", "varchar(64)"),
                    text("amount", "decimal(30,10)"),
                    binary,
                    text("settings", "json"),
                ],
                vec![
                    vec![
                        json!("1"),
                        json!("10"),
                        Value::Null,
                        json!("ACME"),
                        json!("12345678901234567890.1234567890"),
                        json!("AAEC/w=="),
                        json!("{\"region\":\"ap-northeast-1\"}"),
                    ],
                    vec![
                        json!("9223372036854775807"),
                        json!("11"),
                        json!(""),
                        json!("日本語の名前"),
                        json!("0.0000000001"),
                        Value::Null,
                        json!("{}"),
                    ],
                    vec![
                        json!("3"),
                        json!("12"),
                        json!(
                            "a very long note that goes on and on well past any column width the table would give it"
                        ),
                        json!("tab\there"),
                        Value::Null,
                        json!(""),
                        json!("[]"),
                    ],
                ],
                Some(DbStopReason::MaxRows),
                0,
            ),
            elapsed_ms: 12,
            row: 1,
            column: 3,
        });
        model.tab = Tab::Preview;
        model.focus = Focus::Detail;
        model
    }

    pub fn sql() -> DbModel {
        let mut model = preview();
        model.tab = Tab::Sql;
        model.sql = "SELECT status, count(*) FROM orders GROUP BY status".into();
        model
    }

    pub fn running() -> DbModel {
        let mut model = sql();
        model.running = Some(Running {
            ask: DbAsk::Query {
                sql: model.sql.as_str().to_owned(),
            },
            elapsed_secs: 3,
            stopping: false,
        });
        model
    }

    pub fn result() -> DbModel {
        let mut model = sql();
        model.result = Some(Grid {
            source: "query".into(),
            result: rows(
                &["status", "count(*)"],
                vec![
                    vec![json!("open"), json!("2")],
                    vec![json!("paid"), json!("1")],
                ],
            ),
            elapsed_ms: 2,
            row: 0,
            column: 1,
        });
        model.tab = Tab::Result;
        model
    }

    pub fn stopped() -> DbModel {
        let mut model = result();
        model.last_stop = Some(
            "DB_FAILED: the statement was interrupted and the database confirmed it stopped".into(),
        );
        model
    }

    pub fn no_rows() -> DbModel {
        let mut model = result();
        if let Some(grid) = &mut model.result {
            grid.result = rows(&["status", "count(*)"], Vec::new());
        }
        model
    }

    pub fn cell_detail() -> DbModel {
        let mut model = preview();
        model.modal = Some(DbModal::Cell(CellDetail {
            column: "note".into(),
            data_type: Some("text".into()),
            base64: false,
            value: Some(
                "a very long note that goes on and on well past any column width the table would give it"
                    .into(),
            ),
        }));
        model
    }

    pub fn binary_cell() -> DbModel {
        let mut model = preview();
        model.modal = Some(DbModal::Cell(CellDetail {
            column: "payload".into(),
            data_type: Some("blob".into()),
            base64: true,
            value: Some("AAEC/w==".into()),
        }));
        model
    }

    pub fn error() -> DbModel {
        let mut model = sql();
        model.modal = Some(DbModal::Error(
            "DB_REJECTED: the database refused the statement: no such table: order\n\
             hint: check the table and column names with --tables and --describe"
                .into(),
        ));
        model
    }

    pub fn help() -> DbModel {
        let mut model = loaded();
        model.modal = Some(DbModal::Help);
        model
    }

    pub fn searching() -> DbModel {
        let mut model = loaded();
        model.searching = true;
        model.search = "acc".into();
        model.apply_search();
        model
    }

    pub fn no_match() -> DbModel {
        let mut model = loaded();
        model.search = "zzz".into();
        model.apply_search();
        model
    }

    pub fn all() -> Vec<(&'static str, DbModel)> {
        vec![
            ("loading", loading()),
            ("loaded", loaded()),
            ("columns", columns()),
            ("preview", preview()),
            ("sql", sql()),
            ("running", running()),
            ("result", result()),
            ("stopped", stopped()),
            ("no_rows", no_rows()),
            ("cell", cell_detail()),
            ("binary_cell", binary_cell()),
            ("error", error()),
            ("help", help()),
            ("searching", searching()),
            ("no_match", no_match()),
        ]
    }
}
