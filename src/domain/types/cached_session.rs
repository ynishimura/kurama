//! `CachedSession`: the MFA-authenticated session from GetSessionToken, zeroized on drop and redacted in `Debug`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// MFA-authenticated IAM-user session returned by GetSessionToken.
#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct CachedSession {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: String,
    #[zeroize(skip)]
    pub expiration: DateTime<Utc>,
}

impl std::fmt::Debug for CachedSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedSession")
            .field("access_key_id", &"****")
            .field("expiration", &self.expiration)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_session_debug_redacts_credentials() {
        let session = CachedSession {
            access_key_id: "ASIAEXAMPLE123456".into(),
            secret_access_key: "sensitive-secret".into(),
            session_token: "sensitive-token".into(),
            expiration: DateTime::from_timestamp(1800000000, 0).unwrap(),
        };
        let debug = format!("{session:?}");
        assert!(!debug.contains(&session.access_key_id));
        assert!(!debug.contains(&session.secret_access_key));
        assert!(!debug.contains(&session.session_token));
        assert!(debug.contains("expiration"));
    }
}
