//! CLI Effect Executor
//!
//! This module provides the execution engine for CLI effects.
//! It interprets pure Effect data structures and performs the actual I/O.
//!
//! ## Design
//!
//! The executor follows the "Effects as Data" pattern:
//! - Command handlers produce Effect types (pure planning)
//! - Executor interprets and executes effects (impure execution)
//! - Runtime provides dependencies via dependency injection
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    Pure Planning                             │
//! │  plan_profile_command(config) -> Vec<ProfileEffect>          │
//! └─────────────────────────────────────────────────────────────┘
//!                           │
//!                           ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    Effect Executor                           │
//! │  execute_profile_effects(effects, runtime) -> Result<Ctx>    │
//! │                                                              │
//! │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐          │
//! │  │ LoadProfiles│→ │SelectProfile│→ │ AssumeRole  │→ ...     │
//! │  └─────────────┘  └─────────────┘  └─────────────┘          │
//! │         │               │                │                   │
//! │         ▼               ▼                ▼                   │
//! │  Pure Functions    ProfileContext   Workflow Executor        │
//! │   (manager.rs)        (Pure)          (Runtime)              │
//! └─────────────────────────────────────────────────────────────┘
//! ```

use super::effects::{ProfileEffect, ProfileEffectContext, UnsetEffect};
use crate::adapters::aws::federation::FederationService;
use crate::adapters::browser::open_url;
use crate::adapters::env_script::output_shell_script;
use crate::adapters::profile::manager::load_profiles;
use crate::console::progress;
use crate::domain::functions::export::{
    credential_env_vars, generate_credential_json, generate_export_script, generate_unset_script,
};
use crate::domain::{Credentials, OutputFormat, Profile};
use crate::shell::runtime::Runtime;
use anyhow::{Context, Result};
use std::os::unix::process::CommandExt;
use std::process::Command;
use tracing::{debug, info};

/// Executor error type
#[derive(Debug, thiserror::Error)]
pub enum CliExecutorError {
    #[error("Profile not found: {0}")]
    ProfileNotFound(String),

    #[error("Profile not selected")]
    ProfileNotSelected,

    #[error("Credentials not available: {0}")]
    CredentialsNotAvailable(String),

    #[error(
        "the TUI needs a terminal on stdin and stdout with TERM other than dumb, in a run without KURAMA_AGENT"
    )]
    TerminalRequired,

    #[error("failed to run `{program}`")]
    ExecFailed {
        program: String,
        #[source]
        source: std::io::Error,
    },

    /// A verb (or flag) that does not apply to the source's kind.
    #[error("profile '{name}': kind {kind} does not support {verb}")]
    KindUnsupported {
        name: String,
        kind: &'static str,
        verb: &'static str,
    },
}

// =============================================================================
// Profile Effect Executor
// =============================================================================

/// Execute a sequence of profile effects using Runtime
///
/// Effects are executed in order, with results accumulated in the context.
/// All effects use Runtime for dependency injection.
///
/// # Arguments
/// * `effects` - Sequence of effects to execute
/// * `runtime` - Runtime with injected dependencies
///
/// # Returns
/// * `Ok(context)` - Accumulated context with profile and credentials
/// * `Err(error)` - Execution failed
pub async fn execute_profile_effects(
    effects: Vec<ProfileEffect>,
    runtime: &Runtime,
) -> Result<ProfileEffectContext> {
    let mut ctx = ProfileEffectContext::new();

    for effect in effects {
        execute_single_profile_effect(effect, &mut ctx, runtime).await?;
    }

    Ok(ctx)
}

