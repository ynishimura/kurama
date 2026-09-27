//! `[audit]`: whether a call of `api`, `exec`, `db` or `data` is appended to the audit log, and the size the log rotates at.
//!
//! ```toml
//! [audit]
//! # enabled = true    # every run; false: none; absent: only a KURAMA_AGENT run
//! max_bytes = 1048576 # the default: the log moves to audit.jsonl.1 past this
//! ```

use serde::{Deserialize, Serialize};

/// The bound below which a rotation would keep too little to read.
const MIN_MAX_BYTES: u64 = 4096;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditConfig {
    /// `None`: a run under `KURAMA_AGENT` only.
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default = "default_max_bytes")]
    pub max_bytes: u64,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            enabled: None,
            max_bytes: default_max_bytes(),
        }
    }
}

fn default_max_bytes() -> u64 {
    1 << 20
}

impl AuditConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.max_bytes < MIN_MAX_BYTES {
            return Err(format!(
                "max_bytes must be at least {MIN_MAX_BYTES}, got {}",
                self.max_bytes
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::Config;

    #[test]
    fn without_a_section_only_an_agent_run_is_recorded_and_the_log_rotates_at_one_mebibyte() {
        let config = Config::parse("").unwrap();
        assert_eq!(config.audit.enabled, None);
        assert_eq!(config.audit.max_bytes, 1_048_576);
        let config = Config::parse("[audit]\nenabled = false\n").unwrap();
        assert_eq!(config.audit.enabled, Some(false));
    }

    #[test]
    fn a_bound_too_small_to_keep_a_line_is_a_configuration_error() {
        let error = Config::parse("[audit]\nmax_bytes = 10\n").unwrap_err();
        assert!(
            format!("{error:#}").contains("[audit] max_bytes must be at least 4096, got 10"),
            "{error:#}"
        );
        assert!(Config::parse("[audit]\nmax_bytes = 4096\n").is_ok());
    }
}
