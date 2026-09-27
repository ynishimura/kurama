//! `cargo xtask mutate`: mutation testing of the branch diff with
//! `cargo-mutants`.
//!
//! A test can pass for the wrong reason: it exercises the standard library
//! instead of the feature, matches a loose substring, or reads a fake that
//! answers the same constant to every input. Two review rounds on this
//! repository found eight of those, and one of them twice. A mutant that
//! survives every test names exactly that: production code changed, nothing
//! failed. The scope is the diff, because mutating the whole tree runs the
//! suite once per mutant; each kurama mutant runs the tests of the features
//! that own its file (`grouped_runs`).
//!
//! Both packages are mutated: kurama (`src/`) and xtask itself
//! (`xtask/src/`), whose `ready`, `conflicts` and `impact` are what parallel
//! agents act on. Each package is its own cargo-mutants run, because kurama's
//! tests need `--features test-fakes` and xtask has no such feature.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::impact::mutant_tests;
use crate::{default_base, git, load_features, root};

const USAGE: &str = "\
usage: cargo xtask mutate [PATH ...] [--base REF] [--timeout SECONDS]

  PATH                mutate this file or directory instead of the diff
  --base REF          diff against REF instead of the branch base
  --timeout SECONDS   per-mutant test timeout (default: twice the unmutated tests,
                      at least 60 s)

With no PATH the mutated code is the lines this branch and the working tree
changed under src/ and xtask/src/, and each kurama mutant runs the tests of the
features that own its file. A PATH mutates every line of its files: the run
says how many mutants that is next to the diff's count before it starts. A PATH
under xtask/ mutates xtask and runs xtask's tests. Every surviving mutant
is a change to production code that no test noticed, so it is printed and the
command exits 1. The run is written to target/agent/mutation-report.md, the
per-mutant logs to target/agent/mutants.out/.

Each mutant rebuilds the crate, and cargo keeps every generation in target/,
where the bundled DuckDB makes one test binary hundreds of megabytes. A few
hundred mutants fill the disk: scope the run with PATH, and `mise run sweep`
first.

cargo-mutants replaces function bodies and negates conditions. It does not
write these, so a green run does not mean a test would have noticed them, and
a regression of one of these shapes is put in by hand and watched to fail:
  - the value of a `const` (write a value that decides behaviour as a function)
  - an argument swapped or replaced at a call site
  - one arm of a `match` whose other arms still produce plausible output

Needs cargo-mutants: cargo install --locked cargo-mutants
";

/// A mutant's tests may take this many times the unmutated tests before it
/// counts as hanging. A hanging mutant is already detected by timing out; the
/// old fixed 300 s made each of them cost five minutes (18 of 93 on one branch).
const TIMEOUT_MULTIPLIER: u64 = 2;

/// The floor of that timeout, so a fast selection does not time out a mutant
/// on a slow build or a loaded machine.
const MINIMUM_TIMEOUT_SECS: u64 = 60;

/// What rebuilding kurama in place costs a mutant, on top of its tests: 14 to
/// 19 s measured.
const BUILD_SECS: u64 = 20;

/// How a run bounds each mutant's tests, and whether it first runs them on
/// the unmutated tree.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Timing {
    /// `branch-check`: its tests and scenarios just passed on this tree in
    /// `tests_secs`, so the baseline would repeat them -- and give a flaky
    /// scenario one more chance to stop the whole step.
    Measured { tests_secs: u64 },
    /// A run of its own: cargo-mutants runs the tests unmutated first and,
    /// without `--timeout`, bounds each mutant by a multiple of that.
    Baseline { timeout: Option<String> },
}

impl Timing {
    /// The cargo-mutants arguments that say so.
    fn arguments(&self) -> Vec<String> {
        match self {
            Timing::Measured { tests_secs } => vec![
                "--baseline".to_string(),
                "skip".to_string(),
                "--timeout".to_string(),
                (tests_secs * TIMEOUT_MULTIPLIER)
                    .max(MINIMUM_TIMEOUT_SECS)
                    .to_string(),
            ],
            Timing::Baseline {
                timeout: Some(timeout),
            } => vec!["--timeout".to_string(), timeout.clone()],
            Timing::Baseline { timeout: None } => vec![
                "--timeout-multiplier".to_string(),
                TIMEOUT_MULTIPLIER.to_string(),
                "--minimum-test-timeout".to_string(),
                MINIMUM_TIMEOUT_SECS.to_string(),
            ],
        }
    }

    /// One line before the run: how many mutants and how long, so a run that
    /// is larger than meant is seen when it starts, not when it is asked about.
    fn estimate(&self, mutants: usize) -> String {
        match self {
            Timing::Measured { tests_secs } => {
                let each = tests_secs + BUILD_SECS;
                format!(
                    "==> {mutants} mutant(s), at most about {} min: each rebuilds (about {BUILD_SECS} s) \
                     and runs at most the {tests_secs} s of tests the gate just ran",
                    (mutants as u64 * each).div_ceil(60)
                )
            }
            Timing::Baseline { .. } => format!(
                "==> {mutants} mutant(s): each costs about the unmutated run cargo-mutants \
                 times first, plus a rebuild (about {BUILD_SECS} s)"
            ),
        }
    }
}

