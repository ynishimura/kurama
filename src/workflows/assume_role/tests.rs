use super::*;
use crate::domain::functions::{SessionNameConfig, error_mapping::StsErrorKind};
use crate::domain::types::CachedSession;
use crate::domain::{Credentials, Profile};
use chrono::DateTime;
use rstest::rstest;

fn input(mfa: bool, cache: bool) -> AssumeRoleInput {
    let mut profile = Profile::new("test")
        .with_role_arn_raw("arn:aws:iam::123456789012:role/TestRole")
        .with_duration_seconds(7200);
    if mfa {
        profile = profile.with_mfa_serial_raw("arn:aws:iam::123456789012:mfa/user");
    }
    AssumeRoleInput {
        profile,
        mfa_token: None,
        readonly: true,
        session_name_config: SessionNameConfig::new().with_template("custom-{profile}"),
        session_cache: SessionCacheSettings {
            enabled: cache,
            duration_seconds: 129600,
        },
    }
}

fn session() -> CachedSession {
    CachedSession {
        access_key_id: "ASIASESSION123".into(),
        secret_access_key: "secret-value".into(),
        session_token: "session-token-value".into(),
        expiration: DateTime::from_timestamp(1800000000, 0).unwrap(),
    }
}

fn request(effects: &[AssumeRoleEffect]) -> &crate::ports::AssumeRoleRequest {
    effects
        .iter()
        .find_map(|e| match e {
            AssumeRoleEffect::AssumeRole { request } => Some(request),
            _ => None,
        })
        .expect("expected AssumeRole effect")
}

fn assert_role_options(effects: &[AssumeRoleEffect]) {
    let req = request(effects);
    assert_eq!(req.duration_seconds, Some(7200));
    assert_eq!(req.session_name, "custom-test");
    assert_eq!(
        req.policy_arns.as_ref().unwrap(),
        &["arn:aws:iam::aws:policy/ReadOnlyAccess"]
    );
}

#[rstest]
#[case(false, false, "Assuming")]
#[case(false, true, "Assuming")]
#[case(true, false, "WaitingForMfa")]
#[case(true, true, "LoadingSession")]
fn start_routes_by_mfa_and_cache(#[case] mfa: bool, #[case] cache: bool, #[case] expected: &str) {
    let (state, effects) = step(
        AssumeRoleState::Initial,
        AssumeRoleEvent::Start {
            input: input(mfa, cache),
        },
    );
    assert!(format!("{state:?}").starts_with(expected));
    if mfa && cache {
        assert!(effects.iter().any(|e| matches!(e, AssumeRoleEffect::LoadSession { mfa_serial } if mfa_serial.ends_with("mfa/user"))));
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, AssumeRoleEffect::GetMfaToken { .. }))
        );
    } else if mfa {
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, AssumeRoleEffect::GetMfaToken { .. }))
        );
        assert!(!effects.iter().any(|e| matches!(
            e,
            AssumeRoleEffect::LoadSession { .. } | AssumeRoleEffect::GetSessionToken { .. }
        )));
    } else {
        assert_role_options(&effects);
    }
}

#[test]
fn cache_hit_assumes_with_redacted_session_and_no_totp() {
    let (state, effects) = step(
        AssumeRoleState::LoadingSession {
            input: input(true, true),
        },
        AssumeRoleEvent::SessionLoaded {
            session: Some(session()),
        },
    );
    assert!(matches!(
        state,
        AssumeRoleState::Assuming {
            source: CredentialSource::Session { fresh: false, .. },
            ..
        }
    ));
    let req = request(&effects);
    assert!(req.mfa_serial.is_none() && req.mfa_token.is_none());
    assert_eq!(
        req.source_credentials.as_ref().unwrap().access_key_id,
        "ASIASESSION123"
    );
    assert_role_options(&effects);
    let debug = format!("{state:?} {effects:?}");
    assert!(!debug.contains("secret-value") && !debug.contains("session-token-value"));
}

#[test]
fn cache_miss_requests_mfa() {
    let (state, effects) = step(
        AssumeRoleState::LoadingSession {
            input: input(true, true),
        },
        AssumeRoleEvent::SessionLoaded { session: None },
    );
    assert!(matches!(
        state,
        AssumeRoleState::WaitingForMfa {
            attempt: MfaAttempt::First,
            ..
        }
    ));
    assert!(matches!(
        effects.as_slice(),
        [AssumeRoleEffect::GetMfaToken { .. }]
    ));
}

