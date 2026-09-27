//! `cargo xtask verify-real`: what a feature was seen to do against a real
//! service, recorded as evidence the branch gate holds a change to.
//!
//! A fake answers with what its author believed the service does. When that
//! belief is wrong every test passes and the code is wrong with it: MySQL's
//! identifier quote, SQLSTATE classes and PostgreSQL's parameter types were
//! each wrong until a real server answered. So every feature declares how it
//! is verified for real (`real` in `.agent/features/<feature>.toml`): a probe
//! against a real environment, `pending` when it depends on one and has no
//! probe yet, or `not_applicable` with why. A probe pairs each contract it
//! observes with the mock scenarios that pin the same contract, so a real
//! answer and the fake that stands in for it are named side by side.
//!
//! Evidence (`.agent/real/<feature>.json`) is committed with the change it
//! verifies, so the branch gate, a reviewer and CI all read the same record.
//! It is keyed by a hash of the feature's own files: it is fresh while those
//! files are what the probe ran against. It holds test names and outcomes,
//! never the probe's output, and it is scanned for credentials before it is
//! written. What the probe saw is also written as a fixture candidate under
//! `target/agent/real/`, for a person to turn into mock fixtures; nothing
//! here writes a fixture.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::architecture_fixtures::{results, test_sections};
use crate::matrix_inputs::{not_run_reason, read_cases, read_not_run, read_reports};
use crate::real_declaration::all_problems;
pub(crate) use crate::real_declaration::{Access, Probe, Real};
use crate::{Feature, FeatureMap, cargo, claimed_files, feature_entries, git, load_features, root};

const USAGE: &str = "\
usage: cargo xtask verify-real <FEATURE>
       cargo xtask verify-real --check [FEATURE ...]

Runs the real-environment probe a feature declares under `real` in
.agent/features/<FEATURE>.toml, only when every variable it requires is set,
and writes .agent/real/<FEATURE>.json, to commit with the change, and a
fixture candidate under target/agent/real/. A probe that cannot run is
recorded as unverified, one whose contracts fail as failed; neither is a pass.
A feature whose `real` says `cases = <what they reach>` has its `[real]` cases under
tests/cases/<FEATURE>/ as the probe: `cargo xtask verify --layer real
<FEATURE>` runs them, and each report is a contract of the evidence.

--check runs nothing: it holds the evidence of each FEATURE (every feature
with a probe when none is named) to the files it was taken on, its fixtures
and the credential scan, and fails when one is missing, stale or not verified.
";

/// How long a probe may run, its build included.
const DEADLINE: Duration = Duration::from_secs(3600);