const REPORT: &str = "mutation-report.md";

#[derive(Debug, PartialEq)]
enum Request {
    Help,
    Run(Plan),
}

#[derive(Debug, PartialEq)]
struct Plan {
    /// Files or directories to mutate; the branch diff when empty.
    paths: Vec<String>,
    base: Option<String>,
    timeout: Option<String>,
}

/// A package cargo-mutants runs on, with what that run needs.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Package {
    /// `src/`: its tests compile only with `--features test-fakes`.
    Kurama,
    /// `xtask/src/`: no features, and its own tests.
    Xtask,
}

const PACKAGES: [Package; 2] = [Package::Kurama, Package::Xtask];

impl Package {
    /// The git pathspec of the sources a diff is cut to.
    fn sources(self) -> &'static str {
        match self {
            Package::Kurama => "src/*.rs",
            Package::Xtask => "xtask/src/*.rs",
        }
    }

    fn of_path(path: &str) -> Package {
        if path.trim_start_matches("./").starts_with("xtask/") {
            Package::Xtask
        } else {
            Package::Kurama
        }
    }

    fn arguments(self) -> Vec<String> {
        let arguments: &[&str] = match self {
            Package::Kurama => &crate::TEST_FEATURES,
            // A copy without `.git` fails every xtask test that asks git
            // where the clone is, before a single mutant runs.
            Package::Xtask => &["--package", "xtask", "--copy-vcs", "true"],
        };
        arguments.iter().map(|arg| arg.to_string()).collect()
    }

    /// Where the run writes `mutants.out/`: the kurama run where it always
    /// did, the xtask run beside it.
    fn output(self, out_dir: &Path) -> PathBuf {
        match self {
            Package::Kurama => out_dir.to_path_buf(),
            Package::Xtask => out_dir.join("xtask"),
        }
    }
}

/// The four lists cargo-mutants writes, one mutant per line.
#[derive(Debug, Default)]
struct Outcomes {
    survived: Vec<String>,
    caught: Vec<String>,
    unviable: Vec<String>,
    timed_out: Vec<String>,
}

pub fn mutate(args: &[String]) -> Result<(), String> {
    let plan = match parse(args)? {
        Request::Help => {
            print!("{USAGE}");
            return Ok(());
        }
        Request::Run(plan) => plan,
    };
    require_mutants()?;
    let out_dir = out_dir()?;
    let base = plan.base.clone().unwrap_or_else(default_base);
    let timing = Timing::Baseline {
        timeout: plan.timeout.clone(),
    };
    let diff_runs = branch_runs(&base, &out_dir, &[])?;
    if plan.paths.is_empty() {
        return execute(diff_runs, &timing, &out_dir, &base);
    }
    let runs: Vec<Run> = PACKAGES
        .iter()
        .filter_map(|package| path_run(*package, &plan.paths))
        .collect();
    eprintln!(
        "{}",
        compare_with_diff(count(&runs)?, count(&diff_runs)?, &base)
    );
    execute(runs, &timing, &out_dir, &base)
}

/// A PATH mutates whole files, which is easy to reach for when only the
/// lines a branch changed were meant: the two counts side by side say which
/// one is running.
fn compare_with_diff(paths: usize, diff: usize, base: &str) -> String {
    format!(
        "==> PATH: {paths} mutant(s) over whole files; the lines this branch changed against \
         {base}: {diff}. To mutate only those, run `cargo xtask mutate` without PATH"
    )
}

/// What `branch-check` mutates: the lines the branch changed, in kurama and in
/// xtask. A branch answers for what it wrote. Mutating whole files made a
/// one-line change to `src/adapters/config/mod.rs` a run of 162 mutants and
/// about two hours, 9 of which survived in code the branch never touched.
///
/// Each kurama mutant runs the tests `impact` selects for the branch -- its
/// test filters and its scenarios -- rather than the whole suite. The suite is
/// CPU-bound at about 55 s, 49 of them scenarios, so more threads do not help;
/// fewer tests do. The selection also holds the scenarios of the features of
/// the files a file imports, which are often what verifies it; their unit
/// tests never run the mutated file. An empty selection runs everything.
pub(crate) fn mutate_branch(
    base: &str,
    tests: &[String],
    tests_took: Duration,
) -> Result<(), String> {
    require_mutants()?;
    let out_dir = out_dir()?;
    let runs = branch_runs(base, &out_dir, tests)?;
    let timing = Timing::Measured {
        tests_secs: tests_took.as_secs().max(1),
    };
    execute(runs, &timing, &out_dir, base)
}

