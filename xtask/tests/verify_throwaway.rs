//! `cargo xtask verify --layer throwaway` run as the binary over a scratch
//! tree with a fake `kurama` (KURAMA_BINARY), a fake `aws` and fake up/down
//! scripts: it stops at the estimate without `--yes`, and with it creates,
//! runs, and deletes on every path -- a failed case, an `up` that fails
//! half-way, a `down` that fails -- reporting what was left and how to
//! remove it, with no credential in the report.

use std::path::PathBuf;
use std::process::Output;

mod support;

const CASE_IAM_API: &str = r#"
id = "probed_signs_for_the_api"
feature = "probed"

[combination]
command = "api"

[input]
args = ["api", "iam-rest", "/echo"]

[expect]
exit_code = 0

[throwaway]
stacks = ["iam-api"]
config = "[api.iam-rest]\nbase_url = \"{iam_api_url}\"\naws_profile = \"{throwaway_profile}\"\n"

[throwaway.expect]
exit_code = 0
"#;

const CASE_BASTION: &str = r#"
id = "probed_tunnels_to_the_bastion"
feature = "probed"

[combination]
command = "db"

[input]
args = ["db", "bastion", "--tables"]

[expect]
exit_code = 0

[throwaway]
stacks = ["bastion", "iam-api"]
config = "[db.bastion]\nengine = \"postgresql\"\n"

[throwaway.expect]
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

    fn executable(&self, relative: &str, script: &str) {
        use std::os::unix::fs::PermissionsExt;
        let path = self.0.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn read(&self, relative: &str) -> Option<String> {
        std::fs::read_to_string(self.0.join(relative)).ok()
    }

    /// `verify --layer throwaway <args>` with the scratch bin first on PATH
    /// and the fake kurama as the binary that would carry the credentials.
    fn verify_throwaway(&self, args: &[&str]) -> Output {
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
            .env("KURAMA_BINARY", self.0.join("bin/kurama"))
            .args(["verify", "--layer", "throwaway"])
            .args(args)
            .output()
            .unwrap()
    }
}

/// A repository with two cases that declare `[throwaway]` (one needing
/// `iam-api`, one `bastion` and `iam-api`), a fake `kurama` whose `exec`
/// runs the command after `--` and logs the profile and the session cache it
/// was handed, up scripts that log their names and create the stack the
/// run's variable names (`up_status` is the code they exit with), down
/// scripts that remove it -- unless `down-fails` (exit 1) or `down-keeps`
/// (exit 0, stack left) exists in the scratch directory -- and an `aws` whose
/// `describe-stacks` answers the outputs while a stack exists, and "does not
/// exist" otherwise; with `squat` in the scratch directory every stack
/// exists already.
fn scratch(name: &str, up_status: &str) -> Scratch {
    let dir = std::env::temp_dir().join(format!(
        "xtask-verify-throwaway-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let scratch = Scratch(dir);
    scratch.write(
        "tests/cases/probed/probed_signs_for_the_api.toml",
        CASE_IAM_API,
    );
    scratch.write(
        "tests/cases/probed/probed_tunnels_to_the_bastion.toml",
        CASE_BASTION,
    );
    let log = scratch.0.join("run.log");
    let state = scratch.0.join("stacks");
    std::fs::create_dir_all(&state).unwrap();
    scratch.executable(
        "bin/kurama",
        &format!(
            "#!/bin/sh\n[ \"$1\" = exec ] || {{ echo \"fake kurama: $*\" >&2; exit 1; }}\necho \"exec $2\" >> {log}\necho \"cache $KURAMA_TEST_SESSION_CACHE_FILE\" >> {log}\nshift 3\nexec \"$@\"\n",
            log = log.display()
        ),
    );
    for (script, variable) in [
        ("tests/api/iam-api-up.sh", "KURAMA_IAM_API_STACK"),
        ("tests/db/bastion-up.sh", "KURAMA_BASTION_STACK"),
    ] {
        scratch.executable(
            &format!("repo/{script}"),
            &format!(
                "#!/bin/sh\necho {script} >> {log}\ntouch \"{state}/${variable}\"\nexit {up_status}\n",
                log = log.display(),
                state = state.display()
            ),
        );
    }
    for (script, variable) in [
        ("tests/api/iam-api-down.sh", "KURAMA_IAM_API_STACK"),
        ("tests/db/bastion-down.sh", "KURAMA_BASTION_STACK"),
    ] {
        scratch.executable(
            &format!("repo/{script}"),
            &format!(
                "#!/bin/sh\necho {script} >> {log}\n[ -f {dir}/down-fails ] && exit 1\n[ -f {dir}/down-keeps ] && exit 0\nrm -f \"{state}/${variable}\"\n",
                log = log.display(),
                dir = scratch.0.display(),
                state = state.display()
            ),
        );
    }
    scratch.executable(
        "bin/aws",
        &format!(
            r#"#!/bin/sh
# aws cloudformation describe-stacks --stack-name NAME [--query Stacks[0].Outputs] --output json
name=""
query=""
while [ $# -gt 0 ]; do
  case "$1" in
    --stack-name) name=$2; shift ;;
    --query) query=$2; shift ;;
  esac
  shift
done
echo "describe $name" >> {log}
if [ ! -f {dir}/squat ] && [ ! -f "{state}/$name" ]; then
  echo "An error occurred (ValidationError) when calling the DescribeStacks operation: Stack with id $name does not exist" >&2
  exit 254
fi
if [ -n "$query" ]; then
  case "$name" in
    kurama-iam-api-test-*) echo '[{{"OutputKey":"RestApiUrl","OutputValue":"https://r.execute-api.ap-northeast-1.amazonaws.com/v1"}},{{"OutputKey":"FunctionUrl","OutputValue":"https://f.lambda-url.ap-northeast-1.on.aws/"}}]' ;;
    kurama-tunnel-test-*) echo '[{{"OutputKey":"InstanceId","OutputValue":"i-0123456789abcdef0"}}]' ;;
  esac
else
  echo '{{"Stacks":[{{"StackStatus":"CREATE_COMPLETE"}}]}}'
fi
"#,
            log = log.display(),
            dir = scratch.0.display(),
            state = state.display()
        ),
    );
    scratch
}

