//! The 1Password CLI (`op`) adapter: TOTP codes for the `MfaProvider` port and AWS access keys read from an item.

use crate::adapters::auth::op_cli::OpCli;
use crate::adapters::config::OnePasswordConfig;
use crate::adapters::error::CoreError;
use crate::domain::functions::onepassword::{
    ItemGetOutput, OpItem, build_error_message, build_item_get_args, classify_error,
    find_field_value, find_otp_value, is_valid_otp, parse_op_item,
};
use crate::ports::mfa::{MfaError, MfaProvider};
use anyhow::Result;
use async_trait::async_trait;
use tracing::{debug, info, warn};

/// AWS credentials retrieved from 1Password
#[derive(Clone)]
pub struct OnePasswordAwsCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
}

impl std::fmt::Debug for OnePasswordAwsCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OnePasswordAwsCredentials")
            .field("access_key_id", &"[REDACTED]")
            .field("secret_access_key", &"[REDACTED]")
            .finish()
    }
}

/// Trait for 1Password CLI operations (allows mocking in tests)
pub trait OnePasswordCli: Send + Sync {
    /// Check if CLI is available
    fn is_available(&self) -> bool;

    /// Execute CLI command and return output
    fn execute(&self, args: &[&str]) -> std::io::Result<std::process::Output>;
}

/// Real implementation: every call goes through [`OpCli`], which carries the
/// path, the service account token and the deadline.
pub struct RealOnePasswordCli {
    op: OpCli,
}

impl OnePasswordCli for RealOnePasswordCli {
    fn is_available(&self) -> bool {
        self.op.is_available()
    }

    fn execute(&self, args: &[&str]) -> std::io::Result<std::process::Output> {
        self.op.run(args)
    }
}

pub struct OnePasswordManager<C: OnePasswordCli = RealOnePasswordCli> {
    config: OnePasswordConfig,
    cli: C,
}

impl OnePasswordManager<RealOnePasswordCli> {
    pub fn new(config: OnePasswordConfig) -> Self {
        let op = OpCli::from_config(&config);
        Self {
            config,
            cli: RealOnePasswordCli { op },
        }
    }
}

impl<C: OnePasswordCli> OnePasswordManager<C> {
    /// Create a new manager with a custom CLI implementation
    #[cfg(test)]
    pub fn with_cli(config: OnePasswordConfig, cli: C) -> Self {
        Self { config, cli }
    }

    /// AWS credentials (access_key_id and secret_access_key) from 1Password,
    /// or `None` when the CLI is missing or the item cannot be read (logged).
    pub fn get_aws_credentials(&self, item_name: &str) -> Option<OnePasswordAwsCredentials> {
        debug!(
            item_name = %item_name,
            cli_path = %self.config.cli_path,
            "Attempting to get AWS credentials from 1Password"
        );

        // Check if 1Password CLI is available
        if !self.cli.is_available() {
            warn!(
                cli_path = %self.config.cli_path,
                "1Password CLI not found or not executable"
            );
            return None;
        }

        debug!(item_name = %item_name, "1Password CLI available, fetching credentials");

        // Get the credentials from 1Password
        // Note: Logging is handled in fetch_aws_credentials
        self.fetch_aws_credentials(item_name)
            .inspect_err(|e| {
                warn!(
                    item_name = %item_name,
                    error = %e,
                    "Failed to retrieve AWS credentials from 1Password"
                );
            })
            .ok()
    }

