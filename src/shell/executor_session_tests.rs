use super::*;
use crate::domain::{Profile, types::CachedSession};
use crate::ports::{
    mfa::MockMfaProvider,
    session_cache::{MockSessionCache, SessionCacheError},
    sts::{MockStsOperations, StsCredentials, StsError},
};
use crate::workflows::assume_role::SessionCacheSettings;
use chrono::{Duration, Utc};
use mockall::Sequence;
use std::sync::Arc;

fn input(cache: bool) -> AssumeRoleInput {
    AssumeRoleInput {
        profile: Profile::new("test")
            .with_role_arn_raw("arn:aws:iam::123456789012:role/TestRole")
            .with_mfa_serial_raw("arn:aws:iam::123456789012:mfa/user"),
        mfa_token: None,
        readonly: false,
        session_name_config: Default::default(),
        session_cache: SessionCacheSettings {
            enabled: cache,
            duration_seconds: 43200,
        },
    }
}
fn session(seconds: i64) -> CachedSession {
    CachedSession {
        access_key_id: "session-key".into(),
        secret_access_key: "session-secret".into(),
        session_token: "session-token".into(),
        expiration: Utc::now() + Duration::seconds(seconds),
    }
}
fn sts_credentials(key: &str) -> StsCredentials {
    StsCredentials {
        access_key_id: key.into(),
        secret_access_key: "secret".into(),
        session_token: "token".into(),
        expiration: Some(Utc::now() + Duration::hours(12)),
    }
}
fn runtime(sts: MockStsOperations, mfa: MockMfaProvider, cache: MockSessionCache) -> Runtime {
    let mut rt = Runtime::test(sts, mfa);
    rt.session_cache = Arc::new(cache);
    rt
}
fn role_succeeds(sts: &mut MockStsOperations) {
    sts.expect_assume_role()
        .times(1)
        .withf(|r| {
            r.source_credentials
                .as_ref()
                .is_some_and(|c| c.access_key_id == "session-key")
                && r.mfa_token.is_none()
        })
        .returning(|_| Ok(sts_credentials("role-key")));
}

#[tokio::test]
async fn cache_hit_skips_mfa_provider() {
    let mut cache = MockSessionCache::new();
    cache
        .expect_load()
        .withf(|k| k.ends_with("mfa/user"))
        .times(1)
        .returning(|_| Ok(Some(session(3600))));
    let mut sts = MockStsOperations::new();
    role_succeeds(&mut sts);
    let out = execute_assume_role(&runtime(sts, MockMfaProvider::new(), cache), input(true))
        .await
        .unwrap();
    assert_eq!(out.credentials.access_key_id(), "role-key");
}

#[rstest::rstest]
#[case("missing")]
#[case("expired")]
#[case("margin")]
#[case("backend-error")]
#[tokio::test]
async fn cache_miss_obtains_and_stores_session(#[case] scenario: &'static str) {
    let mut cache = MockSessionCache::new();
    cache
        .expect_load()
        .times(1)
        .returning(move |_| match scenario {
            "expired" => Ok(Some(session(-1))),
            "margin" => Ok(Some(session(30))),
            "backend-error" => Err(SessionCacheError::Backend("locked".into())),
            _ => Ok(None),
        });
    let mut sequence = Sequence::new();
    let mut mfa = MockMfaProvider::new();
    mfa.expect_get_token()
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_| Ok(Some("123456".into())));
    let mut sts = MockStsOperations::new();
    sts.expect_get_session_token()
        .times(1)
        .in_sequence(&mut sequence)
        .withf(|r| {
            r.mfa_token == "123456"
                && r.duration_seconds == 43200
                && r.mfa_serial.ends_with("mfa/user")
        })
        .returning(|_| Ok(sts_credentials("session-key")));
    cache
        .expect_store()
        .times(1)
        .in_sequence(&mut sequence)
        .withf(|k, s| k.ends_with("mfa/user") && s.access_key_id == "session-key")
        .returning(|_, _| Ok(()));
    sts.expect_assume_role()
        .times(1)
        .in_sequence(&mut sequence)
        .withf(|r| r.source_credentials.is_some() && r.mfa_token.is_none())
        .returning(|_| Ok(sts_credentials("role-key")));
    let out = execute_assume_role(&runtime(sts, mfa, cache), input(true))
        .await
        .unwrap();
    assert_eq!(out.credentials.access_key_id(), "role-key");
}

#[tokio::test]
async fn cache_store_error_still_uses_fresh_session() {
    let mut cache = MockSessionCache::new();
    cache.expect_load().returning(|_| Ok(None));
    cache
        .expect_store()
        .times(1)
        .returning(|_, _| Err(SessionCacheError::Backend("locked".into())));
    let mut mfa = MockMfaProvider::new();
    mfa.expect_get_token()
        .returning(|_| Ok(Some("123456".into())));
    let mut sts = MockStsOperations::new();
    sts.expect_get_session_token()
        .times(1)
        .returning(|_| Ok(sts_credentials("session-key")));
    role_succeeds(&mut sts);
    assert!(
        execute_assume_role(&runtime(sts, mfa, cache), input(true))
            .await
            .is_ok()
    );
}