/// The runs over the lines the branch changed: one per group of kurama files
/// that select the same features -- their owners and the owners of what they
/// import -- each with the owners' tests and the imported features' scenarios,
/// then xtask.
/// A file no feature owns runs `unowned` (empty: every test).
fn branch_runs(base: &str, out_dir: &Path, unowned: &[String]) -> Result<Vec<Run>, String> {
    let mut runs = Vec::new();
    if let Some(run) = diff_run(Package::Kurama, base, out_dir)? {
        let diff = std::fs::read_to_string(&run.filter[1]).map_err(|e| e.to_string())?;
        let features = load_features()?;
        let imports = imports_by_file();
        let none = BTreeSet::new();
        runs.extend(grouped_runs(run, &files_of_diff(&diff), unowned, |file| {
            mutant_tests(&features, file, imports.get(file).unwrap_or(&none))
        }));
    }
    runs.extend(diff_run(Package::Xtask, base, out_dir)?);
    Ok(runs)
}

/// Every `src/` file with the `src/` files it imports: the import graph of
/// `impact`, read forwards.
fn imports_by_file() -> BTreeMap<String, BTreeSet<String>> {
    let mut imports: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (target, importers) in crate::deps::importers() {
        for importer in importers {
            imports.entry(importer).or_default().insert(target.clone());
        }
    }
    imports
}

/// The files a unified diff changes, from its `+++ b/<path>` lines.
fn files_of_diff(diff: &str) -> Vec<String> {
    diff.lines()
        .filter_map(|line| line.strip_prefix("+++ b/"))
        .map(str::to_string)
        .collect()
}

/// `run` split by the features that own each file: a mutant runs the tests
/// of the feature its code belongs to, not every test the branch selected.
/// A file only a test elsewhere would catch then survives, and says the
/// owning feature's tests miss it. The selection a branch makes for a hub
/// such as `domain/types/http.rs` is most of the suite, 116 s a mutant.
fn grouped_runs(
    run: Run,
    files: &[String],
    unowned: &[String],
    owners_and_tests: impl Fn(&str) -> (Vec<String>, Vec<String>),
) -> Vec<Run> {
    let mut groups: BTreeMap<Vec<String>, (Vec<String>, Vec<String>)> = BTreeMap::new();
    for file in files {
        let (owners, tests) = owners_and_tests(file);
        let tests = if owners.is_empty() {
            unowned.to_vec()
        } else {
            tests
        };
        let group = groups.entry(owners).or_insert_with(|| (Vec::new(), tests));
        group.0.push(file.clone());
    }
    groups
        .into_iter()
        .map(|(owners, (files, tests))| Run {
            package: run.package,
            scope: format!(
                "{} ({})",
                files.join(", "),
                if owners.is_empty() {
                    "no feature owns it".to_string()
                } else {
                    format!("tests of {}", owners.join(", "))
                }
            ),
            filter: run
                .filter
                .iter()
                .cloned()
                .chain(
                    files
                        .iter()
                        .flat_map(|file| ["--file".to_string(), file.clone()]),
                )
                .collect(),
            tests,
        })
        .collect()
}

/// How many mutants `runs` hold, from `cargo mutants --list`, which reads
/// the sources and builds nothing.
fn count(runs: &[Run]) -> Result<usize, String> {
    runs.iter()
        .map(|run| {
            crate::cargo()
                .args(["mutants", "--list"])
                .args(run.package.arguments())
                .args(&run.filter)
                .current_dir(root())
                .output()
                .map(|output| listed(&String::from_utf8_lossy(&output.stdout)))
                .map_err(|e| format!("cargo mutants --list: {e}"))
        })
        .sum()
}

/// The mutants `cargo mutants --list` printed: one a line.
fn listed(stdout: &str) -> usize {
    stdout.lines().filter(|line| !line.is_empty()).count()
}

/// One cargo-mutants run: a package, what the report calls its scope, and the
/// arguments that select the mutated code.
struct Run {
    package: Package,
    scope: String,
    filter: Vec<String>,
    /// `cargo test` filters each mutant runs; empty runs every test.
    tests: Vec<String>,
}

fn out_dir() -> Result<PathBuf, String> {
    let out_dir = crate::agent_dir();
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    Ok(out_dir)
}

/// The package's lines the branch changed, or nothing when it changed none.
fn diff_run(package: Package, base: &str, out_dir: &Path) -> Result<Option<Run>, String> {
    Ok(run_of_diff(
        package,
        base,
        &write_branch_diff(base, package, out_dir)?,
    ))
}

/// A run over the diff in `diff`, unless it is empty: cargo-mutants given an
/// empty diff has nothing to do, and a package the branch did not touch is
/// not a run.
fn run_of_diff(package: Package, base: &str, diff: &Path) -> Option<Run> {
    let empty = std::fs::metadata(diff).map_or(true, |meta| meta.len() == 0);
    (!empty).then(|| Run {
        package,
        scope: format!("{} diff against {base}", package.sources()),
        filter: vec!["--in-diff".to_string(), diff.display().to_string()],
        tests: Vec::new(),
    })
}

