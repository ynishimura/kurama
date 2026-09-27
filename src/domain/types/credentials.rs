//! Credentials value object
//!
//! This module defines AWS credentials as a value object with secure handling.

use chrono::{DateTime, Utc};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// AWS Credentials value object
///
/// Implements secure handling of sensitive data:
/// - Secret access key is zeroized on drop
/// - Session token is zeroized on drop
///
/// # Example
///
/// ```
/// use kurama::domain::types::Credentials;
///
/// let creds = Credentials::new(
///     "AKIAIOSFODNN7EXAMPLE".to_string(),
///     "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string(),
///     Some("session_token".to_string()),
///     None,
/// );
///
/// assert_eq!(creds.access_key_id(), "AKIAIOSFODNN7EXAMPLE");
/// ```
#[derive(Clone, PartialEq, Zeroize, ZeroizeOnDrop)]
pub struct Credentials {
    /// AWS Access Key ID
    #[zeroize(skip)]
    access_key_id: String,

    /// AWS Secret Access Key (sensitive - zeroized on drop)
    secret_access_key: String,

    /// Session Token for temporary credentials (sensitive - zeroized on drop)
    session_token: Option<String>,

    /// Expiration time for temporary credentials
    #[zeroize(skip)]
    expiration: Option<DateTime<Utc>>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("access_key_id", &self.masked_access_key_id())
            .field("secret_access_key", &"[REDACTED]")
            .field(
                "session_token",
                &self.session_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("expiration", &self.expiration)
            .finish()
    }
}

impl Credentials {
    /// Create new credentials with minimal required fields
    pub fn new(
        access_key_id: String,
        secret_access_key: String,
        session_token: Option<String>,
        expiration: Option<DateTime<Utc>>,
    ) -> Self {
        Self {
            access_key_id,
            secret_access_key,
            session_token,
            expiration,
        }
    }

    // =========================================================================
    // Accessors (pure functions)
    // =========================================================================

    /// Get the access key ID
    pub fn access_key_id(&self) -> &str {
        &self.access_key_id
    }

    /// Get the secret access key
    pub fn secret_access_key(&self) -> &str {
        &self.secret_access_key
    }

    /// Get the session token if present
    pub fn session_token(&self) -> Option<&str> {
        self.session_token.as_deref()
    }

    /// Get the expiration time if present
    pub fn expiration(&self) -> Option<DateTime<Utc>> {
        self.expiration
    }

    // =========================================================================
    // Pure functions
    // =========================================================================

    /// Check if credentials are expired at `now` (the caller reads the clock)
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.expiration.map(|exp| exp < now).unwrap_or(false)
    }

    /// Check if credentials will expire within `duration` after `now`
    pub fn expires_within(&self, now: DateTime<Utc>, duration: chrono::Duration) -> bool {
        self.expiration
            .map(|exp| exp < now + duration)
            .unwrap_or(false)
    }

    /// Get masked access key ID for logging (shows first 4 chars)
    pub fn masked_access_key_id(&self) -> String {
        if self.access_key_id.len() > 4 {
            format!("{}...", &self.access_key_id[..4])
        } else {
            "****".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_credentials_new() {
        let expiration = Utc::now() + chrono::Duration::hours(1);
        let creds = Credentials::new(
            "ASIAIOSFODNN7EXAMPLE".to_string(),
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string(),
            Some("session_token".to_string()),
            Some(expiration),
        );

        assert_eq!(creds.access_key_id(), "ASIAIOSFODNN7EXAMPLE");
        assert_eq!(
            creds.secret_access_key(),
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"
        );
        assert_eq!(creds.session_token(), Some("session_token"));
        assert!(creds.expiration().is_some());
    }

    #[test]
    fn debug_does_not_expose_secret_material() {
        let creds = Credentials::new(
            "ASIAIOSFODNN7EXAMPLE".to_string(),
            "secret-access-key".to_string(),
            Some("session-token".to_string()),
            None,
        );

        let debug = format!("{creds:?}");

        assert!(debug.contains("ASIA..."));
        assert!(!debug.contains("secret-access-key"));
        assert!(!debug.contains("session-token"));
    }

    #[test]
    fn test_is_expired() {
        let now = Utc::now();

        // Not expired
        let future_exp = now + chrono::Duration::hours(1);
        let creds = Credentials::new(
            "AKIA123".to_string(),
            "secret".to_string(),
            None,
            Some(future_exp),
        );
        assert!(!creds.is_expired(now));

        // Expired
        let past_exp = now - chrono::Duration::hours(1);
        let creds = Credentials::new(
            "AKIA123".to_string(),
            "secret".to_string(),
            None,
            Some(past_exp),
        );
        assert!(creds.is_expired(now));

        // No expiration (permanent)
        let creds = Credentials::new("AKIA123".to_string(), "secret".to_string(), None, None);
        assert!(!creds.is_expired(now));
    }

    #[test]
    fn test_expires_within() {
        let now = Utc::now();
        let exp = now + chrono::Duration::minutes(30);
        let creds = Credentials::new("AKIA123".to_string(), "secret".to_string(), None, Some(exp));

        assert!(creds.expires_within(now, chrono::Duration::hours(1)));
        assert!(!creds.expires_within(now, chrono::Duration::minutes(15)));
    }

    #[test]
    fn test_masked_access_key_id() {
        let creds = Credentials::new(
            "AKIAIOSFODNN7EXAMPLE".to_string(),
            "secret".to_string(),
            None,
            None,
        );
        assert_eq!(creds.masked_access_key_id(), "AKIA...");

        let creds = Credentials::new("AK".to_string(), "secret".to_string(), None, None);
        assert_eq!(creds.masked_access_key_id(), "****");
    }
}
