//! The fixtures of the architecture registry, run: each violating fixture has
//! to be refused by its rule's detector and each passing fixture allowed, and
//! a fixture that could not say either is not counted as having said yes.
//!
//! A fixture is a `#[test]` under `tests/architecture/` that feeds a detector
//! a source and asserts through `assert_detected` / `assert_allowed`, whose
//! panic names the rule and what went wrong. So a failure that says
//! `ARCH-006: missed a violation` is a detector that missed; any other
//! failure -- a fixture that does not parse, a panic in the harness, the
//! wrong rule's message -- is a fixture that failed for another reason, and a
//! fixture with no result at all (the binary did not build, the run timed
//! out, the test is not under `tests/architecture/`) was not run.

use std::collections::BTreeMap;
use std::io::Read;
use std::process::Stdio;
use std::time::{Duration, Instant};

use crate::{cargo, root};

/// How long the fixture run may take, the build of the architecture tests
/// included.
const DEADLINE: Duration = Duration::from_secs(1800);

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Expect {
    /// A violating fixture: the detector refuses it.
    Refused,
    /// A passing fixture: the detector allows it.
    Allowed,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub(crate) struct Fixture {
    pub(crate) rule: String,
    pub(crate) reference: String,
    pub(crate) expects: Expect,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "outcome", content = "detail", rename_all = "snake_case")]
pub(crate) enum Outcome {
    /// A violating fixture its detector refused.
    Detected,
    /// A passing fixture its detector allowed.
    Allowed,
    /// A violating fixture its detector let through.
    Missed,
    /// A passing fixture its detector refused.
    Refused,
    /// The fixture failed, but not with its rule's message.
    OtherFailure(String),
    /// No result: not built, timed out, or not a test the runner can run.
    NotRun(String),
}

impl Outcome {
    pub(crate) fn is_expected(&self) -> bool {
        matches!(self, Outcome::Detected | Outcome::Allowed)
    }
}

/// `tests/architecture/<module>.rs::<name>` as the architecture binary names
/// the test: `<module>::<name>`.
pub(crate) fn test_name(reference: &str) -> Option<String> {
    let (file, name) = reference.split_once("::")?;
    let module = file
        .strip_prefix("tests/architecture/")?
        .strip_suffix(".rs")?;
    (!module.contains('/') && module != "main").then(|| format!("{module}::{name}"))
}

/// Runs every fixture the architecture binary holds, once, and says what
/// each one did.
pub(crate) fn run(fixtures: &[Fixture]) -> Vec<Outcome> {
    let names: Vec<String> = fixtures
        .iter()
        .filter_map(|fixture| test_name(&fixture.reference))
        .collect();
    if names.is_empty() {
        return outcomes(
            fixtures,
            "",
            Err("no fixture is under tests/architecture/".into()),
        );
    }
    let (output, finished) = run_tests(&names);
    outcomes(fixtures, &output, finished)
}

fn run_tests(names: &[String]) -> (String, Result<(), String>) {
    let mut command = cargo();
    command
        .args(["test", "--locked"])
        .args(crate::TEST_FEATURES)
        .args(["--test", "architecture", "--", "--exact"])
        .args(names)
        .current_dir(root())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => return (String::new(), Err(format!("cargo: {e}"))),
    };
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let out = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let err = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    let started = Instant::now();
    let finished = loop {
        match child.try_wait() {
            Ok(Some(_)) => break Ok(()),
            Ok(None) if started.elapsed() > DEADLINE => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!("timed out after {}s", DEADLINE.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(200)),
            Err(e) => break Err(format!("cargo test: {e}")),
        }
    };
    let output = out.join().unwrap_or_default();
    let errors = err.join().unwrap_or_default();
    let finished = finished.and_then(|()| {
        if output.contains("\ntest ") || output.starts_with("test ") {
            Ok(())
        } else {
            Err(format!(
                "the architecture tests did not run: {}",
                errors
                    .lines()
                    .rev()
                    .find(|line| !line.trim().is_empty())
                    .unwrap_or("no output")
            ))
        }
    });
    (output, finished)
}

