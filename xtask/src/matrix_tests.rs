//! Tests of `matrix.rs`: the coordinates the inventory yields, the status
//! each row lands in, the layers whose reports are stale, the declarations
//! the gate refuses, and the request flags.

use super::*;
use crate::matrix_inputs::{CaseFile, Commit, Declaration, Report};
use serde_json::json;
use std::collections::BTreeMap;
fn document() -> Value {
    json!({
        "commands": [
            { "path": "api", "hidden": false, "arguments": [] },
            { "path": "status", "hidden": false, "arguments": [
                { "id": "kind", "long": "kind", "values": ["data", "db"] },
                { "id": "json", "long": "json", "values": [] }
            ] },
            { "path": "inventory", "hidden": true, "arguments": [] }
        ],
        "config": { "keys": [
            { "section": "[auth.*]", "key": "kind", "kind": "enum", "values": ["oauth", "token"] },
            { "section": "[[data.*.sources]]", "key": "format", "kind": "enum", "values": ["csv"] },
            { "section": "[s3.*]", "key": "shape", "kind": "enum", "values": ["flat"] },
            { "section": "[api.*]", "key": "base_url", "kind": "string", "values": [] }
        ] },
        "secret_schemes": { "values": ["op"] },
        "client_kinds": { "values": ["db"] }
    })
}

#[test]
fn every_enumerated_value_is_one_coordinate_and_a_hidden_command_is_none() {
    let mut document = document();
    // A subcommand's path is its words; a case names it as one word.
    document["commands"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "path": "config check", "hidden": false, "arguments": [] }));
    let coordinates: Vec<String> = items(&document).iter().map(Item::coordinate).collect();
    assert_eq!(
        coordinates,
        [
            "client=db",
            "command=api",
            "command=config-check",
            "command=status",
            "format=csv",
            "s3.shape=flat",
            "secret=op",
            "source=oauth",
            "source=token",
            "status.kind=data",
            "status.kind=db",
        ]
    );
}

#[test]
fn a_config_key_without_a_name_is_its_section_and_key() {
    assert_eq!(config_dimension("[auth.*]", "kind"), "source");
    assert_eq!(config_dimension("[db.*.tunnel]", "kind"), "tunnel");
    assert_eq!(
        config_dimension("[[data.*.sources]]", "shape"),
        "data.sources.shape"
    );
    assert_eq!(config_dimension("[core]", "log_level"), "core.log_level");
}

fn case(id: &str, combination: &[(&str, &str)], real: bool) -> CaseFile {
    CaseFile {
        id: id.into(),
        feature: "f".into(),
        combination: combination
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        layer_combination: BTreeMap::new(),
        expect: toml::from_str("exit_code = 0\napi_calls = { count = 1 }\nfiles = []").unwrap(),
        layers: Vec::new(),
        stacks: Vec::new(),
        declares_real: real,
        real_requires: Vec::new(),
    }
}

fn report(scenario: &str, passed: bool, evidence: &str) -> Report {
    Report {
        scenario: scenario.into(),
        feature: "f".into(),
        passed,
        evidence: evidence.into(),
        combination: None,
        exit_codes: vec![Some(0)],
        failed_checks: if passed {
            Vec::new()
        } else {
            vec![(
                "expect.exit_code".into(),
                "exit code is 0: observed 2".into(),
            )]
        },
    }
}

fn statuses(matrix: &Matrix) -> Vec<(String, Status)> {
    matrix
        .rows
        .iter()
        .map(|row| (row.id.clone(), row.status))
        .collect()
}

fn declaration(combination: &[(&str, &str)]) -> Declaration {
    Declaration {
        reason: "no".into(),
        prepare: None,
        combination: combination
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    }
}

