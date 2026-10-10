//! Executor - Workflow Effect Execution Engine
//!
//! This module provides the execution engine that runs workflow effects.
//! It bridges the pure workflow state machines with the impure shell layer.
//!
//! ## Design
//!
//! The executor follows the "Effects as Data" pattern:
//! 1. Workflows return Effect types (pure data describing what to do)
//! 2. Executor interprets and executes these effects (impure operations)
//! 3. Results are converted to Events and fed back to the workflow
//!
//! This separation allows:
//! - Pure, testable workflow logic
//! - Centralized effect handling
//! - Easy mocking of external dependencies

use crate::domain::Credentials;
use crate::workflows::assume_role::{
    AssumeRoleEffect, AssumeRoleEvent, AssumeRoleInput, AssumeRoleOutput, AssumeRoleState, step,
};
use crate::workflows::common::LogLevel;

use super::runtime::Runtime;
use crate::console::progress;
use tracing::{debug, info, warn};

/// A workflow log effect: `# ...` lines are progress for the person, and
/// every line goes to the log at its level. Shared by every interpreter.
pub(super) fn emit_log(level: LogLevel, message: &str) {
    if message.starts_with('#') {
        progress!("{message}");
    }
    match level {
        LogLevel::Debug => debug!("{message}"),
        LogLevel::Info => info!("{message}"),
        LogLevel::Warn => warn!("{message}"),
    }
}

/// Executor error type
#[derive(Debug, thiserror::Error)]
pub enum ExecutorError {
    #[error("Manual MFA token required for {serial}")]
    MfaRequired { serial: String },
    #[error("MFA token retrieval failed: {0}")]
    MfaFailed(String),

    #[error("{0}")]
    InvalidProfile(String),

    /// STS rejected the request; `kind` selects the error code.
    #[error("{message}")]
    StsFailed {
        kind: crate::domain::functions::error_mapping::StsErrorKind,
        message: String,
    },

    #[error("Unexpected state: {0}")]
    UnexpectedState(String),
}

impl ExecutorError {
    /// The error a workflow that ended in `Failed` stands for.
    pub(crate) fn from_failure(kind: &crate::workflows::common::FailureKind, error: &str) -> Self {
        use crate::workflows::common::FailureKind;
        match kind {
            FailureKind::InvalidProfile => Self::InvalidProfile(error.to_owned()),
            FailureKind::Mfa => Self::MfaFailed(error.to_owned()),
            FailureKind::Sts(kind) => Self::StsFailed {
                kind: kind.clone(),
                message: error.to_owned(),
            },
        }
    }
}