/// What each fixture did, read from the output of one `cargo test` run.
/// `finished` is why a fixture with no result has none.
pub(crate) fn outcomes(
    fixtures: &[Fixture],
    output: &str,
    finished: Result<(), String>,
) -> Vec<Outcome> {
    let results = results(output);
    let failures = test_sections(output);
    fixtures
        .iter()
        .map(|fixture| {
            let Some(name) = test_name(&fixture.reference) else {
                return Outcome::NotRun("only fixtures under tests/architecture/ are run".into());
            };
            match (results.get(name.as_str()), fixture.expects) {
                (Some(true), Expect::Refused) => Outcome::Detected,
                (Some(true), Expect::Allowed) => Outcome::Allowed,
                (Some(false), expects) => {
                    let message = failures.get(name.as_str()).copied().unwrap_or_default();
                    let wanted = match expects {
                        Expect::Refused => format!("{}: missed a violation", fixture.rule),
                        Expect::Allowed => format!("{}: refused allowed code", fixture.rule),
                    };
                    if message.contains(&wanted) {
                        match expects {
                            Expect::Refused => Outcome::Missed,
                            Expect::Allowed => Outcome::Refused,
                        }
                    } else {
                        Outcome::OtherFailure(panic_line(message))
                    }
                }
                (None, _) => Outcome::NotRun(match &finished {
                    Err(reason) => reason.clone(),
                    Ok(()) => "the run has no result for this test".into(),
                }),
            }
        })
        .collect()
}

/// `test <name> ... ok` / `... FAILED` lines: whether each test passed.
pub(crate) fn results(output: &str) -> BTreeMap<&str, bool> {
    output
        .lines()
        .filter_map(|line| line.strip_prefix("test "))
        .filter_map(|rest| rest.split_once(" ... "))
        .filter_map(|(name, result)| match result.trim() {
            "ok" => Some((name, true)),
            "FAILED" => Some((name, false)),
            _ => None,
        })
        .collect()
}

/// The `---- <name> stdout ----` section of each test libtest printed one
/// for: every failed test, and every test under `--show-output`.
pub(crate) fn test_sections(output: &str) -> BTreeMap<&str, &str> {
    let mut messages = BTreeMap::new();
    let mut rest = output;
    while let Some(start) = rest.find("---- ") {
        let header = &rest[start + 5..];
        let Some(end_of_name) = header.find(" stdout ----") else {
            break;
        };
        let name = &header[..end_of_name];
        let body = &header[end_of_name + " stdout ----".len()..];
        let end = body
            .find("\n---- ")
            .or_else(|| body.find("\nfailures:"))
            .or_else(|| body.find("\nsuccesses:"))
            .unwrap_or(body.len());
        messages.insert(name, &body[..end]);
        rest = &body[end..];
    }
    messages
}