/// The stacks the scratch `aws` still knows, by name.
fn stacks_left(scratch: &Scratch) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(scratch.0.join("stacks"))
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn log_lines(scratch: &Scratch) -> Vec<String> {
    scratch
        .read("run.log")
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// Without `--yes` the run prints what it would create with the estimate,
/// writes the approval it needs for the matrix, starts nothing, and fails.
#[test]
fn verify_throwaway_without_approval_prints_the_estimate_and_starts_nothing() {
    let scratch = scratch("no-approval", "0");
    let output = scratch.verify_throwaway(&["all", "--profile", "sandbox"]);
    assert!(!output.status.success(), "{}", text(&output.stdout));
    let stdout = text(&output.stdout);
    assert!(
        stdout.contains("2 case(s) need 2 stack(s) in profile `sandbox`"),
        "{stdout}"
    );
    assert!(stdout.contains("iam-api  $0.000/hour"), "{stdout}");
    assert!(stdout.contains("bastion  $0.012/hour"), "{stdout}");
    assert!(stdout.contains("total    $0.012/hour"), "{stdout}");
    let reason = scratch
        .read("repo/target/agent/scenarios-throwaway/not-run.txt")
        .expect("not-run.txt is written for the matrix");
    assert!(
        reason.starts_with("approval needed: creating these stacks bills the `sandbox` account; run `cargo xtask verify --layer throwaway all --profile sandbox --yes`"),
        "{reason}"
    );
    assert!(
        scratch.read("run.log").is_none(),
        "no script and no kurama ran"
    );
    assert!(
        scratch
            .read("repo/target/agent/scenarios-throwaway/cleanup.sh")
            .is_none(),
        "nothing to clean up"
    );
    let report = scratch
        .read("repo/target/agent/verification-report-throwaway.md")
        .expect("the layer's own report");
    assert!(report.contains("NOT_RUN: approval needed"), "{report}");
}

/// A case that fails (here: the cases cannot even be listed, the scratch
/// tree is no cargo project) still ends with every created stack deleted,
/// and the report says so per stack. The stacks carry this run's own names,
/// handed to the scripts through their variables, and the MFA session is
/// cached outside the report directory, whose every `.json` is a report.
#[test]
fn verify_throwaway_deletes_every_stack_it_created_even_when_the_cases_fail() {
    let scratch = scratch("cases-fail", "0");
    // What a killed run left: the one session file every run reuses.
    let session = scratch.repo().join("target/agent/throwaway-session.json");
    std::fs::create_dir_all(session.parent().unwrap()).unwrap();
    std::fs::write(&session, "a killed run's session").unwrap();
    let output = scratch.verify_throwaway(&["all", "--profile", "sandbox", "--yes"]);
    assert!(!output.status.success(), "the cases could not run");
    let lines = log_lines(&scratch);
    let position = |needle: &str| {
        lines
            .iter()
            .position(|line| line == needle)
            .unwrap_or_else(|| panic!("{needle} not in {lines:?}"))
    };
    assert!(position("tests/api/iam-api-up.sh") < position("tests/db/bastion-up.sh"));
    assert!(position("tests/db/bastion-down.sh") < position("tests/api/iam-api-down.sh"));
    assert!(
        lines
            .iter()
            .all(|line| !line.starts_with("exec ") || line == "exec sandbox"),
        "every AWS call went through kurama exec with the profile: {lines:?}"
    );
    let reports = scratch.repo().join("target/agent/scenarios-throwaway");
    let caches: Vec<&str> = lines
        .iter()
        .filter_map(|line| line.strip_prefix("cache "))
        .collect();
    assert!(!caches.is_empty(), "{lines:?}");
    for cache in &caches {
        assert!(
            !cache.is_empty() && !std::path::Path::new(cache).starts_with(&reports),
            "the session cache is no report: {cache}"
        );
        assert_eq!(std::path::Path::new(cache), session, "one fixed name");
        assert!(
            !std::path::Path::new(cache).exists(),
            "the MFA session of the run is not kept"
        );
    }
    let report = scratch
        .read("repo/target/agent/verification-report-throwaway.md")
        .expect("the layer's own report");
    let run_name = |prefix: &str| {
        let at = report
            .find(&format!("created and deleted ({prefix}-"))
            .unwrap_or_else(|| panic!("no {prefix} in {report}"));
        let name = &report[at + "created and deleted (".len()..];
        name[..name.find(')').unwrap()].to_string()
    };
    let api = run_name("kurama-iam-api-test");
    let bastion = run_name("kurama-tunnel-test");
    assert!(
        api.len() > "kurama-iam-api-test-".len(),
        "a run's own name: {api}"
    );
    assert_eq!(
        api.strip_prefix("kurama-iam-api-test"),
        bastion.strip_prefix("kurama-tunnel-test"),
        "one run, one suffix"
    );
    assert!(report.contains("cleanup:iam-api") && report.contains("cleanup:bastion"));
    assert!(
        lines.contains(&format!("describe {api}")),
        "the run's name is what AWS is asked about: {lines:?}"
    );
    assert!(
        scratch
            .read("repo/target/agent/scenarios-throwaway/cleanup.sh")
            .is_none(),
        "everything was removed, so the fallback script went with it"
    );
    assert!(stacks_left(&scratch).is_empty(), "no stack left");
}

/// An `up` that fails leaves the stack it may have created: the run stops
/// creating, runs no case, deletes what it created, and names the failure.
#[test]
fn verify_throwaway_stops_at_a_failed_up_and_deletes_what_it_created() {
    let scratch = scratch("up-fails", "1");
    let output = scratch.verify_throwaway(&["all", "--yes"]);
    assert!(!output.status.success());
    let lines = log_lines(&scratch);
    assert!(
        lines.contains(&"tests/api/iam-api-up.sh".to_string()),
        "{lines:?}"
    );
    assert!(
        !lines.contains(&"tests/db/bastion-up.sh".to_string()),
        "the second stack was not attempted: {lines:?}"
    );
    assert!(
        lines.contains(&"tests/api/iam-api-down.sh".to_string()),
        "{lines:?}"
    );
    let report = scratch
        .read("repo/target/agent/verification-report-throwaway.md")
        .unwrap();
    assert!(
        report.contains(
            "iam-api: tests/api/iam-api-up.sh exited with exit status: 1; the cases did not run"
        ),
        "{report}"
    );
    assert!(
        report.contains("created and deleted (kurama-iam-api-test-"),
        "{report}"
    );
    assert!(stacks_left(&scratch).is_empty(), "no stack left");
}

/// A `down` that fails is a stack left behind: the report names it and the
/// exact command that removes it, and the fallback script stays, carrying
/// the run's stack names and configuration so a person running it later
/// removes exactly those stacks. While it is there, the next run -- even
/// one that only prints the estimate -- stops before it touches anything.
#[test]
fn verify_throwaway_names_a_stack_it_could_not_delete_and_how_to_remove_it() {
    let scratch = scratch("down-fails", "0");
    scratch.write(".kurama/config.toml", "[core]\n");
    std::fs::write(scratch.0.join("down-fails"), "").unwrap();
    let output = scratch.verify_throwaway(&["probed", "--profile", "sandbox", "--yes"]);
    assert!(!output.status.success());
    let report = scratch
        .read("repo/target/agent/verification-report-throwaway.md")
        .unwrap();
    assert!(
        report.contains(
            "left: tests/api/iam-api-down.sh exited with exit status: 1; remove it with `KURAMA_"
        ),
        "{report}"
    );
    assert!(
        report.contains("exec 'sandbox' -- '")
            && report.contains("tests/api/iam-api-down.sh'` (stack kurama-iam-api-test-"),
        "{report}"
    );
    assert!(
        !report.contains("AKIA") && !report.contains("Bearer "),
        "no credential: {report}"
    );
    let cleanup = scratch
        .read("repo/target/agent/scenarios-throwaway/cleanup.sh")
        .expect("the fallback script stays while a stack is left");
    assert!(
        cleanup.contains("KURAMA_IAM_API_STACK='kurama-iam-api-test-")
            && cleanup.contains("KURAMA_BASTION_STACK='kurama-tunnel-test-")
            && cleanup.contains(&format!(
                "KURAMA_CONFIG_PATH='{}'",
                scratch.repo().join(".kurama/config.toml").display()
            ))
            && cleanup.contains("exec 'sandbox' -- '"),
        "{cleanup}"
    );
    assert_eq!(stacks_left(&scratch).len(), 2, "both stacks are left");

    let again = scratch.verify_throwaway(&["probed", "--profile", "sandbox"]);
    assert!(!again.status.success());
    assert!(
        text(&again.stderr).contains("a previous run may have left stacks"),
        "{}",
        text(&again.stderr)
    );
    assert_eq!(
        scratch
            .read("repo/target/agent/scenarios-throwaway/cleanup.sh")
            .as_deref(),
        Some(cleanup.as_str()),
        "the only record of the stacks left is kept"
    );

    let run_cleanup = || {
        let path = std::env::var("PATH").unwrap_or_default();
        std::process::Command::new("sh")
            .arg(
                scratch
                    .repo()
                    .join("target/agent/scenarios-throwaway/cleanup.sh"),
            )
            .env(
                "PATH",
                format!("{}:{path}", scratch.0.join("bin").display()),
            )
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap()
            .status
    };
    // A down that fails does not stop the ones after it, and the script
    // stays for the next attempt.
    let before = log_lines(&scratch).len();
    assert!(!run_cleanup().success());
    assert_eq!(
        log_lines(&scratch)[before..]
            .iter()
            .filter(|line| line.ends_with("-down.sh"))
            .collect::<Vec<_>>(),
        ["tests/db/bastion-down.sh", "tests/api/iam-api-down.sh"],
        "every down ran"
    );
    assert!(
        scratch
            .repo()
            .join("target/agent/scenarios-throwaway/cleanup.sh")
            .exists(),
        "a failed down keeps the script"
    );

    std::fs::remove_file(scratch.0.join("down-fails")).unwrap();
    assert!(run_cleanup().success());
    assert!(
        stacks_left(&scratch).is_empty(),
        "cleanup.sh removes exactly the stacks this run created: {:?}",
        stacks_left(&scratch)
    );
    assert!(
        scratch
            .read("repo/target/agent/scenarios-throwaway/cleanup.sh")
            .is_none(),
        "and then itself, so the next run may start"
    );
}

/// A `down` that exits 0 without removing the stack is caught by asking AWS
/// afterwards: the stack is reported as left, not as deleted.
#[test]
fn verify_throwaway_asks_aws_whether_a_stack_is_gone_after_down() {
    let scratch = scratch("down-keeps", "0");
    std::fs::write(scratch.0.join("down-keeps"), "").unwrap();
    let output = scratch.verify_throwaway(&["probed", "--yes"]);
    assert!(!output.status.success());
    let report = scratch
        .read("repo/target/agent/verification-report-throwaway.md")
        .unwrap();
    assert!(
        report.contains("left: kurama-iam-api-test-")
            && report.contains(" reported as CREATE_COMPLETE; remove it with"),
        "{report}"
    );
    assert!(!report.contains("created and deleted"), "{report}");
    assert!(
        scratch
            .read("repo/target/agent/scenarios-throwaway/cleanup.sh")
            .is_some()
    );
}

/// A stack of the run's name that AWS already knows is not the run's to
/// use or delete: nothing is created and no `down` runs.
#[test]
fn verify_throwaway_refuses_a_stack_that_exists_already() {
    let scratch = scratch("squat", "0");
    std::fs::write(scratch.0.join("squat"), "").unwrap();
    let output = scratch.verify_throwaway(&["probed", "--yes"]);
    assert!(!output.status.success());
    let lines = log_lines(&scratch);
    assert!(
        !lines.iter().any(|line| line.starts_with("tests/")),
        "no up and no down ran: {lines:?}"
    );
    let report = scratch
        .read("repo/target/agent/verification-report-throwaway.md")
        .unwrap();
    assert!(
        report.contains("iam-api: kurama-iam-api-test-")
            && report.contains(
                "is not free to create (reported as CREATE_COMPLETE); a run touches only the stacks it creates"
            ),
        "{report}"
    );
    assert!(!report.contains("cleanup:"), "{report}");
}

/// A layer nobody knows is refused by name, and a `[throwaway]` naming a
/// stack the catalogue lacks names the case.
#[test]
fn verify_throwaway_refuses_an_unknown_stack_by_the_case_that_named_it() {
    let scratch = scratch("unknown-stack", "0");
    scratch.write(
        "tests/cases/probed/probed_signs_for_the_api.toml",
        &CASE_IAM_API.replace("stacks = [\"iam-api\"]", "stacks = [\"moon\"]"),
    );
    let output = scratch.verify_throwaway(&["all", "--yes"]);
    assert!(!output.status.success());
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("probed_signs_for_the_api: `stacks` names `moon`, which is none of iam-api, bastion, rds-iam, dsql, s3-data"),
        "{stderr}"
    );
    assert!(scratch.read("run.log").is_none(), "nothing ran");
}