    /// Fetch AWS credentials from 1Password item
    fn fetch_aws_credentials(
        &self,
        item_name: &str,
    ) -> Result<OnePasswordAwsCredentials, CoreError> {
        debug!(item_name = %item_name, "Fetching AWS credentials from 1Password item");

        debug!(
            cli_path = %self.config.cli_path,
            item_name = %item_name,
            "Executing 1Password CLI to fetch item"
        );

        let item = self.item_json(item_name)?;

        debug!(
            item_name = %item_name,
            fields_count = item.fields.len(),
            "Parsed 1Password item, searching for credential fields"
        );

        // Extract credentials using configured field names with functional composition
        let access_key_field = &self.config.field_names.access_key_id;
        let secret_key_field = &self.config.field_names.secret_access_key;

        let field = |name: &str| {
            find_field_value(&item.fields, name).ok_or_else(|| {
                CoreError::onepassword_error(format!(
                    "Field '{}' not found in 1Password item '{}'",
                    name, item_name
                ))
            })
        };
        let access_key_id = field(access_key_field)?;
        let secret_access_key = field(secret_key_field)?;
        info!(item_name = %item_name, "Successfully found AWS credentials");
        Ok(OnePasswordAwsCredentials {
            access_key_id,
            secret_access_key,
        })
    }

    /// `op item get --format json`: the whole item, or why it could not be read.
    fn item_json(&self, item_name: &str) -> Result<OpItem, CoreError> {
        debug!(
            cli_path = %self.config.cli_path,
            item_name = %item_name,
            "Executing 1Password CLI to fetch item"
        );
        let args =
            build_item_get_args(item_name, self.config.vault.as_deref(), ItemGetOutput::Json);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let output = self.cli.execute(&args).map_err(|e| {
            warn!(
                cli_path = %self.config.cli_path,
                item_name = %item_name,
                error = %e,
                "Failed to execute 1Password CLI"
            );
            CoreError::onepassword_error(format!(
                "Failed to execute 1Password CLI: {}\n\
                    cli_path: {}\n\
                    Please check:\n\
                    - Are you signed in to 1Password? (op signin)\n\
                    - Does item '{}' exist?",
                e, self.config.cli_path, item_name
            ))
        })?;

        debug!(
            exit_code = ?output.status.code(),
            stdout_len = output.stdout.len(),
            stderr_len = output.stderr.len(),
            "1Password CLI command completed"
        );

        if !output.status.success() {
            let error_msg = String::from_utf8_lossy(&output.stderr);
            warn!(
                item_name = %item_name,
                exit_code = ?output.status.code(),
                stderr = %error_msg,
                "1Password CLI returned error"
            );
            return Err(self.create_detailed_error(item_name, &error_msg));
        }

        parse_op_item(&output.stdout).map_err(|e| {
            warn!(
                item_name = %item_name,
                error = %e,
                "Failed to parse 1Password JSON response"
            );
            CoreError::other(format!(
                "Failed to parse 1Password response.\n\
                Item: {}\n\
                Error: {}\n\
                Please verify that the 1Password item format is correct.",
                item_name, e
            ))
        })
    }

    /// Create detailed error message for common 1Password errors
    /// Uses pure functions from domain layer for error classification
    fn create_detailed_error(&self, item_name: &str, error_msg: &str) -> CoreError {
        let error_kind = classify_error(error_msg);
        let message = build_error_message(&error_kind, item_name, error_msg);
        CoreError::onepassword_error(message)
    }

    pub fn get_mfa_token(&self, mfa_serial: &str) -> Result<Option<String>, CoreError> {
        debug!(
            mfa_serial = %mfa_serial,
            cli_path = %self.config.cli_path,
            "Attempting to get MFA token from 1Password"
        );

        // Check if 1Password CLI is available - return None if not
        if !self.cli.is_available() {
            warn!(
                cli_path = %self.config.cli_path,
                "1Password CLI not found or not executable"
            );
            return Ok(None);
        }

        // Chain: get_item_name -> fetch_totp, converting errors to Ok(None)
        // Using inspect_err to log failures with full context (including item_name)
        let item_name = match self.get_item_name(mfa_serial) {
            Ok(item_name) => item_name,
            Err(error) => {
                warn!(
                    mfa_serial = %mfa_serial,
                    error = %error,
                    "Could not determine 1Password item name for MFA serial"
                );
                return Err(error);
            }
        };
        let result = Ok(item_name)
            .inspect(|item_name: &String| {
                debug!(
                    mfa_serial = %mfa_serial,
                    item_name = %item_name,
                    "Looking up MFA token for 1Password item"
                );
            })
            .and_then(|item_name| {
                self.fetch_totp(&item_name)
                    .inspect(|_| {
                        info!(
                            item_name = %item_name,
                            mfa_serial = %mfa_serial,
                            "Successfully retrieved MFA token from 1Password"
                        );
                    })
                    .inspect_err(|e| {
                        warn!(
                            item_name = %item_name,
                            mfa_serial = %mfa_serial,
                            error = %e,
                            "Failed to retrieve MFA token from 1Password"
                        );
                    })
            });

        // The CLI is installed and configured, so a failure here (not signed
        // in, missing item, no OTP field) is a provider error the user must
        // act on; only a missing CLI falls back to manual entry (`Ok(None)`).
        result.map(Some)
    }

