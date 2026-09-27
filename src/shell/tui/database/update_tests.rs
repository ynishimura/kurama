use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::json;

use super::*;
use crate::shell::tui::database::model::DbModel;
use crate::shell::tui::database::testing::fixtures;

fn key(model: &mut DbModel, code: KeyCode) -> Vec<DbEffect> {
    update(
        model,
        DbMessage::Key(KeyEvent::new(code, KeyModifiers::NONE)),
    )
}

fn asked(effects: &[DbEffect]) -> Option<&DbAsk> {
    effects.iter().find_map(|effect| match effect {
        DbEffect::Ask(ask) => Some(ask),
        _ => None,
    })
}

fn answer(model: &mut DbModel, ask: DbAsk, outcome: Result<Answered, Failure>) -> Vec<DbEffect> {
    update(
        model,
        DbMessage::Answered(Answer {
            ask,
            outcome,
            elapsed_ms: 7,
        }),
    )
}

/// The first page arrives with a key to continue from; reaching the last
/// row asks for exactly the page after it, and the last page ends it.
#[test]
fn db_tui_the_table_list_continues_from_its_cursor_as_the_selection_reaches_the_end() {
    let mut model = fixtures::loading();
    let first = start(&mut model);
    assert!(matches!(asked(&first), Some(DbAsk::Tables { after: None })));
    let next = Some(("main".to_owned(), "b".to_owned()));
    let effects = answer(
        &mut model,
        DbAsk::Tables { after: None },
        Ok(Answered::Listing {
            result: fixtures::listing(&["a", "b"]),
            next: next.clone(),
        }),
    );
    assert!(effects.is_empty(), "the selection is on the first row");
    assert_eq!(model.tables.len(), 2);
    let effects = key(&mut model, KeyCode::Down);
    assert!(
        matches!(asked(&effects), Some(DbAsk::Tables { after }) if *after == next),
        "the last row asks for the page after it"
    );
    answer(
        &mut model,
        DbAsk::Tables { after: next },
        Ok(Answered::Listing {
            result: fixtures::listing(&["c"]),
            next: None,
        }),
    );
    assert_eq!(model.tables.len(), 3);
    assert!(model.more_tables.is_none());
    let effects = key(&mut model, KeyCode::End);
    assert!(effects.is_empty(), "the whole list is here");
}

#[test]
fn db_tui_the_filter_reads_only_what_was_fetched_and_esc_clears_it() {
    let mut model = fixtures::loaded();
    key(&mut model, KeyCode::Char('/'));
    for ch in "PROJ".chars() {
        let effects = key(&mut model, KeyCode::Char(ch));
        assert!(effects.is_empty(), "the filter asks the server nothing");
    }
    let names: Vec<&str> = model
        .filtered
        .iter()
        .map(|index| model.tables[*index].name.as_str())
        .collect();
    assert_eq!(names, ["projects", "open_projects"]);
    key(&mut model, KeyCode::Enter);
    assert!(!model.searching);
    assert_eq!(model.filtered.len(), 2, "Enter keeps the filter");
    // A filtered list is not a reason to fetch the next page.
    assert!(key(&mut model, KeyCode::End).is_empty());
    key(&mut model, KeyCode::Char('/'));
    key(&mut model, KeyCode::Esc);
    assert_eq!(model.filtered.len(), 4);
}

/// One request at a time: a key that would start another says so and asks
/// nothing, and the one running is not disturbed.
#[test]
fn db_tui_a_request_while_one_runs_is_refused_and_says_why() {
    let mut model = fixtures::loaded();
    let effects = key(&mut model, KeyCode::Enter);
    assert!(
        matches!(asked(&effects), Some(DbAsk::Describe { table, .. }) if table == "aws_accounts")
    );
    for code in [KeyCode::Char('p'), KeyCode::F(5)] {
        model.sql = "SELECT 1".into();
        let effects = key(&mut model, code);
        assert!(asked(&effects).is_none());
        assert_eq!(model.notice.as_deref(), Some(BUSY));
    }
    assert!(matches!(
        model.running.as_ref().map(|running| &running.ask),
        Some(DbAsk::Describe { .. })
    ));
}