#[test]
fn each_row_lands_in_one_status() {
    let cases = [
        case("f_pass", &[("command", "api"), ("source", "token")], false),
        case(
            "f_real",
            &[("command", "status"), ("status.kind", "data")],
            true,
        ),
        case("f_never", &[("status.kind", "db")], false),
        case("f_fail", &[("source", "oauth"), ("extra", "yes")], false),
    ];
    let na = [Declaration {
        reason: "no".into(),
        prepare: None,
        combination: [("secret".to_string(), "op".to_string())].into(),
    }];
    let nh = [Declaration {
        reason: "a person".into(),
        prepare: Some("a key".into()),
        combination: [("client".to_string(), "db".to_string())].into(),
    }];
    let reports = [
        report("f_pass", true, "fake"),
        report("f_real", true, "fake"),
        report("f_fail", false, "fake"),
        report("f_rust_only", true, "fake"),
    ];
    let matrix = build(
        items(&document()),
        &cases,
        &na,
        &nh,
        &reports,
        &BTreeMap::new(),
    );
    let rows = statuses(&matrix);
    assert!(rows.contains(&("f_pass".into(), Status::Pass)), "{rows:?}");
    assert!(rows.contains(&("f_real".into(), Status::UnverifiedReal)));
    assert!(rows.contains(&("f_never".into(), Status::Unexecuted)));
    assert!(rows.contains(&("f_fail".into(), Status::Fail)));
    assert!(rows.contains(&("f_rust_only".into(), Status::Pass)));
    assert!(rows.contains(&("secret=op".into(), Status::NotApplicable)));
    assert!(rows.contains(&("client=db".into(), Status::NeedsHuman)));
    assert!(rows.contains(&("format=csv".into(), Status::Unclassified)));
    assert!(rows.contains(&("s3.shape=flat".into(), Status::Unclassified)));
    assert_eq!(matrix.counts["UNCLASSIFIED"], 2);
    assert_eq!(matrix.case_only_dimensions, ["extra"]);
    assert_eq!(matrix.possible_typos, ["extra"]);
    assert_eq!(matrix.gate_problems.len(), 3, "{:?}", matrix.gate_problems);
    assert!(matrix.gate_problems[0].starts_with("FAIL f_fail: expect.exit_code: exit code is 0: observed 2; reproduce: cargo xtask verify f_fail"));
    assert!(
        matrix.gate_problems[1]
            .starts_with("UNCLASSIFIED: format=csv ([[data.*.sources]] format = csv)")
    );
    assert!(
        matrix.gate_problems[2].starts_with("UNCLASSIFIED: s3.shape=flat ([s3.*] shape = flat)")
    );
    let failed = matrix.rows.iter().find(|row| row.id == "f_fail").unwrap();
    assert_eq!(failed.related, Vec::<String>::new());
    assert_eq!(
        failed.expected,
        "api_calls{count=1}, exit_code=0, files=[0]"
    );
    assert_eq!(failed.observed, "fake: exit 0; 1 failed: expect.exit_code");
}

#[test]
fn a_real_report_verifies_a_case_that_declares_one() {
    let cases = [case("f_real", &[("command", "api")], true)];
    let reports = [
        report("f_real", true, "fake"),
        report("f_real", true, "real"),
    ];
    let matrix = build(
        items(&document()),
        &cases,
        &[],
        &[],
        &reports,
        &BTreeMap::new(),
    );
    let row = matrix.rows.iter().find(|row| row.id == "f_real").unwrap();
    assert_eq!(row.status, Status::Pass);
    assert_eq!(row.evidence, "fake, real");
    assert_eq!(row.observed, "fake: exit 0; real: exit 0");
}

#[test]
fn a_failure_names_the_scenarios_that_share_a_coordinate() {
    let cases = [
        case("f_fail", &[("source", "token"), ("secret", "op")], false),
        case("f_sibling", &[("source", "token")], false),
        case("f_other", &[("source", "oauth")], false),
    ];
    let reports = [report("f_fail", false, "fake")];
    let matrix = build(
        items(&document()),
        &cases,
        &[],
        &[],
        &reports,
        &BTreeMap::new(),
    );
    let failed = matrix.rows.iter().find(|row| row.id == "f_fail").unwrap();
    assert_eq!(failed.related, ["f_sibling"]);
}

