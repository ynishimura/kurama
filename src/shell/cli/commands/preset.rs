//! `kurama preset [--json]` and the parsing of `preset setup`: the preset catalog, read without the configuration, and one preset expanded against config.toml and checked with it whole, for `preset_setup.rs` to append and verify.
//!
//! The list reads no configuration, so it works under a file that does not
//! load. `plan_against_file` reads config.toml through the config writer's
//! `ConfigFile` -- to reuse a compatible `[auth.*]` and to name the vault --
//! and `check_plan` runs the fragment through the same append and validate
//! a save would; `preset setup` saves what passed, or on `--dry-run` only
//! reports it.

use anyhow::Result;
use clap::{Arg, ArgAction, ArgMatches, Command, ValueHint};
use clap_complete::engine::{ArgValueCandidates, CompletionCandidate};
use serde_json::json;

use super::preset_setup::SetupMode;
use crate::adapters::config::Config;
use crate::adapters::config::input::InputError;
use crate::adapters::config::writer::{Appended, ConfigFile, Validated};
use crate::adapters::profile::load_profiles;
use crate::domain::functions::preset_render::{
    AuthAction, Existing, PresetError, PresetPlan, PresetRequest, input_keys, plan_preset,
};
use crate::domain::functions::spec_output::render_table;
use crate::domain::types::preset::{PRESETS, Preset, find_preset};

/// Sections the checks of config.toml refused, blamed on the `--set` keys
/// their URLs are built from: the file itself already loaded.
#[derive(Debug, thiserror::Error)]
#[error("the value of --set {} is refused", keys.join(", "))]
pub struct PresetInputRejected {
    keys: Vec<String>,
    #[source]
    source: InputError,
}

/// One `kurama preset` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresetCommand {
    List {
        json: bool,
    },
    Setup {
        id: String,
        request: PresetRequest,
        mode: SetupMode,
        json: bool,
    },
}

pub fn command() -> Command {
    Command::new("preset")
        .about("List the bundled provider presets, or set one up from nothing to a first answer")
        .long_about(
            "List the bundled provider presets (GitHub, Google, Linear, ...), or set one up:\n\
             `preset setup` appends its [auth.*] / [api.*] sections to config.toml, checks\n\
             them, reports whether the credential can be used, writes kurama's Agent Skill\n\
             for Claude Code and sends the preset's example as one GET. The list reads no\n\
             configuration.",
        )
        .arg(json_arg("Print the catalog as JSON"))
        .subcommand(
            Command::new("setup")
                .about("Take a preset from nothing to a first answer: add, check, credential, Claude Code Skill, first read")
                .long_about(
                    "Take a preset from nothing to a first answer, one step at a time: append its\n\
                     sections (reusing a compatible [auth.*]; an [api.*] already there is kept),\n\
                     check config.toml, report whether the credential can be used, write kurama's\n\
                     Agent Skill for Claude Code, and send the preset's example as one GET. Each\n\
                     step says whether it is done and what to run next; run it again after a stop\n\
                     and it resumes. Secret inputs take a reference (op://, aws-secrets://, aws-ssm://).",
                )
                .args(request_args())
                .arg(
                    Arg::new("dry-run")
                        .long("dry-run")
                        .action(ArgAction::SetTrue)
                        .conflicts_with("offline")
                        .help("Plan and check the sections and print their TOML; write, read and send nothing"),
                )
                .arg(
                    Arg::new("offline")
                        .long("offline")
                        .action(ArgAction::SetTrue)
                        .help("Every step but the first read: no secret is read and nothing is sent"),
                )
                .arg(json_arg("Print one JSON document")),
        )
}

/// `ID`, `--as`, `--set` and `--auth-as`.
fn request_args() -> [Arg; 4] {
    [
        Arg::new("id")
            .value_name("ID")
            .required(true)
            .help("The preset; `kurama preset` lists them")
            .add(ArgValueCandidates::new(preset_ids)),
        Arg::new("as")
            .long("as")
            .value_name("NAME")
            .value_parser(parse_name)
            .value_hint(ValueHint::Other)
            .help("Name the [api.*] section NAME; the [auth.*] keeps the preset's name"),
        Arg::new("set")
            .long("set")
            .value_name("KEY=VALUE")
            .value_parser(parse_input)
            .value_hint(ValueHint::Other)
            .action(ArgAction::Append)
            .help("An input of the preset, such as client_id=... or secret=op://...; repeatable"),
        Arg::new("auth-as")
            .long("auth-as")
            .value_name("NAME")
            .value_parser(parse_name)
            .value_hint(ValueHint::Other)
            .help("Name the [auth.*] section NAME and point the [api.*] at it; an [auth.NAME] the file has is reused when it is compatible"),
    ]
}

