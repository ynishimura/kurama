//! `cargo xtask`: the commands an agent (or a human) runs against this
//! repository. It wraps cargo and git so callers never have to remember which
//! tool does what, and it turns `.agent/features/` plus the scenario
//! reports written by `tests/scenarios/` into impact analysis and a
//! verification report. Everything here must stay describable in one line
//! each in docs/development/commands.md.

mod architecture_audit;
mod architecture_fixtures;
mod board;
mod branch_check;
mod cases;
mod check_command;
mod conflicts;
mod deps;
mod doctor;
mod features;
mod gate;
mod impact;
mod install_signed;
mod inventory;
mod issue_check;
mod keychain_partition;
mod map;
mod matrix;
mod matrix_inputs;
mod matrix_report;
mod mutate;
mod ready;
mod real_declaration;
mod scenarios_check;
mod static_checks;
mod sweep;
mod throwaway_stacks;
mod tui_check;
mod verify;
mod verify_layer;
mod verify_real;
mod verify_real_layer;
mod verify_throwaway;
mod worktree;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

pub(crate) use check_command::architecture_gate;
pub(crate) use features::{Feature, FeatureMap, load_features};
pub(crate) use gate::Gate;
pub(crate) use impact::{
    claimed_files, compute_impact, default_base, feature_entries, feature_owns_file,
    including_module, matches_entry,
};
pub(crate) use verify::{
    clear_scenario_reports, conclude_in, coverage_gate_in, list_scenarios, qualify_scenarios,
    scenario_name, verify_against, write_report_in,
};

const HELP: &str = "\
cargo xtask <command>

  check                 every test, the architecture rules, the scenario coverage;
                        writes target/agent/verification-report.{json,md};
                        then caps target/ at 12GB
  sweep                 that cap on its own: the incremental caches first, so a
                        DuckDB build the next gate needs is not what is evicted
  map [FEATURE]         feature map (.agent/features/) with files, tests, scenarios
  search <PATTERN>      public symbols whose name contains PATTERN, with file:line and doc
  impact [--base REF]   changed files -> features and their dependents -> test filters
                        -> scenarios (JSON)
  conflicts <ISSUE>...  whether issues can be implemented at the same time, from the
                        `Affects: <feature>, ...` line each issue body declares [--json]
  verify <TARGET>       run scenarios, write target/agent/verification-report.{json,md}
                        TARGET: all | affected | <feature> | <scenario name prefix>
                        --layer local [all|FEATURE]: db-up, the cases that declare
                        [local] against those databases, db-down whatever happened;
                        reports under target/agent/scenarios-local/ (not-run.txt says why not)
                        --layer throwaway [all|FEATURE] [--profile NAME] [--yes]: the AWS
                        stacks the cases that declare [throwaway] name (tests/api,
                        tests/db), created, verified against and deleted in one run;
                        without --yes it prints the estimate and stops (approval needed)
                        --layer real [all|FEATURE]: the cases that declare [real] against
                        the real services, each only when what it `requires` is there
                        (a profile, a config section, the keychain token); reports under
                        target/agent/scenarios-real/, scanned for credentials first
  tui-check             TUI render snapshots and UI contract, PTY scenarios
                        against the real binary; writes target/agent/tui-report.md with
                        every captured screen (--update-snapshots accepts reviewed changes)
  mutate [PATH ...]     mutation testing of the branch diff, or of PATH (cargo-mutants):
                        a surviving mutant is a test that passes for the wrong reason;
                        writes target/agent/mutation-report.md
                        [--base REF] [--timeout SECONDS]
  db-up                 start the PostgreSQL 17/18 and MySQL 8.4 the real-database
                        tests read, with TLS on and the fixture loaded (needs Docker),
                        then: KURAMA_TEST_DB=1 cargo test --features test-fakes --test real_db
  db-down               remove those databases again
  doctor                toolchain, metadata, test filters and fake-environment checks
  install-signed        build --release, code sign with a stable identity, install it
                        as ~/.cargo/bin/kurama so keychain grants survive a rebuild
                        [--as NAME (install as ~/.cargo/bin/NAME, same signature)]
                        [--identity NAME (kurama-dev)] [--identifier ID (dev.kurama.cli)]
  scenarios-check <ISSUE>
                        whether the scenarios the issue declares under `## シナリオ`
                        exist, which ones this branch adds undeclared, and which no
                        feature claims [--base REF] [--json]
  branch-check          the branch-side gate: the tests of the affected features, the affected scenarios, then mutation testing of
                        the changed src/ and xtask/src/ files. `check` is the
                        integration-side one; at most KURAMA_XTASK_GATE_SLOTS
                        (default 2) run at once, the others wait for a slot
                        [--base REF]
  worktree <SUB>        add <ISSUE>... [SLUG] | list | remove <ISSUE>: one worktree per
                        issue, or per batch of issues, branched from main, its target/
                        cloned from the main checkout's, with the size of every target/
  ready                 the issues an agent may start now: open, unblocked, nobody
                        on them, not a tracker, not a job for a person, and not
                        editing what a running branch edits [--all] [--json];
                        --agents N also picks N offers no two of which are serial;
                        batches: serial offers of one feature, for one worktree
  issue-check <FILE|-|ISSUE>
                        the Affects: line and the `## シナリオ` items of an issue
                        body, before it is posted; a file or stdin reads nothing
                        from GitHub
  claim <ISSUE>         move an issue to In progress without a worktree, refused
                        as `worktree add` refuses; --release <ISSUE> gives it back
  board <SUB>           set-status <STATUS> <ISSUE>...: many issues to one status
                        in one request, for a stocktaking
  deps                  imports that cross into a feature `depends_on` does not name,
                        and what to add [--json]
  architecture-audit    the architecture rules of tests/architecture/rules.toml against
                        their tests, fixtures, exemptions and the list in its main.rs;
                        writes target/agent/architecture-report.{json,md}
                        [--run-fixtures: also run the fixtures and report the rates]
  verify-real <FEATURE> run the real-environment probe a feature declares under `real`,
                        when the variables it requires are set, and write the evidence
                        to target/agent/real/; --check [FEATURE ...] holds the evidence
                        to the current files without running anything
  inventory             what the binary has -- commands, options, config keys with
                        their kinds and enum values, secret schemes, client kinds --
                        read from the hidden `kurama inventory`, with the registered
                        scenarios no report was written for; writes
                        target/agent/inventory.{json,md} [--json | --md] [--from FILE]
  verify-matrix         every enumerated value of `kurama inventory` against the cases
                        (tests/cases/), the declarations (not-applicable.toml,
                        needs-human.toml) and the reports `verify` wrote; writes
                        target/agent/verification-matrix.{json,md}; fails on FAIL,
                        on an UNCLASSIFIED value and on a declaration of other than one
                        enumerated value; a layer run on another commit is stale
                        [--json | --md] [--from FILE]
  generate-cases        write tests/scenarios/cases_generated.rs, one test per case
                        under tests/cases/<feature>/; rerun it when a case is added,
                        removed or renamed

