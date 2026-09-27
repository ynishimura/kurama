//! AWS profile credentials port: the profile and the temporary credentials
//! `kurama api` signs with when an `[api.*]` profile names an `aws_profile`.
//! The shell implements it on the shared AssumeRole executor (session cache,
//! 1Password TOTP, STS); unit tests mock it.

use async_trait::async_trait;

use crate::domain::types::{Credentials, Profile};

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait AwsProfileCredentials: Send + Sync {
    /// The profile from `~/.aws/config`: its region is needed before any
    /// credential is requested. An unknown name is an error.
    async fn load_profile(&self, name: &str) -> anyhow::Result<Profile>;

    /// Temporary credentials for the profile through AssumeRole.
    async fn assume_role(&self, profile: &Profile) -> anyhow::Result<Credentials>;
}
