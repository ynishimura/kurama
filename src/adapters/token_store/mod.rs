//! Token store backends: the macOS keychain in production, a no-op on other
//! platforms, and a file for runtime scenarios (`--features test-fakes` plus
//! `KURAMA_TEST_TOKEN_STORE_FILE`).

use std::sync::Arc;

use crate::ports::token_store::TokenStore;

#[cfg(not(target_os = "macos"))]
mod noop {
    use crate::domain::types::OAuthToken;
    use crate::ports::token_store::{TokenStore, TokenStoreError};

    pub struct NoopTokenStore;

    #[async_trait::async_trait]
    impl TokenStore for NoopTokenStore {
        async fn load(&self, _: &str) -> Result<Option<OAuthToken>, TokenStoreError> {
            Ok(None)
        }
        async fn store(&self, _: &str, _: &OAuthToken) -> Result<(), TokenStoreError> {
            Ok(())
        }
        async fn remove(&self, _: &str) -> Result<(), TokenStoreError> {
            Ok(())
        }
    }
}

#[cfg(target_os = "macos")]
pub mod keyring;

#[cfg(feature = "test-fakes")]
pub mod file;

/// Only the macOS keychain backend persists tokens.
pub fn create_token_store() -> Arc<dyn TokenStore> {
    #[cfg(feature = "test-fakes")]
    if let Some(path) = std::env::var_os("KURAMA_TEST_TOKEN_STORE_FILE") {
        return Arc::new(file::FileTokenStore::new(path.into()));
    }
    #[cfg(target_os = "macos")]
    let store: Arc<dyn TokenStore> = Arc::new(keyring::KeyringTokenStore);
    #[cfg(not(target_os = "macos"))]
    let store: Arc<dyn TokenStore> = Arc::new(noop::NoopTokenStore);
    store
}