check, branch-check, verify, tui-check and mutate run the static checks first
and stop when one fails: tests/scenarios/cases_generated.rs matches the cases,
cargo fmt --check, cargo clippy -D warnings (all targets, test-fakes) and
cargo machete when it is installed.
";

/// What a command runs before its own work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prelude {
    /// It checks the tree by building or running it, so format, clippy and
    /// unused dependencies go first: they answer in a minute or two, where
    /// the tests, scenarios and mutants after them take many more.
    StaticChecks,
    /// It reads files or GitHub, builds something that is not a check, or
    /// observes a real service.
    Nothing,
}

/// Every command `main` dispatches, with its prelude. A command without a row
/// here is an unknown command, so adding one means deciding this.
const COMMANDS: [(&str, Prelude); 26] = [
    ("check", Prelude::StaticChecks),
    ("sweep", Prelude::Nothing),
    ("map", Prelude::Nothing),
    ("search", Prelude::Nothing),
    ("impact", Prelude::Nothing),
    ("conflicts", Prelude::Nothing),
    ("verify", Prelude::StaticChecks),
    ("tui-check", Prelude::StaticChecks),
    ("mutate", Prelude::StaticChecks),
    ("db-up", Prelude::Nothing),
    ("db-down", Prelude::Nothing),
    ("doctor", Prelude::Nothing),
    ("install-signed", Prelude::Nothing),
    ("scenarios-check", Prelude::Nothing),
    ("branch-check", Prelude::StaticChecks),
    ("worktree", Prelude::Nothing),
    ("ready", Prelude::Nothing),
    ("issue-check", Prelude::Nothing),
    ("claim", Prelude::Nothing),
    ("board", Prelude::Nothing),
    ("deps", Prelude::Nothing),
    // The rules are read from files; `--run-fixtures` runs inside `check`.
    ("architecture-audit", Prelude::Nothing),
    // It observes a real service and records what it saw; the branch gate,
    // which runs the static checks first, is what holds the code to it.
    ("verify-real", Prelude::Nothing),
    // It runs the built binary and reads files; nothing it prints is a gate.
    ("inventory", Prelude::Nothing),
    // It reads the reports `verify` wrote and runs nothing itself; `check` and
    // `branch-check`, which run the static checks first, are what gate on it.
    ("verify-matrix", Prelude::Nothing),
    // It writes a file from the cases; the static checks hold the file to them.
    ("generate-cases", Prelude::Nothing),
];