#[rstest::rstest]
#[case(true)]
#[case(false)]
#[tokio::test(start_paused = true)]
async fn invalid_totp_waits_before_fetching_one_new_code(#[case] cache_enabled: bool) {
    let start = tokio::time::Instant::now();
    let mut sequence = Sequence::new();
    let mut cache = MockSessionCache::new();
    if cache_enabled {
        cache.expect_load().times(1).returning(|_| Ok(None));
        cache.expect_store().times(1).returning(|_, _| Ok(()));
    }
    let mut mfa = MockMfaProvider::new();
    mfa.expect_get_token()
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_| Ok(Some("111111".into())));
    mfa.expect_get_token()
        .times(1)
        .in_sequence(&mut sequence)
        .returning(move |_| {
            let elapsed = start.elapsed().as_secs();
            assert!(
                (2..=31).contains(&elapsed),
                "must wait exactly one TOTP rollover: {elapsed}"
            );
            Ok(Some("222222".into()))
        });
    let mut sts = MockStsOperations::new();
    if cache_enabled {
        sts.expect_get_session_token()
            .withf(|r| r.mfa_token == "111111")
            .times(1)
            .returning(|_| Err(StsError::InvalidMfaToken));
        sts.expect_get_session_token()
            .withf(|r| r.mfa_token == "222222")
            .times(1)
            .returning(|_| Ok(sts_credentials("session-key")));
        role_succeeds(&mut sts);
    } else {
        sts.expect_assume_role()
            .withf(|r| r.mfa_token.as_deref() == Some("111111") && r.source_credentials.is_none())
            .times(1)
            .returning(|_| Err(StsError::InvalidMfaToken));
        sts.expect_assume_role()
            .withf(|r| r.mfa_token.as_deref() == Some("222222"))
            .times(1)
            .returning(|_| Ok(sts_credentials("role-key")));
    }
    assert!(
        execute_assume_role(&runtime(sts, mfa, cache), input(cache_enabled))
            .await
            .is_ok()
    );
}

#[tokio::test(start_paused = true)]
async fn second_invalid_totp_is_reported_without_third_attempt() {
    let mut sts = MockStsOperations::new();
    sts.expect_assume_role()
        .times(2)
        .returning(|_| Err(StsError::InvalidMfaToken));
    let mut mfa = MockMfaProvider::new();
    mfa.expect_get_token()
        .times(2)
        .returning(|_| Ok(Some("111111".into())));
    let result =
        execute_assume_role(&runtime(sts, mfa, MockSessionCache::new()), input(false)).await;
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Invalid MFA token")
    );
}

#[rstest::rstest]
#[case(StsError::InvalidCredentials)]
#[case(StsError::AccessDenied("age condition".into()))]
#[tokio::test]
async fn cached_session_rejection_refreshes_even_if_removal_fails(#[case] error: StsError) {
    let mut sequence = Sequence::new();
    let mut cache = MockSessionCache::new();
    cache.expect_load().returning(|_| Ok(Some(session(3600))));
    let mut sts = MockStsOperations::new();
    sts.expect_assume_role()
        .times(1)
        .in_sequence(&mut sequence)
        .returning(move |_| Err(error.clone()));
    cache
        .expect_remove()
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_| Err(SessionCacheError::Backend("locked".into())));
    let mut mfa = MockMfaProvider::new();
    mfa.expect_get_token()
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_| Ok(Some("123456".into())));
    sts.expect_get_session_token()
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_| Ok(sts_credentials("session-key")));
    cache
        .expect_store()
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    sts.expect_assume_role()
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_| Err(StsError::AccessDenied("still denied".into())));
    let result = execute_assume_role(&runtime(sts, mfa, cache), input(true)).await;
    assert!(result.unwrap_err().to_string().contains("still denied"));
}

#[tokio::test]
async fn session_token_without_expiration_is_rejected() {
    let mut cache = MockSessionCache::new();
    cache.expect_load().returning(|_| Ok(None));
    let mut sts = MockStsOperations::new();
    sts.expect_get_session_token().returning(|_| {
        let mut c = sts_credentials("session-key");
        c.expiration = None;
        Ok(c)
    });
    let mut mfa = MockMfaProvider::new();
    mfa.expect_get_token()
        .returning(|_| Ok(Some("123456".into())));
    let result = execute_assume_role(&runtime(sts, mfa, cache), input(true)).await;
    assert!(result.unwrap_err().to_string().contains("expiration"));
}

#[tokio::test]
async fn cache_miss_with_no_provider_requests_manual_mfa() {
    let mut cache = MockSessionCache::new();
    cache.expect_load().returning(|_| Ok(None));
    let mut mfa = MockMfaProvider::new();
    mfa.expect_get_token().returning(|_| Ok(None));
    let result =
        execute_assume_role(&runtime(MockStsOperations::new(), mfa, cache), input(true)).await;
    assert!(
        matches!(result, Err(ExecutorError::MfaRequired { serial }) if serial.ends_with("mfa/user"))
    );
}