/// The cached MFA session of `mfa_serial` when it is still usable at `now`;
/// a cache that cannot be read counts as holding none.
pub(crate) async fn load_usable_session(
    runtime: &Runtime,
    mfa_serial: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<crate::domain::types::CachedSession> {
    match runtime.session_cache.load(mfa_serial).await {
        Ok(session) => {
            session.filter(|s| crate::domain::functions::session_cache::is_session_usable(s, now))
        }
        Err(crate::ports::SessionCacheError::Denied(denied)) => {
            crate::adapters::keychain::log_denied(&format!("MFA session {mfa_serial}"), denied);
            None
        }
        Err(error) => {
            warn!("Failed to load MFA session: {error}");
            None
        }
    }
}

/// Convert STS response to credentials
fn sts_response_to_credentials(response: crate::ports::sts::StsCredentials) -> Credentials {
    Credentials::new(
        response.access_key_id,
        response.secret_access_key,
        Some(response.session_token),
        response.expiration,
    )
}

/// Shared workflow input for the CLI, TUI and signed API requests. The
/// configuration was validated when it was read, so nothing can fail here.
pub fn assume_role_input(
    config: &crate::adapters::config::Config,
    profile: crate::domain::Profile,
    readonly: bool,
    mfa_token: Option<String>,
) -> AssumeRoleInput {
    AssumeRoleInput {
        session_cache: crate::workflows::assume_role::SessionCacheSettings {
            enabled: config.aws.session_cache.enabled,
            duration_seconds: config.aws.session_cache.duration,
        },
        profile,
        mfa_token,
        readonly,
        session_name_config: config.aws.session_name.clone(),
    }
}

/// Build the input and run AssumeRole for a selected profile: the shared
/// path of `env` / `exec` / `console`, the TUI and signed API requests.
pub async fn assume_role_for_profile(
    runtime: &Runtime,
    profile: crate::domain::Profile,
    readonly: bool,
    mfa_token: Option<String>,
) -> Result<AssumeRoleOutput, ExecutorError> {
    let input = assume_role_input(&runtime.config, profile, readonly, mfa_token);
    execute_assume_role(runtime, input).await
}

/// Execute the AssumeRole workflow
///
/// This function runs the workflow state machine to completion,
/// executing effects as needed and feeding results back.
///
/// # Arguments
/// * `runtime` - The runtime containing all dependencies
/// * `input` - The workflow input
///
/// # Returns
/// * `Ok(output)` - The workflow completed successfully
/// * `Err(error)` - The workflow failed
pub async fn execute_assume_role(
    runtime: &Runtime,
    input: AssumeRoleInput,
) -> Result<AssumeRoleOutput, ExecutorError> {
    let mut state = AssumeRoleState::Initial;
    let mut current_event = AssumeRoleEvent::Start { input };

    loop {
        // Run pure state transition
        let (next_state, effects) = step(state, current_event);

        let mut effect_event = None;
        for effect in effects {
            match effect {
                AssumeRoleEffect::Log { level, message } => emit_log(level, &message),
                AssumeRoleEffect::GetMfaToken { .. } => {
                    // Fetch after all effects, including any rollover wait, have run.
                }
                AssumeRoleEffect::LoadSession { mfa_serial } => {
                    let now = chrono::Utc::now();
                    let session = load_usable_session(runtime, &mfa_serial, now).await;
                    if let Some(session) = &session {
                        progress!(
                            "# Using cached MFA session (expires in {}h)",
                            (session.expiration - now).num_hours()
                        );
                    }
                    effect_event = Some(AssumeRoleEvent::SessionLoaded { session });
                }
                AssumeRoleEffect::GetSessionToken {
                    mfa_serial,
                    token,
                    duration_seconds,
                } => {
                    effect_event = Some(
                        match request_session_token(runtime, mfa_serial, token, duration_seconds)
                            .await
                        {
                            Ok(session) => AssumeRoleEvent::SessionTokenReceived { session },
                            Err((kind, error)) => {
                                AssumeRoleEvent::SessionTokenFailed { error, kind }
                            }
                        },
                    );
                }
                AssumeRoleEffect::StoreSession {
                    mfa_serial,
                    session,
                } => {
                    if let Err(error) = runtime.session_cache.store(&mfa_serial, &session).await {
                        warn!("Failed to store MFA session: {error}");
                    }
                }
                AssumeRoleEffect::InvalidateSession { mfa_serial } => {
                    if let Err(error) = runtime.session_cache.remove(&mfa_serial).await {
                        warn!("Failed to remove MFA session: {error}");
                    }
                }
                AssumeRoleEffect::WaitForNextTotpWindow => wait_for_next_totp_window().await,
                AssumeRoleEffect::ReadProfileKeys => {
                    effect_event = Some(match runtime.sts.read_signing_keys().await {
                        Ok(credentials) => AssumeRoleEvent::ProfileKeysRead { credentials },
                        Err(error) => AssumeRoleEvent::ProfileKeysFailed {
                            kind: sts_error_kind(&error),
                            error: error.to_string(),
                        },
                    });
                }
                AssumeRoleEffect::AssumeRole { request } => {
                    effect_event = Some(match runtime.sts.assume_role(request).await {
                        Ok(response) => AssumeRoleEvent::AssumeRoleSucceeded {
                            credentials: sts_response_to_credentials(response),
                        },
                        Err(error) => AssumeRoleEvent::AssumeRoleFailed {
                            kind: sts_error_kind(&error),
                            error: error.to_string(),
                        },
                    });
                }
            }
        }

        state = next_state;

        // Determine next event based on current state using functional composition
        current_event = match &state {
            AssumeRoleState::Completed { output } => return Ok(output.clone()),
            AssumeRoleState::Failed { error, kind } => {
                return Err(ExecutorError::from_failure(kind, error));
            }
            AssumeRoleState::WaitingForMfa { input, .. } => {
                // Get MFA serial and fetch token
                let mfa_serial = input
                    .profile
                    .mfa_serial_raw()
                    .ok_or_else(|| ExecutorError::MfaFailed("No MFA serial".to_string()))?;
                match runtime.mfa_provider.get_token(mfa_serial).await {
                    Ok(Some(token)) => AssumeRoleEvent::MfaTokenReceived { token },
                    Ok(None) => {
                        return Err(ExecutorError::MfaRequired {
                            serial: mfa_serial.to_string(),
                        });
                    }
                    Err(error) => AssumeRoleEvent::MfaTokenFailed {
                        error: error.to_string(),
                    },
                }
            }
            _ => effect_event.ok_or_else(|| {
                ExecutorError::UnexpectedState(format!("Unexpected state: {state:?}"))
            })?,
        };
    }
}

/// GetSessionToken for an MFA device. A response without an expiration cannot
/// be cached and counts as a service error.
pub(crate) async fn request_session_token(
    runtime: &Runtime,
    mfa_serial: String,
    token: String,
    duration_seconds: u64,
) -> Result<
    crate::domain::types::CachedSession,
    (
        crate::domain::functions::error_mapping::StsErrorKind,
        String,
    ),
> {
    use crate::domain::functions::error_mapping::StsErrorKind;
    let response = runtime
        .sts
        .get_session_token(crate::ports::sts::GetSessionTokenRequest {
            mfa_serial,
            mfa_token: token,
            duration_seconds,
        })
        .await
        .map_err(|error| (sts_error_kind(&error), error.to_string()))?;
    match response.expiration {
        Some(expiration) => Ok(crate::domain::types::CachedSession {
            access_key_id: response.access_key_id,
            secret_access_key: response.secret_access_key,
            session_token: response.session_token,
            expiration,
        }),
        None => Err((
            StsErrorKind::ServiceError,
            "GetSessionToken returned no expiration".into(),
        )),
    }
}

/// Sleep until the next 30-second TOTP window, announcing it on stderr.
pub(crate) async fn wait_for_next_totp_window() {
    let seconds = crate::domain::functions::totp_window::secs_until_next_totp_window(
        chrono::Utc::now().timestamp() as u64,
    );
    progress!("# MFA code already used, waiting {seconds}s for the next one");
    tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;
}

pub(crate) fn sts_error_kind(
    error: &crate::ports::StsError,
) -> crate::domain::functions::error_mapping::StsErrorKind {
    use crate::domain::functions::error_mapping::StsErrorKind;
    use crate::ports::StsError;
    match error {
        StsError::InvalidCredentials => StsErrorKind::InvalidCredentials,
        StsError::AccessDenied(_) => StsErrorKind::AccessDenied,
        StsError::MfaRequired => StsErrorKind::MfaRequired,
        StsError::InvalidMfaToken => StsErrorKind::InvalidMfaToken,
        StsError::RoleNotFound(_) => StsErrorKind::RoleNotFound,
        StsError::ServiceError(_) => StsErrorKind::ServiceError,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Profile;
    use crate::domain::functions::SessionNameConfig;
    use crate::ports::StsOperations;
    use crate::ports::mfa::MfaError;
    use crate::ports::sts::{AssumeRoleRequest, StsCredentials, StsError};
    use async_trait::async_trait;

    struct MockSts {
        response: Result<StsCredentials, StsError>,
    }

    #[async_trait]
    impl StsOperations for MockSts {
        async fn get_session_token(
            &self,
            _request: crate::ports::sts::GetSessionTokenRequest,
        ) -> Result<StsCredentials, StsError> {
            Err(StsError::ServiceError(
                "Unexpected GetSessionToken call".into(),
            ))
        }

        async fn assume_role(
            &self,
            _request: AssumeRoleRequest,
        ) -> Result<StsCredentials, StsError> {
            self.response.clone()
        }

        async fn read_signing_keys(&self) -> Result<Credentials, StsError> {
            Err(StsError::ServiceError(
                "Unexpected read of the signing keys".into(),
            ))
        }
    }

    struct MockMfa {
        token: Result<Option<String>, MfaError>,
    }

    #[async_trait]
    impl crate::ports::MfaProvider for MockMfa {
        async fn get_token(&self, _mfa_serial: &str) -> Result<Option<String>, MfaError> {
            self.token.clone()
        }
    }

    fn create_test_profile() -> Profile {
        Profile::new("test").with_role_arn_raw("arn:aws:iam::123456789012:role/TestRole")
    }

    fn create_test_profile_with_mfa() -> Profile {
        Profile::new("test")
            .with_role_arn_raw("arn:aws:iam::123456789012:role/TestRole")
            .with_mfa_serial_raw("arn:aws:iam::123456789012:mfa/user")
    }

    #[tokio::test]
    async fn test_execute_assume_role_success() {
        let runtime = Runtime::test(
            MockSts {
                response: Ok(StsCredentials {
                    access_key_id: "AKIATEST".to_string(),
                    secret_access_key: "secret".to_string(),
                    session_token: "token".to_string(),
                    expiration: None,
                }),
            },
            MockMfa {
                token: Ok(Some("123456".to_string())),
            },
        );

        let input = AssumeRoleInput {
            session_cache: crate::workflows::assume_role::SessionCacheSettings {
                enabled: false,
                duration_seconds: 43200,
            },
            profile: create_test_profile(),
            mfa_token: None,
            readonly: false,
            session_name_config: SessionNameConfig::default(),
        };

        let result = execute_assume_role(&runtime, input).await;
        assert!(result.is_ok());

        let output = result.unwrap();
        assert_eq!(output.profile_name, "test");
    }

    #[tokio::test]
    async fn test_execute_assume_role_with_mfa() {
        let runtime = Runtime::test(
            MockSts {
                response: Ok(StsCredentials {
                    access_key_id: "AKIATEST".to_string(),
                    secret_access_key: "secret".to_string(),
                    session_token: "token".to_string(),
                    expiration: None,
                }),
            },
            MockMfa {
                token: Ok(Some("123456".to_string())),
            },
        );

        let input = AssumeRoleInput {
            session_cache: crate::workflows::assume_role::SessionCacheSettings {
                enabled: false,
                duration_seconds: 43200,
            },
            profile: create_test_profile_with_mfa(),
            mfa_token: None,
            readonly: false,
            session_name_config: SessionNameConfig::default(),
        };

        let result = execute_assume_role(&runtime, input).await;
        assert!(result.is_ok());

        let output = result.unwrap();
        assert_eq!(output.profile_name, "test");
    }

    #[tokio::test]
    async fn test_execute_assume_role_sts_failure() {
        let runtime = Runtime::test(
            MockSts {
                response: Err(StsError::AccessDenied("Access denied".to_string())),
            },
            MockMfa {
                token: Ok(Some("123456".to_string())),
            },
        );

        let input = AssumeRoleInput {
            session_cache: crate::workflows::assume_role::SessionCacheSettings {
                enabled: false,
                duration_seconds: 43200,
            },
            profile: create_test_profile(),
            mfa_token: None,
            readonly: false,
            session_name_config: SessionNameConfig::default(),
        };

        let result = execute_assume_role(&runtime, input).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execute_assume_role_mfa_failure() {
        let runtime = Runtime::test(
            MockSts {
                response: Ok(StsCredentials {
                    access_key_id: "AKIATEST".to_string(),
                    secret_access_key: "secret".to_string(),
                    session_token: "token".to_string(),
                    expiration: None,
                }),
            },
            MockMfa { token: Ok(None) },
        );

        let input = AssumeRoleInput {
            session_cache: crate::workflows::assume_role::SessionCacheSettings {
                enabled: false,
                duration_seconds: 43200,
            },
            profile: create_test_profile_with_mfa(),
            mfa_token: None,
            readonly: false,
            session_name_config: SessionNameConfig::default(),
        };

        let result = execute_assume_role(&runtime, input).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execute_assume_role_uses_configured_template() {
        let runtime = Runtime::test(
            MockSts {
                response: Ok(StsCredentials {
                    access_key_id: "AKIATEST".to_string(),
                    secret_access_key: "secret".to_string(),
                    session_token: "token".to_string(),
                    expiration: None,
                }),
            },
            MockMfa {
                token: Ok(Some("123456".to_string())),
            },
        );

        // Parse a user config and convert it exactly like the shell layer does
        let toml_str = r#"
            [aws.session_name]
            template = "claude-{profile}"
        "#;
        let config = crate::adapters::config::Config::parse(toml_str).unwrap();

        let input = AssumeRoleInput {
            session_cache: crate::workflows::assume_role::SessionCacheSettings {
                enabled: false,
                duration_seconds: 43200,
            },
            profile: create_test_profile(),
            mfa_token: None,
            readonly: false,
            session_name_config: config.aws.session_name.clone(),
        };

        let output = execute_assume_role(&runtime, input).await.unwrap();
        assert_eq!(output.session_name.as_deref(), Some("claude-test"));
    }

    /// A profile without `role_arn` and without MFA hands out the keys its
    /// STS client signs with, and sends STS nothing.
    #[tokio::test]
    async fn an_iam_user_profile_uses_the_signing_keys_without_any_sts_call() {
        let mut sts = crate::ports::sts::MockStsOperations::new();
        sts.expect_read_signing_keys().times(1).returning(|| {
            Ok(Credentials::new(
                "AKIAUSER".into(),
                "user-secret".into(),
                None,
                None,
            ))
        });
        sts.expect_assume_role().never();
        sts.expect_get_session_token().never();
        let runtime = Runtime::test(sts, crate::ports::mfa::MockMfaProvider::new());

        let output = assume_role_for_profile(&runtime, Profile::new("uploader"), false, None)
            .await
            .unwrap();
        assert_eq!(output.credentials.access_key_id(), "AKIAUSER");
        assert_eq!(output.credentials.session_token(), None);
        assert_eq!(output.session_name, None);
    }
}

#[cfg(test)]
#[path = "executor_session_tests.rs"]
mod session_tests;
