//! `kurama preset setup <ID> [--as NAME] [--auth-as NAME] [--set k=v]... [--dry-run | --offline] [--json]`: one preset from nothing to a first answer, step by step, each step saying whether it is done and what a person runs next.
//!
//! The steps, in order:
//!
//! 1. `configure`: the preset expanded against config.toml (reusing only a
//!    compatible `[auth.*]`) and its sections appended through the config
//!    writer, checked whole first -- unless config.toml already has the
//!    `[api.*]`, which is then kept as it is, so a second run after a stop
//!    anywhere resumes instead of failing on a taken name. Inputs still
//!    missing stop the run here with the preset's own setup steps.
//! 2. `check`: config.toml is read again and the `[api.*]` typed.
//! 3. `credential`: whether the API can be used now, from what `status`
//!    reads: configured is not yet connected.
//! 4. `agent`: kurama's Agent Skill written for Claude Code
//!    (`~/.claude/skills/kurama/SKILL.md`), as `agent install` writes it.
//! 5. `first_read`: the preset's example, when it is a plain path, sent as a
//!    GET through `ApiRuntime::call` -- the one request this command makes,
//!    held to the `[agent]` policy and audited like `kurama api`. On a
//!    terminal a grant that needs a person runs here; without one it stops
//!    with the login hint.
//!
//! `--dry-run` plans and checks `configure` and does nothing else: the TOML
//! that would be appended is in the report, nothing is written, read from a
//! secret store or sent. `--offline` runs every step but the first read.
//! The report goes to stdout whatever happened. The first step that failed
//! is the run's error, unchanged, so its code, exit code and hint are the
//! ones the same failure has anywhere else in kurama.

use std::sync::Arc;

use anyhow::Result;
use serde::Serialize;

use super::agent::AGENT_SKILL;
use super::agent_install::place;
use super::api::http_error;
use super::preset::{check_plan, plan_against_file};
use super::status_ready::readiness_rows;
use crate::adapters::config::writer::ConfigFile;
use crate::adapters::config::{ApiProfile, Config};
use crate::adapters::utils::path::get_home_dir;
use crate::domain::functions::api_request::{join_base_path, with_default_headers, with_headers};
use crate::domain::functions::preset_render::{AuthAction, PresetError, PresetRequest};
use crate::domain::functions::skill_install::{KURAMA_SKILL, SkillStatus};
use crate::domain::functions::source_readiness::{Readiness, named};
use crate::domain::types::HttpRequest;
use crate::domain::types::preset::{Preset, find_preset};
use crate::shell::agent_policy::{api_policy, check_api_request, is_agent_run};
use crate::shell::api_runtime::{ApiRuntime, ApiRuntimeOptions};
use crate::shell::cli::client::json_line;
use crate::shell::cli::error_code::ErrorCode;

/// How far `preset setup` goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupMode {
    /// Every step.
    Full,
    /// Every step but the first read: nothing is read from a secret store
    /// or sent.
    Offline,
    /// `configure` planned and checked; nothing written, read or sent.
    DryRun,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum StepState {
    Done,
    /// `--dry-run`: checked, and what it would do is in the report.
    Planned,
    /// A person has to do what `next` says; nothing failed.
    NeedsAction,
    Failed,
    /// Not run: an earlier step stopped the run, or the mode leaves it out.
    Skipped,
}

impl StepState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Planned => "planned",
            Self::NeedsAction => "needs_action",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Serialize)]
struct Step {
    step: &'static str,
    state: StepState,
    detail: String,
    /// What a person runs or does next, in order.
    next: Vec<String>,
}

impl Step {
    fn new(step: &'static str, state: StepState, detail: impl Into<String>) -> Self {
        Self {
            step,
            state,
            detail: detail.into(),
            next: Vec::new(),
        }
    }

    fn next(mut self, next: Vec<String>) -> Self {
        self.next = next;
        self
    }
}

/// The `[auth.*]` the API uses and what `configure` did with it: `add`,
/// `reuse` (a compatible one the file had), or `kept` (the `[api.*]` was
/// already there, with whatever auth it names).
#[derive(Debug, Serialize)]
struct AuthOutcome {
    name: Option<String>,
    action: &'static str,
}

#[derive(Serialize)]
struct SetupReport {
    schema_version: u8,
    kind: &'static str,
    preset: &'static str,
    api: String,
    auth: Option<AuthOutcome>,
    /// The config.toml in use.
    path: Option<String>,
    dry_run: bool,
    /// Every step is done: configured, connected and registered.
    complete: bool,
    steps: Vec<Step>,
    /// The TOML `configure` appended, or would append on a dry run.
    toml: Option<String>,
    /// Scopes a reused auth lacks, and the checks' warnings.
    warnings: Vec<String>,
}

