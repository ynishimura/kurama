//! `cargo xtask verify`: run the scenarios a target asks for, hold the run to
//! the reports it wrote, and write target/agent/verification-report.{json,md};
//! the two spellings of a scenario name are converted here.

use std::collections::BTreeSet;
use std::path::Path;

use crate::features::{FeatureMap, load_features};
use crate::gate::Gate;
use crate::impact::{compute_impact, default_base};
use crate::{
    TEST_FEATURES, agent_dir, cargo, json_files, matrix_inputs, root, run_in_root,
    scenario_report_dir, short_head, verify_layer,
};

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct ScenarioReport {
    scenario: String,
    feature: String,
    command: Vec<String>,
    passed: bool,
    checks: Vec<Check>,
    observed: serde_json::Value,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct Check {
    name: String,
    ok: bool,
    detail: String,
}

pub(crate) fn verify(target: Option<&str>) -> Result<(), String> {
    verify_against(target, &default_base())
}

/// `verify`, with `affected` measured against `base`: `branch-check --base`
/// has to select its scenarios from the same diff as its tests and mutants.
pub(crate) fn verify_against(target: Option<&str>, base: &str) -> Result<(), String> {
    let target = target.ok_or("usage: cargo xtask verify <all|affected|FEATURE|PREFIX>")?;
    let features = load_features()?;
    let known = list_scenarios()?;

    // Which scenarios the target asks for, and how to tell `cargo test` about
    // them. `requested` is what the run is held to afterwards.
    let (label, requested, filter_args): (String, Vec<String>, Vec<String>) = match target {
        "all" => ("all".into(), mapped_scenarios(&features), vec![]),
        "affected" => {
            let impact = compute_impact(base)?;
            if impact.all_features {
                (
                    "affected (unmapped changes: all scenarios)".into(),
                    mapped_scenarios(&features),
                    vec![],
                )
            } else if impact.scenarios.is_empty() {
                return write_not_run_report(
                    target,
                    "no scenario is affected by the current changes",
                );
            } else {
                let mut args = vec!["--exact".to_string()];
                args.extend(qualify_scenarios(&impact.scenarios, &known)?);
                let mut selected = impact.features.clone();
                selected.extend(impact.dependent_features.clone());
                (
                    format!("affected ({})", selected.join(", ")),
                    impact.scenarios.clone(),
                    args,
                )
            }
        }
        name if features.contains_key(name) => {
            let scenarios = &features[name].scenarios;
            if scenarios.is_empty() {
                return write_not_run_report(
                    target,
                    &format!(
                        "feature `{name}` has no runtime scenarios (see its .agent/features/ notes)"
                    ),
                );
            }
            let mut args = vec!["--exact".to_string()];
            args.extend(qualify_scenarios(scenarios, &known)?);
            (format!("feature {name}"), scenarios.clone(), args)
        }
        prefix => {
            let selected: Vec<String> = known
                .iter()
                .filter(|entry| scenario_name(entry).starts_with(prefix))
                .map(|entry| scenario_name(entry).to_string())
                .collect();
            if selected.is_empty() {
                return Err(format!(
                    "`{prefix}` is neither a feature nor a scenario prefix; run `cargo xtask map`"
                ));
            }
            (
                format!("scenarios matching `{prefix}`"),
                selected,
                vec![prefix.to_string()],
            )
        }
    };

    clear_scenario_reports()?;
    eprintln!("==> verify {label}");
    let status = cargo()
        .args(["test", "--locked", "--test", "scenarios"])
        .args(TEST_FEATURES)
        .arg("--")
        .args(&filter_args)
        .current_dir(root())
        .status()
        .map_err(|e| format!("cargo: {e}"))?;

    let mut gates = vec![Gate {
        name: "scenarios".into(),
        ok: status.success(),
        detail: label,
    }];
    // A run that failed already names its failure; the question this answers is
    // what a green run actually verified.
    if status.success() {
        gates.push(coverage_gate(&requested)?);
    }
    conclude_in(&scenario_report_dir(), target, gates)
}

fn write_not_run_report(target: &str, reason: &str) -> Result<(), String> {
    clear_scenario_reports()?;
    verify_layer::not_run(&scenario_report_dir(), target, reason)
}

/// Removes the scenario reports and the TUI screens of the previous run, so
/// a report never lists a scenario that no longer runs.
pub(crate) fn clear_scenario_reports() -> Result<(), String> {
    for dir in [scenario_report_dir(), agent_dir().join("tui")] {
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| format!("clear {}: {e}", dir.display()))?;
        }
    }
    Ok(())
}

