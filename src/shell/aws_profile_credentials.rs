//! The shared AssumeRole path behind one cache. `AssumeRoleCredentials` is
//! the same session cache, 1Password TOTP and STS calls as `kurama env
//! <profile>`, without the readonly policy; `AssumedRoles` puts one cache in
//! front of it, so a `kurama db` tunnel, an IAM auth token, an `aws-*://`
//! secret reference and a SigV4-signed `kurama api` that name one AWS profile
//! assume its role once.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::Mutex;

use crate::adapters::config::Config;
use crate::adapters::profile::{find_profile, load_profiles};
use crate::domain::types::{Credentials, Profile};
use crate::ports::AwsProfileCredentials;
use crate::shell::cli::executor::CliExecutorError;
use crate::shell::executor::assume_role_for_profile;
use crate::shell::runtime::Runtime;

pub struct AssumeRoleCredentials {
    config: Arc<Config>,
}

impl AssumeRoleCredentials {
    pub fn new(config: Arc<Config>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AwsProfileCredentials for AssumeRoleCredentials {
    async fn load_profile(&self, name: &str) -> Result<Profile> {
        let profiles = load_profiles().await?;
        find_profile(&profiles, name)
            .ok_or_else(|| CliExecutorError::ProfileNotFound(name.to_string()).into())
    }

    async fn assume_role(&self, profile: &Profile) -> Result<Credentials> {
        let runtime = Runtime::from_config((*self.config).clone(), profile).await;
        let output = assume_role_for_profile(&runtime, profile.clone(), false, None).await?;
        Ok(output.credentials)
    }
}

/// How long credentials must still be good for to be reused. The explorer
/// lives as long as a person keeps it open, so a cache with no bound would
/// sign a request with a session that ended while they were reading.
///
/// A function and not a `const`, because cargo-mutants replaces function
/// bodies and leaves constants alone: written as a `const`, this number is a
/// decision no mutant can question, and the test that holds it has to be read
/// to be trusted.
fn still_usable_for() -> chrono::Duration {
    chrono::Duration::seconds(60)
}

/// The roles one call has assumed, so a tunnel, an IAM token and a secret
/// reference that name the same AWS profile cost one AssumeRole between them.
///
/// It is the same port in front of the same path, which is what lets every
/// caller share one cache without knowing who else reads it.
pub struct AssumedRoles {
    inner: Arc<dyn AwsProfileCredentials>,
    assumed: Mutex<HashMap<String, Credentials>>,
}

impl AssumedRoles {
    pub fn new(inner: Arc<dyn AwsProfileCredentials>) -> Self {
        Self {
            inner,
            assumed: Mutex::new(HashMap::new()),
        }
    }

    /// The shared AssumeRole path of `kurama env`, cached per profile.
    pub fn of_config(config: Arc<Config>) -> Self {
        Self::new(Arc::new(AssumeRoleCredentials::new(config)))
    }
}

#[async_trait]
impl AwsProfileCredentials for AssumedRoles {
    async fn load_profile(&self, name: &str) -> Result<Profile> {
        self.inner.load_profile(name).await
    }

    async fn assume_role(&self, profile: &Profile) -> Result<Credentials> {
        let usable = self
            .assumed
            .lock()
            .await
            .get(profile.name())
            .filter(|credentials| !credentials.expires_within(Utc::now(), still_usable_for()))
            .cloned();
        if let Some(credentials) = usable {
            return Ok(credentials);
        }
        let credentials = self.inner.assume_role(profile).await?;
        self.assumed
            .lock()
            .await
            .insert(profile.name().to_owned(), credentials.clone());
        Ok(credentials)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::aws_credentials::MockAwsProfileCredentials;

    /// One AssumeRole per profile, however many things ask for it: the tunnel,
    /// the IAM token and each `aws-*://` reference of one `kurama db` call all
    /// go through this.
    #[tokio::test]
    async fn one_profile_is_assumed_once_and_another_is_assumed_again() {
        let mut inner = MockAwsProfileCredentials::new();
        inner.expect_assume_role().times(2).returning(|profile| {
            Ok(Credentials::new(
                format!("AKIA-{}", profile.name()),
                "secret".into(),
                None,
                Some(Utc::now() + chrono::Duration::hours(1)),
            ))
        });
        let roles = AssumedRoles::new(Arc::new(inner));
        let dev = Profile::new("dev");
        let work = Profile::new("work");
        for _ in 0..3 {
            assert_eq!(
                roles.assume_role(&dev).await.unwrap().access_key_id(),
                "AKIA-dev"
            );
        }
        assert_eq!(
            roles.assume_role(&work).await.unwrap().access_key_id(),
            "AKIA-work"
        );
    }

    /// The explorer stays open as long as a person keeps reading, so the cache
    /// has to end with the session: signing a request with credentials that
    /// expired while the screen sat still is a rejection nobody can explain.
    #[tokio::test]
    async fn a_session_that_is_about_to_end_is_assumed_again() {
        let mut inner = MockAwsProfileCredentials::new();
        let mut issued = 0;
        inner.expect_assume_role().times(2).returning(move |_| {
            issued += 1;
            Ok(Credentials::new(
                format!("AKIA-{issued}"),
                "secret".into(),
                None,
                // Both sides of the margin, one second apart, so the value
                // of `still_usable_for` is what this test holds: 59 seconds
                // is inside it and 61 is not. Fixtures far from the boundary
                // pass for any margin between them.
                Some(Utc::now() + chrono::Duration::seconds(if issued == 1 { 59 } else { 61 })),
            ))
        });
        let roles = AssumedRoles::new(Arc::new(inner));
        let dev = Profile::new("dev");
        assert_eq!(
            roles.assume_role(&dev).await.unwrap().access_key_id(),
            "AKIA-1"
        );
        assert_eq!(
            roles.assume_role(&dev).await.unwrap().access_key_id(),
            "AKIA-2"
        );
        // The second one has a session left, so it is the one that is reused.
        assert_eq!(
            roles.assume_role(&dev).await.unwrap().access_key_id(),
            "AKIA-2"
        );
    }
}
