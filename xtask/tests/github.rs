//! The xtask paths that read and write GitHub, run as the binary against
//! `tests/fakes/gh` and a scratch git repository.
//!
//! `worktree add` refuses what the board says is not free and claims what it
//! takes; `remove` gives back only what it claimed. Agents run on those
//! answers, and the unit tests only judged them: nothing started `gh`. The fake
//! answers from the board each test writes, and logs every call and every
//! status it was asked to write, so a test says what was changed and that
//! nothing else was.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};

mod support;

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Sandbox {
    dir: PathBuf,
    repo: PathBuf,
    board: PathBuf,
    log: PathBuf,
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// A repository with one commit on `main`, and a board holding `issues`.
fn sandbox(issues: Value) -> Sandbox {
    let dir = std::env::temp_dir().join(format!(
        "xtask-github-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("README.md"), "scratch\n").unwrap();
    // As in this repository: the configuration a worktree gets is ignored.
    std::fs::write(repo.join(".gitignore"), "/.kurama/\n/target/\n").unwrap();
    git(&repo, &["add", "README.md", ".gitignore"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-q",
            "-m",
            "start",
        ],
    );
    let board = dir.join("board.json");
    std::fs::write(&board, issues.to_string()).unwrap();
    Sandbox {
        log: dir.join("gh.log"),
        dir,
        repo,
        board,
    }
}

fn issue(number: u64, status: Option<&str>, labels: &[&str], blockers: &[(u64, bool)]) -> Value {
    json!({
        "number": number,
        "title": format!("issue {number}"),
        "body": "Affects: verification-harness",
        "labels": labels,
        "blockers": blockers.iter().map(|(n, open)| json!([n, open])).collect::<Vec<_>>(),
        "status": status,
    })
}

impl Sandbox {
    fn run(&self, args: &[&str], mode: &str) -> Output {
        support::xtask(&self.dir)
            .args(args)
            .env("KURAMA_XTASK_ROOT", &self.repo)
            .env("KURAMA_FAKE_GH_BOARD", &self.board)
            .env("KURAMA_FAKE_GH_LOG", &self.log)
            .env("KURAMA_FAKE_GH_MODE", mode)
            .output()
            .unwrap()
    }

    /// `(issue, status)` for every status the fake was asked to write.
    fn writes(&self) -> Vec<(u64, String)> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.strip_prefix("WRITE "))
            .map(|rest| {
                let (number, status) = rest.split_once(' ').unwrap();
                (number.parse().unwrap(), status.to_string())
            })
            .collect()
    }

    /// The scratch repository has no feature map; judge against this one.
    fn copy_feature_map(&self) {
        let features = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.agent/features");
        let target = self.repo.join(".agent/features");
        std::fs::create_dir_all(&target).unwrap();
        for entry in std::fs::read_dir(features).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
        }
    }

    /// How many times the board was read.
    fn board_reads(&self) -> usize {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .filter(|line| line.starts_with('[') && line.contains("query=query"))
            .count()
    }

    fn worktree(&self, issue: u64) -> PathBuf {
        self.dir.join(format!("repo-{issue}"))
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn github_worktree_add_claims_a_free_issue_and_writes_in_progress() {
    let sandbox = sandbox(json!([
        issue(84, Some("Backlog"), &[], &[]),
        issue(85, Some("Backlog"), &[], &[])
    ]));

    let output = sandbox.run(&["worktree", "add", "84", "claim"], "");

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(sandbox.worktree(84).join("README.md").is_file());
    assert!(git(&sandbox.repo, &["branch", "--list", "feat/84-claim"]).contains("feat/84-claim"));
    // The issue it was asked for, to the status it takes, and nothing else.
    assert_eq!(sandbox.writes(), [(84, "In progress".to_string())]);
}

#[test]
fn github_worktree_add_refuses_what_the_board_says_is_not_free() {
    for (state, expected) in [
        (
            issue(84, Some("In progress"), &[], &[]),
            "already `In progress`",
        ),
        (
            issue(84, Some("Backlog"), &["tracker"], &[]),
            "is a tracker",
        ),
        (
            issue(84, Some("Backlog"), &["needs-human"], &[]),
            "a person has to close it",
        ),
        (
            issue(84, Some("Backlog"), &[], &[(75, true)]),
            "blocked by #75",
        ),
        (issue(84, None, &[], &[]), "not an item of project"),
    ] {
        // Another issue on the board: a board where nothing is an item is a
        // wrong project number, which is refused before any issue is judged.
        let sandbox = sandbox(json!([state, issue(85, Some("Backlog"), &[], &[])]));

        let output = sandbox.run(&["worktree", "add", "84"], "");

        assert!(!output.status.success(), "{expected}: it was not refused");
        assert!(
            stderr(&output).contains(expected),
            "{expected}: {}",
            stderr(&output)
        );
        assert!(
            !sandbox.worktree(84).exists(),
            "{expected}: a worktree was made"
        );
        assert!(
            sandbox.writes().is_empty(),
            "{expected}: {:?}",
            sandbox.writes()
        );
    }
}

/// A closed blocker is not a reason to refuse.
#[test]
fn github_worktree_add_takes_an_issue_whose_blocker_is_closed() {
    let sandbox = sandbox(json!([issue(84, Some("Backlog"), &[], &[(75, false)])]));

    let output = sandbox.run(&["worktree", "add", "84"], "");

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(sandbox.writes(), [(84, "In progress".to_string())]);
}

#[test]
fn github_worktree_remove_gives_back_only_an_in_progress_issue() {
    for (status, expected) in [
        ("In progress", vec![(84, "Backlog".to_string())]),
        ("In review", vec![]),
        ("Done", vec![]),
    ] {
        let sandbox = sandbox(json!([issue(84, Some("Backlog"), &[], &[])]));
        let added = sandbox.run(&["worktree", "add", "84"], "");
        assert!(added.status.success(), "{}", stderr(&added));
        // What the board says by the time the worktree is removed.
        std::fs::write(
            &sandbox.board,
            json!([issue(84, Some(status), &[], &[])]).to_string(),
        )
        .unwrap();
        std::fs::remove_file(&sandbox.log).unwrap();

        let output = sandbox.run(&["worktree", "remove", "84"], "");

        assert!(output.status.success(), "{status}: {}", stderr(&output));
        assert!(
            !sandbox.worktree(84).exists(),
            "{status}: the worktree is still there"
        );
        assert_eq!(sandbox.writes(), expected, "{status}");
    }
}

#[test]
fn github_a_token_without_the_project_scope_is_named_and_nothing_is_made() {
    let sandbox = sandbox(json!([issue(84, Some("Backlog"), &[], &[])]));

    let output = sandbox.run(&["worktree", "add", "84"], "no-project-scope");

    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("gh auth refresh -h github.com -s project"),
        "{}",
        stderr(&output)
    );
    assert!(!sandbox.worktree(84).exists());
    assert!(sandbox.writes().is_empty());
}

#[test]
fn github_worktree_add_refuses_when_github_cannot_be_reached() {
    let sandbox = sandbox(json!([issue(84, Some("Backlog"), &[], &[])]));

    let output = sandbox.run(&["worktree", "add", "84"], "unreachable");

    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("git worktree add"),
        "{}",
        stderr(&output)
    );
    assert!(!sandbox.worktree(84).exists());
    assert!(git(&sandbox.repo, &["branch", "--list", "feat/84"]).is_empty());
}

