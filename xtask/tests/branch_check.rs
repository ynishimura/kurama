//! `cargo xtask branch-check` against a scratch repository: the slots that
//! bound how many run at once across the worktrees of one clone.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

mod support;

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Two branch-checks hold the slots; a third waits and says who holds them.
/// The holders are live processes, so the slots are not taken over as stale.
#[test]
fn branch_check_a_third_run_waits_for_a_slot_and_names_the_holders() {
    let dir = std::env::temp_dir().join(format!("xtask-slots-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "dev"]);
    let common = PathBuf::from(
        git(
            &repo,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .trim(),
    );
    let mut holders = Vec::new();
    for slot in 1..=2 {
        let holder = Command::new("sleep").arg("60").spawn().unwrap();
        std::fs::write(
            common.join(format!("kurama-xtask-branch-check.{slot}.lock")),
            format!("{} /w/kurama-{slot}", holder.id()),
        )
        .unwrap();
        holders.push(holder);
    }

    let mut third = support::xtask(&dir)
        .arg("branch-check")
        .env("KURAMA_XTASK_ROOT", &repo)
        .env("KURAMA_XTASK_GATE_SLOTS", "2")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Read on a thread with a deadline: a run that waits without saying so
    // would otherwise hang the test instead of failing it.
    let stderr = third.stderr.take().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let line = BufReader::new(stderr).lines().next();
        let _ = sender.send(line.and_then(Result::ok).unwrap_or_default());
    });
    let first_line = receiver
        .recv_timeout(Duration::from_secs(20))
        .unwrap_or_default();
    let still_waiting = third.try_wait().unwrap().is_none();
    let _ = third.kill();
    third.wait().unwrap();
    for mut holder in holders {
        holder.kill().unwrap();
        holder.wait().unwrap();
    }
    std::fs::remove_dir_all(&dir).unwrap();

    assert!(still_waiting, "the third run did not wait: {first_line}");
    assert!(
        first_line.contains("all 2 branch-check slots are taken"),
        "{first_line}"
    );
    assert!(first_line.contains("in /w/kurama-1"), "{first_line}");
    assert!(first_line.contains("in /w/kurama-2"), "{first_line}");
}