/// The package's files among `paths`, or nothing when none is its own.
fn path_run(package: Package, paths: &[String]) -> Option<Run> {
    let mine: Vec<&String> = paths
        .iter()
        .filter(|path| Package::of_path(path) == package)
        .collect();
    (!mine.is_empty()).then(|| Run {
        package,
        scope: mine
            .iter()
            .map(|path| path.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        filter: mine
            .iter()
            .flat_map(|path| ["--file".to_string(), file_glob(path)])
            .collect(),
        tests: Vec::new(),
    })
}

fn execute(runs: Vec<Run>, timing: &Timing, out_dir: &Path, base: &str) -> Result<(), String> {
    if runs.is_empty() {
        eprintln!("==> nothing to mutate: no change under src/ or xtask/src/ against {base}");
        return Ok(());
    }
    eprintln!("{}", timing.estimate(count(&runs)?));
    let mut outcomes = Outcomes::default();
    let mut scopes = Vec::new();
    let mut failed_runs = Vec::new();
    for run in runs {
        let output = run.package.output(out_dir);
        let mut command = vec!["mutants".to_string()];
        command.extend(run.package.arguments());
        command.extend(timing.arguments());
        command.extend(["--output".to_string(), output.display().to_string()]);
        let clean_before = src_status()?;
        let in_place = in_place_arguments(run.package, &clean_before);
        command.extend(in_place.iter().cloned());
        command.extend(run.filter);
        command.extend(test_arguments(&run.tests));
        eprintln!("==> mutate {}: cargo {}", run.scope, command.join(" "));
        outcomes.extend(run_outcomes(&output, || {
            let status = crate::cargo()
                .args(&command)
                .current_dir(root())
                .status()
                .map_err(|e| format!("cargo mutants: {e}"))?;
            if !status.success() {
                failed_runs.push(format!("{} ({status})", run.scope));
            }
            if !in_place.is_empty() {
                let after = src_status()?;
                if !after.trim().is_empty() {
                    return Err(format!(
                        "cargo mutants left src/ changed after mutating it in place:\n{after}\
                         src/ was clean before the run, so `git restore src/` puts it back"
                    ));
                }
            }
            Ok(())
        })?);
        scopes.push(run.scope);
    }

    let report = render_report(&scopes.join("; "), &outcomes);
    let report_path = out_dir.join(REPORT);
    std::fs::write(&report_path, &report).map_err(|e| e.to_string())?;
    println!("{report}");
    if !outcomes.survived.is_empty() {
        Err(format!(
            "{} mutant(s) survived; each one is production code no test notices",
            outcomes.survived.len()
        ))
    } else if !failed_runs.is_empty() {
        Err(format!(
            "cargo mutants failed: {}; see {}",
            failed_runs.join(", "),
            report_path.display()
        ))
    } else {
        eprintln!("==> every mutant was caught");
        Ok(())
    }
}

/// `git status` of `src/`, which decides whether kurama can be mutated in place.
fn src_status() -> Result<String, String> {
    git(&["status", "--porcelain", "--", "src/"])
}

/// `--in-place` for kurama when nothing under `src/` is uncommitted. A copy of
/// the tree has no `target/`, so every run began by building DuckDB from
/// scratch: 258 s of a 6-mutant run whose mutants took 11 to 18 s to build.
/// Copying `target/` too does not help, because the copy takes new
/// modification times and cargo rebuilds 287 crates. In place, the baseline
/// and each mutant reuse the warm `target/`. The price is that an
/// interrupted run can leave a mutant in the file, so it is only done over a
/// clean `src/`, where `git restore src/` undoes it, and `execute` checks the
/// tree afterwards. xtask stays in a copy: it builds in seconds.
fn in_place_arguments(package: Package, src_status: &str) -> Vec<String> {
    if package == Package::Kurama && src_status.trim().is_empty() {
        vec!["--in-place".to_string()]
    } else {
        Vec::new()
    }
}

/// What follows cargo-mutants' own options: `-- -- <filters>` reaches the
/// test binaries as `cargo test -- <filters>`.
fn test_arguments(tests: &[String]) -> Vec<String> {
    if tests.is_empty() {
        return Vec::new();
    }
    ["--", "--"]
        .iter()
        .map(|arg| arg.to_string())
        .chain(tests.iter().cloned())
        .collect()
}

impl Outcomes {
    fn extend(&mut self, other: Outcomes) {
        self.survived.extend(other.survived);
        self.caught.extend(other.caught);
        self.unviable.extend(other.unviable);
        self.timed_out.extend(other.timed_out);
    }
}

fn parse(args: &[String]) -> Result<Request, String> {
    let mut plan = Plan {
        paths: Vec::new(),
        base: None,
        timeout: None,
    };
    let mut rest = args.iter().map(String::as_str);
    while let Some(arg) = rest.next() {
        match arg {
            "--base" => plan.base = Some(crate::value_of(arg, &mut rest, USAGE)?),
            "--timeout" => plan.timeout = Some(crate::value_of(arg, &mut rest, USAGE)?),
            "--help" | "-h" => return Ok(Request::Help),
            flag if flag.starts_with('-') => {
                return Err(format!("unknown argument `{flag}`\n\n{USAGE}"));
            }
            path => plan.paths.push(path.to_string()),
        }
    }
    Ok(Request::Run(plan))
}

/// cargo-mutants is a binary the developer installs, like cargo-machete; a
/// missing one must name the install command instead of a spawn error.
fn require_mutants() -> Result<(), String> {
    if crate::cargo_has("mutants") {
        Ok(())
    } else {
        Err("cargo-mutants is not installed: cargo install --locked cargo-mutants".into())
    }
}

/// The diff `--in-diff` needs: the branch base against the working tree, in
/// one `git diff`, so that the new text of every hunk is the source
/// cargo-mutants reads (it stops with exit 5 when the two differ). Rust files
/// that are not tracked yet carry no hunk in that diff, so each one is added
/// as the new file it is.
fn write_branch_diff(base: &str, package: Package, out_dir: &Path) -> Result<PathBuf, String> {
    let merge_base = git(&["merge-base", base, "HEAD"])?;
    let mut diff = git(&["diff", merge_base.trim(), "--", package.sources()])?;
    for file in git(&[
        "ls-files",
        "--others",
        "--exclude-standard",
        "--",
        package.sources(),
    ])?
    .lines()
    .filter(|line| !line.is_empty())
    {
        diff.push_str(&new_file_diff(file)?);
    }
    let path = match package {
        Package::Kurama => out_dir.join("mutation-diff.patch"),
        Package::Xtask => out_dir.join("mutation-diff-xtask.patch"),
    };
    std::fs::write(&path, &diff).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// `git diff --no-index` reports a difference with exit code 1, which the
/// shared `git` helper reads as a failure, so this one keeps the output.
fn new_file_diff(path: &str) -> Result<String, String> {
    let output = Command::new("git")
        .args(["diff", "--no-index", "--", "/dev/null", path])
        .current_dir(root())
        .output()
        .map_err(|e| format!("git diff --no-index {path}: {e}"))?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// cargo-mutants selects files by glob, and a directory name matches none of
/// them: `src/adapters/auth/` is the files under it.
fn file_glob(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if root().join(trimmed).is_dir() {
        format!("{trimmed}/**")
    } else {
        path.to_string()
    }
}

/// Runs one cargo-mutants run and reads the `mutants.out/` it wrote under
/// `output`. The one an earlier run left -- or the main checkout's, cloned
/// with `target/` -- is removed first: a run with no mutant to filter writes
/// none, and must read as no result rather than as that earlier one.
fn run_outcomes(
    output: &Path,
    run: impl FnOnce() -> Result<(), String>,
) -> Result<Outcomes, String> {
    let dir = output.join("mutants.out");
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    run()?;
    Ok(read_outcomes(&dir))
}

fn read_outcomes(dir: &Path) -> Outcomes {
    Outcomes {
        survived: read_mutants(dir, "missed.txt"),
        caught: read_mutants(dir, "caught.txt"),
        unviable: read_mutants(dir, "unviable.txt"),
        timed_out: read_mutants(dir, "timeout.txt"),
    }
}

fn read_mutants(dir: &Path, name: &str) -> Vec<String> {
    std::fs::read_to_string(dir.join(name))
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

fn render_report(scope: &str, outcomes: &Outcomes) -> String {
    let head = crate::short_head();
    let mut out = format!(
        "## Mutation report\n\nScope: `{scope}` at `{head}`. Result: **{}**.\n\n\
         | Outcome | Mutants |\n| --- | --- |\n\
         | survived | {} |\n| caught | {} |\n| unviable | {} |\n| timed out | {} |\n",
        if outcomes.survived.is_empty() {
            "PASS"
        } else {
            "FAIL"
        },
        outcomes.survived.len(),
        outcomes.caught.len(),
        outcomes.unviable.len(),
        outcomes.timed_out.len(),
    );
    if outcomes.survived.is_empty() {
        out.push_str("\nNo mutant survived: every change to the mutated code failed a test.\n");
    } else {
        out.push_str(
            "\n### Survived\n\n\
             Each line is a change to production code that the whole suite still passed. \
             The test that should have failed asserts the wrong thing, or the code is \
             unreachable. Fix the test, then run `cargo xtask mutate` again:\n\n",
        );
        for mutant in &outcomes.survived {
            out.push_str(&format!("- `{mutant}`\n"));
        }
    }
    if !outcomes.timed_out.is_empty() {
        out.push_str(
            "\n### Timed out\n\n\
             The suite did not finish within `--timeout` for these; a mutant that hangs is \
             not caught either:\n\n",
        );
        for mutant in &outcomes.timed_out {
            out.push_str(&format!("- `{mutant}`\n"));
        }
    }
    out.push_str(
        "\nPer-mutant logs: `target/agent/mutants.out/`, and `target/agent/xtask/mutants.out/` \
         for xtask.\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args;

    fn plan(request: Request) -> Plan {
        match request {
            Request::Run(plan) => plan,
            Request::Help => panic!("expected a run request"),
        }
    }

    #[test]
    fn mutating_defaults_to_the_branch_diff() {
        assert_eq!(
            plan(parse(&[]).unwrap()),
            Plan {
                paths: vec![],
                base: None,
                timeout: None,
            }
        );
    }

    #[test]
    fn paths_are_positional_and_the_base_and_timeout_are_flags() {
        assert_eq!(
            plan(
                parse(&args(&[
                    "src/adapters/auth/",
                    "src/domain/functions/export.rs",
                    "--base",
                    "main",
                    "--timeout",
                    "90",
                ]))
                .unwrap()
            ),
            Plan {
                paths: vec![
                    "src/adapters/auth/".into(),
                    "src/domain/functions/export.rs".into()
                ],
                base: Some("main".into()),
                timeout: Some("90".into()),
            }
        );
    }

    #[test]
    fn a_flag_without_a_value_is_a_usage_error() {
        assert!(parse(&args(&["--timeout"])).is_err());
        assert!(parse(&args(&["--base"])).is_err());
        assert!(parse(&args(&["--in-diff", "x.patch"])).is_err());
    }

    #[test]
    fn help_is_a_request_of_its_own() {
        assert_eq!(parse(&args(&["--help"])).unwrap(), Request::Help);
    }

    /// A directory passes no glob of cargo-mutants, so the scope override
    /// would silently mutate nothing.
    #[test]
    fn a_directory_scope_becomes_the_glob_of_its_files() {
        assert_eq!(file_glob("src/adapters/auth/"), "src/adapters/auth/**");
        assert_eq!(file_glob("src/adapters/auth"), "src/adapters/auth/**");
        assert_eq!(
            file_glob("src/domain/functions/export.rs"),
            "src/domain/functions/export.rs"
        );
    }

    #[test]
    fn a_surviving_mutant_fails_the_report_and_is_listed() {
        let outcomes = Outcomes {
            survived: vec!["src/domain/functions/export.rs:12:5: replace + with -".into()],
            caught: vec!["src/domain/functions/export.rs:14:9: replace == with !=".into()],
            ..Outcomes::default()
        };

        let report = render_report("diff against origin/dev", &outcomes);

        assert!(report.contains("Result: **FAIL**"), "{report}");
        assert!(report.contains("| survived | 1 |"), "{report}");
        assert!(report.contains("| caught | 1 |"), "{report}");
        assert!(
            report.contains("- `src/domain/functions/export.rs:12:5: replace + with -`"),
            "{report}"
        );
        assert!(
            !report.contains("replace == with !="),
            "a caught mutant is not a finding: {report}"
        );
    }

    #[test]
    fn a_run_with_no_survivor_passes_and_lists_nothing() {
        let outcomes = Outcomes {
            caught: vec!["src/domain/functions/export.rs:14:9: replace == with !=".into()],
            ..Outcomes::default()
        };

        let report = render_report("src/domain/functions/", &outcomes);

        assert!(report.contains("Result: **PASS**"), "{report}");
        assert!(report.contains("No mutant survived"), "{report}");
        assert!(!report.contains("### Survived"), "{report}");
    }

    /// A mutant that hangs the suite is not caught; the report has to say so
    /// rather than leave its line out of both lists.
    #[test]
    fn a_timed_out_mutant_is_reported_separately() {
        let outcomes = Outcomes {
            timed_out: vec!["src/shell/oauth_executor.rs:30:1: replace < with <=".into()],
            ..Outcomes::default()
        };

        let report = render_report("diff against origin/dev", &outcomes);

        assert!(report.contains("| timed out | 1 |"), "{report}");
        assert!(
            report.contains("- `src/shell/oauth_executor.rs:30:1: replace < with <=`"),
            "{report}"
        );
    }

    #[test]
    fn the_outcome_lists_are_read_from_the_files_cargo_mutants_writes() {
        let dir = std::env::temp_dir().join(format!("kurama-mutate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("missed.txt"), "src/a.rs:1:1: replace a\n").unwrap();
        std::fs::write(dir.join("caught.txt"), "src/b.rs:2:2: replace b\n\n").unwrap();

        let outcomes = read_outcomes(&dir);

        assert_eq!(outcomes.survived, ["src/a.rs:1:1: replace a"]);
        assert_eq!(outcomes.caught, ["src/b.rs:2:2: replace b"]);
        // The files a run without them never writes.
        assert!(outcomes.unviable.is_empty());
        assert!(outcomes.timed_out.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A run with no mutant to filter writes no `mutants.out/`; what an
    /// earlier run (or a cloned `target/`) left there is not its result.
    #[test]
    fn a_run_that_writes_nothing_reports_nothing_left_by_an_earlier_one() {
        let output =
            std::env::temp_dir().join(format!("kurama-mutate-stale-{}", std::process::id()));
        let stale = output.join("mutants.out");
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(stale.join("missed.txt"), "xtask/src/impact.rs:1:1: stale\n").unwrap();
        std::fs::write(stale.join("caught.txt"), "xtask/src/impact.rs:2:2: stale\n").unwrap();

        let outcomes = run_outcomes(&output, || Ok(())).unwrap();

        assert!(outcomes.survived.is_empty(), "{:?}", outcomes.survived);
        assert!(outcomes.caught.is_empty(), "{:?}", outcomes.caught);

        // What the run itself writes is read.
        let outcomes = run_outcomes(&output, || {
            std::fs::create_dir_all(&stale).unwrap();
            std::fs::write(stale.join("missed.txt"), "xtask/src/a.rs:3:3: new\n").unwrap();
            Ok(())
        })
        .unwrap();
        assert_eq!(outcomes.survived, ["xtask/src/a.rs:3:3: new"]);
        std::fs::remove_dir_all(&output).unwrap();
    }

    /// kurama's tests need the feature and xtask has none, so a path decides
    /// both the package and the arguments.
    #[test]
    fn a_path_under_xtask_is_mutated_as_the_xtask_package() {
        assert_eq!(Package::of_path("xtask/src/ready.rs"), Package::Xtask);
        assert_eq!(Package::of_path("./xtask/src/"), Package::Xtask);
        assert_eq!(Package::of_path("src/shell/executor.rs"), Package::Kurama);
        assert_eq!(
            Package::Xtask.arguments(),
            ["--package", "xtask", "--copy-vcs", "true"]
        );
        assert_eq!(Package::Kurama.arguments(), ["--features", "test-fakes"]);
        assert_eq!(Package::Xtask.sources(), "xtask/src/*.rs");
        assert_eq!(Package::Kurama.sources(), "src/*.rs");
    }

    /// Two runs write two `mutants.out/`; the kurama one stays where the
    /// report and the documentation say it is.
    #[test]
    fn the_two_packages_write_their_logs_apart() {
        let out = Path::new("/t/agent");
        assert_eq!(Package::Kurama.output(out), out);
        assert_eq!(Package::Xtask.output(out), out.join("xtask"));
    }

    /// Each package gets the paths that are its own and nothing else, so a
    /// kurama file is never run under `--package xtask`.
    #[test]
    fn paths_are_split_by_the_package_they_belong_to() {
        let paths = args(&["src/a.rs", "xtask/src/b.rs", "src/c/"]);

        let kurama = path_run(Package::Kurama, &paths).expect("kurama has paths");
        let xtask = path_run(Package::Xtask, &paths).expect("xtask has one");

        assert_eq!(kurama.scope, "src/a.rs, src/c/");
        assert_eq!(kurama.filter, ["--file", "src/a.rs", "--file", "src/c/"]);
        assert_eq!(xtask.scope, "xtask/src/b.rs");
        assert_eq!(xtask.package, Package::Xtask);
        assert!(path_run(Package::Xtask, &args(&["src/a.rs"])).is_none());
    }

    #[test]
    fn an_empty_diff_is_no_run_and_a_diff_is_one() {
        let dir = std::env::temp_dir().join(format!("kurama-mutate-diff-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let empty = dir.join("empty.patch");
        std::fs::write(&empty, "").unwrap();
        let diff = dir.join("diff.patch");
        std::fs::write(&diff, "--- a/xtask/src/b.rs\n+++ b/xtask/src/b.rs\n").unwrap();

        assert!(run_of_diff(Package::Xtask, "dev", &empty).is_none());
        let run = run_of_diff(Package::Xtask, "dev", &diff).expect("a run");
        assert_eq!(
            run.filter,
            ["--in-diff".to_string(), diff.display().to_string()]
        );
        assert_eq!(run.scope, "xtask/src/*.rs diff against dev");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The report of two runs counts both.
    #[test]
    fn the_outcomes_of_two_runs_add_up() {
        let mut outcomes = Outcomes {
            survived: vec!["a".into()],
            caught: vec!["b".into()],
            ..Outcomes::default()
        };
        outcomes.extend(Outcomes {
            survived: vec!["c".into()],
            caught: vec![],
            unviable: vec!["d".into()],
            timed_out: vec!["e".into()],
        });

        assert_eq!(outcomes.survived, ["a", "c"]);
        assert_eq!(outcomes.caught, ["b"]);
        assert_eq!(outcomes.unviable, ["d"]);
        assert_eq!(outcomes.timed_out, ["e"]);
    }

    #[test]
    fn a_selection_reaches_cargo_test_and_none_runs_everything() {
        assert!(test_arguments(&[]).is_empty());
        assert_eq!(
            test_arguments(&args(&[
                "database::",
                "db_unknown_database_and_missing_operation_exit_2"
            ])),
            [
                "--",
                "--",
                "database::",
                "db_unknown_database_and_missing_operation_exit_2"
            ]
        );
    }

    /// `branch-check` just ran the tests: no baseline, and a timeout of twice
    /// what they took, never under the floor. A run of its own keeps the
    /// baseline and lets cargo-mutants scale the timeout from it, unless
    /// `--timeout` names one.
    #[test]
    fn the_timeout_follows_the_measured_tests_and_the_baseline_is_skipped_only_after_them() {
        assert_eq!(
            Timing::Measured { tests_secs: 116 }.arguments(),
            ["--baseline", "skip", "--timeout", "232"]
        );
        assert_eq!(
            Timing::Measured { tests_secs: 12 }.arguments(),
            ["--baseline", "skip", "--timeout", "60"]
        );
        assert_eq!(
            Timing::Baseline { timeout: None }.arguments(),
            ["--timeout-multiplier", "2", "--minimum-test-timeout", "60"]
        );
        assert_eq!(
            Timing::Baseline {
                timeout: Some("90".into())
            }
            .arguments(),
            ["--timeout", "90"]
        );
    }

    /// The mutant that replaced `open_aws_console` with `Ok(())` survived the
    /// tests of `exec` and `assume-role`, which own `shell/cli/executor.rs`;
    /// only the console-federation cases catch it. The selection a mutant of
    /// that file runs, read from this repository's own map and imports, has
    /// to keep them.
    #[test]
    fn a_mutant_of_the_executor_runs_the_console_federation_cases() {
        let features = crate::features::load_features().unwrap();
        let imports = imports_by_file();

        let (owners, tests) = mutant_tests(
            &features,
            "src/shell/cli/executor.rs",
            &imports["src/shell/cli/executor.rs"],
        );

        assert!(
            owners.contains(&"console-federation".to_string()),
            "{owners:?}"
        );
        for case in [
            "uc07_console_fetches_signin_token_and_opens_login_url",
            "console_federation_failure_exits_4_without_opening_a_browser",
        ] {
            assert!(tests.contains(&case.to_string()), "{case}: {tests:?}");
        }
    }

    #[test]
    fn the_estimate_names_the_count_and_the_minutes() {
        assert_eq!(
            Timing::Measured { tests_secs: 100 }.estimate(28),
            "==> 28 mutant(s), at most about 56 min: each rebuilds (about 20 s) and runs at most \
             the 100 s of tests the gate just ran"
        );
        assert!(
            Timing::Baseline { timeout: None }
                .estimate(333)
                .starts_with("==> 333 mutant(s): each costs about the unmutated run")
        );
    }

    #[test]
    fn a_listed_mutant_is_one_non_empty_line() {
        assert_eq!(
            listed("src/a.rs:1:1: replace a\nsrc/b.rs:2:2: replace b\n\n"),
            2
        );
        assert_eq!(listed(""), 0);
    }

    #[test]
    fn a_path_run_is_counted_next_to_the_diff() {
        assert_eq!(
            compare_with_diff(333, 28, "dev"),
            "==> PATH: 333 mutant(s) over whole files; the lines this branch changed against \
             dev: 28. To mutate only those, run `cargo xtask mutate` without PATH"
        );
    }

    #[test]
    fn the_files_of_a_diff_are_its_new_side() {
        let diff = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n\
                    -x\n+y\ndiff --git a/dev/null b/src/new.rs\n--- /dev/null\n+++ b/src/new.rs\n";
        assert_eq!(files_of_diff(diff), ["src/a.rs", "src/new.rs"]);
        assert!(files_of_diff("").is_empty());
    }

    /// Each group runs the tests of the features that own its files; two
    /// files of one feature are one run, a file two features share runs
    /// both's tests, and a file nobody owns runs what the caller chose.
    #[test]
    fn changed_files_are_grouped_by_their_owners_with_their_tests() {
        let run = Run {
            package: Package::Kurama,
            scope: "src/*.rs diff against dev".into(),
            filter: args(&["--in-diff", "d.patch"]),
            tests: vec![],
        };
        let owners = |file: &str| -> (Vec<String>, Vec<String>) {
            match file {
                "src/a.rs" | "src/b.rs" => (args(&["config"]), args(&["adapters::config::"])),
                "src/c.rs" => (args(&["api-client", "oauth"]), args(&["api_", "oauth::"])),
                _ => (vec![], args(&["ignored"])),
            }
        };
        let runs = grouped_runs(
            run,
            &args(&["src/a.rs", "src/c.rs", "src/b.rs", "src/stray.rs"]),
            &args(&["fallback::"]),
            owners,
        );

        let summary: Vec<(String, Vec<String>, Vec<String>)> = runs
            .into_iter()
            .map(|run| (run.scope, run.filter, run.tests))
            .collect();
        assert_eq!(
            summary,
            [
                (
                    "src/stray.rs (no feature owns it)".to_string(),
                    args(&["--in-diff", "d.patch", "--file", "src/stray.rs"]),
                    args(&["fallback::"]),
                ),
                (
                    "src/c.rs (tests of api-client, oauth)".to_string(),
                    args(&["--in-diff", "d.patch", "--file", "src/c.rs"]),
                    args(&["api_", "oauth::"]),
                ),
                (
                    "src/a.rs, src/b.rs (tests of config)".to_string(),
                    args(&[
                        "--in-diff",
                        "d.patch",
                        "--file",
                        "src/a.rs",
                        "--file",
                        "src/b.rs"
                    ]),
                    args(&["adapters::config::"]),
                ),
            ]
        );
    }

    /// In place only over a clean src/, and never for xtask.
    #[test]
    fn kurama_is_mutated_in_place_only_when_src_has_nothing_uncommitted() {
        assert_eq!(in_place_arguments(Package::Kurama, ""), ["--in-place"]);
        assert!(in_place_arguments(Package::Kurama, " M src/console.rs\n").is_empty());
        assert!(in_place_arguments(Package::Xtask, "").is_empty());
    }
}