/// `conflicts`, `scenarios-check` and `issue-check` read an issue body
/// through `gh issue view`, and what they judge is that body.
#[test]
fn github_a_posted_body_is_what_the_commands_judge() {
    let sandbox = sandbox(json!([
        issue(84, Some("Backlog"), &[], &[]),
        {
            "number": 85, "title": "t", "labels": [], "blockers": [], "status": "Backlog",
            "body": "Affects: nothing-like-a-feature\n"
        }
    ]));
    sandbox.copy_feature_map();

    let good = sandbox.run(&["issue-check", "84"], "");
    let bad = sandbox.run(&["issue-check", "85"], "");
    let missing = sandbox.run(&["conflicts", "84", "99"], "");

    assert!(good.status.success(), "{}", stderr(&good));
    assert!(
        String::from_utf8_lossy(&good.stdout).contains("verification-harness"),
        "{}",
        String::from_utf8_lossy(&good.stdout)
    );
    assert!(!bad.status.success());
    assert!(
        String::from_utf8_lossy(&bad.stdout).contains("nothing-like-a-feature"),
        "{}",
        String::from_utf8_lossy(&bad.stdout)
    );
    assert!(!missing.status.success());
    assert!(
        stderr(&missing).contains("gh issue view 99 failed"),
        "{}",
        stderr(&missing)
    );
}