#[test]
fn an_expected_bearer_token_is_masked_in_the_cell() {
    let expect: toml::Table = toml::from_str(
        "exit_code = 0\napi_calls = { count = 1, authorization = \"Bearer stored-access-token\" }\n",
    )
    .unwrap();
    let cell = expected_cell(&expect);
    assert_eq!(
        cell,
        "api_calls{authorization=<bearer token>, count=1}, exit_code=0"
    );
    assert_eq!(
        mask("{\"authorization\":\"Bearer abc.def\",\"login\":\"x\"} Bearer end"),
        "{\"authorization\":\"<bearer token>\",\"login\":\"x\"} <bearer token>"
    );
}

/// A `[local]` nobody ran leaves the case UNEXECUTED, however green its fake
/// run was, and the row says why `verify --layer local` wrote nothing; a
/// local report with the fake one makes it PASS with both as evidence.
#[test]
fn a_local_layer_nobody_ran_is_unexecuted_with_the_reason() {
    let mut local = case("f_db", &[("command", "db"), ("engine", "sqlite")], false);
    local.layers = vec!["local".into()];
    local.layer_combination = [("engine".to_string(), "postgresql".to_string())].into();
    let cases = [local];
    let not_run = [(
        "local".to_string(),
        "docker is not running: Cannot connect to the Docker daemon".to_string(),
    )]
    .into();

    let matrix = build(
        items(&document()),
        &cases,
        &[],
        &[],
        &[report("f_db", true, "fake")],
        &not_run,
    );
    let row = matrix.rows.iter().find(|row| row.id == "f_db").unwrap();
    assert_eq!(row.status, Status::Unexecuted);
    assert_eq!(row.evidence, "fake");
    assert!(
        row.observed.contains(
            "local: not run (docker is not running: Cannot connect to the Docker daemon)"
        ),
        "{}",
        row.observed
    );
    assert!(
        row.reproduce
            .ends_with("cargo xtask verify --layer local f"),
        "{}",
        row.reproduce
    );
    assert!(
        !matrix
            .rows
            .iter()
            .any(|row| row.status == Status::Unclassified && row.id == "engine=postgresql"),
        "a layer's coordinate is covered by the case"
    );

    let matrix = build(
        items(&document()),
        &cases,
        &[],
        &[],
        &[report("f_db", true, "fake"), report("f_db", true, "local")],
        &BTreeMap::new(),
    );
    let row = matrix.rows.iter().find(|row| row.id == "f_db").unwrap();
    assert_eq!(row.status, Status::Pass);
    assert_eq!(row.evidence, "fake, local");
    assert!(!row.observed.contains("not run"), "{}", row.observed);
}

/// A dimension no rule derives is listed as a possible typo only while one
/// case alone names it: a second case sharing it makes it a refinement.
#[test]
fn a_case_only_dimension_one_case_names_is_a_possible_typo() {
    let one = [case("f_a", &[("command", "api"), ("shape", "flat")], false)];
    let matrix = build(items(&document()), &one, &[], &[], &[], &BTreeMap::new());
    assert_eq!(matrix.case_only_dimensions, ["shape"]);
    assert_eq!(matrix.possible_typos, ["shape"]);

    let two = [
        case("f_a", &[("command", "api"), ("shape", "flat")], false),
        case("f_b", &[("command", "status"), ("shape", "nested")], false),
    ];
    let matrix = build(items(&document()), &two, &[], &[], &[], &BTreeMap::new());
    assert_eq!(matrix.case_only_dimensions, ["shape"]);
    assert!(
        matrix.possible_typos.is_empty(),
        "{:?}",
        matrix.possible_typos
    );
}

