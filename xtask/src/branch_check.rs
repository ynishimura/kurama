//! `cargo xtask branch-check`: the branch half of the two-stage gate, and the
//! lock that turns the integration half into a queue.
//!
//! The heavy gate (`cargo xtask check`) compiles the workspace, runs every test
//! and sweeps `target/`. Charging it to every push means N parallel branches
//! pay for the same answer N times, on one CPU and one disk. What a branch can
//! answer alone is smaller: the tests of the features it changed, the scenarios
//! `impact` selects, and mutation testing of the `src/` and `xtask/src/`
//! files it edited. That
//! is `branch-check`, and `lefthook.yml` runs it for a push from any branch but
//! `dev` and `main`. The full gate still runs once, on the push that puts the
//! work on `dev`; `wait_for_gate_lock` is what makes two of those queue instead
//! of competing. `wait_for_branch_slot` does the same for `branch-check` with
//! N slots rather than one: four gates at once pushed a 12-core machine to a
//! load of 40-50 and PTY scenarios past their deadline.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::{Gate, TEST_FEATURES, compute_impact, default_base, git, mutate, root};

const USAGE: &str = "\
usage: cargo xtask branch-check [--base REF]

The gate a feature branch runs, in this order, stopping at the first failure:

  1. the static checks every checking command runs first (cargo fmt --check,
     cargo clippy -D warnings, cargo machete), cargo xtask architecture-audit,
     and fresh `cargo xtask verify-real` evidence for every changed feature
     with a probe
  2. cargo test --locked --workspace --features test-fakes -- <filters>
  3. cargo xtask verify affected, then cargo xtask verify-matrix over its reports
  4. cargo xtask mutate: the lines this branch changed under src/ and xtask/src/

The filters and the mutated files come from `cargo xtask impact --base REF`, so
what runs is what this branch changed. `cargo xtask check` -- every test and the
scenario coverage -- stays on the integration side: `lefthook.yml` runs it for
a push from `dev` or `main`, and this for a push from anywhere else.

At most KURAMA_XTASK_GATE_SLOTS (default 2) branch-checks of this clone run at
once, the static checks included; another one waits for a slot and says which
processes and worktrees hold them.
";

/// How long a waiting `check` sleeps between two attempts at the lock.
const POLL: Duration = Duration::from_secs(2);

/// The lock file, in the directory every worktree of the clone shares.
const GATE_LOCK: &str = "kurama-xtask-check.lock";

/// The prefix of the `branch-check` slot files, next to `GATE_LOCK`.
const SLOT_LOCK: &str = "kurama-xtask-branch-check";

/// How many `branch-check` runs go at once unless `SLOTS_VARIABLE` says.
const DEFAULT_SLOTS: usize = 2;

const SLOTS_VARIABLE: &str = "KURAMA_XTASK_GATE_SLOTS";

#[derive(Debug, PartialEq)]
enum Request {
    Help,
    Run { base: Option<String> },
}

