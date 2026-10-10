//! Tests of `verify_real.rs`: the declarations, the judgement of a probe's
//! output, the freshness of evidence, the credential scan, and the checked-in
//! feature map.

use super::*;
use crate::real_declaration::{Contract, contract_coverage, declaration_problems};

fn probe(contracts: Vec<Contract>) -> Probe {
    Probe {
        command: vec!["cargo".into(), "test".into()],
        source: "tests/real_db.rs".into(),
        environment: "local-docker".into(),
        access: Access::Throwaway,
        requires: vec!["KURAMA_TEST_DB".into()],
        fixtures: vec![],
        contracts,
    }
}

fn contract(real: &str, mock: &[&str]) -> Contract {
    Contract {
        real: real.into(),
        mock: mock.iter().map(|m| m.to_string()).collect(),
        note: None,
        requires: vec![],
    }
}

fn feature(real: Option<Real>) -> Feature {
    Feature {
        summary: "s".into(),
        entry: "src/a.rs".into(),
        docs: vec![],
        files: vec![],
        tests: vec![],
        scenarios: vec!["db_mock".into()],
        depends_on: vec![],
        notes: None,
        real,
    }
}

const OUTPUT: &str = "\
running 3 tests
test db_real_passes ... ok
test db_real_says_not_run ... ok
test db_real_fails ... FAILED

successes:

---- db_real_passes stdout ----

---- db_real_says_not_run stdout ----
db_real_says_not_run: not run; start the databases

successes:
    db_real_passes
";

#[test]
fn a_probe_is_verified_only_when_every_contract_that_could_run_passed_and_ran() {
    let passing = probe(vec![contract("db_real_passes", &["db_mock"])]);
    let (status, reason, observed) = judge(&passing, OUTPUT, Ok(()), &BTreeSet::new());
    assert_eq!((status, reason), (Status::Verified, None));
    assert_eq!(observed[0].result, "passed");

    let said_not_run = probe(vec![
        contract("db_real_passes", &["db_mock"]),
        contract("db_real_says_not_run", &["db_mock"]),
    ]);
    let (status, reason, observed) = judge(&said_not_run, OUTPUT, Ok(()), &BTreeSet::new());
    assert_eq!(status, Status::Unverified);
    assert_eq!(reason.as_deref(), Some("1 contract(s) did not run"));
    assert_eq!(observed[1].result, "not_run");

    let failing = probe(vec![contract("db_real_fails", &["db_mock"])]);
    let (status, reason, _) = judge(&failing, OUTPUT, Err("exit 101".into()), &BTreeSet::new());
    assert_eq!(
        (status, reason.as_deref()),
        (Status::Failed, Some("1 contract(s) failed"))
    );
}

#[test]
fn a_contract_whose_variable_is_unset_is_skipped_and_a_missing_result_is_not_run() {
    let mut bastion = contract("db_real_says_not_run", &["db_mock"]);
    bastion.requires = vec!["KURAMA_TEST_BASTION".into()];
    let skipped = probe(vec![contract("db_real_passes", &["db_mock"]), bastion]);
    let (status, _, observed) = judge(&skipped, OUTPUT, Ok(()), &BTreeSet::new());
    assert_eq!(status, Status::Verified);
    assert_eq!(observed[1].result, "skipped");

    let absent = probe(vec![contract("db_real_absent", &["db_mock"])]);
    let (status, reason, _) = judge(
        &absent,
        "",
        Err("timed out after 3600s".into()),
        &BTreeSet::new(),
    );
    assert_eq!(status, Status::Unverified);
    assert_eq!(
        reason.as_deref(),
        Some("1 contract(s) did not run: timed out after 3600s")
    );

    let nothing_ran = probe(vec![]);
    let (status, reason, _) = judge(&nothing_ran, OUTPUT, Ok(()), &BTreeSet::new());
    assert_eq!(
        (status, reason.as_deref()),
        (Status::Unverified, Some("no contract ran"))
    );

    let exit_failed = probe(vec![contract("db_real_passes", &["db_mock"])]);
    let (status, _, _) = judge(&exit_failed, OUTPUT, Err("exit 1".into()), &BTreeSet::new());
    assert_eq!(status, Status::Failed);
}

