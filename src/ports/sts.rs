//! STS Operations trait
//!
//! Defines the interface for AWS STS operations.

use async_trait::async_trait;
use chrono::{DateTime, Utc};

/// Error type for STS operations
#[derive(Debug, Clone, thiserror::Error)]
pub enum StsError {
    #[error("Invalid credentials")]
    InvalidCredentials,

    #[error("Access denied: {0}")]
    AccessDenied(String),

    #[error("MFA required")]
    MfaRequired,

    #[error("Invalid MFA token")]
    InvalidMfaToken,

    #[error("Role not found: {0}")]
    RoleNotFound(String),

    #[error("STS service error: {0}")]
    ServiceError(String),
}

/// Credentials used to sign one STS call, never the default provider.
#[derive(Clone, zeroize::Zeroize, zeroize::ZeroizeOnDrop)]
pub struct SourceCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: String,
}

impl std::fmt::Debug for SourceCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceCredentials")
            .field("access_key_id", &"****")
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct GetSessionTokenRequest {
    pub mfa_serial: String,
    pub mfa_token: String,
    pub duration_seconds: u64,
}

impl std::fmt::Debug for GetSessionTokenRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GetSessionTokenRequest")
            .field("mfa_serial", &self.mfa_serial)
            .field("mfa_token", &"[REDACTED]")
            .field("duration_seconds", &self.duration_seconds)
            .finish()
    }
}

/// Request parameters for AssumeRole
#[derive(Clone)]
pub struct AssumeRoleRequest {
    pub role_arn: String,
    pub session_name: String,
    pub duration_seconds: Option<u64>,
    pub mfa_serial: Option<String>,
    pub mfa_token: Option<String>,
    pub policy_arns: Option<Vec<String>>,
    pub source_credentials: Option<SourceCredentials>,
}

impl std::fmt::Debug for AssumeRoleRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AssumeRoleRequest")
            .field("role_arn", &self.role_arn)
            .field("session_name", &self.session_name)
            .field("duration_seconds", &self.duration_seconds)
            .field("mfa_serial", &self.mfa_serial)
            .field("mfa_token", &self.mfa_token.as_ref().map(|_| "[REDACTED]"))
            .field("policy_arns", &self.policy_arns)
            .field("source_credentials", &self.source_credentials)
            .finish()
    }
}

impl AssumeRoleRequest {
    /// Create a new AssumeRoleRequest
    pub fn new(role_arn: impl Into<String>, session_name: impl Into<String>) -> Self {
        Self {
            role_arn: role_arn.into(),
            session_name: session_name.into(),
            duration_seconds: None,
            mfa_serial: None,
            mfa_token: None,
            policy_arns: None,
            source_credentials: None,
        }
    }

    /// Set duration in seconds
    #[must_use]
    pub fn with_duration(mut self, seconds: u64) -> Self {
        self.duration_seconds = Some(seconds);
        self
    }

    /// Set MFA credentials
    #[must_use]
    pub fn with_mfa(mut self, serial: impl Into<String>, token: impl Into<String>) -> Self {
        self.mfa_serial = Some(serial.into());
        self.mfa_token = Some(token.into());
        self
    }

    /// Set policy ARNs to attach
    #[must_use]
    pub fn with_policy_arns(mut self, arns: Vec<String>) -> Self {
        self.policy_arns = Some(arns);
        self
    }
}

/// Temporary credentials returned by AssumeRole or GetSessionToken
#[derive(Clone)]
pub struct StsCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: String,
    pub expiration: Option<DateTime<Utc>>,
}

impl std::fmt::Debug for StsCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StsCredentials")
            .field("access_key_id", &"[REDACTED]")
            .field("secret_access_key", &"[REDACTED]")
            .field("session_token", &"[REDACTED]")
            .field("expiration", &self.expiration)
            .finish()
    }
}

