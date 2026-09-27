//! Bearer token issued by an OAuth 2.0 authorization server, as kept in the
//! token store. Secrets are zeroized on drop and redacted in `Debug`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct OAuthToken {
    pub access_token: String,
    #[serde(default = "default_token_type")]
    #[zeroize(skip)]
    pub token_type: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Absent when the server sent no `expires_in`: the token is used until
    /// the API rejects it.
    #[serde(default)]
    #[zeroize(skip)]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    #[zeroize(skip)]
    pub scope: Option<String>,
}

fn default_token_type() -> String {
    "Bearer".to_string()
}

impl OAuthToken {
    /// A minimal token, for tests and fixtures.
    pub fn bearer(access_token: impl Into<String>) -> Self {
        Self {
            access_token: access_token.into(),
            token_type: default_token_type(),
            refresh_token: None,
            expires_at: None,
            scope: None,
        }
    }

    /// The `Authorization` header value.
    pub fn authorization_header(&self) -> String {
        let scheme = if self.token_type.eq_ignore_ascii_case("bearer") {
            "Bearer"
        } else {
            self.token_type.as_str()
        };
        format!("{scheme} {}", self.access_token)
    }
}

impl std::fmt::Debug for OAuthToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthToken")
            .field("access_token", &"[REDACTED]")
            .field("token_type", &self.token_type)
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("expires_at", &self.expires_at)
            .field("scope", &self.scope)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_both_tokens() {
        let mut token = OAuthToken::bearer("access-secret");
        token.refresh_token = Some("refresh-secret".into());
        let debug = format!("{token:?}");
        assert!(!debug.contains("access-secret"));
        assert!(!debug.contains("refresh-secret"));
        assert!(debug.contains("Bearer"));
    }

    #[test]
    fn stored_json_without_optional_fields_reads_back() {
        let token: OAuthToken = serde_json::from_str(r#"{"access_token":"abc"}"#).unwrap();
        assert_eq!(token.token_type, "Bearer");
        assert!(token.refresh_token.is_none());
        assert!(token.expires_at.is_none());
        assert_eq!(token.authorization_header(), "Bearer abc");
    }

    #[test]
    fn lowercase_bearer_is_normalized_and_other_schemes_are_kept() {
        let mut token = OAuthToken::bearer("abc");
        token.token_type = "bearer".into();
        assert_eq!(token.authorization_header(), "Bearer abc");
        token.token_type = "MAC".into();
        assert_eq!(token.authorization_header(), "MAC abc");
    }
}
