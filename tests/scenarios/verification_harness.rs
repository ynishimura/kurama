//! The harness's own scenario: a case declared as data runs the real binary
//! through the same sandbox and fakes as a Rust scenario, its report carries
//! the contract the matrix reads (`combination`, `evidence`), and a wrong
//! expectation is reported by the key that held it with what was observed.

use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::support::cases;
use crate::support::*;

/// A case the binary answers without any credential: an API that names both
/// a token source and an AWS profile is refused when the configuration is
/// read, so the run is one process and no fake is called.
const CASE: &str = r#"
id = "verification_cases_execute_real_binary_and_compare_contract"
feature = "verification-harness"

[combination]
command = "api"
auth = "oauth-and-aws-profile"

[input]
args = ["api", "both", "/items"]
config = """
[auth.svc]
kind = "oauth"
grant_type = "client_credentials"
token_url = "{server}/oauth/token"
client_id = "svc-client"
client_secret = "op://Agent/Service/client_secret"

[api.both]
base_url = "{server}/api"
auth = "svc"
aws_profile = "dev"
"""

[expect]
error = { code = "CONFIG_INVALID", exit = 2 }
sts_actions = []
token_grants = []
api_calls = { count = 0 }
no_files_written = true
"#;

#[test]
fn verification_cases_execute_real_binary_and_compare_contract() {
    let path = "tests/scenarios/verification_harness.rs";
    let mut v = cases::verification(CASE, path);
    let (combination, evidence) = v.contract();
    let contract_ok = combination.is_some_and(|c| {
        c.get("command").map(String::as_str) == Some("api")
            && c.get("auth").map(String::as_str) == Some("oauth-and-aws-profile")
    }) && evidence == "fake";
    let contract = format!("combination {combination:?}, evidence {evidence:?}");
    v.check(
        "the report carries the case's combination and the evidence it ran against",
        contract_ok,
        contract,
    );

    // The same case with one expectation wrong, run to the same
    // verification but never finished: what it reports is the test.
    let wrong = CASE
        .replace(
            "id = \"verification_cases_execute_real_binary_and_compare_contract\"",
            "id = \"verification_probe_with_a_wrong_expectation\"",
        )
        .replace(
            "error = { code = \"CONFIG_INVALID\", exit = 2 }",
            "error = { code = \"CONFIG_INVALID\", exit = 3 }",
        )
        .replace("sts_actions = []", "sts_actions = [\"AssumeRole\"]");
    let failures = cases::verification(&wrong, path).failed_checks();
    let names_the_key = |key: &str| {
        failures
            .iter()
            .any(|line| line.starts_with(&format!("{key}: ")) && line.contains("observed"))
    };
    v.check(
        "a wrong expectation is reported as its key, the expectation and the observed value",
        failures.len() == 2 && names_the_key("expect.error") && names_the_key("expect.sts_actions"),
        format!("failures {failures:?}"),
    );
    v.finish();

    // After `finish`, the file itself: the matrix reads this, not the struct.
    let report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            report_dir().join("verification_cases_execute_real_binary_and_compare_contract.json"),
        )
        .expect("the case wrote its report"),
    )
    .unwrap();
    assert_eq!(report["combination"]["command"], "api");
    assert_eq!(report["evidence"], "fake");
}

/// A case whose coordinates name the profile's role and region: the report
/// carries them as the `combination` the matrix classifies, and its observed
/// calls say which role was assumed, where STS was called, and where the
/// request was signed, next to the files the run wrote and the secrets it
/// left on disk (none).
const SIGNED_CASE: &str = r#"
id = "verification_matrix_records_profile_role_region_and_side_effects"
feature = "verification-harness"

[combination]
command = "api"
auth = "aws-profile"
role = "assume-role"
region = "profile"
service = "configured"

[input]
args = ["api", "apigw", "/items"]
config = """
[api.apigw]
base_url = "{server}/api"
aws_profile = "dev"
service = "execute-api"
"""

[fakes]
api = "ok"

[expect]
exit_code = 0
sts_actions = ["AssumeRole"]
sts_calls = [{ action = "AssumeRole", role_arn = "arn:aws:iam::123456789012:role/Dev", serial_number = "<none>" }]
sts_signing_region = "us-east-1"
api_calls = { count = 1, sigv4 = { service = "execute-api", region = "ap-northeast-1" } }
token_grants = []
op_calls = 0
no_files_written = true
"#;

