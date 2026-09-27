use super::*;
use crate::domain::functions::error_mapping::StsErrorKind;
use crate::domain::types::CachedSession;
use chrono::DateTime;

const SERIAL: &str = "arn:aws:iam::123456789012:mfa/user";

fn input(force: bool) -> LoginInput {
    LoginInput {
        mfa_serial: SERIAL.into(),
        duration_seconds: 43200,
        force,
    }
}

fn session() -> CachedSession {
    CachedSession {
        access_key_id: "ASIASESSION123".into(),
        secret_access_key: "secret-value".into(),
        session_token: "session-token-value".into(),
        expiration: DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
    }
}

fn start(force: bool) -> (LoginState, Vec<LoginEffect>) {
    step(
        LoginState::Initial,
        LoginEvent::Start {
            input: input(force),
        },
    )
}

fn waiting_for_mfa(attempt: MfaAttempt) -> LoginState {
    LoginState::WaitingForMfa {
        input: input(false),
        attempt,
    }
}

#[test]
fn start_loads_the_cached_session() {
    let (state, effects) = start(false);
    assert!(matches!(state, LoginState::LoadingSession { .. }));
    assert!(matches!(
        effects.as_slice(),
        [LoginEffect::LoadSession { mfa_serial }] if mfa_serial == SERIAL
    ));
}

#[test]
fn force_skips_the_cache_and_asks_for_a_code() {
    let (state, effects) = start(true);
    assert!(matches!(
        state,
        LoginState::WaitingForMfa {
            attempt: MfaAttempt::First,
            ..
        }
    ));
    assert!(matches!(
        effects.as_slice(),
        [LoginEffect::GetMfaToken { mfa_serial }] if mfa_serial == SERIAL
    ));
}

#[test]
fn usable_cached_session_completes_without_sts_or_mfa() {
    let (state, _) = start(false);
    let (state, effects) = step(
        state,
        LoginEvent::SessionLoaded {
            session: Some(session()),
        },
    );
    assert!(effects.is_empty());
    assert!(matches!(
        state,
        LoginState::Completed { output: LoginOutput { reused: true, expiration } }
            if expiration == session().expiration
    ));
}

#[test]
fn missing_session_asks_for_a_code_then_requests_and_stores_a_session() {
    let (state, _) = start(false);
    let (state, effects) = step(state, LoginEvent::SessionLoaded { session: None });
    assert!(matches!(
        effects.as_slice(),
        [LoginEffect::GetMfaToken { .. }]
    ));

    let (state, effects) = step(
        state,
        LoginEvent::MfaTokenReceived {
            token: "654321".into(),
        },
    );
    assert!(matches!(
        effects.as_slice(),
        [LoginEffect::GetSessionToken { mfa_serial, token, duration_seconds: 43200 }]
            if mfa_serial == SERIAL && token == "654321"
    ));

    let (state, effects) = step(
        state,
        LoginEvent::SessionTokenReceived { session: session() },
    );
    assert!(matches!(
        effects.as_slice(),
        [LoginEffect::StoreSession { mfa_serial, .. }] if mfa_serial == SERIAL
    ));
    assert!(matches!(
        state,
        LoginState::Completed {
            output: LoginOutput { reused: false, .. }
        }
    ));
}

#[test]
fn first_invalid_code_waits_for_the_next_window_and_retries_once() {
    let state = LoginState::GettingSessionToken {
        input: input(false),
        attempt: MfaAttempt::First,
    };
    let (state, effects) = step(
        state,
        LoginEvent::SessionTokenFailed {
            error: "invalid".into(),
            kind: StsErrorKind::InvalidMfaToken,
        },
    );
    assert!(matches!(
        effects.as_slice(),
        [
            LoginEffect::Log { .. },
            LoginEffect::WaitForNextTotpWindow,
            LoginEffect::GetMfaToken { .. }
        ]
    ));
    assert!(matches!(
        state,
        LoginState::WaitingForMfa {
            attempt: MfaAttempt::Retry,
            ..
        }
    ));

    let retry = LoginState::GettingSessionToken {
        input: input(false),
        attempt: MfaAttempt::Retry,
    };
    let (state, _) = step(
        retry,
        LoginEvent::SessionTokenFailed {
            error: "invalid again".into(),
            kind: StsErrorKind::InvalidMfaToken,
        },
    );
    assert!(matches!(
        state,
        LoginState::Failed {
            kind: FailureKind::Sts(StsErrorKind::InvalidMfaToken),
            ..
        }
    ));
}

#[test]
fn rejected_session_request_fails_with_the_sts_kind() {
    let state = LoginState::GettingSessionToken {
        input: input(false),
        attempt: MfaAttempt::First,
    };
    let (state, _) = step(
        state,
        LoginEvent::SessionTokenFailed {
            error: "denied".into(),
            kind: StsErrorKind::AccessDenied,
        },
    );
    assert!(matches!(
        state,
        LoginState::Failed { kind: FailureKind::Sts(StsErrorKind::AccessDenied), error } if error == "denied"
    ));
}

#[test]
fn mfa_provider_failure_fails_the_login() {
    let (state, _) = step(
        waiting_for_mfa(MfaAttempt::First),
        LoginEvent::MfaTokenFailed {
            error: "op not signed in".into(),
        },
    );
    assert!(matches!(
        state,
        LoginState::Failed { kind: FailureKind::Mfa, error } if error.contains("op not signed in")
    ));
}

#[test]
fn unexpected_events_leave_the_state_unchanged() {
    let (state, effects) = step(
        LoginState::Initial,
        LoginEvent::SessionLoaded { session: None },
    );
    assert!(matches!(state, LoginState::Initial));
    assert!(effects.is_empty());
}

#[test]
fn debug_output_redacts_the_mfa_code_and_session() {
    let effect = LoginEffect::GetSessionToken {
        mfa_serial: SERIAL.into(),
        token: "effect-mfa-token".into(),
        duration_seconds: 900,
    };
    let event = LoginEvent::MfaTokenReceived {
        token: "event-mfa-token".into(),
    };
    let stored = LoginEffect::StoreSession {
        mfa_serial: SERIAL.into(),
        session: session(),
    };
    assert!(!format!("{effect:?}").contains("effect-mfa-token"));
    assert!(!format!("{event:?}").contains("event-mfa-token"));
    assert!(!format!("{stored:?}").contains("secret-value"));
}
