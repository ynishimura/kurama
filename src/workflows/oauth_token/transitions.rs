//! Pure state transitions of the OAuth token workflow.

use std::time::{Duration, Instant};

use super::types::*;
use crate::domain::functions::oauth::{
    AUTHORIZATION_TIMEOUT_SECS, ClientAuth, DevicePoll, TokenError, TokenGrant,
    build_authorization_url, classify_device_poll, device_authorization_form, discovery_url,
    is_token_usable, parse_callback_query, parse_device_authorization, parse_discovery,
    parse_token_response, pkce_challenge, token_request_form,
};
use crate::domain::types::{EndpointSource, GrantType, OAuthEndpoints, OAuthToken, Secret};
use crate::workflows::common::LogLevel;

/// Seconds added to the polling interval on `slow_down` (RFC 8628 section 3.5).
const SLOW_DOWN_INCREMENT_SECS: u64 = 5;

pub fn step(state: TokenState, event: TokenEvent) -> (TokenState, Vec<TokenEffect>) {
    use TokenEvent as Event;
    use TokenState as State;
    match (state, event) {
        (State::Initial, Event::Start { input }) => {
            let session = Session {
                input,
                endpoints: None,
                secret: None,
            };
            if session.input.mode == TokenMode::ForceGrant {
                return start_grant(session);
            }
            let key = session.input.client.name.clone();
            (
                State::LoadingToken(session),
                vec![TokenEffect::LoadToken { key }],
            )
        }
        (State::LoadingToken(session), Event::TokenLoaded { token }) => after_load(session, token),
        (State::Discovering { mut session, plan }, Event::Responded { status, body, .. }) => {
            if !(200..300).contains(&status) {
                return failed(
                    TokenFailure::Transport,
                    format!("discovery answered HTTP {status}"),
                );
            }
            match parse_discovery(&body) {
                Ok(endpoints) => {
                    session.endpoints = Some(endpoints);
                    need_secret(session, plan)
                }
                Err(message) => failed(TokenFailure::Transport, message),
            }
        }
        (State::Discovering { .. }, Event::RequestFailed { error }) => failed(
            TokenFailure::Transport,
            format!("discovery request failed: {error}"),
        ),
        (State::ResolvingSecret { mut session, plan }, Event::SecretResolved { secret }) => {
            session.secret = Some(secret);
            run_plan(session, plan)
        }
        (State::ResolvingSecret { .. }, Event::SecretFailed { failure, error }) => {
            failed(TokenFailure::Secret(failure), error)
        }
        (
            State::Refreshing { session, previous },
            Event::Responded {
                status,
                body,
                received_at,
                ..
            },
        ) => match parse_token_response(status, &body, received_at, Some(&previous)) {
            Ok(token) => store(session, token),
            // The server judged the refresh token: the grant starts over.
            Err(error @ TokenError::Rejected { .. }) => {
                let (state, mut effects) = start_grant(session);
                effects.insert(
                    0,
                    TokenEffect::Log {
                        level: LogLevel::Warn,
                        message: format!("Token refresh rejected: {error}"),
                    },
                );
                (state, effects)
            }
            // A bare status (a gateway 503) is not a judgement: the refresh
            // token stays, and a person is not needed.
            Err(error @ TokenError::Status { .. }) => failed(
                TokenFailure::Rejected,
                format!("token refresh failed: {error}"),
            ),
            Err(TokenError::Invalid(message)) => failed(TokenFailure::Transport, message),
        },
        (State::Refreshing { .. }, Event::RequestFailed { error }) => failed(
            TokenFailure::Transport,
            format!("token refresh failed: {error}"),
        ),
        (State::ListeningForCallback(session), Event::CallbackListening { redirect_uri }) => {
            let client = &session.input.client;
            let Some(auth_url) = session
                .endpoints
                .as_ref()
                .and_then(|endpoints| endpoints.auth_url.clone())
            else {
                return failed(
                    TokenFailure::Config,
                    "the authorization server names no authorization endpoint".into(),
                );
            };
            match build_authorization_url(
                &auth_url,
                &client.client_id,
                &redirect_uri,
                &client.scopes,
                &session.input.state,
                &pkce_challenge(&session.input.pkce_verifier),
            ) {
                Ok(url) => (
                    State::WaitingForCallback {
                        session,
                        redirect_uri,
                    },
                    vec![TokenEffect::Authorize { url }, TokenEffect::WaitForCallback],
                ),
                Err(message) => failed(TokenFailure::Config, message),
            }
        }
        (State::ListeningForCallback(_), Event::CallbackListenFailed { error }) => failed(
            TokenFailure::Transport,
            format!("could not listen for the browser redirect: {error}"),
        ),
        (
            State::WaitingForCallback {
                session,
                redirect_uri,
            },
            Event::CallbackReceived { query },
        ) => match parse_callback_query(&query, &session.input.state) {
            Ok(code) => {
                let (url, form) = token_post(
                    &session,
                    &TokenGrant::AuthorizationCode {
                        code: &code,
                        redirect_uri: &redirect_uri,
                        code_verifier: &session.input.pkce_verifier,
                    },
                );
                (
                    State::ExchangingCode(session),
                    vec![TokenEffect::PostForm { url, form }],
                )
            }
            Err(message) => failed(
                TokenFailure::Rejected,
                format!("authorization was not granted: {message}"),
            ),
        },
        (State::WaitingForCallback { .. }, Event::CallbackFailed { error }) => {
            failed(TokenFailure::Transport, error)
        }
        (
            State::ExchangingCode(session) | State::RequestingClientCredentials(session),
            Event::Responded {
                status,
                body,
                received_at,
                ..
            },
        ) => match parse_token_response(status, &body, received_at, None) {
            Ok(token) => store(session, token),
            Err(error) => {
                token_failed("the authorization server rejected the token request", error)
            }
        },
        (
            State::RequestingDeviceCode(session),
            Event::Responded {
                status,
                body,
                received_instant,
                ..
            },
        ) => match parse_device_authorization(status, &body) {
            Ok(device) => {
                let polling = DevicePolling {
                    device_code: device.device_code,
                    interval: device.interval,
                    // The person gets the code's lifetime, at most the
                    // authorization wait every grant has.
                    deadline: received_instant
                        + Duration::from_secs(device.expires_in.min(AUTHORIZATION_TIMEOUT_SECS)),
                };
                let (state, mut effects) =
                    sleep_before_next_poll(session, polling, received_instant);
                if matches!(state, State::DeviceSleeping { .. }) {
                    effects.insert(
                        0,
                        TokenEffect::ShowDeviceCode {
                            verification_uri: device.verification_uri,
                            user_code: device.user_code,
                            verification_uri_complete: device.verification_uri_complete,
                        },
                    );
                }
                (state, effects)
            }
            Err(error) => token_failed(
                "the authorization server rejected the device request",
                error,
            ),
        },
        (State::DeviceSleeping { session, device }, Event::Slept) => {
            let (url, form) = token_post(
                &session,
                &TokenGrant::DeviceCode {
                    device_code: &device.device_code,
                },
            );
            (
                State::PollingDevice { session, device },
                vec![TokenEffect::PostForm { url, form }],
            )
        }
        (
            State::PollingDevice {
                session,
                mut device,
            },
            Event::Responded {
                status,
                body,
                received_at,
                received_instant,
            },
        ) => match classify_device_poll(status, &body, received_at) {
            Ok(DevicePoll::Token(token)) => store(session, token),
            Ok(DevicePoll::Pending) => sleep_before_next_poll(session, device, received_instant),
            Ok(DevicePoll::SlowDown) => {
                device.interval += SLOW_DOWN_INCREMENT_SECS;
                sleep_before_next_poll(session, device, received_instant)
            }
            Err(error) => token_failed("the device authorization was not granted", error),
        },
        (
            State::ExchangingCode(_)
            | State::RequestingClientCredentials(_)
            | State::RequestingDeviceCode(_)
            | State::PollingDevice { .. },
            Event::RequestFailed { error },
        ) => failed(
            TokenFailure::Transport,
            format!("token request failed: {error}"),
        ),
        (state, _) => (state, vec![]),
    }
}

