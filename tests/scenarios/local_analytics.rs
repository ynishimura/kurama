//! Runtime verification scenarios of the `local-analytics` feature.
//! `tests/scenarios/main.rs` holds the naming rule and what each
//! scenario writes.

use crate::support;
use crate::support::Scenario;
use crate::support::tui::{self};

#[test]
fn data_sigint_interrupts_worker_and_removes_export() {
    let scenario = support::Scenario::new(
        "data_sigint_interrupts_worker_and_removes_export",
        "local-analytics",
        &[],
    );
    let mut sandbox = support::Sandbox::create(&scenario);
    let directory = sandbox.home.join("interrupted-export");
    std::fs::create_dir(&directory).unwrap();
    let export = directory.join("result.csv");
    let run = sandbox.run_cli_interrupted(
        &[
            "data",
            "--from",
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/data/orders.csv"
            ),
            "--query",
            "SELECT sum(a.i*b.i) FROM range(100000000) a(i), range(100000000) b(i)",
            "--export",
            export.to_str().unwrap(),
            "--json",
        ],
        &directory,
    );
    let export_cleaned =
        !export.exists() && std::fs::read_dir(&directory).unwrap().next().is_none();
    let mut v = sandbox.finish(scenario.id, scenario.feature, vec![run], vec![]);
    v.expect_json_error("DATA_FAILED", 1)
        .expect_sts_actions(&[])
        .expect_op_calls(0)
        .expect_no_files_written();
    let error = v.observed.last().stderr.clone();
    v.check(
        "SIGINT is reported distinctly and leaves no export artifacts",
        error.contains("interrupted") && export_cleaned,
        error,
    );
    v.finish();
}
#[test]
#[cfg_attr(not(feature = "test-fakes"), ignore = "needs --features test-fakes")]
fn data_tty_reports_elapsed_seconds_while_the_engine_runs() {
    let id = "data_tty_reports_elapsed_seconds_while_the_engine_runs";
    let mut t = tui::launch_command(
        Scenario::tui(id).for_feature("local-analytics"),
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
            "3",
        ],
    );
    // A person watching a long scan sees it is alive, and only the seconds.
    t.wait_for("# scanning 1s");
    let screen = t.screen_text();
    // The second run has no pseudo terminal. It is slow the same way, so a
    // ticker that ignored the terminal check would be seen here.
    let mut v = t.exit_then_run(&[&[
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
    ]]);
    v.check(
        "the elapsed line carries no percentage, estimate or byte count",
        screen.lines().any(|line| line.trim() == "# scanning 1s"),
        screen,
    );
    // A PTY merges both streams; a failed run's terminal text is recorded as
    // its stderr.
    let terminal = v.observed.runs[0].stderr.clone();
    v.check(
        "the progress line ends before the error, which stays one line",
        v.observed.runs[0].exit_code == Some(1)
            && terminal.contains("# scanning")
            && terminal
                .lines()
                .any(|line| line.trim_end().starts_with("error[DATA_FAILED]")),
        terminal,
    );
    let piped = v.observed.runs[1].clone();
    v.check(
        "without a terminal the same wait prints no progress and no carriage return",
        piped.exit_code == Some(1)
            && piped.stdout.is_empty()
            && piped.stderr.starts_with("error[DATA_FAILED]")
            && !piped.stderr.contains('#')
            && !piped.stderr.contains('\r'),
        format!("{}|{}", piped.stdout, piped.stderr),
    );
    v.finish();
}
