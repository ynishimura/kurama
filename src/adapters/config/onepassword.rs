//! `[onepassword]`: the 1Password CLI, the items that hold AWS keys and TOTP
//! codes, and how long a prompt may wait for a person.

use crate::domain::functions::validation::extract_mfa_username;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::default_true;

/// 1Password integration configuration
///
/// # Example config.toml
/// ```toml
/// [onepassword]
/// enabled = true
/// item_name = "aws-credentials"  # Default 1Password item (has AWS creds + MFA)
/// vault = "Agent"                # Optional: vault to search (required for service accounts)
///
/// # Optional: field name configuration (if your 1Password item uses different labels)
/// [onepassword.field_names]
/// access_key_id = "access_key_id"      # Field label for access key (default)
/// secret_access_key = "secret_access_key"  # Field label for secret key (default)
///
/// # Optional: profile-specific mappings
/// [onepassword.mappings]
/// default = "personal-aws"       # source_profile "default" uses this item
/// company = "work-aws"           # source_profile "company" uses this item
/// "arn:aws:iam::123:mfa/user" = "specific-mfa-item"  # MFA serial specific
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OnePasswordConfig {
    /// Enable 1Password integration (default: true)
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Path to 1Password CLI (default: "op")
    #[serde(default = "default_op_cli_path")]
    pub cli_path: String,
    /// Default 1Password item name for AWS credentials and MFA
    #[serde(default)]
    pub item_name: String,
    /// 1Password vault to search (default: none)
    /// Required when authenticating with a service account
    /// (OP_SERVICE_ACCOUNT_TOKEN); optional otherwise, where it
    /// narrows the item search scope.
    #[serde(default)]
    pub vault: Option<String>,
    /// Profile/MFA serial to 1Password item name mappings
    /// - Key can be: source_profile name (e.g., "default") or MFA serial ARN
    /// - Value: 1Password item name
    #[serde(default)]
    pub mappings: HashMap<String, String>,
    /// Field name configuration for 1Password items
    #[serde(default)]
    pub field_names: OnePasswordFieldNames,
    /// The keychain service holding a 1Password service account token, read
    /// for the current user and passed to the CLI as
    /// `OP_SERVICE_ACCOUNT_TOKEN`. A token already in the environment wins.
    /// Without either, the CLI asks a person to approve every read.
    #[serde(default)]
    pub service_account_keychain: Option<String>,
    /// Seconds the CLI may take when nobody is at the terminal (default: 30).
    /// A run that reaches it fails (`SECRET_UNAVAILABLE` for an `op read`,
    /// `MFA_PROVIDER_FAILED` for a TOTP code) instead of hanging.
    #[serde(default = "default_op_timeout")]
    pub timeout: u64,
}

fn default_op_timeout() -> u64 {
    30
}

/// Configuration for 1Password item field names
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OnePasswordFieldNames {
    /// Field label for AWS Access Key ID (default: "access_key_id")
    #[serde(default = "default_access_key_field")]
    pub access_key_id: String,
    /// Field label for AWS Secret Access Key (default: "secret_access_key")
    #[serde(default = "default_secret_key_field")]
    pub secret_access_key: String,
}

impl Default for OnePasswordFieldNames {
    fn default() -> Self {
        Self {
            access_key_id: default_access_key_field(),
            secret_access_key: default_secret_key_field(),
        }
    }
}

fn default_access_key_field() -> String {
    "access_key_id".to_string()
}

fn default_secret_key_field() -> String {
    "secret_access_key".to_string()
}

impl OnePasswordConfig {
    /// 指定されたキー（source_profile名またはMFA serial）に対応するアイテム名を取得
    ///
    /// # Arguments
    /// * `key` - source_profile名（例: "default"）またはMFA serial ARN
    ///
    /// # Returns
    /// 優先順位:
    /// 1. mappings[key] - 直接マッチ
    /// 2. mappings[username] - MFA serialからユーザー名を抽出してマッチ
    /// 3. item_name - デフォルトアイテム
    pub fn get_item_for_key(&self, key: &str) -> Option<String> {
        // 1. 直接マッピングを確認
        if let Some(item_name) = self.mappings.get(key) {
            return Some(item_name.clone());
        }

        // 2. MFA serialからユーザー名を抽出してマッピング確認
        // Use domain function for MFA username extraction
        if let Some(username) = extract_mfa_username(key)
            && let Some(item_name) = self.mappings.get(username)
        {
            return Some(item_name.clone());
        }

        // 3. デフォルトアイテム名を使用
        if !self.item_name.is_empty() {
            return Some(self.item_name.clone());
        }

        None
    }
}

impl Default for OnePasswordConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            cli_path: "op".to_string(),
            item_name: String::new(),
            vault: None,
            mappings: HashMap::new(),
            field_names: OnePasswordFieldNames::default(),
            service_account_keychain: None,
            timeout: default_op_timeout(),
        }
    }
}

fn default_op_cli_path() -> String {
    "op".to_string()
}
