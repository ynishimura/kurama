//! `kurama unset`: print `unset` lines for every variable kurama exports.
//!
//! Follows the "Effects as Data" pattern: `plan_unset_command` is pure and
//! `shell/cli/executor.rs` performs the I/O. The AWS variables are fixed;
//! the token variables come from the `env_var` of every `[auth.*]` source,
//! of either kind: `kurama env` exports both, so `unset` clears both. The
//! variable the shell's `KURAMA_AUTH_VAR` names is cleared too, because the
//! configuration may have renamed or removed it since `env` exported it.

use anyhow::Result;

use super::super::effects::UnsetEffect;
use super::super::executor::execute_unset_effects;
use crate::adapters::config::Config;
use crate::domain::functions::export::{ACTIVE_AUTH_ENV_VAR, token_managed_vars};
use crate::domain::types::AuthSource;

/// Plan effects for unset command (pure function)
pub fn plan_unset_command(token_vars: Vec<String>) -> Vec<UnsetEffect> {
    vec![
        UnsetEffect::Log {
            message: "Clearing the exported variables".to_string(),
        },
        UnsetEffect::OutputUnsetScript { token_vars },
        UnsetEffect::PrintSuccess,
    ]
}

/// The token variables `env` and `unset` clear: every configured `env_var`,
/// and the one the shell recorded an earlier `env` exporting into.
pub fn managed_token_vars(config: &Config) -> Result<Vec<String>> {
    let previous = std::env::var(ACTIVE_AUTH_ENV_VAR).ok();
    Ok(token_managed_vars(
        config
            .auth_sources()?
            .iter()
            .map(AuthSource::env_var)
            .collect::<Vec<_>>(),
        previous.as_deref(),
    ))
}

/// Handle the unset command (entry point)
pub fn handle_unset_command(config: &Config) -> Result<()> {
    let token_vars = managed_token_vars(config)?;
    let effects = plan_unset_command(token_vars);
    execute_unset_effects(effects)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::functions::export::generate_unset_script;

    #[test]
    fn test_plan_unset_command() {
        let effects = plan_unset_command(vec!["GITHUB_TOKEN".into()]);

        // Should have: Log, OutputUnsetScript, PrintSuccess
        assert_eq!(effects.len(), 3);
        assert!(matches!(effects[0], UnsetEffect::Log { .. }));
        assert!(matches!(
            &effects[1],
            UnsetEffect::OutputUnsetScript { token_vars } if token_vars == &["GITHUB_TOKEN"]
        ));
        assert!(matches!(effects[2], UnsetEffect::PrintSuccess));
    }

    #[test]
    fn test_generate_unset_script() {
        let script = generate_unset_script(&[]);
        assert!(script.contains("unset AWS_ACCESS_KEY_ID"));
        assert!(script.contains("unset AWS_SECRET_ACCESS_KEY"));
        assert!(script.contains("unset AWS_SESSION_TOKEN"));
    }
}
