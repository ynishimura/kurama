//! `cargo xtask verify --layer local [all|FEATURE]`: start the databases `db-up` starts, run the cases that declare `[local]` against them, stop the databases whatever happened, and write the reports under target/agent/scenarios-local/; `--layer throwaway` is `verify_throwaway.rs` and `--layer real` is `verify_real_layer.rs`, which share the runner here.
//!
//! The fake layer is what `verify` runs; this is the same generated tests
//! with `KURAMA_CASE_LAYER=local`, so a case is one file on both layers. When
//! Docker is not running nothing is executed and `not-run.txt` says why, so
//! `verify-matrix` shows those cases as UNEXECUTED with the reason instead of
//! failing a gate on a laptop without Docker. Databases a person had up
//! (their `db-up`) are used and left up; when only some of them are, `up.sh`
//! removes and recreates all three and they are left up all the same. Ones
//! this run started from none are removed even when `up.sh` failed
//! part-way, a case failed or the run could not start.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::matrix_inputs::{CaseFile, read_cases};
use crate::{
    Gate, TEST_FEATURES, cargo, conclude_in, coverage_gate_in, databases, list_scenarios,
    qualify_scenarios, root, write_report_in,
};

pub(crate) const USAGE: &str = "usage: cargo xtask verify --layer <local|throwaway|real> [all|FEATURE] [throwaway: --profile NAME --yes]";

/// The containers `tests/db/up.sh` starts; any of them running means a
/// person ran `db-up`, and the run leaves them up.
const CONTAINERS: [&str; 3] = ["kurama-pg17", "kurama-pg18", "kurama-mysql84"];

pub(crate) fn verify_layer(layer: &str, args: &[String]) -> Result<(), String> {
    match layer {
        "local" => {}
        "throwaway" => return crate::verify_throwaway::verify_throwaway(args),
        "real" => return crate::verify_real_layer::verify_real_layer(args),
        other => {
            return Err(format!(
                "no layer is named `{other}`; `local`, `throwaway` and `real` are the ones a case can declare\n{USAGE}"
            ));
        }
    }
    let target = layer_target(layer, args)?;
    let root = root();
    let cases: Vec<CaseFile> = read_cases(&root)?
        .into_iter()
        .filter(|case| case.declares("local") && (target == "all" || case.feature == target))
        .collect();
    let dir = layer_report_dir(layer);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("clear {}: {e}", dir.display()))?;
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let label = format!("layer {layer}, {target}");
    if cases.is_empty() {
        return not_run(&dir, &label, "no case declares [local]");
    }
    if let Err(reason) = docker_running() {
        std::fs::write(dir.join("not-run.txt"), format!("{reason}\n"))
            .map_err(|e| format!("{}: {e}", dir.display()))?;
        return not_run(&dir, &label, &reason);
    }
    let running = databases_running();
    // Any container up counts as the person's: `up.sh` recreates all three
    // when some are missing, and the run then leaves them all up.
    let was_up = running > 0;
    if running < CONTAINERS.len()
        && let Err(error) = databases("up.sh")
    {
        // `up.sh` stops at its first failure, with some containers started.
        if !was_up {
            databases("down.sh")
                .map_err(|down| format!("{error}; {down}: run `cargo xtask db-down`"))?;
        }
        return Err(error);
    }
    let requested: Vec<String> = cases.iter().map(|case| case.id.clone()).collect();
    let run = run_cases(&requested, layer, &[]);
    let stopped = if was_up {
        eprintln!("==> the databases were up before this run and stay up");
        Ok(())
    } else {
        databases("down.sh")
    };
    let mut gates = run?;
    if let Err(error) = stopped {
        gates.push(Gate {
            name: "cleanup".into(),
            ok: false,
            detail: format!("the databases were not removed: {error}; run `cargo xtask db-down`"),
        });
    }
    conclude_in(&dir, &label, gates)
}

/// The one target a layer takes, `all` when none is given.
pub(crate) fn layer_target<'a>(layer: &str, args: &'a [String]) -> Result<&'a str, String> {
    match args {
        [] => Ok("all"),
        [target] if !target.starts_with('-') => Ok(target.as_str()),
        _ => Err(format!(
            "`--layer {layer}` takes one target and no option\n{USAGE}"
        )),
    }
}

