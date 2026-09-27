//! State transition tests of the OAuth token workflow.

use std::time::Instant;

use chrono::{DateTime, Duration, Utc};

use crate::workflows::common::LogLevel;

use super::*;
use crate::domain::functions::oauth::{encode_form, pkce_challenge};
use crate::domain::types::{
    DEFAULT_TOKEN_ENV_VAR, EndpointSource, GrantType, OAuthClientConfig, OAuthEndpoints,
    OAuthToken, SecretFailure, SecretRef,
};

const TOKEN_URL: &str = "https://as.example.com/token";
const AUTH_URL: &str = "https://as.example.com/authorize";
const DEVICE_URL: &str = "https://as.example.com/device";

fn now() -> DateTime<Utc> {
    DateTime::from_timestamp(1_800_000_000, 0).unwrap()
}

fn client(grant: GrantType) -> OAuthClientConfig {
    OAuthClientConfig {
        name: "github".into(),
        grant_type: grant,
        endpoints: EndpointSource::Explicit(OAuthEndpoints {
            auth_url: Some(AUTH_URL.into()),
            token_url: TOKEN_URL.into(),
            device_auth_url: Some(DEVICE_URL.into()),
        }),
        client_id: "client-1".into(),
        client_secret: None,
        scopes: vec!["repo".into()],
        env_var: DEFAULT_TOKEN_ENV_VAR.into(),
        redirect_port: None,
    }
}

fn input(client: OAuthClientConfig, mode: TokenMode, interactive: bool) -> TokenInput {
    TokenInput {
        client,
        mode,
        interactive,
        now: now(),
        pkce_verifier: "verifier-1".into(),
        state: "state-1".into(),
    }
}

fn valid_token() -> OAuthToken {
    let mut token = OAuthToken::bearer("stored-access");
    token.expires_at = Some(now() + Duration::hours(1));
    token
}

fn expired_refreshable_token() -> OAuthToken {
    let mut token = OAuthToken::bearer("stored-access");
    token.refresh_token = Some("stored-refresh".into());
    token.expires_at = Some(now() - Duration::minutes(5));
    token
}

fn start(input: TokenInput) -> (TokenState, Vec<TokenEffect>) {
    step(TokenState::Initial, TokenEvent::Start { input })
}

fn form_of(effects: &[TokenEffect]) -> (String, String) {
    match effects
        .iter()
        .find(|effect| matches!(effect, TokenEffect::PostForm { .. }))
    {
        Some(TokenEffect::PostForm { url, form }) => (url.clone(), encode_form(form)),
        other => panic!("expected a PostForm effect, got {other:?}"),
    }
}

fn responded(status: u16, body: &str) -> TokenEvent {
    responded_at(status, body, now())
}

fn responded_at(status: u16, body: &str, received_at: DateTime<Utc>) -> TokenEvent {
    TokenEvent::Responded {
        status,
        body: body.as_bytes().to_vec(),
        received_at,
        received_instant: *START,
    }
}

/// The moment every test response arrives at on the monotonic clock, unless
/// it names how many seconds later it arrived.
static START: std::sync::LazyLock<Instant> = std::sync::LazyLock::new(Instant::now);

fn responded_after(secs: u64, status: u16, body: &str) -> TokenEvent {
    TokenEvent::Responded {
        status,
        body: body.as_bytes().to_vec(),
        received_at: now() + Duration::seconds(secs as i64),
        received_instant: *START + std::time::Duration::from_secs(secs),
    }
}

/// A device flow whose authorization answered `answer` at second 0.
fn device_flow(answer: &str) -> (TokenState, Vec<TokenEffect>) {
    let (state, _) = start(input(
        client(GrantType::DeviceCode),
        TokenMode::ForceGrant,
        true,
    ));
    step(state, responded_after(0, 200, answer))
}

fn has_sleep(effects: &[TokenEffect]) -> bool {
    effects
        .iter()
        .any(|effect| matches!(effect, TokenEffect::Sleep { .. }))
}

fn completed_token(state: &TokenState) -> &OAuthToken {
    match state {
        TokenState::Completed {
            output:
                TokenOutput {
                    token,
                    reused: false,
                    ..
                },
        } => token,
        other => panic!("expected Completed without reuse, got {other:?}"),
    }
}

