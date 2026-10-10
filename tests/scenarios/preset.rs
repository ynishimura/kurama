//! Runtime verification scenarios of the `preset` feature that a TOML case
//! cannot state: the bytes of the file after `preset setup`, compared with
//! the TOML `preset setup --dry-run` printed for the same arguments, in text
//! and in JSON. `tests/scenarios/main.rs` holds the naming rule and what each
//! scenario writes.

use std::os::unix::fs::PermissionsExt;

use crate::support::{Sandbox, Scenario};

/// The file's own bytes -- comments, a key with its trailing comment, the
/// order of its sections, no final newline -- are the start of the file
/// after `preset setup --offline`, then a blank line and exactly the TOML
/// `preset setup --dry-run` prints after `# would append to <path>:`, which
/// is also the `toml` of its JSON report. A save the directory refuses is
/// CONFIG_WRITE_FAILED (exit 1) with the path in the hint, reported as the
/// failed `configure` step, and leaves the file byte for byte as it was.
#[test]
fn preset_setup_keeps_the_existing_comments_and_order() {
    const EXISTING: &str = "# written by hand\n[api.zeta]\nbase_url = \"https://z.example.com\"\n\n\
                            # second, on purpose\n[core]\nlog_level = \"debug\"    # loud";
    const SET: [&str; 2] = ["--set", "secret=op://Agent/kurama-linear/credential"];
    let scenario = Scenario::new(
        "preset_setup_keeps_the_existing_comments_and_order",
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
        sandbox.run_cli(&with_set(&["preset", "setup", "linear", "--dry-run"])),
        sandbox.run_cli(&with_set(&[
            "preset",
            "setup",
            "linear",
            "--dry-run",
            "--json",
        ])),
        sandbox.run_cli(&with_set(&["preset", "setup", "linear", "--offline"])),
    ];
    let saved = std::fs::read_to_string(&config).unwrap();
    let directory = config.parent().unwrap().to_path_buf();
    let set_mode =
        |mode| std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(mode)).unwrap();
    set_mode(0o500);
    runs.push(sandbox.run_cli(&[
        "preset",
        "setup",
        "github",
        "--offline",
        "--set",
        "secret=op://Agent/kurama-github/credential",
    ]));
    set_mode(0o700);
    let after_failure = std::fs::read_to_string(&config).unwrap();
    let path = config.display().to_string();
    let marker = format!("# would append to {path}:\n");
    let shown = runs[0]
        .stdout
        .split_once(&marker)
        .map(|(_, toml)| toml.to_owned())
        .unwrap_or_default();
    let json_toml = serde_json::from_str::<serde_json::Value>(&runs[1].stdout)
        .ok()
        .and_then(|report| report["toml"].as_str().map(str::to_owned))
        .unwrap_or_default();

    let mut v = sandbox.finish(scenario.id, scenario.feature, runs, vec![]);
    v.keyed_run(0, "dry-run", |v| {
        v.expect_exit_code(0)
            .expect_stderr_empty()
            .expect_stdout_contains("nothing was written")
            .expect_stdout_contains(&marker)
    });
    v.keyed_run(1, "dry-run json", |v| {
        v.expect_exit_code(0).expect_stderr_empty()
    });
    v.keyed_run(2, "setup", |v| {
        v.expect_exit_code(0)
            .expect_stderr_empty()
            .expect_stdout_contains("added [auth.linear], [api.linear] to ")
    });
    v.keyed_run(3, "refused write", |v| {
        v.expect_error_beside_stdout("CONFIG_WRITE_FAILED", 1)
            .expect_stdout_contains("configure    failed        error[CONFIG_WRITE_FAILED]\n")
            .expect_stderr_contains(&format!("cannot save {path}: Permission denied"))
            .expect_stderr_contains(&format!(
                "hint: check that the directory of {path} exists and is writable"
            ))
    });
    v.check(
        "the dry run prints the sections it would append",
        shown.contains("[auth.linear]\n") && shown.contains("[api.linear]\n"),
        format!("dry run {shown:?}"),
    )
    .check(
        "the text dry run and the JSON one carry the same TOML",
        json_toml == shown,
        format!("text {shown:?}, json {json_toml:?}"),
    )
    .check(
        "the existing bytes are the start of the file, then a newline, a blank line and the dry run's TOML",
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
    .expect_files_written(&["home/.claude/skills/kurama/SKILL.md", "kurama-config.toml"]);
    v.finish();
}
