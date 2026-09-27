//! `cargo xtask doctor` run as the binary over a scratch tree, with a fake
//! `cargo` and `rustc` first on PATH: every check reports `ok`, `FAIL` or
//! `warn`, and only a required check that fails makes the run fail.

use std::path::{Path, PathBuf};
use std::process::Output;

mod support;

/// What the fakes answer and the tree holds; `good()` passes every required
/// check, and each test case breaks one thing.
#[derive(Clone)]
struct Fixture {
    rustc: &'static str,
    fmt_installed: bool,
    feature_files: &'static str,
    declared_tests: &'static str,
    workspace_tests: &'static str,
    scenarios: &'static str,
    fake_open: bool,
}

fn good() -> Fixture {
    Fixture {
        rustc: "rustc 1.94.1 (e408947bf 2026-03-25)",
        fmt_installed: true,
        feature_files: r#"["src/demo.rs"]"#,
        declared_tests: "[]",
        workspace_tests: "demo::tests::it_runs: test\\nother::tests::it_runs: test\\n",
        scenarios: "demo::demo_works: test\\n",
        fake_open: true,
    }
}

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn executable(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    write(path, body);
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn scratch(name: &str, fixture: &Fixture) -> Scratch {
    let dir = std::env::temp_dir().join(format!("xtask-doctor-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let repo = dir.join("repo");
    write(
        &repo.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nrust-version = \"1.94.1\"\n",
    );
    write(
        &repo.join("src/demo.rs"),
        "#[cfg(test)]\nmod tests {\n    #[test]\n    fn it_runs() {}\n}\n",
    );
    write(
        &repo.join(".agent/features/demo.toml"),
        &format!(
            "[demo]\nsummary = \"demo\"\nentry = \"src/demo.rs\"\nfiles = {}\ntests = {}\nscenarios = [\"demo_works\"]\n",
            fixture.feature_files, fixture.declared_tests
        ),
    );
    executable(&repo.join("tests/fakes/op"), "#!/bin/sh\nexit 0\n");
    if fixture.fake_open {
        executable(&repo.join("tests/fakes/open"), "#!/bin/sh\nexit 0\n");
    }
    std::fs::create_dir_all(repo.join("tests/scenarios")).unwrap();
    let init = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&repo)
        .status()
        .unwrap();
    assert!(init.success());

    let bin = dir.join("bin");
    executable(
        &bin.join("rustc"),
        &format!("#!/bin/sh\necho '{}'\n", fixture.rustc),
    );
    executable(
        &bin.join("cargo"),
        &format!(
            "#!/bin/sh\ncase \"$*\" in\n  'fmt --version') exit {fmt} ;;\n  'clippy --version') exit 0 ;;\n  *'--test scenarios'*) printf '{scenarios}' ;;\n  *--workspace*) printf '{workspace}' ;;\n  *) exit 3 ;;\nesac\n",
            fmt = if fixture.fmt_installed { 0 } else { 1 },
            scenarios = fixture.scenarios,
            workspace = fixture.workspace_tests,
        ),
    );
    let scratch = Scratch(dir);
    // The committed file the first check compares with tests/cases/.
    let generated = run(&scratch, &["generate-cases"]);
    assert!(generated.status.success(), "{generated:?}");
    scratch
}

fn run(scratch: &Scratch, args: &[&str]) -> Output {
    support::xtask(&scratch.0)
        .args(args)
        .env("KURAMA_XTASK_ROOT", scratch.0.join("repo"))
        // Only the fakes and the system tools: no cargo plugin is installed.
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", scratch.0.join("bin").display()),
        )
        .output()
        .unwrap()
}

/// Each check's line, keyed by its name: `(label, detail)`.
fn lines(output: &Output) -> Vec<(String, String)> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (label, rest) = line.split_at_checked(4)?;
            let (name, detail) = rest.trim_start().split_once(": ")?;
            Some((format!("{} {name}", label.trim()), detail.to_string()))
        })
        .collect()
}

fn labels(output: &Output) -> Vec<String> {
    lines(output).into_iter().map(|(label, _)| label).collect()
}

const CHECKS: [&str; 14] = [
    "ok generated cases",
    "ok rust toolchain",
    "ok cargo-fmt",
    "ok cargo-clippy",
    "ok git",
    "ok .agent/features/ paths",
    "ok .agent/features/ test filters",
    "ok .agent/features/ declared filters",
    "ok .agent/features/ scenarios",
    "ok fake op",
    "ok fake open",
    "ok zsh",
    "warn cargo-machete",
    "warn cargo-sweep",
];

#[test]
fn doctor_reports_toolchain_feature_map_and_fake_checks() {
    let tree = scratch("good", &good());
    let output = run(&tree, &["doctor"]);

    // A missing optional tool is a warning, not a failure.
    assert!(output.status.success(), "{output:?}");
    let mut expected: Vec<String> = CHECKS.iter().map(|check| check.to_string()).collect();
    expected.push("warn cargo-llvm-cov".into());
    assert_eq!(labels(&output), expected, "{output:?}");
    let details = lines(&output);
    assert!(
        details.contains(&(
            "ok rust toolchain".into(),
            "1.94.1 (requires 1.94.1)".into()
        )),
        "{details:?}"
    );
    assert!(
        details.contains(&(
            "ok .agent/features/ scenarios".into(),
            "every one of 1 names matches a test".into()
        )),
        "{details:?}"
    );
    drop(tree);

    let broken: [(&str, Fixture, &str, &str); 7] = [
        (
            "old-rustc",
            Fixture {
                rustc: "rustc 1.80.0 (x 2024-07-21)",
                ..good()
            },
            "rust toolchain",
            "1.80.0 (requires 1.94.1)",
        ),
        (
            "no-fmt",
            Fixture {
                fmt_installed: false,
                ..good()
            },
            "cargo-fmt",
            "missing (rustup component add)",
        ),
        (
            "missing-path",
            Fixture {
                feature_files: r#"["src/demo.rs", "src/gone.rs"]"#,
                ..good()
            },
            ".agent/features/ paths",
            "missing [\"src/gone.rs\"]",
        ),
        (
            "empty-filter",
            Fixture {
                workspace_tests: "other::tests::it_runs: test\\n",
                ..good()
            },
            ".agent/features/ test filters",
            "match nothing: [\"[demo] demo::\"]",
        ),
        (
            "redundant-filter",
            Fixture {
                declared_tests: r#"["demo::tests"]"#,
                ..good()
            },
            ".agent/features/ declared filters",
            "already derived from `files`, so delete them: [\"[demo] demo::tests\"]",
        ),
        (
            "unknown-scenario",
            Fixture {
                scenarios: "demo::demo_other: test\\n",
                ..good()
            },
            ".agent/features/ scenarios",
            "demo_works",
        ),
        (
            "no-fake-open",
            Fixture {
                fake_open: false,
                ..good()
            },
            "fake open",
            "tests/fakes/open executable",
        ),
    ];
    for (name, fixture, check, detail) in broken {
        let tree = scratch(name, &fixture);
        let output = run(&tree, &["doctor"]);

        assert!(!output.status.success(), "{name}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("doctor found problems"),
            "{name}: {output:?}"
        );
        let failed: Vec<(String, String)> = lines(&output)
            .into_iter()
            .filter(|(label, _)| label.starts_with("FAIL "))
            .collect();
        assert_eq!(failed.len(), 1, "{name}: {failed:?}");
        assert_eq!(failed[0].0, format!("FAIL {check}"), "{name}");
        assert!(failed[0].1.contains(detail), "{name}: {}", failed[0].1);
    }
}
