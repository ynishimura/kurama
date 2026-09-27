//! `cargo xtask worktree add|list|remove <ISSUE>`: one worktree per issue.
//!
//! Parallel work means one checkout per branch, and a checkout of this
//! repository carries its own `target/`, which DuckDB fills with gigabytes.
//! The three subcommands make the cost visible and reversible: `add` creates
//! the worktree and its branch from `dev`, `list` prints what every worktree
//! spends on `target/` and what they spend together, and `remove` deletes both
//! -- unless the worktree still holds commits that no remote and no other
//! branch has, or changes that were never committed.
//!
//! `add` and `remove` also move the issue on the board: several agents share
//! one account here, so the Project's `Status` is the only place that can say
//! an issue is taken. It is a declaration, not a lock (`board`).
//!
//! `add` also gives the worktree what trying it against real services needs
//! without touching what the other worktrees use: a copy of the daily
//! configuration at `.kurama/config.toml`, so a section one branch adds cannot
//! make another branch's binary refuse the file, and with `--install` a
//! signed `kurama-<ISSUE>`, which reads the same keychain entries because it
//! carries the same signature. `remove` deletes both.
//!
//! `add` starts the worktree's `target/` as an APFS clone (`cp -c -R`) of the
//! main checkout's, so the first build of a worktree does not compile DuckDB
//! again; the blocks stay shared until a build rewrites them. `target/agent/`
//! is dropped from the clone: its reports are the main checkout's. It is a copy,
//! not a shared `CARGO_TARGET_DIR`, which runs another worktree's code.
//!
//! `add` takes several issues for one worktree: small issues of one feature
//! pay for the build and the gate once. The worktree and branch are named
//! after the first; all of them are remembered in the branch's git config
//! (`branch.<name>.kurama-issues`), which is how `remove <FIRST>` gives every
//! one of them back.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::board::{self, IssueState, Standing};
use crate::git;

const USAGE: &str = "\
usage: cargo xtask worktree <add|list|remove> [ISSUE]... [SLUG]

  add <ISSUE>... [SLUG] [--install]
                      create ../<repo>-<ISSUE> on a new branch feat/<ISSUE>-<SLUG>,
                      branched from dev, and move the issues to In progress;
                      refused when the board or the dependencies say one of them
                      is not an issue to start, and then nothing is made. Several
                      issues share one worktree named after the first. target/ is
                      cloned from the main checkout's (cp -c), less target/agent/;
                      the daily config is copied to .kurama/config.toml in it;
                      --install also installs its signed build as
                      ~/.cargo/bin/kurama-<ISSUE>
  list                every worktree of this clone with the size of its target/,
                      and what they add up to
  remove <ISSUE>      delete that worktree together with its target/ and its
                      kurama-<ISSUE>, and put every In progress issue it was
                      added for back to Backlog; refused while it
                      holds uncommitted changes, or commits that no remote and
                      no other branch has
";

/// Feature work branches from `dev` and merges back there (AGENTS.md).
const BASE_BRANCH: &str = "dev";

/// What the listing calls a worktree that is on no branch.
const DETACHED: &str = "(detached)";

#[derive(Debug, PartialEq)]
enum Request {
    Help,
    Add {
        issues: Vec<u64>,
        slug: Option<String>,
        install: bool,
    },
    List,
    Remove {
        issue: u64,
    },
}

#[derive(Debug, PartialEq)]
struct Worktree {
    path: PathBuf,
    branch: String,
}

pub fn worktree(args: &[String]) -> Result<(), String> {
    match parse(args)? {
        Request::Help => {
            print!("{USAGE}");
            Ok(())
        }
        Request::Add {
            issues,
            slug,
            install,
        } => add(&issues, slug.as_deref(), install),
        Request::List => list(),
        Request::Remove { issue } => remove(issue),
    }
}

fn parse(args: &[String]) -> Result<Request, String> {
    let install = args.iter().any(|arg| arg == "--install");
    let args: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|arg| *arg != "--install")
        .collect();
    if install && args.first() != Some(&"add") {
        return Err(format!("--install belongs to `add`\n\n{USAGE}"));
    }
    match args.as_slice() {
        ["--help"] | ["-h"] | ["help"] => Ok(Request::Help),
        ["list"] => Ok(Request::List),
        ["add", rest @ ..] => parse_add(rest, install),
        ["remove", issue] => Ok(Request::Remove {
            issue: board::issue_number(issue, USAGE)?,
        }),
        [] => Err(format!("a subcommand is needed\n\n{USAGE}")),
        _ => Err(format!(
            "`{}` is not a worktree subcommand\n\n{USAGE}",
            args.join(" ")
        )),
    }
}

