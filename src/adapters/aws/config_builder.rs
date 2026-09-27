//! The AWS SDK configuration for STS: profile, region and the 1Password credentials provider; inherited `AWS_*` variables are cleared here, at the SDK boundary.

use crate::adapters::auth::OnePasswordCredentialsProvider;
use crate::adapters::config::OnePasswordConfig;
use crate::console::progress;
use crate::domain::functions::export::AWS_ENV_VARS;
use aws_config::{BehaviorVersion, SdkConfig};
use tracing::{debug, info};

pub struct AwsConfigBuilder {
    profile: Option<String>,
    behavior_version: BehaviorVersion,
    onepassword_config: Option<OnePasswordConfig>,
}

impl Default for AwsConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl AwsConfigBuilder {
    /// Create a new builder with default settings
    pub fn new() -> Self {
        Self {
            profile: None,
            behavior_version: BehaviorVersion::latest(),
            onepassword_config: None,
        }
    }

    /// Clear AWS credential environment variables
    ///
    /// This MUST be called immediately before AWS SDK config is built
    /// to prevent stale credentials from being picked up.
    ///
    /// Uses the shared `AWS_ENV_VARS` list from `domain::functions::export`
    /// to ensure consistency with export/unset operations.
    fn clear_aws_env_vars() -> usize {
        let mut cleared = 0;
        for var in AWS_ENV_VARS {
            if std::env::var(var).is_ok() {
                debug!(var = %var, "Clearing AWS env var before SDK init");
                // SAFETY: called on the way into the SDK, before the runtime
                // this process builds has a second thread that reads the
                // environment.
                unsafe { std::env::remove_var(var) };
                cleared += 1;
            }
        }
        if cleared > 0 {
            progress!("# Cleared {cleared} AWS environment variable(s)");
        }
        cleared
    }

    /// Set the AWS profile name
    pub fn with_profile(mut self, profile: impl Into<String>) -> Self {
        self.profile = Some(profile.into());
        self
    }

    /// Set 1Password configuration for credentials retrieval
    pub fn with_onepassword(mut self, config: OnePasswordConfig) -> Self {
        self.onepassword_config = Some(config);
        self
    }

    /// Build the AWS SDK configuration
    pub async fn build(self) -> SdkConfig {
        // CRITICAL: Clear AWS environment variables RIGHT BEFORE building SDK config
        // This ensures the SDK doesn't pick up stale credentials from environment
        Self::clear_aws_env_vars();

        info!(
            profile = ?self.profile,
            has_onepassword_config = self.onepassword_config.is_some(),
            "Building AWS SDK configuration"
        );

        let mut loader = aws_config::defaults(self.behavior_version);

        if let Some(profile) = &self.profile {
            debug!(profile = %profile, "Setting AWS profile");
            loader = loader.profile_name(profile);
        }

        // Priority: 1Password > default chain
        if let Some(op_config) = self.onepassword_config {
            // Try to create 1Password credentials provider
            let profile_name = self.profile.as_deref().unwrap_or("default");
            debug!(
                profile_name = %profile_name,
                enabled = op_config.enabled,
                "Checking 1Password configuration"
            );

            if let Some(provider) =
                OnePasswordCredentialsProvider::for_profile(op_config.clone(), profile_name)
            {
                info!(
                    profile_name = %profile_name,
                    "Using 1Password credentials provider"
                );
                loader = loader.credentials_provider(provider);
            } else {
                debug!(
                    profile_name = %profile_name,
                    "1Password credentials not configured for profile, using default chain"
                );
            }
        } else {
            debug!("Using default AWS credential chain");
        }

        debug!("Loading AWS SDK configuration...");
        let config = loader.load().await;

        info!(
            region = ?config.region().map(|r| r.as_ref()),
            has_credentials_provider = config.credentials_provider().is_some(),
            "AWS SDK configuration built successfully"
        );

        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::utils::test_env;
    use serial_test::serial;

    #[test]
    #[serial]
    fn clears_legacy_switching_vars_but_preserves_ca_bundle() {
        test_env::set("AWS_DEFAULT_PROFILE", "legacy");
        test_env::set("AWS_CREDENTIAL_EXPIRATION", "legacy-expiration");

        AwsConfigBuilder::clear_aws_env_vars();

        assert!(std::env::var("AWS_DEFAULT_PROFILE").is_err());
        assert!(std::env::var("AWS_CREDENTIAL_EXPIRATION").is_err());
        assert!(!AWS_ENV_VARS.contains(&"AWS_CA_BUNDLE"));
    }

    #[test]
    fn test_builder_fluent_interface() {
        let builder = AwsConfigBuilder::new().with_profile("test");
        // Test that builder pattern works
        assert!(builder.profile.is_some());
        assert_eq!(builder.profile.as_ref().unwrap(), "test");
    }

    #[test]
    fn test_builder_default() {
        let builder = AwsConfigBuilder::default();
        assert!(builder.profile.is_none());
        assert!(builder.onepassword_config.is_none());
    }

    #[test]
    fn test_builder_with_onepassword() {
        let op_config = OnePasswordConfig {
            item_name: "aws-creds".to_string(),
            ..Default::default()
        };

        let builder = AwsConfigBuilder::new().with_onepassword(op_config);

        assert!(builder.onepassword_config.is_some());
    }
}
