//! `cargo xtask verify --layer real [all|FEATURE]`: run the cases that declare `[real]` against the real services the person's own environment reaches, each only when what it `requires` is there, and write the reports under target/agent/scenarios-real/.
//!
//! The real layer is the same generated tests with `KURAMA_CASE_LAYER=real`
//! (`verify_layer.rs` holds the runner): the scenario then talks to the
//! world itself (`Scenario::real_aws`) on the copy of the daily
//! configuration the worktree holds, handed over as `KURAMA_REAL_BASE_CONFIG`.
//! What a case `requires` -- a profile in `~/.aws/config`, a section of that
//! configuration, the keychain entry of the 1Password service account -- is
//! checked before anything runs; a case missing one is written to
//! `not-run.txt` by name with what to prepare, so `verify-matrix` says so on
//! its row instead of failing a gate on a machine without the credentials.
//! The cases run one at a time (a TOTP code is one code per window). The
//! service account token is read from the keychain into the process
//! environment of the scenario binary and nowhere else, and every report the
//! layer wrote is scanned for a credential before the matrix may read it.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::matrix_inputs::{CaseFile, Commit, read_cases, read_layer_commits};
use crate::verify_layer::{layer_report_dir, layer_target, not_run, run_cases};
use crate::verify_real::credential_marks;
use crate::{Gate, conclude_in, root};

/// The keychain entry the 1Password service account token lives in, read for
/// `$USER` the way `kurama` itself reads it (`[onepassword]
/// service_account_keychain`); the environment wins when it is set.
const SERVICE_ACCOUNT_KEYCHAIN: &str = "OP_SERVICE_ACCOUNT_TOKEN";

pub(crate) fn verify_real_layer(args: &[String]) -> Result<(), String> {
    let target = layer_target("real", args)?;
    let root = root();
    let cases: Vec<CaseFile> = read_cases(&root)?
        .into_iter()
        .filter(|case| case.declares_real && (target == "all" || case.feature == target))
        .collect();
    let dir = layer_report_dir("real");
    // One feature's run replaces its own reports and leaves the other
    // features' in place: `verify-real <feature>` runs one feature at a time,
    // and the matrix reads them all together. The report this run writes
    // dates them all, so they are kept only when the last run was on the
    // code the tree holds now; otherwise they go, and the other features
    // read as not run.
    let same_code = read_layer_commits()?
        .get("real")
        .is_some_and(Commit::holds_the_current_code);
    if (target == "all" || !same_code) && dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("clear {}: {e}", dir.display()))?;
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for case in &cases {
        let stale = dir.join(format!("{}.json", case.id));
        if stale.exists() {
            std::fs::remove_file(&stale).map_err(|e| format!("{}: {e}", stale.display()))?;
        }
    }
    let label = format!("layer real, {target}");
    if cases.is_empty() {
        return not_run(&dir, &label, "no case declares [real]");
    }
    let base_config = real_base_config(&root);
    if !base_config.exists() {
        let reason = format!(
            "the base configuration {} does not exist; copy the daily configuration there (`cargo xtask worktree add` does) or set KURAMA_CONFIG_PATH",
            base_config.display()
        );
        write_not_run(&dir, &reason)?;
        return not_run(&dir, &label, &reason);
    }
    let environment = RealEnvironment::read(&base_config);
    let mut skipped = Vec::new();
    let mut requested = Vec::new();
    for case in &cases {
        let missing: Vec<String> = case
            .real_requires
            .iter()
            .filter(|requirement| !environment.has(requirement))
            .map(|requirement| format!("{requirement} ({})", prepare(requirement)))
            .collect();
        if missing.is_empty() {
            requested.push(case.id.clone());
        } else {
            skipped.push(format!("{}: requires {}", case.id, missing.join(", ")));
        }
    }
    write_not_run_lines(&dir, &cases, &skipped)?;
    if requested.is_empty() {
        return not_run(
            &dir,
            &label,
            &format!("no case has what it requires:\n{}", skipped.join("\n")),
        );
    }
    let mut env = vec![
        (
            "KURAMA_REAL_BASE_CONFIG".to_string(),
            base_config.display().to_string(),
        ),
        ("RUST_TEST_THREADS".to_string(), "1".to_string()),
    ];
    if let Some(token) = &environment.service_account_token {
        env.push((SERVICE_ACCOUNT_KEYCHAIN.to_string(), token.clone()));
    }
    // A run that failed half-way may have written reports all the same.
    let mut gates = run_cases(&requested, "real", &env).unwrap_or_else(|error| {
        vec![Gate {
            name: "scenarios".into(),
            ok: false,
            detail: error,
        }]
    });
    gates.push(credential_scan(
        &dir,
        environment.service_account_token.as_deref(),
    )?);
    if !skipped.is_empty() {
        gates.push(Gate {
            name: "requirements".into(),
            ok: true,
            detail: format!("{} case(s) not run: {}", skipped.len(), skipped.join("; ")),
        });
    }
    conclude_in(&dir, &label, gates)
}