pub fn branch_check(args: &[String]) -> Result<(), String> {
    let base = match parse(args)? {
        Request::Help => {
            print!("{USAGE}");
            return Ok(());
        }
        Request::Run { base } => base.unwrap_or_else(default_base),
    };
    refuse_a_foreign_target_dir()?;
    let impact = compute_impact(&base)?;
    let mut selected = impact.features.clone();
    selected.extend(impact.dependent_features.clone());
    let scope = if selected.is_empty() {
        "no feature".to_string()
    } else {
        selected.join(", ")
    };
    eprintln!("==> branch-check against {base}: {scope}");

    // 1. Format, clippy and unused dependencies ran before this function
    //    (`main.rs`'s prelude); they are listed first in the summary.
    let mut gates: Vec<Gate> = crate::static_checks::results();

    // The architecture registry against the tests: reading files, no build.
    let registry = crate::architecture_gate();
    let checked = registry.ok;
    gates.push(registry);

    // Real-environment evidence for the features this branch changed. It
    // reads files only, so it runs before anything is built.
    let real = crate::load_features()
        .and_then(|features| crate::verify_real::gate(&features, &impact.features));
    if let Err(problems) = &real {
        eprintln!("{problems}");
    }
    let checked = checked && real.is_ok();
    gates.push(Gate {
        name: "real-verification".into(),
        ok: real.is_ok(),
        detail: real
            .unwrap_or_else(|problems| problems.lines().next().unwrap_or_default().to_string()),
    });

    // 2. The unit and integration tests of every affected feature. Their
    //    time and the scenarios' is what bounds a mutant's in step 4.
    let tests_started = std::time::Instant::now();
    if checked {
        if impact.test_filters.is_empty() {
            gates.push(Gate::not_run(
                "tests",
                "no feature owns a changed file, so no filter selects a test",
            ));
        } else {
            let args = test_args(&impact.test_filters);
            eprintln!("==> tests: cargo {}", args.join(" "));
            let ok = run_cargo(&args)?;
            gates.push(Gate {
                name: "tests".into(),
                ok,
                detail: impact.test_filters.join(" "),
            });
        }
    }

    // 3. The runtime scenarios `impact` selects.
    if gates.iter().all(|gate| gate.ok) {
        let result = crate::verify_against(Some("affected"), &base);
        gates.push(Gate {
            name: "scenarios".into(),
            ok: result.is_ok(),
            detail: format!("affected ({scope})"),
        });
    }

    //    The matrix over what those scenarios wrote: an enumerated value no
    //    case or declaration classifies fails here whether or not a scenario
    //    ran; a case with no report is counted as UNEXECUTED, not failed.
    if gates.iter().all(|gate| gate.ok) {
        eprintln!("==> matrix: cargo xtask verify-matrix");
        gates.push(crate::matrix::gate());
    }

    // 4. Mutation testing of the production files this branch changed. A test
    //    that fails says more than a mutant that survives, so this runs last
    //    and only while everything before it is green.
    if gates.iter().all(|gate| gate.ok) {
        let paths = mutate_paths(&impact.changed_files, |file| root().join(file).exists());
        if paths.is_empty() {
            gates.push(Gate::not_run(
                "mutate",
                "this branch changed no file under src/ or xtask/src/",
            ));
        } else if !crate::cargo_has("mutants") {
            gates.push(Gate::not_run(
                "mutate",
                "cargo-mutants is not installed (cargo install --locked cargo-mutants)",
            ));
        } else {
            let tests: Vec<String> = impact
                .test_filters
                .iter()
                .chain(&impact.scenarios)
                .cloned()
                .collect();
            let result = mutate::mutate_branch(&base, &tests, tests_started.elapsed());
            gates.push(Gate {
                name: "mutate".into(),
                ok: result.is_ok(),
                detail: paths.join(" "),
            });
        }
    }

    eprint!("{}", render_summary(&gates));
    if gates.iter().all(|gate| gate.ok) {
        Ok(())
    } else {
        Err("branch-check failed; see the output above".into())
    }
}

/// Refuses a `CARGO_TARGET_DIR` outside this worktree. Cargo names a
/// workspace crate's artifacts without its path and judges them fresh by
/// modification time, so two worktrees sharing one target directory take each
/// other's builds: measured on 2026-09-23, a worktree listed and ran a test
/// that only the other worktree's source had. A gate that passes on another
/// branch's code has verified nothing.
pub(crate) fn refuse_a_foreign_target_dir() -> Result<(), String> {
    match foreign_target_dir(std::env::var_os("CARGO_TARGET_DIR").as_deref(), &root()) {
        Some(problem) => Err(problem),
        None => Ok(()),
    }
}

fn foreign_target_dir(value: Option<&std::ffi::OsStr>, root: &Path) -> Option<String> {
    let dir = Path::new(value?);
    if dir.as_os_str().is_empty() || dir.is_relative() || dir.starts_with(root) {
        return None;
    }
    Some(format!(
        "CARGO_TARGET_DIR={} is outside this worktree ({}); worktrees that share a target \
         directory run each other's builds, so a gate there checks another branch's code. \
         Unset it for the gate",
        dir.display(),
        root.display()
    ))
}

