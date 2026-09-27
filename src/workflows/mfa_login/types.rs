//! Pure data for the MFA login workflow.

use chrono::{DateTime, Utc};

use crate::domain::functions::error_mapping::StsErrorKind;
use crate::domain::types::CachedSession;
use crate::workflows::common::{FailureKind, LogLevel, MfaAttempt};

#[derive(Debug, Clone)]
pub struct LoginInput {
    pub mfa_serial: String,
    /// GetSessionToken duration (`[aws.session_cache] duration`).
    pub duration_seconds: u64,
    /// Ignore a usable cached session and get a new one.
    pub force: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginOutput {
    pub expiration: DateTime<Utc>,
    /// A usable cached session was kept; nothing was requested.
    pub reused: bool,
}

#[derive(Clone)]
pub enum LoginEffect {
    /// Load the cached session; the shell passes it on only while usable.
    LoadSession {
        mfa_serial: String,
    },
    GetMfaToken {
        mfa_serial: String,
    },
    GetSessionToken {
        mfa_serial: String,
        token: String,
        duration_seconds: u64,
    },
    /// Failing to store is a failed login.
    StoreSession {
        mfa_serial: String,
        session: CachedSession,
    },
    WaitForNextTotpWindow,
    Log {
        level: LogLevel,
        message: String,
    },
}

impl std::fmt::Debug for LoginEffect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LoadSession { mfa_serial } => f
                .debug_struct("LoadSession")
                .field("mfa_serial", mfa_serial)
                .finish(),
            Self::GetMfaToken { mfa_serial } => f
                .debug_struct("GetMfaToken")
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
            Self::WaitForNextTotpWindow => f.write_str("WaitForNextTotpWindow"),
            Self::Log { level, message } => f
                .debug_struct("Log")
                .field("level", level)
                .field("message", message)
                .finish(),
        }
    }
}

#[derive(Clone)]
pub enum LoginEvent {
    Start { input: LoginInput },
    SessionLoaded { session: Option<CachedSession> },
    MfaTokenReceived { token: String },
    MfaTokenFailed { error: String },
    SessionTokenReceived { session: CachedSession },
    SessionTokenFailed { error: String, kind: StsErrorKind },
}

impl std::fmt::Debug for LoginEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Start { input } => f.debug_struct("Start").field("input", input).finish(),
            Self::SessionLoaded { session } => f
                .debug_struct("SessionLoaded")
                .field("session", session)
                .finish(),
            Self::MfaTokenReceived { token: _ } => f
                .debug_struct("MfaTokenReceived")
                .field("token", &"[REDACTED]")
                .finish(),
            Self::MfaTokenFailed { error } => f
                .debug_struct("MfaTokenFailed")
                .field("error", error)
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
        }
    }
}

#[derive(Debug, Clone, Default)]
pub enum LoginState {
    #[default]
    Initial,
    LoadingSession {
        input: LoginInput,
    },
    WaitingForMfa {
        input: LoginInput,
        attempt: MfaAttempt,
    },
    GettingSessionToken {
        input: LoginInput,
        attempt: MfaAttempt,
    },
    Completed {
        output: LoginOutput,
    },
    Failed {
        error: String,
        kind: FailureKind,
    },
}