    fn get_item_name(&self, mfa_serial: &str) -> Result<String, CoreError> {
        // Use unified config method for item lookup
        if let Some(item_name) = self.config.get_item_for_key(mfa_serial) {
            debug!(
                mfa_serial = %mfa_serial,
                item_name = %item_name,
                "Found 1Password item for MFA serial"
            );
            info!("Using configured default 1Password item: '{}'", item_name);
            return Ok(item_name);
        }

        debug!(
            mfa_serial = %mfa_serial,
            "1Password item mapping not found for MFA serial"
        );
        Err(CoreError::onepassword_error(format!(
            "1Password item mapping not configured for MFA serial '{}'.",
            mfa_serial
        )))
    }

    fn fetch_totp(&self, item_name: &str) -> Result<String, CoreError> {
        // Try to get TOTP directly first (only if CLI execution succeeds)
        let otp_args =
            build_item_get_args(item_name, self.config.vault.as_deref(), ItemGetOutput::Otp);
        let otp_args: Vec<&str> = otp_args.iter().map(String::as_str).collect();
        // A deadline is per invocation, so a fallback would wait for it twice.
        // Its message already says what to do; `cli_error` would bury it under
        // advice that needs the terminal this run does not have.
        let direct = match self.cli.execute(&otp_args) {
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {
                return Err(CoreError::onepassword_error(error.to_string()));
            }
            other => other,
        };
        let direct_otp_result = direct
            .ok() // I/O error → try fallback (not a fatal error for --otp attempt)
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|token| is_valid_otp(token));

        // Return early if direct OTP fetch succeeded
        if let Some(token) = direct_otp_result {
            return Ok(token);
        }

        // Fallback: get full item and extract TOTP via JSON parsing
        let item = self.item_json(item_name)?;
        find_otp_value(&item.fields).ok_or_else(|| {
                    CoreError::onepassword_error(format!(
                        "OTP/TOTP field not found in 1Password item '{}'.\n\
                        Please check:\n\
                        - Is two-factor authentication (OTP/TOTP) configured for the item?\n\
                        - Does the field label contain 'OTP', 'TOTP', or 'MFA'?\n\
                        - Is the one-time password a 6-digit number?\n\
                        \n\
                        Please open the item in 1Password and verify the two-factor authentication settings.",
                        item_name
                    ))
                })
    }
}