/// The prelude of the command `args` names.
fn prelude(args: &[String]) -> Result<(), String> {
    match prelude_of(args) {
        Prelude::StaticChecks => static_checks::run(),
        Prelude::Nothing => Ok(()),
    }
}

/// What runs before the command `args` names; `--help` and an unknown
/// command run nothing.
fn prelude_of(args: &[String]) -> Prelude {
    let Some(name) = args.first() else {
        return Prelude::Nothing;
    };
    let asks_for_help = args[1..].iter().any(|arg| arg == "--help" || arg == "-h");
    // A layer run starts the databases and the scenario binary `verify`
    // already gated; the static checks would only delay `db-down`.
    let runs_a_layer = name == "verify" && args.get(1).is_some_and(|arg| arg == "--layer");
    match COMMANDS.iter().find(|(command, _)| command == name) {
        Some((_, Prelude::StaticChecks)) if !asks_for_help && !runs_a_layer => {
            Prelude::StaticChecks
        }
        _ => Prelude::Nothing,
    }
}

/// A `branch-check` that will run waits for one of the N slots first;
/// `--help` and every other command take none.
fn branch_slot(args: &[String]) -> Result<Option<branch_check::GateLock>, String> {
    if takes_a_branch_slot(args) {
        branch_check::wait_for_branch_slot().map(Some)
    } else {
        Ok(None)
    }
}

fn takes_a_branch_slot(args: &[String]) -> bool {
    args.first().is_some_and(|name| name == "branch-check")
        && prelude_of(args) == Prelude::StaticChecks
}

/// Cargo features every test run needs (file-backed session cache for scenarios).
pub(crate) const TEST_FEATURES: [&str; 2] = ["--features", "test-fakes"];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // The slot is held until the command returns: the static checks are part
    // of the load it bounds.
    let result = branch_slot(&args).and_then(|_slot| prelude(&args).and_then(|()| dispatch(&args)));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn dispatch(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("check") => check_command::check(),
        Some("sweep") => sweep::sweep(&args[1..]),
        Some("map") => map::map(args.get(1).map(String::as_str)),
        Some("search") => map::search(args.get(1).map(String::as_str)),
        Some("impact") => impact::impact_command(&args[1..]),
        Some("conflicts") => conflicts::conflicts(&args[1..]),
        Some("verify") if args.get(1).is_some_and(|arg| arg == "--layer") => {
            verify_layer::verify_layer(args.get(2).ok_or(verify_layer::USAGE)?, &args[3..])
        }
        Some("verify") => verify::verify(args.get(1).map(String::as_str)),
        Some("tui-check") => tui_check::tui_check(&args[1..]),
        Some("mutate") => mutate::mutate(&args[1..]),
        Some("db-up") => databases("up.sh"),
        Some("db-down") => databases("down.sh"),
        Some("doctor") => doctor::doctor(),
        Some("install-signed") => install_signed::install_signed(&args[1..]),
        Some("scenarios-check") => scenarios_check::scenarios_check(&args[1..]),
        Some("branch-check") => branch_check::branch_check(&args[1..]),
        Some("ready") => ready::ready(&args[1..]),
        Some("worktree") => worktree::worktree(&args[1..]),
        Some("issue-check") => issue_check::issue_check(&args[1..]),
        Some("claim") => worktree::claim(&args[1..]),
        Some("board") => board::board(&args[1..]),
        Some("deps") => deps::deps(&args[1..]),
        Some("architecture-audit") => architecture_audit::architecture_audit(&args[1..]),
        Some("verify-real") => verify_real::verify_real(&args[1..]),
        Some("inventory") => inventory::inventory(&args[1..]),
        Some("verify-matrix") => matrix::verify_matrix(&args[1..]),
        Some("generate-cases") => cases::generate_cases(),
        Some("--help") | Some("-h") | Some("help") | None => {
            print!("{HELP}");
            Ok(())
        }
        Some(other) => Err(format!("unknown command `{other}`\n\n{HELP}")),
    }
}