/// A `[throwaway]` nobody approved is UNEXECUTED with the approval the
/// runner asked for, and its reproduce line is the approving command; a
/// throwaway report makes it PASS with `fake, throwaway` as evidence.
#[test]
fn a_throwaway_layer_nobody_approved_is_unexecuted_with_the_approval_asked_for() {
    let mut throwaway = case(
        "f_api",
        &[("command", "api"), ("auth", "aws-profile")],
        false,
    );
    throwaway.layers = vec!["throwaway".into()];
    throwaway.stacks = vec!["iam-api".into()];
    throwaway.layer_combination = [("tls".to_string(), "verify-full".to_string())].into();
    let cases = [throwaway];
    let not_run = [(
        "throwaway".to_string(),
        "approval needed: creating these stacks bills the `kurama-sandbox` account; run `cargo xtask verify --layer throwaway all --profile kurama-sandbox --yes`".to_string(),
    )]
    .into();

    let matrix = build(
        items(&document()),
        &cases,
        &[],
        &[],
        &[report("f_api", true, "fake")],
        &not_run,
    );
    let row = matrix.rows.iter().find(|row| row.id == "f_api").unwrap();
    assert_eq!(row.status, Status::Unexecuted);
    assert_eq!(row.evidence, "fake");
    assert!(
        row.observed.contains(
            "throwaway: not run (approval needed: creating these stacks bills the `kurama-sandbox` account"
        ),
        "{}",
        row.observed
    );
    assert!(
        row.reproduce
            .ends_with("cargo xtask verify --layer throwaway f --profile <profile> --yes"),
        "{}",
        row.reproduce
    );
    assert!(
        !matrix
            .rows
            .iter()
            .any(|row| row.status == Status::Unclassified && row.id == "tls=verify-full"),
        "the layer's coordinate is covered by the case"
    );

    let matrix = build(
        items(&document()),
        &cases,
        &[],
        &[],
        &[
            report("f_api", true, "fake"),
            report("f_api", true, "throwaway"),
        ],
        &BTreeMap::new(),
    );
    let row = matrix.rows.iter().find(|row| row.id == "f_api").unwrap();
    assert_eq!(row.status, Status::Pass);
    assert_eq!(row.evidence, "fake, throwaway");
}

/// A layer that failed fails the case, and the gate with it, however green
/// the fake run was.
#[test]
fn a_failed_layer_report_fails_the_case_whatever_the_fake_run_said() {
    let cases = [case("f_real", &[("command", "api")], true)];
    let reports = [
        report("f_real", true, "fake"),
        report("f_real", false, "real"),
    ];
    let matrix = build(
        items(&document()),
        &cases,
        &[],
        &[],
        &reports,
        &BTreeMap::new(),
    );
    let row = matrix.rows.iter().find(|row| row.id == "f_real").unwrap();
    assert_eq!(row.status, Status::Fail);
    assert!(
        matrix
            .gate_problems
            .iter()
            .any(|problem| problem.starts_with("FAIL f_real: expect.exit_code")),
        "{:?}",
        matrix.gate_problems
    );
}

/// A layer report is the case run again elsewhere, not the case itself: with
/// no fake report the case was not executed, and the row says so.
#[test]
fn a_case_with_only_a_layer_report_is_unexecuted() {
    let mut local = case("f_db", &[("command", "api")], false);
    local.layers = vec!["local".into()];
    let matrix = build(
        items(&document()),
        &[local],
        &[],
        &[],
        &[report("f_db", true, "local")],
        &BTreeMap::new(),
    );
    let row = matrix.rows.iter().find(|row| row.id == "f_db").unwrap();
    assert_eq!(row.status, Status::Unexecuted);
    assert!(row.observed.contains("fake: not run"), "{}", row.observed);
}

fn commit(head: &str, dirty: bool) -> Commit {
    Commit {
        head: head.into(),
        dirty,
    }
}