#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Status {
    Verified,
    Unverified,
    Failed,
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
struct Observed {
    test: String,
    /// `passed`, `failed`, `not_run` (the test said so, or has no result) or
    /// `skipped` (a variable it requires is not set).
    result: String,
    mock: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct Evidence {
    schema_version: u32,
    feature: String,
    status: Status,
    reason: Option<String>,
    commit: String,
    /// The feature's own files as the probe ran against them.
    feature_hash: String,
    /// `git diff HEAD` at the time: what was not committed yet.
    diff_hash: String,
    environment: String,
    access: Access,
    command: Vec<String>,
    generated_at: String,
    observed: Vec<Observed>,
    fixtures: BTreeMap<String, String>,
}

impl Evidence {
    /// What a run of `feature` records whatever it ran: the verdict, the
    /// commit, the feature's files and the uncommitted diff it ran on, and
    /// how it reached the environment. It observed nothing and read no
    /// fixture until the caller says otherwise.
    fn new(
        name: &str,
        feature: &Feature,
        status: Status,
        reason: Option<String>,
        environment: String,
        access: Access,
        command: Vec<String>,
    ) -> Self {
        Evidence {
            schema_version: 1,
            feature: name.to_string(),
            status,
            reason,
            commit: git(&["rev-parse", "HEAD"])
                .unwrap_or_default()
                .trim()
                .to_string(),
            feature_hash: feature_hash(feature),
            diff_hash: hash_text(&git(&["diff", "HEAD"]).unwrap_or_default()),
            environment,
            access,
            command,
            generated_at: chrono::Utc::now().to_rfc3339(),
            observed: Vec::new(),
            fixtures: BTreeMap::new(),
        }
    }
}

pub fn verify_real(args: &[String]) -> Result<(), String> {
    let features = load_features()?;
    match args {
        [flag] if flag == "--help" || flag == "-h" => {
            print!("{USAGE}");
            Ok(())
        }
        [flag, names @ ..] if flag == "--check" => check(&features, names),
        [name] if !name.starts_with('-') => run(&features, name),
        _ => Err(USAGE.trim_end().to_string()),
    }
}

fn run(features: &FeatureMap, name: &str) -> Result<(), String> {
    let feature = features
        .get(name)
        .ok_or_else(|| format!("no feature named `{name}` in .agent/features/"))?;
    let problems = all_problems(name, feature);
    if !problems.is_empty() {
        return Err(problems.join("\n"));
    }
    let probe = match &feature.real {
        Some(Real::Probe(probe)) => probe,
        Some(Real::Cases { cases }) => return run_cases_layer(name, feature, cases),
        Some(Real::Pending { pending }) => {
            return Err(format!(
                "{name}: unverified, no probe is registered: {pending}"
            ));
        }
        Some(Real::NotApplicable { not_applicable }) => {
            eprintln!("==> {name}: not applicable: {not_applicable}");
            return Ok(());
        }
        None => return Err(format!("{name}: declares no `real` verification")),
    };
    let missing: Vec<&String> = probe
        .requires
        .iter()
        .filter(|var| std::env::var_os(var).is_none())
        .collect();
    let (status, reason, observed) = if missing.is_empty() {
        eprintln!("==> verify-real {name}: {}", probe.command.join(" "));
        let (output, finished) = run_probe(&probe.command);
        let environment: BTreeSet<String> = probe
            .contracts
            .iter()
            .flat_map(|contract| &contract.requires)
            .filter(|var| std::env::var_os(var).is_some())
            .cloned()
            .collect();
        judge(probe, &output, finished, &environment)
    } else {
        (
            Status::Unverified,
            Some(format!(
                "not run: set {} (the environment is {})",
                missing
                    .iter()
                    .map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                probe.environment
            )),
            vec![],
        )
    };
    let evidence = Evidence {
        observed,
        fixtures: fixture_hashes(probe),
        ..Evidence::new(
            name,
            feature,
            status,
            reason,
            probe.environment.clone(),
            probe.access,
            probe.command.clone(),
        )
    };
    write_evidence(&evidence, &probe.fixtures)?;
    conclude(name, &evidence)
}

/// The `[real]` cases of the feature are its probe: `verify --layer real
/// <feature>` runs them against the real services, and their reports are the
/// contracts observed -- each paired with the same case on the fake layer,
/// which is the mock that pins it. A case the layer did not run (a
/// requirement missing) leaves the feature unverified, with what to prepare.
fn run_cases_layer(name: &str, feature: &Feature, reach: &str) -> Result<(), String> {
    eprintln!("==> verify-real {name}: cargo xtask verify --layer real {name} ({reach})");
    let finished = crate::verify_layer::verify_layer("real", &[name.to_string()]);
    let root = root();
    let reports = read_reports()?;
    let not_run = read_not_run();
    let observed: Vec<Observed> = read_cases(&root)?
        .into_iter()
        .filter(|case| case.feature == name && case.declares_real)
        .map(|case| {
            let report = reports
                .iter()
                .find(|report| report.scenario == case.id && report.evidence == "real");
            let result = match report {
                Some(report) if report.passed => "passed".to_string(),
                Some(_) => "failed".to_string(),
                None => format!("not_run: {}", not_run_reason(&not_run, "real", &case.id)),
            };
            Observed {
                test: case.id.clone(),
                result,
                mock: vec![case.id],
            }
        })
        .collect();
    let (status, reason) = status_of(&observed, finished, "case");
    let evidence = Evidence {
        observed,
        ..Evidence::new(
            name,
            feature,
            status,
            reason,
            "real-services".into(),
            Access::ReadOnly,
            ["cargo", "xtask", "verify", "--layer", "real", name]
                .into_iter()
                .map(str::to_string)
                .collect(),
        )
    };
    write_evidence(&evidence, &[])?;
    conclude(name, &evidence)
}

fn conclude(name: &str, evidence: &Evidence) -> Result<(), String> {
    match evidence.status {
        Status::Verified => {
            eprintln!("==> {name}: verified");
            Ok(())
        }
        other => Err(format!(
            "{name}: {other:?}: {}",
            evidence.reason.as_deref().unwrap_or("see .agent/real/")
        )),
    }
}

fn run_probe(command: &[String]) -> (String, Result<(), String>) {
    let mut process = if command[0] == "cargo" {
        cargo()
    } else {
        Command::new(&command[0])
    };
    process
        .args(&command[1..])
        .current_dir(root())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let mut child = match process.spawn() {
        Ok(child) => child,
        Err(e) => return (String::new(), Err(format!("{}: {e}", command[0]))),
    };
    let mut stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let started = Instant::now();
    let finished = loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break Ok(()),
            Ok(Some(status)) => break Err(format!("the probe exited with {status}")),
            Ok(None) if started.elapsed() > DEADLINE => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!("timed out after {}s", DEADLINE.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(500)),
            Err(e) => break Err(e.to_string()),
        }
    };
    (reader.join().unwrap_or_default(), finished)
}

