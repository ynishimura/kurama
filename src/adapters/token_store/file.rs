//! Test-only token store backed by a JSON file.
//!
//! Compiled only with `--features test-fakes` and selected through
//! `KURAMA_TEST_TOKEN_STORE_FILE`. Runtime scenarios use it to verify that
//! `login` stores a token and `api` reuses it, without touching the login
//! keychain. Callers serialize access with the profile lock, so read-modify-
//! write is safe across processes. Release builds never contain this code.
//! `KURAMA_TEST_KEYCHAIN_DENIED=<status code>` makes every read answer as
//! the keychain answers a build it refuses.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::domain::types::OAuthToken;
use crate::ports::token_store::{TokenStore, TokenStoreError};

pub struct FileTokenStore {
    path: PathBuf,
}

impl FileTokenStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn read(&self) -> Result<BTreeMap<String, OAuthToken>, TokenStoreError> {
        match std::fs::read_to_string(&self.path) {
            Ok(content) => serde_json::from_str(&content).map_err(|_| TokenStoreError::InvalidData),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(error) => Err(TokenStoreError::Backend(error.to_string())),
        }
    }

    fn write(&self, tokens: &BTreeMap<String, OAuthToken>) -> Result<(), TokenStoreError> {
        let content = serde_json::to_string(tokens).map_err(|_| TokenStoreError::InvalidData)?;
        std::fs::write(&self.path, content).map_err(|e| TokenStoreError::Backend(e.to_string()))
    }
}

#[async_trait::async_trait]
impl TokenStore for FileTokenStore {
    async fn load(&self, key: &str) -> Result<Option<OAuthToken>, TokenStoreError> {
        if let Some(denied) = crate::adapters::keychain::fake_denial() {
            return Err(TokenStoreError::Denied(denied));
        }
        Ok(self.read()?.remove(key))
    }

    async fn store(&self, key: &str, token: &OAuthToken) -> Result<(), TokenStoreError> {
        let mut tokens = self.read()?;
        tokens.insert(key.to_string(), token.clone());
        self.write(&tokens)
    }

    async fn remove(&self, key: &str) -> Result<(), TokenStoreError> {
        let mut tokens = self.read()?;
        tokens.remove(key);
        self.write(&tokens)
    }
}
