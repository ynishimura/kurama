//! Runtime verification scenarios of the `database` feature that a TOML case
//! cannot say: the explorer `kurama db <DB>` opens on a terminal, driven on
//! a pseudo terminal against the SQLite fixture. `tests/scenarios/main.rs`
//! holds the naming rule and what each scenario writes.

use crate::support::tui::{self, Key};
use crate::support::{OnePassword, Scenario};

/// The fixture as a `[db.*]` section, and the same file with a short
/// deadline so a statement that never ends meets it within the test.
const APP_CONFIG: &str = concat!(
    "[db.app]\nengine = \"sqlite\"\npath = \"",
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/db/app.sqlite3\"\n\n",
    "[db.slow]\nengine = \"sqlite\"\nquery_timeout_secs = 4\npath = \"",
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/db/app.sqlite3\"\n"
);
/// A statement that runs until something stops it.
const FOREVER: &str = "WITH RECURSIVE forever(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM forever) SELECT count(*) AS n FROM forever";

#[test]
fn tui_db_opens_on_a_terminal_and_exits_2_without_one() {
    let id = "tui_db_opens_on_a_terminal_and_exits_2_without_one";
    let mut t = tui::launch_command(
        Scenario::tui(id)
            .for_feature("database")
            .with_extra_config(APP_CONFIG),
        tui::STANDARD,
        &["db", "app"],
    );
    t.wait_for("Tables (3)")
        .expect_text("kurama db")
        .expect_text("READ ONLY")
        .expect_text("app  sqlite")
        .expect_row_with(&["▸", "customers"])
        .expect_footer_contains("↑↓ move")
        .snapshot("opened");
    t.key(Key::Char('q'));
    let mut v = t.exit_then_run(&[&["db", "app"]]);
    // The last run is the CLI without a terminal; the first the screen.
    v.keyed_run(0, "the screen quits cleanly", |v| v.expect_exit_code(0))
        .expect_error("DB_INVALID", 2)
        .expect_sts_actions(&[])
        .expect_op_calls(0)
        .expect_no_files_written();
    v.finish();
}

#[test]
fn tui_db_lists_tables_columns_and_a_preview_over_one_connection() {
    let id = "tui_db_lists_tables_columns_and_a_preview_over_one_connection";
    let mut t = tui::launch_command(
        Scenario::tui(id)
            .for_feature("database")
            .with_extra_config(APP_CONFIG),
        tui::STANDARD,
        &["db", "app"],
    );
    t.wait_for("Tables (3)")
        .expect_row_with(&["customers"])
        .expect_row_with(&["open_orders (view)"])
        .expect_row_with(&["orders"])
        .key(Key::End)
        .expect_row_with(&["▸", "orders"])
        .key(Key::Enter)
        .wait_for("[Columns]")
        .expect_row_with(&["column", "declared_type", "nullable"])
        .expect_row_with(&["customer_id", "INTEGER", "customers.id"])
        .expect_text("5 rows")
        .snapshot("columns")
        .key(Key::Char('p'))
        .wait_for("[Preview]")
        .expect_row_with(&["id", "customer_id", "status", "amount", "note"])
        .expect_row_with(&["9223372036854775807", "big"])
        .expect_row_with(&["not a number"])
        .expect_text("4 rows")
        .snapshot("preview")
        .key(Key::Esc)
        .key(Key::Char('/'))
        .type_text("cust")
        .wait_for("Tables 1/3  /cust_")
        .expect_row_with(&["▸", "customers"])
        .key(Key::Enter)
        .key(Key::Char('p'))
        .wait_for("[Preview]  SQL  Result  customers")
        .expect_row_with(&["日本語"])
        .snapshot("filtered-preview")
        .resize(tui::SMALL)
        .expect_footer_contains("cell")
        .expect_no_text("Tables (")
        .snapshot("compact")
        .key(Key::Esc)
        .wait_for("Tables 1/3")
        .snapshot("compact-tables");
    let mut v = t.quit();
    v.expect_exit_code(0)
        .expect_sts_actions(&[])
        .expect_op_calls(0)
        .expect_no_files_written();
    v.finish();
}

#[test]
fn tui_db_runs_a_statement_and_scrolls_the_result_with_a_cell_detail() {
    let id = "tui_db_runs_a_statement_and_scrolls_the_result_with_a_cell_detail";
    let mut t = tui::launch_command(
        Scenario::tui(id)
            .for_feature("database")
            .with_extra_config(APP_CONFIG)
            .with_fake_tools(),
        tui::SMALL,
        &["db", "app"],
    );
    t.wait_for("Tables (3)")
        .key(Key::Char('s'))
        .wait_for("[SQL]")
        .type_text("SELECT o.id, o.customer_id, o.status, o.amount, o.note, c.name AS customer, o.id AS id FROM orders o JOIN customers c ON c.id = o.customer_id ORDER BY o.id")
        .key(Key::Enter)
        .wait_for("[Result]")
        .expect_text("4 rows")
        .expect_row_with(&["id", "customer_id", "status"])
        .snapshot("result");
    for _ in 0..5 {
        t.key(Key::Right);
    }
    t.expect_row_with(&["customer", "id"])
        .expect_row_with(&["日本語"])
        .snapshot("scrolled-right")
        .key(Key::Down)
        .key(Key::Char('y'))
        .wait_for("copied to the clipboard")
        .key(Key::Char('Y'))
        .key(Key::Left)
        .key(Key::Enter)
        .wait_for("┏ Cell ")
        .expect_text("column    note")
        .expect_row_with(&["NULL"])
        .snapshot("cell-null")
        .key(Key::Esc)
        .key(Key::Down)
        .key(Key::Enter)
        .wait_for("┏ Cell ")
        .expect_text("(empty string)")
        .snapshot("cell-empty")
        .key(Key::Esc);
    let mut v = t.quit();
    v.expect_exit_code(0)
        .expect_sts_actions(&[])
        .expect_op_calls(0)
        .expect_no_files_written();
    let copied = v.observed.runs[0].clipboard_calls.clone();
    v.check(
        "y copied the cell and Y the row as tab-separated text",
        copied == ["日本語", "2\t2\tpaid\t1.5\t\t日本語\t2"],
        format!("{copied:?}"),
    );
    v.finish();
}