#[test]
fn every_way_a_declaration_can_be_wrong_is_named() {
    let exists = |path: &str| path == "tests/real_db.rs";
    assert_eq!(
        declaration_problems("a", &feature(None), &exists),
        ["a: declare `real` in .agent/features/a.toml: a probe, `pending` or `not_applicable`"]
    );
    let unexplained = feature(Some(Real::Pending {
        pending: " ".into(),
    }));
    assert_eq!(
        declaration_problems("a", &unexplained, &exists),
        ["a: say why under `real`"]
    );
    let explained = feature(Some(Real::NotApplicable {
        not_applicable: "pure".into(),
    }));
    assert!(declaration_problems("a", &explained, &exists).is_empty());

    let mut bad = probe(vec![
        contract("db_real_x", &["db_not_a_scenario"]),
        contract("db_real_x", &["db_mock"]),
        contract("db_real_y", &[]),
    ]);
    bad.environment = "production".into();
    bad.requires = vec![];
    bad.command = vec![];
    bad.fixtures = vec!["tests/db/gone.sql".into()];
    assert_eq!(
        declaration_problems("a", &feature(Some(Real::Probe(bad))), &exists),
        [
            "a: the probe names no command",
            "a: the probe requires no variable, so nothing keeps it from running unasked",
            "a: `production` is not a throwaway environment; a probe may write only to [\"local-docker\"]",
            "a: tests/db/gone.sql does not exist",
            "a: db_real_x pairs with db_not_a_scenario, which is not a scenario of a",
            "a: db_real_x is a contract twice",
            "a: db_real_y names no mock scenario and no note saying why",
        ]
    );
    let sound = probe(vec![contract("db_real_x", &["db_mock"])]);
    assert!(declaration_problems("a", &feature(Some(Real::Probe(sound))), &exists).is_empty());
}

#[test]
fn every_test_of_the_probe_is_a_contract_and_every_contract_a_test() {
    let real = Real::Probe(probe(vec![
        contract("db_real_x", &["db_mock"]),
        contract("db_real_gone", &["db_mock"]),
    ]));
    let tests: BTreeSet<String> = ["db_real_x", "db_real_new"].map(String::from).into();
    assert_eq!(
        contract_coverage("a", &tests, &real),
        [
            "a: tests/real_db.rs test db_real_new is no contract",
            "a: contract db_real_gone is not a test of tests/real_db.rs",
        ]
    );
}

fn evidence(status: Status, hash: &str) -> Evidence {
    Evidence {
        schema_version: 1,
        feature: "database".into(),
        status,
        reason: None,
        commit: "0123456789abcdef".into(),
        feature_hash: hash.into(),
        diff_hash: hash_text(""),
        environment: "local-docker".into(),
        access: Access::Throwaway,
        command: vec!["cargo".into()],
        generated_at: "2026-09-23T00:00:00Z".into(),
        observed: vec![Observed {
            test: "db_real_x".into(),
            result: "passed".into(),
            mock: vec!["db_mock".into()],
        }],
        fixtures: BTreeMap::new(),
    }
}

#[test]
fn evidence_holds_only_when_verified_on_these_files_and_these_fixtures() {
    let none = BTreeMap::new();
    assert_eq!(
        judge_evidence(
            "database",
            Some(&evidence(Status::Verified, "h")),
            "h",
            &none
        ),
        Ok("database: verified at 0123456 on local-docker".into())
    );
    let hint = "run `cargo xtask verify-real database` against its environment";
    assert_eq!(
        judge_evidence("database", None, "h", &none),
        Err(format!("database: no real-environment evidence; {hint}"))
    );
    assert_eq!(
        judge_evidence(
            "database",
            Some(&evidence(Status::Verified, "old")),
            "h",
            &none
        ),
        Err(format!(
            "database: the evidence was taken on other files than these; {hint}"
        ))
    );
    assert!(
        judge_evidence(
            "database",
            Some(&evidence(Status::Unverified, "h")),
            "h",
            &none
        )
        .unwrap_err()
        .starts_with("database: the last real run is Unverified")
    );
    let changed = BTreeMap::from([("tests/db/mysql.sql".to_string(), "x".to_string())]);
    assert_eq!(
        judge_evidence(
            "database",
            Some(&evidence(Status::Verified, "h")),
            "h",
            &changed
        ),
        Err(format!(
            "database: a fixture changed since the evidence was taken; {hint}"
        ))
    );
    let mut leaked = evidence(Status::Verified, "h");
    leaked.reason = Some("Authorization: Bearer abc".into());
    assert_eq!(
        judge_evidence("database", Some(&leaked), "h", &none),
        Err("database: the evidence holds a bearer token, an Authorization header".into())
    );
}