/// What the probe's run says about each contract, and the status that
/// follows: verified only when every contract that could run passed and
/// said it ran.
fn judge(
    probe: &Probe,
    output: &str,
    finished: Result<(), String>,
    set: &BTreeSet<String>,
) -> (Status, Option<String>, Vec<Observed>) {
    let results = results(output);
    let sections = test_sections(output);
    let observed: Vec<Observed> = probe
        .contracts
        .iter()
        .map(|contract| {
            let result = if contract.requires.iter().any(|var| !set.contains(var)) {
                "skipped"
            } else {
                let said_not_run = sections
                    .get(contract.real.as_str())
                    .is_some_and(|section| section.contains(": not run"));
                match results.get(contract.real.as_str()) {
                    Some(true) if !said_not_run => "passed",
                    Some(false) => "failed",
                    _ => "not_run",
                }
            };
            Observed {
                test: contract.real.clone(),
                result: result.into(),
                mock: contract.mock.clone(),
            }
        })
        .collect();
    let (status, reason) = status_of(&observed, finished, "contract");
    (status, reason, observed)
}

/// The status that follows from what was observed: verified only when every
/// `what` (a contract, a case) that could run passed and the run finished.
/// A `not_run` result may carry its reason after a colon, which is repeated
/// so the evidence says what to prepare.
fn status_of(
    observed: &[Observed],
    finished: Result<(), String>,
    what: &str,
) -> (Status, Option<String>) {
    let count = |result: &str| {
        observed
            .iter()
            .filter(|o| o.result == result || o.result.starts_with(&format!("{result}: ")))
            .count()
    };
    if count("failed") > 0 {
        (
            Status::Failed,
            Some(format!("{} {what}(s) failed", count("failed"))),
        )
    } else if count("not_run") > 0 {
        let reasons: Vec<String> = observed
            .iter()
            .filter_map(|o| o.result.strip_prefix("not_run: "))
            .map(str::to_string)
            .collect();
        (
            Status::Unverified,
            Some(format!(
                "{} {what}(s) did not run{}{}",
                count("not_run"),
                finished.err().map(|e| format!(": {e}")).unwrap_or_default(),
                if reasons.is_empty() {
                    String::new()
                } else {
                    format!(": {}", reasons.join("; "))
                }
            )),
        )
    } else if count("passed") == 0 {
        (Status::Unverified, Some(format!("no {what} ran")))
    } else if let Err(e) = finished {
        (Status::Failed, Some(e))
    } else {
        (Status::Verified, None)
    }
}

/// FNV-1a over the feature's own files, path and content, in path order.
fn feature_hash(feature: &Feature) -> String {
    let mut files: Vec<String> = feature_entries(feature)
        .flat_map(|e| claimed_files(e))
        .collect();
    files.sort();
    files.dedup();
    let mut text = String::new();
    for file in files {
        text.push_str(&file);
        text.push('\0');
        text.push_str(&std::fs::read_to_string(root().join(&file)).unwrap_or_default());
        text.push('\0');
    }
    hash_text(&text)
}

fn fixture_hashes(probe: &Probe) -> BTreeMap<String, String> {
    probe
        .fixtures
        .iter()
        .map(|path| {
            let content = std::fs::read(root().join(path)).unwrap_or_default();
            (path.clone(), hash_text(&String::from_utf8_lossy(&content)))
        })
        .collect()
}

