//! Pure state transitions; clocks and I/O belong to the shell.
use super::types::*;
use crate::domain::functions::{
    AssumeRoleInput as DomainInput, build_assume_role_params, error_mapping::StsErrorKind,
};
use crate::ports::sts::SourceCredentials;
use crate::workflows::common::{FailureKind, LogLevel, MfaAttempt};

pub fn step(
    state: AssumeRoleState,
    event: AssumeRoleEvent,
) -> (AssumeRoleState, Vec<AssumeRoleEffect>) {
    use AssumeRoleEvent as Event;
    use AssumeRoleState as State;
    match (state, event) {
        (State::Initial, Event::Start { input })
            if !input.profile.can_assume_role() && input.readonly =>
        {
            failed(
                format!(
                    "profile '{}' has no role_arn: --readonly attaches ReadOnlyAccess to a role session, and an IAM user's long-term keys take no session policy",
                    input.profile.name()
                ),
                FailureKind::InvalidProfile,
            )
        }
        (State::Initial, Event::Start { input })
            if !input.profile.can_assume_role() && !input.profile.requires_mfa() =>
        {
            (
                State::ReadingKeys { input },
                vec![AssumeRoleEffect::ReadProfileKeys],
            )
        }
        (State::ReadingKeys { input }, Event::ProfileKeysRead { credentials }) => {
            complete_as_iam_user(&input, credentials)
        }
        (State::ReadingKeys { .. }, Event::ProfileKeysFailed { error, kind }) => {
            failed(error, FailureKind::Sts(kind))
        }
        (State::Initial, Event::Start { mut input }) => {
            let token = input.mfa_token.take();
            let (state, effects) = if input.profile.requires_mfa() {
                if let Some(token) = token {
                    mfa_received(input, token, MfaAttempt::First)
                } else if input.session_cache.enabled {
                    let mfa_serial = serial(&input);
                    (
                        State::LoadingSession { input },
                        vec![AssumeRoleEffect::LoadSession { mfa_serial }],
                    )
                } else {
                    wait_for_mfa(input, MfaAttempt::First)
                }
            } else {
                assume(input, token, CredentialSource::Default, MfaAttempt::First)
            };
            (state, effects)
        }
        (State::LoadingSession { input }, Event::SessionLoaded { session }) => match session {
            Some(session) if !input.profile.can_assume_role() => {
                complete_as_iam_user(&input, session_credentials(&session))
            }
            Some(session) => assume(
                input,
                None,
                CredentialSource::Session {
                    session,
                    fresh: false,
                },
                MfaAttempt::First,
            ),
            None => wait_for_mfa(input, MfaAttempt::First),
        },
        (State::WaitingForMfa { input, attempt }, Event::MfaTokenReceived { token }) => {
            mfa_received(input, token, attempt)
        }
        (State::WaitingForMfa { .. }, Event::MfaTokenFailed { error }) => failed(
            format!("Failed to get MFA token: {error}"),
            FailureKind::Mfa,
        ),
        (
            State::GettingSessionToken { input, attempt },
            Event::SessionTokenReceived { session },
        ) => {
            let store = input
                .session_cache
                .enabled
                .then(|| AssumeRoleEffect::StoreSession {
                    mfa_serial: serial(&input),
                    session: session.clone(),
                });
            let (state, mut effects) = if input.profile.can_assume_role() {
                assume(
                    input,
                    None,
                    CredentialSource::Session {
                        session,
                        fresh: true,
                    },
                    attempt,
                )
            } else {
                complete_as_iam_user(&input, session_credentials(&session))
            };
            effects.splice(0..0, store);
            (state, effects)
        }
        (
            State::GettingSessionToken {
                input,
                attempt: MfaAttempt::First,
            },
            Event::SessionTokenFailed {
                kind: StsErrorKind::InvalidMfaToken,
                ..
            },
        ) => retry_mfa(input),
        (State::GettingSessionToken { .. }, Event::SessionTokenFailed { error, kind }) => {
            failed(error, FailureKind::Sts(kind))
        }
        (
            State::Assuming {
                input,
                session_name,
                ..
            },
            Event::AssumeRoleSucceeded { credentials },
        ) => (
            State::Completed {
                output: AssumeRoleOutput {
                    credentials,
                    profile_name: input.profile.name().to_string(),
                    session_name: Some(session_name),
                },
            },
            vec![AssumeRoleEffect::Log {
                level: LogLevel::Info,
                message: format!(
                    "Successfully assumed role for profile '{}'",
                    input.profile.name()
                ),
            }],
        ),
        (
            State::Assuming {
                input,
                source: CredentialSource::Default,
                attempt: MfaAttempt::First,
                ..
            },
            Event::AssumeRoleFailed {
                kind: StsErrorKind::InvalidMfaToken,
                ..
            },
        ) if input.profile.requires_mfa() => retry_mfa(input),
        (
            State::Assuming {
                input,
                source: CredentialSource::Session { fresh: false, .. },
                ..
            },
            Event::AssumeRoleFailed {
                kind: StsErrorKind::InvalidCredentials | StsErrorKind::AccessDenied,
                ..
            },
        ) => {
            let invalidate = AssumeRoleEffect::InvalidateSession {
                mfa_serial: serial(&input),
            };
            let (state, mut effects) = wait_for_mfa(input, MfaAttempt::First);
            effects.insert(0, invalidate);
            effects.insert(
                0,
                AssumeRoleEffect::Log {
                    level: LogLevel::Warn,
                    message: "# cached MFA session rejected, refreshing".into(),
                },
            );
            (state, effects)
        }
        (State::Assuming { .. }, Event::AssumeRoleFailed { error, kind }) => {
            failed(error, FailureKind::Sts(kind))
        }
        (state, _) => (state, vec![]),
    }
}