/// The steps in order, for the ones a stop or the mode leaves out.
const STEPS: [&str; 5] = ["configure", "check", "credential", "agent", "first_read"];

/// What `configure` did, for the report.
#[derive(Default)]
struct Configured {
    path: Option<String>,
    auth: Option<AuthOutcome>,
    toml: Option<String>,
    warnings: Vec<String>,
}

pub async fn run(id: &str, request: &PresetRequest, mode: SetupMode, json: bool) -> Result<()> {
    let preset = find_preset(id).ok_or_else(|| PresetError::NotFound(id.to_owned()))?;
    let api_name = request
        .api_name
        .clone()
        .unwrap_or_else(|| preset.api.name.to_owned());
    let mut steps = Vec::new();
    let mut configured = Configured::default();
    let failure = run_steps(
        preset,
        request,
        &api_name,
        mode,
        &mut steps,
        &mut configured,
    )
    .await;
    let skipped = match mode {
        SetupMode::DryRun => "--dry-run writes nothing and sends nothing",
        SetupMode::Full | SetupMode::Offline => "an earlier step stopped the run",
    };
    for name in STEPS {
        if !steps.iter().any(|step| step.step == name) {
            steps.push(Step::new(name, StepState::Skipped, skipped));
        }
    }
    let report = SetupReport {
        schema_version: 1,
        kind: "preset_setup",
        preset: preset.id,
        api: api_name,
        auth: configured.auth,
        path: configured.path,
        dry_run: mode == SetupMode::DryRun,
        complete: steps.iter().all(|step| step.state == StepState::Done),
        steps,
        toml: configured.toml,
        warnings: configured.warnings,
    };
    if json {
        println!("{}", json_line(&report));
    } else {
        print!("{}", render_text(&report));
    }
    failure.map_or(Ok(()), Err)
}

/// Run the steps into `steps`; the error of the first that failed.
async fn run_steps(
    preset: &'static Preset,
    request: &PresetRequest,
    api_name: &str,
    mode: SetupMode,
    steps: &mut Vec<Step>,
    configured: &mut Configured,
) -> Option<anyhow::Error> {
    let (step, stop) = configure(preset, request, api_name, mode, configured).await;
    steps.push(step);
    if stop.is_some() || mode == SetupMode::DryRun {
        return stop;
    }

    let config = match ConfigFile::open().and_then(|file| Config::parse(file.content())) {
        Ok(config) => config,
        Err(error) => return Some(failed(steps, "check", error)),
    };
    let api = match config.api_profile(api_name) {
        Ok(Some(api)) => api,
        Ok(None) => unreachable!("configure found or added [api.{api_name}]"),
        Err(error) => return Some(failed(steps, "check", error)),
    };
    steps.push(Step::new(
        "check",
        StepState::Done,
        match &api.auth {
            Some(auth) => format!("config.toml loads; [api.{api_name}] uses [auth.{auth}]"),
            None => format!("config.toml loads; [api.{api_name}] sends no credential"),
        },
    ));

    let mut failure = None;
    match readiness_rows(&config).await {
        Ok(rows) => {
            let row = named(&rows, "api", api_name);
            let state = match row.state {
                Readiness::Ready | Readiness::WillPrompt => StepState::Done,
                Readiness::NeedsHuman => StepState::NeedsAction,
                Readiness::Misconfigured => StepState::Failed,
            };
            steps.push(Step::new("credential", state, row.reason).next(row.next_actions));
        }
        Err(error) => failure = Some(failed(steps, "credential", error)),
    }

    match register_skill() {
        Ok(step) => steps.push(step),
        Err(error) => {
            let error = failed(steps, "agent", error);
            failure.get_or_insert(error);
        }
    }

    if mode == SetupMode::Offline {
        steps.push(
            Step::new(
                "first_read",
                StepState::Skipped,
                "--offline reads no secret and sends nothing",
            )
            .next(vec![resume_command(preset, request)]),
        );
        return failure;
    }
    let (step, error) = first_read(preset, &api, config).await;
    steps.push(step);
    failure.or(error)
}

/// The command that runs the setup again for the same sections: the names
/// it was given, not the inputs, which configure no longer needs.
fn resume_command(preset: &Preset, request: &PresetRequest) -> String {
    let mut command = format!("kurama preset setup {}", preset.id);
    for (flag, name) in [
        ("--as", &request.api_name),
        ("--auth-as", &request.auth_name),
    ] {
        if let Some(name) = name {
            command.push_str(&format!(" {flag} {name}"));
        }
    }
    command
}

