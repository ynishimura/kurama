//! Credential sources share one namespace: the AWS profiles in
//! `~/.aws/config` and the `[auth.*]` sections of `config.toml`. Every verb
//! that takes a `<PROFILE>` resolves it here; a name present in both places
//! is a `CONFIG_INVALID` error, never a silent choice.

use std::sync::Arc;

use anyhow::{Context, Result};

use crate::adapters::config::Config;
use crate::adapters::config::references::name_collisions;
use crate::adapters::profile::{find_profile, load_profiles};
use crate::domain::Profile;
use crate::domain::types::{
    AuthSource, OAuthClientConfig, RequestAuth, Secret, SecretsSourceConfig, SourceCredential,
};
use crate::shell::api_runtime::{ApiRuntime, ApiRuntimeOptions};
use crate::shell::cli::executor::CliExecutorError;
use crate::workflows::oauth_token::{TokenMode, TokenOutput};

pub enum CredentialSource {
    Aws(Profile),
    Auth(AuthSource),
}

/// Resolve `name` among the AWS profiles and the `[auth.*]` sources.
pub async fn resolve_source(config: &Config, name: &str) -> Result<CredentialSource> {
    let profiles = load_profiles().await?;
    check_name_collisions(&profiles, config)?;
    if let Some(source) = config.auth_source(name)? {
        return Ok(CredentialSource::Auth(source));
    }
    find_profile(&profiles, name)
        .map(CredentialSource::Aws)
        .ok_or_else(|| CliExecutorError::ProfileNotFound(name.to_string()).into())
}

/// A usable token for an OAuth `client` through a fresh `ApiRuntime`: the
/// stored one, a refreshed one, or a new grant (`login`). `TokenOutput`
/// carries what only a grant has -- whether it was reused and whether it
/// could be stored -- which is why `login` uses this and not the credential
/// of any kind below.
pub async fn ensure_source_token(
    config: Config,
    client: &OAuthClientConfig,
    mode: TokenMode,
    options: ApiRuntimeOptions,
) -> Result<TokenOutput> {
    ApiRuntime::from_config(Arc::new(config), options)?
        .ensure_token(client, mode)
        .await
        .with_context(|| format!("Failed to get a token for '{}'", client.name))
}

/// The credential of an `oauth` or `token` source (`token`, `env`, `exec`).
pub async fn ensure_source_credential(
    config: Config,
    source: &RequestAuth,
    options: ApiRuntimeOptions,
) -> Result<SourceCredential> {
    ApiRuntime::from_config(Arc::new(config), options)?
        .ensure_credential(source, TokenMode::Reuse)
        .await
        .with_context(|| format!("Failed to get a token for '{}'", source.name()))
}

/// Every value of a `secrets` source (`env`, `exec`), or the failure of the
/// first reference that could not be read.
pub async fn resolve_source_secrets(
    config: Config,
    source: &SecretsSourceConfig,
    options: ApiRuntimeOptions,
) -> Result<Vec<(String, Secret)>> {
    ApiRuntime::from_config(Arc::new(config), options)?
        .resolve_secrets(source)
        .await
}

/// An `[auth.*]` name that is also an AWS profile name is a configuration error.
pub fn check_name_collisions(profiles: &[Profile], config: &Config) -> Result<()> {
    match name_collisions(profiles, config).into_iter().next() {
        Some((_, error)) => Err(error.into()),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::config::AuthToml;
    use crate::adapters::error::CoreError;

    fn config_with_auth(name: &str) -> Config {
        let mut config = Config::default();
        config.auth.insert(
            name.into(),
            toml::from_str::<AuthToml>(
                "kind = \"oauth\"\ngrant_type = \"client_credentials\"\ntoken_url = \"https://as/token\"\nclient_id = \"id\"\n",
            )
            .unwrap(),
        );
        config
    }

    #[test]
    fn a_shared_name_is_a_configuration_error() {
        let profiles = [Profile::new("dev"), Profile::new("github")];
        let error = check_name_collisions(&profiles, &config_with_auth("github")).unwrap_err();
        assert!(matches!(
            error.downcast_ref::<CoreError>(),
            Some(CoreError::Configuration(message)) if message.contains("[auth.github]")
        ));
        assert!(check_name_collisions(&profiles, &config_with_auth("svc")).is_ok());
        assert!(check_name_collisions(&profiles, &Config::default()).is_ok());
    }
}
