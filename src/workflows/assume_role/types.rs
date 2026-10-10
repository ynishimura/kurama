//! Pure data for the AssumeRole state machine.
use crate::domain::functions::{SessionNameConfig, error_mapping::StsErrorKind};
use crate::domain::types::CachedSession;
use crate::domain::{Credentials, Profile};
use crate::ports::AssumeRoleRequest;
use crate::workflows::common::{FailureKind, LogLevel, MfaAttempt};

#[derive(Debug, Clone)]
pub struct SessionCacheSettings {
    pub enabled: bool,
    pub duration_seconds: u64,
}

#[derive(Clone)]
pub struct AssumeRoleInput {
    pub profile: Profile,
    pub mfa_token: Option<String>,
    pub readonly: bool,
    pub session_name_config: SessionNameConfig,
    pub session_cache: SessionCacheSettings,
}

impl std::fmt::Debug for AssumeRoleInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AssumeRoleInput")
            .field("profile", &self.profile)
            .field("mfa_token", &self.mfa_token.as_ref().map(|_| "[REDACTED]"))
            .field("readonly", &self.readonly)
            .field("session_name_config", &self.session_name_config)
            .field("session_cache", &self.session_cache)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct AssumeRoleOutput {
    pub credentials: Credentials,
    pub profile_name: String,
    /// The role session's name; `None` for an IAM user profile (no
    /// `role_arn`), whose own keys or MFA session are the credentials.
    pub session_name: Option<String>,
}

#[derive(Debug, Clone)]
pub enum CredentialSource {
    Default,
    Session { session: CachedSession, fresh: bool },
}

#[derive(Clone)]
pub enum AssumeRoleEffect {
    GetMfaToken {
        mfa_serial: String,
    },
    LoadSession {
        mfa_serial: String,
    },
    GetSessionToken {
        mfa_serial: String,
        token: String,
        duration_seconds: u64,
    },
    StoreSession {
        mfa_serial: String,
        session: CachedSession,
    },
    InvalidateSession {
        mfa_serial: String,
    },
    WaitForNextTotpWindow,
    AssumeRole {
        request: AssumeRoleRequest,
    },
    /// Read the long-term keys the profile signs with (1Password or the
    /// shared credentials file): an IAM user profile without MFA uses them
    /// as they are.
    ReadProfileKeys,
    Log {
        level: LogLevel,
        message: String,
    },
}

impl std::fmt::Debug for AssumeRoleEffect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::GetMfaToken { mfa_serial } => f
                .debug_struct("GetMfaToken")
                .field("mfa_serial", mfa_serial)
                .finish(),
            Self::LoadSession { mfa_serial } => f
                .debug_struct("LoadSession")
                .field("mfa_serial", mfa_serial)
                .finish(),
            Self::GetSessionToken {
                mfa_serial,
                token: _,
                duration_seconds,
            } => f
                .debug_struct("GetSessionToken")
                .field("mfa_serial", mfa_serial)
                .field("token", &"[REDACTED]")
                .field("duration_seconds", duration_seconds)
                .finish(),
            Self::StoreSession {
                mfa_serial,
                session,
            } => f
                .debug_struct("StoreSession")
                .field("mfa_serial", mfa_serial)
                .field("session", session)
                .finish(),
            Self::InvalidateSession { mfa_serial } => f
                .debug_struct("InvalidateSession")
                .field("mfa_serial", mfa_serial)
                .finish(),
            Self::WaitForNextTotpWindow => f.write_str("WaitForNextTotpWindow"),
            Self::ReadProfileKeys => f.write_str("ReadProfileKeys"),
            Self::AssumeRole { request } => f
                .debug_struct("AssumeRole")
                .field("request", request)
                .finish(),
            Self::Log { level, message } => f
                .debug_struct("Log")
                .field("level", level)
                .field("message", message)
                .finish(),
        }
    }
}