pub(crate) fn hash_text(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Text that looks like a credential: an AWS access key id, a private key, a
/// bearer token, an Authorization header, a secret field, a GitHub token or
/// the value of an API key header. The evidence carries test names, so any of
/// these means something leaked into it.
pub(crate) fn credential_marks(text: &str) -> Vec<&'static str> {
    let key_id = text
        .match_indices("AKIA")
        .chain(text.match_indices("ASIA"))
        .any(|(at, _)| {
            text[at + 4..]
                .chars()
                .take(16)
                .filter(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
                .count()
                == 16
        });
    let mut marks = Vec::new();
    if key_id {
        marks.push("an AWS access key id");
    }
    for (needle, mark) in [
        ("-----BEGIN ", "a PEM block"),
        ("Bearer ", "a bearer token"),
        ("Authorization:", "an Authorization header"),
        ("aws_secret_access_key", "a secret access key"),
        ("SecretAccessKey", "a secret access key"),
        ("x-amz-security-token", "a session token"),
    ] {
        if text.contains(needle) {
            marks.push(mark);
        }
    }
    if has_github_token(text) {
        marks.push("a GitHub token");
    }
    if has_api_key_header_value(text) {
        marks.push("an API key header");
    }
    marks
}

/// How many characters a token is made of from the start of `text`.
fn token_length(text: &str) -> usize {
    text.chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .count()
}

/// A GitHub token prefix followed by the token itself, not the prefix named
/// on its own.
fn has_github_token(text: &str) -> bool {
    ["ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_"]
        .iter()
        .any(|prefix| {
            text.match_indices(prefix)
                .any(|(at, _)| token_length(&text[at + prefix.len()..]) >= 20)
        })
}

/// An `*-api-key` header (`X-Api-Key: value`, or `"x-api-key": "value"` in
/// JSON) with a value in it: a masked `****` or a TOML key naming the header
/// is not one.
fn has_api_key_header_value(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.match_indices("api-key").any(|(at, needle)| {
        let rest = &lower[at + needle.len()..];
        let rest = rest.strip_prefix('"').unwrap_or(rest);
        let Some(value) = rest.strip_prefix(':') else {
            return false;
        };
        let value = value.trim_start();
        token_length(value.strip_prefix('"').unwrap_or(value)) >= 8
    })
}

fn evidence_dir() -> PathBuf {
    root().join(".agent/real")
}

fn candidate_dir() -> PathBuf {
    crate::agent_dir().join("real")
}

fn write_evidence(evidence: &Evidence, fixtures: &[String]) -> Result<(), String> {
    let json = serde_json::to_string_pretty(evidence).unwrap();
    let mut marks = credential_marks(&json);
    for path in fixtures {
        marks.extend(credential_marks(
            &std::fs::read_to_string(root().join(path)).unwrap_or_default(),
        ));
    }
    if !marks.is_empty() {
        return Err(format!(
            "{}: refusing to write evidence that holds {}",
            evidence.feature,
            marks.join(", ")
        ));
    }
    let dir = evidence_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    std::fs::write(dir.join(format!("{}.json", evidence.feature)), &json)
        .map_err(|e| e.to_string())?;
    // A candidate, never a fixture: a person reads it and decides what the
    // mock side should learn from it.
    let candidate = serde_json::json!({
        "derived_from": { "commit": evidence.commit, "feature_hash": evidence.feature_hash },
        "environment": evidence.environment,
        "contracts": evidence.observed,
    });
    let candidates = candidate_dir();
    std::fs::create_dir_all(&candidates).map_err(|e| format!("{}: {e}", candidates.display()))?;
    std::fs::write(
        candidates.join(format!("{}.fixture-candidate.json", evidence.feature)),
        serde_json::to_string_pretty(&candidate).unwrap(),
    )
    .map_err(|e| e.to_string())
}

fn read_evidence(name: &str) -> Option<Evidence> {
    let text = std::fs::read_to_string(evidence_dir().join(format!("{name}.json"))).ok()?;
    serde_json::from_str(&text).ok()
}

/// Whether a feature's evidence holds for the files as they are: `Ok` with a
/// summary, `Err` with why not.
pub(crate) fn evidence_holds(name: &str, feature: &Feature) -> Result<String, String> {
    let fixtures = match &feature.real {
        Some(Real::Probe(probe)) => fixture_hashes(probe),
        Some(Real::Cases { .. }) => BTreeMap::new(),
        _ => return Ok(format!("{name}: no probe")),
    };
    judge_evidence(
        name,
        read_evidence(name).as_ref(),
        &feature_hash(feature),
        &fixtures,
    )
}

fn judge_evidence(
    name: &str,
    evidence: Option<&Evidence>,
    current_hash: &str,
    fixtures: &BTreeMap<String, String>,
) -> Result<String, String> {
    let hint = format!("run `cargo xtask verify-real {name}` against its environment");
    let Some(evidence) = evidence else {
        return Err(format!("{name}: no real-environment evidence; {hint}"));
    };
    if evidence.status != Status::Verified {
        return Err(format!(
            "{name}: the last real run is {:?}: {}; {hint}",
            evidence.status,
            evidence.reason.as_deref().unwrap_or("no reason recorded")
        ));
    }
    if evidence.feature_hash != current_hash {
        return Err(format!(
            "{name}: the evidence was taken on other files than these; {hint}"
        ));
    }
    if &evidence.fixtures != fixtures {
        return Err(format!(
            "{name}: a fixture changed since the evidence was taken; {hint}"
        ));
    }
    let marks = credential_marks(&serde_json::to_string(evidence).unwrap());
    if !marks.is_empty() {
        return Err(format!("{name}: the evidence holds {}", marks.join(", ")));
    }
    let skipped = evidence
        .observed
        .iter()
        .filter(|o| o.result == "skipped")
        .count();
    Ok(format!(
        "{name}: verified at {} on {}{}",
        &evidence.commit[..evidence.commit.len().min(7)],
        evidence.environment,
        if skipped > 0 {
            format!(" ({skipped} contract(s) skipped)")
        } else {
            String::new()
        }
    ))
}

fn check(features: &FeatureMap, names: &[String]) -> Result<(), String> {
    let selected: Vec<&String> = if names.is_empty() {
        features
            .iter()
            .filter(|(_, f)| matches!(f.real, Some(Real::Probe(_) | Real::Cases { .. })))
            .map(|(name, _)| name)
            .collect()
    } else {
        names.iter().collect()
    };
    let mut failed = Vec::new();
    for name in selected {
        let feature = features
            .get(name)
            .ok_or_else(|| format!("no feature named `{name}`"))?;
        match evidence_holds(name, feature) {
            Ok(summary) => eprintln!("ok   {summary}"),
            Err(problem) => {
                eprintln!("FAIL {problem}");
                failed.push(problem);
            }
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} feature(s) lack fresh real evidence",
            failed.len()
        ))
    }
}

