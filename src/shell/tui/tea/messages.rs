//! TUI Messages - User inputs and async operation results
//!
//! Messages represent all events that can affect the application state.
//! They are processed by the pure `update` function.

use crate::domain::Credentials;
use crate::domain::functions::profile_status::{ProfileStatus, SessionState, describe_session};
use chrono::{DateTime, Utc};
use crossterm::event::KeyEvent;

/// All possible messages in the TUI application
#[derive(Debug, Clone)]
pub enum TuiMessage {
    /// Keyboard input event
    Key(KeyEvent),

    /// Terminal resize event
    Resize,

    /// The clock, read by the runtime at start and on every tick
    Tick(DateTime<Utc>),

    /// Profile loading completed
    ProfilesLoaded(ProfilesLoadedResult),

    /// Assume role operation completed
    AssumeRoleCompleted(AssumeRoleResult),

    /// Console URL generation completed
    ConsoleUrlGenerated(ConsoleUrlResult),

    /// The URL opener finished
    BrowserOpened(BrowserOpenResult),

    /// The rows of the Auth, API, DB and Data tabs
    SourcesLoaded(super::sources::Sources),

    /// The clipboard took the example command, or why not
    Copied(Result<(), String>),

    /// The palette's operations and history, read from disk
    PaletteItemsLoaded(Vec<super::palette::PaletteItem>),
}

/// Result of loading profiles
#[derive(Debug, Clone)]
pub struct ProfilesLoadedResult {
    pub profiles: Result<Vec<ProfileRow>, String>,
}

/// One row of the home screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRow {
    pub name: String,
    /// Provider kind; every profile in ~/.aws/config is `aws`.
    pub kind: &'static str,
    /// Session column, as in `kurama status`: `-`, `none`, `valid (11h 59m)`, ...
    pub session: String,
    /// When the cached MFA session ends, for a usable one.
    pub session_expires_at: Option<DateTime<Utc>>,
    /// The shell holds this profile's credentials (`KURAMA_AWS`).
    pub active: bool,
    pub role_arn: Option<String>,
    pub region: Option<String>,
    pub mfa_serial: Option<String>,
    /// Enter will ask for an MFA code: no usable session and no provider.
    pub needs_human: bool,
}

impl ProfileRow {
    pub fn from_status(status: &ProfileStatus, now: DateTime<Utc>) -> Self {
        Self {
            name: status.name.clone(),
            kind: "aws",
            session: describe_session(&status.session, now),
            session_expires_at: match status.session {
                SessionState::Valid { expires_at } => Some(expires_at),
                _ => None,
            },
            active: status.active,
            role_arn: status.role_arn.clone(),
            region: status.region.clone(),
            mfa_serial: status.mfa_serial.clone(),
            needs_human: status.needs_human,
        }
    }
}

/// Result of assume role operation
#[derive(Debug, Clone)]
pub struct AssumeRoleResult {
    pub profile_name: String,
    pub result: Result<CredentialsInfo, AssumeRoleError>,
}

/// Credentials information for display
#[derive(Debug, Clone)]
pub struct CredentialsInfo {
    pub access_key_id: String,
    pub credentials: Credentials,
    pub region: Option<String>,
}

/// Assume role error types
#[derive(Debug, Clone)]
pub enum AssumeRoleError {
    /// MFA is required
    MfaRequired { serial: String },
    /// Generic error
    Failed { message: String },
}

/// Result of console URL generation
#[derive(Debug, Clone)]
pub struct ConsoleUrlResult {
    pub result: Result<String, String>,
}

/// Result of handing the console URL to the URL opener
#[derive(Debug, Clone)]
pub struct BrowserOpenResult {
    pub result: Result<(), String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::functions::profile_status::SessionState;

    #[test]
    fn row_shows_the_same_session_text_as_status() {
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let status = ProfileStatus {
            name: "ops-mfa".into(),
            role_arn: Some("arn:aws:iam::123456789012:role/Ops".into()),
            region: Some("ap-northeast-1".into()),
            mfa_serial: Some("arn:aws:iam::123456789012:mfa/agent".into()),
            active: true,
            session: SessionState::Valid {
                expires_at: now + chrono::Duration::minutes(90),
            },
            needs_human: false,
        };
        assert_eq!(
            ProfileRow::from_status(&status, now),
            ProfileRow {
                name: "ops-mfa".into(),
                kind: "aws",
                session: "valid (1h 30m)".into(),
                session_expires_at: Some(now + chrono::Duration::minutes(90)),
                active: true,
                role_arn: Some("arn:aws:iam::123456789012:role/Ops".into()),
                region: Some("ap-northeast-1".into()),
                mfa_serial: Some("arn:aws:iam::123456789012:mfa/agent".into()),
                needs_human: false,
            }
        );
    }
}