const CASE_DSQL: &str = r#"
id = "probed_opens_the_cluster"
feature = "probed"

[combination]
command = "db"

[input]
args = ["db", "dsql", "--tables"]

[expect]
exit_code = 0

[throwaway]
stacks = ["dsql"]
config = "[db.dsql]\nengine = \"postgresql\"\nhost = \"{dsql_host}\"\n"

[throwaway.expect]
exit_code = 0
"#;

const CASE_S3_DATA: &str = r#"
id = "probed_reads_both_buckets"
feature = "probed"

[combination]
command = "data"

[input]
args = ["data", "--from", "s3://x/orders.csv"]

[expect]
exit_code = 0

[throwaway]
stacks = ["s3-data"]
args = ["data", "--from", "s3://{s3_bucket_other_region}/orders.csv"]

[throwaway.expect]
exit_code = 0
"#;

/// A repository with a `dsql` case and an `s3-data` case, fake `dsql`
/// scripts, a fake `s3-up.sh` that creates both of its stacks, and the
/// repository's own `tests/s3/s3-down.sh` over a fake `aws` that logs every
/// call: `describe-stacks` answers the outputs of each stack while it exists
/// -- the `-other` one only when asked in its own Region, as AWS does --
/// the bucket name to the text query the down script makes, and
/// `delete-stack` removes the stack, except the `-other` one while
/// `keep-other` exists in the scratch directory.
fn scratch_with_dsql_and_s3(name: &str) -> Scratch {
    let dir = std::env::temp_dir().join(format!(
        "xtask-verify-throwaway-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let scratch = Scratch(dir);
    scratch.write(
        "tests/cases/probed/probed_opens_the_cluster.toml",
        CASE_DSQL,
    );
    scratch.write(
        "tests/cases/probed/probed_reads_both_buckets.toml",
        CASE_S3_DATA,
    );
    let log = scratch.0.join("run.log");
    let state = scratch.0.join("stacks");
    std::fs::create_dir_all(&state).unwrap();
    scratch.executable(
        "bin/kurama",
        &format!(
            "#!/bin/sh\n[ \"$1\" = exec ] || {{ echo \"fake kurama: $*\" >&2; exit 1; }}\necho \"exec $2\" >> {log}\nshift 3\nexec \"$@\"\n",
            log = log.display()
        ),
    );
    scratch.executable(
        "repo/tests/db/dsql-up.sh",
        &format!(
            "#!/bin/sh\necho tests/db/dsql-up.sh >> {log}\ntouch \"{state}/$KURAMA_DSQL_STACK\"\n",
            log = log.display(),
            state = state.display()
        ),
    );
    scratch.executable(
        "repo/tests/s3/s3-up.sh",
        &format!(
            "#!/bin/sh\necho tests/s3/s3-up.sh >> {log}\ntouch \"{state}/$KURAMA_S3_DATA_STACK\" \"{state}/$KURAMA_S3_DATA_STACK-other\"\n",
            log = log.display(),
            state = state.display()
        ),
    );
    scratch.executable(
        "repo/tests/db/dsql-down.sh",
        &format!(
            "#!/bin/sh\necho tests/db/dsql-down.sh >> {log}\nrm -f \"{state}/$KURAMA_DSQL_STACK\"\n",
            log = log.display(),
            state = state.display()
        ),
    );
    // The real down script, so what it asks `aws` for is what is checked.
    let real_down = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/s3/s3-down.sh");
    scratch.executable(
        "repo/tests/s3/s3-down.sh",
        &std::fs::read_to_string(real_down).expect("tests/s3/s3-down.sh is in the repository"),
    );
    scratch.executable(
        "bin/aws",
        &format!(
            r#"#!/bin/sh
echo "aws $*" >> {log}
service=$1; operation=$2; shift 2
name=""; query=""; region=""
while [ $# -gt 0 ]; do
  case "$1" in
    --stack-name) name=$2; shift ;;
    --query) query=$2; shift ;;
    --region) region=$2; shift ;;
  esac
  shift
done
case "$name" in
  *-other) [ "$region" = us-west-2 ] || name="$name@$region" ;;
esac
case "$service $operation" in
  "s3 rm") exit 0 ;;
  "cloudformation wait") exit 0 ;;
  "cloudformation delete-stack")
    case "$name" in *-other) [ -f {dir}/keep-other ] && exit 0 ;; esac
    rm -f "{state}/$name"; exit 0 ;;
  "cloudformation describe-stacks") ;;
  *) echo "fake aws: $service $operation" >&2; exit 1 ;;