/// Implementation of MfaProvider port for OnePasswordManager
///
/// This allows OnePasswordManager to be used through the port abstraction,
/// enabling dependency injection and easier testing.
#[async_trait]
impl<C: OnePasswordCli> MfaProvider for OnePasswordManager<C> {
    async fn get_token(&self, mfa_serial: &str) -> Result<Option<String>, MfaError> {
        // Delegate to the existing synchronous implementation
        // Note: The underlying 1Password CLI call is blocking, but wrapped in async
        self.get_mfa_token(mfa_serial).map_err(|e| {
            let error_str = e.to_string();
            if error_str.contains("not found") || error_str.contains("mapping not configured") {
                MfaError::TokenNotFound(mfa_serial.to_string())
            } else {
                MfaError::ProviderError(error_str)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    /// Mock CLI for testing
    struct MockOnePasswordCli {
        available: bool,
        credentials_response: Option<OnePasswordAwsCredentials>,
        totp_response: Option<String>,
        error_message: Option<String>,
    }

    impl MockOnePasswordCli {
        fn available() -> Self {
            Self {
                available: true,
                credentials_response: None,
                totp_response: None,
                error_message: None,
            }
        }

        fn unavailable() -> Self {
            Self {
                available: false,
                credentials_response: None,
                totp_response: None,
                error_message: None,
            }
        }

        fn with_credentials(mut self, access_key: &str, secret_key: &str) -> Self {
            self.credentials_response = Some(OnePasswordAwsCredentials {
                access_key_id: access_key.to_string(),
                secret_access_key: secret_key.to_string(),
            });
            self
        }

        fn with_totp(mut self, token: &str) -> Self {
            self.totp_response = Some(token.to_string());
            self
        }

        fn with_error(mut self, message: &str) -> Self {
            self.error_message = Some(message.to_string());
            self
        }
    }

    impl OnePasswordCli for MockOnePasswordCli {
        fn is_available(&self) -> bool {
            self.available
        }

        fn execute(&self, args: &[&str]) -> std::io::Result<std::process::Output> {
            // Check if this is an error case
            if let Some(ref error_msg) = self.error_message {
                return Ok(std::process::Output {
                    status: ExitStatus::from_raw(256), // exit code 1
                    stdout: vec![],
                    stderr: error_msg.as_bytes().to_vec(),
                });
            }

            // Check if this is an OTP request
            if args.contains(&"--otp") {
                if let Some(ref token) = self.totp_response {
                    return Ok(std::process::Output {
                        status: ExitStatus::from_raw(0),
                        stdout: token.as_bytes().to_vec(),
                        stderr: vec![],
                    });
                }
                // Return empty for OTP, will fall back to JSON parsing
                return Ok(std::process::Output {
                    status: ExitStatus::from_raw(256),
                    stdout: vec![],
                    stderr: b"No OTP configured".to_vec(),
                });
            }

            // JSON format request for credentials
            if args.contains(&"--format") && args.contains(&"json") {
                if let Some(ref creds) = self.credentials_response {
                    // Use default field names matching OnePasswordFieldNames::default()
                    let json = format!(
                        r#"{{
                            "fields": [
                                {{"label": "access_key_id", "value": "{}"}},
                                {{"label": "secret_access_key", "value": "{}"}}
                            ]
                        }}"#,
                        creds.access_key_id, creds.secret_access_key
                    );
                    return Ok(std::process::Output {
                        status: ExitStatus::from_raw(0),
                        stdout: json.into_bytes(),
                        stderr: vec![],
                    });
                }

                // Check if we have TOTP in JSON format
                if let Some(ref token) = self.totp_response {
                    let json = format!(
                        r#"{{
                            "fields": [
                                {{"label": "OTP", "value": "{}"}}
                            ]
                        }}"#,
                        token
                    );
                    return Ok(std::process::Output {
                        status: ExitStatus::from_raw(0),
                        stdout: json.into_bytes(),
                        stderr: vec![],
                    });
                }
            }

            // Default: item not found
            Ok(std::process::Output {
                status: ExitStatus::from_raw(256),
                stdout: vec![],
                stderr: b"could not find item".to_vec(),
            })
        }
    }

    /// Mock CLI that records every executed argument list.
    /// Cloning shares the same recording buffer.
    #[derive(Clone)]
    struct RecordingCli {
        calls: std::sync::Arc<std::sync::Mutex<Vec<Vec<String>>>>,
    }

    impl RecordingCli {
        fn new() -> Self {
            Self {
                calls: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            }
        }

        fn recorded_calls(&self) -> Vec<Vec<String>> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl OnePasswordCli for RecordingCli {
        fn is_available(&self) -> bool {
            true
        }

        fn execute(&self, args: &[&str]) -> std::io::Result<std::process::Output> {
            self.calls
                .lock()
                .unwrap()
                .push(args.iter().map(|s| s.to_string()).collect());
            let json = r#"{
                "fields": [
                    {"label": "access_key_id", "value": "AKIATEST"},
                    {"label": "secret_access_key", "value": "secrettest"}
                ]
            }"#;
            Ok(std::process::Output {
                status: ExitStatus::from_raw(0),
                stdout: json.as_bytes().to_vec(),
                stderr: vec![],
            })
        }
    }