#[derive(Clone)]
pub enum AssumeRoleEvent {
    Start { input: AssumeRoleInput },
    MfaTokenReceived { token: String },
    MfaTokenFailed { error: String },
    SessionLoaded { session: Option<CachedSession> },
    SessionTokenReceived { session: CachedSession },
    SessionTokenFailed { error: String, kind: StsErrorKind },
    AssumeRoleSucceeded { credentials: Credentials },
    AssumeRoleFailed { error: String, kind: StsErrorKind },
    ProfileKeysRead { credentials: Credentials },
    ProfileKeysFailed { error: String, kind: StsErrorKind },
}

impl std::fmt::Debug for AssumeRoleEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Start { input } => f.debug_struct("Start").field("input", input).finish(),
            Self::MfaTokenReceived { token: _ } => f
                .debug_struct("MfaTokenReceived")
                .field("token", &"[REDACTED]")
                .finish(),
            Self::MfaTokenFailed { error } => f
                .debug_struct("MfaTokenFailed")
                .field("error", error)
                .finish(),
            Self::SessionLoaded { session } => f
                .debug_struct("SessionLoaded")
                .field("session", session)
                .finish(),
            Self::SessionTokenReceived { session } => f
                .debug_struct("SessionTokenReceived")
                .field("session", session)
                .finish(),
            Self::SessionTokenFailed { error, kind } => f
                .debug_struct("SessionTokenFailed")
                .field("error", error)
                .field("kind", kind)
                .finish(),
            Self::AssumeRoleSucceeded { credentials } => f
                .debug_struct("AssumeRoleSucceeded")
                .field("credentials", credentials)
                .finish(),
            Self::AssumeRoleFailed { error, kind } => f
                .debug_struct("AssumeRoleFailed")
                .field("error", error)
                .field("kind", kind)
                .finish(),
            Self::ProfileKeysRead { credentials } => f
                .debug_struct("ProfileKeysRead")
                .field("credentials", credentials)
                .finish(),
            Self::ProfileKeysFailed { error, kind } => f
                .debug_struct("ProfileKeysFailed")
                .field("error", error)
                .field("kind", kind)
                .finish(),
        }
    }
}

/// Keep the original input so retries preserve readonly, duration and name settings.
#[derive(Debug, Clone, Default)]
pub enum AssumeRoleState {
    #[default]
    Initial,
    LoadingSession {
        input: AssumeRoleInput,
    },
    WaitingForMfa {
        input: AssumeRoleInput,
        attempt: MfaAttempt,
    },
    GettingSessionToken {
        input: AssumeRoleInput,
        attempt: MfaAttempt,
    },
    Assuming {
        input: AssumeRoleInput,
        session_name: String,
        source: CredentialSource,
        attempt: MfaAttempt,
    },
    /// An IAM user profile without MFA, waiting for its long-term keys.
    ReadingKeys {
        input: AssumeRoleInput,
    },
    Completed {
        output: AssumeRoleOutput,
    },
    Failed {
        error: String,
        kind: FailureKind,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_debug_does_not_expose_mfa_or_session_secrets() {
        let input = AssumeRoleInput {
            profile: Profile::new("test"),
            mfa_token: Some("workflow-mfa-token".into()),
            readonly: false,
            session_name_config: SessionNameConfig::default(),
            session_cache: SessionCacheSettings {
                enabled: false,
                duration_seconds: 900,
            },
        };
        let effect = AssumeRoleEffect::GetSessionToken {
            mfa_serial: "serial".into(),
            token: "effect-mfa-token".into(),
            duration_seconds: 900,
        };
        let event = AssumeRoleEvent::MfaTokenReceived {
            token: "event-mfa-token".into(),
        };

        assert!(!format!("{input:?}").contains("workflow-mfa-token"));
        assert!(!format!("{effect:?}").contains("effect-mfa-token"));
        assert!(!format!("{event:?}").contains("event-mfa-token"));
    }
}
