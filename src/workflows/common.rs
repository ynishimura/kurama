//! What the workflows share: the log level of a `Log` effect, which MFA code
//! a step is on, and why a workflow stopped.
use crate::domain::functions::error_mapping::StsErrorKind;

#[derive(Debug, Clone, Copy)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MfaAttempt {
    First,
    Retry,
}

/// Why the workflow stopped; the shell maps this to an error code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureKind {
    /// The profile cannot be assumed as configured (for example no role ARN).
    InvalidProfile,
    /// The MFA provider returned an error.
    Mfa,
    /// STS rejected GetSessionToken or AssumeRole.
    Sts(StsErrorKind),
}
