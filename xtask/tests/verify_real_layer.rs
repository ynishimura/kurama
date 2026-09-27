//! `cargo xtask verify --layer real` run as the binary over a scratch tree:
//! what it does when a case requires something the environment does not
//! hold (nothing runs, `not-run.txt` names the case and what to prepare),
//! when the base configuration is missing, and that `verify-matrix` then
//! shows the case with that reason and its `reproduce` naming the layer.

use std::path::PathBuf;
use std::process::Output;

mod support;

const CASE_WITH_REAL: &str = r#"
id = "probed_reads_the_real_service"
feature = "probed"

[combination]
command = "api"

[input]
args = ["api", "svc", "/items"]

[expect]
exit_code = 0

[real]
requires = ["profile:sandbox", "auth:svc", "keychain:OP_SERVICE_ACCOUNT_TOKEN"]
config = "[api.svc]\nbase_url = \"https://example.invalid\"\n"

[real.combination]
target = "example"

[real.expect]
exit_code = 0
"#;

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
        let output = std::process::Command::new("git")
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

    /// Commits the whole tree (`target/` ignored), a repository first if
    /// there is none.
    fn commit_all(&self) {
        if !self.repo().join(".git").exists() {
            self.write(".gitignore", "target/\n");
            self.git(&["init", "-q", "-b", "dev"]);
        }
        self.git(&["add", "-A"]);
        self.git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-qm",
            "commit",
        ]);
    }

    /// The layer report a real run on the current HEAD of a clean tree writes.
    fn real_layer_ran_on_head(&self) {
        let head = self.git(&["rev-parse", "--short", "HEAD"]);
        self.write(
            "target/agent/verification-report-real.json",
            &format!(r#"{{"schema_version":2,"git":{{"head":"{head}","dirty":false}}}}"#),
        );
    }

    fn read(&self, relative: &str) -> Option<String> {
        std::fs::read_to_string(self.0.join(relative)).ok()
    }

    /// `verify --layer real <target>` with a HOME that holds no `~/.aws`,
    /// no token in the environment and a `security` that finds nothing.
    fn verify_real_layer(&self, target: &str) -> Output {
        use std::os::unix::fs::PermissionsExt;
        let security = self.0.join("bin/security");
        std::fs::create_dir_all(security.parent().unwrap()).unwrap();
        std::fs::write(&security, "#!/bin/sh\nexit 44\n").unwrap();
        std::fs::set_permissions(&security, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut command = support::xtask(&self.0);
        let path = command
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .and_then(|(_, value)| value)
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
        command
            .env("PATH", format!("{}:{path}", self.0.join("bin").display()))
            .env("KURAMA_XTASK_ROOT", self.repo())
            .env_remove("OP_SERVICE_ACCOUNT_TOKEN")
            .args(["verify", "--layer", "real", target])
            .output()
            .unwrap()
    }
}

/// A repository with one case that declares `[real]` and, when `base_config`
/// is given, the copied daily configuration under `.kurama/`.
fn scratch(name: &str, base_config: Option<&str>) -> Scratch {
    let dir = std::env::temp_dir().join(format!(
        "xtask-verify-real-layer-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let scratch = Scratch(dir);
    scratch.write(
        "tests/cases/probed/probed_reads_the_real_service.toml",
        CASE_WITH_REAL,
    );
    if let Some(config) = base_config {
        scratch.write(".kurama/config.toml", config);
    }
    scratch
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The environment holds none of what the case requires: nothing is run,
/// the command succeeds (a laptop without the credentials is not a failed
/// verification), and `not-run.txt` names the case with each missing
/// requirement and what a person prepares for it.
#[test]
fn verify_real_layer_names_what_is_missing_and_runs_nothing() {
    let scratch = scratch("missing", Some("[core]\n"));
    let output = scratch.verify_real_layer("all");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    let reason = scratch
        .read("repo/target/agent/scenarios-real/not-run.txt")
        .expect("not-run.txt is written");
    assert!(
        reason.starts_with("probed_reads_the_real_service: requires profile:sandbox (a `[profile sandbox]` in ~/.aws/config), auth:svc (an `[auth.svc]` section in the base configuration), keychain:OP_SERVICE_ACCOUNT_TOKEN (the keychain entry"),
        "{reason}"
    );
    assert!(
        std::fs::read_dir(scratch.repo().join("target/agent/scenarios-real"))
            .unwrap()
            .flatten()
            .all(|entry| entry.path().extension().is_none_or(|ext| ext != "json")),
        "no report was written"
    );
    let report = scratch
        .read("repo/target/agent/verification-report-real.md")
        .expect("the layer's own report");
    assert!(
        report.contains("NOT_RUN: no case has what it requires"),
        "{report}"
    );
}

/// A requirement the environment does hold (the profile, the section) is not
/// what stops the run: only the ones missing are named.
#[test]
fn verify_real_layer_names_only_the_missing_requirements() {
    let scratch = scratch("partly", Some("[core]\n\n[auth.svc]\nkind = \"token\"\n"));
    // `support::xtask` makes `scratch/home` the HOME the runner reads.
    std::fs::create_dir_all(scratch.0.join("home/.aws")).unwrap();
    std::fs::write(
        scratch.0.join("home/.aws/config"),
        "[profile sandbox]\nregion = ap-northeast-1\n",
    )
    .unwrap();
    let output = scratch.verify_real_layer("probed");
    assert!(output.status.success(), "{}", text(&output.stderr));
    let reason = scratch
        .read("repo/target/agent/scenarios-real/not-run.txt")
        .expect("not-run.txt is written");
    assert!(!reason.contains("profile:sandbox"), "{reason}");
    assert!(!reason.contains("auth:svc"), "{reason}");
    assert!(
        reason.contains("keychain:OP_SERVICE_ACCOUNT_TOKEN"),
        "{reason}"
    );
}

/// Without the base configuration nothing can run: the reason says where it
/// is expected and how it gets there.
#[test]
fn verify_real_layer_without_the_base_configuration_says_where_it_goes() {
    let scratch = scratch("no-config", None);
    let output = scratch.verify_real_layer("all");
    assert!(output.status.success(), "{}", text(&output.stderr));
    let reason = scratch
        .read("repo/target/agent/scenarios-real/not-run.txt")
        .expect("not-run.txt is written");
    assert!(
        reason.contains(".kurama/config.toml does not exist")
            && reason.contains("cargo xtask worktree add"),
        "{reason}"
    );
}

/// A layer nobody knows is refused by name, and both known ones are named.
#[test]
fn verify_real_layer_refuses_a_layer_nobody_knows() {
    let scratch = scratch("unknown", Some("[core]\n"));
    let mut command = support::xtask(&scratch.0);
    let output = command
        .env("KURAMA_XTASK_ROOT", scratch.repo())
        .args(["verify", "--layer", "staging", "all"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = text(&output.stderr);
    assert!(
        stderr
            .contains("no layer is named `staging`; `local`, `throwaway` and `real` are the ones"),
        "{stderr}"
    );
}

const CASE_WITHOUT_REQUIREMENTS: &str = r#"
id = "probed_reads_without_requirements"
feature = "probed"

[combination]
command = "api"

[input]
args = ["api", "svc", "/items"]

[expect]
exit_code = 0

[real]
config = "[api.svc]\nbase_url = \"https://example.invalid\"\n"

[real.expect]
exit_code = 0
"#;

/// A report that holds the service account token itself -- no `Bearer`, no
/// pattern, just the value this run handed the cases -- is removed and named
/// without the value, and that holds on the path where the cases could not
/// even run (the scratch tree is no cargo project).
#[test]
fn verify_real_layer_scans_for_the_known_token_even_when_the_run_fails() {
    let scratch = scratch("scan", Some("[core]\n"));
    scratch.write(
        "tests/cases/probed/probed_reads_without_requirements.toml",
        CASE_WITHOUT_REQUIREMENTS,
    );
    std::fs::remove_file(
        scratch
            .repo()
            .join("tests/cases/probed/probed_reads_the_real_service.toml"),
    )
    .unwrap();
    let token = "ops_scratchTokenValue0123456789";
    // Another feature's report from a run on this very code, so the run
    // keeps it and the scan is what removes it.
    scratch.commit_all();
    scratch.real_layer_ran_on_head();
    scratch.write(
        "target/agent/scenarios-real/other_feature_case.json",
        &format!("{{\"stderr\":\"op said {token}\"}}"),
    );
    let mut command = support::xtask(&scratch.0);
    let output = command
        .env("KURAMA_XTASK_ROOT", scratch.repo())
        .env("OP_SERVICE_ACCOUNT_TOKEN", token)
        .args(["verify", "--layer", "real", "probed"])
        .output()
        .unwrap();
    assert!(!output.status.success(), "{}", text(&output.stdout));
    assert!(
        !scratch
            .repo()
            .join("target/agent/scenarios-real/other_feature_case.json")
            .exists(),
        "the report holding the token is removed"
    );
    let report = scratch
        .read("repo/target/agent/verification-report-real.md")
        .expect("the layer's own report");
    assert!(
        report.contains("other_feature_case.json held the service account token"),
        "{report}"
    );
    assert!(!report.contains(token), "{report}");
    assert!(!text(&output.stdout).contains(token));
    assert!(!text(&output.stderr).contains(token));
}

/// One feature's run keeps the other features' reports only while the last
/// real run was on the code the tree holds now: the report this run writes
/// dates them all, so reports from older code would otherwise read as fresh.
#[test]
fn verify_real_layer_for_one_feature_clears_reports_written_on_other_code() {
    let scratch = scratch("stale", Some("[core]\n"));
    scratch.commit_all();
    scratch.real_layer_ran_on_head();
    let other = "target/agent/scenarios-real/other_feature_case.json";
    scratch.write(
        other,
        r#"{"schema_version":1,"scenario":"other_feature_case","feature":"other","command":["kurama","api"],"passed":true,"combination":null,"evidence":"real","checks":[],"observed":{"runs":[{"exit_code":0}],"files_written":[]},"seeded":{}}"#,
    );
    let output = scratch.verify_real_layer("probed");
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(
        scratch.repo().join(other).exists(),
        "a report from the current code is kept"
    );
    // `verify-real` writes its evidence after the layer report; the next
    // feature's run still finds the same code, and its own report is clean.
    scratch.write(".agent/real/probed.json", "{}\n");
    let output = scratch.verify_real_layer("probed");
    assert!(output.status.success(), "{}", text(&output.stderr));
    let output = scratch.verify_real_layer("probed");
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(
        scratch.repo().join(other).exists(),
        "uncommitted evidence changes no code"
    );

    scratch.write("src/probed.rs", "fn main() {}\n");
    scratch.commit_all();
    let output = scratch.verify_real_layer("probed");
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(
        !scratch.repo().join(other).exists(),
        "a report from older code is removed"
    );
}
