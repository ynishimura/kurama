//! 1Password related pure functions
//!
//! This module contains pure functions for processing 1Password data.
//! All functions are:
//! - Pure (no side effects)
//! - Deterministic (same input → same output)
//! - Easily testable without mocks

use serde::Deserialize;

// ============================================================================
// Data Types
// ============================================================================

/// 1Password item structure
#[derive(Clone, Deserialize)]
pub struct OpItem {
    pub fields: Vec<OpField>,
}

/// 1Password field structure
///
/// A one-time password field has `type = "OTP"`, no label, the
/// `otpauth://` URI as its `value` and the current code in `totp`.
#[derive(Clone, Default, Deserialize)]
pub struct OpField {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default, rename = "type")]
    pub field_type: Option<String>,
    #[serde(default)]
    pub totp: Option<String>,
}

impl std::fmt::Debug for OpItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpItem")
            .field("fields", &self.fields)
            .finish()
    }
}

impl std::fmt::Debug for OpField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpField")
            .field("label", &self.label)
            .field("value", &self.value.as_ref().map(|_| "[REDACTED]"))
            .field("type", &self.field_type)
            .field("totp", &self.totp.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

/// Classified 1Password error
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnePasswordErrorKind {
    /// User is not signed in
    NotSignedIn,
    /// Session has expired
    SessionExpired,
    /// A service account requires an explicit vault
    VaultRequired,
    /// Item was not found
    ItemNotFound,
    /// Generic/unknown error
    Other,
}

/// Output mode for `op item get`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemGetOutput {
    /// Full item as JSON (`--format json`)
    Json,
    /// One-time password only (`--otp`)
    Otp,
}

// ============================================================================
// Pure Functions
// ============================================================================

/// Find field value by label (case-insensitive)
///
/// # Arguments
/// * `fields` - List of 1Password fields
/// * `label` - Label to search for
///
/// # Returns
/// The field value if found, None otherwise
///
/// # Example
/// ```
/// use kurama::domain::functions::onepassword::{OpField, find_field_value};
///
/// let fields = vec![
///     OpField {
///         label: Some("Access_Key_ID".to_string()),
///         value: Some("AKIATEST".to_string()),
///         ..Default::default()
///     },
/// ];
/// assert_eq!(find_field_value(&fields, "access_key_id"), Some("AKIATEST".to_string()));
/// ```
pub fn find_field_value(fields: &[OpField], label: &str) -> Option<String> {
    let label_lower = label.to_lowercase();
    fields.iter().find_map(|field| {
        let field_label = field.label.as_ref()?.to_lowercase();
        if field_label == label_lower {
            field.value.clone()
        } else {
            None
        }
    })
}

/// Check if a value is a valid OTP (6 digits)
///
/// # Arguments
/// * `value` - Value to check
///
/// # Returns
/// True if the value is a valid 6-digit OTP
///
/// # Example
/// ```
/// use kurama::domain::functions::onepassword::is_valid_otp;
///
/// assert!(is_valid_otp("123456"));
/// assert!(!is_valid_otp("12345"));  // Too short
/// assert!(!is_valid_otp("1234567")); // Too long
/// assert!(!is_valid_otp("12345a")); // Contains non-digit
/// ```
pub fn is_valid_otp(value: &str) -> bool {
    value.len() == 6 && value.chars().all(|c| c.is_ascii_digit())
}

/// Check if a field label indicates an OTP field
///
/// Checks if the label contains "otp", "totp", or "mfa" (case-insensitive)
///
/// # Arguments
/// * `label` - Field label to check
///
/// # Returns
/// True if the label indicates an OTP field
///
/// # Example
/// ```
/// use kurama::domain::functions::onepassword::is_otp_field_label;
///
/// assert!(is_otp_field_label("OTP"));
/// assert!(is_otp_field_label("One-Time Password (TOTP)"));
/// assert!(is_otp_field_label("MFA Token"));
/// assert!(!is_otp_field_label("Username"));
/// ```
pub fn is_otp_field_label(label: &str) -> bool {
    let label_lower = label.to_lowercase();
    label_lower.contains("otp") || label_lower.contains("totp") || label_lower.contains("mfa")
}

