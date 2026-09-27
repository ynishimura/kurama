//! `kurama login <PROFILE> [--force] [--no-browser]`: make the credential
//! source ready for unattended use.
//!
//! An AWS profile gets an MFA session for its MFA device, stored in the
//! session cache; a profile without `mfa_serial` has nothing to log in to
//! and succeeds. Without the session cache the session would be thrown
//! away, so that configuration is an error instead of a silent no-op.
//!
//! An `[auth.*]` source of kind `oauth` runs its grant and stores the
//! token; a usable stored token makes it a no-op. A token that cannot be
//! stored is a failure here even though it is usable, because the next
//! unattended run would find nothing and ask for a person again.
//!
//! A source of kind `token` has nothing to authorize and nothing to store:
//! its credential is read from a secret store on every use, so `login`
//! succeeds and says so, the way a profile without `mfa_serial` does.

use anyhow::{Context, Result};
use chrono::Utc;

use super::source::{CredentialSource, ensure_source_token, resolve_source};
use crate::adapters::config::Config;
use crate::adapters::error::CoreError;
use crate::console::progress;
use crate::domain::Profile;
use crate::domain::functions::profile_status::describe_remaining;
use crate::domain::types::{AuthSource, OAuthClientConfig, TokenSourceConfig};
use crate::ports::TokenStoreError;
use crate::shell::api_runtime::ApiRuntimeOptions;
use crate::shell::mfa_login_executor::execute_mfa_login;
use crate::shell::runtime::Runtime;
use crate::workflows::mfa_login::LoginInput;
use crate::workflows::oauth_token::TokenMode;

pub async fn handle_login_command(
    profile_name: &str,
    force: bool,
    no_browser: bool,
    verbose: bool,
    config: Config,
) -> Result<()> {
    match resolve_source(&config, profile_name).await? {
        CredentialSource::Aws(profile) => login_aws(&profile, force, config).await,
        CredentialSource::Auth(AuthSource::OAuth(client)) => {
            login_oauth(&client, force, no_browser, verbose, config).await
        }
        CredentialSource::Auth(AuthSource::Token(source)) => {
            login_issued(&source);
            Ok(())
        }
    }
}

/// Nothing is authorized and nothing is cached: saying so is the whole of
/// `login` here, and it keeps `kurama login <source>` from being an error a
/// person has to learn does not apply.
fn login_issued(source: &TokenSourceConfig) {
    progress!(
        "# Source '{}' uses a credential issued elsewhere; there is nothing to log in to",
        source.name
    );
}

async fn login_aws(profile: &Profile, force: bool, config: Config) -> Result<()> {
    let profile_name = profile.name();
    let Some(mfa_serial) = profile.mfa_serial_raw().map(str::to_string) else {
        progress!("# Profile '{profile_name}' has no mfa_serial; nothing to log in to");
        return Ok(());
    };
    if !config.aws.session_cache.enabled {
        return Err(CoreError::config(
            "`kurama login` stores the MFA session in the session cache, but [aws.session_cache] enabled = false",
        )
        .into());
    }

    let duration_seconds = config.aws.session_cache.duration;
    let runtime = Runtime::from_config(config, profile).await;
    let output = execute_mfa_login(
        &runtime,
        LoginInput {
            mfa_serial: mfa_serial.clone(),
            duration_seconds,
            force,
        },
    )
    .await
    .context("MFA login failed")?;

    let remaining = describe_remaining(output.expiration, Utc::now());
    if output.reused {
        progress!(
            "# MFA session for {mfa_serial} is still valid ({remaining} left); use --force for a new one"
        );
    } else {
        progress!("# Logged in: MFA session for {mfa_serial} is valid for {remaining}");
    }
    Ok(())
}

async fn login_oauth(
    client: &OAuthClientConfig,
    force: bool,
    no_browser: bool,
    verbose: bool,
    config: Config,
) -> Result<()> {
    let mode = if force {
        TokenMode::ForceGrant
    } else {
        TokenMode::Reuse
    };
    let options = ApiRuntimeOptions {
        open_browser: !no_browser,
        report_secret_reads: verbose,
        ..ApiRuntimeOptions::default()
    };
    let output = ensure_source_token(config, client, mode, options).await?;
    let name = &client.name;
    if let Some(error) = &output.cache_error {
        // `login` exists to make the source ready for an unattended run. A
        // token it could not store has not done that, so saying "logged in"
        // would send the caller into a loop through the same exit code 3.
        return Err(TokenStoreError::Backend(format!(
            "the token for '{name}' was issued but could not be stored: {error}"
        ))
        .into());
    }
    let validity = match output.token.expires_at {
        Some(expires_at) => format!("valid for {}", describe_remaining(expires_at, Utc::now())),
        None => "valid until the API rejects it".to_string(),
    };
    if output.reused {
        progress!("# Token for '{name}' is still {validity}; use --force for a new one");
    } else {
        progress!("# Logged in: token for '{name}' is {validity}");
    }
    Ok(())
}
