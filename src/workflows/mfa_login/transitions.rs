//! Pure state transitions of the MFA login workflow.

use super::types::*;
use crate::domain::functions::error_mapping::StsErrorKind;
use crate::workflows::common::{FailureKind, LogLevel, MfaAttempt};

pub fn step(state: LoginState, event: LoginEvent) -> (LoginState, Vec<LoginEffect>) {
    use LoginEvent as Event;
    use LoginState as State;
    match (state, event) {
        (State::Initial, Event::Start { input }) if input.force => {
            wait_for_mfa(input, MfaAttempt::First)
        }
        (State::Initial, Event::Start { input }) => {
            let mfa_serial = input.mfa_serial.clone();
            (
                State::LoadingSession { input },
                vec![LoginEffect::LoadSession { mfa_serial }],
            )
        }
        (
            State::LoadingSession { .. },
            Event::SessionLoaded {
                session: Some(session),
            },
        ) => (
            State::Completed {
                output: LoginOutput {
                    expiration: session.expiration,
                    reused: true,
                },
            },
            vec![],
        ),
        (State::LoadingSession { input }, Event::SessionLoaded { session: None }) => {
            wait_for_mfa(input, MfaAttempt::First)
        }
        (State::WaitingForMfa { input, attempt }, Event::MfaTokenReceived { token }) => {
            let effect = LoginEffect::GetSessionToken {
                mfa_serial: input.mfa_serial.clone(),
                token,
                duration_seconds: input.duration_seconds,
            };
            (State::GettingSessionToken { input, attempt }, vec![effect])
        }
        (State::WaitingForMfa { .. }, Event::MfaTokenFailed { error }) => failed(
            format!("Failed to get MFA token: {error}"),
            FailureKind::Mfa,
        ),
        (State::GettingSessionToken { input, .. }, Event::SessionTokenReceived { session }) => (
            State::Completed {
                output: LoginOutput {
                    expiration: session.expiration,
                    reused: false,
                },
            },
            vec![LoginEffect::StoreSession {
                mfa_serial: input.mfa_serial,
                session,
            }],
        ),
        (
            State::GettingSessionToken {
                input,
                attempt: MfaAttempt::First,
            },
            Event::SessionTokenFailed {
                kind: StsErrorKind::InvalidMfaToken,
                ..
            },
        ) => {
            let (state, mut effects) = wait_for_mfa(input, MfaAttempt::Retry);
            effects.splice(
                0..0,
                [
                    LoginEffect::Log {
                        level: LogLevel::Warn,
                        message: "MFA code rejected; retrying with the next code".into(),
                    },
                    LoginEffect::WaitForNextTotpWindow,
                ],
            );
            (state, effects)
        }
        (State::GettingSessionToken { .. }, Event::SessionTokenFailed { error, kind }) => {
            failed(error, FailureKind::Sts(kind))
        }
        (state, _) => (state, vec![]),
    }
}

fn wait_for_mfa(input: LoginInput, attempt: MfaAttempt) -> (LoginState, Vec<LoginEffect>) {
    let mfa_serial = input.mfa_serial.clone();
    (
        LoginState::WaitingForMfa { input, attempt },
        vec![LoginEffect::GetMfaToken { mfa_serial }],
    )
}

fn failed(error: String, kind: FailureKind) -> (LoginState, Vec<LoginEffect>) {
    (
        LoginState::Failed {
            error: error.clone(),
            kind,
        },
        vec![LoginEffect::Log {
            level: LogLevel::Warn,
            message: error,
        }],
    )
}