/// The configuration the real layer builds on: `KURAMA_CONFIG_PATH` when
/// set, else the copy `cargo xtask worktree add` made under `.kurama/`.
fn real_base_config(root: &Path) -> PathBuf {
    std::env::var_os("KURAMA_CONFIG_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join(".kurama/config.toml"))
}

fn write_not_run(dir: &Path, reason: &str) -> Result<(), String> {
    std::fs::write(dir.join("not-run.txt"), format!("{reason}\n"))
        .map_err(|e| format!("{}: {e}", dir.display()))
}

/// `not-run.txt` as one line per skipped case: the lines of the cases this
/// run selected are replaced, the other features' lines stay, and a file
/// that would say nothing is removed.
fn write_not_run_lines(dir: &Path, cases: &[CaseFile], skipped: &[String]) -> Result<(), String> {
    let path = dir.join("not-run.txt");
    let mut lines: Vec<String> = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .filter(|line| {
            line.split_once(": ")
                .is_some_and(|(id, _)| !cases.iter().any(|case| case.id == id))
        })
        .map(str::to_string)
        .collect();
    lines.extend(skipped.iter().cloned());
    if lines.is_empty() {
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        return Ok(());
    }
    std::fs::write(&path, format!("{}\n", lines.join("\n")))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// What the person's environment holds, read once: the profiles of their
/// `~/.aws/config`, the sections of the base configuration, and the service
/// account token (from the environment, else the keychain), which is handed
/// to the scenario processes and never printed or written.
struct RealEnvironment {
    profiles: Vec<String>,
    sections: Vec<String>,
    service_account_token: Option<String>,
}

impl RealEnvironment {
    fn read(base_config: &Path) -> Self {
        let aws_config = std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(".aws/config"))
            .and_then(|path| std::fs::read_to_string(path).ok())
            .unwrap_or_default();
        let profiles = aws_config
            .lines()
            .filter_map(|line| line.trim().strip_prefix('[')?.strip_suffix(']'))
            .map(|name| {
                name.strip_prefix("profile ")
                    .unwrap_or(name)
                    .trim()
                    .to_string()
            })
            .collect();
        let sections = std::fs::read_to_string(base_config)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.trim().strip_prefix('[')?.strip_suffix(']'))
            .map(|name| name.trim().to_string())
            .collect();
        let service_account_token = std::env::var(SERVICE_ACCOUNT_KEYCHAIN)
            .ok()
            .filter(|token| !token.is_empty())
            .or_else(keychain_service_account_token);
        Self {
            profiles,
            sections,
            service_account_token,
        }
    }

    fn has(&self, requirement: &str) -> bool {
        let Some((kind, name)) = requirement.split_once(':') else {
            return false;
        };
        match kind {
            "profile" => self.profiles.iter().any(|profile| profile == name),
            "auth" | "api" => self.sections.iter().any(|section| {
                section
                    .strip_prefix(&format!("{kind}."))
                    .is_some_and(|rest| {
                        rest == name
                            || rest == format!("\"{name}\"")
                            || rest.starts_with(&format!("{name}."))
                    })
            }),
            "keychain" => name == SERVICE_ACCOUNT_KEYCHAIN && self.service_account_token.is_some(),
            "env" => std::env::var_os(name).is_some_and(|value| !value.is_empty()),
            _ => false,
        }
    }
}

/// What a person prepares for a requirement, said next to the one missing.
fn prepare(requirement: &str) -> String {
    match requirement.split_once(':') {
        Some(("profile", name)) => format!("a `[profile {name}]` in ~/.aws/config"),
        Some(("auth", name)) => format!("an `[auth.{name}]` section in the base configuration"),
        Some(("api", name)) => format!("an `[api.{name}]` section in the base configuration"),
        Some(("keychain", name)) => {
            format!("the keychain entry `{name}` for $USER, or the variable exported")
        }
        Some(("env", name)) => format!("`{name}` exported"),
        _ => "a requirement of the form <kind>:<name>".into(),
    }
}