/// A body `gh issue view` answers with, for an issue on the board.
fn posted(number: u64, body: &str) -> Value {
    let mut state = issue(number, Some("Backlog"), &[], &[]);
    state["body"] = json!(body);
    state
}

/// The issue numbers `gh issue view` was asked for, in order.
fn viewed_issues(sandbox: &Sandbox) -> Vec<u64> {
    std::fs::read_to_string(&sandbox.log)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Vec<String>>(line).ok())
        .filter(|args| args[..2] == ["issue", "view"])
        .map(|args| args[2].parse().unwrap())
        .collect()
}

/// `scenarios-check` judges the checklist of the body `gh` answers against
/// the scenarios the test binary lists, which a fake `cargo` stands in for.
#[test]
fn github_scenarios_check_judges_the_posted_checklist() {
    let sandbox = sandbox(json!([
        posted(
            84,
            "Affects: oauth\n\n## Scenarios\n\n- [ ] oauth_login_works\n"
        ),
        posted(
            85,
            "Affects: oauth\n\n## Scenarios\n\n- [ ] oauth_logout_works\n"
        )
    ]));
    sandbox.copy_feature_map();
    let bin = sandbox.dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let cargo = bin.join("cargo");
    std::fs::write(
        &cargo,
        "#!/bin/sh\nprintf 'oauth::oauth_login_works: test\\n'\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755)).unwrap();
    let run = |issue: &str| {
        support::xtask(&sandbox.dir)
            .args(["scenarios-check", issue])
            .env("KURAMA_XTASK_ROOT", &sandbox.repo)
            .env("KURAMA_FAKE_GH_BOARD", &sandbox.board)
            .env("KURAMA_FAKE_GH_LOG", &sandbox.log)
            .env(
                "PATH",
                format!(
                    "{}:{}:{}",
                    bin.display(),
                    support::fakes().display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .output()
            .unwrap()
    };

    let present = run("84");
    let missing = run("85");

    assert!(present.status.success(), "{}", stderr(&present));
    assert!(
        String::from_utf8_lossy(&present.stdout).contains("#84  satisfied"),
        "{}",
        String::from_utf8_lossy(&present.stdout)
    );
    assert!(!missing.status.success());
    let text = String::from_utf8_lossy(&missing.stdout);
    assert!(text.contains("#85  incomplete"), "{text}");
    assert!(
        text.contains("missing (declared") && text.contains("oauth_logout_works"),
        "{text}"
    );
    assert!(
        stderr(&missing).contains("issue #85 declares 1 scenario(s) that do not exist"),
        "{}",
        stderr(&missing)
    );
    assert_eq!(viewed_issues(&sandbox), [84, 85]);
}

/// `conflicts` judges the `Affects:` lines of the bodies `gh` answers with:
/// two issues of one feature are serial, two of unrelated features parallel.
#[test]
fn github_conflicts_judges_the_posted_affects_lines() {
    let sandbox = sandbox(json!([
        posted(84, "Affects: oauth\n"),
        posted(85, "Affects: oauth\n"),
        posted(86, "Affects: local-analytics\n")
    ]));
    sandbox.copy_feature_map();

    let output = sandbox.run(&["conflicts", "84", "85", "86", "--json"], "");

    assert!(output.status.success(), "{}", stderr(&output));
    let answer: Value = serde_json::from_slice(&output.stdout).unwrap();
    let verdicts: Vec<(u64, u64, &str)> = answer["pairs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|pair| {
            (
                pair["a"].as_u64().unwrap(),
                pair["b"].as_u64().unwrap(),
                pair["verdict"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        verdicts,
        [
            (84, 85, "serial"),
            (84, 86, "parallel"),
            (85, 86, "parallel")
        ]
    );
    assert_eq!(viewed_issues(&sandbox), [84, 85, 86]);

    let text = sandbox.run(&["conflicts", "84", "85"], "");
    assert!(text.status.success(), "{}", stderr(&text));
    assert!(
        String::from_utf8_lossy(&text.stdout).contains("#84 + #85  serial"),
        "{}",
        String::from_utf8_lossy(&text.stdout)
    );
}

/// Each worktree reads its own copy of the daily configuration, so a key one
/// branch adds cannot make another branch's binary refuse the file; removing
/// the worktree removes the copy and the `kurama-<ISSUE>` it installed.
#[test]
fn github_a_worktree_gets_its_own_configuration_and_remove_takes_its_binary() {
    let sandbox = sandbox(json!([
        issue(84, Some("Backlog"), &[], &[]),
        issue(85, Some("Backlog"), &[], &[])
    ]));
    let daily = sandbox.dir.join("home/.config/kurama/config.toml");
    std::fs::create_dir_all(daily.parent().unwrap()).unwrap();
    std::fs::write(&daily, "[aws]\nregion = \"ap-northeast-1\"\n").unwrap();
    let bin = sandbox.dir.join("cargo/bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("kurama"), "daily").unwrap();

    for number in ["84", "85"] {
        let output = sandbox.run(&["worktree", "add", number], "");
        assert!(output.status.success(), "{}", stderr(&output));
        // git answers /private/var for the /var the temp directory names.
        let hint = stderr(&output);
        let line = hint
            .lines()
            .find(|line| line.contains("export KURAMA_CONFIG_PATH="))
            .unwrap_or_else(|| panic!("no export line: {hint}"));
        assert!(
            line.ends_with(&format!("/repo-{number}/.kurama/config.toml")),
            "{line}"
        );
    }
    let own = sandbox.worktree(84).join(".kurama/config.toml");
    let other = sandbox.worktree(85).join(".kurama/config.toml");
    assert_eq!(
        std::fs::read_to_string(&own).unwrap(),
        std::fs::read_to_string(&daily).unwrap()
    );
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&own).unwrap().permissions().mode() & 0o777,
        0o600
    );
    // A section #84's branch adds reaches neither #85 nor the daily file.
    std::fs::write(&own, "[aws]\nregion = \"ap-northeast-1\"\n[new_section]\n").unwrap();
    assert!(
        !std::fs::read_to_string(&other)
            .unwrap()
            .contains("new_section")
    );
    assert!(
        !std::fs::read_to_string(&daily)
            .unwrap()
            .contains("new_section")
    );

    // What `add --install` would have left.
    std::fs::write(bin.join("kurama-84"), "branch build").unwrap();
    let output = sandbox.run(&["worktree", "remove", "84"], "");

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!own.exists());
    assert!(!bin.join("kurama-84").exists());
    assert_eq!(
        std::fs::read_to_string(bin.join("kurama")).unwrap(),
        "daily"
    );
    assert!(other.exists());
}

/// How many mutation requests the fake answered.
fn mutation_requests(sandbox: &Sandbox) -> usize {
    std::fs::read_to_string(&sandbox.log)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.starts_with('[') && line.contains("query=mutation"))
        .count()
}

#[test]
fn github_claim_takes_an_issue_without_a_worktree_and_release_gives_it_back() {
    let sandbox = sandbox(json!([
        issue(84, Some("Backlog"), &[], &[]),
        issue(85, Some("In progress"), &[], &[]),
        issue(86, Some("Backlog"), &[], &[])
    ]));

    let claimed = sandbox.run(&["claim", "84"], "");
    let taken = sandbox.run(&["claim", "85"], "");

    assert!(claimed.status.success(), "{}", stderr(&claimed));
    assert!(!sandbox.worktree(84).exists());
    assert!(!taken.status.success());
    assert!(
        stderr(&taken).contains("already `In progress`"),
        "{}",
        stderr(&taken)
    );
    assert_eq!(sandbox.writes(), [(84, "In progress".to_string())]);

    std::fs::remove_file(&sandbox.log).unwrap();
    let released = sandbox.run(&["claim", "--release", "85"], "");
    let not_taken = sandbox.run(&["claim", "--release", "86"], "");

    assert!(released.status.success(), "{}", stderr(&released));
    assert!(not_taken.status.success(), "{}", stderr(&not_taken));
    assert_eq!(sandbox.writes(), [(85, "Backlog".to_string())]);
}

/// The fake writes what a mutation set back to the board, so the next run
/// reads it: the same issue cannot be claimed twice, and can be again once
/// it is released.
#[test]
fn github_claim_and_release_update_the_fake_board_state() {
    let sandbox = sandbox(json!([issue(84, Some("Backlog"), &[], &[])]));

    let claimed = sandbox.run(&["claim", "84"], "");
    let again = sandbox.run(&["claim", "84"], "");

    assert!(claimed.status.success(), "{}", stderr(&claimed));
    assert!(!again.status.success(), "a claimed issue was claimed again");
    assert!(
        stderr(&again).contains("already `In progress`"),
        "{}",
        stderr(&again)
    );

    let released = sandbox.run(&["claim", "--release", "84"], "");
    let reclaimed = sandbox.run(&["claim", "84"], "");

    assert!(released.status.success(), "{}", stderr(&released));
    assert!(reclaimed.status.success(), "{}", stderr(&reclaimed));
    assert_eq!(
        sandbox.writes(),
        [
            (84, "In progress".to_string()),
            (84, "Backlog".to_string()),
            (84, "In progress".to_string())
        ]
    );
}

#[test]
fn github_board_set_status_writes_every_issue_in_one_request() {
    let sandbox = sandbox(json!([
        issue(84, Some("Backlog"), &[], &[]),
        issue(85, Some("In progress"), &[], &[]),
        issue(86, None, &[], &[])
    ]));

    let output = sandbox.run(&["board", "set-status", "Ready", "84", "85"], "");

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        sandbox.writes(),
        [(84, "Ready".to_string()), (85, "Ready".to_string())]
    );
    assert_eq!(mutation_requests(&sandbox), 1);

    // One that cannot be written is named, and the others are still written.
    std::fs::remove_file(&sandbox.log).unwrap();
    let partial = sandbox.run(&["board", "set-status", "Done", "84", "86", "99"], "");

    assert!(!partial.status.success());
    assert!(stderr(&partial).contains("#86"), "{}", stderr(&partial));
    assert!(
        stderr(&partial).contains("#99: not an open issue"),
        "{}",
        stderr(&partial)
    );
    assert!(
        stderr(&partial).contains("1 of 3 written"),
        "{}",
        stderr(&partial)
    );
    assert_eq!(sandbox.writes(), [(84, "Done".to_string())]);
}

/// A finished branch merged into main is not work to give back.
#[test]
fn github_worktree_remove_leaves_the_board_alone_for_a_merged_branch() {
    let sandbox = sandbox(json!([issue(84, Some("Backlog"), &[], &[])]));
    let added = sandbox.run(&["worktree", "add", "84"], "");
    assert!(added.status.success(), "{}", stderr(&added));
    let worktree = sandbox.worktree(84);
    std::fs::write(worktree.join("work.txt"), "done\n").unwrap();
    git(&worktree, &["add", "work.txt"]);
    git(
        &worktree,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-q",
            "-m",
            "work",
        ],
    );
    git(&sandbox.repo, &["merge", "-q", "--ff-only", "feat/84"]);
    std::fs::write(
        &sandbox.board,
        json!([issue(84, Some("In progress"), &[], &[])]).to_string(),
    )
    .unwrap();
    std::fs::remove_file(&sandbox.log).unwrap();

    let output = sandbox.run(&["worktree", "remove", "84"], "");

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("is merged into main"),
        "{}",
        stderr(&output)
    );
    assert!(sandbox.writes().is_empty(), "{:?}", sandbox.writes());
}

