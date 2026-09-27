//! `cargo xtask verify-matrix` run as the binary over a scratch tree: an
//! inventory document (`--from`), case files, the declarations and the
//! scenario reports a `verify` run left, and what the gate says about them.

use std::path::PathBuf;
use std::process::{Command, Output};

mod support;

/// An inventory the way `kurama inventory` prints one: two commands, one
/// enum key with three values, one secret key, two schemes, one client kind.
const DOCUMENT: &str = r#"{
  "schema_version": 1,
  "binary": { "name": "kurama", "version": "0.0.0-test" },
  "commands": [
    { "path": "api", "hidden": false, "about": "call", "source": "src/shell/cli/args.rs: build_command",
      "arguments": [
        { "id": "api", "positional": true, "long": null, "short": null, "value_name": "API", "required": true, "takes_value": true, "values": [], "value_hint": "Unknown", "completer": true }
      ] },
    { "path": "status", "hidden": false, "about": "status", "source": "src/shell/cli/args.rs: build_command",
      "arguments": [
        { "id": "kind", "positional": false, "long": "kind", "short": null, "value_name": null, "required": false, "takes_value": true, "values": ["data", "db"], "value_hint": "Other", "completer": false }
      ] },
    { "path": "inventory", "hidden": true, "about": "this", "source": "src/shell/cli/args.rs: build_command", "arguments": [] }
  ],
  "config": {
    "source": "the config types",
    "keys": [
      { "section": "[api.*]", "key": "base_url", "kind": "string", "values": [] },
      { "section": "[auth.*]", "key": "kind", "kind": "enum", "values": ["oauth", "token", "freshly_added_kind"] },
      { "section": "[auth.*]", "key": "token", "kind": "secret", "values": [] }
    ]
  },
  "secret_schemes": { "source": "secret_ref.rs", "values": ["op", "aws-ssm"] },
  "client_kinds": { "source": "client.rs", "values": ["data"] }
}"#;

/// A case that covers `command=api`, `source=token`, `secret=aws-ssm`.
const CASE_TOKEN: &str = r#"
id = "probed_token_over_ssm"
feature = "probed"

[combination]
command = "api"
source = "token"
secret = "aws-ssm"
api_response = "unauthorized"

[input]
args = ["api", "example", "/voices"]

[expect]
exit_code = 4
error = "API_REJECTED"
"#;

/// A case that declares a real run nothing has done yet.
const CASE_WITH_REAL: &str = r#"
id = "probed_status_data"
feature = "probed"

[combination]
command = "status"
"status.kind" = "data"
client = "data"

[input]
args = ["status", "--kind", "data"]

[expect]
exit_code = 0

[real]
requires = ["a real account"]
"#;

/// Everything the document has that no case names, declared not applicable
/// or needing a person -- except `source=freshly_added_kind`.
const NOT_APPLICABLE: &str = r#"
[[not_applicable]]
reason = "the device flow needs a person at the browser; it is a needs-human entry elsewhere"
[not_applicable.combination]
source = "oauth"

[[not_applicable]]
reason = "the 1Password path is verified by the secrets feature's own scenarios"
[not_applicable.combination]
secret = "op"
"#;

const NEEDS_HUMAN: &str = r#"
[[needs_human]]
reason = "the db status needs a database a person configures"
prepare = "a [db.*] section pointing at a reachable server"
[needs_human.combination]
"status.kind" = "db"
"#;

