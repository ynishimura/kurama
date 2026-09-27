//! `cargo xtask tui-check`: the one command to run after changing the TUI.
//!
//! The static checks (`main.rs`'s prelude), then the render regression (TestBackend snapshots of every
//! screen state at every verified size plus the UI contract), then the
//! pseudo-terminal scenarios against the real binary. The report lists every
//! captured screen and embeds its text, so an agent reads one file to see what
//! a user would see, and `screen.png` next to it when a change is visual.

use std::path::{Path, PathBuf};

use crate::{Gate, TEST_FEATURES, git, root, scenario_report_dir};

const USAGE: &str = "usage: cargo xtask tui-check [--update-snapshots]";
const SNAPSHOT_DIR: &str = "tests/tui_snapshots";

/// One captured screen, from a scenario report.
struct Screen {
    scenario: String,
    step: String,
    cols: u64,
    rows: u64,
    dir: PathBuf,
}

pub fn tui_check(args: &[String]) -> Result<(), String> {
    let update = match args {
        [] => false,
        [flag] if flag == "--update-snapshots" => true,
        _ => return Err(USAGE.into()),
    };
    let steps: [(&str, Vec<&str>); 2] = [
        (
            "render",
            [
                &["test", "--locked"][..],
                &TEST_FEATURES,
                &["--lib", "--", "tui"],
            ]
            .concat(),
        ),
        (
            "pty",
            [
                &["test", "--locked"][..],
                &TEST_FEATURES,
                &["--test", "scenarios", "--", "tui_"],
            ]
            .concat(),
        ),
    ];
    // Screens and snapshot diffs of a previous run must not pass as current.
    crate::clear_scenario_reports()?;
    for name in pending_snapshots() {
        std::fs::remove_file(root().join(SNAPSHOT_DIR).join(name)).map_err(|e| e.to_string())?;
    }
    let mut gates = crate::static_checks::results();
    for (name, args) in steps {
        eprintln!("==> {name}: cargo {}", args.join(" "));
        let mut command = crate::cargo();
        command.args(&args).current_dir(root());
        if name == "render" && update {
            command.env("KURAMA_UPDATE_SNAPSHOTS", "1");
        }
        let status = command.status().map_err(|e| format!("cargo: {e}"))?;
        gates.push(Gate {
            name: name.into(),
            ok: status.success(),
            detail: if status.success() {
                "passed".into()
            } else {
                "failed; see output above".into()
            },
        });
    }

    let pending = pending_snapshots();
    let changed = changed_snapshots();
    let screens = captured_screens()?;
    let all_ok = gates.iter().all(|gate| gate.ok) && pending.is_empty();
    let report = render_report(&gates, &pending, &changed, &screens, all_ok);
    let out_dir = crate::agent_dir();
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let report_path = out_dir.join("tui-report.md");
    std::fs::write(&report_path, &report).map_err(|e| e.to_string())?;

    // The summary; the screens themselves are in the report.
    let end = report.find("### Screen text").unwrap_or(report.len());
    println!("{}", &report[..end]);
    println!("Full report with every screen: {}", report_path.display());
    if all_ok {
        eprintln!("==> tui-check passed");
        Ok(())
    } else {
        Err("tui-check failed; see the report above".into())
    }
}

/// `.txt.new` files: renderings that differ from their snapshot.
fn pending_snapshots() -> Vec<String> {
    let dir = root().join(SNAPSHOT_DIR);
    let mut pending: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".txt.new"))
        .collect();
    pending.sort();
    pending
}

/// Snapshot files added or modified in the working tree.
fn changed_snapshots() -> Vec<String> {
    git(&["status", "--porcelain", "--", SNAPSHOT_DIR])
        .unwrap_or_default()
        .lines()
        .map(|line| line.trim().to_string())
        .collect()
}