/// A commit no remote and no other branch has stops `remove`: the worktree,
/// its branch and its `target/` stay, and the board is not written.
#[test]
fn github_worktree_remove_preserves_unpushed_work_and_board_status() {
    let sandbox = sandbox(json!([issue(84, Some("Backlog"), &[], &[])]));
    let added = sandbox.run(&["worktree", "add", "84"], "");
    assert!(added.status.success(), "{}", stderr(&added));
    let worktree = sandbox.worktree(84);
    std::fs::create_dir_all(worktree.join("target/debug")).unwrap();
    std::fs::write(worktree.join("target/debug/built"), "built").unwrap();
    std::fs::write(worktree.join("work.txt"), "unpushed\n").unwrap();
    git(&worktree, &["add", "work.txt"]);
    git(
        &worktree,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-q",
            "-m",
            "unpushed work",
        ],
    );
    std::fs::remove_file(&sandbox.log).unwrap();

    let output = sandbox.run(&["worktree", "remove", "84"], "");

    assert!(
        !output.status.success(),
        "a worktree with unpushed work was removed"
    );
    assert!(
        stderr(&output).contains("still holds work: 1 commit(s)")
            && stderr(&output).contains("unpushed work"),
        "{}",
        stderr(&output)
    );
    assert!(worktree.join("work.txt").is_file());
    assert!(worktree.join("target/debug/built").is_file());
    assert!(git(&sandbox.repo, &["branch", "--list", "feat/84"]).contains("feat/84"));
    assert!(sandbox.writes().is_empty(), "{:?}", sandbox.writes());
    assert_eq!(mutation_requests(&sandbox), 0);
}