#[rstest]
#[case(false)]
#[case(true)]
fn received_mfa_routes_to_sts_operation(#[case] cache: bool) {
    let (state, effects) = step(
        AssumeRoleState::WaitingForMfa {
            input: input(true, cache),
            attempt: MfaAttempt::Retry,
        },
        AssumeRoleEvent::MfaTokenReceived {
            token: "654321".into(),
        },
    );
    if cache {
        assert!(matches!(
            state,
            AssumeRoleState::GettingSessionToken {
                attempt: MfaAttempt::Retry,
                ..
            }
        ));
        assert!(
            matches!(effects.as_slice(), [AssumeRoleEffect::GetSessionToken { token, duration_seconds: 129600, .. }] if token == "654321")
        );
    } else {
        assert!(matches!(
            state,
            AssumeRoleState::Assuming {
                attempt: MfaAttempt::Retry,
                source: CredentialSource::Default,
                ..
            }
        ));
        assert_eq!(request(&effects).mfa_token.as_deref(), Some("654321"));
        assert_role_options(&effects);
    }
}

#[rstest]
#[case(false)]
#[case(true)]
fn supplied_mfa_token_is_consumed_without_prompt(#[case] cache: bool) {
    let mut input = input(true, cache);
    input.mfa_token = Some("123456".into());
    let (state, effects) = step(AssumeRoleState::Initial, AssumeRoleEvent::Start { input });
    assert!(!effects.iter().any(|e| matches!(
        e,
        AssumeRoleEffect::GetMfaToken { .. } | AssumeRoleEffect::LoadSession { .. }
    )));
    if cache {
        assert!(matches!(state, AssumeRoleState::GettingSessionToken { .. }));
    } else {
        assert_eq!(request(&effects).mfa_token.as_deref(), Some("123456"));
    }
}

#[test]
fn fresh_session_is_stored_before_assuming() {
    let (state, effects) = step(
        AssumeRoleState::GettingSessionToken {
            input: input(true, true),
            attempt: MfaAttempt::Retry,
        },
        AssumeRoleEvent::SessionTokenReceived { session: session() },
    );
    assert!(matches!(
        state,
        AssumeRoleState::Assuming {
            source: CredentialSource::Session { fresh: true, .. },
            attempt: MfaAttempt::Retry,
            ..
        }
    ));
    assert!(
        matches!(effects.first(), Some(AssumeRoleEffect::StoreSession { mfa_serial, .. }) if mfa_serial.ends_with("mfa/user"))
    );
    assert!(request(&effects).mfa_token.is_none());
    assert_role_options(&effects);
}

fn assert_retry(state: AssumeRoleState, effects: &[AssumeRoleEffect]) {
    assert!(matches!(
        state,
        AssumeRoleState::WaitingForMfa {
            attempt: MfaAttempt::Retry,
            ..
        }
    ));
    assert!(matches!(
        effects,
        [
            AssumeRoleEffect::Log { .. },
            AssumeRoleEffect::WaitForNextTotpWindow,
            AssumeRoleEffect::GetMfaToken { .. }
        ]
    ));
}

#[rstest]
#[case(MfaAttempt::First, StsErrorKind::InvalidMfaToken, true)]
#[case(MfaAttempt::Retry, StsErrorKind::InvalidMfaToken, false)]
#[case(MfaAttempt::First, StsErrorKind::AccessDenied, false)]
#[case(MfaAttempt::First, StsErrorKind::InvalidCredentials, false)]
#[case(MfaAttempt::First, StsErrorKind::ServiceError, false)]
fn session_token_failure_retries_once(
    #[case] attempt: MfaAttempt,
    #[case] kind: StsErrorKind,
    #[case] retry: bool,
) {
    let (state, effects) = step(
        AssumeRoleState::GettingSessionToken {
            input: input(true, true),
            attempt,
        },
        AssumeRoleEvent::SessionTokenFailed {
            error: "STS failure".into(),
            kind,
        },
    );
    if retry {
        assert_retry(state, &effects);
    } else {
        assert!(matches!(state, AssumeRoleState::Failed { error, .. } if error == "STS failure"));
        assert!(matches!(effects.as_slice(), [AssumeRoleEffect::Log { .. }]));
    }
}