fn parse(args: &[String]) -> Result<Request, String> {
    let mut base = None;
    let mut rest = args.iter().map(String::as_str);
    while let Some(arg) = rest.next() {
        match arg {
            "--base" => {
                base = Some(
                    rest.next()
                        .ok_or_else(|| format!("--base needs a value\n\n{USAGE}"))?
                        .to_string(),
                )
            }
            "--help" | "-h" => return Ok(Request::Help),
            other => return Err(format!("unknown argument `{other}`\n\n{USAGE}")),
        }
    }
    Ok(Request::Run { base })
}

/// `--workspace`: some of the filters a feature declares name tests of the
/// xtask package (`conflicts::`, `impact_`), which the default package
/// selection leaves out.
fn test_args(filters: &[String]) -> Vec<String> {
    let mut args: Vec<String> = ["test", "--locked", "--workspace"]
        .iter()
        .map(|arg| arg.to_string())
        .collect();
    args.extend(TEST_FEATURES.iter().map(|arg| arg.to_string()));
    args.push("--".to_string());
    args.extend(filters.iter().cloned());
    args
}

/// The changed files cargo-mutants can mutate: the sources of kurama and of
/// xtask that still exist. A deleted file carries no code, and tests and
/// manifests are not mutated.
fn mutate_paths(changed_files: &[String], exists: impl Fn(&str) -> bool) -> Vec<String> {
    changed_files
        .iter()
        .filter(|file| {
            (file.starts_with("src/") || file.starts_with("xtask/src/"))
                && file.ends_with(".rs")
                && exists(file)
        })
        .cloned()
        .collect()
}

fn render_summary(gates: &[Gate]) -> String {
    let mut out = String::from("==> branch-check summary\n");
    for gate in gates {
        let detail = gate
            .detail
            .strip_prefix("NOT_RUN: ")
            .unwrap_or(&gate.detail);
        out.push_str(&format!(
            "    {:<10} {:<8} {detail}\n",
            gate.name,
            gate.result()
        ));
    }
    out
}

fn run_cargo(args: &[String]) -> Result<bool, String> {
    crate::cargo()
        .args(args)
        .current_dir(root())
        .status()
        .map(|status| status.success())
        .map_err(|error| format!("cargo: {error}"))
}

// ---------------------------------------------------------------------------
// the integration-side queue
// ---------------------------------------------------------------------------

/// Held for as long as one `cargo xtask check` runs. Dropping it lets the next
/// one start.
pub struct GateLock {
    path: PathBuf,
}

impl Drop for GateLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Takes the gate lock, waiting for whoever holds it. Two `check` runs at once
/// take longer than the same two in sequence: both compile the workspace, and
/// DuckDB is built from source.
pub fn wait_for_gate_lock() -> Result<GateLock, String> {
    let path = gate_lock_path()?;
    let mut announced = false;
    loop {
        if let Some(lock) = take_lock(&path, std::process::id())? {
            if announced {
                eprintln!("==> gate: the other check finished");
            }
            return Ok(lock);
        }
        if !announced {
            eprintln!(
                "==> gate: another `cargo xtask check` is running; waiting for {}",
                path.display()
            );
            announced = true;
        }
        std::thread::sleep(POLL);
    }
}

/// The git *common* directory: the one every linked worktree of the clone
/// shares. `target/` is per worktree, and a lock there would let two worktrees
/// compile at the same time, which is what the queue exists to stop.
fn gate_lock_path() -> Result<PathBuf, String> {
    Ok(common_dir()?.join(GATE_LOCK))
}

fn common_dir() -> Result<PathBuf, String> {
    let common = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    Ok(PathBuf::from(common.trim()))
}