fn report_json(scenario: &str, passed: bool, failed_check: Option<(&str, &str)>) -> String {
    let checks = match failed_check {
        Some((name, detail)) => format!(
            r#"[{{"name":"every run finishes within 60s","ok":true,"detail":"timed out runs: []"}},{{"name":"{name}","ok":false,"detail":"{detail}"}}]"#
        ),
        None => {
            r#"[{"name":"every run finishes within 60s","ok":true,"detail":"timed out runs: []"}]"#
                .to_string()
        }
    };
    let exit = if passed { 4 } else { 2 };
    format!(
        r#"{{"schema_version":1,"scenario":"{scenario}","feature":"probed","command":["kurama","api"],"passed":{passed},"combination":null,"evidence":"fake","checks":{checks},"observed":{{"runs":[{{"exit_code":{exit}}}],"files_written":[]}},"seeded":{{}}}}"#
    )
}

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

    fn write(&self, relative: &str, content: &str) {
        let path = self.repo().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(self.repo())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// Declares `freshly_added_kind` and commits it, so the tree is clean.
    fn classify_freshly_added(&self) {
        self.write(
            "tests/cases/not-applicable.toml",
            &format!("{NOT_APPLICABLE}{FRESHLY_ADDED_NOT_APPLICABLE}"),
        );
        self.git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-qam",
            "classify",
        ]);
    }

    /// The verification report a `verify --layer <layer>` run writes, dated
    /// with `head` and whether the tree was dirty then.
    fn layer_ran_on(&self, layer: &str, head: &str, dirty: bool) {
        self.write(
            &format!("target/agent/verification-report-{layer}.json"),
            &format!(r#"{{"schema_version":2,"git":{{"head":"{head}","dirty":{dirty}}}}}"#),
        );
    }

    fn commit_all(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-qm",
            message,
        ]);
    }

    fn report(&self, name: &str) -> String {
        std::fs::read_to_string(self.repo().join("target/agent").join(name)).unwrap()
    }
}