    fn create_test_config() -> OnePasswordConfig {
        use crate::adapters::config::onepassword::OnePasswordFieldNames;
        OnePasswordConfig {
            enabled: true,
            cli_path: "op".to_string(),
            item_name: "aws-default".to_string(),
            vault: None,
            mappings: HashMap::new(),
            field_names: OnePasswordFieldNames::default(),
            ..Default::default()
        }
    }

    #[test]
    fn test_get_item_name_from_mapping() {
        let mut config = create_test_config();
        config.mappings.insert(
            "arn:aws:iam::123456789012:mfa/testuser".to_string(),
            "my-aws-account".to_string(),
        );

        let manager = OnePasswordManager::with_cli(config, MockOnePasswordCli::unavailable());
        let item_name = manager
            .get_item_name("arn:aws:iam::123456789012:mfa/testuser")
            .unwrap();
        assert_eq!(item_name, "my-aws-account");
    }

    #[test]
    fn test_get_item_name_from_username() {
        use crate::adapters::config::onepassword::OnePasswordFieldNames;
        // Config without item_name to test username extraction
        let config = OnePasswordConfig {
            enabled: true,
            cli_path: "op".to_string(),
            item_name: String::new(), // No default
            vault: None,
            mappings: HashMap::new(),
            field_names: OnePasswordFieldNames::default(),
            ..Default::default()
        };
        let manager = OnePasswordManager::with_cli(config, MockOnePasswordCli::unavailable());

        // Since there's no default and no mapping, this should fail
        let result = manager.get_item_name("arn:aws:iam::123456789012:mfa/john.doe");
        assert!(result.is_err());
    }

    #[test]
    fn test_get_item_name_default() {
        let config = create_test_config();
        let manager = OnePasswordManager::with_cli(config, MockOnePasswordCli::unavailable());

        // Test with invalid MFA serial format
        let item_name = manager.get_item_name("invalid-mfa-serial").unwrap();
        assert_eq!(item_name, "aws-default");
    }

    #[test]
    fn test_get_item_name_username_mapping() {
        let mut config = create_test_config();
        config
            .mappings
            .insert("john.doe".to_string(), "johns-aws".to_string());

        let manager = OnePasswordManager::with_cli(config, MockOnePasswordCli::unavailable());
        let item_name = manager
            .get_item_name("arn:aws:iam::123456789012:mfa/john.doe")
            .unwrap();
        assert_eq!(item_name, "johns-aws");
    }

    #[test]
    fn test_cli_unavailable() {
        let config = create_test_config();
        let manager = OnePasswordManager::with_cli(config, MockOnePasswordCli::unavailable());

        assert!(manager.get_aws_credentials("test-item").is_none());
    }

    #[test]
    fn test_get_aws_credentials_success() {
        let config = create_test_config();
        let mock_cli =
            MockOnePasswordCli::available().with_credentials("AKIAIOSFODNN7EXAMPLE", "secret123");
        let manager = OnePasswordManager::with_cli(config, mock_cli);

        let creds = manager.get_aws_credentials("test-item").unwrap();
        assert_eq!(creds.access_key_id, "AKIAIOSFODNN7EXAMPLE");
        assert_eq!(creds.secret_access_key, "secret123");
    }

    #[test]
    fn test_get_aws_credentials_item_not_found() {
        let config = create_test_config();
        let mock_cli = MockOnePasswordCli::available(); // No credentials set
        let manager = OnePasswordManager::with_cli(config, mock_cli);

        assert!(manager.get_aws_credentials("nonexistent-item").is_none());
    }

