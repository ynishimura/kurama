use super::*;
use crate::domain::{Profile, types::CachedSession};
use crate::ports::{
    mfa::MockMfaProvider,
    session_cache::MockSessionCache,
    sts::{MockStsOperations, StsCredentials},
};
use std::sync::Arc;

fn profile() -> Profile {
    Profile::new("test")
        .with_role_arn_raw("arn:aws:iam::123456789012:role/TestRole")
        .with_mfa_serial_raw("arn:aws:iam::123456789012:mfa/user")
}
fn effect() -> AssumeRoleEffect {
    AssumeRoleEffect {
        profile_name: "test".into(),
        mfa_token: None,
        readonly: false,
    }
}

#[tokio::test]
async fn tea_cache_hit_skips_manual_mfa_screen() {
    let mut cache = MockSessionCache::new();
    cache.expect_load().times(1).returning(|_| {
        Ok(Some(CachedSession {
            access_key_id: "session-key".into(),
            secret_access_key: "secret".into(),
            session_token: "token".into(),
            expiration: chrono::Utc::now() + chrono::Duration::hours(1),
        }))
    });
    let mut sts = MockStsOperations::new();
    sts.expect_assume_role()
        .times(1)
        .withf(|r| r.source_credentials.is_some())
        .returning(|_| {
            Ok(StsCredentials {
                access_key_id: "role-key".into(),
                secret_access_key: "secret".into(),
                session_token: "token".into(),
                expiration: None,
            })
        });
    let mut rt = Runtime::test(sts, MockMfaProvider::new());
    rt.session_cache = Arc::new(cache);
    let result = assume_role_with_profile(&rt, profile(), effect())
        .await
        .unwrap();
    assert_eq!(result.access_key_id, "role-key");
}

#[tokio::test]
async fn tea_cache_miss_checks_provider_before_requesting_manual_mfa() {
    let mut cache = MockSessionCache::new();
    cache.expect_load().times(1).returning(|_| Ok(None));
    let mut mfa = MockMfaProvider::new();
    mfa.expect_get_token().times(1).returning(|_| Ok(None));
    let mut rt = Runtime::test(MockStsOperations::new(), mfa);
    rt.session_cache = Arc::new(cache);
    assert!(
        matches!(assume_role_with_profile(&rt, profile(), effect()).await,
        Err(AssumeRoleError::MfaRequired { serial }) if serial.ends_with("mfa/user"))
    );
}