#[rstest]
#[case(
    "default",
    true,
    MfaAttempt::First,
    StsErrorKind::InvalidMfaToken,
    "retry"
)]
#[case(
    "default",
    true,
    MfaAttempt::Retry,
    StsErrorKind::InvalidMfaToken,
    "fail"
)]
#[case(
    "default",
    false,
    MfaAttempt::First,
    StsErrorKind::InvalidMfaToken,
    "fail"
)]
#[case("default", true, MfaAttempt::First, StsErrorKind::AccessDenied, "fail")]
#[case(
    "cached",
    true,
    MfaAttempt::First,
    StsErrorKind::InvalidCredentials,
    "refresh"
)]
#[case(
    "cached",
    true,
    MfaAttempt::First,
    StsErrorKind::AccessDenied,
    "refresh"
)]
#[case("cached", true, MfaAttempt::First, StsErrorKind::ServiceError, "fail")]
#[case(
    "cached",
    true,
    MfaAttempt::First,
    StsErrorKind::InvalidMfaToken,
    "fail"
)]
#[case(
    "fresh",
    true,
    MfaAttempt::First,
    StsErrorKind::InvalidCredentials,
    "fail"
)]
#[case("fresh", true, MfaAttempt::First, StsErrorKind::AccessDenied, "fail")]
#[case(
    "fresh",
    true,
    MfaAttempt::First,
    StsErrorKind::InvalidMfaToken,
    "fail"
)]
fn assume_failure_is_bounded(
    #[case] source: &str,
    #[case] mfa: bool,
    #[case] attempt: MfaAttempt,
    #[case] kind: StsErrorKind,
    #[case] action: &str,
) {
    let source = match source {
        "default" => CredentialSource::Default,
        value => CredentialSource::Session {
            session: session(),
            fresh: value == "fresh",
        },
    };
    let (state, effects) = step(
        AssumeRoleState::Assuming {
            input: input(mfa, true),
            session_name: "custom-test".into(),
            source,
            attempt,
        },
        AssumeRoleEvent::AssumeRoleFailed {
            error: "last STS error".into(),
            kind,
        },
    );
    match action {
        "retry" => assert_retry(state, &effects),
        "refresh" => {
            assert!(matches!(
                state,
                AssumeRoleState::WaitingForMfa {
                    attempt: MfaAttempt::First,
                    ..
                }
            ));
            assert!(
                matches!(effects.as_slice(), [AssumeRoleEffect::Log { message, .. }, AssumeRoleEffect::InvalidateSession { .. }, AssumeRoleEffect::GetMfaToken { .. }] if message.contains("cached MFA session rejected"))
            );
        }
        _ => {
            assert!(
                matches!(state, AssumeRoleState::Failed { error, .. } if error == "last STS error")
            )
        }
    }
}

#[test]
fn mfa_provider_failure_is_terminal() {
    let (state, effects) = step(
        AssumeRoleState::WaitingForMfa {
            input: input(true, true),
            attempt: MfaAttempt::First,
        },
        AssumeRoleEvent::MfaTokenFailed {
            error: "provider unavailable".into(),
        },
    );
    assert!(
        matches!(state, AssumeRoleState::Failed { error, .. } if error.contains("provider unavailable"))
    );
    assert!(matches!(effects.as_slice(), [AssumeRoleEffect::Log { .. }]));
}

#[test]
fn successful_assumption_returns_output() {
    let (state, _) = step(
        AssumeRoleState::Assuming {
            input: input(false, false),
            session_name: "custom-test".into(),
            source: CredentialSource::Default,
            attempt: MfaAttempt::First,
        },
        AssumeRoleEvent::AssumeRoleSucceeded {
            credentials: Credentials::new(
                "role-key".into(),
                "secret".into(),
                Some("token".into()),
                None,
            ),
        },
    );
    let AssumeRoleState::Completed { output } = state else {
        panic!("not completed")
    };
    assert_eq!(output.profile_name, "test");
    assert_eq!(output.session_name.as_deref(), Some("custom-test"));
    assert_eq!(output.credentials.access_key_id(), "role-key");
}

/// A profile without `role_arn`: an IAM user whose own keys are the credentials.
fn iam_user(mfa: bool, cache: bool) -> AssumeRoleInput {
    let mut profile = Profile::new("uploader");
    if mfa {
        profile = profile.with_mfa_serial_raw("arn:aws:iam::123456789012:mfa/user");
    }
    AssumeRoleInput {
        profile,
        readonly: false,
        ..input(false, cache)
    }
}