#[test]
fn verification_matrix_records_profile_role_region_and_side_effects() {
    let path = "tests/scenarios/verification_harness.rs";
    let mut v = cases::verification(SIGNED_CASE, path);
    let (combination, _) = v.contract();
    let coordinates = combination.cloned().unwrap_or_default();
    v.check(
        "the report's combination names the role and region coordinates the case declared",
        ["auth", "role", "region", "service"]
            .iter()
            .all(|key| coordinates.contains_key(*key)),
        format!("combination {coordinates:?}"),
    );
    v.finish();

    let report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            report_dir()
                .join("verification_matrix_records_profile_role_region_and_side_effects.json"),
        )
        .expect("the case wrote its report"),
    )
    .unwrap();
    let run = &report["observed"]["runs"][0];
    assert_eq!(report["combination"]["role"], "assume-role");
    assert_eq!(report["combination"]["region"], "profile");
    assert_eq!(
        run["sts_calls"][0]["role_arn"],
        "arn:aws:iam::123456789012:role/Dev"
    );
    assert_eq!(run["sts_calls"][0]["signing_region"], "us-east-1");
    assert_eq!(run["api_calls"][0]["sigv4"]["region"], "ap-northeast-1");
    assert_eq!(run["api_calls"][0]["sigv4"]["service"], "execute-api");
    assert_eq!(report["observed"]["files_written"], serde_json::json!([]));
    assert_eq!(report["observed"]["secrets_on_disk"], serde_json::json!([]));
}

/// More than a pipe holds (64KiB on macOS), so writing it blocks until the
/// child reads.
const PIPE_OVERFLOW: usize = 1 << 20;

/// The stdin a scenario gives is written under the same deadline as the run:
/// a child that never reads it is killed at the deadline instead of holding
/// the writer (and the suite) forever, and a child that exits without
/// reading it is an ordinary run, not a panic of the harness.
#[test]
fn verification_harness_stdin_delivery_is_timeout_bounded() {
    let input = "x".repeat(PIPE_OVERFLOW);
    // `agent --skill` prints a page and exits without reading stdin.
    let mut v = run(Scenario::new(
        "verification_harness_stdin_delivery_is_timeout_bounded",
        "verification-harness",
        &["agent", "--skill"],
    )
    .with_stdin(input.clone()));
    v.expect_exit_code(0).expect_no_files_written();

    let limit = Duration::from_secs(1);
    let mut sleeper = Command::new("/bin/sleep");
    sleeper.arg("5");
    let started = Instant::now();
    let output = run_with_deadline(sleeper, Some(&input), limit);
    let elapsed = started.elapsed();
    v.check(
        "a child that never reads its stdin is killed at the deadline",
        output.timed_out && output.exit_code.is_none() && elapsed < Duration::from_secs(4),
        format!(
            "timed out {}, exit {:?}, after {elapsed:?}",
            output.timed_out, output.exit_code
        ),
    );
    v.finish();
}

/// Each credential the fake 1Password CLI hands out, in a file that is not
/// UTF-8: the scan reads bytes, and knows every synthetic credential.
const SEEDED_OP_SECRETS: [(&str, &[u8]); 3] = [
    (
        ".cache/client-secret.bin",
        b"\xff\xfe fake-client-secret \xff",
    ),
    (".cache/access-key.bin", b"\xff\xfe AKIAOPSOURCEKEY \xff"),
    (".cache/secret-key.bin", b"\xff\xfe op-source-secret \xff"),
];

/// The on-disk secret scan fails closed: a file that holds a synthetic
/// credential is found whatever its encoding, and a path the scan cannot
/// list or read is a failed scan rather than a clean one.
#[test]
fn verification_harness_secret_scan_fails_closed() {
    let mut v = run(Scenario::new(
        "verification_harness_secret_scan_fails_closed",
        "verification-harness",
        &["agent", "--skill"],
    ));
    v.expect_exit_code(0).expect_no_files_written();

    // A scenario with the secrets seeded, run to the same verification but
    // never finished: what it reports is the test.
    let seeded = SEEDED_OP_SECRETS.iter().fold(
        Scenario::new(
            "verification_probe_with_secrets_on_disk",
            "verification-harness",
            &["agent", "--skill"],
        ),
        |scenario, (path, bytes)| scenario.with_home_bytes(path, bytes),
    );
    let probe = run(seeded);
    let found = probe.observed.secrets_on_disk.clone();
    let failures = probe.failed_checks();
    v.check(
        "every credential the fake op hands out is found on disk, in bytes that are not UTF-8",
        SEEDED_OP_SECRETS
            .iter()
            .all(|(path, _)| found.contains(&format!("home/{path}")))
            && failures
                .iter()
                .any(|line| line.starts_with("credentials and tokens are not written to disk")),
        format!("found {found:?}, failures {failures:?}"),
    );

    // A directory the walk cannot list and a file it cannot read.
    let dir = tempfile::tempdir().unwrap();
    let locked = dir.path().join("locked");
    let sealed = dir.path().join("sealed");
    std::fs::create_dir(&locked).unwrap();
    std::fs::write(locked.join("inside"), FAKE_CLIENT_SECRET).unwrap();
    std::fs::write(&sealed, FAKE_CLIENT_SECRET).unwrap();
    for path in [&locked, &sealed] {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).unwrap();
    }
    let (files, unlisted) = snapshot(dir.path());
    let (_, unread) = scan_for_secrets(files.keys(), &[FAKE_CLIENT_SECRET.to_string()], false);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
    v.check(
        "a directory the scan cannot list and a file it cannot read are reported, not passed",
        unlisted == [locked.clone()] && unread == [sealed.clone()],
        format!("unlisted {unlisted:?}, unread {unread:?}"),
    );
    v.finish();
}