/// A layer's reports count only when its verification report was written on
/// the code the tree holds now (`Commit::holds_the_current_code`, pinned
/// against a scratch repository in `xtask/tests/matrix.rs`): an older run's
/// PASS is set aside, and the case says which commit it came from instead of
/// counting it.
#[test]
fn a_layer_run_on_another_commit_is_set_aside_as_stale() {
    let reports = vec![
        report("f_real", true, "fake"),
        report("f_real", true, "real"),
        report("f_db", true, "fake"),
        report("f_db", true, "local"),
        report("f_api", true, "fake"),
        report("f_api", true, "throwaway"),
    ];
    let commits = [
        ("real".to_string(), commit("a1ab2e0", false)),
        ("local".to_string(), commit("347c547", false)),
    ]
    .into();
    let current = |c: &Commit| c.head == "347c547";
    let mut not_run = [("real".to_string(), "f_real: requires x".to_string())].into();
    let kept = set_aside_stale_layers(reports, &mut not_run, &commits, current);
    let kept: Vec<(&str, &str)> = kept
        .iter()
        .map(|r| (r.scenario.as_str(), r.evidence.as_str()))
        .collect();
    assert_eq!(
        kept,
        [
            ("f_real", "fake"),
            ("f_db", "fake"),
            ("f_db", "local"),
            ("f_api", "fake"),
        ]
    );
    assert_eq!(
        not_run["real"],
        "stale: its reports were written on a1ab2e0, not on the code the tree holds now"
    );
    assert_eq!(
        not_run["throwaway"],
        "stale: no verification-report-throwaway.json says which commit its reports were written on"
    );
    assert!(!not_run.contains_key("local"));

    // Set aside, the real report no longer verifies the case.
    let cases = [case("f_real", &[("command", "api")], true)];
    let mut not_run = BTreeMap::new();
    let kept = set_aside_stale_layers(
        vec![
            report("f_real", true, "fake"),
            report("f_real", true, "real"),
        ],
        &mut not_run,
        &[("real".to_string(), commit("a1ab2e0", false))].into(),
        current,
    );
    let matrix = build(items(&document()), &cases, &[], &[], &kept, &not_run);
    let row = matrix.rows.iter().find(|row| row.id == "f_real").unwrap();
    assert_eq!(row.status, Status::UnverifiedReal);
    assert!(
        row.observed.contains(
            "real: not run (stale: its reports were written on a1ab2e0, not on the code the tree holds now)"
        ),
        "{}",
        row.observed
    );
}

/// A declaration classifies one coordinate: one with two would classify
/// each of them alone, which it never said.
#[test]
fn a_declaration_names_exactly_one_coordinate() {
    let matrix = build(
        items(&document()),
        &[],
        &[declaration(&[("secret", "op"), ("client", "db")])],
        &[declaration(&[])],
        &[],
        &BTreeMap::new(),
    );
    let refused: Vec<&String> = matrix
        .gate_problems
        .iter()
        .filter(|problem| {
            problem.contains("declares 2 coordinates") || problem.contains("declares 0 coordinates")
        })
        .collect();
    assert_eq!(refused.len(), 2, "{:?}", matrix.gate_problems);
    assert!(
        refused[0].starts_with(&format!(
            "{NOT_APPLICABLE_FILE}: `client=db, secret=op` declares 2 coordinates"
        )),
        "{refused:?}"
    );
    assert!(
        refused[1].starts_with(&format!("{NEEDS_HUMAN_FILE}: `-` declares 0 coordinates")),
        "{refused:?}"
    );
}

/// A value on a dimension the inventory enumerates, that the inventory does
/// not have, is a misspelling or a value that no longer exists: a
/// declaration of one fails the gate, and a case naming one (a deliberate
/// invalid value, or a refinement such as `source=aws-profile`) is listed.
#[test]
fn a_value_the_inventory_does_not_enumerate() {
    let cases = [case(
        "f_rejected",
        &[("command", "api"), ("secret", "unknown-scheme")],
        false,
    )];
    let matrix = build(
        items(&document()),
        &cases,
        &[declaration(&[("source", "tokn")])],
        &[declaration(&[("shape", "anything")])],
        &[report("f_rejected", true, "fake")],
        &BTreeMap::new(),
    );
    assert_eq!(matrix.unenumerated_case_values, ["secret=unknown-scheme"]);
    let unenumerated: Vec<&String> = matrix
        .gate_problems
        .iter()
        .filter(|problem| problem.contains("the inventory does not enumerate"))
        .collect();
    assert_eq!(
        unenumerated,
        [&format!(
            "{NOT_APPLICABLE_FILE}: `source=tokn` is a value the inventory does not enumerate for `source` (oauth, token)"
        )],
        "{:?}",
        matrix.gate_problems
    );
}
