//! What `kurama status` reports for a profile: its MFA session state in the
//! local cache, whether the shell holds its credentials, and whether
//! `kurama env` would need a person to act.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use super::session_cache::SESSION_REUSE_MARGIN;
use crate::domain::types::Profile;

/// What the session cache returned for one MFA device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionLookup {
    /// A session is stored; it may already be expired.
    Cached { expires_at: DateTime<Utc> },
    /// The cache could not be read (for example a locked keychain).
    Unreadable,
}

/// MFA session state of one profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    /// The profile has no `mfa_serial`: there is nothing to log in to.
    NotRequired,
    /// `[aws.session_cache] enabled = false`.
    CacheDisabled,
    /// The cache could not be read.
    Unreadable,
    /// No usable session is cached for the profile's MFA device.
    Missing,
    /// A usable session is cached.
    Valid { expires_at: DateTime<Utc> },
}

impl SessionState {
    /// Stable identifier for JSON output.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NotRequired => "not_required",
            Self::CacheDisabled => "cache_disabled",
            Self::Unreadable => "unreadable",
            Self::Missing => "missing",
            Self::Valid { .. } => "valid",
        }
    }
}

/// Status of one profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileStatus {
    pub name: String,
    pub role_arn: Option<String>,
    pub region: Option<String>,
    pub mfa_serial: Option<String>,
    /// The shell holds this profile's credentials (`KURAMA_AWS`).
    pub active: bool,
    pub session: SessionState,
    /// `kurama env` would stop with exit code 3: MFA is required, no usable
    /// session is cached and no MFA provider is enabled.
    pub needs_human: bool,
}

/// Everything besides the profile that decides its status.
pub struct StatusInputs<'a> {
    /// Keyed by MFA serial; a device without an entry has no cached session.
    pub sessions: &'a BTreeMap<String, SessionLookup>,
    pub active_profile: Option<&'a str>,
    pub session_cache_enabled: bool,
    pub mfa_provider_enabled: bool,
    pub now: DateTime<Utc>,
}

pub fn profile_status(profile: &Profile, inputs: &StatusInputs) -> ProfileStatus {
    let session = match profile.mfa_serial_raw() {
        None => SessionState::NotRequired,
        Some(_) if !inputs.session_cache_enabled => SessionState::CacheDisabled,
        Some(serial) => match inputs.sessions.get(serial) {
            Some(SessionLookup::Cached { expires_at })
                if *expires_at - inputs.now > SESSION_REUSE_MARGIN =>
            {
                SessionState::Valid {
                    expires_at: *expires_at,
                }
            }
            Some(SessionLookup::Unreadable) => SessionState::Unreadable,
            _ => SessionState::Missing,
        },
    };
    let needs_human = !matches!(
        session,
        SessionState::NotRequired | SessionState::Valid { .. }
    ) && !inputs.mfa_provider_enabled;
    ProfileStatus {
        name: profile.name().to_string(),
        role_arn: profile.role_arn_raw().map(str::to_string),
        region: profile.region_raw().map(str::to_string),
        mfa_serial: profile.mfa_serial_raw().map(str::to_string),
        active: inputs.active_profile == Some(profile.name()),
        session,
        needs_human,
    }
}

/// Short text for a session column: `-`, `none`, `valid (11h 59m)`, ...
pub fn describe_session(session: &SessionState, now: DateTime<Utc>) -> String {
    match session {
        SessionState::NotRequired => "-".to_string(),
        SessionState::CacheDisabled => "cache disabled".to_string(),
        SessionState::Unreadable => "unreadable".to_string(),
        SessionState::Missing => "none".to_string(),
        SessionState::Valid { expires_at } => {
            format!("valid ({})", describe_remaining(*expires_at, now))
        }
    }
}

/// Time left until `expires_at`: `11h 59m`, `59m`, `0m` once expired.
pub fn describe_remaining(expires_at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let minutes = (expires_at - now).num_minutes().max(0);
    if minutes >= 60 {
        format!("{}h {}m", minutes / 60, minutes % 60)
    } else {
        format!("{minutes}m")
    }
}

/// How close a credential is to its end, for a screen that shows the time
/// left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expiry {
    Later,
    /// Less than `EXPIRY_WARNING` left.
    Soon,
    Expired,
}

/// Under this much time left a credential is about to expire.
pub const EXPIRY_WARNING: chrono::TimeDelta = chrono::TimeDelta::minutes(15);

/// Where `expires_at` stands at `now`.
fn expiry(expires_at: DateTime<Utc>, now: DateTime<Utc>) -> Expiry {
    let left = expires_at - now;
    if left <= chrono::TimeDelta::zero() {
        Expiry::Expired
    } else if left < EXPIRY_WARNING {
        Expiry::Soon
    } else {
        Expiry::Later
    }
}