/// The issue numbers, then at most one slug: the first word that is not an
/// issue number.
fn parse_add(rest: &[&str], install: bool) -> Result<Request, String> {
    let count = rest
        .iter()
        .take_while(|arg| board::issue_number(arg, USAGE).is_ok())
        .count();
    let issues: Vec<u64> = rest[..count]
        .iter()
        .map(|arg| board::issue_number(arg, USAGE))
        .collect::<Result<_, _>>()?;
    let slug = match &rest[count..] {
        [] => None,
        [slug] => Some((*slug).to_string()),
        more => {
            return Err(format!(
                "`{}` is more than one slug\n\n{USAGE}",
                more.join(" ")
            ));
        }
    };
    if issues.is_empty() {
        return Err(format!("`add` needs an issue number\n\n{USAGE}"));
    }
    Ok(Request::Add {
        issues,
        slug,
        install,
    })
}

// ---------------------------------------------------------------------------
// add
// ---------------------------------------------------------------------------

/// The git config key that lists the issues a worktree was added for.
fn issues_key(branch: &str) -> String {
    format!("branch.{branch}.kurama-issues")
}

fn add(issues: &[u64], slug: Option<&str>, install: bool) -> Result<(), String> {
    let issue = issues[0];
    let main = main_worktree()?;
    let path = worktree_path(&main, issue);
    if path.exists() {
        return Err(format!("{} already exists", path.display()));
    }
    // The board is the only thing that can say an issue is taken, so a board
    // that cannot be reached is a refusal rather than a warning: creating the
    // worktree anyway drops every check below it.
    let board_issues = board::open_issues().map_err(|error| {
        format!(
            "{error}\n    `worktree add` needs the board to see whether anyone is on \
             {}; to work offline, `git worktree add` makes the branch without it",
            board::numbers(issues)
        )
    })?;
    // Every issue is judged before anything is made, and every refusal is
    // named: a batch is taken whole or not at all.
    let mut states = Vec::new();
    let mut refusals = Vec::new();
    for number in issues {
        match board::find_open(&board_issues, *number) {
            Ok(state) => match refusal(state) {
                Some(refusal) => refusals.push(refusal),
                None => states.push(state),
            },
            Err(error) => refusals.push(error),
        }
    }
    if !refusals.is_empty() {
        return Err(refusals.join("\n"));
    }
    let branch = branch_name(issue, slug);
    git(&[
        "worktree",
        "add",
        "-b",
        &branch,
        &path.to_string_lossy(),
        BASE_BRANCH,
    ])?;
    let listed: Vec<String> = issues.iter().map(u64::to_string).collect();
    git(&["config", &issues_key(&branch), &listed.join(" ")])?;
    // The worktree is created first: a failure there must not leave the board
    // saying someone is on an issue nobody is on.
    let changes: Vec<(&IssueState, &str)> = states
        .iter()
        .map(|state| (*state, board::IN_PROGRESS))
        .collect();
    for (number, result) in board::set_statuses(&changes) {
        if let Err(error) = result {
            eprintln!(
                "==> the worktree is there, but the board was not updated: {error}\n                 set #{number} to `{}` by hand, or others will pick it up too",
                board::IN_PROGRESS
            );
        }
    }
    println!("{}\t{branch}", path.display());
    eprintln!("{}", clone_target(&main, &path));
    let config = match copy_daily_config(&path) {
        Ok(copied) => copied,
        Err(error) => {
            eprintln!("==> the configuration was not copied: {error}");
            None
        }
    };
    let installed = install && install_alias(&path, issue);
    eprintln!(
        "==> cd {} && mise trust && cargo xtask branch-check",
        path.display()
    );
    eprint!(
        "{}",
        real_environment_hint(&path, issue, config.is_some(), installed)
    );
    Ok(())
}

/// Starts the worktree's `target/` as a clone of the main checkout's
/// (`cp -c`: APFS clonefile, so only what a build rewrites takes new space),
/// less `target/agent/`, whose reports belong to the main checkout's runs.
/// Without one, or when the clone fails, the worktree builds from scratch as
/// it always did; the line returned says which.
fn clone_target(main: &Path, worktree: &Path) -> String {
    let source = main.join("target");
    if !source.is_dir() {
        return format!(
            "==> {} has no target/ to clone; the first build starts from scratch",
            main.display()
        );
    }
    let output = Command::new("cp")
        .args(["-c", "-R"])
        .arg(&source)
        .arg(worktree.join("target"))
        .output();
    match output {
        Ok(output) if output.status.success() => {
            // The reports under target/agent/ are the main checkout's runs,
            // not results of this branch. The clone was just made, so the
            // only error is its absence, which is what is wanted.
            let _ = std::fs::remove_dir_all(worktree.join("target/agent"));
            format!(
                "==> target/ cloned from {} without agent/ (shared blocks until a build rewrites them)",
                source.display()
            )
        }
        Ok(output) => format!(
            "==> target/ was not cloned ({}); the first build starts from scratch",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        Err(error) => {
            format!("==> target/ was not cloned (cp: {error}); the first build starts from scratch")
        }
    }
}

/// The file kurama reads when nothing names another one. The same precedence
/// as `Config::config_source`: `KURAMA_CONFIG_PATH`, else
/// `~/.config/kurama/config.toml`.
fn daily_config() -> Option<PathBuf> {
    match std::env::var("KURAMA_CONFIG_PATH") {
        Ok(named) if !named.is_empty() => Some(PathBuf::from(named)),
        _ => std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(".config/kurama/config.toml")),
    }
}

