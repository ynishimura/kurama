//! Profile value object
//!
//! This module defines the AWS profile configuration as a type-safe value object.
//!
//! ## Design Notes
//!
//! The Profile struct keeps raw String fields for serialization
//! compatibility with AWS config files.

use crate::domain::functions::validation::extract_account_id;
use serde::{Deserialize, Serialize};

/// How a profile turns into credentials: by assuming its `role_arn`, or --
/// with no `role_arn` -- as the IAM user its long-term keys belong to (the
/// keys themselves, or the MFA session they get with `mfa_serial`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileAuth {
    Role,
    IamUser,
}

impl ProfileAuth {
    /// The stable name `status --json` reports and errors name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Role => "role",
            Self::IamUser => "iam_user",
        }
    }
}

/// AWS profile configuration
///
/// Represents an AWS CLI profile with role assumption configuration.
///
/// # Builder Pattern
///
/// Use the `with_*()` methods for fluent construction:
///
/// ```ignore
/// let profile = Profile::new("prod")
///     .with_role_arn_raw(role_arn)
///     .with_mfa_serial_raw(mfa)
///     .with_region_raw(region);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Profile {
    /// Profile name
    name: String,

    /// IAM Role ARN to assume
    role_arn: Option<String>,

    /// Source profile for credentials
    source_profile: Option<String>,

    /// MFA device serial number
    mfa_serial: Option<String>,

    /// AWS region
    region: Option<String>,

    /// Custom role session name
    role_session_name: Option<String>,

    /// Session duration in seconds
    duration_seconds: Option<u64>,
}

impl Profile {
    /// Create a new profile with the given name
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    // =========================================================================
    // Accessors
    // =========================================================================

    /// Get the profile name
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the raw role ARN string
    pub fn role_arn_raw(&self) -> Option<&str> {
        self.role_arn.as_deref()
    }

    /// Get the source profile name
    pub fn source_profile(&self) -> Option<&str> {
        self.source_profile.as_deref()
    }

    /// Get the raw MFA serial string
    pub fn mfa_serial_raw(&self) -> Option<&str> {
        self.mfa_serial.as_deref()
    }

    /// Get the raw region string
    pub fn region_raw(&self) -> Option<&str> {
        self.region.as_deref()
    }

    /// Get the role session name
    #[cfg(test)]
    pub fn role_session_name(&self) -> Option<&str> {
        self.role_session_name.as_deref()
    }

    /// Get the duration in seconds
    pub fn duration_seconds(&self) -> Option<u64> {
        self.duration_seconds
    }

    // =========================================================================
    // Pure predicate functions
    // =========================================================================

    /// Check if profile requires MFA
    pub fn requires_mfa(&self) -> bool {
        self.mfa_serial.is_some()
    }

    /// Check if profile can assume a role
    pub fn can_assume_role(&self) -> bool {
        self.role_arn.is_some()
    }

    /// Check if profile has a region configured
    pub fn has_region(&self) -> bool {
        self.region.is_some()
    }

    // =========================================================================
    // Pure extraction functions (delegate to domain functions)
    // =========================================================================

    /// Extract account ID from role ARN
    ///
    /// Returns `None` if no valid role ARN is set or account ID is invalid.
    /// Uses domain validation to ensure account ID is 12 digits.
    pub fn account_id(&self) -> Option<&str> {
        self.role_arn
            .as_ref()
            .and_then(|arn| extract_account_id(arn))
    }

    // =========================================================================
    // Builder pattern (FP style - returns new instance)
    // =========================================================================

    /// Set the role ARN from a raw string (for AWS config parsing)
    #[must_use]
    #[cfg(test)]
    pub fn with_role_arn_raw(mut self, role_arn: impl Into<String>) -> Self {
        self.role_arn = Some(role_arn.into());
        self
    }

    /// Set the MFA serial from a raw string (for AWS config parsing)
    #[must_use]
    #[cfg(test)]
    pub fn with_mfa_serial_raw(mut self, mfa_serial: impl Into<String>) -> Self {
        self.mfa_serial = Some(mfa_serial.into());
        self
    }

