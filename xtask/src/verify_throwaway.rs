//! `cargo xtask verify --layer throwaway [all|FEATURE] [--profile NAME] [--yes]`: create the disposable AWS stacks the `[throwaway]` cases name, run those cases against them, delete every stack whatever happened, and write the reports under target/agent/scenarios-throwaway/ with a cleanup gate per stack.
//!
//! Creating a stack bills the account, so without `--yes` this prints what
//! it would create with the estimate and stops; `not-run.txt` then tells
//! `verify-matrix` to show the cases as UNEXECUTED with "approval needed"
//! and the command a person runs. A gate never passes `--yes`.
//!
//! With `--yes`, every AWS call goes through `kurama exec <profile> --`, so
//! the credentials live in the child's environment and nowhere else; the
//! MFA session is cached in one file next to the report directory, never in
//! it, because every `.json` there is read as a report (the file-backed
//! cache of the `test-fakes` build, mode 600, removed at the end) so one
//! TOTP serves the scripts and every case. Every stack carries this run's
//! own name, and one AWS already knows is refused rather than used and
//! deleted. Before the first stack is created, `cleanup.sh` next to the
//! reports lists every `down` this run would need with the names, Region
//! and configuration it runs with, for the case where the run itself is
//! killed; it stays while a stack may be left, and while it is there the
//! next run stops before it clears the directory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::matrix_inputs::{CaseFile, read_cases};
use crate::throwaway_stacks::{
    CloudFormationStack, Stack, absence_verdict, cloudformation_stacks, estimate, needed,
    output_variables, script_variables, stack_name,
};
use crate::verify_layer::{layer_report_dir, not_run, run_cases};
use crate::{Gate, TEST_FEATURES, cargo, root, target_dir, write_report_in};

pub(crate) const USAGE: &str =
    "usage: cargo xtask verify --layer throwaway [all|FEATURE] [--profile NAME] [--yes]";

/// The sandbox the stacks are created in when `--profile` says nothing.
const DEFAULT_PROFILE: &str = "kurama-sandbox";

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Options {
    pub(crate) target: String,
    pub(crate) profile: String,
    pub(crate) approved: bool,
}

pub(crate) fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut options = Options {
        target: "all".to_string(),
        profile: DEFAULT_PROFILE.to_string(),
        approved: false,
    };
    let mut target_given = false;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--yes" => options.approved = true,
            "--profile" => {
                options.profile = args
                    .next()
                    .cloned()
                    .ok_or_else(|| format!("--profile needs a name\n{USAGE}"))?;
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown option `{other}`\n{USAGE}"));
            }
            other if !target_given => {
                options.target = other.to_string();
                target_given = true;
            }
            other => return Err(format!("one target, not `{other}` too\n{USAGE}")),
        }
    }
    Ok(options)
}

/// The command a person runs to approve exactly this run.
fn approval_command(options: &Options) -> String {
    format!(
        "cargo xtask verify --layer throwaway {} --profile {} --yes",
        options.target, options.profile
    )
}

pub(crate) fn verify_throwaway(args: &[String]) -> Result<(), String> {
    let options = parse_args(args)?;
    let root = root();
    let cases: Vec<CaseFile> = read_cases(&root)?
        .into_iter()
        .filter(|case| {
            case.declares("throwaway")
                && (options.target == "all" || case.feature == options.target)
        })
        .collect();
    let dir = layer_report_dir("throwaway");
    let leftover = dir.join("cleanup.sh");
    if leftover.exists() {
        return Err(format!(
            "a previous run may have left stacks: run `sh {}`, which removes each one and then itself",
            leftover.display()
        ));
    }
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("clear {}: {e}", dir.display()))?;
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let label = format!("layer throwaway, {}", options.target);
    if cases.is_empty() {
        return not_run(&dir, &label, "no case declares [throwaway]");
    }
    let stacks = needed(
        cases
            .iter()
            .map(|case| (case.id.as_str(), case.stacks.as_slice())),
    )?;
    let estimate = estimate(&stacks);
    println!(
        "==> {} case(s) need {} stack(s) in profile `{}`:\n{estimate}",
        cases.len(),
        stacks.len(),
        options.profile
    );
    if !options.approved {
        // One line of reason for the matrix and the gate table, the
        // estimate below it for the person who reads the file.
        let reason = format!(
            "approval needed: creating these stacks bills the `{}` account; run `{}`",
            options.profile,
            approval_command(&options)
        );
        std::fs::write(dir.join("not-run.txt"), format!("{reason}\n{estimate}"))
            .map_err(|e| format!("{}: {e}", dir.display()))?;
        not_run(&dir, &label, &reason)?;
        return Err(format!(
            "not run: the stacks above need a person's approval; `{}` creates, verifies and deletes them",
            approval_command(&options)
        ));
    }

    let run = Run::prepare(&root, &dir, &options, &stacks)?;
    let requested: Vec<String> = cases.iter().map(|case| case.id.clone()).collect();
    let outcome = run.create_verify_delete(&requested);
    let passed = outcome.iter().all(|gate| gate.ok);
    let report_gates = outcome.clone();
    let report = write_report_in(&dir, &label, outcome)?;
    println!("{report}");
    if passed {
        Ok(())
    } else {
        Err(failure_message(&report_gates))
    }
}

