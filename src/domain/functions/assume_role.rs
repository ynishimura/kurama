//! AssumeRole related pure functions
//!
//! This module contains pure functions for building AssumeRole requests
//! and processing responses. All functions are:
//! - Pure (no side effects)
//! - Deterministic (same input → same output)
//! - Easily testable without mocks

use crate::domain::Profile;
use crate::domain::constants;
use crate::domain::types::SessionDuration;
use crate::ports::AssumeRoleRequest;

use super::session::{SessionNameConfig, SessionNameContext, render_session_name};
use super::validation::{extract_account_id, extract_role_name};

// ============================================================================
// Request Building Types
// ============================================================================

/// Parameters for AssumeRole request (pure data structure)
#[derive(Clone)]
pub struct AssumeRoleParams {
    pub role_arn: String,
    pub session_name: String,
    pub duration_seconds: u64,
    pub mfa_serial: Option<String>,
    pub mfa_token: Option<String>,
    pub policy_arns: Option<Vec<String>>,
}

impl std::fmt::Debug for AssumeRoleParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AssumeRoleParams")
            .field("role_arn", &self.role_arn)
            .field("session_name", &self.session_name)
            .field("duration_seconds", &self.duration_seconds)
            .field("mfa_serial", &self.mfa_serial)
            .field("mfa_token", &self.mfa_token.as_ref().map(|_| "[REDACTED]"))
            .field("policy_arns", &self.policy_arns)
            .finish()
    }
}

/// Input for building AssumeRole parameters
#[derive(Clone)]
pub struct AssumeRoleInput<'a> {
    pub profile: &'a Profile,
    pub mfa_token: Option<String>,
    pub readonly: bool,
    pub session_name_config: SessionNameConfig,
}

impl std::fmt::Debug for AssumeRoleInput<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AssumeRoleInput")
            .field("profile", &self.profile)
            .field("mfa_token", &self.mfa_token.as_ref().map(|_| "[REDACTED]"))
            .field("readonly", &self.readonly)
            .field("session_name_config", &self.session_name_config)
            .finish()
    }
}

// ============================================================================
// Pure Functions
// ============================================================================

/// Build AssumeRole parameters from profile and options
///
/// This is a pure function that transforms profile data into request parameters.
///
/// # Arguments
/// * `input` - Input containing profile, MFA token, readonly flag, and session name config
///
/// # Returns
/// * `Ok(AssumeRoleParams)` - Parameters ready for STS call
/// * `Err(String)` - Error message if required fields are missing
///
/// # Example
/// ```ignore
/// let params = build_assume_role_params(&AssumeRoleInput {
///     profile: &profile,
///     mfa_token: Some("123456".to_string()),
///     readonly: false,
///     session_name_config: SessionNameConfig::default(),
/// })?;
/// ```
pub fn build_assume_role_params(input: &AssumeRoleInput) -> Result<AssumeRoleParams, String> {
    let profile = input.profile;

    // Extract role_arn (required)
    let role_arn = profile
        .role_arn_raw()
        .ok_or_else(|| "Role ARN not found in profile".to_string())?
        .to_string();

    // Render session name from the configured template (pure domain function)
    let ctx = SessionNameContext {
        profile_name: profile.name(),
        role_name: extract_role_name(&role_arn),
        account_id: extract_account_id(&role_arn),
    };
    let session_name = render_session_name(&input.session_name_config, &ctx, input.readonly)?;

    // Get duration with default
    let duration_seconds = profile
        .duration_seconds()
        .unwrap_or_else(|| SessionDuration::default().as_secs());

    // Build MFA parameters
    let (mfa_serial, mfa_token) = build_mfa_params(profile, input.mfa_token.clone())?;

    // Build policy ARNs for readonly mode
    let policy_arns = build_policy_arns(input.readonly);

    Ok(AssumeRoleParams {
        role_arn,
        session_name,
        duration_seconds,
        mfa_serial,
        mfa_token,
        policy_arns,
    })
}

