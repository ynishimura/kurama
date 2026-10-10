//! `kurama env`, `kurama exec` and `kurama console`: turn a credential
//! source into credentials, then export them, run a command with them, or
//! open the console.
//!
//! ## AWS profiles (Effects as Data)
//!
//! ```text
//! plan_profile_command(config) -> Vec<ProfileEffect>          (pure)
//!   [LoadProfiles, SelectProfile, AssumeRole,
//!    OutputCredentials + PrintSuccess | ExecCommand | OpenAwsConsole]
//! execute_profile_effects(effects, runtime)                   (I/O)
//! ```
//!
//! ## `[auth.*]` sources
//!
//! `env` exports the credential into the source's `env_var` -- or, for a
//! `kind = "secrets"` source, each secret into its own variable -- with the
//! source name in `KURAMA_AUTH` and the variable names in `KURAMA_AUTH_VAR`;
//! `exec` puts them in the command's environment, after every value was
//! read. `console`, `--readonly` and `--json` do not apply to any kind:
//! `token --json` is the one way a source's credential reaches stdout as a
//! document, and a `secrets` source's values are for `exec`.

use super::source::{ensure_source_credential, resolve_source_secrets};
use super::unset::managed_token_vars;
use crate::adapters::config::Config;
use crate::adapters::env_script::output_shell_script;
use crate::console::progress;
use crate::domain::OutputFormat;
use crate::domain::Profile;
use crate::domain::functions::export::{
    auth_env_vars, generate_auth_export_script, token_env_vars,
};
use crate::domain::types::AuthSource;
use crate::domain::types::ProfileAuth;
use crate::shell::agent_policy::{AgentPolicyDenied, exec_is_read_only};
use crate::shell::api_runtime::ApiRuntimeOptions;
use crate::shell::cli::effects::ProfileEffect;
use crate::shell::cli::executor::{CliExecutorError, exec_command, execute_profile_effects};
use crate::shell::runtime::Runtime;
use anyhow::Result;

/// What to do with the credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileAction {
    /// `env`: the export script or JSON
    Export(OutputFormat),
    /// `exec`: replace kurama with the command, credentials in its environment
    Exec(Vec<String>),
    /// `console`: open the AWS console in the browser
    OpenConsole,
}

impl ProfileAction {
    /// The subcommand name, for the log: an `exec` command line can carry a
    /// secret, so the log names the action and never its arguments.
    fn name(&self) -> &'static str {
        match self {
            Self::Export(_) => "env",
            Self::Exec(_) => "exec",
            Self::OpenConsole => "console",
        }
    }
}

/// Inputs of `env`, `exec` and `console`.
#[derive(Debug, Clone)]
pub struct ProfileCommandConfig {
    /// Name of the credential source
    pub profile_name: String,
    /// Whether to attach the readonly policy (AWS only)
    pub readonly: bool,
    /// `exec --confirm`: a person agreed, so an agent's run keeps the role's
    /// own permissions instead of the `[agent] exec_readonly` ReadOnlyAccess.
    pub confirm: bool,
    /// Say each secret store read on stderr (`[auth.*]` only)
    pub verbose: bool,
    pub action: ProfileAction,
}

/// Plan the effects of `env` / `exec` / `console` on an AWS profile (pure function).
pub fn plan_profile_command(config: &ProfileCommandConfig) -> Vec<ProfileEffect> {
    let mut effects = vec![
        ProfileEffect::info(format!(
            "Starting profile command: profile={}, readonly={}, action={}",
            config.profile_name,
            config.readonly,
            config.action.name()
        )),
        ProfileEffect::print_stderr(format!("# Loading profile: {}", config.profile_name)),
    ];

    if config.readonly {
        effects.push(ProfileEffect::print_stderr("# Running in readonly mode"));
    }

    effects.extend([
        ProfileEffect::LoadProfiles,
        ProfileEffect::SelectProfile {
            profile_name: config.profile_name.clone(),
        },
        ProfileEffect::AssumeRole {
            readonly: config.readonly,
        },
    ]);

    match &config.action {
        ProfileAction::Export(output_format) => effects.extend([
            ProfileEffect::OutputCredentials {
                output_format: *output_format,
            },
            ProfileEffect::PrintSuccess {
                profile_name: config.profile_name.clone(),
            },
        ]),
        ProfileAction::Exec(command) => effects.push(ProfileEffect::ExecCommand {
            command: command.clone(),
        }),
        ProfileAction::OpenConsole => effects.push(ProfileEffect::OpenAwsConsole),
    }

    effects
}