/// Where a worktree keeps its own configuration. `.kurama/` is ignored by git,
/// so the copy is never committed and does not stop `remove`.
fn worktree_config(worktree: &Path) -> PathBuf {
    worktree.join(".kurama/config.toml")
}

/// Copies the daily configuration into the worktree, readable by the owner
/// only. `None` when there is no daily configuration to copy.
fn copy_daily_config(worktree: &Path) -> Result<Option<PathBuf>, String> {
    let Some(source) = daily_config().filter(|source| source.is_file()) else {
        return Ok(None);
    };
    let target = worktree_config(worktree);
    std::fs::create_dir_all(target.parent().unwrap())
        .map_err(|error| format!("{}: {error}", target.display()))?;
    std::fs::copy(&source, &target).map_err(|error| format!("{}: {error}", target.display()))?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("{}: {error}", target.display()))?;
    Ok(Some(target))
}

/// The name the worktree's build is installed under.
fn alias(issue: u64) -> String {
    format!("kurama-{issue}")
}

/// `cargo xtask install-signed --as kurama-<ISSUE>`, run in the worktree so it
/// builds the worktree's code. A failure is reported, not fatal: the worktree
/// is already there and is what the issue needs.
fn install_alias(worktree: &Path, issue: u64) -> bool {
    let arguments = install_arguments(issue);
    eprintln!("==> cargo {}", arguments.join(" "));
    let installed = crate::cargo()
        .args(&arguments)
        .current_dir(worktree)
        .env_remove("KURAMA_XTASK_ROOT")
        .status()
        .is_ok_and(|status| status.success());
    if !installed {
        eprintln!(
            "==> {} was not installed; run `cargo {}` in the worktree",
            alias(issue),
            arguments.join(" ")
        );
    }
    installed
}

fn install_arguments(issue: u64) -> Vec<String> {
    ["xtask", "install-signed", "--as", &alias(issue)]
        .iter()
        .map(|arg| arg.to_string())
        .collect()
}

/// The one place that says how to start checking the worktree for real.
fn real_environment_hint(worktree: &Path, issue: u64, config: bool, installed: bool) -> String {
    let mut out = String::from("==> to try this worktree against real services:\n");
    if config {
        out.push_str(&format!(
            "    export KURAMA_CONFIG_PATH={}\n",
            worktree_config(worktree).display()
        ));
    } else {
        out.push_str("    (no daily configuration was found to copy)\n");
    }
    if installed {
        out.push_str(&format!("    {} status\n", alias(issue)));
    } else {
        out.push_str(&format!(
            "    cargo xtask install-signed --as {}   # then {} status\n",
            alias(issue),
            alias(issue)
        ));
    }
    out
}

/// Why this issue is not one to start, in the words the person needs. `None`
/// means nothing on GitHub's side stands in the way; whether another agent
/// took it a second ago is not knowable, and is not claimed to be.
fn refusal(state: &IssueState) -> Option<String> {
    let issue = state.number;
    match board::standing(state) {
        Standing::Ready => None,
        Standing::Taken { status } => Some(format!(
            "issue #{issue} is already `{status}`{}; \
             `cargo xtask ready` lists what is free",
            state
                .status_updated
                .as_deref()
                .map(|since| format!(" (since {since})"))
                .unwrap_or_default()
        )),
        Standing::Tracker => Some(format!(
            "issue #{issue} is a tracker: implement one of its sub-issues instead"
        )),
        Standing::NeedsHuman => Some(format!(
            "issue #{issue} is labelled `{}`: a person has to close it",
            board::NEEDS_HUMAN
        )),
        Standing::Blocked { by } => Some(format!(
            "issue #{issue} is blocked by {}; close those first",
            board::numbers(&by)
        )),
        Standing::OffBoard => Some(format!(
            "issue #{issue} is not an item of project {}, so the board cannot say \
             whether anyone is on it",
            board::PROJECT
        )),
        Standing::Disagrees { status, by } => Some(format!(
            "issue #{issue} says `{status}` on the board but {} is still open",
            board::numbers(&by)
        )),
    }
}