/// Esc stops what runs, once; the SQL and the last result stay, and the
/// stop is a line under them rather than an error box. The next run goes.
#[test]
fn db_tui_esc_stops_the_running_statement_and_keeps_the_sql_and_the_last_result() {
    let mut model = fixtures::result();
    model.sql =
        "WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM r) SELECT count(*) FROM r"
            .into();
    let sql = model.sql.as_str().to_owned();
    let effects = key(&mut model, KeyCode::F(5));
    assert!(matches!(asked(&effects), Some(DbAsk::Query { sql: asked }) if *asked == sql));
    update(&mut model, DbMessage::Elapsed(2));
    assert_eq!(model.running.as_ref().unwrap().elapsed_secs, 2);
    let effects = key(&mut model, KeyCode::Esc);
    assert!(matches!(effects.as_slice(), [DbEffect::Stop]));
    assert!(
        key(&mut model, KeyCode::Esc).is_empty(),
        "one stop is enough"
    );
    answer(
        &mut model,
        DbAsk::Query { sql: sql.clone() },
        Err(Failure {
            message: "DB_FAILED: the statement was interrupted".into(),
            stopped: true,
        }),
    );
    assert!(model.running.is_none());
    assert!(model.modal.is_none());
    assert_eq!(model.sql.as_str(), sql);
    assert_eq!(model.result.as_ref().unwrap().result.row_count, 2);
    assert!(model.last_stop.is_some());
    let effects = key(&mut model, KeyCode::F(5));
    assert!(matches!(asked(&effects), Some(DbAsk::Query { .. })));
    assert!(model.last_stop.is_none());
}

/// A failure nobody asked for is an error box; the connection is the
/// runtime's to open again, and the next request is sent as usual.
#[test]
fn db_tui_a_failed_request_shows_the_error_and_the_next_one_still_goes() {
    let mut model = fixtures::loaded();
    let effects = key(&mut model, KeyCode::Char('p'));
    let ask = asked(&effects).cloned().unwrap();
    answer(
        &mut model,
        ask,
        Err(Failure {
            message: "DB_UNREACHABLE: the connection to the database was lost".into(),
            stopped: false,
        }),
    );
    assert!(matches!(model.modal, Some(DbModal::Error(_))));
    key(&mut model, KeyCode::Enter);
    assert!(model.modal.is_none());
    let effects = key(&mut model, KeyCode::Char('p'));
    assert!(matches!(asked(&effects), Some(DbAsk::Preview { .. })));
}

#[test]
fn db_tui_a_result_moves_by_cell_opens_its_detail_and_copies_a_cell_and_a_row() {
    let mut model = fixtures::loaded();
    let effects = key(&mut model, KeyCode::Char('p'));
    let ask = asked(&effects).cloned().unwrap();
    answer(
        &mut model,
        ask,
        Ok(Answered::Rows(fixtures::rows(
            &["id", "id", "note"],
            vec![
                vec![json!("1"), json!("10"), serde_json::Value::Null],
                vec![json!("2"), json!("20"), json!("tab\there")],
            ],
        ))),
    );
    assert_eq!(model.tab, Tab::Preview);
    assert_eq!(model.focus, Focus::Detail);
    key(&mut model, KeyCode::Down);
    key(&mut model, KeyCode::Right);
    key(&mut model, KeyCode::Right);
    key(&mut model, KeyCode::Right);
    let grid = model.grid().unwrap();
    assert_eq!((grid.row, grid.column), (1, 2), "the last column stops it");
    let copied = |effects: Vec<DbEffect>| match effects.as_slice() {
        [DbEffect::CopyToClipboard { text }] => text.clone(),
        _ => panic!("no copy"),
    };
    assert_eq!(copied(key(&mut model, KeyCode::Char('y'))), "tab\there");
    assert_eq!(
        copied(key(&mut model, KeyCode::Char('Y'))),
        "2\t20\ttab\\there"
    );
    key(&mut model, KeyCode::Enter);
    assert!(matches!(
        &model.modal,
        Some(DbModal::Cell(cell)) if cell.column == "note" && cell.value.as_deref() == Some("tab\there")
    ));
    key(&mut model, KeyCode::Esc);
    key(&mut model, KeyCode::Up);
    key(&mut model, KeyCode::Enter);
    assert!(matches!(&model.modal, Some(DbModal::Cell(cell)) if cell.value.is_none()));
}