/// `count` open issues touching `feature`, numbered from 1: more than one
/// page of the board.
fn many(count: u64, feature: &str) -> Vec<Value> {
    (1..=count)
        .map(|number| {
            let mut state = issue(number, Some("Backlog"), &[], &[]);
            state["body"] = json!(format!("Affects: {feature}"));
            state
        })
        .collect()
}

fn ready_json(sandbox: &Sandbox) -> Value {
    let output = sandbox.run(&["ready", "--json"], "");
    assert!(output.status.success(), "{}", stderr(&output));
    serde_json::from_slice(&output.stdout).unwrap()
}

fn numbers(list: &Value) -> Vec<u64> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["number"].as_u64().unwrap())
        .collect()
}

/// An issue past the first page of open issues is still an open issue:
/// `ready` offers it and `worktree add` takes it.
#[test]
fn github_an_issue_past_the_first_page_is_read() {
    let mut board = many(100, "oauth");
    let mut late = issue(101, Some("Backlog"), &[], &[]);
    late["body"] = json!("Affects: tui");
    board.push(late);
    let sandbox = sandbox(Value::Array(board));
    sandbox.copy_feature_map();

    let answer = ready_json(&sandbox);

    assert!(
        numbers(&answer["ready"]).contains(&101),
        "{}",
        answer["ready"]
    );
    assert_eq!(sandbox.board_reads(), 2);

    let output = sandbox.run(&["worktree", "add", "101"], "");

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(sandbox.writes(), [(101, "In progress".to_string())]);
}

