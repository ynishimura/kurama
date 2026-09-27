//! 1Password AWS Credentials Provider
//!
//! This module provides a custom AWS credentials provider that retrieves
//! credentials from 1Password.

use super::onepassword::OnePasswordManager;
use crate::adapters::config::OnePasswordConfig;
use aws_credential_types::{
    Credentials,
    provider::{self, ProvideCredentials, future},
};
use tracing::{info, warn};

/// A credentials provider that retrieves AWS credentials from 1Password
#[derive(Debug, Clone)]
pub struct OnePasswordCredentialsProvider {
    config: OnePasswordConfig,
    item_name: String,
}

impl OnePasswordCredentialsProvider {
    /// Create a new 1Password credentials provider
    pub fn new(config: OnePasswordConfig, item_name: String) -> Self {
        Self { config, item_name }
    }

    /// Create a provider for a specific profile (source_profile name)
    pub fn for_profile(config: OnePasswordConfig, profile_name: &str) -> Option<Self> {
        if !config.enabled {
            return None;
        }
        let item_name = config.get_item_for_key(profile_name)?;
        Some(Self::new(config, item_name))
    }

    /// Get credentials from 1Password
    async fn get_credentials_internal(&self) -> provider::Result {
        info!(
            item_name = %self.item_name,
            "Fetching AWS credentials from 1Password"
        );

        let manager = OnePasswordManager::new(self.config.clone());

        match manager.get_aws_credentials(&self.item_name) {
            Some(creds) => {
                // アクセスキーの先頭4文字のみ表示（セキュリティのため）
                let masked_access_key = if creds.access_key_id.len() > 4 {
                    format!("{}...", &creds.access_key_id[..4])
                } else {
                    "****".to_string()
                };
                info!(
                    item_name = %self.item_name,
                    access_key_id = %masked_access_key,
                    "Successfully retrieved AWS credentials from 1Password"
                );

                Ok(Credentials::new(
                    creds.access_key_id,
                    creds.secret_access_key,
                    None, // No session token for static credentials
                    None, // No expiration for static credentials
                    "1password",
                ))
            }
            None => {
                warn!(
                    item_name = %self.item_name,
                    "1Password CLI is not available or not configured"
                );
                Err(provider::error::CredentialsError::not_loaded(
                    "1Password CLI is not available or not configured",
                ))
            }
        }
    }
}

impl ProvideCredentials for OnePasswordCredentialsProvider {
    fn provide_credentials<'a>(&'a self) -> future::ProvideCredentials<'a>
    where
        Self: 'a,
    {
        future::ProvideCredentials::new(self.get_credentials_internal())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn create_test_config() -> OnePasswordConfig {
        use crate::adapters::config::onepassword::OnePasswordFieldNames;
        OnePasswordConfig {
            enabled: true,
            cli_path: "op".to_string(),
            item_name: "aws-credentials".to_string(),
            vault: None,
            mappings: HashMap::new(),
            field_names: OnePasswordFieldNames::default(),
            ..Default::default()
        }
    }

    #[test]
    fn test_provider_creation() {
        let config = create_test_config();
        let provider = OnePasswordCredentialsProvider::new(config, "test-item".to_string());
        assert_eq!(provider.item_name, "test-item");
    }

    #[test]
    fn test_for_profile() {
        let mut config = create_test_config();
        config
            .mappings
            .insert("prod".to_string(), "prod-aws-creds".to_string());

        let provider = OnePasswordCredentialsProvider::for_profile(config.clone(), "prod");
        assert!(provider.is_some());
        assert_eq!(provider.unwrap().item_name, "prod-aws-creds");

        // Test with default item
        let provider = OnePasswordCredentialsProvider::for_profile(config.clone(), "dev");
        assert!(provider.is_some());
        assert_eq!(provider.unwrap().item_name, "aws-credentials");

        // Test with disabled 1Password
        let mut disabled_config = config;
        disabled_config.enabled = false;
        let provider = OnePasswordCredentialsProvider::for_profile(disabled_config, "prod");
        assert!(provider.is_none());
    }
}