/// The branch of an issue: its number, and the words of the slug that says
/// what the work is. Without a slug the number alone names the branch.
fn branch_name(issue: u64, slug: Option<&str>) -> String {
    let mut words = String::new();
    for character in slug.unwrap_or_default().chars() {
        if character.is_ascii_alphanumeric() {
            words.push(character.to_ascii_lowercase());
        } else if !words.ends_with('-') {
            words.push('-');
        }
    }
    match words.trim_matches('-') {
        "" => format!("feat/{issue}"),
        words => format!("feat/{issue}-{words}"),
    }
}

/// Worktrees live next to the checkout they came from, named after it: the
/// main worktree of this clone plus the issue number.
fn worktree_path(main: &Path, issue: u64) -> PathBuf {
    let name = main.file_name().unwrap_or_default().to_string_lossy();
    main.with_file_name(format!("{name}-{issue}"))
}

/// The checkout the clone was made in: the parent of the common git directory,
/// which every linked worktree points at.
fn main_worktree() -> Result<PathBuf, String> {
    let common = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    Path::new(common.trim())
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("{} has no parent directory", common.trim()))
}

// ---------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------

fn list() -> Result<(), String> {
    let worktrees = parse_worktrees(&git(&["worktree", "list", "--porcelain"])?);
    let sizes: Vec<u64> = worktrees
        .iter()
        .map(|worktree| directory_size(&worktree.path.join("target")))
        .collect();
    print!("{}", render_list(&worktrees, &sizes));
    Ok(())
}

/// `git worktree list --porcelain`: one paragraph per worktree, `worktree
/// <path>` first and `branch refs/heads/<name>` (or `detached`) among the rest.
fn parse_worktrees(porcelain: &str) -> Vec<Worktree> {
    let mut worktrees: Vec<Worktree> = Vec::new();
    for line in porcelain.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            worktrees.push(Worktree {
                path: PathBuf::from(path),
                branch: DETACHED.to_string(),
            });
        } else if let Some(reference) = line.strip_prefix("branch ")
            && let Some(last) = worktrees.last_mut()
        {
            last.branch = reference
                .strip_prefix("refs/heads/")
                .unwrap_or(reference)
                .to_string();
        }
    }
    worktrees
}

fn render_list(worktrees: &[Worktree], sizes: &[u64]) -> String {
    let paths: Vec<String> = worktrees
        .iter()
        .map(|worktree| worktree.path.display().to_string())
        .collect();
    let path_width = paths.iter().map(String::len).chain([5]).max().unwrap_or(5);
    let branch_width = worktrees
        .iter()
        .map(|worktree| worktree.branch.len())
        .chain([6])
        .max()
        .unwrap_or(6);
    let mut out = String::new();
    for ((path, worktree), size) in paths.iter().zip(worktrees).zip(sizes) {
        out.push_str(&format!(
            "{path:path_width$}  {:branch_width$}  {}\n",
            worktree.branch,
            human_size(*size)
        ));
    }
    out.push_str(&format!(
        "{:path_width$}  {:branch_width$}  {}\n",
        "total",
        "",
        human_size(sizes.iter().sum())
    ));
    out.push_str(
        "(a target/ that `worktree add` cloned shares its unchanged blocks with the main \
         checkout's; du counts them in each, so the total overstates the disk in use)\n",
    );
    out
}

/// `du -sk`, because walking a `target/` of a few hundred thousand files to
/// add up its blocks is the slow way to ask the file system what it knows.
pub(crate) fn directory_size(path: &Path) -> u64 {
    if !path.exists() {
        return 0;
    }
    Command::new("du")
        .args(["-sk", &path.to_string_lossy()])
        .output()
        .map(|output| kilobytes(&String::from_utf8_lossy(&output.stdout)))
        .unwrap_or(0)
}

fn kilobytes(du_output: &str) -> u64 {
    du_output
        .split_whitespace()
        .next()
        .and_then(|blocks| blocks.parse().ok())
        .unwrap_or(0)
}

pub(crate) fn human_size(kilobytes: u64) -> String {
    const MEGABYTE: u64 = 1024;
    const GIGABYTE: u64 = 1024 * 1024;
    if kilobytes >= GIGABYTE {
        format!("{:.1} GB", kilobytes as f64 / GIGABYTE as f64)
    } else if kilobytes >= MEGABYTE {
        format!("{:.1} MB", kilobytes as f64 / MEGABYTE as f64)
    } else {
        format!("{kilobytes} KB")
    }
}

// ---------------------------------------------------------------------------
// remove
// ---------------------------------------------------------------------------

