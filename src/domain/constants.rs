//! Constants that business rules depend on.
//!
//! Only values used by pure domain code live here. Application defaults
//! (durations, UI timings, file modes) live in `adapters::config::constants`,
//! which re-exports these so adapter code keeps a single import path.

/// Managed policy attached to the session in readonly mode.
pub const DEFAULT_READONLY_POLICY_ARN: &str = "arn:aws:iam::aws:policy/ReadOnlyAccess";

/// Issuer name sent to the console federation endpoint.
pub const FEDERATION_ISSUER: &str = "kurama";

/// AWS console federation endpoint.
pub const FEDERATION_ENDPOINT: &str = "https://signin.aws.amazon.com/federation";

/// Console URL opened when no destination is requested.
pub const DEFAULT_CONSOLE_URL: &str = "https://console.aws.amazon.com/";