/// `target/agent/scenarios-<layer>`, honoring `CARGO_TARGET_DIR`: where the
/// harness writes a report whose `evidence` is that layer.
pub(crate) fn layer_report_dir(layer: &str) -> PathBuf {
    crate::agent_dir().join(format!("scenarios-{layer}"))
}

pub(crate) fn not_run(dir: &Path, label: &str, reason: &str) -> Result<(), String> {
    let report = write_report_in(dir, label, vec![Gate::not_run("scenarios", reason)])?;
    println!("{report}");
    Ok(())
}

/// The cases, through the scenario binary with the layer chosen by
/// environment (and `env`, what the layer hands the cases: a throwaway
/// run's stack outputs), then the coverage gate over what they reported.
pub(crate) fn run_cases(
    requested: &[String],
    layer: &str,
    env: &[(String, String)],
) -> Result<Vec<Gate>, String> {
    let known = list_scenarios()?;
    let mut args = vec!["--exact".to_string()];
    args.extend(qualify_scenarios(requested, &known)?);
    eprintln!("==> verify --layer {layer}: {} cases", requested.len());
    let [mut build, mut run] = scenario_commands(layer, env, &args);
    let built = build.status().map_err(|e| format!("cargo: {e}"))?;
    let status = if built.success() {
        run.status().map_err(|e| format!("cargo: {e}"))?
    } else {
        built
    };
    let mut gates = vec![Gate {
        name: "scenarios".into(),
        ok: status.success(),
        detail: format!("layer {layer}"),
    }];
    if status.success() {
        gates.push(coverage_gate_in(&layer_report_dir(layer), requested)?);
    }
    Ok(gates)
}

/// The scenario binary built with nothing of `env`, then run with it: a
/// build runs the build scripts and proc macros of every dependency, and
/// `env` (the 1Password service account token on the real layer) is for the
/// scenario processes only.
fn scenario_commands(layer: &str, env: &[(String, String)], args: &[String]) -> [Command; 2] {
    let mut build = cargo();
    build
        .args(["test", "--no-run", "--locked", "--test", "scenarios"])
        .args(TEST_FEATURES)
        .current_dir(root());
    let mut run = cargo();
    run.env("KURAMA_CASE_LAYER", layer)
        .envs(
            env.iter()
                .map(|(name, value)| (name.as_str(), value.as_str())),
        )
        .args(["test", "--locked", "--test", "scenarios"])
        .args(TEST_FEATURES)
        .arg("--")
        .args(args)
        .current_dir(root());
    [build, run]
}

/// Why Docker cannot take a container, or nothing when it can. The daemon's
/// own last line is the reason a person reads (`Cannot connect to the Docker
/// daemon ...`), and a missing binary is its own sentence.
fn docker_running() -> Result<(), String> {
    let output = Command::new("docker")
        .arg("info")
        .output()
        .map_err(|e| format!("docker is not installed: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let last = stderr.lines().rev().find(|line| !line.trim().is_empty());
    Err(format!(
        "docker is not running: {}",
        last.unwrap_or("`docker info` failed").trim()
    ))
}

/// How many of the containers `up.sh` starts are running already.
fn databases_running() -> usize {
    CONTAINERS
        .iter()
        .filter(|name| {
            Command::new("docker")
                .args([
                    "ps",
                    "--filter",
                    &format!("name=^{name}$"),
                    "--format",
                    "{{.Names}}",
                ])
                .output()
                .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).trim() == **name)
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layer_environment_reaches_the_run_and_not_the_build() {
        let env = [("OP_SERVICE_ACCOUNT_TOKEN".to_string(), "ops_x".to_string())];
        let [build, run] = scenario_commands("real", &env, &["--exact".to_string()]);
        let set = |command: &Command, name: &str| {
            command
                .get_envs()
                .any(|(key, value)| key == name && value.is_some())
        };
        assert!(build.get_args().any(|arg| arg == "--no-run"));
        assert!(!set(&build, "OP_SERVICE_ACCOUNT_TOKEN"));
        assert!(!set(&build, "KURAMA_CASE_LAYER"));
        assert!(set(&run, "OP_SERVICE_ACCOUNT_TOKEN"));
        assert!(set(&run, "KURAMA_CASE_LAYER"));
    }
}