fn json_arg(help: &'static str) -> Arg {
    Arg::new("json")
        .long("json")
        .action(ArgAction::SetTrue)
        .help(help)
}

/// The preset ids, for zsh: read from the catalog, never the configuration.
fn preset_ids() -> Vec<CompletionCandidate> {
    PRESETS
        .iter()
        .map(|preset| CompletionCandidate::new(preset.id).help(Some(preset.title.into())))
        .collect()
}

/// A section name kurama can write without quoting: letters, digits, `-`, `_`.
fn parse_name(value: &str) -> Result<String, String> {
    if !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Ok(value.to_owned())
    } else {
        Err("a name is letters, digits, - and _".to_owned())
    }
}

fn parse_input(value: &str) -> Result<(String, String), String> {
    match value.split_once('=') {
        Some((key, value)) if !key.is_empty() => Ok((key.to_owned(), value.to_owned())),
        _ => Err("expected KEY=VALUE".to_owned()),
    }
}

impl PresetCommand {
    pub fn parse(preset: &ArgMatches) -> Self {
        match preset.subcommand() {
            Some(("setup", setup)) => Self::Setup {
                id: preset_id(setup),
                request: request_of(setup),
                mode: if setup.get_flag("dry-run") {
                    SetupMode::DryRun
                } else if setup.get_flag("offline") {
                    SetupMode::Offline
                } else {
                    SetupMode::Full
                },
                json: setup.get_flag("json"),
            },
            _ => Self::List {
                json: preset.get_flag("json"),
            },
        }
    }

    /// Whether stdout carries one JSON document.
    pub fn json(&self) -> bool {
        match self {
            Self::List { json } | Self::Setup { json, .. } => *json,
        }
    }

    pub async fn run(self) -> Result<()> {
        match self {
            Self::List { json } => {
                print_list(json);
                Ok(())
            }
            Self::Setup {
                id,
                request,
                mode,
                json,
            } => super::preset_setup::run(&id, &request, mode, json).await,
        }
    }

    /// The `[api.*]` `setup` sends its first read to, for the audit log;
    /// `None` when nothing is sent.
    pub fn sent_to(&self) -> Option<&str> {
        match self {
            Self::Setup {
                id, request, mode, ..
            } if *mode == SetupMode::Full => request
                .api_name
                .as_deref()
                .or_else(|| find_preset(id).map(|preset| preset.api.name)),
            Self::Setup { .. } | Self::List { .. } => None,
        }
    }
}

fn preset_id(matches: &ArgMatches) -> String {
    matches
        .get_one::<String>("id")
        .expect("clap requires ID")
        .clone()
}

fn request_of(matches: &ArgMatches) -> PresetRequest {
    PresetRequest {
        api_name: matches.get_one::<String>("as").cloned(),
        auth_name: matches.get_one::<String>("auth-as").cloned(),
        inputs: matches
            .get_many::<(String, String)>("set")
            .into_iter()
            .flatten()
            .cloned()
            .collect(),
    }
}

fn print_list(json: bool) {
    if json {
        let presets: Vec<_> = PRESETS.iter().map(preset_json).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&presets).expect("the catalog serializes")
        );
        return;
    }
    let rows: Vec<[String; 5]> = PRESETS
        .iter()
        .map(|preset| {
            [
                preset.id.to_owned(),
                preset.title.to_owned(),
                preset.auth.kind.summary(),
                input_keys(preset).join(", "),
                preset.docs_url.to_owned(),
            ]
        })
        .collect();
    print!(
        "{}",
        render_table(&["ID", "TITLE", "AUTH", "INPUTS", "SETUP"], &rows)
    );
}

fn preset_json(preset: &Preset) -> serde_json::Value {
    json!({
        "id": preset.id,
        "title": preset.title,
        "auth": {"name": preset.auth.name, "kind": preset.auth.kind.summary()},
        "api": {"name": preset.api.name, "base_url": preset.api.base_url},
        "inputs": preset.inputs.iter().map(|input| json!({
            "key": input.key,
            "help": input.help,
            "secret": input.secret_field.is_some(),
        })).collect::<Vec<_>>(),
        "setup": preset.docs_url,
    })
}