fn after_load(session: Session, token: Option<OAuthToken>) -> (TokenState, Vec<TokenEffect>) {
    let now = session.input.now;
    match token {
        Some(token) if session.input.mode == TokenMode::Reuse && is_token_usable(&token, now) => (
            TokenState::Completed {
                output: TokenOutput {
                    token,
                    reused: true,
                    cache_error: None,
                },
            },
            vec![],
        ),
        Some(token) if token.refresh_token.is_some() => {
            let (state, mut effects) = need_endpoints(session, Plan::Refresh(token));
            effects.insert(
                0,
                TokenEffect::Log {
                    level: LogLevel::Info,
                    message: "# Refreshing the stored token".into(),
                },
            );
            (state, effects)
        }
        _ => start_grant(session),
    }
}

/// Run the grant, unless it needs a person and there is no terminal.
fn start_grant(session: Session) -> (TokenState, Vec<TokenEffect>) {
    let client = &session.input.client;
    if client.grant_type.needs_human() && !session.input.interactive {
        return failed(
            TokenFailure::LoginRequired,
            format!(
                "profile '{}' has no usable token and getting one needs a person",
                client.name
            ),
        );
    }
    need_endpoints(session, Plan::Grant)
}

fn need_endpoints(mut session: Session, plan: Plan) -> (TokenState, Vec<TokenEffect>) {
    if session.endpoints.is_some() {
        return need_secret(session, plan);
    }
    match &session.input.client.endpoints {
        EndpointSource::Issuer(issuer) => {
            let url = discovery_url(issuer);
            (
                TokenState::Discovering { session, plan },
                vec![TokenEffect::Get { url }],
            )
        }
        EndpointSource::Explicit(endpoints) => {
            session.endpoints = Some(endpoints.clone());
            need_secret(session, plan)
        }
    }
}