/// Takes one of the `branch-check` slots, waiting while every one is held.
/// The same lock files as `check`'s, N of them: the gate's static checks,
/// tests, scenarios and mutants all run inside the slot.
pub fn wait_for_branch_slot() -> Result<GateLock, String> {
    let slots = slot_count(std::env::var(SLOTS_VARIABLE).ok().as_deref())?;
    let paths = slot_paths(&common_dir()?, slots);
    let mut announced = false;
    loop {
        if let Some(lock) = take_slot(&paths, std::process::id())? {
            if announced {
                eprintln!("==> gate: a branch-check slot is free");
            }
            return Ok(lock);
        }
        if !announced {
            eprintln!("{}", waiting_line(&paths));
            announced = true;
        }
        std::thread::sleep(POLL);
    }
}

fn slot_count(value: Option<&str>) -> Result<usize, String> {
    match value {
        None | Some("") => Ok(DEFAULT_SLOTS),
        Some(value) => value
            .parse()
            .ok()
            .filter(|slots| *slots > 0)
            .ok_or_else(|| format!("{SLOTS_VARIABLE}={value} is not a number of slots above 0")),
    }
}

fn slot_paths(common: &Path, slots: usize) -> Vec<PathBuf> {
    (1..=slots)
        .map(|slot| common.join(format!("{SLOT_LOCK}.{slot}.lock")))
        .collect()
}

fn take_slot(paths: &[PathBuf], pid: u32) -> Result<Option<GateLock>, String> {
    for path in paths {
        if let Some(lock) = take_lock(path, pid)? {
            return Ok(Some(lock));
        }
    }
    Ok(None)
}

/// Who holds the slots, read from what each lock file says: the process and
/// the worktree it gates.
fn waiting_line(paths: &[PathBuf]) -> String {
    let holders: Vec<String> = paths
        .iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .map(|text| match text.trim().split_once(' ') {
            Some((pid, worktree)) => format!("pid {pid} in {worktree}"),
            None => format!("pid {}", text.trim()),
        })
        .collect();
    format!(
        "==> gate: all {} branch-check slots are taken ({}); waiting for one \
         ({SLOTS_VARIABLE} sets how many)",
        paths.len(),
        holders.join("; ")
    )
}

/// `Ok(None)` means a running process holds the lock. Creating the file is the
/// atomic step, so two callers that race it cannot both win.
fn take_lock(path: &Path, pid: u32) -> Result<Option<GateLock>, String> {
    // Two rounds: one to create the lock, one more after a lock left behind by
    // a process that no longer exists was cleared.
    for _ in 0..2 {
        match std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
        {
            Ok(mut file) => {
                write!(file, "{pid} {}", root().display())
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                return Ok(Some(GateLock {
                    path: path.to_path_buf(),
                }));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                match holder(path) {
                    Some(holder) if is_running(holder) => return Ok(None),
                    // A gate stopped with Ctrl-C never ran its `Drop`, and a
                    // lock nobody holds is not a queue but a deadlock.
                    _ => std::fs::remove_file(path)
                        .map_err(|error| format!("{}: {error}", path.display()))?,
                }
            }
            Err(error) => return Err(format!("{}: {error}", path.display())),
        }
    }
    Ok(None)
}