/// The step `name` as failed with `error`'s code and hint, pushed onto
/// `steps`; the error.
fn failed(steps: &mut Vec<Step>, name: &'static str, error: anyhow::Error) -> anyhow::Error {
    let (step, error) = failed_step(name, error);
    steps.push(step);
    error
}

/// The step `name` as failed with `error`'s code and hint, and the error.
fn failed_step(name: &'static str, error: anyhow::Error) -> (Step, anyhow::Error) {
    let code = ErrorCode::classify(&error);
    let step = Step::new(name, StepState::Failed, format!("error[{}]", code.as_str()))
        .next(code.hint(&error).into_iter().collect());
    (step, error)
}

/// The `[api.*]` kept when config.toml has it, else the preset's sections
/// appended (planned and checked only on a dry run); the error that stops
/// the run.
async fn configure(
    preset: &'static Preset,
    request: &PresetRequest,
    api_name: &str,
    mode: SetupMode,
    configured: &mut Configured,
) -> (Step, Option<anyhow::Error>) {
    let stopped = |error| {
        let (step, error) = failed_step("configure", error);
        (step, Some(error))
    };
    let file = match ConfigFile::open() {
        Ok(file) => file,
        Err(error) => return stopped(error),
    };
    let path = file.path().display().to_string();
    configured.path = Some(path.clone());
    match Config::parse(file.content()) {
        Ok(config) if config.api.contains_key(api_name) => {
            configured.auth = Some(AuthOutcome {
                name: config.api[api_name].auth.clone(),
                action: "kept",
            });
            return (
                Step::new(
                    "configure",
                    StepState::Done,
                    format!("[api.{api_name}] is already in {path}; kept as it is"),
                ),
                None,
            );
        }
        Ok(_) => {}
        Err(error) => return stopped(error),
    }
    let plan = match plan_against_file(preset.id, request) {
        Ok((_, plan)) => plan,
        Err(error) => return stopped(error),
    };
    configured.auth = Some(AuthOutcome {
        name: Some(plan.auth.clone()),
        action: plan.auth_action.as_str(),
    });
    configured.warnings = plan.warnings.clone();
    if let Err(PresetError::MissingInputs { keys, .. }) = &plan.fragment {
        let error = plan.fragment.clone().expect_err("matched above");
        return (
            Step::new(
                "configure",
                StepState::NeedsAction,
                format!("preset {} needs {}", preset.id, keys.join(", ")),
            )
            .next(plan.setup[..plan.after_append].to_vec()),
            Some(error.into()),
        );
    }
    let (appended, validated) = match check_plan(&file, &plan).await {
        Ok(checked) => checked,
        Err(error) => return stopped(error),
    };
    configured.warnings = validated.warnings.clone();
    configured.toml = plan.fragment.as_ref().ok().cloned();
    let sections: Vec<String> = appended
        .units
        .iter()
        .map(|unit| format!("[{unit}]"))
        .collect();
    let what = match plan.auth_action {
        AuthAction::Add => sections.join(", "),
        AuthAction::Reuse => format!("{} (reuses [auth.{}])", sections.join(", "), plan.auth),
    };
    if mode == SetupMode::DryRun {
        return (
            Step::new(
                "configure",
                StepState::Planned,
                format!("would add {what} to {path}; nothing was written"),
            ),
            None,
        );
    }
    match file.save(validated) {
        Ok(()) => (
            Step::new(
                "configure",
                StepState::Done,
                format!("added {what} to {path}"),
            ),
            None,
        ),
        Err(error) => stopped(error.into()),
    }
}

/// kurama's Agent Skill for Claude Code: written when it is missing or
/// says something else.
fn register_skill() -> Result<Step> {
    let dir = get_home_dir()?.join(".claude").join("skills");
    let row = place(&dir, KURAMA_SKILL, None, AGENT_SKILL, false)?;
    let path = row.path.unwrap_or_default();
    Ok(Step::new(
        "agent",
        StepState::Done,
        match row.status {
            SkillStatus::Unchanged => format!("Claude Code already has kurama's Skill: {path}"),
            SkillStatus::Written | SkillStatus::Skipped => {
                format!("wrote kurama's Skill for Claude Code: {path}")
            }
        },
    )
    .next(vec![
        "kurama agent install  # adds a Skill per [api.*] with a description".into(),
    ]))
}