fn remove(issue: u64) -> Result<(), String> {
    let path = worktree_path(&main_worktree()?, issue);
    let worktrees = parse_worktrees(&git(&["worktree", "list", "--porcelain"])?);
    let Some(entry) = worktrees.iter().find(|worktree| worktree.path == path) else {
        return Err(format!(
            "no worktree at {}; run `cargo xtask worktree list`",
            path.display()
        ));
    };
    let query = unpushed_query(&entry.branch);
    let query: Vec<&str> = query.iter().map(String::as_str).collect();
    let blockers = removal_blockers(
        &git_in(&path, &query)?,
        &git_in(&path, &["status", "--porcelain"])?,
    );
    if !blockers.is_empty() {
        return Err(format!(
            "{} still holds work: {}",
            path.display(),
            blockers.join("; ")
        ));
    }

    let target = path.join("target");
    let freed = directory_size(&target);
    if target.exists() {
        std::fs::remove_dir_all(&target)
            .map_err(|error| format!("{}: {error}", target.display()))?;
    }
    let branch = entry.branch.clone();
    let merged = merged_into_base(&branch);
    let issues = batch_of(
        issue,
        git(&["config", "--get", &issues_key(&branch)])
            .ok()
            .as_deref(),
    );
    git(&["worktree", "remove", "--force", &path.to_string_lossy()])?;
    remove_alias(issue);
    if merged {
        let listed = board::numbers(&issues);
        eprintln!(
            "==> {branch} is merged into {BASE_BRANCH}, so {listed} stays where it is on the \
             board: pushing {BASE_BRANCH} closes it (`Closes #N`), or set it by hand"
        );
    } else {
        release(&issues);
    }
    println!("removed {} ({} freed)", path.display(), human_size(freed));
    // Removing a worktree leaves its branch, which is why nothing is lost here.
    if branch != DETACHED {
        eprintln!("==> the branch is still here: git branch -d {branch}");
    }
    Ok(())
}

/// The commits this worktree's branch holds alone: on no remote, and on no
/// other branch either. `dev` itself is usually ahead of `origin/dev`, and a
/// worktree branched from it inherits those commits; counting them would make
/// every worktree unremovable while saying nothing about this one's work.
///
/// The exclusion is relative to `refs/heads/`, and it is `--branches` rather
/// than `--all` because `--all` adds `HEAD` back whatever is excluded -- which
/// subtracts the branch from itself and reports every worktree as clean.
fn unpushed_query(branch: &str) -> Vec<String> {
    let mut args: Vec<String> = ["log", "--oneline", "HEAD", "--not"]
        .iter()
        .map(|arg| arg.to_string())
        .collect();
    if branch != DETACHED {
        args.push(format!("--exclude={branch}"));
    }
    args.push("--branches".to_string());
    args.push("--remotes".to_string());
    args
}

/// Whether the branch holds work that reached the base branch: it moved from
/// where it was created, and its tip is in the base. A branch that never
/// moved is also contained in the base, and is an abandoned worktree rather
/// than a finished one.
fn merged_into_base(branch: &str) -> bool {
    if branch == DETACHED {
        return false;
    }
    let reference = format!("refs/heads/{branch}");
    let created = git(&["reflog", "show", "--format=%H", &reference])
        .ok()
        .and_then(|log| log.lines().last().map(str::to_string));
    let tip = git(&["rev-parse", &reference]).ok();
    let (Some(created), Some(tip)) = (created, tip) else {
        return false;
    };
    let in_base = git(&["merge-base", "--is-ancestor", tip.trim(), BASE_BRANCH]).is_ok();
    was_merged(created.trim(), tip.trim(), in_base)
}

fn was_merged(created: &str, tip: &str, tip_in_base: bool) -> bool {
    created != tip && tip_in_base
}

// ---------------------------------------------------------------------------
// claim
// ---------------------------------------------------------------------------

const CLAIM_USAGE: &str = "\
usage: cargo xtask claim <ISSUE>
       cargo xtask claim --release <ISSUE>

  <ISSUE>            move the issue to In progress without making a worktree,
                     refused for the same reasons `worktree add` refuses
  --release <ISSUE>  put an In progress issue back to Backlog
";

/// `worktree add`'s claim without the worktree: an agent working one issue
/// after another in the main checkout still says on the board what it took.
pub fn claim(args: &[String]) -> Result<(), String> {
    let (release_it, issue) = match args {
        [flag] if flag == "--help" || flag == "-h" => {
            print!("{CLAIM_USAGE}");
            return Ok(());
        }
        [issue] => (false, board::issue_number(issue, USAGE)?),
        [flag, issue] if flag == "--release" => (true, board::issue_number(issue, USAGE)?),
        _ => {
            return Err(format!(
                "`{}` is not a claim\n\n{CLAIM_USAGE}",
                args.join(" ")
            ));
        }
    };
    let issues = board::open_issues()?;
    let state = board::find_open(&issues, issue)?;
    if release_it {
        if state.status.as_deref() != Some(board::IN_PROGRESS) {
            println!(
                "#{issue} is `{}`, not `{}`; nothing to give back",
                state.status.as_deref().unwrap_or("off the board"),
                board::IN_PROGRESS
            );
            return Ok(());
        }
        board::set_status(state, board::BACKLOG)?;
        println!("#{issue} is back in `{}`", board::BACKLOG);
        return Ok(());
    }
    if let Some(refusal) = refusal(state) {
        return Err(refusal);
    }
    board::set_status(state, board::IN_PROGRESS)?;
    println!("#{issue} is `{}`", board::IN_PROGRESS);
    Ok(())
}

