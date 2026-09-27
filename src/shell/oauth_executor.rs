//! Interpreter of the OAuth token workflow: runs each effect through the
//! `ApiRuntime` ports and feeds the result back as an event.
//!
//! The browser, the loopback listener and the sleeps of the device flow
//! live here; the decisions live in `workflows::oauth_token`.

use std::time::Duration;

use anyhow::Result;
use chrono::Utc;
use tracing::warn;

use super::api_runtime::ApiRuntime;
use super::executor::emit_log;
use crate::adapters::browser::open_url;
use crate::adapters::config::constants::oauth::AUTHORIZATION_TIMEOUT_SECS;
use crate::adapters::keychain::log_denied;
use crate::adapters::oauth::CallbackListener;
use crate::console::progress;
use crate::domain::functions::oauth::encode_form;
use crate::domain::types::SecretFailure;
use crate::ports::{HttpRequest, TokenStoreError};
use crate::workflows::oauth_token::{
    TokenEffect, TokenEvent, TokenFailure, TokenInput, TokenOutput, TokenState, step,
};

/// Why a token could not be obtained; `ErrorCode` maps each to a code.
#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    /// A person has to log in: no usable token, nothing to refresh, and
    /// the grant needs a browser or a code entry but there is no terminal.
    #[error("{message}")]
    LoginRequired { profile: String, message: String },
    /// The authorization server refused.
    #[error("{0}")]
    Rejected(String),
    /// The server could not be reached, answered nonsense, or the browser
    /// never came back.
    #[error("{0}")]
    Failed(String),
    /// The client secret could not be read. `failure` is what the store said
    /// happened, and it is what decides the error code: a 1Password prompt
    /// nobody answered and an AWS refusal are not the same failure.
    #[error("{message}")]
    Secret {
        failure: SecretFailure,
        message: String,
    },
    /// The endpoints do not cover the grant.
    #[error("{0}")]
    Config(String),
    #[error("Unexpected state: {0}")]
    UnexpectedState(String),
}