fn failure(state: &TokenState) -> (&TokenFailure, &str) {
    match state {
        TokenState::Failed { kind, message } => (kind, message),
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[test]
fn usable_stored_token_is_reused_without_any_request() {
    let (state, effects) = start(input(
        client(GrantType::AuthorizationCode),
        TokenMode::Reuse,
        false,
    ));
    assert!(matches!(
        effects.as_slice(),
        [TokenEffect::LoadToken { key }] if key == "github"
    ));
    let (state, effects) = step(
        state,
        TokenEvent::TokenLoaded {
            token: Some(valid_token()),
        },
    );
    assert!(effects.is_empty());
    assert!(matches!(
        state,
        TokenState::Completed { output: TokenOutput { reused: true, token, .. } }
            if token.access_token == "stored-access"
    ));
}

#[test]
fn expired_token_with_refresh_token_is_refreshed_and_stored() {
    let (state, _) = start(input(
        client(GrantType::AuthorizationCode),
        TokenMode::Reuse,
        false,
    ));
    let (state, effects) = step(
        state,
        TokenEvent::TokenLoaded {
            token: Some(expired_refreshable_token()),
        },
    );
    let (url, form) = form_of(&effects);
    assert_eq!(url, TOKEN_URL);
    assert_eq!(
        form,
        "grant_type=refresh_token&refresh_token=stored-refresh&client_id=client-1"
    );
    let (state, effects) = step(
        state,
        responded(200, r#"{"access_token":"new-access","expires_in":3600}"#),
    );
    assert!(matches!(
        effects.as_slice(),
        [TokenEffect::StoreToken { key, token }]
            if key == "github"
                && token.access_token == "new-access"
                && token.refresh_token.as_deref() == Some("stored-refresh")
    ));
    assert_eq!(completed_token(&state).access_token, "new-access");
}

#[test]
fn a_bare_status_during_refresh_fails_without_a_grant() {
    let (state, _) = start(input(
        client(GrantType::AuthorizationCode),
        TokenMode::Reuse,
        true,
    ));
    let (state, _) = step(
        state,
        TokenEvent::TokenLoaded {
            token: Some(expired_refreshable_token()),
        },
    );
    let (state, effects) = step(state, responded(503, "<html>Service Unavailable</html>"));
    let (kind, message) = failure(&state);
    assert_eq!(*kind, TokenFailure::Rejected);
    assert!(message.contains("HTTP 503"), "{message}");
    assert!(
        effects
            .iter()
            .all(|effect| matches!(effect, TokenEffect::Log { .. }))
    );
}

#[test]
fn a_rejected_refresh_reuses_the_discovered_endpoints_and_the_resolved_secret() {
    let mut client = client(GrantType::ClientCredentials);
    client.endpoints = EndpointSource::Issuer("https://accounts.example.com".into());
    client.client_secret = Some(SecretRef::parse("op://Agent/Item/secret").unwrap());
    let (state, _) = start(input(client, TokenMode::Reuse, false));
    let (state, _) = step(
        state,
        TokenEvent::TokenLoaded {
            token: Some(expired_refreshable_token()),
        },
    );
    let (state, _) = step(
        state,
        responded(
            200,
            r#"{"token_endpoint":"https://accounts.example.com/oauth2/token"}"#,
        ),
    );
    let (state, _) = step(
        state,
        TokenEvent::SecretResolved {
            secret: "s3cret".into(),
        },
    );
    assert!(matches!(state, TokenState::Refreshing { .. }));
    let (state, effects) = step(state, responded(400, r#"{"error":"invalid_grant"}"#));
    assert!(matches!(state, TokenState::RequestingClientCredentials(_)));
    assert!(effects.iter().all(|effect| !matches!(
        effect,
        TokenEffect::Get { .. } | TokenEffect::ResolveSecret { .. }
    )));
    let (url, form) = form_of(&effects);
    assert_eq!(url, "https://accounts.example.com/oauth2/token");
    assert_eq!(
        form,
        "grant_type=client_credentials&client_id=client-1&client_secret=s3cret&scope=repo"
    );
}

#[test]
fn expires_at_counts_from_the_answer_not_from_the_start() {
    let (state, _) = start(input(
        client(GrantType::ClientCredentials),
        TokenMode::ForceGrant,
        false,
    ));
    let received_at = now() + Duration::minutes(3);
    let (state, _) = step(
        state,
        responded_at(
            200,
            r#"{"access_token":"at","expires_in":600}"#,
            received_at,
        ),
    );
    assert_eq!(
        completed_token(&state).expires_at,
        Some(received_at + Duration::seconds(600))
    );
}

#[test]
fn rejected_refresh_needs_a_person_without_a_terminal_and_restarts_the_grant_with_one() {
    for interactive in [false, true] {
        let (state, _) = start(input(
            client(GrantType::AuthorizationCode),
            TokenMode::Reuse,
            interactive,
        ));
        let (state, _) = step(
            state,
            TokenEvent::TokenLoaded {
                token: Some(expired_refreshable_token()),
            },
        );
        let (state, effects) = step(state, responded(400, r#"{"error":"invalid_grant"}"#));
        if interactive {
            assert!(matches!(state, TokenState::ListeningForCallback(_)));
            assert!(
                effects
                    .iter()
                    .any(|e| matches!(e, TokenEffect::ListenForCallback { port: None }))
            );
        } else {
            let (kind, message) = failure(&state);
            assert_eq!(*kind, TokenFailure::LoginRequired);
            assert!(message.contains("github"), "{message}");
        }
    }
}

#[test]
fn missing_token_without_a_terminal_stops_before_any_request() {
    for grant in [GrantType::AuthorizationCode, GrantType::DeviceCode] {
        let (state, _) = start(input(client(grant), TokenMode::Reuse, false));
        let (state, effects) = step(state, TokenEvent::TokenLoaded { token: None });
        assert_eq!(failure(&state).0, &TokenFailure::LoginRequired);
        // The reason is the error line; the log must not repeat it as a warning.
        assert!(effects.iter().all(|e| matches!(
            e,
            TokenEffect::Log {
                level: LogLevel::Debug,
                ..
            }
        )));
    }
}

#[test]
fn client_credentials_run_without_a_terminal_and_a_force_skips_the_load() {
    let (state, effects) = start(input(
        client(GrantType::ClientCredentials),
        TokenMode::ForceGrant,
        false,
    ));
    assert!(matches!(state, TokenState::RequestingClientCredentials(_)));
    let (url, form) = form_of(&effects);
    assert_eq!(url, TOKEN_URL);
    assert_eq!(
        form,
        "grant_type=client_credentials&client_id=client-1&scope=repo"
    );
    let (state, effects) = step(state, responded(200, r#"{"access_token":"cc-token"}"#));
    assert_eq!(completed_token(&state).access_token, "cc-token");
    assert!(matches!(
        effects.as_slice(),
        [TokenEffect::StoreToken { token, .. }] if token.access_token == "cc-token"
    ));
}

#[test]
fn authorization_code_flow_opens_the_browser_and_exchanges_the_code_with_pkce() {
    let mut client = client(GrantType::AuthorizationCode);
    client.redirect_port = Some(8080);
    let (state, effects) = start(input(client, TokenMode::ForceGrant, true));
    assert!(matches!(
        effects.as_slice(),
        [TokenEffect::ListenForCallback { port: Some(8080) }]
    ));
    let (state, effects) = step(
        state,
        TokenEvent::CallbackListening {
            redirect_uri: "http://127.0.0.1:8080/callback".into(),
        },
    );
    let TokenEffect::Authorize { url } = &effects[0] else {
        panic!("expected Authorize, got {effects:?}");
    };
    assert!(url.starts_with(AUTH_URL));
    assert!(url.contains("state=state-1"));
    assert!(url.contains(&format!("code_challenge={}", pkce_challenge("verifier-1"))));
    assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A8080%2Fcallback"));
    assert!(url.contains("scope=repo"));
    assert!(matches!(&effects[1], TokenEffect::WaitForCallback));
    let (state, effects) = step(
        state,
        TokenEvent::CallbackReceived {
            query: "code=auth-code&state=state-1".into(),
        },
    );
    assert!(matches!(state, TokenState::ExchangingCode(_)));
    let (url, form) = form_of(&effects);
    assert_eq!(url, TOKEN_URL);
    assert_eq!(
        form,
        "grant_type=authorization_code&code=auth-code&redirect_uri=http%3A%2F%2F127.0.0.1%3A8080%2Fcallback&code_verifier=verifier-1&client_id=client-1"
    );
    let (state, effects) = step(
        state,
        responded(200, r#"{"access_token":"at","refresh_token":"rt"}"#),
    );
    assert_eq!(completed_token(&state).refresh_token.as_deref(), Some("rt"));
    assert!(matches!(
        effects.as_slice(),
        [TokenEffect::StoreToken { token, .. }] if token.refresh_token.as_deref() == Some("rt")
    ));
}

#[test]
fn a_redirect_with_the_wrong_state_or_an_error_is_a_rejection() {
    let queries = [
        "code=auth-code&state=other",
        "error=access_denied&state=state-1",
    ];
    for query in queries {
        let (state, _) = start(input(
            client(GrantType::AuthorizationCode),
            TokenMode::ForceGrant,
            true,
        ));
        let (state, _) = step(
            state,
            TokenEvent::CallbackListening {
                redirect_uri: "http://127.0.0.1:1/callback".into(),
            },
        );
        let (state, _) = step(
            state,
            TokenEvent::CallbackReceived {
                query: query.into(),
            },
        );
        assert_eq!(failure(&state).0, &TokenFailure::Rejected, "{query}");
    }
}

#[test]
fn callback_timeout_and_listener_failure_are_transport_failures() {
    let (state, _) = start(input(
        client(GrantType::AuthorizationCode),
        TokenMode::ForceGrant,
        true,
    ));
    let (failed_listen, _) = step(
        state.clone(),
        TokenEvent::CallbackListenFailed {
            error: "port busy".into(),
        },
    );
    assert_eq!(failure(&failed_listen).0, &TokenFailure::Transport);
    let (state, _) = step(
        state,
        TokenEvent::CallbackListening {
            redirect_uri: "http://127.0.0.1:1/callback".into(),
        },
    );
    let (state, _) = step(
        state,
        TokenEvent::CallbackFailed {
            error: "timed out".into(),
        },
    );
    let (kind, message) = failure(&state);
    assert_eq!(*kind, TokenFailure::Transport);
    assert_eq!(message, "timed out");
}

#[test]
fn device_flow_shows_the_code_polls_and_honors_slow_down() {
    let (state, effects) = start(input(
        client(GrantType::DeviceCode),
        TokenMode::ForceGrant,
        true,
    ));
    let (url, form) = form_of(&effects);
    assert_eq!(url, DEVICE_URL);
    assert_eq!(form, "client_id=client-1&scope=repo");
    let (state, effects) = step(
        state,
        responded(
            200,
            r#"{"device_code":"dc","user_code":"ABCD-1234","verification_uri":"https://as.example.com/device/verify","expires_in":12,"interval":5}"#,
        ),
    );
    assert!(matches!(
        &effects[0],
        TokenEffect::ShowDeviceCode { user_code, verification_uri, .. }
            if user_code == "ABCD-1234" && verification_uri == "https://as.example.com/device/verify"
    ));
    assert!(matches!(&effects[1], TokenEffect::Sleep { seconds: 5 }));
    let (state, effects) = step(state, TokenEvent::Slept);
    let (url, form) = form_of(&effects);
    assert_eq!(url, TOKEN_URL);
    assert_eq!(
        form,
        "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code&device_code=dc&client_id=client-1"
    );
    let (state, effects) = step(state, responded(400, r#"{"error":"slow_down"}"#));
    assert!(matches!(
        effects.as_slice(),
        [TokenEffect::Sleep { seconds: 10 }]
    ));
    let (state, _) = step(state, TokenEvent::Slept);
    assert!(matches!(state, TokenState::PollingDevice { .. }));
    let (denied, _) = step(
        state.clone(),
        responded(400, r#"{"error":"access_denied"}"#),
    );
    assert_eq!(failure(&denied).0, &TokenFailure::Rejected);
    let (gateway, _) = step(state.clone(), responded(502, "<html>Bad gateway</html>"));
    assert_eq!(failure(&gateway).0, &TokenFailure::Rejected);
    let (offline, _) = step(
        state.clone(),
        TokenEvent::RequestFailed {
            error: "reset".into(),
        },
    );
    assert_eq!(failure(&offline).0, &TokenFailure::Transport);
    let (state, effects) = step(state, responded(200, r#"{"access_token":"device-token"}"#));
    assert!(matches!(
        effects.as_slice(),
        [TokenEffect::StoreToken { token, .. }] if token.access_token == "device-token"
    ));
    assert_eq!(completed_token(&state).access_token, "device-token");
}

#[test]
fn device_flow_wait_is_capped_at_the_authorization_timeout() {
    // 900 seconds of lifetime, capped at 300: polls at 100 and 200, and the
    // one at 300 would be at the deadline.
    let (mut state, _) = device_flow(
        r#"{"device_code":"dc","user_code":"u","verification_uri":"https://x","expires_in":900,"interval":100}"#,
    );
    for second in [100, 200] {
        let (next, effects) = step(state, TokenEvent::Slept);
        assert!(matches!(effects.as_slice(), [TokenEffect::PostForm { .. }]));
        let (next, _) = step(
            next,
            responded_after(second, 400, r#"{"error":"authorization_pending"}"#),
        );
        state = next;
    }
    let (kind, message) = failure(&state);
    assert_eq!(*kind, TokenFailure::Rejected);
    assert!(message.contains("expired"), "{message}");
}

#[test]
fn device_flow_does_not_poll_after_the_code_expires() {
    // expires_in 10, interval 8: the poll at 8 is pending, and the next one
    // would be at 16, after the code expired.
    let (state, effects) = device_flow(
        r#"{"device_code":"dc","user_code":"u","verification_uri":"https://x","expires_in":10,"interval":8}"#,
    );
    assert!(matches!(&effects[1], TokenEffect::Sleep { seconds: 8 }));
    let (state, _) = step(state, TokenEvent::Slept);
    let (state, effects) = step(
        state,
        responded_after(8, 400, r#"{"error":"authorization_pending"}"#),
    );
    assert!(!has_sleep(&effects), "{effects:?}");
    let (kind, message) = failure(&state);
    assert_eq!(*kind, TokenFailure::Rejected);
    assert!(message.contains("expired"), "{message}");
}

#[test]
fn device_flow_counts_the_time_a_poll_took_against_the_expiry() {
    // expires_in 10, interval 4: the poll answered at 7 (three seconds of
    // request), so the next one would be at 11.
    let (state, _) = device_flow(
        r#"{"device_code":"dc","user_code":"u","verification_uri":"https://x","expires_in":10,"interval":4}"#,
    );
    let (state, _) = step(state, TokenEvent::Slept);
    let (state, effects) = step(
        state,
        responded_after(7, 400, r#"{"error":"authorization_pending"}"#),
    );
    assert!(!has_sleep(&effects), "{effects:?}");
    assert!(failure(&state).1.contains("expired"));
}

#[test]
fn device_flow_whose_first_interval_passes_the_expiry_never_sleeps() {
    let (state, effects) = device_flow(
        r#"{"device_code":"dc","user_code":"u","verification_uri":"https://x","expires_in":5,"interval":5}"#,
    );
    assert!(!has_sleep(&effects), "{effects:?}");
    let (kind, message) = failure(&state);
    assert_eq!(*kind, TokenFailure::Rejected);
    assert!(message.contains("expired"), "{message}");
}

#[test]
fn device_flow_with_a_zero_interval_never_polls() {
    let (state, effects) = device_flow(
        r#"{"device_code":"dc","user_code":"u","verification_uri":"https://x","expires_in":60,"interval":0}"#,
    );
    assert!(!has_sleep(&effects), "{effects:?}");
    // An answer kurama cannot use, like any other malformed one.
    let (kind, message) = failure(&state);
    assert_eq!(*kind, TokenFailure::Transport);
    assert!(message.contains("interval"), "{message}");
}

#[test]
fn discovered_remote_http_token_endpoint_fails_before_the_secret_is_resolved() {
    let mut client = client(GrantType::ClientCredentials);
    client.endpoints = EndpointSource::Issuer("https://accounts.example.com/".into());
    client.client_secret = Some(SecretRef::parse("op://Agent/Item/secret").unwrap());
    let (state, _) = start(input(client, TokenMode::Reuse, false));
    let (state, _) = step(state, TokenEvent::TokenLoaded { token: None });
    let (state, effects) = step(
        state,
        responded(
            200,
            r#"{"token_endpoint":"http://accounts.example.com/oauth2/token"}"#,
        ),
    );
    assert!(
        effects
            .iter()
            .all(|effect| matches!(effect, TokenEffect::Log { .. })),
        "{effects:?}"
    );
    // A discovery document kurama cannot use, like a malformed one.
    let (kind, message) = failure(&state);
    assert_eq!(*kind, TokenFailure::Transport);
    assert!(message.contains("token_endpoint"), "{message}");
}

#[test]
fn issuer_is_discovered_once_and_the_secret_is_resolved_before_the_request() {
    let mut client = client(GrantType::ClientCredentials);
    client.endpoints = EndpointSource::Issuer("https://accounts.example.com/".into());
    client.client_secret = Some(SecretRef::parse("op://Agent/Item/secret").unwrap());
    let (state, _) = start(input(client, TokenMode::Reuse, false));
    let (state, effects) = step(state, TokenEvent::TokenLoaded { token: None });
    assert!(matches!(
        effects.as_slice(),
        [TokenEffect::Get { url }] if url == "https://accounts.example.com/.well-known/openid-configuration"
    ));
    let (state, effects) = step(
        state,
        responded(
            200,
            r#"{"token_endpoint":"https://accounts.example.com/oauth2/token"}"#,
        ),
    );
    assert!(matches!(
        effects.as_slice(),
        [TokenEffect::ResolveSecret { secret: SecretRef::OnePassword(reference) }]
            if reference == "op://Agent/Item/secret"
    ));
    let (state, effects) = step(
        state,
        TokenEvent::SecretResolved {
            secret: "s3cret".into(),
        },
    );
    let (url, form) = form_of(&effects);
    assert_eq!(url, "https://accounts.example.com/oauth2/token");
    assert_eq!(
        form,
        "grant_type=client_credentials&client_id=client-1&client_secret=s3cret&scope=repo"
    );
    assert!(matches!(state, TokenState::RequestingClientCredentials(_)));
}

#[test]
fn discovery_and_secret_failures_stop_the_workflow() {
    let mut client = client(GrantType::ClientCredentials);
    client.endpoints = EndpointSource::Issuer("https://accounts.example.com".into());
    let (state, _) = start(input(client.clone(), TokenMode::ForceGrant, false));
    let (missing_endpoint, _) = step(state.clone(), responded(200, r#"{"issuer":"x"}"#));
    assert_eq!(failure(&missing_endpoint).0, &TokenFailure::Transport);
    let (not_found, _) = step(state.clone(), responded(404, "<html>Not Found</html>"));
    let (kind, message) = failure(&not_found);
    assert_eq!(*kind, TokenFailure::Transport);
    assert!(message.contains("HTTP 404"), "{message}");
    let (unreachable, _) = step(
        state,
        TokenEvent::RequestFailed {
            error: "connection refused".into(),
        },
    );
    assert_eq!(failure(&unreachable).0, &TokenFailure::Transport);

    let mut browser_client = client.clone();
    browser_client.grant_type = GrantType::AuthorizationCode;
    let (state, _) = start(input(browser_client, TokenMode::ForceGrant, true));
    let (state, _) = step(
        state,
        responded(
            200,
            r#"{"token_endpoint":"https://accounts.example.com/token"}"#,
        ),
    );
    let (state, _) = step(
        state,
        TokenEvent::CallbackListening {
            redirect_uri: "http://127.0.0.1:1/callback".into(),
        },
    );
    let (kind, message) = failure(&state);
    assert_eq!(*kind, TokenFailure::Config);
    assert!(message.contains("authorization endpoint"), "{message}");

    client.endpoints = EndpointSource::Explicit(OAuthEndpoints {
        auth_url: None,
        token_url: TOKEN_URL.into(),
        device_auth_url: None,
    });
    client.client_secret = Some(SecretRef::parse("op://Agent/Item/secret").unwrap());
    let (state, _) = start(input(client, TokenMode::ForceGrant, false));
    let (state, _) = step(
        state,
        TokenEvent::SecretFailed {
            failure: SecretFailure::NeedsAPerson,
            error: "not signed in".into(),
        },
    );
    assert_eq!(
        failure(&state).0,
        &TokenFailure::Secret(SecretFailure::NeedsAPerson)
    );
}

#[test]
fn rejected_invalid_and_bare_status_token_responses_are_classified() {
    let (state, _) = start(input(
        client(GrantType::ClientCredentials),
        TokenMode::ForceGrant,
        false,
    ));
    let (rejected, _) = step(
        state.clone(),
        responded(
            401,
            r#"{"error":"invalid_client","error_description":"bad secret"}"#,
        ),
    );
    let (kind, message) = failure(&rejected);
    assert_eq!(*kind, TokenFailure::Rejected);
    assert!(message.contains("invalid_client: bad secret"), "{message}");
    let (invalid, _) = step(state.clone(), responded(200, "<html>"));
    assert_eq!(failure(&invalid).0, &TokenFailure::Transport);
    let (unreachable, _) = step(
        state.clone(),
        TokenEvent::RequestFailed {
            error: "timeout".into(),
        },
    );
    assert_eq!(failure(&unreachable).0, &TokenFailure::Transport);
    let (gateway, _) = step(state, responded(503, "upstream down"));
    let (kind, message) = failure(&gateway);
    assert_eq!(*kind, TokenFailure::Rejected);
    assert!(message.contains("HTTP 503: upstream down"), "{message}");
}

#[test]
fn force_refresh_refreshes_a_refreshable_token_and_regrants_otherwise() {
    let (state, _) = start(input(
        client(GrantType::ClientCredentials),
        TokenMode::ForceRefresh,
        false,
    ));
    let (refreshing, effects) = step(
        state.clone(),
        TokenEvent::TokenLoaded {
            token: Some(expired_refreshable_token()),
        },
    );
    assert!(matches!(refreshing, TokenState::Refreshing { .. }));
    assert!(form_of(&effects).1.starts_with("grant_type=refresh_token"));
    let (granting, effects) = step(
        state,
        TokenEvent::TokenLoaded {
            token: Some(valid_token()),
        },
    );
    assert!(matches!(
        granting,
        TokenState::RequestingClientCredentials(_)
    ));
    assert!(
        form_of(&effects)
            .1
            .starts_with("grant_type=client_credentials")
    );
}

#[test]
fn unexpected_events_leave_the_state_alone() {
    let (state, _) = start(input(
        client(GrantType::ClientCredentials),
        TokenMode::Reuse,
        false,
    ));
    let (same, effects) = step(state, TokenEvent::Slept);
    assert!(matches!(same, TokenState::LoadingToken(_)));
    assert!(effects.is_empty());
}

#[test]
fn debug_of_a_response_and_of_device_polling_leaves_the_secrets_out() {
    let response = format!(
        "{:?}",
        responded(200, r#"{"access_token":"secret-access-token"}"#)
    );
    assert!(
        response.contains("Responded") && response.contains("body_bytes"),
        "{response}"
    );
    assert!(!response.contains("secret-access-token"), "{response}");
    let (state, _) = device_flow(
        r#"{"device_code":"secret-device-code","user_code":"u","verification_uri":"https://x","expires_in":60,"interval":5}"#,
    );
    let polling = match &state {
        TokenState::DeviceSleeping { device, .. } => format!("{device:?}"),
        other => panic!("expected DeviceSleeping, got {other:?}"),
    };
    assert!(
        polling.contains("[REDACTED]")
            && polling.contains("interval: 5")
            && polling.contains("deadline"),
        "{polling}"
    );
    assert!(!polling.contains("secret-device-code"), "{polling}");
}