fn holder(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path)
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// `kill -0`: the signal that only asks whether the process is there. Its
/// output is captured, because "no such process" is this function's answer and
/// not something the gate has to say out loud.
fn is_running(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args;

    fn temp_path(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("kurama-branch-check-{}-{name}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn branch_check_takes_only_an_optional_base() {
        assert_eq!(parse(&[]).unwrap(), Request::Run { base: None });
        assert_eq!(
            parse(&args(&["--base", "main"])).unwrap(),
            Request::Run {
                base: Some("main".into())
            }
        );
        assert_eq!(parse(&args(&["--help"])).unwrap(), Request::Help);
        assert!(parse(&args(&["--base"])).is_err());
        assert!(parse(&args(&["affected"])).is_err());
    }

    /// The filters a feature declares name tests of the xtask package too, and
    /// the default package selection never runs those.
    #[test]
    fn the_test_step_runs_the_whole_workspace_with_the_impact_filters() {
        let args = test_args(&["conflicts::".to_string(), "impact_".to_string()]);

        assert_eq!(
            args,
            [
                "test",
                "--locked",
                "--workspace",
                "--features",
                "test-fakes",
                "--",
                "conflicts::",
                "impact_",
            ]
        );
    }

    #[test]
    fn only_changed_production_sources_are_mutated() {
        let changed = args(&[
            "src/domain/functions/export.rs",
            "src/shell/removed.rs",
            "tests/scenarios/api.rs",
            "Cargo.lock",
            "xtask/src/branch_check.rs",
            "src/notes.md",
        ]);

        let paths = mutate_paths(&changed, |file| file != "src/shell/removed.rs");

        assert_eq!(
            paths,
            [
                "src/domain/functions/export.rs",
                "xtask/src/branch_check.rs"
            ]
        );
    }

    #[test]
    fn a_skipped_step_is_not_reported_as_a_pass() {
        let gates = vec![
            Gate {
                name: "tests".into(),
                ok: true,
                detail: "conflicts::".into(),
            },
            Gate::not_run("mutate", "this branch changed no file under src/"),
        ];

        let summary = render_summary(&gates);

        assert!(summary.contains("tests      pass"), "{summary}");
        assert!(summary.contains("mutate     NOT_RUN"), "{summary}");
        assert!(
            summary.contains("this branch changed no file under src/"),
            "{summary}"
        );
        assert!(!summary.contains("NOT_RUN: this"), "{summary}");
    }

    /// Two `check` runs must not compile at the same time. The second caller
    /// here is a second thread, which reaches the lock file exactly the way a
    /// second process does: nothing about it lives in this process.
    #[test]
    fn a_second_check_waits_until_the_first_one_releases_the_lock() {
        let path = temp_path("queue");

        let first = take_lock(&path, std::process::id())
            .unwrap()
            .expect("the first run takes the lock");
        let while_held = std::thread::scope(|scope| {
            scope
                .spawn(|| take_lock(&path, std::process::id()).unwrap().is_some())
                .join()
                .unwrap()
        });
        drop(first);
        let after_release = take_lock(&path, std::process::id()).unwrap();

        assert!(
            !while_held,
            "a second check started while the first held it"
        );
        assert!(
            after_release.is_some(),
            "the lock outlived the run that took it"
        );
    }

    /// A gate stopped with Ctrl-C never runs `Drop`; a lock nobody holds would
    /// otherwise queue every later run behind a process that is gone.
    #[test]
    fn a_lock_left_by_a_dead_process_is_taken_over() {
        let path = temp_path("stale");
        // Above the pid range macOS hands out, so no process can hold it.
        std::fs::write(&path, "999999").unwrap();

        let taken = take_lock(&path, std::process::id()).unwrap();

        assert!(taken.is_some(), "a lock with no live holder is not a queue");
        assert_eq!(holder(&path), Some(std::process::id()));
    }

    /// Two slots held, a third `branch-check` waits, and what it prints names
    /// the process and the worktree behind each slot.
    #[test]
    fn a_third_branch_check_waits_and_names_who_holds_the_slots() {
        let dir = std::env::temp_dir().join(format!("kurama-slots-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let paths = slot_paths(&dir, 2);
        for path in &paths {
            let _ = std::fs::remove_file(path);
        }

        let first = take_slot(&paths, std::process::id()).unwrap();
        let second = take_slot(&paths, std::process::id()).unwrap();
        let third = take_slot(&paths, std::process::id()).unwrap();
        let line = waiting_line(&paths);

        assert!(first.is_some() && second.is_some());
        assert!(third.is_none(), "a third run took a slot");
        let pid = std::process::id().to_string();
        let expected = format!("pid {pid} in {}", root().display());
        assert_eq!(line.matches(&expected).count(), 2, "{line}");
        assert!(line.contains("all 2 branch-check slots"), "{line}");
        drop(first);
        assert!(take_slot(&paths, std::process::id()).unwrap().is_some());
        drop(second);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn two_slots_unless_the_variable_says_otherwise() {
        assert_eq!(slot_count(None).unwrap(), 2);
        assert_eq!(slot_count(Some("")).unwrap(), 2);
        assert_eq!(slot_count(Some("3")).unwrap(), 3);
        assert!(slot_count(Some("0")).is_err());
        assert!(slot_count(Some("many")).is_err());
        assert_eq!(
            slot_paths(Path::new("/g"), 2),
            [
                PathBuf::from("/g/kurama-xtask-branch-check.1.lock"),
                PathBuf::from("/g/kurama-xtask-branch-check.2.lock"),
            ]
        );
    }

    /// `target/` is per worktree, so a lock there would serialize nothing.
    #[test]
    fn the_gate_lock_is_shared_by_every_worktree_of_the_clone() {
        let path = gate_lock_path().unwrap();
        let common = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"]).unwrap();

        assert_eq!(path.parent().unwrap(), Path::new(common.trim()));
        assert!(!path.starts_with(root().join("target")), "{path:?}");
    }

    // -----------------------------------------------------------------------
    // the pre-push hook
    // -----------------------------------------------------------------------

    /// The one-line shell script `lefthook.yml` runs before a push.
    fn pre_push_gate_script() -> String {
        let leftover = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../lefthook.yml"));
        let line = leftover
            .lines()
            .find(|line| line.contains("run:") && line.contains("xtask branch-check"))
            .expect("lefthook.yml runs branch-check before a push");
        let (_, script) = line.split_once("run:").unwrap();
        script.trim().trim_matches('\'').to_string()
    }

    /// Runs that script with `git` and `mise` replaced by stubs, so the branch
    /// it reads is the one under test and the gate it picks is only printed.
    fn gate_for_branch(branch: &str) -> String {
        let dir = std::env::temp_dir().join(format!(
            "kurama-pre-push-{}-{}",
            std::process::id(),
            branch.replace('/', "-")
        ));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, body) in [
            (
                "git",
                "#!/bin/sh\n[ \"$1 $2 $3\" = \"rev-parse --abbrev-ref HEAD\" ] && echo \"$STUB_BRANCH\"\n",
            ),
            ("mise", "#!/bin/sh\necho \"$@\"\n"),
        ] {
            let path = dir.join(name);
            std::fs::write(&path, body).unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let output = Command::new("sh")
            .arg("-c")
            .arg(pre_push_gate_script())
            .env("PATH", format!("{}:/usr/bin:/bin", dir.display()))
            .env("STUB_BRANCH", branch)
            .output()
            .unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    #[test]
    fn a_push_from_an_integration_branch_runs_the_full_gate() {
        for branch in ["dev", "main"] {
            let gate = gate_for_branch(branch);
            assert!(gate.contains("cargo xtask check"), "{branch}: {gate}");
            assert!(!gate.contains("branch-check"), "{branch}: {gate}");
        }
    }

    #[test]
    fn a_push_from_a_feature_branch_runs_the_branch_gate() {
        let gate = gate_for_branch("feat/53-two-stage-gate");

        assert!(gate.contains("cargo xtask branch-check"), "{gate}");
    }

    #[test]
    fn a_target_dir_outside_the_worktree_stops_the_gate() {
        let root = Path::new("/w/kurama-87");
        let outside = std::ffi::OsStr::new("/w/shared-target");

        let problem = foreign_target_dir(Some(outside), root).expect("refused");

        assert!(problem.contains("/w/shared-target"), "{problem}");
        assert!(problem.contains("another branch's code"), "{problem}");
        for fine in ["/w/kurama-87/target", "target", ""] {
            assert_eq!(
                foreign_target_dir(Some(std::ffi::OsStr::new(fine)), root),
                None,
                "{fine}"
            );
        }
        assert_eq!(foreign_target_dir(None, root), None);
    }
}