/// Execute a single profile effect
async fn execute_single_profile_effect(
    effect: ProfileEffect,
    ctx: &mut ProfileEffectContext,
    runtime: &Runtime,
) -> Result<()> {
    match effect {
        ProfileEffect::Log { message } => info!("{message}"),

        ProfileEffect::PrintMessage { message } => {
            progress!("{message}");
        }

        ProfileEffect::LoadProfiles => {
            ctx.profiles = load_profiles().await?;
            debug!("Loaded {} profiles", ctx.profiles.len());
        }

        ProfileEffect::SelectProfile { profile_name } => {
            let profile = ctx
                .profiles
                .iter()
                .find(|p| p.name() == profile_name)
                .cloned()
                .ok_or_else(|| CliExecutorError::ProfileNotFound(profile_name.clone()))?;

            ctx.selected_profile = Some(profile);
            debug!("Selected profile: {}", profile_name);
        }

        ProfileEffect::AssumeRole { readonly } => {
            let profile = ctx
                .selected_profile
                .as_ref()
                .ok_or(CliExecutorError::ProfileNotSelected)?;

            let output = crate::shell::executor::assume_role_for_profile(
                runtime,
                profile.clone(),
                readonly,
                None,
            )
            .await?;

            ctx.credentials = Some(output.credentials);
            ctx.session_name = Some(output.session_name);
            ctx.readonly = readonly;
            debug!("AssumeRole succeeded for profile: {}", output.profile_name);
        }

        ProfileEffect::OutputCredentials { output_format } => {
            let profile = ctx.profile().ok_or(CliExecutorError::ProfileNotSelected)?;
            let credentials = ctx.credentials.as_ref().ok_or_else(|| {
                CliExecutorError::CredentialsNotAvailable("No credentials".into())
            })?;

            match output_format {
                OutputFormat::Shell => output_export_script(credentials, profile, ctx.readonly)?,
                OutputFormat::Json => {
                    println!("{}", generate_credential_json(credentials));
                }
            }
        }

        ProfileEffect::ExecCommand { command } => {
            let profile = ctx.profile().ok_or(CliExecutorError::ProfileNotSelected)?;
            let credentials = ctx
                .credentials
                .as_ref()
                .ok_or_else(|| CliExecutorError::CredentialsNotAvailable("exec".into()))?;
            let vars: Vec<(String, String)> =
                credential_env_vars(credentials, profile, ctx.readonly)
                    .into_iter()
                    .map(|(name, value)| (name.to_string(), value))
                    .collect();
            return Err(exec_command(&command, vars));
        }

        ProfileEffect::OpenAwsConsole => {
            let credentials = ctx.credentials.as_ref().ok_or_else(|| {
                CliExecutorError::CredentialsNotAvailable("Console access".into())
            })?;

            open_aws_console(credentials).await?;
        }

        ProfileEffect::PrintSuccess { profile_name } => {
            progress!("# Profile '{}' loaded successfully", profile_name);
        }
    }

    Ok(())
}

// =============================================================================
// Unset Effect Executor
// =============================================================================

/// Execute a sequence of unset effects
pub fn execute_unset_effects(effects: Vec<UnsetEffect>) -> Result<()> {
    for effect in effects {
        execute_single_unset_effect(effect)?;
    }
    Ok(())
}

/// Execute a single unset effect
fn execute_single_unset_effect(effect: UnsetEffect) -> Result<()> {
    match effect {
        UnsetEffect::Log { message } => info!("{message}"),

        UnsetEffect::OutputUnsetScript { token_vars } => {
            let script = generate_unset_script(&token_vars);
            output_shell_script(&script)?;
        }

        UnsetEffect::PrintSuccess => {
            progress!(
                "# Cleared the exported variables (AWS_*, KURAMA_AWS, token variables, KURAMA_AUTH)"
            );
        }
    }

    Ok(())
}

// =============================================================================
// Helper Functions (Impure - actual I/O operations)
// =============================================================================

/// The command for `kurama exec`: the credentials in its environment, and no
/// `KURAMA_ENV_SCRIPT`, so a nested `kurama env` cannot write into the file the
/// outer shell wrapper sources.
pub fn build_child_command(command: &[String], vars: Vec<(String, String)>) -> Command {
    let mut child = Command::new(&command[0]);
    child
        .args(&command[1..])
        .envs(vars)
        .env_remove("KURAMA_ENV_SCRIPT");
    child
}

/// Replace this process with `command`. exec(2) returns only when the
/// command could not be started, so what comes back is always that failure.
pub fn exec_command(command: &[String], vars: Vec<(String, String)>) -> anyhow::Error {
    // kurama is about to become the command and never sees its exit code.
    crate::shell::audit::finish(None, None);
    let source = build_child_command(command, vars).exec();
    CliExecutorError::ExecFailed {
        program: command[0].clone(),
        source,
    }
    .into()
}

/// Output shell export script
fn output_export_script(
    credentials: &Credentials,
    profile: &Profile,
    readonly: bool,
) -> Result<()> {
    let script = generate_export_script(credentials, profile, readonly);
    output_shell_script(&script)
}