esac
if [ ! -f "{state}/$name" ]; then
  echo "An error occurred (ValidationError) when calling the DescribeStacks operation: Stack with id $name does not exist" >&2
  exit 254
fi
case "$query" in
  *BucketName*) case "$name" in *-other) echo kurama-data-test-123456789012-other ;; *) echo kurama-data-test-123456789012-same ;; esac ;;
  "Stacks[0].Outputs") case "$name" in
    kurama-dsql-test-*) echo '[{{"OutputKey":"Endpoint","OutputValue":"abcdefghijklmnopqrst.dsql.ap-northeast-1.on.aws"}},{{"OutputKey":"Identifier","OutputValue":"abcdefghijklmnopqrst"}}]' ;;
    kurama-s3-data-test-*) echo '[{{"OutputKey":"BucketName","OutputValue":"kurama-data-test-123456789012-same"}},{{"OutputKey":"BucketRegion","OutputValue":"ap-northeast-1"}},{{"OutputKey":"OtherBucket","OutputValue":"kurama-data-test-123456789012-other"}},{{"OutputKey":"OtherRegion","OutputValue":"us-west-2"}}]' ;;
  esac ;;
  *) echo '{{"Stacks":[{{"StackStatus":"CREATE_COMPLETE"}}]}}' ;;
