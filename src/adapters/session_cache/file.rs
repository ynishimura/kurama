//! Test-only session cache backed by a JSON file.
//!
//! Compiled only with `--features test-fakes` and selected through
//! `KURAMA_TEST_SESSION_CACHE_FILE`. Runtime scenarios use it to verify that a
//! second process reuses the MFA session, without touching the login keychain.
//! Release builds never contain this code.
//! `KURAMA_TEST_KEYCHAIN_DENIED=<status code>` makes every read answer as
//! the keychain answers a build it refuses.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::domain::types::CachedSession;
use crate::ports::session_cache::{SessionCache, SessionCacheError};

pub struct FileSessionCache {
    path: PathBuf,
}

impl FileSessionCache {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn read(&self) -> Result<BTreeMap<String, CachedSession>, SessionCacheError> {
        match std::fs::read_to_string(&self.path) {
            Ok(content) => {
                serde_json::from_str(&content).map_err(|_| SessionCacheError::InvalidData)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(error) => Err(SessionCacheError::Backend(error.to_string())),
        }
    }

    fn write(&self, sessions: &BTreeMap<String, CachedSession>) -> Result<(), SessionCacheError> {
        let content =
            serde_json::to_string(sessions).map_err(|_| SessionCacheError::InvalidData)?;
        std::fs::write(&self.path, content).map_err(|e| SessionCacheError::Backend(e.to_string()))
    }
}

#[async_trait::async_trait]
impl SessionCache for FileSessionCache {
    async fn load(&self, key: &str) -> Result<Option<CachedSession>, SessionCacheError> {
        if let Some(denied) = crate::adapters::keychain::fake_denial() {
            return Err(SessionCacheError::Denied(denied));
        }
        Ok(self.read()?.remove(key))
    }

    async fn store(&self, key: &str, session: &CachedSession) -> Result<(), SessionCacheError> {
        let mut sessions = self.read()?;
        sessions.insert(key.to_string(), session.clone());
        self.write(&sessions)
    }

    async fn remove(&self, key: &str) -> Result<(), SessionCacheError> {
        let mut sessions = self.read()?;
        sessions.remove(key);
        self.write(&sessions)
    }
}