#[test]
fn tui_db_esc_stops_a_running_statement_and_the_next_one_still_runs() {
    let id = "tui_db_esc_stops_a_running_statement_and_the_next_one_still_runs";
    let mut t = tui::launch_command(
        Scenario::tui(id)
            .for_feature("database")
            .with_extra_config(APP_CONFIG),
        tui::STANDARD,
        &["db", "slow"],
    );
    t.wait_for("Tables (3)")
        .key(Key::Char('s'))
        .type_text(FOREVER)
        .key(Key::Enter)
        .wait_for("running the statement…")
        // The clock is redrawn while the statement runs.
        .wait_for("running the statement… 1 s")
        .expect_footer_contains("Esc stop")
        .snapshot("running")
        .key(Key::Esc)
        .wait_for("the database confirmed it stopped")
        .expect_text("SELECT count(*) AS n FROM forever")
        .snapshot("stopped")
        .key(Key::Ctrl('u'))
        .type_text("SELECT count(*) AS n FROM orders")
        .key(Key::Enter)
        .wait_for("[Result]")
        .expect_row_with(&["4"])
        .expect_text("1 row")
        .snapshot("next-runs")
        .key(Key::Char('s'))
        .key(Key::Ctrl('u'))
        .type_text(FOREVER)
        .key(Key::Enter)
        .wait_for("┏ Error ")
        .expect_text("timed out after 4 seconds")
        .snapshot("deadline")
        .key(Key::Enter)
        .key(Key::Esc)
        .key(Key::Char('p'))
        .wait_for("[Preview]")
        .expect_text("2 rows");
    let mut v = t.quit();
    v.expect_exit_code(0)
        .expect_sts_actions(&[])
        .expect_op_calls(0)
        .expect_no_files_written();
    v.finish();
}

/// Everything that can fail before there is anything to look at fails
/// before the screen opens, with the CLI's own error: here a server that
/// does not answer, behind a tunnel that opened and was closed again.
#[test]
fn tui_db_an_unreachable_database_exits_without_opening_the_screen() {
    let id = "tui_db_an_unreachable_database_exits_without_opening_the_screen";
    let mut t = tui::launch_command(
        Scenario::tui(id)
            .for_feature("database")
            .with_fake_tools()
            .onepassword(OnePassword::Enabled)
            .with_extra_config(
                "[db.remote]\n\
                 engine = \"postgresql\"\n\
                 host = \"rds.internal.example.com\"\n\
                 database = \"app\"\n\
                 username = \"reader\"\n\
                 password = \"op://Agent/db/password\"\n\
                 tls = \"verify-ca\"\n\
                 connect_timeout_secs = 2\n\n\
                 [db.remote.tunnel]\n\
                 kind = \"ssm\"\n\
                 aws_profile = \"dev\"\n\
                 instance_name = \"bastion\"\n",
            ),
        tui::WIDE,
        &["db", "remote"],
    );
    t.wait_for("error[DB_UNREACHABLE]")
        .expect_no_text("Tables")
        .snapshot("unreachable");
    let mut v = t.exit();
    v.expect_error("DB_UNREACHABLE", 1)
        .expect_sts_actions(&["AssumeRole"])
        .expect_ssm_calls(&[
            "DescribeInstanceInformation",
            "StartSession",
            "TerminateSession",
        ])
        .expect_op_calls_containing("item get db --format json --vault Agent", 1)
        .expect_no_files_written();
    v.finish();
}

#[test]
fn tui_db_no_color_keeps_the_selection_and_the_read_only_badge() {
    let id = "tui_db_no_color_keeps_the_selection_and_the_read_only_badge";
    let mut t = tui::launch_command(
        Scenario::tui(id)
            .for_feature("database")
            .with_extra_config(APP_CONFIG)
            .with_env("NO_COLOR", "1"),
        tui::STANDARD,
        &["db", "app"],
    );
    t.wait_for("Tables (3)")
        .expect_default_colors()
        .expect_bold_text("▸")
        .expect_bold_text("READ ONLY")
        .snapshot("list")
        .key(Key::Char('p'))
        .wait_for("[Preview]")
        .expect_default_colors()
        .expect_bold_text("▸")
        .snapshot("preview")
        .resize(tui::WIDE)
        .expect_default_colors()
        .expect_bold_text("READ ONLY")
        .snapshot("wide");
    let mut v = t.quit();
    v.expect_exit_code(0)
        .expect_sts_actions(&[])
        .expect_op_calls(0)
        .expect_no_files_written();
    v.finish();
}

#[test]
fn tui_db_dumb_terminal_exits_2_without_control_sequences() {
    let id = "tui_db_dumb_terminal_exits_2_without_control_sequences";
    let mut t = tui::launch_command(
        Scenario::tui(id)
            .for_feature("database")
            .with_extra_config(APP_CONFIG)
            .with_env("TERM", "dumb"),
        tui::WIDE,
        &["db", "app"],
    );
    t.wait_for("error[DB_INVALID]")
        .expect_plain_output()
        .snapshot("unsupported");
    let mut v = t.exit();
    v.expect_error("DB_INVALID", 2)
        .expect_sts_actions(&[])
        .expect_op_calls(0)
        .expect_no_files_written();
    v.finish();
}