/// The branch gate over the features a branch changed: every one with a
/// probe needs fresh verified evidence, and one that is `pending` is named as
/// unverified without failing the gate.
pub(crate) fn gate(features: &FeatureMap, changed: &[String]) -> Result<String, String> {
    let mut problems = Vec::new();
    let mut verified = Vec::new();
    let mut pending = Vec::new();
    for name in changed {
        let Some(feature) = features.get(name) else {
            continue;
        };
        let declared = all_problems(name, feature);
        if !declared.is_empty() {
            problems.extend(declared);
            continue;
        }
        match &feature.real {
            Some(Real::Probe(_) | Real::Cases { .. }) => match evidence_holds(name, feature) {
                Ok(summary) => verified.push(summary),
                Err(problem) => problems.push(problem),
            },
            Some(Real::Pending { .. }) => pending.push(name.as_str()),
            Some(Real::NotApplicable { .. }) => {}
            None => problems.push(format!("{name}: declares no `real` verification")),
        }
    }
    if !problems.is_empty() {
        return Err(problems.join("\n"));
    }
    let mut parts = verified;
    if !pending.is_empty() {
        parts.push(format!(
            "unverified, no probe registered: {}",
            pending.join(", ")
        ));
    }
    Ok(if parts.is_empty() {
        "no changed feature talks to a real environment".into()
    } else {
        parts.join("; ")
    })
}

#[cfg(test)]
#[path = "verify_real_tests.rs"]
mod tests;
