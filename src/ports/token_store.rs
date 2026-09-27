//! Token store port: the OAuth token of each `[auth.*]` source, keyed by its
//! name. The macOS keychain in production, a file in runtime scenarios.

use async_trait::async_trait;

use crate::domain::types::OAuthToken;
use crate::ports::session_cache::KeychainDenied;

#[derive(Debug, thiserror::Error)]
pub enum TokenStoreError {
    #[error("token store backend: {0}")]
    Backend(String),
    #[error("token store backend: {0}")]
    Denied(KeychainDenied),
    #[error("invalid token store data")]
    InvalidData,
}

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait TokenStore: Send + Sync {
    async fn load(&self, key: &str) -> Result<Option<OAuthToken>, TokenStoreError>;
    async fn store(&self, key: &str, token: &OAuthToken) -> Result<(), TokenStoreError>;
    async fn remove(&self, key: &str) -> Result<(), TokenStoreError>;
}