/// Start or remove the databases the real-database tests read. The script is
/// what talks to Docker; nothing else in the repository needs it, and the gate
/// never runs it.
pub(crate) fn databases(script: &str) -> Result<(), String> {
    let path = root().join("tests/db").join(script);
    let status = Command::new(&path)
        .status()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("{} failed", path.display()))
}

// ---------------------------------------------------------------------------
// shared helpers
// ---------------------------------------------------------------------------

/// The repository xtask works on: the one it was built from, unless
/// `KURAMA_XTASK_ROOT` names another. The override exists for
/// `xtask/tests/`, which run `worktree add` against a scratch repository so
/// that a test never adds a worktree to this one.
pub(crate) fn root() -> PathBuf {
    std::env::var_os("KURAMA_XTASK_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .to_path_buf()
        })
}

pub(crate) fn scenario_report_dir() -> PathBuf {
    agent_dir().join("scenarios")
}

/// `target/`, or what `CARGO_TARGET_DIR` names.
pub(crate) fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root().join("target"))
}

/// `target/agent/`, under `CARGO_TARGET_DIR` when it is set: where every
/// report xtask writes goes.
pub(crate) fn agent_dir() -> PathBuf {
    target_dir().join("agent")
}

fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// The `.json` files directly under `dir`, in path order.
pub(crate) fn json_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    Ok(files)
}

fn git(args: &[&str]) -> Result<String, String> {
    // quotepath=off keeps non-ASCII file names readable in the JSON output.
    let mut full = vec!["-c", "core.quotepath=off"];
    full.extend_from_slice(args);
    run_in_root("git", &full)
}

/// The variables `cargo run` sets for the crate it runs: xtask's own
/// manifest, package and binary. A cargo started from xtask inherits them,
/// and ring's build reads `CARGO_MANIFEST_DIR`, so a build started from
/// `cargo xtask` and one started from the shell each found the other's ring
/// stale -- and through rustls and reqwest, the build script of
/// libduckdb-sys: five minutes of DuckDB after every switch.
const INHERITED_FROM_CARGO_RUN: [&str; 6] = [
    "CARGO_MANIFEST_DIR",
    "CARGO_MANIFEST_PATH",
    "CARGO_CRATE_NAME",
    "CARGO_BIN_NAME",
    "CARGO_PRIMARY_PACKAGE",
    "CARGO_RUSTC_CURRENT_DIR",
];

/// `cargo`, as the shell would start it. Every cargo xtask starts goes
/// through here (`tests/architecture/`), so a build from a gate and a build
/// from the shell see the same environment and reuse each other's work.
pub(crate) fn cargo() -> Command {
    let mut command = Command::new("cargo");
    for (key, _) in std::env::vars_os() {
        let key = key.to_string_lossy();
        if key.starts_with("CARGO_PKG_") || INHERITED_FROM_CARGO_RUN.contains(&key.as_ref()) {
            command.env_remove(key.as_ref());
        }
    }
    command
}

/// The value after `flag` on the command line, or what the command's `usage`
/// says it takes.
pub(crate) fn value_of<'a>(
    flag: &str,
    rest: &mut impl Iterator<Item = &'a str>,
    usage: &str,
) -> Result<String, String> {
    rest.next()
        .map(str::to_string)
        .ok_or_else(|| format!("{flag} needs a value\n\n{usage}"))
}