/// The SQL tab takes letters as text: `q` is typed, not quit; Enter runs.
#[test]
fn db_tui_the_sql_tab_types_letters_and_enter_runs_them() {
    let mut model = fixtures::loaded();
    key(&mut model, KeyCode::Char('s'));
    assert_eq!((model.tab, model.focus), (Tab::Sql, Focus::Detail));
    for ch in "select q".chars() {
        key(&mut model, KeyCode::Char(ch));
    }
    assert!(!model.should_exit);
    assert_eq!(model.sql.as_str(), "select q");
    let edit = update(
        &mut model,
        DbMessage::Key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL)),
    );
    assert!(matches!(edit.as_slice(), [DbEffect::EditSql { text }] if text == "select q"));
    update(
        &mut model,
        DbMessage::SqlEdited(Ok("SELECT 1\nFROM t\n".into())),
    );
    assert_eq!(model.sql.as_str(), "SELECT 1\nFROM t");
    let effects = key(&mut model, KeyCode::Enter);
    assert!(matches!(asked(&effects), Some(DbAsk::Query { sql }) if sql == "SELECT 1\nFROM t"));
    // Nothing to run is said, not sent.
    let mut empty = fixtures::loaded();
    assert!(key(&mut empty, KeyCode::F(5)).is_empty());
    assert!(empty.notice.is_some());
}

#[test]
fn db_tui_quit_works_from_the_list_and_ctrl_c_from_the_sql_tab() {
    let mut model = fixtures::loaded();
    key(&mut model, KeyCode::Char('q'));
    assert!(model.should_exit);
    let mut model = fixtures::sql();
    update(
        &mut model,
        DbMessage::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
    );
    assert!(model.should_exit);
}

/// Every key the footer names does what it says, on every screen.
#[test]
fn db_tui_every_hinted_key_acts() {
    use crate::shell::tui::database::view::hints;
    for (state, model) in fixtures::all() {
        let state_hints = hints(&model);
        for (key_name, action) in state_hints {
            let codes: Vec<KeyEvent> = match *key_name {
                "↑↓" | "↑↓←→" | "type" => continue,
                "Enter" => vec![KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)],
                "Esc" => vec![KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)],
                "Tab" => vec![KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)],
                "^E" => vec![KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL)],
                other => {
                    let ch = other.chars().next().unwrap();
                    vec![KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)]
                }
            };
            for code in codes {
                let mut after = fixtures::all()
                    .into_iter()
                    .find(|(name, _)| *name == state)
                    .unwrap()
                    .1;
                let before = fingerprint(&after);
                let effects = update(&mut after, DbMessage::Key(code));
                assert!(
                    !effects.is_empty() || fingerprint(&after) != before,
                    "{state}: {key_name} {action} did nothing"
                );
            }
        }
    }
}

/// What a key can change, enough to tell that it changed something.
fn fingerprint(model: &DbModel) -> String {
    format!(
        "{:?} {:?} {} {} {} {} {} {:?} {:?}",
        model.focus,
        model.tab,
        model.searching,
        model.search.as_str(),
        model.selected,
        model.should_exit,
        model.modal.is_some(),
        model.notice,
        model.grid().map(|grid| (grid.row, grid.column)),
    )
}

/// The keys the explorer reads besides the footer's: `e` from the list,
/// Backspace in the filter, Shift-Tab in the SQL tab, and a binary cell.
#[test]
fn db_tui_editor_filter_backspace_backtab_and_a_binary_cell() {
    let mut model = fixtures::loaded();
    model.sql = "SELECT 1".into();
    let effects = key(&mut model, KeyCode::Char('e'));
    assert!(matches!(effects.as_slice(), [DbEffect::EditSql { text }] if text == "SELECT 1"));

    key(&mut model, KeyCode::Char('/'));
    for ch in "projz".chars() {
        key(&mut model, KeyCode::Char(ch));
    }
    assert!(model.filtered.is_empty());
    key(&mut model, KeyCode::Backspace);
    assert_eq!(model.search.as_str(), "proj");
    assert_eq!(model.filtered.len(), 2);
    key(&mut model, KeyCode::Esc);

    key(&mut model, KeyCode::Char('s'));
    key(&mut model, KeyCode::BackTab);
    assert_eq!(model.tab, Tab::Preview);

    let mut model = fixtures::preview();
    if let Some(grid) = &mut model.preview {
        grid.row = 0;
        grid.column = 5;
    }
    key(&mut model, KeyCode::Enter);
    assert!(
        matches!(&model.modal, Some(DbModal::Cell(cell)) if cell.base64 && cell.column == "payload")
    );
    key(&mut model, KeyCode::Esc);
    if let Some(grid) = &mut model.preview {
        grid.column = 3;
    }
    key(&mut model, KeyCode::Enter);
    assert!(matches!(&model.modal, Some(DbModal::Cell(cell)) if !cell.base64));
}