/// A repository with one feature, the two cases, both declaration files and
/// a passing report for the token case, committed so a layer's report can
/// name the commit it ran on.
fn scratch(name: &str) -> Scratch {
    let dir = std::env::temp_dir().join(format!("xtask-matrix-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let scratch = Scratch(dir);
    scratch.write(
        ".agent/features/probed.toml",
        "[probed]\nsummary = \"s\"\nentry = \"src/probed.rs\"\nfiles = [\"src/probed.rs\"]\n",
    );
    scratch.write("tests/cases/probed/probed_token_over_ssm.toml", CASE_TOKEN);
    scratch.write("tests/cases/probed/probed_status_data.toml", CASE_WITH_REAL);
    scratch.write("tests/cases/not-applicable.toml", NOT_APPLICABLE);
    scratch.write("tests/cases/needs-human.toml", NEEDS_HUMAN);
    scratch.write(".gitignore", "target/\n");
    scratch.write(
        "target/agent/scenarios/probed_token_over_ssm.json",
        &report_json("probed_token_over_ssm", true, None),
    );
    std::fs::write(scratch.0.join("document.json"), DOCUMENT).unwrap();
    scratch.git(&["init", "-q", "-b", "dev"]);
    scratch.git(&["add", "."]);
    scratch.git(&[
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@example.com",
        "commit",
        "-qm",
        "init",
    ]);
    scratch
}

/// `freshly_added_kind`, the one value nothing classifies, declared not
/// applicable, so the gate has only what a test puts in front of it.
const FRESHLY_ADDED_NOT_APPLICABLE: &str = r#"
[[not_applicable]]
reason = "the test adds it to watch it arrive"
[not_applicable.combination]
source = "freshly_added_kind"
"#;

fn verify_matrix(scratch: &Scratch, args: &[&str]) -> Output {
    support::xtask(&scratch.0)
        .arg("verify-matrix")
        .arg("--from")
        .arg(scratch.0.join("document.json"))
        .args(args)
        .env("KURAMA_XTASK_ROOT", scratch.repo())
        .output()
        .unwrap()
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn matrix_rejects_unclassified_valid_combination() {
    let dir = scratch("unclassified");
    let output = verify_matrix(&dir, &[]);
    assert!(!output.status.success(), "{}", text(&output));
    let printed = text(&output);
    // The enum value nothing declares is named with where it comes from, and
    // the values a case or a declaration covers are not.
    assert!(
        printed.contains(
            "UNCLASSIFIED: source=freshly_added_kind ([auth.*] kind = freshly_added_kind)"
        ),
        "{printed}"
    );
    assert!(!printed.contains("UNCLASSIFIED: source=token"), "{printed}");
    assert!(!printed.contains("UNCLASSIFIED: source=oauth"), "{printed}");
    assert!(!printed.contains("UNCLASSIFIED: secret=op"), "{printed}");
    assert!(
        !printed.contains("UNCLASSIFIED: status.kind=db"),
        "{printed}"
    );
    assert!(
        printed.contains("tests/cases/not-applicable.toml"),
        "{printed}"
    );
    let json: serde_json::Value =
        serde_json::from_str(&dir.report("verification-matrix.json")).unwrap();
    assert_eq!(json["gate"]["ok"], false);
    assert_eq!(json["counts"]["UNCLASSIFIED"], 1);
    assert_eq!(json["counts"]["NOT_APPLICABLE"], 2);
    assert_eq!(json["counts"]["NEEDS_HUMAN"], 1);

    let markdown = dir.report("verification-matrix.md");
    assert!(
        markdown.contains("| - | `[auth.*] kind` | source=freshly_added_kind |"),
        "{markdown}"
    );

    // A baseline file is not read any more: listing the value there does not
    // take it off the gate.
    dir.write(
        "tests/cases/unclassified-baseline.toml",
        "items = [\"source=freshly_added_kind\"]\n",
    );
    let output = verify_matrix(&dir, &[]);
    assert!(!output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("UNCLASSIFIED: source=freshly_added_kind"),
        "{}",
        text(&output)
    );
}

#[test]
fn matrix_reports_reproducible_failure() {
    let dir = scratch("failure");
    dir.classify_freshly_added();
    dir.write(
        "target/agent/scenarios/probed_token_over_ssm.json",
        &report_json(
            "probed_token_over_ssm",
            false,
            Some(("expect.exit_code", "exit code is 4: observed 2")),
        ),
    );
    let output = verify_matrix(&dir, &[]);
    assert!(!output.status.success(), "{}", text(&output));
    let markdown = dir.report("verification-matrix.md");
    let row = markdown
        .lines()
        .find(|line| {
            line.starts_with("| probed | `api` | ") && line.contains("probed_token_over_ssm")
        })
        .unwrap_or_else(|| panic!("{markdown}"));
    assert!(row.contains("| FAIL |"), "{row}");
    assert!(
        row.contains("`cargo xtask verify probed_token_over_ssm`"),
        "{row}"
    );
    assert!(row.contains("exit_code=4"), "{row}");
    assert!(row.contains("exit 2"), "{row}");
    assert!(
        markdown.contains("expect.exit_code: exit code is 4: observed 2"),
        "{markdown}"
    );
    let json: serde_json::Value =
        serde_json::from_str(&dir.report("verification-matrix.json")).unwrap();
    let failure = json["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "probed_token_over_ssm")
        .unwrap();
    assert_eq!(failure["status"], "FAIL");
    assert_eq!(
        failure["reproduce"],
        "cargo xtask verify probed_token_over_ssm"
    );
    assert_eq!(failure["failed_checks"][0]["name"], "expect.exit_code");
    assert_eq!(json["counts"]["FAIL"], 1);
    assert!(
        text(&output).contains("FAIL probed_token_over_ssm"),
        "{}",
        text(&output)
    );
}

#[test]
fn matrix_counts_unexecuted_and_unverified_real_without_failing() {
    let dir = scratch("layers");
    dir.classify_freshly_added();
    // No report for the case with `[real]`: it was not run at all.
    let output = verify_matrix(&dir, &["--json"]);
    assert!(output.status.success(), "{}", text(&output));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["counts"]["UNEXECUTED"], 1);
    assert_eq!(json["counts"]["PASS"], 1);

    // A passing fake report for it: real is still unverified, and the row
    // says so instead of PASS.
    dir.write(
        "target/agent/scenarios/probed_status_data.json",
        &report_json("probed_status_data", true, None),
    );
    let output = verify_matrix(&dir, &["--json"]);
    assert!(output.status.success(), "{}", text(&output));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["counts"]["UNEXECUTED"], 0);
    assert_eq!(json["counts"]["UNVERIFIED_REAL"], 1);
    let row = json["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "probed_status_data")
        .unwrap();
    assert_eq!(row["evidence"], "fake");
    assert!(
        row["observed"]
            .as_str()
            .unwrap()
            .contains("real: not run (no `verify --layer real` run)"),
        "{row}"
    );

    // The real layer ran and left the case out by name: the row carries the
    // requirement it found missing, and its reproduce names the layer.
    dir.write(
        "target/agent/scenarios-real/not-run.txt",
        "probed_status_data: requires keychain:OP_SERVICE_ACCOUNT_TOKEN (the keychain entry `OP_SERVICE_ACCOUNT_TOKEN` for $USER, or the variable exported)\n",
    );
    let output = verify_matrix(&dir, &["--json"]);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let row = json["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "probed_status_data")
        .unwrap();
    assert_eq!(json["counts"]["UNVERIFIED_REAL"], 1);
    assert!(
        row["observed"].as_str().unwrap().contains(
            "real: not run (requires keychain:OP_SERVICE_ACCOUNT_TOKEN (the keychain entry"
        ),
        "{row}"
    );
    assert!(
        row["reproduce"]
            .as_str()
            .unwrap()
            .ends_with("cargo xtask verify --layer real probed"),
        "{row}"
    );
    // What a person prepares is in the NEEDS_HUMAN row, not only the count.
    let markdown = dir.report("verification-matrix.md");
    assert!(
        markdown.contains("NEEDS_HUMAN")
            && markdown.contains("prepare: a [db.*] section pointing at a reachable server"),
        "{markdown}"
    );
    std::fs::remove_file(dir.repo().join("target/agent/scenarios-real/not-run.txt")).unwrap();

    // A real report next to it, from a run on an older commit: stale, so
    // it verifies nothing and the row says which commit it came from.
    dir.write(
        "target/agent/scenarios-real/probed_status_data.json",
        &report_json("probed_status_data", true, None).replace("\"fake\"", "\"real\""),
    );
    dir.layer_ran_on("real", "a1ab2e0", false);
    let head = dir.git(&["rev-parse", "--short", "HEAD"]);
    let output = verify_matrix(&dir, &["--json"]);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["counts"]["UNVERIFIED_REAL"], 1, "{}", text(&output));
    let row = json["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "probed_status_data")
        .unwrap();
    assert_eq!(row["evidence"], "fake");
    assert!(
        row["observed"].as_str().unwrap().contains(
            "real: not run (stale: its reports were written on a1ab2e0, not on the code the tree holds now)"
        ),
        "{row}"
    );

    // The same report from a run on HEAD: verified.
    dir.layer_ran_on("real", &head, false);
    let output = verify_matrix(&dir, &["--json"]);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["counts"]["UNVERIFIED_REAL"], 0);
    assert_eq!(json["counts"]["PASS"], 2);
    let row = json["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "probed_status_data")
        .unwrap();
    assert_eq!(row["evidence"], "fake, real");
}

fn unverified_real(dir: &Scratch) -> serde_json::Value {
    let output = verify_matrix(dir, &["--json"]);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    json["counts"]["UNVERIFIED_REAL"].clone()
}

/// A real run counts while the tree holds the code it ran on, whatever
/// happened to `.agent/real/` since: `verify-real` writes its evidence there
/// after the layer report, and committing that evidence moves HEAD. Any
/// other difference -- a tracked file edited, a file added -- makes it
/// stale, and so does a report written on a dirty tree, whose code nothing
/// can recover.
#[test]
fn matrix_counts_a_real_run_until_the_code_outside_the_evidence_changes() {
    let dir = scratch("same-code");
    dir.classify_freshly_added();
    dir.write(
        "target/agent/scenarios/probed_status_data.json",
        &report_json("probed_status_data", true, None),
    );
    dir.write(
        "target/agent/scenarios-real/probed_status_data.json",
        &report_json("probed_status_data", true, None).replace("\"fake\"", "\"real\""),
    );
    let head = dir.git(&["rev-parse", "--short", "HEAD"]);
    dir.layer_ran_on("real", &head, false);
    assert_eq!(unverified_real(&dir), 0);

    dir.write(".agent/real/probed.json", "{}\n");
    assert_eq!(unverified_real(&dir), 0, "uncommitted evidence");
    dir.commit_all("evidence");
    assert_eq!(unverified_real(&dir), 0, "committed evidence");

    dir.write("src/probed.rs", "fn main() {}\n");
    assert_eq!(unverified_real(&dir), 1, "an untracked file");
    std::fs::remove_file(dir.repo().join("src/probed.rs")).unwrap();
    assert_eq!(unverified_real(&dir), 0);

    let features = dir.repo().join(".agent/features/probed.toml");
    let original = std::fs::read_to_string(&features).unwrap();
    dir.write(
        ".agent/features/probed.toml",
        &format!("{original}# edited\n"),
    );
    assert_eq!(unverified_real(&dir), 1, "a tracked file edited");
    dir.commit_all("edit");
    assert_eq!(unverified_real(&dir), 1, "a commit after the run");
    dir.layer_ran_on("real", &dir.git(&["rev-parse", "--short", "HEAD"]), false);
    assert_eq!(unverified_real(&dir), 0);

    dir.layer_ran_on("real", &dir.git(&["rev-parse", "--short", "HEAD"]), true);
    assert_eq!(unverified_real(&dir), 1, "a report written on a dirty tree");
}

/// The reports are read where `verify` wrote them: under `CARGO_TARGET_DIR`
/// when it is set, not a `target/` an older run left behind.
#[test]
fn matrix_reads_the_reports_under_cargo_target_dir() {
    let dir = scratch("target-dir");
    dir.classify_freshly_added();
    let elsewhere = dir.0.join("elsewhere");
    let failed = report_json(
        "probed_token_over_ssm",
        false,
        Some(("expect.exit_code", "exit code is 4: observed 2")),
    );
    std::fs::create_dir_all(elsewhere.join("agent/scenarios")).unwrap();
    std::fs::write(
        elsewhere.join("agent/scenarios/probed_token_over_ssm.json"),
        failed,
    )
    .unwrap();
    let output = support::xtask(&dir.0)
        .arg("verify-matrix")
        .arg("--from")
        .arg(dir.0.join("document.json"))
        .env("KURAMA_XTASK_ROOT", dir.repo())
        .env("CARGO_TARGET_DIR", &elsewhere)
        .output()
        .unwrap();
    assert!(!output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("FAIL probed_token_over_ssm"),
        "{}",
        text(&output)
    );
}

#[test]
fn matrix_refuses_a_report_that_carries_a_credential() {
    let dir = scratch("credential");
    dir.classify_freshly_added();
    dir.write(
        "target/agent/scenarios/probed_token_over_ssm.json",
        &report_json(
            "probed_token_over_ssm",
            false,
            Some(("expect.stderr", "Authorization: Bearer abc.def")),
        ),
    );
    let output = verify_matrix(&dir, &[]);
    assert!(!output.status.success(), "{}", text(&output));
    let printed = text(&output);
    assert!(printed.contains("a bearer token"), "{printed}");
    assert!(printed.contains("an Authorization header"), "{printed}");
    assert!(
        !dir.repo()
            .join("target/agent/verification-matrix.json")
            .exists(),
        "nothing is written when the matrix would carry a credential"
    );
}

/// A bare GitHub token and the value of an API key header in a check detail
/// stop the matrix before either file is written.
#[test]
fn matrix_rejects_pat_and_custom_api_key_values() {
    for (detail, mark) in [
        (
            "observed ghp_16C7e42F292c6912E7710c838347Ae178B4a",
            "a GitHub token",
        ),
        ("sent x-api-key: key-secret-value", "an API key header"),
    ] {
        let dir = scratch("api-key");
        dir.classify_freshly_added();
        dir.write(
            "target/agent/scenarios/probed_token_over_ssm.json",
            &report_json(
                "probed_token_over_ssm",
                false,
                Some(("expect.stderr", detail)),
            ),
        );
        let output = verify_matrix(&dir, &[]);
        assert!(!output.status.success(), "{detail}: {}", text(&output));
        let printed = text(&output);
        assert!(printed.contains(mark), "{detail}: {printed}");
        for name in ["verification-matrix.json", "verification-matrix.md"] {
            assert!(
                !dir.repo().join("target/agent").join(name).exists(),
                "{detail}: {name} was written"
            );
        }
    }
}

#[test]
fn matrix_refuses_an_unknown_flag_and_a_missing_file() {
    let dir = scratch("usage");
    let output = verify_matrix(&dir, &["--nope"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--nope"));
    let output = support::xtask(&dir.0)
        .args(["verify-matrix", "--from"])
        .arg(dir.0.join("absent.json"))
        .env("KURAMA_XTASK_ROOT", dir.repo())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("absent.json"));
}