/// Every report the scenarios of the last run wrote under `dir`
/// (`scenarios/`, or `scenarios-<layer>/`), by the name they carry.
fn read_scenario_reports_in(dir: &Path) -> Result<Vec<ScenarioReport>, String> {
    let mut reports: Vec<ScenarioReport> = Vec::new();
    // A layer that could not run leaves `not-run.txt` next to no report.
    for path in json_files(dir).unwrap_or_default() {
        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        reports
            .push(serde_json::from_str(&content).map_err(|e| format!("{}: {e}", path.display()))?);
    }
    reports.sort_by(|a, b| a.scenario.cmp(&b.scenario));
    Ok(reports)
}

/// Reads every scenario report, writes `verification-report.{json,md}` and
/// returns the markdown.
pub(crate) fn write_report(target: &str, gates: Vec<Gate>) -> Result<String, String> {
    write_report_in(&scenario_report_dir(), target, gates)
}

/// The same for the reports under `dir`: `scenarios/` writes
/// `verification-report.{json,md}` next to it, `scenarios-<layer>/` writes
/// `verification-report-<layer>.{json,md}`, so a layer run never replaces
/// what the fake run said.
pub(crate) fn write_report_in(
    dir: &Path,
    target: &str,
    gates: Vec<Gate>,
) -> Result<String, String> {
    let reports = read_scenario_reports_in(dir)?;
    let stem = dir
        .file_name()
        .map(|name| {
            name.to_string_lossy()
                .replacen("scenarios", "verification-report", 1)
        })
        .unwrap_or_else(|| "verification-report".into());

    let head = short_head();
    let dirty = matrix_inputs::tree_is_dirty();
    let all_passed =
        gates.iter().all(|g| g.ok && !g.is_not_run()) && reports.iter().all(|r| r.passed);
    let status = if gates.iter().any(Gate::is_not_run) {
        "NOT_RUN"
    } else if all_passed {
        "PASS"
    } else {
        "FAIL"
    };
    let json = serde_json::json!({
        "schema_version": 2,
        "generated_at": chrono::Utc::now().to_rfc3339(),
        "git": { "head": head, "dirty": dirty },
        "target": target,
        "passed": all_passed,
        "status": status,
        "gates": gates,
        "scenarios": reports,
    });
    let out_dir = agent_dir();
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    std::fs::write(
        out_dir.join(format!("{stem}.json")),
        serde_json::to_string_pretty(&json).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    let markdown = render_markdown(target, &head, dirty, &gates, &reports, all_passed);
    std::fs::write(out_dir.join(format!("{stem}.md")), &markdown).map_err(|e| e.to_string())?;
    Ok(markdown)
}

/// Writes the report of a run over the reports under `dir`, prints it, and
/// fails when a gate did.
pub(crate) fn conclude_in(dir: &Path, target: &str, gates: Vec<Gate>) -> Result<(), String> {
    let passed = gates.iter().all(|gate| gate.ok);
    let report = write_report_in(dir, target, gates)?;
    println!("{report}");
    if passed {
        Ok(())
    } else {
        Err("verification failed; see the report above".into())
    }
}

/// An `error[...]` / `hint:` line with what changes from run to run taken
/// out: the checkout the scenarios ran from, a scratch directory the harness
/// made, and the port of a local fake server. The report is what a refactor
/// and an integration diff against another run, and those three made every
/// run differ in 16 rows that said the same thing.
fn stable_line(line: &str, root: &Path, temp: &Path) -> String {
    let mut line = line.replace(&root.display().to_string(), "<repo>");
    let scratch = format!(
        "{}.tmp",
        temp.display().to_string().trim_end_matches('/').to_owned() + "/"
    );
    while let Some(start) = line.find(&scratch) {
        let rest = &line[start + scratch.len()..];
        let length = rest
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(rest.len());
        line.replace_range(start..start + scratch.len() + length, "<tmp>");
    }
    let mut out = String::new();
    let mut rest = line.as_str();
    while let Some(found) = rest.find("127.0.0.1:") {
        let after = &rest[found + "127.0.0.1:".len()..];
        let digits = after.chars().take_while(char::is_ascii_digit).count();
        out.push_str(&rest[..found]);
        out.push_str("127.0.0.1:");
        out.push_str(if digits > 0 { "<port>" } else { "" });
        rest = &after[digits..];
    }
    out.push_str(rest);
    out
}

fn render_markdown(
    target: &str,
    head: &str,
    dirty: bool,
    gates: &[Gate],
    reports: &[ScenarioReport],
    all_passed: bool,
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "## Verification report\n\nTarget: `{target}` at `{head}`{}. Result: **{}**.\n\n",
        if dirty { " (uncommitted changes)" } else { "" },
        if gates.iter().any(Gate::is_not_run) {
            "NOT_RUN"
        } else if all_passed {
            "PASS"
        } else {
            "FAIL"
        },
    ));
    out.push_str(&Gate::markdown_table(gates));
    out.push_str(&format!("\n{} scenario(s):\n\n", reports.len()));
    out.push_str(
        "| Scenario | Feature | Result | Runs | STS calls | 1Password | Browser | Files written | Error lines |\n",
    );
    out.push_str("| --- | --- | --- | --- | --- | --- | --- | --- | --- |\n");
    for report in reports {
        let runs = report.observed["runs"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let sts: Vec<String> = runs
            .iter()
            .flat_map(|run| run["sts_calls"].as_array().cloned().unwrap_or_default())
            .map(|call| {
                let mfa = if call["token_code"].is_string() {
                    "+MFA"
                } else {
                    ""
                };
                format!("{}{mfa}", call["action"].as_str().unwrap_or("?"))
            })
            .collect();
        let op: usize = runs
            .iter()
            .map(|run| run["op_calls"].as_array().map(Vec::len).unwrap_or(0))
            .sum();
        let browser: usize = runs
            .iter()
            .map(|run| run["open_calls"].as_array().map(Vec::len).unwrap_or(0))
            .sum();
        let files = report.observed["files_written"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0);
        // The `error[CODE]` and `hint:` lines of every run: a refactor diffs
        // two reports to see a changed message.
        let errors: Vec<String> = runs
            .iter()
            .flat_map(|run| {
                run["stderr"]
                    .as_str()
                    .unwrap_or_default()
                    .lines()
                    .filter(|line| line.starts_with("error[") || line.starts_with("hint: "))
                    .map(|line| {
                        stable_line(line, &root(), &std::env::temp_dir()).replace('|', "\\|")
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        out.push_str(&format!(
            "| `{}` | {} | {} | {} | {} | {op} | {browser} | {files} | {} |\n",
            report.scenario,
            report.feature,
            if report.passed { "pass" } else { "FAIL" },
            runs.len(),
            if sts.is_empty() {
                "none".to_string()
            } else {
                sts.join(", ")
            },
            if errors.is_empty() {
                "-".to_string()
            } else {
                errors.join("<br>")
            },
        ));
    }
    let screens: Vec<(&str, &serde_json::Value)> = reports
        .iter()
        .flat_map(|report| {
            report.observed["screens"]
                .as_array()
                .into_iter()
                .flatten()
                .map(move |screen| (report.scenario.as_str(), screen))
        })
        .collect();
    if !screens.is_empty() {
        out.push_str("\n### TUI screens\n\n");
        for (scenario, screen) in screens {
            out.push_str(&format!(
                "- `{scenario}` {} ({}x{}): `{}/screen.{{txt,ansi,png}}`\n",
                screen["step"].as_str().unwrap_or("?"),
                screen["cols"],
                screen["rows"],
                screen["dir"].as_str().unwrap_or("?")
            ));
        }
    }
    let failed: Vec<&ScenarioReport> = reports.iter().filter(|r| !r.passed).collect();
    if !failed.is_empty() {
        out.push_str("\n### Failed checks\n\n");
        for report in failed {
            for check in report.checks.iter().filter(|c| !c.ok) {
                out.push_str(&format!(
                    "- `{}`: {} ({})\n",
                    report.scenario, check.name, check.detail
                ));
            }
        }
    }
    out
}

/// Scenario names from `cargo test --test scenarios -- --list`, as the test
/// binary spells them: one module per feature, so a scenario is listed
/// qualified by that module. The harness's own unit tests are compiled into
/// the same binary under `support::` and are not scenarios.
pub(crate) fn list_scenarios() -> Result<BTreeSet<String>, String> {
    let output = run_in_root(
        "cargo",
        &[
            &["test", "--locked", "--quiet"][..],
            &TEST_FEATURES,
            &["--test", "scenarios", "--", "--list"],
        ]
        .concat(),
    )?;
    Ok(test_names(&output)
        .into_iter()
        .filter(|name| !name.starts_with("support::"))
        .collect())
}

/// The scenario's own name. The test binary qualifies a scenario by the module
/// of its feature, while `.agent/features/`, the issue bodies and the reports
/// the harness writes all name the scenario itself. One spelling of that
/// conversion is what keeps the two from drifting apart again.
pub(crate) fn scenario_name(listed: &str) -> &str {
    listed.rsplit("::").next().unwrap_or(listed)
}

/// The listed names for the scenarios a target asked for. `--exact` matches
/// what the binary lists, not what the feature map holds, and a filter that
/// matches nothing makes `cargo test` exit 0 having run nothing: a name no
/// test carries has to stop the run here rather than pass as an empty one.
pub(crate) fn qualify_scenarios(
    requested: &[String],
    listed: &BTreeSet<String>,
) -> Result<Vec<String>, String> {
    let mut qualified = Vec::new();
    let mut unknown = Vec::new();
    for name in requested {
        match listed.iter().find(|entry| scenario_name(entry) == name) {
            Some(entry) => qualified.push(entry.clone()),
            None => unknown.push(name.as_str()),
        }
    }
    if unknown.is_empty() {
        Ok(qualified)
    } else {
        Err(format!(
            "no test in tests/scenarios/ carries these names: {}",
            unknown.join(", ")
        ))
    }
}

/// The scenarios a run was asked for that wrote no report. A `cargo test` that
/// selected nothing still exits 0, so this is what tells a green run that
/// verified something from one that verified nothing.
fn scenarios_not_reported(requested: &[String], reports: &[ScenarioReport]) -> Vec<String> {
    requested
        .iter()
        .filter(|name| !reports.iter().any(|report| &&report.scenario == name))
        .cloned()
        .collect()
}

/// The gate that compares what a run asked for against what it reported.
pub(crate) fn coverage_gate(requested: &[String]) -> Result<Gate, String> {
    coverage_gate_in(&scenario_report_dir(), requested)
}

/// The coverage gate over the reports of one layer.
pub(crate) fn coverage_gate_in(dir: &Path, requested: &[String]) -> Result<Gate, String> {
    let missing = scenarios_not_reported(requested, &read_scenario_reports_in(dir)?);
    Ok(Gate {
        name: "coverage".into(),
        ok: missing.is_empty(),
        detail: if missing.is_empty() {
            format!(
                "{} scenarios asked for, {} reported",
                requested.len(),
                requested.len()
            )
        } else {
            format!(
                "{} scenarios asked for; {} wrote no report: {}",
                requested.len(),
                missing.len(),
                missing.join(", ")
            )
        },
    })
}

/// Every scenario the feature map claims, which is what `all` asks for. A
/// scenario is claimed by every feature it verifies -- `errors` and
/// `shell-integration` claim scenarios another feature owns -- so the same
/// name appears several times and is counted once here.
pub(crate) fn mapped_scenarios(features: &FeatureMap) -> Vec<String> {
    features
        .values()
        .flat_map(|feature| feature.scenarios.iter().cloned())
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect()
}

pub(crate) fn test_names(listing: &str) -> BTreeSet<String> {
    listing
        .lines()
        .filter_map(|line| line.strip_suffix(": test"))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::Feature;

    fn listing(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn report(scenario: &str) -> ScenarioReport {
        ScenarioReport {
            scenario: scenario.to_string(),
            feature: "oauth".into(),
            command: vec![],
            passed: true,
            checks: vec![],
            observed: serde_json::Value::Null,
        }
    }

    /// `tests/scenarios/` became one module per feature, which changed the
    /// name the test binary lists and nothing else. `--exact` matches that
    /// name, so the feature map's name has to be resolved to it.
    #[test]
    fn a_mapped_name_resolves_to_the_name_the_test_binary_lists() {
        let listed = listing(&[
            "oauth::oauth_rejected_grant_exits_4",
            "mfa_login::login_caches_the_mfa_session_once,",
        ]);
        assert_eq!(
            qualify_scenarios(&["oauth_rejected_grant_exits_4".to_string()], &listed),
            Ok(vec!["oauth::oauth_rejected_grant_exits_4".to_string()])
        );
    }

    /// The failure this is here for: a filter built from a name no test
    /// carries selects nothing, and `cargo test` exits 0 having run nothing.
    #[test]
    fn a_name_no_test_carries_stops_the_run_instead_of_selecting_nothing() {
        let listed = listing(&["oauth::oauth_rejected_grant_exits_4"]);
        let error = qualify_scenarios(&["oauth_renamed_away".to_string()], &listed)
            .expect_err("a name no test carries is an error");
        assert!(error.contains("oauth_renamed_away"), "{error}");
    }

    /// The same drift seen from the other side: the run was green, and the
    /// scenarios it was asked for wrote no report.
    #[test]
    fn scenarios_that_wrote_no_report_are_what_a_green_run_did_not_verify() {
        let requested = vec!["a".to_string(), "b".to_string()];
        assert_eq!(
            scenarios_not_reported(&requested, &[]),
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(
            scenarios_not_reported(&requested, &[report("a")]),
            vec!["b".to_string()]
        );
        assert!(scenarios_not_reported(&requested, &[report("a"), report("b")]).is_empty());
    }

    /// A scenario is claimed by every feature it verifies, so the map holds
    /// the same name more than once. Asking for it twice would say `257
    /// scenarios asked for` where the repository has 212.
    #[test]
    fn a_scenario_two_features_claim_is_asked_for_once() {
        let mut features = FeatureMap::new();
        for (name, scenarios) in [
            ("oauth", vec!["oauth_rejected_grant_exits_4", "oauth_login"]),
            ("errors", vec!["oauth_rejected_grant_exits_4"]),
        ] {
            features.insert(
                name.to_string(),
                Feature {
                    summary: String::new(),
                    entry: String::new(),
                    files: vec![],
                    tests: vec![],
                    scenarios: scenarios.iter().map(|s| s.to_string()).collect(),
                    depends_on: vec![],
                    notes: None,
                    real: None,
                },
            );
        }
        assert_eq!(
            mapped_scenarios(&features),
            vec![
                "oauth_login".to_string(),
                "oauth_rejected_grant_exits_4".to_string()
            ]
        );
    }

    /// A scenario is named by the last segment whether it is qualified or not.
    #[test]
    fn the_scenario_name_is_the_last_segment_of_the_listed_name() {
        assert_eq!(
            scenario_name("oauth::oauth_rejected_grant_exits_4"),
            "oauth_rejected_grant_exits_4"
        );
        assert_eq!(scenario_name("api_timeout_exits_1"), "api_timeout_exits_1");
    }

    #[test]
    fn not_run_gate_is_distinguished_from_a_pass() {
        let gates = vec![Gate {
            name: "scenarios".into(),
            ok: true,
            detail: "NOT_RUN: no scenarios are mapped".into(),
        }];

        let markdown = render_markdown("affected", "abc1234", false, &gates, &[], false);

        assert!(markdown.contains("Result: **NOT_RUN**"));
        assert!(markdown.contains("| scenarios | NOT_RUN |"));
    }

    /// Two runs of the same scenario on two checkouts print the same line.
    #[test]
    fn a_report_line_keeps_nothing_that_changes_between_runs() {
        let root = Path::new("/w/kurama-28");
        let temp = Path::new("/var/folders/z1/abc/T/");
        let line = "hint: fix /var/folders/z1/abc/T/.tmp4AmzOW/kurama-config.toml; \
                    load /w/kurama-28/tests/fixtures/openapi/invalid.json from \
                    http://127.0.0.1:53133/spec and http://127.0.0.1:53615/x";

        assert_eq!(
            stable_line(line, root, temp),
            "hint: fix <tmp>/kurama-config.toml; load <repo>/tests/fixtures/openapi/invalid.json \
             from http://127.0.0.1:<port>/spec and http://127.0.0.1:<port>/x"
        );
        assert_eq!(
            stable_line("error[X]: plain", root, temp),
            "error[X]: plain"
        );
    }
}
