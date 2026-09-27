//! `cargo xtask verify --layer local` run as the binary over a scratch tree
//! with a fake `docker` and fake `tests/db/up.sh` / `down.sh`: what it does
//! when Docker is down, when nothing declares `[local]`, and that the
//! databases it started are stopped when the run fails after them.

use std::path::PathBuf;
use std::process::Output;

mod support;

const CASE_WITH_LOCAL: &str = r#"
id = "probed_reads_a_server"
feature = "probed"

[combination]
command = "db"

[input]
args = ["db", "app", "--tables", "--json"]

[expect]
exit_code = 0

[local]
config = "[db.app]\nengine = \"postgresql\"\n"

[local.combination]
engine = "postgresql"

[local.expect]
exit_code = 0
"#;

const CASE_FAKE_ONLY: &str = r#"
id = "probed_reads_a_file"
feature = "probed"

[input]
args = ["db", "app", "--tables"]

[expect]
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

    /// An executable under `bin/`, which goes first on PATH.
    fn executable(&self, name: &str, script: &str) {
        use std::os::unix::fs::PermissionsExt;
        let path = self.0.join("bin").join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn read(&self, relative: &str) -> Option<String> {
        std::fs::read_to_string(self.0.join(relative)).ok()
    }

    /// `verify --layer local <target>` with the scratch bin first on PATH.
    fn verify_layer(&self, target: &str) -> Output {
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
            .args(["verify", "--layer", "local", target])
            .output()
            .unwrap()
    }
}