/// A case has to expect something of every layer it runs on: the fake layer
/// always runs it, so `[expect]` can never be left out or empty, a layer
/// block replaces `[expect]` on its layer and has to expect something of its
/// own, and a `[[expect.runs]]` block that names only its run says nothing.
#[test]
fn verification_harness_rejects_cases_without_expectations() {
    let path = "tests/scenarios/verification_harness.rs";
    let case = CASE.replace(
        "id = \"verification_cases_execute_real_binary_and_compare_contract\"",
        "id = \"verification_harness_rejects_cases_without_expectations\"",
    );
    let mut v = cases::verification(&case, path);
    let expect = "[expect]\nerror = { code = \"CONFIG_INVALID\", exit = 2 }\nsts_actions = []\ntoken_grants = []\napi_calls = { count = 0 }\nno_files_written = true\n";
    assert!(case.contains(expect), "the case this starts from");
    let variants = [
        ("no [expect]", case.replace(expect, "")),
        ("an empty [expect]", case.replace(expect, "[expect]\n")),
        (
            "an [expect] whose keys check nothing",
            case.replace(
                expect,
                "[expect]\nstderr_empty = false\nstdout_excludes = []\n",
            ),
        ),
        (
            "an [expect] with a run block that names only its run",
            case.replace(expect, "[expect]\n[[expect.runs]]\nrun = 0\n"),
        ),
        (
            "a layer that expects nothing",
            format!("{case}\n[real]\nrequires = [\"env:HOME\"]\n"),
        ),
    ];
    let accepted: Vec<&str> = variants
        .iter()
        .filter(|(_, text)| {
            cases::refusal(text, path).is_none_or(|reason| !reason.contains("expects nothing"))
        })
        .map(|(name, _)| *name)
        .collect();
    v.check(
        "a case, a run block or a layer that expects nothing is refused when it is read",
        accepted.is_empty(),
        format!("read without a refusal: {accepted:?}"),
    );
    v.check(
        "the case they were made from is read",
        cases::refusal(&case, path).is_none(),
        "the unchanged case",
    );
    v.finish();
}

#[path = "../architecture/sigint_listener.rs"]
mod sigint_listener;

/// ARCH-043's detector refuses a `ctrl_c()` listener polled first after its
/// work may have started, in every shape the rule's fixtures pin, accepts the
/// bound-then-selected shape, and finds nothing in `db.rs`, which registers
/// `signal(SignalKind::interrupt())` before it starts instead.
#[test]
fn verification_harness_architecture_detects_late_ctrl_c_listener() {
    let mut v = run(Scenario::new(
        "verification_harness_architecture_detects_late_ctrl_c_listener",
        "verification-harness",
        &["agent", "--skill"],
    ));
    v.expect_exit_code(0).expect_no_files_written();
    let missed: Vec<&str> = sigint_listener::LATE_LISTENERS
        .into_iter()
        .filter(|fixture| sigint_listener::late_ctrl_c_listeners(fixture).is_empty())
        .collect();
    v.check(
        "every late listener is found",
        missed.is_empty(),
        format!("missed: {missed:?}"),
    );
    let db = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/shell/cli/commands/db.rs"),
    )
    .unwrap();
    let refused = [
        sigint_listener::late_ctrl_c_listeners(sigint_listener::REGISTERED_WITH_THE_WORK),
        sigint_listener::late_ctrl_c_listeners(&db),
    ];
    v.check(
        "the bound-then-selected listener is accepted, and db.rs has no late one",
        refused.iter().all(Vec::is_empty) && db.contains("SignalKind::interrupt"),
        format!("lines refused: {refused:?}"),
    );
    v.finish();
}

#[path = "../architecture/error_code_exemptions.rs"]
mod error_code_exemptions;

/// ARCH-040's check of the error codes no scenario pins refuses an
/// exemption that repeats, gives no reason, names no code of
/// `error_code.rs`, or names a code a scenario pins already, and accepts a
/// sound one.
#[test]
fn verification_harness_architecture_rejects_invalid_error_code_exemptions() {
    use error_code_exemptions::*;
    let mut v = run(Scenario::new(
        "verification_harness_architecture_rejects_invalid_error_code_exemptions",
        "verification-harness",
        &["agent", "--skill"],
    ));
    v.expect_exit_code(0).expect_no_files_written();
    let refused = error_code_exemption_problems(&UNSOUND, FIXTURE_ERROR_CODES, FIXTURE_SCENARIOS);
    v.check(
        "a repeated, unexplained, unknown and pinned exemption are each refused by name",
        refused == UNSOUND_PROBLEMS,
        format!("problems {refused:?}"),
    );
    let accepted = error_code_exemption_problems(&SOUND, FIXTURE_ERROR_CODES, FIXTURE_SCENARIOS);
    v.check(
        "a real code no scenario expects, with its reasons, is accepted",
        accepted.is_empty(),
        format!("problems {accepted:?}"),
    );
    v.finish();
}