/// A running branch past the first page holds its feature back like any
/// other.
#[test]
fn github_an_in_progress_issue_past_the_first_page_holds_its_feature() {
    let mut board = many(100, "oauth");
    board[0]["body"] = json!("Affects: local-analytics");
    let mut running = issue(101, Some("In progress"), &[], &[]);
    running["body"] = json!("Affects: local-analytics");
    board.push(running);
    let sandbox = sandbox(Value::Array(board));
    sandbox.copy_feature_map();

    let answer = ready_json(&sandbox);

    assert_eq!(numbers(&answer["running"]), [101]);
    assert_eq!(numbers(&answer["held"]), [1]);
    assert_eq!(answer["held"][0]["behind"], json!([101]));
}

/// A board that fits one page is read with one request, as before.
#[test]
fn github_a_board_of_one_page_is_read_once() {
    let sandbox = sandbox(Value::Array(many(100, "oauth")));
    sandbox.copy_feature_map();

    let answer = ready_json(&sandbox);

    assert_eq!(numbers(&answer["ready"]).len(), 100);
    assert_eq!(sandbox.board_reads(), 1);
}

/// A worktree starts from a clone of the main checkout's `target/`, so its
/// first build reuses what the main checkout compiled; without one it says so
/// and goes on as before.
#[test]
fn github_worktree_add_clones_the_main_target_directory() {
    let sandbox = sandbox(json!([
        issue(84, Some("Backlog"), &[], &[]),
        issue(85, Some("Backlog"), &[], &[]),
        issue(86, Some("Backlog"), &[], &[])
    ]));

    let bare = sandbox.run(&["worktree", "add", "85"], "");
    assert!(bare.status.success(), "{}", stderr(&bare));
    assert!(
        stderr(&bare).contains("has no target/ to clone"),
        "{}",
        stderr(&bare)
    );
    assert!(!sandbox.worktree(85).join("target").exists());

    let built = sandbox.repo.join("target/debug/build/libduckdb-sys-1/out");
    std::fs::create_dir_all(&built).unwrap();
    std::fs::write(built.join("libduckdb.a"), "compiled once").unwrap();
    // The reports are the main checkout's own: a mutation report there is not
    // a result of the worktree's branch.
    let reports = sandbox.repo.join("target/agent/xtask/mutants.out");
    std::fs::create_dir_all(&reports).unwrap();
    std::fs::write(reports.join("missed.txt"), "xtask/src/impact.rs:1:1: m\n").unwrap();

    let output = sandbox.run(&["worktree", "add", "84"], "");

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("target/ cloned from"),
        "{}",
        stderr(&output)
    );
    // A clone that fails is said and skipped; the worktree is still made.
    let unreadable = sandbox.repo.join("target/debug/unreadable");
    std::fs::write(&unreadable, "").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000)).unwrap();
    let failed = sandbox.run(&["worktree", "add", "86"], "");
    std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(failed.status.success(), "{}", stderr(&failed));
    assert!(
        stderr(&failed).contains("target/ was not cloned")
            && stderr(&failed).contains("Permission denied"),
        "{}",
        stderr(&failed)
    );

    let cloned = sandbox
        .worktree(84)
        .join("target/debug/build/libduckdb-sys-1/out/libduckdb.a");
    assert_eq!(std::fs::read_to_string(&cloned).unwrap(), "compiled once");
    assert!(!sandbox.worktree(84).join("target/agent").exists());
    assert!(reports.join("missed.txt").exists());
    // A copy, not a link: a build in the worktree leaves the main one alone.
    std::fs::write(&cloned, "rebuilt in the worktree").unwrap();
    assert_eq!(
        std::fs::read_to_string(built.join("libduckdb.a")).unwrap(),
        "compiled once"
    );
}

