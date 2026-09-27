//! Interpreter of the MFA login workflow (`kurama login`).
//!
//! It shares GetSessionToken and the TOTP-window wait with the AssumeRole
//! interpreter in `super::executor`. Unlike AssumeRole, a failed cache write
//! fails the login: storing the session is the whole point.

use anyhow::Result;

use super::executor::{
    ExecutorError, emit_log, load_usable_session, request_session_token, wait_for_next_totp_window,
};
use super::runtime::Runtime;
use crate::workflows::mfa_login::{
    LoginEffect, LoginEvent, LoginInput, LoginOutput, LoginState, step,
};

/// Run the MFA login workflow to completion.
///
/// Errors keep their type in the chain: `ExecutorError` for MFA and STS
/// failures, `SessionCacheError` when the session cannot be stored.
pub async fn execute_mfa_login(runtime: &Runtime, input: LoginInput) -> Result<LoginOutput> {
    let mut state = LoginState::Initial;
    let mut event = LoginEvent::Start { input };

    loop {
        let (next_state, effects) = step(state, event);

        let mut effect_event = None;
        for effect in effects {
            match effect {
                LoginEffect::Log { level, message } => emit_log(level, &message),
                LoginEffect::LoadSession { mfa_serial } => {
                    let session =
                        load_usable_session(runtime, &mfa_serial, chrono::Utc::now()).await;
                    effect_event = Some(LoginEvent::SessionLoaded { session });
                }
                LoginEffect::GetMfaToken { mfa_serial } => {
                    effect_event = Some(match runtime.mfa_provider.get_token(&mfa_serial).await {
                        Ok(Some(token)) => LoginEvent::MfaTokenReceived { token },
                        Ok(None) => {
                            return Err(ExecutorError::MfaRequired { serial: mfa_serial }.into());
                        }
                        Err(error) => LoginEvent::MfaTokenFailed {
                            error: error.to_string(),
                        },
                    });
                }
                LoginEffect::GetSessionToken {
                    mfa_serial,
                    token,
                    duration_seconds,
                } => {
                    effect_event = Some(
                        match request_session_token(runtime, mfa_serial, token, duration_seconds)
                            .await
                        {
                            Ok(session) => LoginEvent::SessionTokenReceived { session },
                            Err((kind, error)) => LoginEvent::SessionTokenFailed { error, kind },
                        },
                    );
                }
                LoginEffect::StoreSession {
                    mfa_serial,
                    session,
                } => runtime.session_cache.store(&mfa_serial, &session).await?,
                LoginEffect::WaitForNextTotpWindow => wait_for_next_totp_window().await,
            }
        }

        state = next_state;
        event = match &state {
            LoginState::Completed { output } => return Ok(output.clone()),
            LoginState::Failed { error, kind } => {
                return Err(ExecutorError::from_failure(kind, error).into());
            }
            _ => effect_event.ok_or_else(|| {
                ExecutorError::UnexpectedState(format!("Unexpected state: {state:?}"))
            })?,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::functions::error_mapping::StsErrorKind;
    use crate::domain::types::CachedSession;
    use crate::ports::{
        SessionCacheError,
        mfa::MockMfaProvider,
        session_cache::MockSessionCache,
        sts::{MockStsOperations, StsCredentials, StsError},
    };
    use std::sync::Arc;

    const SERIAL: &str = "arn:aws:iam::123456789012:mfa/user";

    fn input(force: bool) -> LoginInput {
        LoginInput {
            mfa_serial: SERIAL.into(),
            duration_seconds: 43200,
            force,
        }
    }

    fn session_credentials() -> StsCredentials {
        StsCredentials {
            access_key_id: "ASIASESSION".into(),
            secret_access_key: "secret".into(),
            session_token: "token".into(),
            expiration: Some(chrono::Utc::now() + chrono::Duration::hours(12)),
        }
    }

    fn runtime(sts: MockStsOperations, mfa: MockMfaProvider, cache: MockSessionCache) -> Runtime {
        let mut runtime = Runtime::test(sts, mfa);
        runtime.session_cache = Arc::new(cache);
        runtime
    }

    #[tokio::test]
    async fn valid_cached_session_is_kept_without_mfa_or_sts() {
        let mut cache = MockSessionCache::new();
        cache.expect_load().times(1).returning(|_| {
            Ok(Some(CachedSession {
                access_key_id: "ASIASESSION".into(),
                secret_access_key: "secret".into(),
                session_token: "token".into(),
                expiration: chrono::Utc::now() + chrono::Duration::hours(3),
            }))
        });
        let output = execute_mfa_login(
            &runtime(MockStsOperations::new(), MockMfaProvider::new(), cache),
            input(false),
        )
        .await
        .unwrap();
        assert!(output.reused);
    }

    #[tokio::test]
    async fn new_session_is_requested_with_the_code_and_stored() {
        let mut cache = MockSessionCache::new();
        cache
            .expect_store()
            .times(1)
            .withf(|key, session| key == SERIAL && session.access_key_id == "ASIASESSION")
            .returning(|_, _| Ok(()));
        let mut mfa = MockMfaProvider::new();
        mfa.expect_get_token()
            .times(1)
            .returning(|_| Ok(Some("123456".into())));
        let mut sts = MockStsOperations::new();
        sts.expect_get_session_token()
            .times(1)
            .withf(|r| r.mfa_token == "123456" && r.duration_seconds == 43200)
            .returning(|_| Ok(session_credentials()));

        let output = execute_mfa_login(&runtime(sts, mfa, cache), input(true))
            .await
            .unwrap();
        assert!(!output.reused);
    }

    #[tokio::test(start_paused = true)]
    async fn login_retries_invalid_totp_once_and_caches_session() {
        let start = tokio::time::Instant::now();
        let mut sequence = mockall::Sequence::new();
        let mut mfa = MockMfaProvider::new();
        let mut sts = MockStsOperations::new();
        let mut cache = MockSessionCache::new();
        mfa.expect_get_token()
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_| Ok(Some("111111".into())));
        sts.expect_get_session_token()
            .times(1)
            .in_sequence(&mut sequence)
            .withf(|r| r.mfa_token == "111111")
            .returning(|_| Err(StsError::InvalidMfaToken));
        mfa.expect_get_token()
            .times(1)
            .in_sequence(&mut sequence)
            .returning(move |_| {
                let elapsed = start.elapsed().as_secs();
                assert!(
                    (2..=31).contains(&elapsed),
                    "the second code is read after the next TOTP window: {elapsed}s"
                );
                Ok(Some("222222".into()))
            });
        sts.expect_get_session_token()
            .times(1)
            .in_sequence(&mut sequence)
            .withf(|r| r.mfa_token == "222222" && r.duration_seconds == 43200)
            .returning(|_| Ok(session_credentials()));
        cache
            .expect_store()
            .times(1)
            .in_sequence(&mut sequence)
            .withf(|key, session| key == SERIAL && session.access_key_id == "ASIASESSION")
            .returning(|_, _| Ok(()));

        let output = execute_mfa_login(&runtime(sts, mfa, cache), input(true))
            .await
            .unwrap();
        assert!(!output.reused);
    }

    #[tokio::test]
    async fn failed_store_fails_the_login_with_the_cache_error() {
        let mut cache = MockSessionCache::new();
        cache
            .expect_store()
            .returning(|_, _| Err(SessionCacheError::Backend("locked".into())));
        let mut mfa = MockMfaProvider::new();
        mfa.expect_get_token()
            .returning(|_| Ok(Some("123456".into())));
        let mut sts = MockStsOperations::new();
        sts.expect_get_session_token()
            .returning(|_| Ok(session_credentials()));

        let error = execute_mfa_login(&runtime(sts, mfa, cache), input(true))
            .await
            .unwrap_err();
        assert!(
            error.downcast_ref::<SessionCacheError>().is_some(),
            "{error:#}"
        );
    }

    #[tokio::test]
    async fn missing_mfa_provider_is_mfa_required() {
        let mut mfa = MockMfaProvider::new();
        mfa.expect_get_token().returning(|_| Ok(None));

        let error = execute_mfa_login(
            &runtime(MockStsOperations::new(), mfa, MockSessionCache::new()),
            input(true),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<ExecutorError>(),
            Some(ExecutorError::MfaRequired { serial }) if serial == SERIAL
        ));
    }

    #[tokio::test]
    async fn rejected_session_request_is_an_sts_failure() {
        let mut mfa = MockMfaProvider::new();
        mfa.expect_get_token()
            .returning(|_| Ok(Some("123456".into())));
        let mut sts = MockStsOperations::new();
        sts.expect_get_session_token()
            .returning(|_| Err(StsError::AccessDenied("denied".into())));

        let error = execute_mfa_login(&runtime(sts, mfa, MockSessionCache::new()), input(true))
            .await
            .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<ExecutorError>(),
            Some(ExecutorError::StsFailed {
                kind: StsErrorKind::AccessDenied,
                ..
            })
        ));
    }
}
