//! Whether a cached GetSessionToken session is still worth reusing.

use crate::domain::types::CachedSession;
use chrono::{DateTime, Duration, Utc};

pub const SESSION_REUSE_MARGIN: Duration = Duration::minutes(1);

/// A session only needs to outlive the AssumeRole call it signs.
pub fn is_session_usable(session: &CachedSession, now: DateTime<Utc>) -> bool {
    session.expiration - now > SESSION_REUSE_MARGIN
}

#[cfg(test)]
mod tests {
    use super::*;

    #[rstest::rstest]
    #[case(3600, true)]
    #[case(61, true)]
    #[case(60, false)]
    #[case(59, false)]
    #[case(0, false)]
    #[case(-1, false)]
    fn session_cache_usability(#[case] remaining: i64, #[case] expected: bool) {
        let now = DateTime::from_timestamp(1800000000, 0).unwrap();
        let session = CachedSession {
            access_key_id: "session-key".into(),
            secret_access_key: "secret".into(),
            session_token: "token".into(),
            expiration: now + Duration::seconds(remaining),
        };
        assert_eq!(is_session_usable(&session, now), expected);
    }
}
