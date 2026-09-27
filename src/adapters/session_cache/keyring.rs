//! Session cache in the macOS keychain, service `kurama-session`, keyed by
//! the MFA device ARN.

use crate::adapters::keychain::{self, KeychainError};
use crate::domain::types::CachedSession;
use crate::ports::session_cache::{SessionCache, SessionCacheError};

const SERVICE: &str = "kurama-session";

pub struct KeyringSessionCache;

#[async_trait::async_trait]
impl SessionCache for KeyringSessionCache {
    async fn load(&self, key: &str) -> Result<Option<CachedSession>, SessionCacheError> {
        keychain::load_json(SERVICE, key).map_err(convert)
    }
    async fn store(&self, key: &str, session: &CachedSession) -> Result<(), SessionCacheError> {
        keychain::store_json(SERVICE, key, session).map_err(convert)
    }
    async fn remove(&self, key: &str) -> Result<(), SessionCacheError> {
        keychain::remove(SERVICE, key).map_err(convert)
    }
}

fn convert(error: KeychainError) -> SessionCacheError {
    match error {
        KeychainError::Backend(message) => SessionCacheError::Backend(message),
        KeychainError::Denied(denied) => SessionCacheError::Denied(denied),
        KeychainError::InvalidData => SessionCacheError::InvalidData,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> CachedSession {
        CachedSession {
            access_key_id: "test-access-key".into(),
            secret_access_key: "test-secret".into(),
            session_token: "test-token".into(),
            expiration: chrono::DateTime::from_timestamp(1800000000, 0).unwrap(),
        }
    }

    #[test]
    fn keyring_password_decodes_all_fields() {
        let expected = session();
        let actual: CachedSession = keychain::decode(Ok(serde_json::to_string(&expected).unwrap()))
            .unwrap()
            .unwrap();
        assert_eq!(actual.access_key_id, expected.access_key_id);
        assert_eq!(actual.secret_access_key, expected.secret_access_key);
        assert_eq!(actual.session_token, expected.session_token);
        assert_eq!(actual.expiration, expected.expiration);
    }

    #[tokio::test]
    #[ignore = "Uses the real macOS keychain; run manually on an unlocked login keychain"]
    async fn keyring_round_trip_missing_and_corrupt_entry() {
        let key = format!(
            "kurama-test-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        );
        let cache = KeyringSessionCache;
        let entry = keyring::Entry::new(SERVICE, &key).unwrap();
        struct Cleanup(keyring::Entry);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = self.0.delete_credential();
            }
        }
        let cleanup = Cleanup(entry);
        assert!(cache.load(&key).await.unwrap().is_none());
        cache.remove(&key).await.unwrap();
        let expected = session();
        cache.store(&key, &expected).await.unwrap();
        let actual = cache.load(&key).await.unwrap().unwrap();
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(&expected).unwrap()
        );
        cleanup.0.set_password("not-json").unwrap();
        assert!(matches!(
            cache.load(&key).await,
            Err(SessionCacheError::InvalidData)
        ));
        cache.remove(&key).await.unwrap();
        assert!(cache.load(&key).await.unwrap().is_none());
    }
}