fn serial(input: &AssumeRoleInput) -> String {
    // These states are only entered after requires_mfa() succeeds.
    input
        .profile
        .mfa_serial_raw()
        .expect("MFA workflow requires a serial")
        .to_string()
}

fn wait_for_mfa(
    input: AssumeRoleInput,
    attempt: MfaAttempt,
) -> (AssumeRoleState, Vec<AssumeRoleEffect>) {
    let mfa_serial = serial(&input);
    (
        AssumeRoleState::WaitingForMfa { input, attempt },
        vec![AssumeRoleEffect::GetMfaToken { mfa_serial }],
    )
}

fn retry_mfa(input: AssumeRoleInput) -> (AssumeRoleState, Vec<AssumeRoleEffect>) {
    let (state, mut effects) = wait_for_mfa(input, MfaAttempt::Retry);
    effects.insert(0, AssumeRoleEffect::WaitForNextTotpWindow);
    effects.insert(
        0,
        AssumeRoleEffect::Log {
            level: LogLevel::Warn,
            message: "MFA code rejected; retrying with the next code".into(),
        },
    );
    (state, effects)
}

fn mfa_received(
    input: AssumeRoleInput,
    token: String,
    attempt: MfaAttempt,
) -> (AssumeRoleState, Vec<AssumeRoleEffect>) {
    // An IAM user profile has no AssumeRole to hand the code to: its MFA
    // session is the credential, cached or not.
    if input.session_cache.enabled || !input.profile.can_assume_role() {
        let effect = AssumeRoleEffect::GetSessionToken {
            mfa_serial: serial(&input),
            token,
            duration_seconds: input.session_cache.duration_seconds,
        };
        (
            AssumeRoleState::GettingSessionToken { input, attempt },
            vec![effect],
        )
    } else {
        assume(input, Some(token), CredentialSource::Default, attempt)
    }
}

fn assume(
    input: AssumeRoleInput,
    token: Option<String>,
    source: CredentialSource,
    attempt: MfaAttempt,
) -> (AssumeRoleState, Vec<AssumeRoleEffect>) {
    let params = build_assume_role_params(&DomainInput {
        profile: &input.profile,
        mfa_token: token,
        readonly: input.readonly,
        session_name_config: input.session_name_config.clone(),
    });
    match params {
        Ok(params) => {
            let mut request = params.to_request();
            if let CredentialSource::Session { session, .. } = &source {
                request.source_credentials = Some(SourceCredentials {
                    access_key_id: session.access_key_id.clone(),
                    secret_access_key: session.secret_access_key.clone(),
                    session_token: session.session_token.clone(),
                });
            }
            (
                AssumeRoleState::Assuming {
                    input,
                    session_name: params.session_name,
                    source,
                    attempt,
                },
                vec![AssumeRoleEffect::AssumeRole { request }],
            )
        }
        Err(error) => failed(error, FailureKind::InvalidProfile),
    }
}

/// The credentials of an MFA session, as the workflow hands them out.
fn session_credentials(
    session: &crate::domain::types::CachedSession,
) -> crate::domain::Credentials {
    crate::domain::Credentials::new(
        session.access_key_id.clone(),
        session.secret_access_key.clone(),
        Some(session.session_token.clone()),
        Some(session.expiration),
    )
}

/// An IAM user profile (no `role_arn`) ends with its own credentials: the
/// long-term keys, or the MFA session they got.
fn complete_as_iam_user(
    input: &AssumeRoleInput,
    credentials: crate::domain::Credentials,
) -> (AssumeRoleState, Vec<AssumeRoleEffect>) {
    let profile_name = input.profile.name().to_string();
    let message =
        format!("Using the IAM user credentials of profile '{profile_name}' (no role_arn)");
    (
        AssumeRoleState::Completed {
            output: AssumeRoleOutput {
                credentials,
                profile_name,
                session_name: None,
            },
        },
        vec![AssumeRoleEffect::Log {
            level: LogLevel::Info,
            message,
        }],
    )
}

fn failed(error: String, kind: FailureKind) -> (AssumeRoleState, Vec<AssumeRoleEffect>) {
    (
        AssumeRoleState::Failed {
            error: error.clone(),
            kind,
        },
        vec![AssumeRoleEffect::Log {
            level: LogLevel::Warn,
            message: error,
        }],
    )
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