/// A repository with one case that declares `[local]`, one that does not,
/// a `docker` that answers `info` the way `docker_info` says (running when
/// empty) and `ps` with the containers a `running-<name>` file in the
/// scratch directory says are running, and `up.sh` / `down.sh` that log
/// their names to `db.log` (`up.sh` then fails while `up-fails` exists).
fn scratch(name: &str, docker_info: &str) -> Scratch {
    let dir =
        std::env::temp_dir().join(format!("xtask-verify-layer-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let scratch = Scratch(dir);
    scratch.write(
        "tests/cases/probed/probed_reads_a_server.toml",
        CASE_WITH_LOCAL,
    );
    scratch.write(
        "tests/cases/probed/probed_reads_a_file.toml",
        CASE_FAKE_ONLY,
    );
    let log = scratch.0.join("db.log");
    for script in ["up.sh", "down.sh"] {
        scratch.write(
            &format!("tests/db/{script}"),
            &format!(
                "#!/bin/sh\necho {script} >> {}\n[ {script} = up.sh ] && [ -f {}/up-fails ] && exit 1\nexit 0\n",
                log.display(),
                scratch.0.display()
            ),
        );
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            scratch.repo().join("tests/db").join(script),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    let info = if docker_info.is_empty() {
        "exit 0".to_string()
    } else {
        format!("echo '{docker_info}' >&2; exit 1")
    };
    let ps = format!(
        "for arg; do case $arg in name=*) name=${{arg#name=^}}; name=${{name%$}} ;; esac; done; [ -f {}/running-$name ] && echo $name; exit 0",
        scratch.0.display()
    );
    scratch.executable(
        "docker",
        &format!("#!/bin/sh\ncase \"$1\" in\n  info) {info} ;;\n  ps) {ps} ;;\n  *) echo \"fake docker: $*\" >&2; exit 1 ;;\nesac\n"),
    );
    scratch
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Docker down: nothing starts, the reason lands in `not-run.txt` for the
/// matrix to show, the report says NOT_RUN, and the command succeeds -- a
/// laptop without Docker is not a failed verification.
#[test]
fn verify_layer_records_why_docker_could_not_run_and_starts_nothing() {
    let scratch = scratch(
        "docker-down",
        "Cannot connect to the Docker daemon at unix:///var/run/docker.sock",
    );
    let output = scratch.verify_layer("all");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    let reason = scratch
        .read("repo/target/agent/scenarios-local/not-run.txt")
        .expect("not-run.txt is written");
    assert_eq!(
        reason.trim(),
        "docker is not running: Cannot connect to the Docker daemon at unix:///var/run/docker.sock"
    );
    assert!(scratch.read("db.log").is_none(), "up.sh was not run");
    let report = scratch
        .read("repo/target/agent/verification-report-local.md")
        .expect("the layer's own report");
    assert!(
        report.contains("NOT_RUN: docker is not running"),
        "{report}"
    );
    assert!(
        scratch
            .read("repo/target/agent/verification-report.md")
            .is_none(),
        "the fake layer's report is not touched"
    );
}

/// No case declares `[local]`: nothing is started, not even Docker is asked.
#[test]
fn verify_layer_with_nothing_declared_runs_nothing() {
    let scratch = scratch("nothing-declared", "");
    std::fs::remove_file(
        scratch
            .repo()
            .join("tests/cases/probed/probed_reads_a_server.toml"),
    )
    .unwrap();
    let output = scratch.verify_layer("all");
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(text(&output.stdout).contains("NOT_RUN: no case declares [local]"));
    assert!(scratch.read("db.log").is_none(), "up.sh was not run");
    assert!(
        scratch
            .read("repo/target/agent/scenarios-local/not-run.txt")
            .is_none(),
        "there is nothing for the matrix to explain"
    );
}

/// Docker up, the databases started, then the run cannot even list the
/// scenarios (the scratch tree is no cargo project): the command fails, and
/// `down.sh` ran after `up.sh` all the same.
#[test]
fn verify_layer_stops_the_databases_it_started_when_the_run_fails() {
    let scratch = scratch("run-fails", "");
    let output = scratch.verify_layer("probed");
    assert!(
        !output.status.success(),
        "a run that could not list its scenarios is a failure: {}",
        text(&output.stdout)
    );
    assert_eq!(
        scratch.read("db.log").as_deref(),
        Some("up.sh\ndown.sh\n"),
        "the databases this run started are removed on the way out"
    );
    assert!(
        scratch
            .read("repo/target/agent/scenarios-local/not-run.txt")
            .is_none(),
        "a failure is not a reason for not running"
    );
}

/// `up.sh` that fails part-way may have started some containers: they are
/// removed before the command fails.
#[test]
fn verify_layer_stops_the_databases_when_up_fails() {
    let scratch = scratch("up-fails", "");
    std::fs::write(scratch.0.join("up-fails"), "").unwrap();
    let output = scratch.verify_layer("probed");
    assert!(!output.status.success());
    assert!(
        text(&output.stderr).contains("up.sh failed"),
        "{}",
        text(&output.stderr)
    );
    assert_eq!(scratch.read("db.log").as_deref(), Some("up.sh\ndown.sh\n"));
}

/// Some of the containers running is a person's `db-up` too: `up.sh` starts
/// the rest, and the run leaves them all up.
#[test]
fn verify_layer_leaves_up_the_databases_a_person_was_running() {
    let scratch = scratch("partly-up", "");
    std::fs::write(scratch.0.join("running-kurama-pg17"), "").unwrap();
    let output = scratch.verify_layer("probed");
    assert!(
        !output.status.success(),
        "the scratch tree is no cargo project"
    );
    assert_eq!(scratch.read("db.log").as_deref(), Some("up.sh\n"));

    let scratch = scratch_all_up();
    let output = scratch.verify_layer("probed");
    assert!(!output.status.success());
    assert_eq!(scratch.read("db.log"), None, "nothing to start or stop");
}

fn scratch_all_up() -> Scratch {
    let scratch = scratch("all-up", "");
    for name in ["kurama-pg17", "kurama-pg18", "kurama-mysql84"] {
        std::fs::write(scratch.0.join(format!("running-{name}")), "").unwrap();
    }
    scratch
}

/// A target that names a feature without a `[local]` case runs nothing for
/// it, and an unknown layer is refused by name.
#[test]
fn verify_layer_names_the_layers_it_knows() {
    let scratch = scratch("unknown-layer", "");
    let mut command = support::xtask(&scratch.0);
    let output = command
        .env("KURAMA_XTASK_ROOT", scratch.repo())
        .args(["verify", "--layer", "staging"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        text(&output.stderr).contains("no layer is named `staging`"),
        "{}",
        text(&output.stderr)
    );
}