/// `id` expanded against config.toml: the file as read and the plan. What
/// `preset show` prints and `preset add` saves both start here.
pub fn plan_against_file(id: &str, request: &PresetRequest) -> Result<(ConfigFile, PresetPlan)> {
    let preset = find_preset(id).ok_or_else(|| PresetError::NotFound(id.to_owned()))?;
    let file = ConfigFile::open()?;
    let config = Config::parse(file.content())?;
    let auth = config.auth_source(request.auth_name(preset))?;
    let apis: Vec<String> = config.api.keys().cloned().collect();
    let path = file.path().display().to_string();
    let existing = Existing {
        apis: &apis,
        auth: auth.as_ref(),
        vault: config.onepassword.vault.as_deref(),
        config_path: &path,
    };
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let plan = plan_preset(preset, request, &existing, &today)?;
    Ok((file, plan))
}

/// The plan's TOML appended to the file and validated whole, as a save
/// would take it, its warnings after the plan's own; an input the plan still
/// needs is the error. An auth the
/// plan adds under a name an AWS profile has is refused first, with a hint
/// that does not ask to fix an input the person did not write.
pub async fn check_plan(file: &ConfigFile, plan: &PresetPlan) -> Result<(Appended, Validated)> {
    let fragment = plan.fragment.clone()?;
    if plan.auth_action == AuthAction::Add
        && load_profiles()
            .await?
            .iter()
            .any(|profile| profile.name() == plan.auth)
    {
        return Err(PresetError::AuthNamedLikeAwsProfile(plan.auth.clone()).into());
    }
    let refused = |error: anyhow::Error| match error.downcast::<InputError>() {
        Ok(source) => PresetInputRejected {
            keys: plan.url_inputs.clone(),
            source,
        }
        .into(),
        Err(error) => error,
    };
    let appended = file.append(&fragment).map_err(refused)?;
    let mut validated = file.validate(&appended).await.map_err(refused)?;
    validated
        .warnings
        .splice(0..0, plan.warnings.iter().cloned());
    Ok((appended, validated))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::AuthSource;

    fn parse(args: &[&str]) -> PresetCommand {
        let matches = crate::build_command()
            .try_get_matches_from(std::iter::once("kurama").chain(args.iter().copied()))
            .unwrap();
        let (_, preset) = matches.subcommand().unwrap();
        PresetCommand::parse(preset)
    }

    /// Every input a preset declares, with a value of the right shape.
    fn full_request(preset: &Preset) -> PresetRequest {
        PresetRequest {
            api_name: None,
            auth_name: None,
            inputs: preset
                .inputs
                .iter()
                .map(|input| {
                    let value = match input.secret_field {
                        Some(field) => format!("op://Agent/{}/{field}", preset.auth.item()),
                        None => format!("{}-value", input.key),
                    };
                    (input.key.to_owned(), value)
                })
                .collect(),
        }
    }

    fn plan_for(preset: &'static Preset, request: &PresetRequest, config: &Config) -> PresetPlan {
        let apis: Vec<String> = config.api.keys().cloned().collect();
        let auth: Option<AuthSource> = config.auth_source(preset.auth.name).unwrap();
        plan_preset(
            preset,
            request,
            &Existing {
                apis: &apis,
                auth: auth.as_ref(),
                vault: Some("Agent"),
                config_path: "/c.toml",
            },
            "2026-09-26",
        )
        .unwrap()
    }

    #[test]
    fn each_form_parses_to_what_it_was_asked() {
        assert_eq!(parse(&["preset"]), PresetCommand::List { json: false });
        assert_eq!(
            parse(&["preset", "--json"]),
            PresetCommand::List { json: true }
        );
        assert_eq!(
            parse(&[
                "preset",
                "setup",
                "google-sheets",
                "--as",
                "sheets-work",
                "--set",
                "client_id=a=b",
                "--set",
                "client_secret=op://V/i/f",
                "--auth-as",
                "google-work",
                "--dry-run",
                "--json",
            ]),
            PresetCommand::Setup {
                id: "google-sheets".into(),
                request: PresetRequest {
                    api_name: Some("sheets-work".into()),
                    auth_name: Some("google-work".into()),
                    inputs: [
                        ("client_id".to_owned(), "a=b".to_owned()),
                        ("client_secret".to_owned(), "op://V/i/f".to_owned()),
                    ]
                    .into(),
                },
                mode: SetupMode::DryRun,
                json: true,
            }
        );
        let mode = |args: &[&str]| match parse(args) {
            PresetCommand::Setup { mode, .. } => mode,
            other => panic!("{other:?}"),
        };
        assert_eq!(mode(&["preset", "setup", "linear"]), SetupMode::Full);
        assert_eq!(
            mode(&["preset", "setup", "linear", "--offline"]),
            SetupMode::Offline
        );
        let refused = |args: &[&str]| {
            crate::build_command()
                .try_get_matches_from(std::iter::once("kurama").chain(args.iter().copied()))
                .is_err()
        };
        assert!(refused(&["preset", "setup", "github", "--set", "novalue"]));
        assert!(refused(&["preset", "setup", "github", "--as", "a.b"]));
        assert!(refused(&[
            "preset",
            "setup",
            "github",
            "--dry-run",
            "--offline"
        ]));
        assert!(refused(&["preset", "show", "github"]));
        assert!(refused(&["preset", "add", "github"]));
    }

    /// Every preset, added to an empty file, is a configuration kurama loads.
    #[test]
    fn every_preset_parses_standalone() {
        for preset in PRESETS {
            let plan = plan_for(preset, &full_request(preset), &Config::default());
            let text = plan.fragment.unwrap();
            let config = Config::parse(&text)
                .unwrap_or_else(|error| panic!("{}: {error:#}\n{text}", preset.id));
            assert!(config.auth_source(preset.auth.name).unwrap().is_some());
            let api = config.api_profile(preset.api.name).unwrap().unwrap();
            assert_eq!(api.auth.as_deref(), Some(preset.auth.name));
        }
    }

    /// A preset that shares an auth, added after another, reuses it, and the
    /// file with both is still a configuration kurama loads.
    #[test]
    fn a_reused_auth_is_checked_with_the_file_it_joins() {
        for first in PRESETS {
            let text = plan_for(first, &full_request(first), &Config::default())
                .fragment
                .unwrap();
            let config = Config::parse(&text).unwrap();
            for second in PRESETS.iter().filter(|second| {
                second.auth.name == first.auth.name && second.api.name != first.api.name
            }) {
                let plan = plan_for(second, &PresetRequest::default(), &config);
                assert_eq!(plan.auth_action, AuthAction::Reuse, "{}", second.id);
                let joined = format!("{text}\n{}", plan.fragment.unwrap());
                Config::parse(&joined)
                    .unwrap_or_else(|error| panic!("{} after {}: {error:#}", second.id, first.id));
            }
        }
    }

    /// Each `kurama ...` line a setup step prints is a command this binary
    /// accepts (the same check `commands::agent` makes of the guide), with
    /// placeholders filled in, on each side of a pipe.
    #[test]
    fn setup_commands_parse_as_real_subcommands() {
        let (mut seen, mut scope_commands) = (0, 0);
        let mut plans = Vec::new();
        for preset in PRESETS {
            for request in [PresetRequest::default(), full_request(preset)] {
                plans.push((preset, plan_for(preset, &request, &Config::default())));
            }
            // An auth another preset added, reused with the scopes it lacks:
            // the steps and the warning name `kurama config set`.
            let first = plan_for(preset, &full_request(preset), &Config::default());
            let config = Config::parse(&first.fragment.unwrap()).unwrap();
            for second in PRESETS.iter().filter(|second| {
                second.auth.name == preset.auth.name && second.api.name != preset.api.name
            }) {
                plans.push((second, plan_for(second, &PresetRequest::default(), &config)));
            }
        }
        for (preset, plan) in plans {
            let warnings = plan.warnings.iter().flat_map(|warning| {
                warning
                    .split('`')
                    .filter(|part| part.starts_with("kurama "))
            });
            for line in plan
                .setup
                .iter()
                .flat_map(|step| step.lines())
                .chain(warnings)
            {
                // A command is a line of its own; prose may say "kurama".
                if !line.starts_with("kurama ") {
                    continue;
                }
                for command in shell_words::split(line)
                    .unwrap_or_else(|error| panic!("{}: {line}: {error}", preset.id))
                    .split(|word| word == "|")
                {
                    let command: Vec<String> = command
                        .iter()
                        .map(|word| {
                            if word.starts_with('<') {
                                "x".to_owned()
                            } else {
                                word.replace(['<', '>'], "")
                            }
                        })
                        .collect();
                    assert_eq!(command[0], "kurama", "{}: {line}", preset.id);
                    if let Err(error) = crate::build_command().try_get_matches_from(&command) {
                        panic!("{}: {line}: {error}", preset.id);
                    }
                    seen += 1;
                    scope_commands += usize::from(line.starts_with("kurama config set "));
                }
            }
        }
        assert!(seen >= PRESETS.len() * 2, "{seen}");
        assert!(scope_commands > 0, "no scope shortfall was planned");
    }
}
