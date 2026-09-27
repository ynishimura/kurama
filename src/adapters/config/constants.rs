//! Application-wide constants and configuration values.
//!
//! This module organizes all magic numbers and configuration
//! constants in a centralized location for easy maintenance.
//!
//! NOTE: Session name configuration has been consolidated in
//! domain::functions::session::SessionNameConfig.

/// Application-wide constants
pub mod app {
    use crate::domain::types::session_duration::MAX_DURATION_SECS;

    /// AWS federation session duration in seconds (12 hours)
    /// Uses maximum allowed by AWS for console sessions
    pub const FEDERATION_SESSION_DURATION: u64 = MAX_DURATION_SECS;
}

pub mod tui {
    /// Tick rate for TUI event loop in milliseconds
    pub const TICK_RATE_MS: u64 = 250;
}

pub mod file {
    /// Secure file permission mode (owner read/write only)
    pub const SECURE_FILE_MODE: u32 = 0o600;
}

// NOTE: Environment variable constants have been consolidated in domain::functions::export::AWS_ENV_VARS
// Use that as the single source of truth for AWS environment variables managed by kurama.

pub mod aws {
    /// AWS federation endpoint (defined in the domain layer)
    pub use crate::domain::constants::FEDERATION_ENDPOINT;
}

pub mod oauth {
    /// Default timeout of one HTTP request (`kurama api --timeout`, token requests)
    pub const HTTP_TIMEOUT_SECS: u64 = 60;

    /// How long a person may take in the browser or at the device code page
    /// (defined in the domain layer, where the device flow counts it down)
    pub use crate::domain::functions::oauth::AUTHORIZATION_TIMEOUT_SECS;
}