/// The last line of a failed run: the gates that failed, and whether a stack
/// was left behind, which is the one thing a person has to act on now.
fn failure_message(gates: &[Gate]) -> String {
    let failed: Vec<&str> = gates
        .iter()
        .filter(|gate| !gate.ok)
        .map(|gate| gate.name.as_str())
        .collect();
    let stacks = if failed.iter().any(|name| name.starts_with("cleanup:")) {
        "a stack was left behind: its `cleanup` gate names the command that removes it"
    } else {
        "every stack was deleted; see the report above"
    };
    format!("verification failed: {}; {stacks}", failed.join(", "))
}

/// One approved run: the binary that carries the credentials, where the
/// MFA session is cached, the run's id every stack name carries, and the
/// stacks in the order they are created.
struct Run<'a> {
    root: PathBuf,
    binary: PathBuf,
    profile: String,
    run: String,
    /// What every `kurama exec` of the run is handed: the scripts' variables
    /// and the configuration.
    variables: Vec<(String, String)>,
    session_cache: PathBuf,
    cleanup_script: PathBuf,
    stacks: &'a [&'static Stack],
}

impl<'a> Run<'a> {
    fn prepare(
        root: &Path,
        dir: &Path,
        options: &Options,
        stacks: &'a [&'static Stack],
    ) -> Result<Self, String> {
        let binary = kurama_binary(root)?;
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        let run = format!("{seconds}-{}", std::process::id());
        let mut variables = script_variables(stacks, &run);
        let config = std::env::var_os("KURAMA_CONFIG_PATH")
            .map(PathBuf::from)
            .or_else(|| {
                let copied = root.join(".kurama/config.toml");
                copied.exists().then_some(copied)
            });
        if let Some(config) = config {
            variables.push(("KURAMA_CONFIG_PATH".into(), config.display().to_string()));
        }
        // One name for every run, emptied here: a killed run's session is
        // overwritten by the next one instead of piling up.
        let session_cache = dir.with_file_name("throwaway-session.json");
        create_private(&session_cache)?;
        let run = Self {
            root: root.to_path_buf(),
            binary,
            profile: options.profile.clone(),
            run,
            variables,
            session_cache,
            cleanup_script: dir.join("cleanup.sh"),
            stacks,
        };
        let mut script = String::from(
            "#!/bin/sh\n# Written before the first stack was created: what removes every stack this run needs, with the names and configuration it ran with. A failed down does not stop the others; it removes itself once every down succeeded.\nset -x\nfailed=\n",
        );
        for stack in stacks.iter().rev() {
            script.push_str(&run.down_command(stack));
            script.push_str(" || failed=1\n");
        }
        script.push_str("[ -z \"$failed\" ] && rm -f -- \"$0\"\n");
        std::fs::write(&run.cleanup_script, script)
            .map_err(|e| format!("{}: {e}", run.cleanup_script.display()))?;
        Ok(run)
    }

    /// `kurama exec <profile> -- <command...>` from the repository root,
    /// with the run's variables.
    fn command(&self, command: &[&str]) -> Command {
        let mut process = Command::new(&self.binary);
        process
            .arg("exec")
            .arg(&self.profile)
            .arg("--")
            .args(command)
            .current_dir(&self.root)
            .env("KURAMA_TEST_SESSION_CACHE_FILE", &self.session_cache)
            .envs(
                self.variables
                    .iter()
                    .map(|(name, value)| (name.as_str(), value.as_str())),
            )
            .stdin(Stdio::null());
        process
    }

    /// The shell line that runs the stack's `down` the way this run does,
    /// for `cleanup.sh` and a `cleanup` gate that failed.
    fn down_command(&self, stack: &Stack) -> String {
        let mut line: Vec<String> = self
            .variables
            .iter()
            .map(|(name, value)| format!("{name}={}", quote(value)))
            .collect();
        line.push(quote(&self.binary.display().to_string()));
        line.push("exec".into());
        line.push(quote(&self.profile));
        line.push("--".into());
        line.push(quote(&self.root.join(stack.down).display().to_string()));
        line.join(" ")
    }

    fn create_verify_delete(&self, requested: &[String]) -> Vec<Gate> {
        let mut gates = Vec::new();
        let mut created: Vec<&Stack> = Vec::new();
        let mut variables: BTreeMap<String, String> = BTreeMap::new();
        let mut failure: Option<String> = None;
        'create: for stack in self.stacks {
            for existing in cloudformation_stacks(stack, &self.run) {
                if let Err(why) = self.absent(&existing) {
                    failure = Some(format!(
                        "{}: {existing} is not free to create ({why}); a run touches only the stacks it creates",
                        stack.name
                    ));
                    break 'create;
                }
            }
            eprintln!("==> creating {} ({})", stack.name, stack.creation);
            match self.command(&[stack.up]).status() {
                Ok(status) if status.success() => created.push(stack),
                Ok(status) => {
                    // The scripts trap their own failures: the stack may exist.
                    created.push(stack);
                    failure = Some(format!("{}: {} exited with {status}", stack.name, stack.up));
                    break;
                }
                Err(error) => {
                    failure = Some(format!("{}: {}: {error}", stack.name, stack.up));
                    break;
                }
            }
            match self.outputs(stack) {
                Ok(found) => variables.extend(found),
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
        match failure {
            None => {
                variables.insert("KURAMA_THROWAWAY_PROFILE".into(), self.profile.clone());
                variables.insert(
                    "KURAMA_THROWAWAY_SESSION_CACHE".into(),
                    self.session_cache.display().to_string(),
                );
                variables.insert(
                    "KURAMA_STACK_RDS_CA_FILE".into(),
                    self.root
                        .join("tests/db/rds-global-bundle.pem")
                        .display()
                        .to_string(),
                );
                let env: Vec<(String, String)> = variables.into_iter().collect();
                match run_cases(requested, "throwaway", &env) {
                    Ok(run) => gates.extend(run),
                    Err(error) => gates.push(Gate {
                        name: "scenarios".into(),
                        ok: false,
                        detail: error,
                    }),
                }
            }
            Some(error) => gates.push(Gate {
                name: "create".into(),
                ok: false,
                detail: format!("{error}; the cases did not run"),
            }),
        }
        let mut all_deleted = true;
        for stack in created.iter().rev() {
            eprintln!("==> deleting {} ({})", stack.name, stack.deletion);
            let instances = cloudformation_stacks(stack, &self.run);
            let deleted = match self.command(&[stack.down]).status() {
                Ok(status) if status.success() => {
                    let left: Vec<String> = instances
                        .iter()
                        .filter_map(|instance| {
                            self.absent(instance)
                                .err()
                                .map(|why| format!("{instance} {why}"))
                        })
                        .collect();
                    if left.is_empty() {
                        Ok(())
                    } else {
                        Err(left.join("; "))
                    }
                }
                Ok(status) => Err(format!("{} exited with {status}", stack.down)),
                Err(error) => Err(format!("{}: {error}", stack.down)),
            };
            let names = instances
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" and ");
            gates.push(match deleted {
                Ok(()) => Gate {
                    name: format!("cleanup:{}", stack.name),
                    ok: true,
                    detail: format!("created and deleted ({names})"),
                },
                Err(why) => {
                    all_deleted = false;
                    Gate {
                        name: format!("cleanup:{}", stack.name),
                        ok: false,
                        detail: format!(
                            "left: {why}; remove it with `{}` (stack {names})",
                            self.down_command(stack)
                        ),
                    }
                }
            });
        }
        let _ = std::fs::remove_file(&self.session_cache);
        if all_deleted {
            let _ = std::fs::remove_file(&self.cleanup_script);
        } else {
            eprintln!(
                "==> a stack was left behind; {} removes every stack of this run",
                self.cleanup_script.display()
            );
        }
        gates
    }

    /// The outputs of a stack that is up, as the variables its placeholders read.
    fn outputs(&self, stack: &Stack) -> Result<BTreeMap<String, String>, String> {
        let name = stack_name(stack, &self.run);
        let output = self
            .command(&[
                "aws",
                "cloudformation",
                "describe-stacks",
                "--stack-name",
                &name,
                "--query",
                "Stacks[0].Outputs",
                "--output",
                "json",
            ])
            .stderr(Stdio::inherit())
            .output()
            .map_err(|e| format!("{}: describe-stacks: {e}", stack.name))?;
        if !output.status.success() {
            return Err(format!(
                "{}: describe-stacks exited with {}",
                stack.name, output.status
            ));
        }
        output_variables(stack, &name, &String::from_utf8_lossy(&output.stdout))
    }

    /// Whether AWS does not know the stack: asked before `up`, so a run
    /// never takes over a stack it did not create, and after `down`.
    fn absent(&self, stack: &CloudFormationStack) -> Result<(), String> {
        let mut command = vec!["aws", "cloudformation", "describe-stacks"];
        if let Some(region) = &stack.region {
            command.extend(["--region", region.as_str()]);
        }
        command.extend(["--stack-name", stack.name.as_str(), "--output", "json"]);
        let output = self
            .command(&command)
            .output()
            .map_err(|e| format!("describe-stacks: {e}"))?;
        absence_verdict(
            output.status.success(),
            &String::from_utf8_lossy(&output.stdout),
            &String::from_utf8_lossy(&output.stderr),
        )
    }
}