    /// Set the region from a raw string (for AWS config parsing)
    #[must_use]
    pub fn with_region_raw(mut self, region: impl Into<String>) -> Self {
        self.region = Some(region.into());
        self
    }

    /// Set the duration in seconds
    #[must_use]
    #[cfg(test)]
    pub fn with_duration_seconds(mut self, duration: u64) -> Self {
        self.duration_seconds = Some(duration);
        self
    }

    // =========================================================================
    // Mutable setters (for config parsing)
    // =========================================================================

    /// Set role ARN (mutable, for config parsing)
    pub fn set_role_arn(&mut self, role_arn: impl Into<String>) {
        self.role_arn = Some(role_arn.into());
    }

    /// Set source profile (mutable, for config parsing)
    pub fn set_source_profile(&mut self, source: impl Into<String>) {
        self.source_profile = Some(source.into());
    }

    /// Set MFA serial (mutable, for config parsing)
    pub fn set_mfa_serial(&mut self, serial: impl Into<String>) {
        self.mfa_serial = Some(serial.into());
    }

    /// Set region (mutable, for config parsing)
    pub fn set_region(&mut self, region: impl Into<String>) {
        self.region = Some(region.into());
    }

    /// Set role session name (mutable, for config parsing)
    pub fn set_role_session_name(&mut self, name: impl Into<String>) {
        self.role_session_name = Some(name.into());
    }

    /// Set duration seconds (mutable, for config parsing)
    pub fn set_duration_seconds(&mut self, duration: u64) {
        self.duration_seconds = Some(duration);
    }

    // =========================================================================
    // Applicative-style validation
    // =========================================================================
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_profile() {
        let profile = Profile::new("test");
        assert_eq!(profile.name(), "test");
        assert!(profile.role_arn_raw().is_none());
        assert!(profile.source_profile().is_none());
        assert!(profile.mfa_serial_raw().is_none());
        assert!(profile.region_raw().is_none());
    }

    #[test]
    fn test_requires_mfa() {
        let profile = Profile::new("test");
        assert!(!profile.requires_mfa());

        let profile = profile.with_mfa_serial_raw("arn:aws:iam::123456789012:mfa/user");
        assert!(profile.requires_mfa());
    }

    #[test]
    fn test_can_assume_role() {
        let profile = Profile::new("test");
        assert!(!profile.can_assume_role());

        let profile = profile.with_role_arn_raw("arn:aws:iam::123456789012:role/TestRole");
        assert!(profile.can_assume_role());
    }

    #[test]
    fn profile_auth_names_are_stable() {
        assert_eq!(ProfileAuth::IamUser.as_str(), "iam_user");
        assert_eq!(ProfileAuth::Role.as_str(), "role");
    }

    #[test]
    fn test_builder() {
        let profile = Profile::new("prod")
            .with_role_arn_raw("arn:aws:iam::123456789012:role/MyRole")
            .with_mfa_serial_raw("arn:aws:iam::123456789012:mfa/user")
            .with_region_raw("ap-northeast-1")
            .with_duration_seconds(3600);

        assert_eq!(profile.name(), "prod");
        assert!(profile.can_assume_role());
        assert!(profile.requires_mfa());
        assert_eq!(profile.region_raw(), Some("ap-northeast-1"));
        assert_eq!(profile.duration_seconds(), Some(3600));
    }

    #[test]
    fn test_account_id() {
        let profile = Profile::new("test");
        assert!(profile.account_id().is_none());

        let profile = profile.with_role_arn_raw("arn:aws:iam::123456789012:role/TestRole");
        assert_eq!(profile.account_id(), Some("123456789012"));
    }

    #[test]
    fn test_mutable_setters() {
        let mut profile = Profile::new("test");
        profile.set_role_arn("arn:aws:iam::123456789012:role/TestRole");
        profile.set_source_profile("default");
        profile.set_mfa_serial("arn:aws:iam::123456789012:mfa/user");
        profile.set_region("us-east-1");
        profile.set_duration_seconds(7200);

        assert!(profile.can_assume_role());
        assert!(profile.requires_mfa());
        assert_eq!(profile.source_profile(), Some("default"));
        assert_eq!(profile.region_raw(), Some("us-east-1"));
        assert_eq!(profile.duration_seconds(), Some(7200));
    }
}