/// `<label> 1h 30m`, or `<label> expired`, with where it stands.
pub fn describe_time_left(
    label: &str,
    expires_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> (String, Expiry) {
    let state = expiry(expires_at, now);
    let text = match state {
        Expiry::Expired => format!("{label} expired"),
        Expiry::Soon | Expiry::Later => {
            format!("{label} {}", describe_remaining(expires_at, now))
        }
    };
    (text, state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn time_left_turns_soon_under_15_minutes_and_expired_at_0() {
        let at =
            |seconds: i64| describe_time_left("MFA", now() + Duration::seconds(seconds), now());
        assert_eq!(at(90 * 60), ("MFA 1h 30m".to_string(), Expiry::Later));
        assert_eq!(at(15 * 60), ("MFA 15m".to_string(), Expiry::Later));
        assert_eq!(at(15 * 60 - 1), ("MFA 14m".to_string(), Expiry::Soon));
        assert_eq!(at(1), ("MFA 0m".to_string(), Expiry::Soon));
        assert_eq!(at(0), ("MFA expired".to_string(), Expiry::Expired));
        assert_eq!(at(-60), ("MFA expired".to_string(), Expiry::Expired));
    }

    const SERIAL: &str = "arn:aws:iam::123456789012:mfa/user";

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000, 0).unwrap()
    }

    fn mfa_profile() -> Profile {
        Profile::new("ops")
            .with_role_arn_raw("arn:aws:iam::123456789012:role/Ops")
            .with_region_raw("ap-northeast-1")
            .with_mfa_serial_raw(SERIAL)
    }

    fn status_with(
        profile: &Profile,
        lookup: Option<SessionLookup>,
        cache_enabled: bool,
        provider_enabled: bool,
    ) -> ProfileStatus {
        let sessions: BTreeMap<String, SessionLookup> = lookup
            .into_iter()
            .map(|lookup| (SERIAL.to_string(), lookup))
            .collect();
        profile_status(
            profile,
            &StatusInputs {
                sessions: &sessions,
                active_profile: Some("ops"),
                session_cache_enabled: cache_enabled,
                mfa_provider_enabled: provider_enabled,
                now: now(),
            },
        )
    }

    #[test]
    fn profile_without_mfa_needs_no_login() {
        let status = status_with(&Profile::new("dev"), None, true, false);
        assert_eq!(status.session, SessionState::NotRequired);
        assert!(!status.needs_human);
        assert!(!status.active);
    }

    #[test]
    fn cached_session_outside_the_reuse_margin_is_valid() {
        let expires_at = now() + Duration::hours(2);
        let status = status_with(
            &mfa_profile(),
            Some(SessionLookup::Cached { expires_at }),
            true,
            false,
        );
        assert_eq!(status.session, SessionState::Valid { expires_at });
        assert!(!status.needs_human);
        assert!(status.active);
        assert_eq!(status.mfa_serial.as_deref(), Some(SERIAL));
        assert_eq!(status.region.as_deref(), Some("ap-northeast-1"));
    }

    #[rstest::rstest]
    #[case(60)]
    #[case(-3600)]
    fn session_inside_the_reuse_margin_is_missing(#[case] remaining_secs: i64) {
        let status = status_with(
            &mfa_profile(),
            Some(SessionLookup::Cached {
                expires_at: now() + Duration::seconds(remaining_secs),
            }),
            true,
            true,
        );
        assert_eq!(status.session, SessionState::Missing);
    }

    #[rstest::rstest]
    #[case(None, true, SessionState::Missing)]
    #[case(Some(SessionLookup::Unreadable), true, SessionState::Unreadable)]
    #[case(None, false, SessionState::CacheDisabled)]
    fn without_a_usable_session_a_person_acts_only_when_no_provider_is_enabled(
        #[case] lookup: Option<SessionLookup>,
        #[case] cache_enabled: bool,
        #[case] expected: SessionState,
    ) {
        let manual = status_with(&mfa_profile(), lookup.clone(), cache_enabled, false);
        let onepassword = status_with(&mfa_profile(), lookup, cache_enabled, true);
        assert_eq!(manual.session, expected);
        assert!(manual.needs_human);
        assert!(!onepassword.needs_human);
    }

    #[rstest::rstest]
    #[case(SessionState::NotRequired, "-")]
    #[case(SessionState::CacheDisabled, "cache disabled")]
    #[case(SessionState::Unreadable, "unreadable")]
    #[case(SessionState::Missing, "none")]
    #[case(SessionState::Valid { expires_at: now() + Duration::minutes(719) }, "valid (11h 59m)")]
    #[case(SessionState::Valid { expires_at: now() + Duration::minutes(59) }, "valid (59m)")]
    fn session_descriptions(#[case] session: SessionState, #[case] expected: &str) {
        assert_eq!(describe_session(&session, now()), expected);
    }

    #[test]
    fn remaining_time_is_hours_and_minutes_and_never_negative() {
        assert_eq!(
            describe_remaining(now() + Duration::minutes(719), now()),
            "11h 59m"
        );
        assert_eq!(
            describe_remaining(now() + Duration::minutes(59), now()),
            "59m"
        );
        assert_eq!(
            describe_remaining(now() - Duration::minutes(5), now()),
            "0m"
        );
    }
}