    #[test]
    fn test_get_mfa_token_cli_unavailable() {
        let config = create_test_config();
        let manager = OnePasswordManager::with_cli(config, MockOnePasswordCli::unavailable());

        let result = manager.get_mfa_token("arn:aws:iam::123456789012:mfa/test");
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_get_mfa_token_success() {
        let config = create_test_config();
        let mock_cli = MockOnePasswordCli::available().with_totp("654321");
        let manager = OnePasswordManager::with_cli(config, mock_cli);

        let result = manager.get_mfa_token("arn:aws:iam::123456789012:mfa/test");
        assert!(result.is_ok());
        let token = result.unwrap().unwrap();
        assert_eq!(token, "654321");
    }

    #[test]
    fn test_error_message_not_signed_in() {
        let config = create_test_config();
        let mock_cli =
            MockOnePasswordCli::available().with_error("not currently signed in to 1Password");
        let manager = OnePasswordManager::with_cli(config, mock_cli);

        assert!(manager.get_aws_credentials("test-item").is_none());
    }

    #[test]
    fn test_error_message_session_expired() {
        let config = create_test_config();
        let mock_cli = MockOnePasswordCli::available().with_error("session expired");
        let manager = OnePasswordManager::with_cli(config, mock_cli);

        assert!(manager.get_aws_credentials("test-item").is_none());
    }

    #[test]
    fn test_custom_field_names() {
        use crate::adapters::config::onepassword::OnePasswordFieldNames;

        // Create config with custom field names
        let config = OnePasswordConfig {
            enabled: true,
            cli_path: "op".to_string(),
            item_name: "aws-default".to_string(),
            vault: None,
            mappings: HashMap::new(),
            field_names: OnePasswordFieldNames {
                access_key_id: "my_access_key".to_string(),
                secret_access_key: "my_secret_key".to_string(),
            },
            ..Default::default()
        };

        // Create a mock that returns credentials with custom field names
        struct CustomFieldMock;
        impl OnePasswordCli for CustomFieldMock {
            fn is_available(&self) -> bool {
                true
            }
            fn execute(&self, args: &[&str]) -> std::io::Result<std::process::Output> {
                if args.contains(&"--format") && args.contains(&"json") {
                    let json = r#"{
                        "fields": [
                            {"label": "my_access_key", "value": "AKIACUSTOM123"},
                            {"label": "my_secret_key", "value": "secretcustom456"}
                        ]
                    }"#;
                    return Ok(std::process::Output {
                        status: ExitStatus::from_raw(0),
                        stdout: json.as_bytes().to_vec(),
                        stderr: vec![],
                    });
                }
                Ok(std::process::Output {
                    status: ExitStatus::from_raw(256),
                    stdout: vec![],
                    stderr: b"not found".to_vec(),
                })
            }
        }

        let manager = OnePasswordManager::with_cli(config, CustomFieldMock);
        let creds = manager.get_aws_credentials("test-item").unwrap();
        assert_eq!(creds.access_key_id, "AKIACUSTOM123");
        assert_eq!(creds.secret_access_key, "secretcustom456");
    }

    #[test]
    fn test_field_name_case_insensitive() {
        let config = create_test_config();

        // Create a mock that returns credentials with different case
        struct CaseInsensitiveMock;
        impl OnePasswordCli for CaseInsensitiveMock {
            fn is_available(&self) -> bool {
                true
            }
            fn execute(&self, args: &[&str]) -> std::io::Result<std::process::Output> {
                if args.contains(&"--format") && args.contains(&"json") {
                    // Use uppercase labels
                    let json = r#"{
                        "fields": [
                            {"label": "ACCESS_KEY_ID", "value": "AKIAUPPERCASE"},
                            {"label": "SECRET_ACCESS_KEY", "value": "secretupper"}
                        ]
                    }"#;
                    return Ok(std::process::Output {
                        status: ExitStatus::from_raw(0),
                        stdout: json.as_bytes().to_vec(),
                        stderr: vec![],
                    });
                }
                Ok(std::process::Output {
                    status: ExitStatus::from_raw(256),
                    stdout: vec![],
                    stderr: b"not found".to_vec(),
                })
            }
        }

