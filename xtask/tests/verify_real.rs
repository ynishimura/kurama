//! `cargo xtask verify-real` run as the binary over a scratch repository whose
//! one feature has a probe: a shell command that prints what libtest prints.
//! The evidence it writes, `--check` holding it to the files, and a probe that
//! cannot run being recorded as unverified.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod support;

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Scratch {
    fn repo(&self) -> PathBuf {
        self.0.join("repo")
    }

    fn run(&self, args: &[&str], envs: &[(&str, &str)]) -> Output {
        let mut command = support::xtask(&self.0);
        command
            .arg("verify-real")
            .args(args)
            .env("KURAMA_XTASK_ROOT", self.repo())
            .env_remove("KURAMA_TEST_PROBE");
        for (key, value) in envs {
            command.env(key, value);
        }
        command.output().unwrap()
    }

    fn evidence(&self) -> serde_json::Value {
        let text = std::fs::read_to_string(self.repo().join(".agent/real/probed.json")).unwrap();
        serde_json::from_str(&text).unwrap()
    }
}

/// A repository with one feature, `probed`, whose probe prints `result` for
/// its one contract.
fn scratch(name: &str, result: &str) -> Scratch {
    let dir = std::env::temp_dir().join(format!("xtask-real-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let repo = dir.join("repo");
    std::fs::create_dir_all(repo.join(".agent/features")).unwrap();
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::create_dir_all(repo.join("tests")).unwrap();
    std::fs::write(repo.join("src/probed.rs"), "//! Probed.\n").unwrap();
    std::fs::write(repo.join("tests/seed.sql"), "CREATE TABLE t (a int);\n").unwrap();
    std::fs::write(
        repo.join("tests/probe.rs"),
        "#[test]\nfn real_contract() {}\n",
    )
    .unwrap();
    std::fs::write(
        repo.join(".agent/features/probed.toml"),
        format!(
            r#"[probed]
summary = "s"
entry = "src/probed.rs"
files = ["src/probed.rs"]
scenarios = ["probed_mock"]

[probed.real]
command = ["sh", "-c", "printf 'running 1 test\ntest real_contract ... {result}\n'"]
source = "tests/probe.rs"
environment = "scratch"
access = "read-only"
requires = ["KURAMA_TEST_PROBE"]
fixtures = ["tests/seed.sql"]

[[probed.real.contracts]]
real = "real_contract"
mock = ["probed_mock"]
"#
        ),
    )
    .unwrap();
    let git = |args: &[&str]| {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(&repo)
                .output()
                .unwrap()
                .status
                .success()
        );
    };
    git(&["init", "-q", "-b", "dev"]);
    git(&["add", "."]);
    git(&[
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@example.com",
        "commit",
        "-qm",
        "init",
    ]);
    Scratch(dir)
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn verify_real_records_a_verified_run_and_check_holds_it_to_the_files() {
    let dir = scratch("verified", "ok");
    let run = dir.run(&["probed"], &[("KURAMA_TEST_PROBE", "1")]);
    assert!(run.status.success(), "{}", stderr(&run));
    let evidence = dir.evidence();
    assert_eq!(evidence["status"], "verified");
    assert_eq!(evidence["environment"], "scratch");
    assert_eq!(evidence["observed"][0]["test"], "real_contract");
    assert_eq!(evidence["observed"][0]["result"], "passed");
    assert_eq!(evidence["observed"][0]["mock"][0], "probed_mock");
    assert!(
        dir.repo()
            .join("target/agent/real/probed.fixture-candidate.json")
            .exists()
    );

    let seed = evidence["fixtures"]["tests/seed.sql"].as_str().unwrap();
    assert!(
        seed.len() == 16 && seed.chars().all(|c| c.is_ascii_hexdigit()),
        "{seed}"
    );

    let check = dir.run(&["--check"], &[]);
    assert!(check.status.success(), "{}", stderr(&check));

    std::fs::write(
        dir.repo().join("tests/seed.sql"),
        "CREATE TABLE t (b int);\n",
    )
    .unwrap();
    let reseeded = dir.run(&["--check", "probed"], &[]);
    assert!(
        stderr(&reseeded).contains("probed: a fixture changed since the evidence was taken"),
        "{}",
        stderr(&reseeded)
    );
    std::fs::write(
        dir.repo().join("tests/seed.sql"),
        "CREATE TABLE t (a int);\n",
    )
    .unwrap();

    std::fs::write(dir.repo().join("src/probed.rs"), "//! Probed, changed.\n").unwrap();
    let stale = dir.run(&["--check", "probed"], &[]);
    assert!(!stale.status.success());
    assert!(
        stderr(&stale).contains("probed: the evidence was taken on other files than these"),
        "{}",
        stderr(&stale)
    );
}

#[test]
fn verify_real_without_its_variable_is_unverified_and_check_refuses_it() {
    let dir = scratch("unset", "ok");
    let run = dir.run(&["probed"], &[]);
    assert!(!run.status.success());
    let evidence = dir.evidence();
    assert_eq!(evidence["status"], "unverified");
    assert_eq!(
        evidence["reason"],
        "not run: set KURAMA_TEST_PROBE (the environment is scratch)"
    );
    let check = dir.run(&["--check"], &[]);
    assert!(!check.status.success());
    assert!(stderr(&check).contains("the last real run is Unverified"));
}

#[test]
fn verify_real_records_a_failed_contract_as_failed() {
    let dir = scratch("failed", "FAILED");
    let run = dir.run(&["probed"], &[("KURAMA_TEST_PROBE", "1")]);
    assert!(!run.status.success());
    assert_eq!(dir.evidence()["status"], "failed");
    assert_eq!(dir.evidence()["observed"][0]["result"], "failed");
}

#[test]
fn verify_real_prints_its_usage_and_refuses_an_unknown_feature() {
    let dir = scratch("usage", "ok");
    let help = dir.run(&["--help"], &[]);
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).starts_with("usage: cargo xtask verify-real"));
    let flag = dir.run(&["--json"], &[]);
    assert!(
        stderr(&flag).starts_with("error: usage: cargo xtask verify-real"),
        "{}",
        stderr(&flag)
    );
    let unknown = dir.run(&["nothing"], &[]);
    assert!(!unknown.status.success());
    assert!(stderr(&unknown).contains("no feature named `nothing`"));
    assert!(!Path::new(&dir.repo().join(".agent/real")).exists());
}