fn need_secret(session: Session, plan: Plan) -> (TokenState, Vec<TokenEffect>) {
    match &session.input.client.client_secret {
        Some(secret) if session.secret.is_none() => {
            let secret = secret.clone();
            (
                TokenState::ResolvingSecret { session, plan },
                vec![TokenEffect::ResolveSecret { secret }],
            )
        }
        _ => run_plan(session, plan),
    }
}

fn run_plan(session: Session, plan: Plan) -> (TokenState, Vec<TokenEffect>) {
    let scopes = session.input.client.scopes.clone();
    match plan {
        Plan::Refresh(previous) => {
            let refresh_token = previous.refresh_token.clone().unwrap_or_default();
            let (url, form) = token_post(
                &session,
                &TokenGrant::RefreshToken {
                    refresh_token: &refresh_token,
                },
            );
            (
                TokenState::Refreshing { session, previous },
                vec![TokenEffect::PostForm { url, form }],
            )
        }
        Plan::Grant => match session.input.client.grant_type {
            GrantType::AuthorizationCode => {
                let port = session.input.client.redirect_port;
                (
                    TokenState::ListeningForCallback(session),
                    vec![TokenEffect::ListenForCallback { port }],
                )
            }
            GrantType::DeviceCode => {
                let Some(url) = endpoints(&session).device_auth_url.clone() else {
                    return failed(
                        TokenFailure::Config,
                        "the authorization server names no device authorization endpoint".into(),
                    );
                };
                let form = device_authorization_form(&client_auth(&session), &scopes);
                (
                    TokenState::RequestingDeviceCode(session),
                    vec![TokenEffect::PostForm { url, form }],
                )
            }
            GrantType::ClientCredentials => {
                let (url, form) = token_post(&session, &TokenGrant::ClientCredentials);
                (
                    TokenState::RequestingClientCredentials(session),
                    vec![TokenEffect::PostForm { url, form }],
                )
            }
        },
    }
}

/// Sleep for the interval, unless the poll after it would be at or after the
/// device code's deadline: that poll could only send an expired code.
fn sleep_before_next_poll(
    session: Session,
    device: DevicePolling,
    now: Instant,
) -> (TokenState, Vec<TokenEffect>) {
    if now + Duration::from_secs(device.interval) >= device.deadline {
        return failed(
            TokenFailure::Rejected,
            "the device code expired before it was entered".into(),
        );
    }
    let seconds = device.interval;
    (
        TokenState::DeviceSleeping { session, device },
        vec![TokenEffect::Sleep { seconds }],
    )
}

/// The executor stores before it reads `Completed`, and only logs a store
/// failure (the keychain is a cache), so the workflow has no state for it;
/// the executor reports it through `TokenOutput::cache_error` instead.
fn store(session: Session, token: OAuthToken) -> (TokenState, Vec<TokenEffect>) {
    (
        TokenState::Completed {
            output: TokenOutput {
                token: token.clone(),
                reused: false,
                cache_error: None,
            },
        },
        vec![TokenEffect::StoreToken {
            key: session.input.client.name.clone(),
            token,
        }],
    )
}

/// The failure becomes the `error[...]` line, so the log stays at debug
/// level: piped stderr would otherwise print the reason twice.
fn failed(kind: TokenFailure, message: String) -> (TokenState, Vec<TokenEffect>) {
    (
        TokenState::Failed {
            kind,
            message: message.clone(),
        },
        vec![TokenEffect::Log {
            level: LogLevel::Debug,
            message,
        }],
    )
}

/// The endpoints are always known once a plan runs.
fn endpoints(session: &Session) -> &OAuthEndpoints {
    session
        .endpoints
        .as_ref()
        .expect("endpoints are resolved before a plan runs")
}

/// The token endpoint and the form of one grant to POST to it.
fn token_post(session: &Session, grant: &TokenGrant) -> (String, Vec<(String, String)>) {
    let form = token_request_form(grant, &client_auth(session), &session.input.client.scopes);
    (endpoints(session).token_url.clone(), form)
}

/// A token request the server refused, or answered with something that is
/// not a token: `context` says which request it was.
fn token_failed(context: &str, error: TokenError) -> (TokenState, Vec<TokenEffect>) {
    match error {
        TokenError::Rejected { .. } | TokenError::Status { .. } => {
            failed(TokenFailure::Rejected, format!("{context}: {error}"))
        }
        TokenError::Invalid(message) => failed(TokenFailure::Transport, message),
    }
}

fn client_auth(session: &Session) -> ClientAuth<'_> {
    ClientAuth {
        client_id: &session.input.client.client_id,
        client_secret: session.secret.as_ref().map(Secret::expose),
    }
}