/// Several issues of one feature share one worktree: named after the first,
/// every one of them claimed in one request.
#[test]
fn github_worktree_add_takes_several_issues_into_one_worktree() {
    let sandbox = sandbox(json!([
        issue(169, Some("Backlog"), &[], &[]),
        issue(170, Some("Ready"), &[], &[]),
        issue(171, Some("Backlog"), &[], &[]),
        issue(172, Some("Backlog"), &[], &[])
    ]));

    let output = sandbox.run(&["worktree", "add", "169", "170", "171", "tests"], "");

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(sandbox.worktree(169).is_dir());
    assert!(!sandbox.worktree(170).exists());
    assert!(git(&sandbox.repo, &["branch", "--list", "feat/169-tests"]).contains("feat/169-tests"));
    assert_eq!(
        sandbox.writes(),
        [
            (169, "In progress".to_string()),
            (170, "In progress".to_string()),
            (171, "In progress".to_string())
        ]
    );
    assert_eq!(mutation_requests(&sandbox), 1);

    // `remove` by the first number gives every one of them back.
    std::fs::write(
        &sandbox.board,
        json!([
            issue(169, Some("In progress"), &[], &[]),
            issue(170, Some("In progress"), &[], &[]),
            issue(171, Some("In review"), &[], &[]),
            issue(172, Some("In progress"), &[], &[])
        ])
        .to_string(),
    )
    .unwrap();
    std::fs::remove_file(&sandbox.log).unwrap();

    let removed = sandbox.run(&["worktree", "remove", "169"], "");

    assert!(removed.status.success(), "{}", stderr(&removed));
    assert!(!sandbox.worktree(169).exists());
    assert_eq!(
        sandbox.writes(),
        [(169, "Backlog".to_string()), (170, "Backlog".to_string())]
    );
}