/// `value` as one single-quoted shell word.
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// The kurama that carries the credentials: `KURAMA_BINARY` when set (the
/// tests point it at a fake), else this tree's `test-fakes` debug build,
/// built now so the file-backed session cache is compiled in.
fn kurama_binary(root: &Path) -> Result<PathBuf, String> {
    if let Some(binary) = std::env::var_os("KURAMA_BINARY") {
        return Ok(PathBuf::from(binary));
    }
    eprintln!("==> building kurama with test-fakes (the session cache is a file in this build)");
    let status = cargo()
        .args(["build", "--locked"])
        .args(TEST_FEATURES)
        .current_dir(root)
        .status()
        .map_err(|e| format!("cargo: {e}"))?;
    if !status.success() {
        return Err("cargo build failed".into());
    }
    Ok(target_dir().join("debug").join("kurama"))
}

/// A file only its owner can read, holding no session yet, for the MFA
/// session of this run (emptied when it exists already). It holds an empty
/// JSON map rather than nothing: kurama's file-backed cache refuses to load
/// or store into anything it cannot parse.
fn create_private(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, "{}").map_err(|e| format!("{}: {e}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args;

    fn gate(name: &str, ok: bool) -> Gate {
        Gate {
            name: name.into(),
            ok,
            detail: String::new(),
        }
    }

    #[test]
    fn the_failure_line_names_the_gates_that_failed() {
        let cases_only =
            failure_message(&[gate("scenarios", false), gate("cleanup:iam-api", true)]);
        assert_eq!(
            cases_only,
            "verification failed: scenarios; every stack was deleted; see the report above"
        );
        let left = failure_message(&[
            gate("scenarios", true),
            gate("cleanup:iam-api", false),
            gate("cleanup:dsql", true),
        ]);
        assert_eq!(
            left,
            "verification failed: cleanup:iam-api; a stack was left behind: its `cleanup` gate names the command that removes it"
        );
    }

    #[test]
    fn the_session_cache_starts_as_a_document_the_file_cache_reads() {
        // kurama's file-backed cache reads a JSON map and refuses anything
        // else, store included: an empty file made every process fetch a
        // TOTP of its own, and AWS rejects a code used twice in one window.
        let dir = std::env::temp_dir().join(format!("kurama-session-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("throwaway-session.json");
        std::fs::write(&path, "stale").unwrap();
        create_private(&path).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let sessions: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(&content).expect("an empty JSON map");
        assert!(sessions.is_empty());
    }

    #[test]
    fn the_arguments_are_a_target_a_profile_and_the_approval() {
        assert_eq!(
            parse_args(&args(&[])).unwrap(),
            Options {
                target: "all".into(),
                profile: "kurama-sandbox".into(),
                approved: false
            }
        );
        assert_eq!(
            parse_args(&args(&["database", "--profile", "sandbox", "--yes"])).unwrap(),
            Options {
                target: "database".into(),
                profile: "sandbox".into(),
                approved: true
            }
        );
        assert!(
            parse_args(&args(&["--profile"]))
                .unwrap_err()
                .contains("needs a name")
        );
        assert!(
            parse_args(&args(&["--force"]))
                .unwrap_err()
                .contains("unknown option")
        );
        assert!(
            parse_args(&args(&["a", "b"]))
                .unwrap_err()
                .contains("one target")
        );
    }

    #[test]
    fn a_quoted_word_survives_the_shell() {
        assert_eq!(quote("/a b/c"), "'/a b/c'");
        assert_eq!(quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn the_approval_command_repeats_the_target_and_profile() {
        let options = parse_args(&args(&["api-client", "--profile", "kurama-sandbox"])).unwrap();
        assert_eq!(
            approval_command(&options),
            "cargo xtask verify --layer throwaway api-client --profile kurama-sandbox --yes"
        );
    }
}