/// The preset's example as a GET, when it is a plain path: the one request
/// this command sends.
async fn first_read(
    preset: &'static Preset,
    api: &ApiProfile,
    config: Config,
) -> (Step, Option<anyhow::Error>) {
    let path = preset.api.example;
    if !path.starts_with('/') || path.contains(char::is_whitespace) {
        return (
            Step::new(
                "first_read",
                StepState::Skipped,
                "the preset's example is not a plain GET; try it yourself",
            )
            .next(vec![format!("kurama api {} {path}", api.name)]),
            None,
        );
    }
    let request = with_default_headers(with_headers(
        HttpRequest::new("GET", join_base_path(&api.base_url, path)),
        api.headers.iter(),
    ));
    let policy = api_policy(&config, &api.name, is_agent_run(), false);
    let sent = async {
        check_api_request(policy.as_ref(), &request)?;
        crate::shell::audit::note_request(&request.method, &request.url);
        let runtime = ApiRuntime::from_config(Arc::new(config), ApiRuntimeOptions::default())?;
        let response = runtime.call(api, request).await?;
        crate::shell::audit::note_status(response.status);
        if (200..300).contains(&response.status) {
            Ok(response.status)
        } else {
            Err(http_error(&response).into())
        }
    };
    match sent.await {
        Ok(status) => (
            Step::new(
                "first_read",
                StepState::Done,
                format!("GET {path} answered HTTP {status}"),
            )
            .next(vec![format!("kurama api {} {path}", api.name)]),
            None,
        ),
        Err(error) => {
            let (mut step, error) = failed_step("first_read", error);
            step.detail = format!("GET {path} failed: {}", step.detail);
            (step, Some(error))
        }
    }
}

/// One line per step, its `next` indented under it, then the warnings and,
/// on a dry run, the TOML that would be appended.
fn render_text(report: &SetupReport) -> String {
    let mut out = format!(
        "# kurama preset setup {} ([api.{}])\n",
        report.preset, report.api
    );
    for step in &report.steps {
        out.push_str(&format!(
            "{:<12} {:<13} {}\n",
            step.step,
            step.state.as_str(),
            step.detail
        ));
        for next in &step.next {
            let mut lines = next.lines();
            out.push_str(&format!(
                "{:<26} next: {}\n",
                "",
                lines.next().unwrap_or_default()
            ));
            for line in lines {
                out.push_str(&format!("{:<32} {line}\n", ""));
            }
        }
    }
    for warning in &report.warnings {
        out.push_str(&format!("# warning: {warning}\n"));
    }
    if report.dry_run {
        if let (Some(toml), Some(path)) = (&report.toml, &report.path) {
            out.push_str(&format!("# would append to {path}:\n{toml}"));
        }
        return out;
    }
    out.push_str(if report.complete {
        "# complete: configured, connected and registered\n"
    } else {
        "# not complete: run the next step shown, then this command again\n"
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(steps: Vec<Step>) -> SetupReport {
        SetupReport {
            schema_version: 1,
            kind: "preset_setup",
            preset: "github",
            api: "github".into(),
            auth: None,
            path: Some("/c.toml".into()),
            dry_run: false,
            complete: false,
            steps,
            toml: None,
            warnings: vec![],
        }
    }

    #[test]
    fn the_text_names_each_step_its_state_and_what_comes_next() {
        let report = report(vec![
            Step::new("configure", StepState::Done, "added [api.github]"),
            Step::new("credential", StepState::NeedsAction, "no token is stored")
                .next(vec!["Log in:\nkurama login github".into()]),
        ]);
        assert_eq!(
            render_text(&report),
            "# kurama preset setup github ([api.github])\n\
             configure    done          added [api.github]\n\
             credential   needs_action  no token is stored\n\
             \x20                          next: Log in:\n\
             \x20                                kurama login github\n\
             # not complete: run the next step shown, then this command again\n"
        );
    }

    /// A dry run ends with the TOML it would append, and claims nothing
    /// about completion.
    #[test]
    fn a_dry_run_ends_with_the_toml_it_would_append() {
        let mut report = report(vec![Step::new(
            "configure",
            StepState::Planned,
            "would add [api.github]",
        )]);
        report.dry_run = true;
        report.toml = Some("[api.github]\n".into());
        report.warnings = vec!["scope".into()];
        let text = render_text(&report);
        assert!(
            text.ends_with("# warning: scope\n# would append to /c.toml:\n[api.github]\n"),
            "{text}"
        );
        assert!(!text.contains("complete"), "{text}");
    }
}