        let manager = OnePasswordManager::with_cli(config, CaseInsensitiveMock);
        let creds = manager.get_aws_credentials("test-item").unwrap();
        assert_eq!(creds.access_key_id, "AKIAUPPERCASE");
        assert_eq!(creds.secret_access_key, "secretupper");
    }

    // =========================================================================
    // MfaProvider trait implementation tests
    // =========================================================================

    #[tokio::test]
    async fn test_mfa_provider_get_token_success() {
        let config = create_test_config();
        let mock_cli = MockOnePasswordCli::available().with_totp("654321");
        let manager = OnePasswordManager::with_cli(config, mock_cli);

        // Test through the MfaProvider trait
        let result = MfaProvider::get_token(&manager, "arn:aws:iam::123456789012:mfa/test").await;
        assert!(result.is_ok());
        let token = result.unwrap();
        assert_eq!(token, Some("654321".to_string()));
    }

    #[tokio::test]
    async fn test_mfa_provider_get_token_returns_none_when_cli_unavailable() {
        let config = create_test_config();
        let manager = OnePasswordManager::with_cli(config, MockOnePasswordCli::unavailable());

        let result = MfaProvider::get_token(&manager, "arn:aws:iam::123456789012:mfa/test").await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_mfa_provider_get_token_fails_when_item_has_no_totp() {
        let config = create_test_config();
        let mock_cli = MockOnePasswordCli::available(); // No TOTP configured
        let manager = OnePasswordManager::with_cli(config, mock_cli);

        // The CLI works but the item is unusable: a provider error the user
        // must act on, not a silent fallback to manual entry.
        let result = MfaProvider::get_token(&manager, "arn:aws:iam::123456789012:mfa/test").await;
        assert!(
            matches!(result, Err(MfaError::TokenNotFound(_))),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn test_mfa_provider_get_token_fails_when_not_signed_in() {
        let config = create_test_config();
        let mock_cli = MockOnePasswordCli::available()
            .with_error("[ERROR] You are not currently signed in. Please run 'op signin'");
        let manager = OnePasswordManager::with_cli(config, mock_cli);

        let result = MfaProvider::get_token(&manager, "arn:aws:iam::123456789012:mfa/test").await;
        assert!(
            matches!(result, Err(MfaError::ProviderError(_))),
            "{result:?}"
        );
    }

    #[test]
    fn test_vault_flag_added_to_credentials_fetch_when_configured() {
        let mut config = create_test_config();
        config.vault = Some("Agent".to_string());
        let cli = RecordingCli::new();
        let recorder = cli.clone();
        let manager = OnePasswordManager::with_cli(config, cli);

        manager.get_aws_credentials("test-item");

        let calls = recorder.recorded_calls();
        assert_eq!(
            calls[0],
            vec![
                "item",
                "get",
                "test-item",
                "--format",
                "json",
                "--vault",
                "Agent"
            ]
        );
    }

    #[test]
    fn test_vault_flag_absent_from_credentials_fetch_when_not_configured() {
        let config = create_test_config();
        let cli = RecordingCli::new();
        let recorder = cli.clone();
        let manager = OnePasswordManager::with_cli(config, cli);

        manager.get_aws_credentials("test-item");

        let calls = recorder.recorded_calls();
        assert_eq!(
            calls[0],
            vec!["item", "get", "test-item", "--format", "json"]
        );
    }

    #[test]
    fn test_vault_flag_added_to_otp_fetch_when_configured() {
        let mut config = create_test_config();
        config.vault = Some("Agent".to_string());
        let cli = RecordingCli::new();
        let recorder = cli.clone();
        let manager = OnePasswordManager::with_cli(config, cli);

        // Resolves to default item "aws-default"; OTP value is irrelevant here.
        let _ = manager.get_mfa_token("arn:aws:iam::123456789012:mfa/test");

        let calls = recorder.recorded_calls();
        assert_eq!(
            calls[0],
            vec!["item", "get", "aws-default", "--otp", "--vault", "Agent"]
        );
        assert_eq!(
            calls[1],
            vec![
                "item",
                "get",
                "aws-default",
                "--format",
                "json",
                "--vault",
                "Agent"
            ]
        );
    }
}