/// Deletes `kurama-<ISSUE>` when `add --install` put one there.
fn remove_alias(issue: u64) {
    let Ok(installed) = crate::install_signed::installed_path(&alias(issue)) else {
        return;
    };
    if installed.exists() {
        match std::fs::remove_file(&installed) {
            Ok(()) => eprintln!("==> removed {}", installed.display()),
            Err(error) => eprintln!("==> {} is still there: {error}", installed.display()),
        }
    }
}

/// What a removal would leave nowhere else. A worktree is deleted with its
/// `target/` and everything else inside it.
fn removal_blockers(unpushed: &str, status: &str) -> Vec<String> {
    let mut blockers = Vec::new();
    let commits: Vec<&str> = unpushed.lines().filter(|line| !line.is_empty()).collect();
    if !commits.is_empty() {
        blockers.push(format!(
            "{} commit(s) no remote and no other branch has ({})",
            commits.len(),
            commits.join(", ")
        ));
    }
    let changes = status.lines().filter(|line| !line.is_empty()).count();
    if changes > 0 {
        blockers.push(format!("{changes} uncommitted change(s)"));
    }
    blockers
}

/// The issues a worktree was added for: what `add` wrote to the branch's
/// config for a batch, else the one issue it is named after.
fn batch_of(issue: u64, configured: Option<&str>) -> Vec<u64> {
    let listed: Vec<u64> = configured
        .unwrap_or_default()
        .split_whitespace()
        .filter_map(|number| number.parse().ok())
        .collect();
    if listed.is_empty() {
        vec![issue]
    } else {
        listed
    }
}

/// An abandoned worktree leaves the issues claimed, so removing it puts them
/// back. Only `In progress` is moved: `In review` and `Done` were set by
/// the pull request, and a removal is not a reason to undo them. A board that
/// cannot be reached is reported, not treated as a failed removal -- the
/// worktree is already gone.
fn release(numbers: &[u64]) {
    let issues = match board::open_issues() {
        Ok(issues) => issues,
        Err(error) => return eprintln!("==> the board was not read: {error}"),
    };
    let changes: Vec<(&IssueState, &str)> = issues
        .iter()
        .filter(|state| {
            numbers.contains(&state.number) && state.status.as_deref() == Some(board::IN_PROGRESS)
        })
        .map(|state| (state, board::BACKLOG))
        .collect();
    if changes.is_empty() {
        return;
    }
    for (issue, result) in board::set_statuses(&changes) {
        match result {
            Ok(()) => eprintln!("==> #{issue} is back in `{}`", board::BACKLOG),
            Err(error) => eprintln!(
                "==> #{issue} is still `{}` on the board: {error}",
                board::IN_PROGRESS
            ),
        }
    }
}