/// Build MFA parameters from profile and token
///
/// # Returns
/// * `Ok((Some(serial), Some(token)))` - MFA is required and token provided
/// * `Ok((None, None))` - MFA not required
/// * `Err(message)` - Token provided but no serial, or serial exists but no token
fn build_mfa_params(
    profile: &Profile,
    mfa_token: Option<String>,
) -> Result<(Option<String>, Option<String>), String> {
    match (profile.mfa_serial_raw(), mfa_token) {
        // MFA serial exists and token provided
        (Some(serial), Some(token)) => Ok((Some(serial.to_string()), Some(token))),

        // No MFA required
        (None, None) => Ok((None, None)),

        // Token provided but no serial configured
        (None, Some(_)) => Err("MFA token provided but no MFA serial in profile".to_string()),

        // Serial exists but no token - this is OK, caller may want to prompt
        (Some(_), None) => Ok((None, None)),
    }
}

/// Build policy ARNs for readonly mode
///
/// Returns readonly policy ARN if readonly is true, None otherwise.
fn build_policy_arns(readonly: bool) -> Option<Vec<String>> {
    if readonly {
        Some(vec![constants::DEFAULT_READONLY_POLICY_ARN.to_string()])
    } else {
        None
    }
}

// Note: `requires_mfa` and `can_assume_role` are available as methods on `Profile`.
// Use `profile.requires_mfa()` and `profile.can_assume_role()` directly.

// ============================================================================
// Type Conversion Functions
// ============================================================================

/// Convert AssumeRoleParams to AssumeRoleRequest (for ports layer)
///
/// This is a pure function that converts domain parameters to the port request type.
impl AssumeRoleParams {
    /// Convert to AssumeRoleRequest for the STS port
    pub fn to_request(&self) -> AssumeRoleRequest {
        let mut request = AssumeRoleRequest::new(&self.role_arn, &self.session_name)
            .with_duration(self.duration_seconds);

        if let (Some(serial), Some(token)) = (&self.mfa_serial, &self.mfa_token) {
            request = request.with_mfa(serial, token);
        }

        if let Some(arns) = &self.policy_arns {
            request = request.with_policy_arns(arns.clone());
        }

        request
    }
}

impl From<AssumeRoleParams> for AssumeRoleRequest {
    fn from(params: AssumeRoleParams) -> Self {
        params.to_request()
    }
}