/// Run the token workflow to completion. Errors keep their type in the chain
/// as `OAuthError`. A keychain that cannot be read or written is not one: the
/// token is used without being cached, and the reason is reported through
/// `TokenOutput::cache_error` for the caller that needs the cache.
pub async fn execute_token_workflow(
    runtime: &ApiRuntime,
    input: TokenInput,
) -> Result<TokenOutput> {
    let profile = input.client.name.clone();
    let mut state = TokenState::Initial;
    let mut event = TokenEvent::Start { input };
    let mut listener: Option<CallbackListener> = None;
    let mut cache_error: Option<String> = None;

    loop {
        let (next_state, effects) = step(state, event);

        let mut effect_event = None;
        for effect in effects {
            match effect {
                TokenEffect::Log { level, message } => emit_log(level, &message),
                TokenEffect::LoadToken { key } => {
                    let token = match runtime.token_store.load(&key).await {
                        Ok(token) => token,
                        Err(TokenStoreError::Denied(denied)) => {
                            log_denied(&format!("token of [auth.{key}]"), denied);
                            None
                        }
                        Err(error) => {
                            warn!("Failed to load the stored token for '{key}': {error}");
                            None
                        }
                    };
                    effect_event = Some(TokenEvent::TokenLoaded { token });
                }
                TokenEffect::Get { url } => {
                    let request =
                        HttpRequest::new("GET", url).with_header("Accept", "application/json");
                    effect_event = Some(http_event(runtime, request).await);
                }
                TokenEffect::PostForm { url, form } => {
                    let request = HttpRequest::new("POST", url)
                        .with_header("Accept", "application/json")
                        .with_header("Content-Type", "application/x-www-form-urlencoded")
                        .with_body(encode_form(&form).into_bytes());
                    effect_event = Some(http_event(runtime, request).await);
                }
                TokenEffect::ResolveSecret { secret } => {
                    effect_event = Some(match runtime.resolve_secret(&secret).await {
                        Ok(secret) => TokenEvent::SecretResolved { secret },
                        Err(error) => TokenEvent::SecretFailed {
                            failure: error.failure,
                            error: error.message,
                        },
                    });
                }
                TokenEffect::ListenForCallback { port } => {
                    effect_event = Some(match CallbackListener::bind(port).await {
                        Ok(bound) => {
                            let redirect_uri = bound.redirect_uri();
                            listener = Some(bound);
                            TokenEvent::CallbackListening { redirect_uri }
                        }
                        Err(error) => TokenEvent::CallbackListenFailed {
                            error: error.to_string(),
                        },
                    });
                }
                TokenEffect::Authorize { url } => {
                    if runtime.should_open_browser() {
                        progress!(
                            "# Opening the browser to authorize kurama; if nothing opens, visit:"
                        );
                        if let Err(error) = open_url(&url) {
                            warn!("Could not open the browser: {error}");
                        }
                    } else {
                        progress!("# Open this URL in a browser to authorize kurama:");
                    }
                    progress!("# {url}");
                }
                TokenEffect::WaitForCallback => {
                    let Some(bound) = listener.take() else {
                        return Err(OAuthError::UnexpectedState(
                            "waiting for a redirect without a listener".into(),
                        )
                        .into());
                    };
                    progress!(
                        "# Waiting for the browser (up to {} minutes)",
                        AUTHORIZATION_TIMEOUT_SECS / 60
                    );
                    effect_event = Some(
                        match bound
                            .wait(Duration::from_secs(AUTHORIZATION_TIMEOUT_SECS))
                            .await
                        {
                            Ok(query) => TokenEvent::CallbackReceived { query },
                            Err(error) => TokenEvent::CallbackFailed { error },
                        },
                    );
                }
                TokenEffect::ShowDeviceCode {
                    verification_uri,
                    user_code,
                    verification_uri_complete,
                } => {
                    progress!("# Open {verification_uri} and enter the code {user_code}");
                    if runtime.should_open_browser() {
                        let url = verification_uri_complete
                            .as_deref()
                            .unwrap_or(&verification_uri);
                        if let Err(error) = open_url(url) {
                            warn!("Could not open the browser: {error}");
                        }
                    }
                    progress!(
                        "# Waiting for the code to be entered (up to {} minutes)",
                        AUTHORIZATION_TIMEOUT_SECS / 60
                    );
                }
                TokenEffect::Sleep { seconds } => {
                    tokio::time::sleep(Duration::from_secs(seconds)).await;
                    effect_event = Some(TokenEvent::Slept);
                }
                TokenEffect::StoreToken { key, token } => {
                    // The keychain is a cache. macOS grants access per build
                    // signature, so a rebuilt kurama cannot write entries an
                    // earlier one made; failing here would throw away a token
                    // that is already valid. `login` still reports it, because
                    // filling the cache is the whole point of that command.
                    if let Err(error) = runtime.token_store.store(&key, &token).await {
                        warn!(
                            profile = %profile,
                            %error,
                            "the token could not be cached; it is used for this run only"
                        );
                        cache_error = Some(error.to_string());
                    }
                }
            }
        }

        state = next_state;
        event = match &state {
            TokenState::Completed { output } => {
                return Ok(TokenOutput {
                    cache_error,
                    ..output.clone()
                });
            }
            TokenState::Failed { kind, message } => {
                let message = message.clone();
                return Err(match kind {
                    TokenFailure::LoginRequired => OAuthError::LoginRequired {
                        profile: profile.clone(),
                        message,
                    },
                    TokenFailure::Rejected => OAuthError::Rejected(message),
                    TokenFailure::Transport => OAuthError::Failed(message),
                    TokenFailure::Secret(failure) => OAuthError::Secret {
                        failure: *failure,
                        message,
                    },
                    TokenFailure::Config => OAuthError::Config(message),
                }
                .into());
            }
            _ => effect_event.ok_or_else(|| {
                OAuthError::UnexpectedState(format!("Unexpected state: {state:?}"))
            })?,
        };
    }
}

