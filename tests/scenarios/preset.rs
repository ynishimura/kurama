//! Runtime verification scenarios of the `preset` feature that a TOML case
//! cannot state: the bytes of the file after `preset add`, compared with what
//! `preset show` and `preset add --dry-run` printed for the same arguments.
//! `tests/scenarios/main.rs` holds the naming rule and what each scenario
//! writes.

use std::os::unix::fs::PermissionsExt;

use crate::support::{Sandbox, Scenario};

/// The file's own bytes -- comments, a key with its trailing comment, the
/// order of its sections, no final newline -- are the start of the file
/// after `preset add`, then a blank line and exactly the TOML `preset show`
/// and `preset add --dry-run` print. A save the directory refuses is
/// CONFIG_WRITE_FAILED (exit 1) with the path in the hint, and leaves the
/// file byte for byte as it was.
#[test]
fn preset_add_keeps_the_existing_comments_and_order() {
    const EXISTING: &str = "# written by hand\n[api.zeta]\nbase_url = \"https://z.example.com\"\n\n\
                            # second, on purpose\n[core]\nlog_level = \"debug\"    # loud";
    const SET: [&str; 2] = ["--set", "secret=op://Agent/kurama-linear/credential"];
    let scenario = Scenario::new(
        "preset_add_keeps_the_existing_comments_and_order",
        "preset",
        &[],
    )
    .with_config(EXISTING);
    let mut sandbox = Sandbox::create(&scenario);
    let config = sandbox.kurama_config().to_path_buf();
    let with_set = |args: &[&'static str]| -> Vec<&'static str> {
        args.iter().chain(SET.iter()).copied().collect()
    };
    let mut runs = vec![
        sandbox.run_cli(&with_set(&["preset", "show", "linear"])),
        sandbox.run_cli(&with_set(&["preset", "add", "linear", "--dry-run"])),
        sandbox.run_cli(&with_set(&["preset", "add", "linear"])),
    ];
    let saved = std::fs::read_to_string(&config).unwrap();
    let directory = config.parent().unwrap().to_path_buf();
    let set_mode =
        |mode| std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(mode)).unwrap();
    set_mode(0o500);
    runs.push(sandbox.run_cli(&with_set(&["preset", "add", "github"])));
    set_mode(0o700);
    let after_failure = std::fs::read_to_string(&config).unwrap();
    let path = config.display().to_string();
    let shown = runs[0].stdout.clone();
    let dry_run = runs[1].stdout.clone();

    let mut v = sandbox.finish(scenario.id, scenario.feature, runs, vec![]);
    v.keyed_run(0, "show", |v| {
        v.expect_exit_code(0)
            .expect_stdout_contains("[auth.linear]\n")
    });
    v.keyed_run(1, "dry-run", |v| {
        v.expect_exit_code(0)
            .expect_stderr_contains("nothing was written")
    });
    v.keyed_run(2, "add", |v| {
        v.expect_exit_code(0)
            .expect_stdout_empty()
            .expect_stderr_contains("# added [auth.linear], [api.linear] to ")
    });
    v.keyed_run(3, "refused write", |v| {
        v.expect_error("CONFIG_WRITE_FAILED", 1)
            .expect_stdout_empty()
            .expect_stderr_contains(&format!("cannot save {path}: Permission denied"))
            .expect_stderr_contains(&format!(
                "hint: check that the directory of {path} exists and is writable"
            ))
    });
    v.check(
        "`preset add --dry-run` prints what `preset show` prints",
        dry_run == shown,
        format!("show {shown:?}, dry run {dry_run:?}"),
    )
    .check(
        "the existing bytes are the start of the file, then a newline, a blank line and the shown TOML",
        saved == format!("{EXISTING}\n\n{shown}"),
        format!("observed {saved:?}"),
    )
    .check(
        "a refused save leaves the file byte for byte as it was",
        after_failure == saved,
        format!("observed {after_failure:?}"),
    )
    .expect_sts_actions(&[])
    .expect_op_calls(0)
    .expect_files_written(&["kurama-config.toml"]);
    v.finish();
}
