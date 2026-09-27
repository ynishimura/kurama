//! Error mapping pure functions
//!
//! This module contains pure functions for error classification and mapping.
//! All functions are:
//! - Pure (no side effects)
//! - Deterministic (same input → same output)
//! - Easily testable without mocks

// ============================================================================
// Error Classification Types
// ============================================================================

/// Classified AWS STS error types
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StsErrorKind {
    /// MFA is required but not provided
    MfaRequired,
    /// MFA token is invalid
    InvalidMfaToken,
    /// Access is denied
    AccessDenied,
    /// Role was not found
    RoleNotFound,
    /// Credentials are invalid
    InvalidCredentials,
    /// Generic service error
    ServiceError,
}

// ============================================================================
// Pure Functions
// ============================================================================

/// Classify an error message into STS error kind
///
/// # Arguments
/// * `error_msg` - Error message to classify
///
/// # Returns
/// The classified error kind
///
/// # Example
/// ```
/// use kurama::domain::functions::error_mapping::{classify_sts_error, StsErrorKind};
///
/// assert_eq!(classify_sts_error("MFA token required"), StsErrorKind::MfaRequired);
/// assert_eq!(classify_sts_error("Invalid MFA token"), StsErrorKind::InvalidMfaToken);
/// assert_eq!(classify_sts_error("AccessDenied"), StsErrorKind::AccessDenied);
/// ```
pub fn classify_sts_error(error_msg: &str) -> StsErrorKind {
    // Check for MFA-related errors first
    if error_msg.contains("MFA") || error_msg.contains("mfa") {
        if error_msg.contains("required") {
            return StsErrorKind::MfaRequired;
        }
        if error_msg.contains("invalid") || error_msg.contains("Invalid") {
            return StsErrorKind::InvalidMfaToken;
        }
    }

    // Check for access denied
    if error_msg.contains("Access denied") || error_msg.contains("AccessDenied") {
        return StsErrorKind::AccessDenied;
    }

    // Check for role not found
    if error_msg.contains("not found") || error_msg.contains("NoSuchEntity") {
        return StsErrorKind::RoleNotFound;
    }

    // Check for invalid credentials
    if error_msg.contains("InvalidClientTokenId") || error_msg.contains("SignatureDoesNotMatch") {
        return StsErrorKind::InvalidCredentials;
    }

    // Check for expired token
    if error_msg.contains("ExpiredToken") || error_msg.contains("expired") {
        return StsErrorKind::InvalidCredentials;
    }

    StsErrorKind::ServiceError
}

#[cfg(test)]
mod tests {
    use super::*;

    mod classify_sts_error_tests {
        use super::*;

        #[test]
        fn classifies_mfa_required() {
            assert_eq!(
                classify_sts_error("MFA authentication required"),
                StsErrorKind::MfaRequired
            );
            assert_eq!(
                classify_sts_error("mfa token required"),
                StsErrorKind::MfaRequired
            );
        }

        #[test]
        fn classifies_invalid_mfa_token() {
            assert_eq!(
                classify_sts_error("Invalid MFA token"),
                StsErrorKind::InvalidMfaToken
            );
            assert_eq!(
                classify_sts_error("mfa code invalid"),
                StsErrorKind::InvalidMfaToken
            );
        }

        #[test]
        fn classifies_access_denied() {
            assert_eq!(
                classify_sts_error("AccessDenied: User is not authorized"),
                StsErrorKind::AccessDenied
            );
            assert_eq!(
                classify_sts_error("Access denied for user"),
                StsErrorKind::AccessDenied
            );
        }

        #[test]
        fn classifies_role_not_found() {
            assert_eq!(
                classify_sts_error("Role not found: TestRole"),
                StsErrorKind::RoleNotFound
            );
            assert_eq!(
                classify_sts_error("NoSuchEntity: Role does not exist"),
                StsErrorKind::RoleNotFound
            );
        }

        #[test]
        fn classifies_invalid_credentials() {
            assert_eq!(
                classify_sts_error("InvalidClientTokenId: The security token is invalid"),
                StsErrorKind::InvalidCredentials
            );
            assert_eq!(
                classify_sts_error("SignatureDoesNotMatch: Credential mismatch"),
                StsErrorKind::InvalidCredentials
            );
            assert_eq!(
                classify_sts_error("ExpiredToken: Token has expired"),
                StsErrorKind::InvalidCredentials
            );
        }

        #[test]
        fn classifies_generic_error() {
            assert_eq!(
                classify_sts_error("Something went wrong"),
                StsErrorKind::ServiceError
            );
            assert_eq!(
                classify_sts_error("Unknown error"),
                StsErrorKind::ServiceError
            );
        }
    }
}
