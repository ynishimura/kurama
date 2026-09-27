//! Token store in the macOS keychain, service `kurama-token`, keyed by the
//! `[auth.*]` source name.

use crate::adapters::keychain::{self, KeychainError};
use crate::domain::types::OAuthToken;
use crate::ports::token_store::{TokenStore, TokenStoreError};

const SERVICE: &str = "kurama-token";

pub struct KeyringTokenStore;

#[async_trait::async_trait]
impl TokenStore for KeyringTokenStore {
    async fn load(&self, key: &str) -> Result<Option<OAuthToken>, TokenStoreError> {
        keychain::load_json(SERVICE, key).map_err(convert)
    }
    async fn store(&self, key: &str, token: &OAuthToken) -> Result<(), TokenStoreError> {
        keychain::store_json(SERVICE, key, token).map_err(convert)
    }
    async fn remove(&self, key: &str) -> Result<(), TokenStoreError> {
        keychain::remove(SERVICE, key).map_err(convert)
    }
}

fn convert(error: KeychainError) -> TokenStoreError {
    match error {
        KeychainError::Backend(message) => TokenStoreError::Backend(message),
        KeychainError::Denied(denied) => TokenStoreError::Denied(denied),
        KeychainError::InvalidData => TokenStoreError::InvalidData,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyring_password_decodes_a_token() {
        let token: OAuthToken = keychain::decode(Ok(
            r#"{"access_token":"at","refresh_token":"rt"}"#.to_string(),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(token.access_token, "at");
        assert_eq!(token.refresh_token.as_deref(), Some("rt"));
        assert!(matches!(
            convert(KeychainError::InvalidData),
            TokenStoreError::InvalidData
        ));
    }
}