fn git_in(directory: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(directory)
        .output()
        .map_err(|error| format!("git: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(format!(
            "git {} in {} failed: {}",
            args.join(" "),
            directory.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args;

    #[test]
    fn worktree_takes_one_subcommand_and_an_issue_number() {
        assert_eq!(parse(&args(&["list"])).unwrap(), Request::List);
        assert_eq!(
            parse(&args(&["add", "48"])).unwrap(),
            Request::Add {
                issues: vec![48],
                slug: None,
                install: false,
            }
        );
        assert_eq!(
            parse(&args(&["add", "#48", "two-stage-gate"])).unwrap(),
            Request::Add {
                issues: vec![48],
                slug: Some("two-stage-gate".into()),
                install: false,
            }
        );
        assert_eq!(
            parse(&args(&["add", "48", "--install"])).unwrap(),
            Request::Add {
                issues: vec![48],
                slug: None,
                install: true,
            }
        );
        assert_eq!(
            parse(&args(&["add", "169", "#170", "171", "xtask-tests"])).unwrap(),
            Request::Add {
                issues: vec![169, 170, 171],
                slug: Some("xtask-tests".into()),
                install: false,
            }
        );
        assert!(parse(&args(&["add", "48", "a", "b"])).is_err());
        assert!(parse(&args(&["add", "slug-only"])).is_err());
        assert!(parse(&args(&["remove", "48", "--install"])).is_err());
        assert_eq!(
            parse(&args(&["remove", "48"])).unwrap(),
            Request::Remove { issue: 48 }
        );
        assert_eq!(parse(&args(&["--help"])).unwrap(), Request::Help);
        assert!(parse(&[]).is_err());
        assert!(parse(&args(&["add"])).is_err());
        assert!(parse(&args(&["remove", "HEAD"])).is_err());
        assert!(parse(&args(&["prune"])).is_err());
    }

    #[test]
    fn a_branch_is_named_after_the_issue_and_its_slug() {
        assert_eq!(
            branch_name(53, Some("two-stage-gate")),
            "feat/53-two-stage-gate"
        );
        assert_eq!(
            branch_name(53, Some("Two Stage Gate")),
            "feat/53-two-stage-gate"
        );
        assert_eq!(branch_name(53, None), "feat/53");
        // A title in another script leaves no word a branch name can carry.
        assert_eq!(branch_name(53, Some("ゲート")), "feat/53");
    }

    /// The path is derived from the main worktree, not from the current one:
    /// `add` run inside `kurama-53` must not create `kurama-53-54`.
    #[test]
    fn a_worktree_is_named_after_the_clone_and_the_issue() {
        assert_eq!(
            worktree_path(Path::new("/w/kurama"), 48),
            PathBuf::from("/w/kurama-48")
        );
    }

    #[test]
    fn worktrees_are_read_from_the_porcelain_listing() {
        let porcelain = "worktree /w/kurama\nHEAD abc\nbranch refs/heads/dev\n\n\
                         worktree /w/kurama-53\nHEAD def\nbranch refs/heads/feat/53-two-stage-gate\n\n\
                         worktree /w/kurama-9\nHEAD 012\ndetached\n";

        let worktrees = parse_worktrees(porcelain);

        assert_eq!(
            worktrees,
            [
                Worktree {
                    path: "/w/kurama".into(),
                    branch: "dev".into()
                },
                Worktree {
                    path: "/w/kurama-53".into(),
                    branch: "feat/53-two-stage-gate".into()
                },
                Worktree {
                    path: "/w/kurama-9".into(),
                    branch: "(detached)".into()
                },
            ]
        );
    }

    /// What parallel work costs is the sum, which no single `target/` shows.
    #[test]
    fn the_listing_ends_with_what_every_target_adds_up_to() {
        let worktrees = vec![
            Worktree {
                path: "/w/kurama".into(),
                branch: "dev".into(),
            },
            Worktree {
                path: "/w/kurama-53".into(),
                branch: "feat/53-two-stage-gate".into(),
            },
        ];

        let listing = render_list(&worktrees, &[6 * 1024 * 1024, 2 * 1024 * 1024]);

        assert!(listing.contains("/w/kurama-53"), "{listing}");
        assert!(listing.contains("feat/53-two-stage-gate"), "{listing}");
        assert!(listing.contains("6.0 GB"), "{listing}");
        assert!(listing.contains("8.0 GB\n"), "{listing}");
        // A cloned target/ is counted in each worktree du reads it from.
        assert!(listing.contains("du counts them in each"), "{listing}");
    }

    #[test]
    fn sizes_are_read_from_du_and_printed_in_the_unit_they_reach() {
        assert_eq!(kilobytes("2868432\t/w/kurama/target\n"), 2_868_432);
        assert_eq!(kilobytes(""), 0);
        assert_eq!(human_size(2_868_432), "2.7 GB");
        assert_eq!(human_size(5 * 1024), "5.0 MB");
        assert_eq!(human_size(0), "0 KB");
    }

    /// A worktree is deleted with its `target/` and everything else in it, so
    /// a commit that is nowhere else is what stops a removal.
    #[test]
    fn a_worktree_with_unpushed_commits_is_not_removed() {
        let blockers = removal_blockers("9fceb02 add the two-stage gate\n", "");

        assert_eq!(
            blockers,
            ["1 commit(s) no remote and no other branch has (9fceb02 add the two-stage gate)"]
        );
    }

    /// `dev` is regularly ahead of `origin/dev`, and every worktree branched
    /// from it carries those commits. Counting them would refuse every removal
    /// while saying nothing about the work done in this worktree.
    #[test]
    fn commits_the_branch_inherited_are_not_counted_as_its_own() {
        assert_eq!(
            unpushed_query("feat/53-two-stage-gate"),
            [
                "log",
                "--oneline",
                "HEAD",
                "--not",
                "--exclude=feat/53-two-stage-gate",
                "--branches",
                "--remotes",
            ]
        );
        // Nothing to exclude, and nothing to lose: a detached HEAD another ref
        // holds is reported by the same query.
        assert_eq!(
            unpushed_query(DETACHED),
            [
                "log",
                "--oneline",
                "HEAD",
                "--not",
                "--branches",
                "--remotes"
            ]
        );
    }

    #[test]
    fn uncommitted_changes_stop_a_removal_too() {
        let blockers = removal_blockers("", " M xtask/src/main.rs\n?? xtask/src/worktree.rs\n");

        assert_eq!(blockers, ["2 uncommitted change(s)"]);
    }

    #[test]
    fn a_pushed_and_clean_worktree_is_removed() {
        assert!(removal_blockers("", "").is_empty());
    }

    fn state(status: Option<&str>) -> IssueState {
        IssueState {
            number: 84,
            title: "claim".into(),
            body: "Affects: verification-harness".into(),
            labels: vec![],
            blockers: vec![],
            status: status.map(str::to_string),
            status_updated: Some("2026-09-22T15:13:23Z".into()),
            item: None,
        }
    }

    /// The board is what one agent leaves for the next, so `add` reads it
    /// before it makes a worktree nobody should be making.
    #[test]
    fn an_issue_someone_is_on_is_not_taken_twice() {
        let refusal = refusal(&state(Some(board::IN_PROGRESS))).expect("a refusal");

        assert!(refusal.contains("already `In progress`"), "{refusal}");
        assert!(refusal.contains("since 2026-09-22T15:13:23Z"), "{refusal}");
        assert!(refusal.contains("cargo xtask ready"), "{refusal}");
    }

    #[test]
    fn a_free_issue_is_not_refused() {
        assert_eq!(refusal(&state(Some(board::BACKLOG))), None);
        assert_eq!(refusal(&state(Some(board::READY))), None);
    }

    /// Each refusal says something different to do about it: implement a
    /// child, hand it to a person, close the blocker, fix the board.
    #[test]
    fn every_refusal_names_what_to_do_about_it() {
        let mut tracker = state(Some(board::BACKLOG));
        tracker.labels.push(board::TRACKER.into());
        assert!(refusal(&tracker).unwrap().contains("sub-issues"));

        let mut human = state(Some(board::BACKLOG));
        human.labels.push(board::NEEDS_HUMAN.into());
        assert!(
            refusal(&human)
                .unwrap()
                .contains("a person has to close it")
        );

        let mut blocked = state(Some(board::BACKLOG));
        blocked.blockers = vec![board::Blocker {
            number: 75,
            open: true,
        }];
        let message = refusal(&blocked).unwrap();
        assert!(message.contains("blocked by #75"), "{message}");

        assert!(
            refusal(&state(None))
                .unwrap()
                .contains("not an item of project")
        );

        let mut disagreeing = state(Some(board::READY));
        disagreeing.blockers = vec![board::Blocker {
            number: 75,
            open: true,
        }];
        let message = refusal(&disagreeing).unwrap();
        assert!(message.contains("says `Ready` on the board"), "{message}");
    }

    /// The installed name is the issue's, and the command is the one that
    /// keeps the signature.
    #[test]
    fn install_puts_the_worktree_build_under_the_issue_name() {
        assert_eq!(
            install_arguments(86),
            ["xtask", "install-signed", "--as", "kurama-86"]
        );
    }

    #[test]
    fn the_hint_names_the_config_and_the_binary_to_use() {
        let hint = real_environment_hint(Path::new("/w/kurama-86"), 86, true, true);
        assert!(
            hint.contains("export KURAMA_CONFIG_PATH=/w/kurama-86/.kurama/config.toml"),
            "{hint}"
        );
        assert!(hint.contains("kurama-86 status"), "{hint}");

        let bare = real_environment_hint(Path::new("/w/kurama-86"), 86, false, false);
        assert!(bare.contains("no daily configuration"), "{bare}");
        assert!(bare.contains("install-signed --as kurama-86"), "{bare}");
    }

    /// `remove` gives back every issue `add` took for the worktree.
    #[test]
    fn a_batch_worktree_is_removed_with_every_issue_it_was_added_for() {
        assert_eq!(batch_of(169, Some("169 170 171\n")), [169, 170, 171]);
        assert_eq!(batch_of(84, None), [84]);
        assert_eq!(batch_of(84, Some("")), [84]);
        assert_eq!(
            issues_key("feat/169-tests"),
            "branch.feat/169-tests.kurama-issues"
        );
    }

    /// A branch that never moved is contained in the base too, and that is
    /// an abandoned worktree, not a merged one.
    #[test]
    fn only_a_branch_that_moved_and_reached_the_base_is_merged() {
        assert!(was_merged("aaa", "bbb", true));
        assert!(!was_merged("aaa", "aaa", true), "an untouched branch");
        assert!(!was_merged("aaa", "bbb", false), "work not in dev yet");
    }
}
