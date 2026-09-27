//! Runtime - Dependency Injection Container
//!
//! This module provides the runtime environment for executing workflows.
//! It holds all the dependencies needed to execute side effects.
//!
//! ## Design
//!
//! The Runtime follows the "Dependency Injection Container" pattern:
//! - All dependencies are expressed as trait objects (ports)
//! - Production and test implementations are interchangeable
//! - Dependencies are resolved at application startup

use std::sync::Arc;

use crate::adapters::auth::OnePasswordManager;
use crate::adapters::aws::sts::service::StsService;
use crate::adapters::config::{Config, OnePasswordConfig};
use crate::domain::types::Profile;
use crate::ports::{MfaProvider, StsOperations};

/// Dependency Injection Container
///
/// Holds all dependencies needed for workflow execution.
/// Use `Runtime::from_config()` for real operations,
/// or `Runtime::test()` for testing with mocks.
pub struct Runtime {
    /// STS operations port
    pub sts: Arc<dyn StsOperations>,

    /// MFA provider port
    pub mfa_provider: Arc<dyn MfaProvider>,

    /// Application configuration
    pub config: Arc<Config>,
    pub session_cache: Arc<dyn crate::ports::SessionCache>,
}

impl Runtime {
    /// Create a runtime from a configuration: real STS client, 1Password MFA
    /// provider when enabled, session cache backend per configuration.
    pub async fn from_config(config: Config, profile: &Profile) -> Self {
        let sts = StsService::from_config(&config, profile).await;

        // Initialize MFA provider
        let mfa_provider = create_mfa_provider(&config.onepassword);

        Self {
            session_cache: crate::adapters::session_cache::create_session_cache(
                config.aws.session_cache.enabled,
            ),
            sts: Arc::new(sts),
            mfa_provider,
            config: Arc::new(config),
        }
    }

    /// Create a test runtime with mock dependencies
    #[cfg(test)]
    pub fn test(sts: impl StsOperations + 'static, mfa: impl MfaProvider + 'static) -> Self {
        Self {
            session_cache: Arc::new(crate::adapters::session_cache::NoopSessionCache),
            sts: Arc::new(sts),
            mfa_provider: Arc::new(mfa),
            config: Arc::new(Config::default()),
        }
    }
}

/// Create MFA provider based on configuration
fn create_mfa_provider(op_config: &OnePasswordConfig) -> Arc<dyn MfaProvider> {
    if op_config.enabled {
        Arc::new(OnePasswordManager::new(op_config.clone()))
    } else {
        Arc::new(crate::ports::mfa::ManualMfaProvider)
    }
}