// ============================================================================
// Response Conversion Functions
// ============================================================================

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_profile(name: &str) -> Profile {
        Profile::new(name)
    }

    fn create_role_profile(name: &str, role_arn: &str) -> Profile {
        let mut profile = Profile::new(name);
        profile.set_role_arn(role_arn);
        profile
    }

    fn create_mfa_profile(name: &str, role_arn: &str, mfa_serial: &str) -> Profile {
        let mut profile = Profile::new(name);
        profile.set_role_arn(role_arn);
        profile.set_mfa_serial(mfa_serial);
        profile
    }

    #[test]
    fn assume_role_inputs_debug_are_redacted() {
        let profile = create_role_profile("test", "arn:aws:iam::123456789012:role/TestRole");
        let input = AssumeRoleInput {
            profile: &profile,
            mfa_token: Some("mfa-token".into()),
            readonly: false,
            session_name_config: SessionNameConfig::default(),
        };
        let params = AssumeRoleParams {
            role_arn: "role".into(),
            session_name: "session".into(),
            duration_seconds: 900,
            mfa_serial: Some("serial".into()),
            mfa_token: Some("params-mfa-token".into()),
            policy_arns: None,
        };

        assert!(!format!("{input:?}").contains("mfa-token"));
        assert!(!format!("{params:?}").contains("params-mfa-token"));
    }

    mod build_assume_role_params_tests {
        use super::*;

        #[test]
        fn builds_basic_params() {
            let profile = create_role_profile("test", "arn:aws:iam::123456789012:role/TestRole");
            let input = AssumeRoleInput {
                profile: &profile,
                mfa_token: None,
                readonly: false,
                session_name_config: SessionNameConfig::default(),
            };

            let params = build_assume_role_params(&input).unwrap();

            assert_eq!(params.role_arn, "arn:aws:iam::123456789012:role/TestRole");
            assert_eq!(params.session_name, "kurama-test");
            assert_eq!(
                params.duration_seconds,
                SessionDuration::default().as_secs()
            );
            assert!(params.mfa_serial.is_none());
            assert!(params.mfa_token.is_none());
            assert!(params.policy_arns.is_none());
        }

        #[test]
        fn builds_readonly_params() {
            let profile = create_role_profile("prod", "arn:aws:iam::123456789012:role/ProdRole");
            let input = AssumeRoleInput {
                profile: &profile,
                mfa_token: None,
                readonly: true,
                session_name_config: SessionNameConfig::default(),
            };

            let params = build_assume_role_params(&input).unwrap();

            assert_eq!(params.session_name, "kurama-ro-prod");
            assert!(params.policy_arns.is_some());
            let arns = params.policy_arns.unwrap();
            assert!(arns.contains(&constants::DEFAULT_READONLY_POLICY_ARN.to_string()));
        }

        #[test]
        fn builds_mfa_params() {
            let profile = create_mfa_profile(
                "secure",
                "arn:aws:iam::123456789012:role/SecureRole",
                "arn:aws:iam::123456789012:mfa/user",
            );
            let input = AssumeRoleInput {
                profile: &profile,
                mfa_token: Some("123456".to_string()),
                readonly: false,
                session_name_config: SessionNameConfig::default(),
            };

            let params = build_assume_role_params(&input).unwrap();

            assert_eq!(
                params.mfa_serial,
                Some("arn:aws:iam::123456789012:mfa/user".to_string())
            );
            assert_eq!(params.mfa_token, Some("123456".to_string()));
        }

        #[test]
        fn fails_without_role_arn() {
            let profile = create_test_profile("no-role");
            let input = AssumeRoleInput {
                profile: &profile,
                mfa_token: None,
                readonly: false,
                session_name_config: SessionNameConfig::default(),
            };

            let result = build_assume_role_params(&input);

            assert!(result.is_err());
            assert!(result.unwrap_err().contains("Role ARN not found"));
        }

        #[test]
        fn fails_with_token_but_no_serial() {
            let profile = create_role_profile("test", "arn:aws:iam::123456789012:role/TestRole");
            let input = AssumeRoleInput {
                profile: &profile,
                mfa_token: Some("123456".to_string()),
                readonly: false,
                session_name_config: SessionNameConfig::default(),
            };

            let result = build_assume_role_params(&input);

            assert!(result.is_err());
            assert!(result.unwrap_err().contains("no MFA serial"));
        }

        #[test]
        fn succeeds_with_serial_but_no_token() {
            // MFA serial exists but no token provided - this is valid
            // (caller may prompt for token later)
            let profile = create_mfa_profile(
                "test",
                "arn:aws:iam::123456789012:role/TestRole",
                "arn:aws:iam::123456789012:mfa/user",
            );
            let input = AssumeRoleInput {
                profile: &profile,
                mfa_token: None,
                readonly: false,
                session_name_config: SessionNameConfig::default(),
            };

            let result = build_assume_role_params(&input);

            assert!(result.is_ok());
            let params = result.unwrap();
            // MFA params should be None since no token was provided
            assert!(params.mfa_serial.is_none());
            assert!(params.mfa_token.is_none());
        }

        #[test]
        fn uses_custom_duration() {
            let mut profile =
                create_role_profile("test", "arn:aws:iam::123456789012:role/TestRole");
            profile.set_duration_seconds(7200); // 2 hours

            let input = AssumeRoleInput {
                profile: &profile,
                mfa_token: None,
                readonly: false,
                session_name_config: SessionNameConfig::default(),
            };

            let params = build_assume_role_params(&input).unwrap();

            assert_eq!(params.duration_seconds, 7200);
        }

        #[test]
        fn uses_default_duration_when_not_set() {
            let profile = create_role_profile("test", "arn:aws:iam::123456789012:role/TestRole");
            let input = AssumeRoleInput {
                profile: &profile,
                mfa_token: None,
                readonly: false,
                session_name_config: SessionNameConfig::default(),
            };

            let params = build_assume_role_params(&input).unwrap();

            // Default is 3600 seconds (1 hour)
            assert_eq!(
                params.duration_seconds,
                SessionDuration::default().as_secs()
            );
        }

        #[test]
        fn uses_custom_session_name_template() {
            let profile = create_role_profile("test", "arn:aws:iam::123456789012:role/TestRole");
            let input = AssumeRoleInput {
                profile: &profile,
                mfa_token: None,
                readonly: false,
                session_name_config: SessionNameConfig::new().with_template("claude-{profile}"),
            };

            let params = build_assume_role_params(&input).unwrap();

            assert_eq!(params.session_name, "claude-test");
        }

        #[test]
        fn role_and_account_placeholders_use_role_arn() {
            let profile = create_role_profile("test", "arn:aws:iam::123456789012:role/TestRole");
            let input = AssumeRoleInput {
                profile: &profile,
                mfa_token: None,
                readonly: false,
                session_name_config: SessionNameConfig::new().with_template("{role}-{account}"),
            };

            let params = build_assume_role_params(&input).unwrap();

            assert_eq!(params.session_name, "TestRole-123456789012");
        }

        #[test]
        fn fails_when_rendered_session_name_is_too_short() {
            let profile = create_role_profile("test", "arn:aws:iam::123456789012:role/TestRole");
            let input = AssumeRoleInput {
                profile: &profile,
                mfa_token: None,
                readonly: false,
                session_name_config: SessionNameConfig::new().with_template("{readonly}"),
            };

            let result = build_assume_role_params(&input);

            assert!(result.is_err());
        }
    }

    // Note: Tests for `requires_mfa` and `can_assume_role` are in
    // src/domain/types/profile.rs since they are Profile methods.

    mod params_to_request_tests {
        use super::*;

        #[test]
        fn converts_basic_params_to_request() {
            let params = AssumeRoleParams {
                role_arn: "arn:aws:iam::123456789012:role/TestRole".to_string(),
                session_name: "kurama-test".to_string(),
                duration_seconds: 3600,
                mfa_serial: None,
                mfa_token: None,
                policy_arns: None,
            };

            let request = params.to_request();

            assert_eq!(request.role_arn, "arn:aws:iam::123456789012:role/TestRole");
            assert_eq!(request.session_name, "kurama-test");
            assert_eq!(request.duration_seconds, Some(3600));
            assert!(request.mfa_serial.is_none());
            assert!(request.mfa_token.is_none());
            assert!(request.policy_arns.is_none());
        }

        #[test]
        fn converts_params_with_mfa() {
            let params = AssumeRoleParams {
                role_arn: "arn:aws:iam::123456789012:role/TestRole".to_string(),
                session_name: "kurama-test".to_string(),
                duration_seconds: 3600,
                mfa_serial: Some("arn:aws:iam::123456789012:mfa/user".to_string()),
                mfa_token: Some("123456".to_string()),
                policy_arns: None,
            };

            let request = params.to_request();

            assert_eq!(
                request.mfa_serial,
                Some("arn:aws:iam::123456789012:mfa/user".to_string())
            );
            assert_eq!(request.mfa_token, Some("123456".to_string()));
        }

        #[test]
        fn converts_params_with_policy_arns() {
            let params = AssumeRoleParams {
                role_arn: "arn:aws:iam::123456789012:role/TestRole".to_string(),
                session_name: "kurama-ro-test".to_string(),
                duration_seconds: 3600,
                mfa_serial: None,
                mfa_token: None,
                policy_arns: Some(vec!["arn:aws:iam::aws:policy/ReadOnlyAccess".to_string()]),
            };

            let request = params.to_request();

            assert!(request.policy_arns.is_some());
            let arns = request.policy_arns.unwrap();
            assert_eq!(arns.len(), 1);
            assert!(arns.contains(&"arn:aws:iam::aws:policy/ReadOnlyAccess".to_string()));
        }

        #[test]
        fn from_trait_works() {
            let params = AssumeRoleParams {
                role_arn: "arn:aws:iam::123456789012:role/TestRole".to_string(),
                session_name: "kurama-test".to_string(),
                duration_seconds: 7200,
                mfa_serial: None,
                mfa_token: None,
                policy_arns: None,
            };

            let request: AssumeRoleRequest = params.into();

            assert_eq!(request.role_arn, "arn:aws:iam::123456789012:role/TestRole");
            assert_eq!(request.duration_seconds, Some(7200));
        }
    }
}