/// Find OTP value from fields
///
/// Returns the first 6-digit code of either:
/// 1. a field of type `OTP` (1Password's one-time password), from its `totp`
/// 2. a field whose label contains "otp", "totp", or "mfa", from its `value`
///    (a code typed into a field of another type)
///
/// # Arguments
/// * `fields` - List of 1Password fields
///
/// # Returns
/// The OTP value if found, None otherwise
pub fn find_otp_value(fields: &[OpField]) -> Option<String> {
    fields.iter().find_map(|field| {
        let code = if field.field_type.as_deref() == Some("OTP") {
            field.totp.as_ref()
        } else {
            field
                .value
                .as_ref()
                .filter(|_| field.label.as_deref().is_some_and(is_otp_field_label))
        }?;
        is_valid_otp(code).then(|| code.clone())
    })
}

/// Classify a 1Password error message
///
/// # Arguments
/// * `error_msg` - Error message from 1Password CLI
///
/// # Returns
/// The classified error kind
///
/// # Example
/// ```
/// use kurama::domain::functions::onepassword::{classify_error, OnePasswordErrorKind};
///
/// assert_eq!(classify_error("not currently signed in"), OnePasswordErrorKind::NotSignedIn);
/// assert_eq!(classify_error("session expired"), OnePasswordErrorKind::SessionExpired);
/// assert_eq!(classify_error("could not find item"), OnePasswordErrorKind::ItemNotFound);
/// assert_eq!(classify_error("vault query must be provided"), OnePasswordErrorKind::VaultRequired);
/// ```
pub fn classify_error(error_msg: &str) -> OnePasswordErrorKind {
    if error_msg.contains("not currently signed in") {
        OnePasswordErrorKind::NotSignedIn
    } else if error_msg.contains("session expired") {
        OnePasswordErrorKind::SessionExpired
    } else if error_msg.contains("vault query must be provided") {
        OnePasswordErrorKind::VaultRequired
    } else if error_msg.contains("could not find item") || error_msg.contains("not found") {
        OnePasswordErrorKind::ItemNotFound
    } else {
        OnePasswordErrorKind::Other
    }
}

/// Build a user-friendly error message based on error kind
///
/// # Arguments
/// * `kind` - The classified error kind
/// * `item_name` - The item name that was being accessed
/// * `original_error` - The original error message
///
/// # Returns
/// One line naming the cause: what to do about it belongs in the `hint:`
/// line, which `ErrorCode` adds. A failure is one greppable line.
pub fn build_error_message(
    kind: &OnePasswordErrorKind,
    item_name: &str,
    original_error: &str,
) -> String {
    match kind {
        OnePasswordErrorKind::NotSignedIn => {
            format!("You are not signed in to 1Password: {original_error}")
        }
        OnePasswordErrorKind::SessionExpired => {
            format!("Your 1Password session has expired: {original_error}")
        }
        OnePasswordErrorKind::ItemNotFound => {
            format!("1Password item '{item_name}' not found: {original_error}")
        }
        OnePasswordErrorKind::VaultRequired => {
            format!(
                "1Password service accounts require an explicit vault: set \
                 `vault = \"<name>\"` under [onepassword] in config.toml: {original_error}"
            )
        }
        OnePasswordErrorKind::Other => {
            format!("1Password CLI error on item '{item_name}': {original_error}")
        }
    }
}

/// Parse 1Password JSON response into OpItem
///
/// # Arguments
/// * `json_bytes` - JSON bytes from 1Password CLI
///
/// # Returns
/// Parsed OpItem or error message
pub fn parse_op_item(json_bytes: &[u8]) -> Result<OpItem, String> {
    serde_json::from_slice(json_bytes)
        .map_err(|e| format!("Failed to parse 1Password JSON response: {}", e))
}