#[test]
fn a_credential_is_found_and_ordinary_text_is_not() {
    assert_eq!(
        credential_marks("key AKIAIOSFODNN7EXAMPLE here"),
        ["an AWS access key id"]
    );
    assert_eq!(
        credential_marks("ASIAABCDEFGHIJKLMNOP -----BEGIN RSA PRIVATE KEY-----"),
        ["an AWS access key id", "a PEM block"]
    );
    assert_eq!(
        credential_marks("aws_secret_access_key SecretAccessKey x-amz-security-token"),
        [
            "a secret access key",
            "a secret access key",
            "a session token"
        ]
    );
    assert!(credential_marks("AKIA is a prefix; db_real_x passed").is_empty());
}

#[test]
fn a_github_token_and_an_api_key_header_value_are_found() {
    for text in [
        "ghp_16C7e42F292c6912E7710c838347Ae178B4a",
        "token=gho_16C7e42F292c6912E7710c838347Ae178B4a",
        "github_pat_11ABCDEFG0123456789_abcdefghijklmnopqrstuvwxyz",
    ] {
        assert_eq!(credential_marks(text), ["a GitHub token"], "{text}");
    }
    for text in [
        "X-Api-Key: key-secret-value",
        "> xi-api-key: sk_0123456789abcdef",
        r#"{"x-api-key": "key-secret-value"}"#,
    ] {
        assert_eq!(credential_marks(text), ["an API key header"], "{text}");
    }
    for text in [
        "ghp_ is a prefix, ghp_short",
        "> xi-api-key: ****",
        r#"header = "xi-api-key""#,
        "headers cannot set xi-api-key: [auth.example] sends its credential in it",
    ] {
        assert!(credential_marks(text).is_empty(), "{text}");
    }
}

#[test]
fn the_gate_needs_fresh_evidence_for_a_probe_and_only_names_a_pending_feature() {
    let features: FeatureMap = BTreeMap::from([
        (
            "pure".to_string(),
            feature(Some(Real::NotApplicable {
                not_applicable: "x".into(),
            })),
        ),
        (
            "later".to_string(),
            feature(Some(Real::Pending {
                pending: "no probe".into(),
            })),
        ),
        ("undeclared".to_string(), feature(None)),
    ]);
    assert_eq!(
        gate(&features, &["pure".into()]),
        Ok("no changed feature talks to a real environment".into())
    );
    assert_eq!(
        gate(&features, &["pure".into(), "later".into()]),
        Ok("unverified, no probe registered: later".into())
    );
    assert_eq!(
        gate(&features, &["undeclared".into()]),
        Err("undeclared: declare `real` in .agent/features/undeclared.toml: a probe, `pending` or `not_applicable`".into())
    );
}

#[test]
fn the_hash_is_stable_and_tells_texts_apart() {
    assert_eq!(hash_text(""), "cbf29ce484222325");
    assert_eq!(hash_text("a"), "af63dc4c8601ec8c");
    assert_eq!(hash_text("foobar"), "85944171f73967e8");
    assert_eq!(hash_text("a"), hash_text("a"));
    assert_ne!(hash_text("a\0b"), hash_text("ab\0"));
}

/// Every feature of the checked-in map declares how it is verified for real,
/// and the probe of each one names every test of its source as a contract.
#[test]
fn every_feature_declares_how_it_is_verified_for_real() {
    let features = load_features().unwrap();
    let mut problems = Vec::new();
    for (name, feature) in &features {
        problems.extend(all_problems(name, feature));
    }
    assert_eq!(problems, Vec::<String>::new());
    assert!(
        features
            .values()
            .any(|feature| matches!(feature.real, Some(Real::Probe(_)))),
        "at least one feature has a probe"
    );
}