/// `exec` of an agent's run attaches ReadOnlyAccess unless `[agent]
/// exec_readonly = false` or `--confirm` says a person agreed. A profile
/// with no `role_arn` has no role session to attach it to, so that `exec` is
/// refused rather than handing the agent the IAM user's own permissions.
pub fn apply_agent_policy(
    mut config: ProfileCommandConfig,
    app_config: &Config,
    agent_run: bool,
    profile: &Profile,
) -> Result<ProfileCommandConfig, AgentPolicyDenied> {
    if matches!(config.action, ProfileAction::Exec(_))
        && exec_is_read_only(app_config, agent_run, config.confirm)
    {
        if !profile.can_assume_role() {
            return Err(AgentPolicyDenied(format!(
                "exec on '{}': the profile has no role_arn, so ReadOnlyAccess cannot narrow the IAM user's own permissions",
                profile.name()
            )));
        }
        config.readonly = true;
    }
    Ok(config)
}

/// What a profile with no `role_arn` (an IAM user) cannot do, refused
/// before any credential is read: the console sign-in takes a role session,
/// which an IAM user's keys and MFA session are not.
pub fn check_iam_user_profile(
    config: &ProfileCommandConfig,
    profile: &Profile,
) -> Result<(), CliExecutorError> {
    if !profile.can_assume_role() && config.action == ProfileAction::OpenConsole {
        return Err(CliExecutorError::KindUnsupported {
            name: profile.name().to_string(),
            kind: ProfileAuth::IamUser.as_str(),
            verb: "console",
        });
    }
    Ok(())
}

/// Plan and execute `env` / `exec` / `console` on an AWS profile.
pub async fn handle_aws_profile_command(
    config: ProfileCommandConfig,
    runtime: &Runtime,
) -> Result<()> {
    let effects = plan_profile_command(&config);
    execute_profile_effects(effects, runtime).await?;
    Ok(())
}