/// The service account token from the keychain, read for `$USER` the way a
/// person's shell does (`security find-generic-password -w`): the value goes
/// to the scenario processes' environment and nowhere else.
fn keychain_service_account_token() -> Option<String> {
    let user = std::env::var("USER").ok()?;
    let output = Command::new("security")
        .args([
            "find-generic-password",
            "-a",
            &user,
            "-s",
            SERVICE_ACCOUNT_KEYCHAIN,
            "-w",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let token = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!token.is_empty()).then_some(token)
}

/// Every report the layer wrote, scanned the way real evidence is, and for
/// the service account token this run handed the cases: a report that holds
/// either is removed and named, so the matrix never reads it and the value
/// is not in this output either.
fn credential_scan(dir: &Path, token: Option<&str>) -> Result<Gate, String> {
    let mut leaked = Vec::new();
    let files = crate::json_files(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for file in files {
        let text =
            std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let mut marks = credential_marks(&text);
        if token.is_some_and(|token| text.contains(token)) {
            marks.push("the service account token");
        }
        if !marks.is_empty() {
            std::fs::remove_file(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            leaked.push(format!(
                "{} held {} and was removed",
                file.file_name().unwrap_or_default().to_string_lossy(),
                marks.join(", ")
            ));
        }
    }
    Ok(Gate {
        name: "credential scan".into(),
        ok: leaked.is_empty(),
        detail: if leaked.is_empty() {
            "no report holds a credential".into()
        } else {
            leaked.join("; ")
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment(profiles: &[&str], sections: &[&str], token: Option<&str>) -> RealEnvironment {
        RealEnvironment {
            profiles: profiles.iter().map(|s| s.to_string()).collect(),
            sections: sections.iter().map(|s| s.to_string()).collect(),
            service_account_token: token.map(str::to_string),
        }
    }

    #[test]
    fn a_requirement_is_met_by_what_the_environment_holds() {
        let env = environment(
            &["default", "kurama-sandbox"],
            &[
                "onepassword",
                "auth.kurama-real-oauth",
                "api.kurama-real-oauth",
                "auth.\"quoted\"",
            ],
            Some("token"),
        );
        assert!(env.has("profile:kurama-sandbox"));
        assert!(!env.has("profile:kurama-real"));
        assert!(env.has("auth:kurama-real-oauth"));
        assert!(env.has("api:kurama-real-oauth"));
        assert!(env.has("auth:quoted"));
        assert!(!env.has("api:github"));
        assert!(env.has("keychain:OP_SERVICE_ACCOUNT_TOKEN"));
        assert!(!env.has("keychain:OTHER"));
        assert!(
            !env.has("bucket:kurama-sandbox"),
            "an unknown kind is never met"
        );
        assert!(!environment(&[], &[], None).has("keychain:OP_SERVICE_ACCOUNT_TOKEN"));
    }

    #[test]
    fn what_to_prepare_names_the_place() {
        assert_eq!(
            prepare("profile:kurama-sandbox"),
            "a `[profile kurama-sandbox]` in ~/.aws/config"
        );
        assert_eq!(
            prepare("auth:x"),
            "an `[auth.x]` section in the base configuration"
        );
        assert!(prepare("keychain:OP_SERVICE_ACCOUNT_TOKEN").contains("keychain entry"));
    }

    #[test]
    fn a_report_with_a_credential_is_removed_and_named_without_its_value() {
        let dir =
            std::env::temp_dir().join(format!("xtask-credential-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("clean.json"), "{\"scenario\":\"a\"}").unwrap();
        std::fs::write(
            dir.join("leaky.json"),
            "{\"stderr\":\"Authorization: Bearer abc.def\"}",
        )
        .unwrap();
        std::fs::write(dir.join("token.json"), "{\"stderr\":\"ops_value\"}").unwrap();
        let gate = credential_scan(&dir, Some("ops_value")).unwrap();
        assert!(!gate.ok);
        assert!(gate.detail.contains("leaky.json held"), "{}", gate.detail);
        assert!(!gate.detail.contains("abc.def"), "{}", gate.detail);
        assert!(
            gate.detail
                .contains("token.json held the service account token and was removed"),
            "{}",
            gate.detail
        );
        assert!(!gate.detail.contains("ops_value"), "{}", gate.detail);
        assert!(!dir.join("leaky.json").exists());
        assert!(dir.join("clean.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