/// Every screen the `tui_*` scenarios captured, from their JSON reports.
fn captured_screens() -> Result<Vec<Screen>, String> {
    let mut screens = Vec::new();
    for entry in std::fs::read_dir(scenario_report_dir())
        .into_iter()
        .flatten()
        .flatten()
    {
        let content = std::fs::read_to_string(entry.path()).map_err(|e| e.to_string())?;
        let report: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| format!("{}: {e}", entry.path().display()))?;
        let scenario = report["scenario"].as_str().unwrap_or_default().to_string();
        for screen in report["observed"]["screens"]
            .as_array()
            .into_iter()
            .flatten()
        {
            screens.push(Screen {
                scenario: scenario.clone(),
                step: screen["step"].as_str().unwrap_or_default().to_string(),
                cols: screen["cols"].as_u64().unwrap_or(0),
                rows: screen["rows"].as_u64().unwrap_or(0),
                dir: root().join(screen["dir"].as_str().unwrap_or_default()),
            });
        }
    }
    screens.sort_by(|a, b| (&a.scenario, &a.step).cmp(&(&b.scenario, &b.step)));
    Ok(screens)
}

fn render_report(
    gates: &[Gate],
    pending: &[String],
    changed: &[String],
    screens: &[Screen],
    all_ok: bool,
) -> String {
    let head = crate::short_head();
    let mut out = format!(
        "## TUI check report\n\nAt `{head}`. Result: **{}**.\n\n",
        if all_ok { "PASS" } else { "FAIL" }
    );
    out.push_str(&Gate::markdown_table(gates));

    out.push_str("\n### Snapshots\n\n");
    let count = std::fs::read_dir(root().join(SNAPSHOT_DIR))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".txt"))
        .count();
    out.push_str(&format!("{count} snapshot(s) in `{SNAPSHOT_DIR}`.\n"));
    if pending.is_empty() {
        out.push_str("No rendering differs from its snapshot.\n");
    } else {
        out.push_str(&format!(
            "\n{} rendering(s) differ from their snapshot. Read each `.txt.new` next to its `.txt`, \
             decide whether the change is intended, then accept with \
             `cargo xtask tui-check --update-snapshots`:\n\n",
            pending.len()
        ));
        for name in pending {
            out.push_str(&format!("- `{SNAPSHOT_DIR}/{name}`\n"));
        }
    }
    if !changed.is_empty() {
        out.push_str(
            "\nSnapshot files changed in the working tree (review the diff before committing):\n\n",
        );
        for line in changed {
            out.push_str(&format!("- `{line}`\n"));
        }
    }

    out.push_str("\n### Captured screens\n\n");
    if screens.is_empty() {
        out.push_str(
            "No screen was captured (the pty gate did not run or no scenario reported).\n",
        );
    } else {
        out.push_str("| Scenario | Step | Size | Files |\n| --- | --- | --- | --- |\n");
        for screen in screens {
            out.push_str(&format!(
                "| `{}` | {} | {}x{} | `{}/screen.{{txt,ansi,png}}` |\n",
                screen.scenario,
                screen.step,
                screen.cols,
                screen.rows,
                relative(&screen.dir)
            ));
        }
    }

    out.push_str(
        "\n### Next steps for the agent\n\n\
         1. Read every screen below (or `screen.txt` in the directory) and compare with the intent.\n\
         2. For a visual change (color, emphasis, layout) look at `screen.png`; `screen.ansi` \
         holds the escape sequences.\n\
         3. The render gate covers 80x24, 100x30, 120x40 and 160x50 for every screen state; the \
         resize scenario covers 120x40 -> 80x24 -> 160x50 on the real binary.\n\
         4. A snapshot is accepted only after its `.txt.new` has been read and the change is intended.\n",
    );

    out.push_str("\n### Screen text\n");
    for screen in screens {
        let text = std::fs::read_to_string(screen.dir.join("screen.txt")).unwrap_or_default();
        out.push_str(&format!(
            "\n#### `{}` / {} ({}x{})\n\n```text\n{}```\n",
            screen.scenario, screen.step, screen.cols, screen.rows, text
        ));
    }
    out
}

fn relative(path: &Path) -> String {
    path.strip_prefix(root())
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}