/// `env` / `exec` on an `[auth.*]` source: get its credential, then export
/// it or run the command with it.
pub async fn handle_auth_profile_command(
    config: ProfileCommandConfig,
    source: AuthSource,
    app_config: Config,
) -> Result<()> {
    let unsupported = |verb: &'static str| CliExecutorError::KindUnsupported {
        name: config.profile_name.clone(),
        kind: source.kind().as_str(),
        verb,
    };
    if config.readonly {
        return Err(unsupported("--readonly").into());
    }
    match config.action {
        ProfileAction::OpenConsole => return Err(unsupported("console").into()),
        ProfileAction::Export(OutputFormat::Json) => return Err(unsupported("env --json").into()),
        ProfileAction::Export(OutputFormat::Shell) | ProfileAction::Exec(_) => {}
    }
    let managed = managed_token_vars(&app_config)?;
    let options = ApiRuntimeOptions {
        report_secret_reads: config.verbose,
        ..ApiRuntimeOptions::default()
    };
    // What was exported, for the progress line, and the variables.
    let (what, vars) = match source.request_auth() {
        Ok(source) => {
            let credential = ensure_source_credential(app_config, &source, options).await?;
            ("Token for", token_env_vars(&credential, &source))
        }
        Err(secrets) => {
            let values = resolve_source_secrets(app_config, &secrets, options)
                .await?
                .into_iter()
                // Exposed here: these are the variables the export script
                // and the command's environment carry.
                .map(|(var, value)| (var, value.expose().to_owned()))
                .collect();
            ("Secrets of", auth_env_vars(&secrets.name, values))
        }
    };

    match config.action {
        ProfileAction::Export(OutputFormat::Shell) => {
            let exported = vars.last().map(|(_, names)| names.clone());
            output_shell_script(&generate_auth_export_script(vars, &managed))?;
            progress!(
                "# {what} '{}' exported into {}",
                config.profile_name,
                exported.unwrap_or_default()
            );
        }
        ProfileAction::Exec(command) => return Err(exec_command(&command, vars)),
        ProfileAction::Export(OutputFormat::Json) | ProfileAction::OpenConsole => {
            unreachable!("rejected above")
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(action: ProfileAction, readonly: bool) -> ProfileCommandConfig {
        ProfileCommandConfig {
            profile_name: "dev".to_string(),
            readonly,
            confirm: false,
            verbose: false,
            action,
        }
    }

    fn role() -> Profile {
        Profile::new("dev").with_role_arn_raw("arn:aws:iam::123456789012:role/Dev")
    }

    /// An agent's `exec` is read-only unless a person confirmed it or the
    /// configuration says otherwise; `env` and a person's run are untouched.
    #[test]
    fn an_agents_exec_attaches_read_only_access_unless_confirmed() {
        let exec = || ProfileAction::Exec(vec!["true".into()]);
        let defaults = Config::default();
        let applied = |config: ProfileCommandConfig, app: &Config, agent: bool| {
            apply_agent_policy(config, app, agent, &role())
                .unwrap()
                .readonly
        };
        assert!(applied(config(exec(), false), &defaults, true));
        assert!(!applied(config(exec(), false), &defaults, false));
        let confirmed = ProfileCommandConfig {
            confirm: true,
            ..config(exec(), false)
        };
        assert!(!applied(confirmed, &defaults, true));
        let env = config(ProfileAction::Export(OutputFormat::Shell), false);
        assert!(!applied(env, &defaults, true));
        let off = Config::parse("[agent]\nexec_readonly = false\n").unwrap();
        assert!(!applied(config(exec(), false), &off, true));
    }

    /// On an IAM user profile the read-only `exec` an agent would get cannot
    /// be had, so it is refused; a confirmed run, a person's and `env` pass.
    #[test]
    fn an_agents_exec_on_an_iam_user_profile_needs_a_confirmation() {
        let exec = || ProfileAction::Exec(vec!["true".into()]);
        let user = Profile::new("uploader");
        let defaults = Config::default();
        let error = apply_agent_policy(config(exec(), false), &defaults, true, &user).unwrap_err();
        assert!(error.0.contains("'uploader'"), "{error}");
        assert!(error.0.contains("no role_arn"), "{error}");
        let confirmed = ProfileCommandConfig {
            confirm: true,
            ..config(exec(), false)
        };
        let passed = apply_agent_policy(confirmed, &defaults, true, &user).unwrap();
        assert!(!passed.readonly);
        assert!(apply_agent_policy(config(exec(), false), &defaults, false, &user).is_ok());
        let env = config(ProfileAction::Export(OutputFormat::Shell), false);
        assert!(apply_agent_policy(env, &defaults, true, &user).is_ok());
    }

    #[test]
    fn the_console_needs_a_role() {
        let console = config(ProfileAction::OpenConsole, false);
        assert!(matches!(
            check_iam_user_profile(&console, &Profile::new("uploader")),
            Err(CliExecutorError::KindUnsupported {
                kind: "iam_user",
                verb: "console",
                ..
            })
        ));
        assert!(check_iam_user_profile(&console, &role()).is_ok());
        let exec = config(ProfileAction::Exec(vec!["true".into()]), false);
        assert!(check_iam_user_profile(&exec, &Profile::new("uploader")).is_ok());
    }

    fn position(effects: &[ProfileEffect], wanted: fn(&ProfileEffect) -> bool) -> usize {
        effects.iter().position(wanted).expect("effect is planned")
    }

    #[test]
    fn the_log_names_the_action_and_not_the_exec_arguments() {
        let effects = plan_profile_command(&config(
            ProfileAction::Exec(vec!["curl".into(), "--token=secret".into()]),
            false,
        ));

        let ProfileEffect::Log { message } = &effects[0] else {
            panic!("the first effect is the log line");
        };
        assert_eq!(
            message,
            "Starting profile command: profile=dev, readonly=false, action=exec"
        );
    }

    #[test]
    fn env_assumes_the_role_then_outputs_credentials() {
        let effects =
            plan_profile_command(&config(ProfileAction::Export(OutputFormat::Json), false));

        let load = position(&effects, |e| matches!(e, ProfileEffect::LoadProfiles));
        let select = position(
            &effects,
            |e| matches!(e, ProfileEffect::SelectProfile { profile_name } if profile_name == "dev"),
        );
        let assume = position(&effects, |e| {
            matches!(e, ProfileEffect::AssumeRole { readonly: false })
        });
        let output = position(&effects, |e| {
            matches!(
                e,
                ProfileEffect::OutputCredentials {
                    output_format: OutputFormat::Json
                }
            )
        });
        let success = position(&effects, |e| {
            matches!(e, ProfileEffect::PrintSuccess { .. })
        });
        assert!(load < select && select < assume && assume < output);
        assert!(output < success);
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, ProfileEffect::OpenAwsConsole))
        );
    }

    #[test]
    fn console_opens_the_console_without_exporting() {
        let effects = plan_profile_command(&config(ProfileAction::OpenConsole, false));

        let assume = position(&effects, |e| matches!(e, ProfileEffect::AssumeRole { .. }));
        let console = position(&effects, |e| matches!(e, ProfileEffect::OpenAwsConsole));
        assert!(assume < console);
        assert!(!effects.iter().any(|e| matches!(
            e,
            ProfileEffect::OutputCredentials { .. } | ProfileEffect::PrintSuccess { .. }
        )));
    }

    #[test]
    fn exec_runs_the_command_last() {
        let command = vec!["aws".to_string(), "s3".to_string(), "ls".to_string()];
        let effects = plan_profile_command(&config(ProfileAction::Exec(command.clone()), false));

        let assume = position(&effects, |e| matches!(e, ProfileEffect::AssumeRole { .. }));
        assert!(assume < effects.len() - 1);
        assert!(matches!(
            effects.last(),
            Some(ProfileEffect::ExecCommand { command: planned }) if *planned == command
        ));
        assert!(!effects.iter().any(|e| matches!(
            e,
            ProfileEffect::OutputCredentials { .. } | ProfileEffect::PrintSuccess { .. }
        )));
    }

    #[test]
    fn readonly_is_announced_and_passed_to_assume_role() {
        let effects = plan_profile_command(&config(ProfileAction::OpenConsole, true));

        assert!(
            effects
                .iter()
                .any(|e| matches!(e, ProfileEffect::AssumeRole { readonly: true }))
        );
        assert!(effects.iter().any(|e| {
            matches!(e, ProfileEffect::PrintMessage { message } if message.contains("readonly"))
        }));
    }

    #[tokio::test]
    async fn auth_sources_reject_console_and_readonly_before_any_token_work() {
        use crate::adapters::config::AuthToml;
        let client = toml::from_str::<AuthToml>(
            "kind = \"oauth\"\ngrant_type = \"client_credentials\"\ntoken_url = \"https://as/token\"\nclient_id = \"id\"\n",
        )
        .unwrap()
        .typed("svc")
        .unwrap();
        for (action, readonly, verb) in [
            (ProfileAction::OpenConsole, false, "console"),
            (
                ProfileAction::Export(OutputFormat::Json),
                false,
                "env --json",
            ),
            (
                ProfileAction::Export(OutputFormat::Shell),
                true,
                "--readonly",
            ),
        ] {
            let error = handle_auth_profile_command(
                ProfileCommandConfig {
                    profile_name: "svc".into(),
                    readonly,
                    confirm: false,
                    verbose: false,
                    action,
                },
                client.clone(),
                Config::default(),
            )
            .await
            .unwrap_err();
            assert!(matches!(
                error.downcast_ref::<CliExecutorError>(),
                Some(CliExecutorError::KindUnsupported { verb: v, kind: "oauth", .. }) if *v == verb
            ));
        }
    }
}