/// One refused issue refuses the batch: nothing is made, nothing is claimed,
/// and each refusal says which issue and why.
#[test]
fn github_worktree_add_of_a_batch_with_one_refused_issue_makes_nothing() {
    let sandbox = sandbox(json!([
        issue(169, Some("Backlog"), &[], &[]),
        issue(170, Some("In progress"), &[], &[]),
        issue(171, Some("Backlog"), &[], &[(75, true)])
    ]));

    let output = sandbox.run(&["worktree", "add", "169", "170", "171", "172"], "");

    assert!(!output.status.success());
    let text = stderr(&output);
    assert!(text.contains("#170 is already `In progress`"), "{text}");
    assert!(text.contains("#171 is blocked by #75"), "{text}");
    assert!(text.contains("#172 is not open"), "{text}");
    assert!(!text.contains("#169"), "{text}");
    assert!(!sandbox.worktree(169).exists());
    assert!(git(&sandbox.repo, &["branch", "--list", "feat/169"]).is_empty());
    assert!(sandbox.writes().is_empty(), "{:?}", sandbox.writes());
}

/// `ready` names the offers one worktree can take together, and only those
/// closed over one feature.
#[test]
fn github_ready_lists_batches_of_one_feature() {
    let mut board = many(3, "xtask");
    board[1]["body"] = json!("Affects: xtask, oauth");
    let mut other = issue(4, Some("Backlog"), &[], &[]);
    other["body"] = json!("Affects: xtask");
    board.push(other);
    let sandbox = sandbox(Value::Array(board));
    sandbox.copy_feature_map();

    let answer = ready_json(&sandbox);

    assert_eq!(
        answer["batches"],
        json!([{"feature": "xtask", "issues": [1, 3, 4]}])
    );
    let text = sandbox.run(&["ready"], "");
    assert!(
        String::from_utf8_lossy(&text.stdout).contains("cargo xtask worktree add 1 3 4"),
        "{}",
        String::from_utf8_lossy(&text.stdout)
    );
}
