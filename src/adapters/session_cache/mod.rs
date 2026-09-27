//! Session cache backends: the macOS keychain in production, a no-op
//! elsewhere or when caching is disabled, and a file for runtime scenarios
//! (`--features test-fakes` plus `KURAMA_TEST_SESSION_CACHE_FILE`).

use crate::domain::types::CachedSession;
use crate::ports::session_cache::{SessionCache, SessionCacheError};
use std::sync::Arc;

pub struct NoopSessionCache;

#[async_trait::async_trait]
impl SessionCache for NoopSessionCache {
    async fn load(&self, _: &str) -> Result<Option<CachedSession>, SessionCacheError> {
        Ok(None)
    }
    async fn store(&self, _: &str, _: &CachedSession) -> Result<(), SessionCacheError> {
        Ok(())
    }
    async fn remove(&self, _: &str) -> Result<(), SessionCacheError> {
        Ok(())
    }
}

#[cfg(target_os = "macos")]
pub mod keyring;

#[cfg(feature = "test-fakes")]
pub mod file;

/// Only the macOS keychain backend is supported. Other platforms do not persist sessions.
pub fn create_session_cache(enabled: bool) -> Arc<dyn SessionCache> {
    if !enabled {
        return Arc::new(NoopSessionCache);
    }
    #[cfg(feature = "test-fakes")]
    if let Some(path) = std::env::var_os("KURAMA_TEST_SESSION_CACHE_FILE") {
        return Arc::new(file::FileSessionCache::new(path.into()));
    }
    #[cfg(target_os = "macos")]
    let cache: Arc<dyn SessionCache> = Arc::new(keyring::KeyringSessionCache);
    #[cfg(not(target_os = "macos"))]
    let cache: Arc<dyn SessionCache> = Arc::new(NoopSessionCache);
    cache
}