/// Open AWS console in browser
async fn open_aws_console(credentials: &Credentials) -> Result<()> {
    progress!("# Opening AWS console...");

    let session_token = credentials
        .session_token()
        .ok_or_else(|| anyhow::anyhow!("Session token is required for console access"))?;

    let console_url = FederationService::new()
        .console_url(
            credentials.access_key_id(),
            credentials.secret_access_key(),
            session_token,
        )
        .await
        .context("Failed to get federation token")?;
    progress!("# Console URL: {}", console_url);

    open_url(&console_url).context("Failed to open browser")?;

    progress!("# AWS console opened in browser");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_profile_effect_context_default() {
        let ctx = ProfileEffectContext::default();
        assert!(ctx.profiles.is_empty());
        assert!(ctx.selected_profile.is_none());
        assert!(ctx.credentials.is_none());
    }

    #[test]
    fn test_execute_unset_effects_output_script() {
        let script = generate_unset_script(&[]);
        assert!(script.contains("unset AWS_ACCESS_KEY_ID"));
    }

    #[test]
    fn exec_child_gets_the_credentials_but_not_the_wrapper_script() {
        let command = vec!["aws".to_string(), "s3".to_string(), "ls".to_string()];
        let vars = vec![
            ("AWS_ACCESS_KEY_ID".to_string(), "ASIA".to_string()),
            ("KURAMA_AWS".to_string(), "dev".to_string()),
        ];
        let child = build_child_command(&command, vars);

        assert_eq!(child.get_program(), "aws");
        assert_eq!(child.get_args().collect::<Vec<_>>(), ["s3", "ls"]);
        let envs: Vec<_> = child.get_envs().collect();
        assert!(envs.contains(&("AWS_ACCESS_KEY_ID".as_ref(), Some("ASIA".as_ref()))));
        assert!(envs.contains(&("KURAMA_AWS".as_ref(), Some("dev".as_ref()))));
        assert!(envs.contains(&("KURAMA_ENV_SCRIPT".as_ref(), None)));
    }
}

#[cfg(test)]
mod session_cache_integration_tests {
    use super::*;
    use crate::ports::{
        mfa::MockMfaProvider,
        session_cache::MockSessionCache,
        sts::{MockStsOperations, StsCredentials},
    };
    use std::sync::Arc;

    #[tokio::test]
    async fn cli_passes_session_cache_settings_to_workflow() {
        let mut sts = MockStsOperations::new();
        sts.expect_get_session_token()
            .times(1)
            .withf(|r| r.duration_seconds == 900)
            .returning(|_| {
                Ok(StsCredentials {
                    access_key_id: "session-key".into(),
                    secret_access_key: "secret".into(),
                    session_token: "token".into(),
                    expiration: Some(chrono::Utc::now() + chrono::Duration::minutes(15)),
                })
            });
        sts.expect_assume_role()
            .times(1)
            .withf(|r| r.source_credentials.is_some())
            .returning(|_| {
                Ok(StsCredentials {
                    access_key_id: "role-key".into(),
                    secret_access_key: "secret".into(),
                    session_token: "token".into(),
                    expiration: None,
                })
            });
        let mut mfa = MockMfaProvider::new();
        mfa.expect_get_token()
            .times(1)
            .returning(|_| Ok(Some("123456".into())));
        let mut cache = MockSessionCache::new();
        cache.expect_load().times(1).returning(|_| Ok(None));
        cache.expect_store().times(1).returning(|_, _| Ok(()));
        let mut rt = Runtime::test(sts, mfa);
        Arc::make_mut(&mut rt.config).aws.session_cache.duration = 900;
        rt.session_cache = Arc::new(cache);
        let mut ctx = ProfileEffectContext::new();
        ctx.selected_profile = Some(
            Profile::new("test")
                .with_role_arn_raw("arn:aws:iam::123456789012:role/TestRole")
                .with_mfa_serial_raw("arn:aws:iam::123456789012:mfa/user"),
        );
        execute_single_profile_effect(ProfileEffect::AssumeRole { readonly: false }, &mut ctx, &rt)
            .await
            .unwrap();
        assert_eq!(ctx.credentials.unwrap().access_key_id(), "role-key");
    }
}