/// The line of a failure section that says why it panicked.
fn panic_line(message: &str) -> String {
    let mut lines = message
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let panicked = lines.by_ref().find(|line| line.contains("panicked at"));
    match (panicked, lines.next()) {
        (Some(_), Some(reason)) => reason.to_string(),
        _ => "failed with no panic message".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(rule: &str, reference: &str, expects: Expect) -> Fixture {
        Fixture {
            rule: rule.into(),
            reference: reference.into(),
            expects,
        }
    }

    const OUTPUT: &str = "\
running 5 tests
test layers::refused ... ok
test layers::allowed ... ok
test layers::missed ... FAILED
test errors::refused_wrongly ... FAILED
test errors::other ... FAILED

failures:

---- layers::missed stdout ----

thread 'layers::missed' panicked at tests/architecture/layers.rs:9:5:
ARCH-001: missed a violation: use std::fs;

---- errors::refused_wrongly stdout ----

thread 'errors::refused_wrongly' panicked at tests/architecture/errors.rs:3:5:
ARCH-006: refused allowed code: fn f() {}

---- errors::other stdout ----

thread 'errors::other' panicked at tests/architecture/syntax.rs:1:1:
does not parse: expected `;`

failures:
    layers::missed
";

    #[test]
    fn each_fixture_is_read_as_what_its_rule_said() {
        let fixtures = [
            fixture(
                "ARCH-001",
                "tests/architecture/layers.rs::refused",
                Expect::Refused,
            ),
            fixture(
                "ARCH-001",
                "tests/architecture/layers.rs::allowed",
                Expect::Allowed,
            ),
            fixture(
                "ARCH-001",
                "tests/architecture/layers.rs::missed",
                Expect::Refused,
            ),
            fixture(
                "ARCH-006",
                "tests/architecture/errors.rs::refused_wrongly",
                Expect::Allowed,
            ),
            fixture(
                "ARCH-006",
                "tests/architecture/errors.rs::other",
                Expect::Refused,
            ),
            fixture(
                "ARCH-006",
                "tests/architecture/errors.rs::absent",
                Expect::Refused,
            ),
            fixture("ARCH-021", "xtask/src/deps.rs::elsewhere", Expect::Refused),
        ];
        assert_eq!(
            outcomes(&fixtures, OUTPUT, Ok(())),
            [
                Outcome::Detected,
                Outcome::Allowed,
                Outcome::Missed,
                Outcome::Refused,
                Outcome::OtherFailure("does not parse: expected `;`".into()),
                Outcome::NotRun("the run has no result for this test".into()),
                Outcome::NotRun("only fixtures under tests/architecture/ are run".into()),
            ]
        );
    }

    #[test]
    fn a_failure_with_another_rules_message_is_not_a_miss_of_this_one() {
        let fixtures = [fixture(
            "ARCH-009",
            "tests/architecture/layers.rs::missed",
            Expect::Refused,
        )];
        assert_eq!(
            outcomes(&fixtures, OUTPUT, Ok(())),
            [Outcome::OtherFailure(
                "ARCH-001: missed a violation: use std::fs;".into()
            )]
        );
    }

    #[test]
    fn a_run_that_did_not_finish_leaves_every_fixture_not_run_with_why() {
        let fixtures = [fixture(
            "ARCH-001",
            "tests/architecture/layers.rs::refused",
            Expect::Refused,
        )];
        let outcome = outcomes(&fixtures, "", Err("timed out after 1800s".into()));
        assert_eq!(outcome, [Outcome::NotRun("timed out after 1800s".into())]);
        assert!(!outcome[0].is_expected());
        assert!(Outcome::Detected.is_expected() && Outcome::Allowed.is_expected());
        assert!(!Outcome::Missed.is_expected() && !Outcome::Refused.is_expected());
    }

    #[test]
    fn only_a_module_of_the_architecture_binary_names_a_runnable_test() {
        assert_eq!(
            test_name("tests/architecture/layers.rs::x"),
            Some("layers::x".into())
        );
        assert_eq!(test_name("tests/architecture/main.rs::x"), None);
        assert_eq!(test_name("tests/architecture/a/b.rs::x"), None);
        assert_eq!(test_name("tests/scenarios/exec.rs::x"), None);
        assert_eq!(test_name("tests/architecture/layers.rs"), None);
    }

    #[test]
    fn a_section_holds_its_own_text_and_nothing_after_it() {
        let sections = test_sections(OUTPUT);
        assert_eq!(
            sections["layers::missed"],
            "\n\nthread 'layers::missed' panicked at tests/architecture/layers.rs:9:5:\n\
             ARCH-001: missed a violation: use std::fs;\n"
        );
        assert_eq!(sections.len(), 3);
    }

    #[test]
    fn a_failure_without_a_panic_message_says_so() {
        assert_eq!(panic_line("\nsomething\n"), "failed with no panic message");
    }
}