fn completed(state: &AssumeRoleState) -> &AssumeRoleOutput {
    match state {
        AssumeRoleState::Completed { output } => output,
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[test]
fn an_iam_user_without_mfa_reads_its_keys_and_calls_no_sts() {
    let (state, effects) = step(
        AssumeRoleState::Initial,
        AssumeRoleEvent::Start {
            input: iam_user(false, true),
        },
    );
    assert!(matches!(state, AssumeRoleState::ReadingKeys { .. }));
    assert!(matches!(effects[..], [AssumeRoleEffect::ReadProfileKeys]));

    let keys = Credentials::new("AKIAUSER".into(), "user-secret".into(), None, None);
    let (state, effects) = step(
        state,
        AssumeRoleEvent::ProfileKeysRead {
            credentials: keys.clone(),
        },
    );
    let output = completed(&state);
    assert_eq!(output.credentials.access_key_id(), "AKIAUSER");
    assert_eq!(output.credentials.session_token(), None);
    assert_eq!(output.profile_name, "uploader");
    assert!(output.session_name.is_none());
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, AssumeRoleEffect::AssumeRole { .. }))
    );
}

#[test]
fn an_iam_user_whose_keys_cannot_be_read_fails_with_the_sts_kind() {
    let (state, _) = step(
        AssumeRoleState::ReadingKeys {
            input: iam_user(false, true),
        },
        AssumeRoleEvent::ProfileKeysFailed {
            error: "no keys".into(),
            kind: StsErrorKind::ServiceError,
        },
    );
    assert!(matches!(
        state,
        AssumeRoleState::Failed {
            kind: FailureKind::Sts(StsErrorKind::ServiceError),
            ..
        }
    ));
}

#[test]
fn an_iam_user_with_a_cached_session_uses_it_without_any_call() {
    let (state, effects) = step(
        AssumeRoleState::LoadingSession {
            input: iam_user(true, true),
        },
        AssumeRoleEvent::SessionLoaded {
            session: Some(session()),
        },
    );
    let output = completed(&state);
    assert_eq!(output.credentials.access_key_id(), "ASIASESSION123");
    assert_eq!(
        output.credentials.session_token(),
        Some("session-token-value")
    );
    assert!(
        effects
            .iter()
            .all(|e| matches!(e, AssumeRoleEffect::Log { .. }))
    );
}

#[rstest]
#[case(true)]
#[case(false)]
fn an_iam_user_with_mfa_gets_a_session_token_and_stores_it_only_with_the_cache(
    #[case] cache: bool,
) {
    let (state, effects) = step(
        AssumeRoleState::WaitingForMfa {
            input: iam_user(true, cache),
            attempt: MfaAttempt::First,
        },
        AssumeRoleEvent::MfaTokenReceived {
            token: "123456".into(),
        },
    );
    assert!(matches!(state, AssumeRoleState::GettingSessionToken { .. }));
    assert!(matches!(
        effects[..],
        [AssumeRoleEffect::GetSessionToken {
            duration_seconds: 129600,
            ..
        }]
    ));

    let (state, effects) = step(
        state,
        AssumeRoleEvent::SessionTokenReceived { session: session() },
    );
    assert_eq!(
        completed(&state).credentials.access_key_id(),
        "ASIASESSION123"
    );
    assert_eq!(
        effects
            .iter()
            .any(|e| matches!(e, AssumeRoleEffect::StoreSession { .. })),
        cache
    );
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, AssumeRoleEffect::AssumeRole { .. }))
    );
}

#[test]
fn an_iam_user_cannot_take_the_readonly_policy() {
    let (state, effects) = step(
        AssumeRoleState::Initial,
        AssumeRoleEvent::Start {
            input: AssumeRoleInput {
                readonly: true,
                ..iam_user(true, true)
            },
        },
    );
    let AssumeRoleState::Failed { error, kind } = state else {
        panic!("expected Failed");
    };
    assert_eq!(kind, FailureKind::InvalidProfile);
    assert!(error.contains("no role_arn"), "{error}");
    assert!(
        effects
            .iter()
            .all(|e| matches!(e, AssumeRoleEffect::Log { .. }))
    );
}

#[test]
fn unrelated_event_leaves_state_unchanged() {
    let (state, effects) = step(
        AssumeRoleState::Initial,
        AssumeRoleEvent::SessionLoaded { session: None },
    );
    assert!(matches!(state, AssumeRoleState::Initial));
    assert!(effects.is_empty());
}