/// Build arguments for `op item get`
///
/// Appends `--vault <vault>` when a vault is given. Service accounts
/// require an explicit vault; for user accounts it narrows the search
/// scope.
///
/// # Example
/// ```
/// use kurama::domain::functions::onepassword::{build_item_get_args, ItemGetOutput};
///
/// let args = build_item_get_args("aws_cm", Some("Agent"), ItemGetOutput::Otp);
/// assert_eq!(args, vec!["item", "get", "aws_cm", "--otp", "--vault", "Agent"]);
/// ```
pub fn build_item_get_args(
    item_name: &str,
    vault: Option<&str>,
    output: ItemGetOutput,
) -> Vec<String> {
    let mut args = vec!["item".to_string(), "get".to_string(), item_name.to_string()];
    match output {
        ItemGetOutput::Json => {
            args.push("--format".to_string());
            args.push("json".to_string());
        }
        ItemGetOutput::Otp => args.push("--otp".to_string()),
    }
    if let Some(vault) = vault {
        args.push("--vault".to_string());
        args.push(vault.to_string());
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    mod find_field_value_tests {
        use super::*;

        #[test]
        fn finds_exact_match() {
            let fields = vec![OpField {
                label: Some("access_key_id".to_string()),
                value: Some("AKIATEST".to_string()),
                ..Default::default()
            }];
            assert_eq!(
                find_field_value(&fields, "access_key_id"),
                Some("AKIATEST".to_string())
            );
        }

        #[test]
        fn finds_case_insensitive_match() {
            let fields = vec![OpField {
                label: Some("ACCESS_KEY_ID".to_string()),
                value: Some("AKIATEST".to_string()),
                ..Default::default()
            }];
            assert_eq!(
                find_field_value(&fields, "access_key_id"),
                Some("AKIATEST".to_string())
            );
        }

        #[test]
        fn returns_none_when_not_found() {
            let fields = vec![OpField {
                label: Some("username".to_string()),
                value: Some("test".to_string()),
                ..Default::default()
            }];
            assert_eq!(find_field_value(&fields, "access_key_id"), None);
        }

        #[test]
        fn handles_empty_fields() {
            let fields: Vec<OpField> = vec![];
            assert_eq!(find_field_value(&fields, "access_key_id"), None);
        }

        #[test]
        fn handles_none_label() {
            let fields = vec![OpField {
                label: None,
                value: Some("AKIATEST".to_string()),
                ..Default::default()
            }];
            assert_eq!(find_field_value(&fields, "access_key_id"), None);
        }
    }

    mod is_valid_otp_tests {
        use super::*;

        #[test]
        fn valid_6_digit_otp() {
            assert!(is_valid_otp("123456"));
            assert!(is_valid_otp("000000"));
            assert!(is_valid_otp("999999"));
        }

        #[test]
        fn invalid_length() {
            assert!(!is_valid_otp("12345"));
            assert!(!is_valid_otp("1234567"));
            assert!(!is_valid_otp(""));
        }

        #[test]
        fn invalid_characters() {
            assert!(!is_valid_otp("12345a"));
            assert!(!is_valid_otp("abcdef"));
            assert!(!is_valid_otp("12 456"));
        }
    }

    mod is_otp_field_label_tests {
        use super::*;

        #[test]
        fn detects_otp_labels() {
            assert!(is_otp_field_label("OTP"));
            assert!(is_otp_field_label("otp"));
            assert!(is_otp_field_label("One-Time Password (OTP)"));
        }

        #[test]
        fn detects_totp_labels() {
            assert!(is_otp_field_label("TOTP"));
            assert!(is_otp_field_label("totp"));
            assert!(is_otp_field_label("Time-based TOTP"));
        }

        #[test]
        fn detects_mfa_labels() {
            assert!(is_otp_field_label("MFA"));
            assert!(is_otp_field_label("mfa"));
            assert!(is_otp_field_label("MFA Token"));
        }

        #[test]
        fn rejects_non_otp_labels() {
            assert!(!is_otp_field_label("Username"));
            assert!(!is_otp_field_label("Password"));
            assert!(!is_otp_field_label("Access Key"));
        }
    }

    mod find_otp_value_tests {
        use super::*;

        #[test]
        fn finds_otp_field() {
            let fields = vec![
                OpField {
                    label: Some("Username".to_string()),
                    value: Some("test".to_string()),
                    ..Default::default()
                },
                OpField {
                    label: Some("OTP".to_string()),
                    value: Some("123456".to_string()),
                    ..Default::default()
                },
            ];
            assert_eq!(find_otp_value(&fields), Some("123456".to_string()));
        }

        #[test]
        fn returns_none_when_no_otp() {
            let fields = vec![OpField {
                label: Some("Username".to_string()),
                value: Some("test".to_string()),
                ..Default::default()
            }];
            assert_eq!(find_otp_value(&fields), None);
        }

        /// The shape `op item get --format json` gives a one-time password
        /// field: no label, the `otpauth://` URI as its value, and the
        /// current code in `totp`.
        const REAL_OTP_FIELD: &[u8] = br#"{"fields": [
            {"id": "username", "label": "username", "value": "agent"},
            {"id": "TOTP_abc", "section": {"id": "add more"}, "type": "OTP",
             "value": "otpauth://totp/agent?secret=ABCDEFGH234567&issuer=AWS",
             "reference": "op://Agent/aws_cm/TOTP_abc", "totp": "123456"}
        ]}"#;

        #[test]
        fn returns_the_totp_of_an_otp_typed_field_without_a_label() {
            let item = parse_op_item(REAL_OTP_FIELD).unwrap();
            assert_eq!(find_otp_value(&item.fields), Some("123456".to_string()));
        }

        #[test]
        fn ignores_an_otp_typed_field_whose_totp_is_not_six_digits() {
            let item = parse_op_item(
                br#"{"fields": [{"type": "OTP", "value": "otpauth://totp/a", "totp": "12345"}]}"#,
            )
            .unwrap();
            assert_eq!(find_otp_value(&item.fields), None);
        }

        #[test]
        fn debug_redacts_the_totp() {
            let item = parse_op_item(REAL_OTP_FIELD).unwrap();
            let debug = format!("{item:?}");
            assert!(!debug.contains("123456"));
            assert!(!debug.contains("ABCDEFGH234567"));
        }

        #[test]
        fn ignores_invalid_otp_value() {
            let fields = vec![OpField {
                label: Some("OTP".to_string()),
                value: Some("12345".to_string()), // Invalid - not 6 digits
                ..Default::default()
            }];
            assert_eq!(find_otp_value(&fields), None);
        }
    }

    mod classify_error_tests {
        use super::*;

        #[test]
        fn classifies_not_signed_in() {
            assert_eq!(
                classify_error("You are not currently signed in to 1Password"),
                OnePasswordErrorKind::NotSignedIn
            );
        }

        #[test]
        fn classifies_session_expired() {
            assert_eq!(
                classify_error("Your session expired"),
                OnePasswordErrorKind::SessionExpired
            );
        }

        #[test]
        fn classifies_item_not_found() {
            assert_eq!(
                classify_error("could not find item 'test'"),
                OnePasswordErrorKind::ItemNotFound
            );
            assert_eq!(
                classify_error("Item not found"),
                OnePasswordErrorKind::ItemNotFound
            );
        }

        #[test]
        fn classifies_other() {
            assert_eq!(
                classify_error("Unknown error occurred"),
                OnePasswordErrorKind::Other
            );
        }

        #[test]
        fn classifies_vault_required() {
            assert_eq!(
                classify_error(
                    "a vault query must be provided when this command is called by a service account"
                ),
                OnePasswordErrorKind::VaultRequired
            );
        }
    }

    mod build_error_message_tests {
        use super::*;

        #[test]
        fn vault_required_message_names_config_key() {
            let msg = build_error_message(
                &OnePasswordErrorKind::VaultRequired,
                "aws_cm",
                "a vault query must be provided",
            );
            assert!(msg.contains("[onepassword]"));
            assert!(msg.contains("vault = "));
            assert!(msg.contains("config.toml"));
            assert!(msg.contains("a vault query must be provided"));
        }
    }

    mod parse_op_item_tests {
        use super::*;

        #[test]
        fn debug_redacts_field_values() {
            let item =
                parse_op_item(br#"{"fields": [{"label": "password", "value": "secret-value"}]}"#)
                    .unwrap();
            let debug = format!("{item:?}");

            assert!(!debug.contains("secret-value"));
            assert!(debug.contains("password"));
        }

        #[test]
        fn parses_valid_json() {
            let json = br#"{"fields": [{"label": "test", "value": "value"}]}"#;
            let item = parse_op_item(json).unwrap();
            assert_eq!(item.fields.len(), 1);
            assert_eq!(item.fields[0].label, Some("test".to_string()));
        }

        #[test]
        fn handles_invalid_json() {
            let json = b"not json";
            assert!(parse_op_item(json).is_err());
        }
    }

    mod build_item_get_args_tests {
        use super::*;

        #[test]
        fn json_without_vault_has_no_vault_flag() {
            assert_eq!(
                build_item_get_args("aws_cm", None, ItemGetOutput::Json),
                vec!["item", "get", "aws_cm", "--format", "json"]
            );
        }

        #[test]
        fn otp_without_vault_has_no_vault_flag() {
            assert_eq!(
                build_item_get_args("aws_cm", None, ItemGetOutput::Otp),
                vec!["item", "get", "aws_cm", "--otp"]
            );
        }

        #[test]
        fn json_with_vault_appends_vault_flag() {
            assert_eq!(
                build_item_get_args("aws_cm", Some("Agent"), ItemGetOutput::Json),
                vec![
                    "item", "get", "aws_cm", "--format", "json", "--vault", "Agent"
                ]
            );
        }

        #[test]
        fn otp_with_vault_appends_vault_flag() {
            assert_eq!(
                build_item_get_args("aws_cm", Some("Agent"), ItemGetOutput::Otp),
                vec!["item", "get", "aws_cm", "--otp", "--vault", "Agent"]
            );
        }
    }
}
