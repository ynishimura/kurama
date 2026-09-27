//! AWS console sign-in through the federation endpoint: role credentials in,
//! a console URL out. Building the URL and the session document is pure and
//! lives in `domain::functions::federation`; this file makes the HTTP call.

use crate::adapters::config::constants::{
    app::FEDERATION_SESSION_DURATION, aws::FEDERATION_ENDPOINT,
};
use crate::domain::functions::federation::{build_console_url, build_session_credentials};
use serde::Deserialize;

/// Why the federation endpoint gave no sign-in token.
#[derive(Debug, thiserror::Error)]
pub enum FederationError {
    #[error("Federation request failed: {0}")]
    RequestFailed(String),

    #[error("Invalid response: {0}")]
    InvalidResponse(String),

    #[error("Network error: {0}")]
    NetworkError(String),
}

/// The endpoint's answer; the token is a credential, so `Debug` hides it.
#[derive(Deserialize)]
struct FederationToken {
    #[serde(rename = "SigninToken")]
    signin_token: String,
}

impl std::fmt::Debug for FederationToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FederationToken")
            .field("signin_token", &"[REDACTED]")
            .finish()
    }
}

pub struct FederationService {
    client: reqwest::Client,
    /// `KURAMA_FEDERATION_ENDPOINT` overrides the AWS endpoint so runtime
    /// scenarios can point the sign-in token request at a local fake.
    endpoint: String,
}

impl FederationService {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            endpoint: std::env::var("KURAMA_FEDERATION_ENDPOINT")
                .unwrap_or_else(|_| FEDERATION_ENDPOINT.to_string()),
        }
    }

    /// The console URL that signs in with these role credentials, valid for
    /// the longest session federation allows.
    pub async fn console_url(
        &self,
        access_key_id: &str,
        secret_access_key: &str,
        session_token: &str,
    ) -> Result<String, FederationError> {
        let session = build_session_credentials(access_key_id, secret_access_key, session_token);
        let params = [
            ("Action", "getSigninToken"),
            ("DurationSeconds", &FEDERATION_SESSION_DURATION.to_string()),
            ("SessionType", "json"),
            ("Session", &session.to_string()),
        ];

        // The request URL carries the role credentials in `Session`, so a
        // Reqwest error loses its URL before it becomes a message.
        let response = self
            .client
            .get(&self.endpoint)
            .query(&params)
            .send()
            .await
            .map_err(|e| FederationError::NetworkError(e.without_url().to_string()))?;

        if !response.status().is_success() {
            return Err(FederationError::RequestFailed(format!(
                "Federation request failed with status: {}",
                response.status()
            )));
        }

        let token: FederationToken = response
            .json()
            .await
            .map_err(|e| FederationError::InvalidResponse(e.without_url().to_string()))?;
        Ok(build_console_url(&token.signin_token, None))
    }
}

impl Default for FederationService {
    fn default() -> Self {
        Self::new()
    }
}