esac
"#,
            log = log.display(),
            dir = scratch.0.display(),
            state = state.display()
        ),
    );
    scratch
}

/// The `dsql` and `s3-data` stacks of #109: the estimate names both at $0,
/// `--yes` creates both, reads their outputs, and on the way down the
/// repository's own `s3-down.sh` empties each bucket -- by the name the
/// stack's outputs give, in the bucket's own Region -- before it deletes the
/// stack, and both stacks end deleted.
#[test]
fn verify_throwaway_empties_both_buckets_before_deleting_the_dsql_and_s3_stacks() {
    let scratch = scratch_with_dsql_and_s3("dsql-s3");
    let output = scratch.verify_throwaway(&["all", "--profile", "sandbox"]);
    let stdout = text(&output.stdout);
    assert!(
        stdout.contains("2 case(s) need 2 stack(s) in profile `sandbox`"),
        "{stdout}"
    );
    assert!(stdout.contains("dsql     $0.000/hour"), "{stdout}");
    assert!(stdout.contains("s3-data  $0.000/hour"), "{stdout}");
    assert!(
        stdout.contains("tests/s3/s3-up.sh / tests/s3/s3-down.sh"),
        "{stdout}"
    );

    let output = scratch.verify_throwaway(&["all", "--profile", "sandbox", "--yes"]);
    assert!(
        !output.status.success(),
        "the cases could not run in a scratch tree"
    );
    let lines = log_lines(&scratch);
    let position = |needle: &str| {
        lines
            .iter()
            .position(|line| line.starts_with(needle))
            .unwrap_or_else(|| panic!("{needle} not in {lines:?}"))
    };
    assert!(position("tests/db/dsql-up.sh") < position("tests/s3/s3-up.sh"));
    assert!(
        lines.iter().any(|line| line
            .starts_with("aws cloudformation describe-stacks --stack-name kurama-dsql-test-")
            && line.contains("--query Stacks[0].Outputs")),
        "the cluster's outputs were read: {lines:?}"
    );
    let same_rm = position("aws s3 rm s3://kurama-data-test-123456789012-same --recursive");
    let other_rm = position("aws s3 rm s3://kurama-data-test-123456789012-other --recursive");
    assert!(
        lines[other_rm].ends_with("--region us-west-2"),
        "the other bucket is emptied in its own Region: {}",
        lines[other_rm]
    );
    let same_delete = position("aws cloudformation delete-stack --stack-name kurama-s3-data-test-");
    let other_delete = position(
        "aws cloudformation delete-stack --region us-west-2 --stack-name kurama-s3-data-test-",
    );
    assert!(lines[other_delete].ends_with("-other"), "{lines:?}");
    assert!(
        same_rm < same_delete && other_rm < other_delete,
        "{lines:?}"
    );
    assert!(
        position("tests/db/dsql-down.sh") > other_delete,
        "stacks go down in reverse order"
    );
    let report = scratch
        .read("repo/target/agent/verification-report-throwaway.md")
        .expect("the layer's own report");
    assert!(
        report.contains("cleanup:dsql")
            && report.contains("created and deleted (kurama-dsql-test-"),
        "{report}"
    );
    assert!(
        report.contains("cleanup:s3-data")
            && report.contains("created and deleted (kurama-s3-data-test-")
            && report.contains("-other in us-west-2)"),
        "{report}"
    );
    assert!(stacks_left(&scratch).is_empty(), "no stack left");
}

/// The `-other` stack `s3-up.sh` makes in another Region is checked in that
/// Region too: one that outlives `s3-down.sh` is reported as left.
#[test]
fn verify_throwaway_checks_the_other_region_stack_of_s3_data() {
    let scratch = scratch_with_dsql_and_s3("keep-other");
    std::fs::write(scratch.0.join("keep-other"), "").unwrap();
    let output = scratch.verify_throwaway(&["all", "--yes"]);
    assert!(!output.status.success());
    let report = scratch
        .read("repo/target/agent/verification-report-throwaway.md")
        .unwrap();
    assert!(
        report.contains("cleanup:dsql")
            && report.contains("created and deleted (kurama-dsql-test-"),
        "{report}"
    );
    assert!(
        report.contains("left: kurama-s3-data-test-")
            && report.contains("-other in us-west-2 reported as CREATE_COMPLETE"),
        "{report}"
    );
    let left = stacks_left(&scratch);
    assert!(left.len() == 1 && left[0].ends_with("-other"), "{left:?}");
}
