//! `kurama logout <PROFILE> | --all`: remove cached MFA sessions and stored
//! OAuth tokens.
//!
//! AWS sessions are keyed by MFA device, so logging out one profile also
//! logs out every other profile that uses the same device. An `[auth.*]`
//! source of kind `oauth` drops its token from the token store. A source of
//! kind `token` stored nothing, so there is nothing to remove and `--all`
//! does not name it.

use std::collections::BTreeSet;

use anyhow::Result;

use super::source::check_name_collisions;
use crate::adapters::config::Config;
use crate::adapters::profile::{find_profile, load_profiles};
use crate::adapters::session_cache::create_session_cache;
use crate::adapters::token_store::create_token_store;
use crate::console::progress;
use crate::domain::Profile;
use crate::domain::types::AuthSource;
use crate::ports::{SessionCache, SessionCacheError, TokenStore, TokenStoreError};
use crate::shell::cli::executor::CliExecutorError;

/// Which credentials to remove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogoutTarget {
    /// The session of this AWS profile's MFA device, or this source's token
    Profile(String),
    /// Every MFA device in ~/.aws/config and every `[auth.*]` token
    All,
}

pub async fn handle_logout_command(target: &LogoutTarget, config: &Config) -> Result<()> {
    let profiles = load_profiles().await?;
    check_name_collisions(&profiles, config)?;
    let (aws_profiles, auth_names): (Vec<Profile>, Vec<String>) = match target {
        LogoutTarget::All => (profiles, stored_token_sources(config)?),
        LogoutTarget::Profile(name) if config.auth.contains_key(name) => {
            if !stored_token_sources(config)?.contains(name) {
                progress!(
                    "# Source '{name}' uses a credential issued elsewhere; there is nothing to remove"
                );
                return Ok(());
            }
            (vec![], vec![name.clone()])
        }
        LogoutTarget::Profile(name) => (
            vec![
                find_profile(&profiles, name)
                    .ok_or_else(|| CliExecutorError::ProfileNotFound(name.clone()))?,
            ],
            vec![],
        ),
    };
    // Removing must work even when caching is disabled in configuration.
    let cache = create_session_cache(true);
    let mut removed = clear_sessions(&aws_profiles, cache.as_ref()).await?;
    if !auth_names.is_empty() {
        removed.extend(clear_tokens(&auth_names, create_token_store().as_ref()).await?);
    }
    if removed.is_empty() {
        progress!("# No MFA device or token configured; nothing to log out");
    }
    Ok(())
}

/// The sources that can have a token in the store: only a grant writes one,
/// so a `kind = "token"` source is not among them.
fn stored_token_sources(config: &Config) -> Result<Vec<String>> {
    Ok(config
        .auth_sources()?
        .into_iter()
        .filter(|source| matches!(source, AuthSource::OAuth(_)))
        .map(|source| source.name().to_string())
        .collect())
}

/// Remove the session of each distinct MFA device; returns the removed keys.
pub async fn clear_sessions(
    profiles: &[Profile],
    cache: &dyn SessionCache,
) -> Result<Vec<String>, SessionCacheError> {
    let keys: BTreeSet<&str> = profiles
        .iter()
        .filter_map(Profile::mfa_serial_raw)
        .collect();
    let mut removed = Vec::new();
    for key in keys {
        cache.remove(key).await?;
        progress!("# Cleared MFA session: {key}");
        removed.push(key.to_string());
    }
    Ok(removed)
}

/// Remove the token of each `[auth.*]` source; returns the removed names.
pub async fn clear_tokens(
    names: &[String],
    store: &dyn TokenStore,
) -> Result<Vec<String>, TokenStoreError> {
    let mut removed = Vec::new();
    for name in names {
        store.remove(name).await?;
        progress!("# Cleared token: {name}");
        removed.push(name.clone());
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::session_cache::MockSessionCache;
    use crate::ports::token_store::MockTokenStore;

    fn profiles() -> Vec<Profile> {
        vec![
            Profile::new("one").with_mfa_serial_raw("arn:aws:iam::111111111111:mfa/user"),
            Profile::new("two").with_mfa_serial_raw("arn:aws:iam::111111111111:mfa/user"),
            Profile::new("three").with_mfa_serial_raw("arn:aws:iam::222222222222:mfa/user"),
            Profile::new("no-mfa"),
        ]
    }

    #[tokio::test]
    async fn clears_each_mfa_serial_once() {
        let mut cache = MockSessionCache::new();
        for account in ["111111111111", "222222222222"] {
            cache
                .expect_remove()
                .times(1)
                .withf(move |k| k.contains(account))
                .returning(|_| Ok(()));
        }
        let keys = clear_sessions(&profiles(), &cache).await.unwrap();
        assert_eq!(keys.len(), 2);
        assert!(keys[0].contains("111111111111") && keys[1].contains("222222222222"));
    }

    #[tokio::test]
    async fn failed_removal_is_reported() {
        let mut cache = MockSessionCache::new();
        cache
            .expect_remove()
            .returning(|_| Err(SessionCacheError::Backend("locked".into())));
        assert!(clear_sessions(&profiles()[..1], &cache).await.is_err());
    }

    #[tokio::test]
    async fn no_mfa_profiles_needs_no_keychain() {
        assert!(
            clear_sessions(&[Profile::new("no-mfa")], &MockSessionCache::new())
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// `logout --all` must not name a `kind = "token"` source: nothing ever
    /// wrote a token store entry for it, so removing one would be a keychain
    /// call and a "Cleared token" line about a credential that is still
    /// exactly where it was.
    #[test]
    fn only_oauth_sources_have_a_token_to_remove() {
        let config = Config::parse(
            "[auth.github]\nkind = \"oauth\"\ngrant_type = \"client_credentials\"\n\
             token_url = \"https://as/token\"\nclient_id = \"id\"\n\
             [auth.example]\nkind = \"token\"\ntoken = \"op://Agent/Example/credential\"\n",
        )
        .unwrap();
        assert_eq!(stored_token_sources(&config).unwrap(), ["github"]);
    }

    #[tokio::test]
    async fn tokens_are_removed_by_source_name() {
        let mut store = MockTokenStore::new();
        store
            .expect_remove()
            .times(1)
            .withf(|name| name == "github")
            .returning(|_| Ok(()));
        let removed = clear_tokens(&["github".to_string()], &store).await.unwrap();
        assert_eq!(removed, ["github"]);
        let mut failing = MockTokenStore::new();
        failing
            .expect_remove()
            .returning(|_| Err(TokenStoreError::Backend("locked".into())));
        assert!(
            clear_tokens(&["github".to_string()], &failing)
                .await
                .is_err()
        );
    }
}
