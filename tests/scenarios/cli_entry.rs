//! Runtime verification scenarios of the `cli-entry` feature that a TOML case
//! cannot state: the binary started with no arguments on a terminal.

use crate::support::Scenario;
use crate::support::tui;

#[test]
fn cli_without_arguments_opens_tui_in_terminal() {
    // No arguments on a terminal: the parser yields the TUI, dispatch starts
    // it, and nothing is assumed, read from 1Password or written until a key
    // asks for it. The failing side, no terminal, is the case
    // `cli_without_command_or_terminal_exits_2_with_hint`.
    let mut t = tui::launch(
        Scenario::tui("cli_without_arguments_opens_tui_in_terminal").for_feature("cli-entry"),
        tui::STANDARD,
    );
    t.wait_for("Profiles (3)")
        .expect_row_with(&["▸", "aws", "default"])
        .expect_footer_contains("q quit")
        .snapshot("home");
    let mut v = t.quit();
    v.expect_exit_code(0)
        .expect_sts_actions(&[])
        .expect_op_calls(0)
        .expect_no_files_written();
    v.finish();
}
#[test]
fn cli_non_interactive_on_a_pty_exits_2_instead_of_opening_the_tui() {
    // A harness that runs kurama on a pseudo terminal for an agent sets
    // KURAMA_AGENT: no arguments is then the same usage error as a pipe,
    // before raw mode or the alternate screen, instead of a screen nobody
    // reads.
    let mut t = tui::launch(
        Scenario::tui("cli_non_interactive_on_a_pty_exits_2_instead_of_opening_the_tui")
            .for_feature("cli-entry")
            .with_env("KURAMA_AGENT", "1"),
        tui::WIDE,
    );
    t.wait_for("error[TERMINAL_REQUIRED]")
        .expect_text("KURAMA_AGENT")
        .expect_text("hint: run `kurama status`")
        .expect_plain_output()
        .snapshot("refused");
    let mut v = t.exit();
    v.expect_error("TERMINAL_REQUIRED", 2)
        .expect_sts_actions(&[])
        .expect_op_calls(0)
        .expect_no_files_written();
    v.finish();
}
#[test]
#[cfg_attr(not(feature = "test-fakes"), ignore = "needs --features test-fakes")]
fn cli_non_interactive_never_rewrites_a_progress_line() {
    // The same slow scan that shows `# scanning 1s` to a person
    // (`data_tty_reports_elapsed_seconds_while_the_engine_runs`) shows an
    // agent's pseudo terminal only the error line.
    let mut t = tui::launch_command(
        Scenario::tui("cli_non_interactive_never_rewrites_a_progress_line")
            .for_feature("cli-entry")
            .with_env("KURAMA_AGENT", "1"),
        tui::STANDARD,
        &[
            "data",
            "--from",
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/data/orders.csv"
            ),
            "--query",
            "SELECT sum(a.i*b.i) FROM range(100000000) a(i), range(100000000) b(i)",
            "--timeout",
            "2",
        ],
    );
    t.wait_for("error[DATA_FAILED]");
    let raw = String::from_utf8_lossy(&t.raw_output()).into_owned();
    let mut v = t.exit();
    v.expect_error("DATA_FAILED", 1)
        .expect_sts_actions(&[])
        .expect_op_calls(0);
    // The line discipline puts `\r` before every `\n`; any other `\r` is kurama's.
    let rewritten = raw.replace("\r\n", "\n").contains('\r');
    v.check(
        "no progress line and no carriage return of kurama's own",
        !raw.contains("# scanning") && !rewritten,
        raw,
    );
    v.finish();
}
