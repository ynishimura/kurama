//! `SessionCache`: where GetSessionToken sessions are kept (the keychain, or a file under `test-fakes`).

use crate::domain::types::CachedSession;
use async_trait::async_trait;

/// macOS refused this build a keychain entry: it grants access per binary
/// signature, and an ad-hoc build is signed with a new hash every time. The
/// session cache and the token store both report it, so the caller can say it
/// once per process instead of once per entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "macOS denied this build access to the keychain entry (error {code}); \
     an ad-hoc build is signed with a new hash every time, so approve it \
     once in a terminal, or install with `cargo xtask install-signed` so \
     the signature stops changing"
)]
pub struct KeychainDenied {
    pub code: i32,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionCacheError {
    #[error("Session cache backend: {0}")]
    Backend(String),
    #[error("Session cache backend: {0}")]
    Denied(KeychainDenied),
    #[error("Invalid session cache data")]
    InvalidData,
}

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait SessionCache: Send + Sync {
    async fn load(&self, key: &str) -> Result<Option<CachedSession>, SessionCacheError>;
    async fn store(&self, key: &str, session: &CachedSession) -> Result<(), SessionCacheError>;
    async fn remove(&self, key: &str) -> Result<(), SessionCacheError>;
}