/// Whether `cargo <subcommand>` is there: its `--version` answers.
pub(crate) fn cargo_has(subcommand: &str) -> bool {
    cargo()
        .args([subcommand, "--version"])
        .current_dir(root())
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// A cargo plugin (`cargo-machete`, `cargo-sweep`) invoked directly, without
/// the `CARGO` / `CARGO_*` variables that `cargo run` sets for xtask itself:
/// with `CARGO` present the tool expects cargo-style arguments
/// (`cargo-machete machete ...`) and panics.
pub(crate) fn cargo_plugin(program: &str) -> Command {
    let mut command = Command::new(program);
    for (key, _) in std::env::vars() {
        if key == "CARGO" || key.starts_with("CARGO_") {
            command.env_remove(&key);
        }
    }
    command
}

/// Whether the plugin `program` is installed: its `--version` answers.
pub(crate) fn plugin_installed(program: &str) -> bool {
    cargo_plugin(program)
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// The commit HEAD is at, abbreviated, or nothing when git cannot say.
pub(crate) fn short_head() -> String {
    git(&["rev-parse", "--short", "HEAD"])
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// A command line as the tests write it.
#[cfg(test)]
pub(crate) fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

fn run_in_root(program: &str, args: &[&str]) -> Result<String, String> {
    let mut command = if program == "cargo" {
        cargo()
    } else {
        Command::new(program)
    };
    let output = command
        .args(args)
        .current_dir(root())
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(format!(
            "{program} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// The test module of this file is named after it: xtask's test binary lists
/// these tests as `main_tests::...`, which is the filter `derived_filter`
/// gives this file. `tests::` would select every unit test of the workspace.
#[cfg(test)]
mod main_tests {
    use std::collections::BTreeSet;

    use super::*;

    /// The commands that build or run the tree check it statically first;
    /// the ones that read files or GitHub do not wait for clippy.
    #[test]
    fn the_checking_commands_run_the_static_checks_first() {
        let checking: Vec<&str> = COMMANDS
            .iter()
            .filter(|(_, prelude)| *prelude == Prelude::StaticChecks)
            .map(|(name, _)| *name)
            .collect();
        assert_eq!(
            checking,
            ["check", "verify", "tui-check", "mutate", "branch-check",]
        );
        assert!(HELP.contains(
            "check, branch-check, verify, tui-check and mutate run the static checks first"
        ));
    }

    /// A checking command runs the static checks whatever its arguments,
    /// except `--help`; a reading command and an unknown one run nothing.
    #[test]
    fn the_prelude_follows_the_table_and_help_runs_none() {
        for command in [
            &["branch-check"][..],
            &["check"],
            &["verify", "affected"],
            &["mutate", "--base", "dev"],
            &["tui-check", "--update-snapshots"],
        ] {
            assert_eq!(
                prelude_of(&args(command)),
                Prelude::StaticChecks,
                "{command:?}"
            );
        }
        for command in [
            &["branch-check", "--help"][..],
            &["mutate", "-h"],
            &["map", "api-client"],
            &["verify-real", "database"],
            &["verify", "--layer", "local", "database"],
            &["no-such-command"],
            &[],
        ] {
            assert_eq!(prelude_of(&args(command)), Prelude::Nothing, "{command:?}");
        }
    }

    /// Only a `branch-check` that runs takes a slot; `check` has its own lock.
    #[test]
    fn only_a_running_branch_check_takes_a_slot() {
        assert!(takes_a_branch_slot(&args(&["branch-check"])));
        assert!(takes_a_branch_slot(&args(&[
            "branch-check",
            "--base",
            "dev"
        ])));
        for command in [
            &["branch-check", "--help"][..],
            &["check"],
            &["verify", "affected"],
            &[],
        ] {
            assert!(!takes_a_branch_slot(&args(command)), "{command:?}");
        }
    }

    #[test]
    fn sweep_help_names_the_target_cap() {
        assert!(
            HELP.contains("check") && HELP.contains(sweep::TARGET_MAX_SIZE),
            "check is the command that names the cap"
        );
    }

    // -----------------------------------------------------------------------
    // the three lists of commands
    // -----------------------------------------------------------------------

    /// The commands `main` dispatches: every `Some("<name>")` arm of its
    /// `match`, except the spellings of help.
    fn dispatched_commands() -> BTreeSet<String> {
        let source = include_str!("main.rs");
        let start = source
            .find("    match args.first().map(String::as_str) {")
            .expect("dispatch matches on the first argument");
        let end = start
            + source[start..]
                .find("Some(other) =>")
                .expect("the dispatch ends with the unknown-command arm");
        source[start..end]
            .split("Some(\"")
            .skip(1)
            .filter_map(|rest| rest.split_once('"').map(|(name, _)| name.to_string()))
            .filter(|name| !matches!(name.as_str(), "--help" | "-h" | "help"))
            .collect()
    }

    /// The commands `HELP` names: the first word of every line indented by
    /// exactly two spaces.
    fn help_commands() -> BTreeSet<String> {
        HELP.lines()
            .filter(|line| line.starts_with("  ") && !line.starts_with("   "))
            .filter_map(|line| line.split_whitespace().next())
            .map(str::to_string)
            .collect()
    }

    /// The commands the table of docs/development/commands.md names, from the first
    /// cell of each row. A cell names several commands as `` `cargo xtask
    /// db-up` / `db-down` ``; after a command that takes arguments the other
    /// pieces are its subcommands (`worktree add` / `list` / `remove`).
    fn documented_commands(agents: &str) -> BTreeSet<String> {
        let mut commands = BTreeSet::new();
        for line in agents.lines() {
            let Some(cell) = line
                .strip_prefix("| `cargo xtask ")
                .and_then(|rest| rest.split(" | ").next())
            else {
                continue;
            };
            let pieces: Vec<&str> = cell.split(" / ").map(|p| p.trim_matches('`')).collect();
            let first: Vec<&str> = pieces[0].split_whitespace().collect();
            commands.insert(first[0].to_string());
            if first.len() == 1 {
                for piece in &pieces[1..] {
                    let piece = piece.strip_prefix("cargo xtask ").unwrap_or(piece);
                    if let Some(word) = piece.split_whitespace().next() {
                        commands.insert(word.to_string());
                    }
                }
            }
        }
        commands
    }

    fn missing(from: &BTreeSet<String>, against: &BTreeSet<String>) -> Vec<String> {
        against.difference(from).cloned().collect()
    }

    /// Each branch appends a command to three hand-written lists and one
    /// `match`, and a branch that forgets one of the four merges without
    /// anything noticing: the list an agent reads first then names a
    /// command that is not there, or leaves out one that is.
    #[test]
    fn the_commands_table_the_help_and_the_dispatch_name_the_same_commands() {
        let agents = std::fs::read_to_string(root().join("docs/development/commands.md")).unwrap();
        let (table, help, dispatch) = (
            documented_commands(&agents),
            help_commands(),
            dispatched_commands(),
        );
        let preludes: BTreeSet<String> = super::COMMANDS
            .iter()
            .map(|(name, _)| name.to_string())
            .collect();
        assert!(
            dispatch.len() > 10,
            "the dispatch was not read: {dispatch:?}"
        );
        let mut problems = Vec::new();
        for (name, list) in [
            ("the table of docs/development/commands.md", &table),
            ("HELP in xtask/src/main.rs", &help),
            ("the `match` in dispatch()", &dispatch),
            ("COMMANDS, which decides each prelude", &preludes),
        ] {
            let all: BTreeSet<String> = table
                .union(&help)
                .chain(&dispatch)
                .chain(&preludes)
                .cloned()
                .collect();
            let absent = missing(list, &all);
            if !absent.is_empty() {
                problems.push(format!("{name} lacks {}", absent.join(", ")));
            }
        }
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    #[test]
    fn a_table_row_names_its_command_and_its_siblings_but_not_subcommands() {
        let agents = "| Command | Purpose |\n| --- | --- |\n\
                      | `cargo xtask map [FEATURE]` | x |\n\
                      | `cargo xtask db-up` / `db-down` | x |\n\
                      | `cargo xtask worktree add <ISSUE>` / `list` / `remove <ISSUE>` | x |\n";

        assert_eq!(
            documented_commands(agents),
            ["db-down", "db-up", "map", "worktree"]
                .iter()
                .map(|name| name.to_string())
                .collect()
        );
    }

    /// What `cargo run` set for xtask must not reach the cargo it starts.
    #[test]
    fn the_cargo_xtask_starts_does_not_inherit_what_cargo_run_set_for_xtask() {
        // `cargo test` sets these for this test binary, as `cargo run` does
        // for xtask.
        assert!(std::env::var_os("CARGO_MANIFEST_DIR").is_some());
        assert!(std::env::var_os("CARGO_PKG_NAME").is_some());

        let command = cargo();
        let removed: Vec<String> = command
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(key, _)| key.to_string_lossy().into_owned())
            .collect();

        assert!(
            removed.contains(&"CARGO_MANIFEST_DIR".to_string()),
            "{removed:?}"
        );
        assert!(
            removed.contains(&"CARGO_PKG_NAME".to_string()),
            "{removed:?}"
        );
        assert!(!removed.contains(&"CARGO_HOME".to_string()), "{removed:?}");
        assert!(
            !removed.contains(&"CARGO_TARGET_DIR".to_string()),
            "{removed:?}"
        );
    }
}
