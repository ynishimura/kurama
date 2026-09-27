//! MFA Provider trait
//!
//! Defines the interface for MFA token retrieval.

use async_trait::async_trait;

/// Error type for MFA operations
#[derive(Debug, Clone, thiserror::Error)]
pub enum MfaError {
    #[error("MFA token not found for serial: {0}")]
    TokenNotFound(String),

    #[error("MFA provider error: {0}")]
    ProviderError(String),
}

/// Trait for MFA token providers
///
/// Implementations can retrieve MFA tokens from various sources:
/// - 1Password
/// - Manual input
/// - Other password managers
///
/// # Example
///
/// ```ignore
/// struct OnePasswordMfaProvider;
///
/// #[async_trait]
/// impl MfaProvider for OnePasswordMfaProvider {
///     async fn get_token(&self, mfa_serial: &str) -> Result<Option<String>, MfaError> {
///         // Retrieve from 1Password
///     }
/// }
/// ```
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait MfaProvider: Send + Sync {
    /// Get MFA token for the given MFA serial
    ///
    /// # Arguments
    /// * `mfa_serial` - The MFA device serial (ARN)
    ///
    /// # Returns
    /// * `Ok(Some(token))` - Token retrieved successfully
    /// * `Ok(None)` - Token not available (e.g., not configured)
    /// * `Err(error)` - Error during retrieval
    async fn get_token(&self, mfa_serial: &str) -> Result<Option<String>, MfaError>;
}

/// Manual MFA input provider (reads from stdin)
///
/// This is a fallback provider that prompts the user for manual input.
pub struct ManualMfaProvider;

#[async_trait]
impl MfaProvider for ManualMfaProvider {
    async fn get_token(&self, _mfa_serial: &str) -> Result<Option<String>, MfaError> {
        // Note: Actual stdin reading should be done at the shell layer
        // This provider just indicates manual input is needed
        Ok(None)
    }
}