/// Discovery and token requests go to the authorization server through
/// `token_http`, which always verifies certificates.
async fn http_event(runtime: &ApiRuntime, request: HttpRequest) -> TokenEvent {
    match runtime.send_token_request(request).await {
        Ok(response) => TokenEvent::Responded {
            status: response.status,
            body: response.body,
            received_at: Utc::now(),
            received_instant: tokio::time::Instant::now().into_std(),
        },
        Err(error) => TokenEvent::RequestFailed {
            error: error.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::config::{AuthToml, Config};
    use crate::domain::types::{OAuthToken, SecretFailure, SecretRef};
    use crate::ports::HttpResponse;
    use crate::ports::aws_credentials::MockAwsProfileCredentials;
    use crate::ports::http::MockHttpClient;
    use crate::ports::secret::{MockSecretResolver, SecretError};
    use crate::ports::token_store::{MockTokenStore, TokenStoreError};
    use crate::workflows::oauth_token::TokenMode;
    use std::sync::Arc;

    fn client(toml: &str) -> crate::domain::types::OAuthClientConfig {
        match toml::from_str::<AuthToml>(toml).unwrap().typed("svc") {
            Ok(crate::domain::types::AuthSource::OAuth(client)) => client,
            other => panic!("expected an oauth source, got {other:?}"),
        }
    }

    const CLIENT_CREDENTIALS: &str = "kind = \"oauth\"\ngrant_type = \"client_credentials\"\ntoken_url = \"https://as.example.com/token\"\nclient_id = \"id\"\nclient_secret = \"op://Agent/Item/secret\"\n";
    const AUTH_CODE: &str = "kind = \"oauth\"\ngrant_type = \"authorization_code\"\nauth_url = \"https://as.example.com/auth\"\ntoken_url = \"https://as.example.com/token\"\nclient_id = \"id\"\n";

    fn input(toml: &str, mode: TokenMode, interactive: bool) -> TokenInput {
        TokenInput {
            client: client(toml),
            mode,
            interactive,
            now: Utc::now(),
            pkce_verifier: "verifier".into(),
            state: "state".into(),
        }
    }

    fn runtime(
        http: MockHttpClient,
        store: MockTokenStore,
        secrets: MockSecretResolver,
        interactive: bool,
    ) -> ApiRuntime {
        ApiRuntime::test(
            Config::default(),
            http,
            store,
            secrets,
            MockAwsProfileCredentials::new(),
            interactive,
        )
    }

    #[tokio::test]
    async fn client_credentials_post_the_resolved_secret_and_store_the_token() {
        let mut http = MockHttpClient::new();
        http.expect_send()
            .times(1)
            .withf(|request| {
                request.method == "POST"
                    && request.url == "https://as.example.com/token"
                    && request.header("content-type") == Some("application/x-www-form-urlencoded")
                    && request.header("accept") == Some("application/json")
                    && request.body.as_deref()
                        == Some(
                            b"grant_type=client_credentials&client_id=id&client_secret=s3cret"
                                .as_slice(),
                        )
            })
            .returning(|_| {
                Ok(HttpResponse {
                    status: 200,
                    headers: vec![],
                    body: br#"{"access_token":"cc","expires_in":60}"#.to_vec(),
                })
            });
        let mut store = MockTokenStore::new();
        store.expect_load().times(1).returning(|_| Ok(None));
        store
            .expect_store()
            .times(1)
            .withf(|key, token| key == "svc" && token.access_token == "cc")
            .returning(|_, _| Ok(()));
        let mut secrets = MockSecretResolver::new();
        secrets
            .expect_resolve()
            .times(1)
            .withf(|secret| matches!(secret, SecretRef::OnePassword(r) if r == "op://Agent/Item/secret"))
            .returning(|_| Ok("s3cret".into()));
        let output = execute_token_workflow(
            &runtime(http, store, secrets, false),
            input(CLIENT_CREDENTIALS, TokenMode::Reuse, false),
        )
        .await
        .unwrap();
        assert_eq!(output.token.access_token, "cc");
        assert!(!output.reused);
    }

    #[tokio::test]
    async fn token_requests_use_the_authorization_server_client() {
        let mut token_http = MockHttpClient::new();
        token_http
            .expect_send()
            .times(1)
            .withf(|request| request.url == "https://as.example.com/token")
            .returning(|_| {
                Ok(HttpResponse {
                    status: 200,
                    headers: vec![],
                    body: br#"{"access_token":"cc"}"#.to_vec(),
                })
            });
        let mut store = MockTokenStore::new();
        store.expect_load().returning(|_| Ok(None));
        store.expect_store().returning(|_, _| Ok(()));
        let mut secrets = MockSecretResolver::new();
        secrets.expect_resolve().returning(|_| Ok("s".into()));
        // The API client, the one `kurama api -k` may relax, expects nothing.
        let mut runtime = runtime(MockHttpClient::new(), store, secrets, false);
        runtime.set_token_http(Arc::new(token_http));
        let output =
            execute_token_workflow(&runtime, input(CLIENT_CREDENTIALS, TokenMode::Reuse, false))
                .await
                .unwrap();
        assert_eq!(output.token.access_token, "cc");
    }

    #[tokio::test]
    async fn a_usable_stored_token_needs_no_request() {
        let mut store = MockTokenStore::new();
        store
            .expect_load()
            .returning(|_| Ok(Some(OAuthToken::bearer("stored"))));
        let output = execute_token_workflow(
            &runtime(
                MockHttpClient::new(),
                store,
                MockSecretResolver::new(),
                false,
            ),
            input(AUTH_CODE, TokenMode::Reuse, false),
        )
        .await
        .unwrap();
        assert!(output.reused);
        assert_eq!(output.token.access_token, "stored");
    }

    #[tokio::test]
    async fn an_unreadable_store_counts_as_no_token() {
        let mut store = MockTokenStore::new();
        store
            .expect_load()
            .returning(|_| Err(TokenStoreError::Backend("locked".into())));
        let error = execute_token_workflow(
            &runtime(
                MockHttpClient::new(),
                store,
                MockSecretResolver::new(),
                false,
            ),
            input(AUTH_CODE, TokenMode::Reuse, false),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<OAuthError>(),
            Some(OAuthError::LoginRequired { profile, .. }) if profile == "svc"
        ));
    }

    #[tokio::test]
    async fn failures_keep_their_type() {
        let mut secrets = MockSecretResolver::new();
        secrets
            .expect_resolve()
            .returning(|_| Err(SecretError::needs_a_person("not signed in")));
        let mut store = MockTokenStore::new();
        store.expect_load().returning(|_| Ok(None));
        let error = execute_token_workflow(
            &runtime(MockHttpClient::new(), store, secrets, false),
            input(CLIENT_CREDENTIALS, TokenMode::Reuse, false),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<OAuthError>(),
            Some(OAuthError::Secret { failure: SecretFailure::NeedsAPerson, message })
                if message == "not signed in"
        ));

        let mut http = MockHttpClient::new();
        http.expect_send().returning(|_| {
            Ok(HttpResponse {
                status: 400,
                headers: vec![],
                body: br#"{"error":"invalid_client"}"#.to_vec(),
            })
        });
        let mut store = MockTokenStore::new();
        store.expect_load().returning(|_| Ok(None));
        let mut secrets = MockSecretResolver::new();
        secrets.expect_resolve().returning(|_| Ok("s".into()));
        let error = execute_token_workflow(
            &runtime(http, store, secrets, false),
            input(CLIENT_CREDENTIALS, TokenMode::Reuse, false),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<OAuthError>(),
            Some(OAuthError::Rejected(message)) if message.contains("invalid_client")
        ));
    }

    /// macOS grants keychain access per build signature, so a rebuilt kurama
    /// cannot write the entries an earlier one made. That must not throw away
    /// a token the authorization server just handed out.
    #[tokio::test]
    async fn a_token_that_cannot_be_cached_is_still_returned() {
        let mut http = MockHttpClient::new();
        http.expect_send().returning(|_| {
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: br#"{"access_token":"cc"}"#.to_vec(),
            })
        });
        let mut store = MockTokenStore::new();
        store.expect_load().returning(|_| Ok(None));
        store
            .expect_store()
            .returning(|_, _| Err(TokenStoreError::Backend("locked".into())));
        let mut secrets = MockSecretResolver::new();
        secrets.expect_resolve().returning(|_| Ok("s".into()));
        let output = execute_token_workflow(
            &runtime(http, store, secrets, false),
            input(CLIENT_CREDENTIALS, TokenMode::Reuse, false),
        )
        .await
        .expect("an uncacheable token is still a token");
        assert_eq!(output.token.access_token, "cc");
    }
}