/// Trait for AWS STS operations
///
/// Implementations can interact with:
/// - Real AWS STS service
/// - Mock STS for testing
/// - LocalStack for development
///
/// # Example
///
/// ```ignore
/// struct AwsStsAdapter { client: StsClient }
///
/// #[async_trait]
/// impl StsOperations for AwsStsAdapter {
///     async fn assume_role(&self, request: AssumeRoleRequest) -> Result<StsCredentials, StsError> {
///         // Call real AWS STS
///     }
/// }
/// ```
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait StsOperations: Send + Sync {
    async fn get_session_token(
        &self,
        request: GetSessionTokenRequest,
    ) -> Result<StsCredentials, StsError>;

    /// Assume an IAM role
    ///
    /// # Arguments
    /// * `request` - The AssumeRole request parameters
    ///
    /// # Returns
    /// * `Ok(response)` - Temporary credentials
    /// * `Err(error)` - STS error
    async fn assume_role(&self, request: AssumeRoleRequest) -> Result<StsCredentials, StsError>;

    /// The keys STS calls are signed with (1Password, or the profile's own
    /// entry in the shared credentials file), read without calling STS: an
    /// IAM user profile without MFA hands them out as they are.
    async fn read_signing_keys(&self) -> Result<crate::domain::Credentials, StsError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_assume_role_request_builder() {
        let request =
            AssumeRoleRequest::new("arn:aws:iam::123456789012:role/TestRole", "test-session")
                .with_duration(3600)
                .with_mfa("arn:aws:iam::123456789012:mfa/user", "123456")
                .with_policy_arns(vec!["arn:aws:iam::aws:policy/ReadOnlyAccess".to_string()]);

        assert_eq!(request.role_arn, "arn:aws:iam::123456789012:role/TestRole");
        assert_eq!(request.session_name, "test-session");
        assert_eq!(request.duration_seconds, Some(3600));
        assert_eq!(
            request.mfa_serial,
            Some("arn:aws:iam::123456789012:mfa/user".to_string())
        );
        assert_eq!(request.mfa_token, Some("123456".to_string()));
        assert!(request.policy_arns.is_some());
    }

    #[test]
    fn test_assume_role_request_minimal() {
        let request =
            AssumeRoleRequest::new("arn:aws:iam::123456789012:role/TestRole", "test-session");

        assert!(request.duration_seconds.is_none());
        assert!(request.mfa_serial.is_none());
        assert!(request.mfa_token.is_none());
        assert!(request.policy_arns.is_none());
    }
}

#[cfg(test)]
mod source_tests {
    use super::*;
    #[test]
    fn source_credentials_debug_is_redacted_in_request() {
        let mut request = AssumeRoleRequest::new("role", "name");
        request.source_credentials = Some(SourceCredentials {
            access_key_id: "ASIASOURCE1234".into(),
            secret_access_key: "source-secret".into(),
            session_token: "source-token".into(),
        });
        let debug = format!("{request:?}");
        for sensitive in ["ASIASOURCE1234", "source-secret", "source-token"] {
            assert!(!debug.contains(sensitive));
        }
    }

    #[test]
    fn sts_requests_and_responses_debug_are_redacted() {
        let session_token_request = GetSessionTokenRequest {
            mfa_serial: "arn:aws:iam::123456789012:mfa/user".into(),
            mfa_token: "mfa-token".into(),
            duration_seconds: 900,
        };
        let assume_role_request =
            AssumeRoleRequest::new("role", "name").with_mfa("serial", "assume-mfa-token");
        let credentials = StsCredentials {
            access_key_id: "ASIA123".into(),
            secret_access_key: "sts-secret".into(),
            session_token: "sts-session-token".into(),
            expiration: None,
        };

        for debug in [
            format!("{session_token_request:?}"),
            format!("{assume_role_request:?}"),
            format!("{credentials:?}"),
        ] {
            assert!(!debug.contains("mfa-token"));
            assert!(!debug.contains("assume-mfa-token"));
            assert!(!debug.contains("sts-secret"));
            assert!(!debug.contains("sts-session-token"));
        }
    }
}
