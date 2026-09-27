//! `[agent]` and `[api.<name>.agent]`: what a run under `KURAMA_AGENT` may do without `--confirm`.
//!
//! ```toml
//! [agent]
//! allow_methods = ["GET", "HEAD"]   # the default
//! allow_paths = []                  # empty: every path
//! deny_paths = ["/admin*"]
//! exec_readonly = true              # the default: exec attaches ReadOnlyAccess
//!
//! [api.github.agent]                # each key named replaces the [agent] one
//! allow_methods = ["GET", "POST"]
//! ```

use serde::{Deserialize, Serialize};

use super::default_true;
use crate::domain::functions::agent_policy::{AgentPolicy, DEFAULT_ALLOW_METHODS, check_rules};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    #[serde(default = "default_methods")]
    pub allow_methods: Vec<String>,
    #[serde(default)]
    pub allow_paths: Vec<String>,
    #[serde(default)]
    pub deny_paths: Vec<String>,
    #[serde(default = "default_true")]
    pub exec_readonly: bool,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            allow_methods: default_methods(),
            allow_paths: Vec::new(),
            deny_paths: Vec::new(),
            exec_readonly: true,
        }
    }
}

fn default_methods() -> Vec<String> {
    DEFAULT_ALLOW_METHODS.map(str::to_owned).to_vec()
}

/// `[api.<name>.agent]`: a key it names replaces the `[agent]` one for
/// this API only.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiAgentToml {
    #[serde(default)]
    pub allow_methods: Option<Vec<String>>,
    #[serde(default)]
    pub allow_paths: Option<Vec<String>>,
    #[serde(default)]
    pub deny_paths: Option<Vec<String>>,
}

impl AgentConfig {
    pub fn validate(&self) -> Result<(), String> {
        check_rules(
            &self.allow_methods,
            &[self.allow_paths.as_slice(), &self.deny_paths].concat(),
        )
    }

    /// The rules a request to an API is held to: `[agent]`, with the keys
    /// `[api.<name>.agent]` names replaced.
    pub fn policy_for(&self, api: Option<&ApiAgentToml>) -> AgentPolicy {
        let api = api.cloned().unwrap_or_default();
        AgentPolicy {
            allow_methods: api
                .allow_methods
                .unwrap_or_else(|| self.allow_methods.clone())
                .iter()
                .map(|method| method.to_ascii_uppercase())
                .collect(),
            allow_paths: api.allow_paths.unwrap_or_else(|| self.allow_paths.clone()),
            deny_paths: api.deny_paths.unwrap_or_else(|| self.deny_paths.clone()),
        }
    }
}

impl ApiAgentToml {
    pub fn validate(&self) -> Result<(), String> {
        let patterns = [
            self.allow_paths.as_deref().unwrap_or_default(),
            self.deny_paths.as_deref().unwrap_or_default(),
        ]
        .concat();
        check_rules(self.allow_methods.as_deref().unwrap_or_default(), &patterns)
            .map_err(|error| format!("agent: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::super::Config;

    #[test]
    fn without_a_section_an_agent_may_get_and_head_anything_and_exec_read_only() {
        let config = Config::parse("").unwrap();
        let policy = config.agent.policy_for(None);
        assert_eq!(policy.allow_methods, ["GET", "HEAD"]);
        assert!(policy.allow_paths.is_empty() && policy.deny_paths.is_empty());
        assert!(config.agent.exec_readonly);
    }

    #[test]
    fn an_api_section_replaces_only_the_keys_it_names() {
        let config = Config::parse(
            "[agent]\nallow_methods = [\"get\"]\ndeny_paths = [\"/admin*\"]\n\n\
             [api.pets]\nbase_url = \"https://pets.example\"\n\n\
             [api.pets.agent]\nallow_methods = [\"GET\", \"post\"]\n",
        )
        .unwrap();
        let widened = config.agent.policy_for(config.api["pets"].agent.as_ref());
        assert_eq!(widened.allow_methods, ["GET", "POST"]);
        assert_eq!(widened.deny_paths, ["/admin*"]);
        let global = config.agent.policy_for(None);
        assert_eq!(global.allow_methods, ["GET"]);
    }

    #[test]
    fn an_unknown_method_or_a_relative_pattern_is_a_configuration_error_naming_the_section() {
        let error = Config::parse("[agent]\nallow_methods = [\"FETCH\"]\n").unwrap_err();
        assert!(format!("{error:#}").contains("[agent] allow_methods names \"FETCH\""));
        let error = Config::parse(
            "[api.pets]\nbase_url = \"https://pets.example\"\n\n[api.pets.agent]\ndeny_paths = [\"admin\"]\n",
        )
        .unwrap_err();
        assert!(
            format!("{error:#}").contains("[api.pets] agent: a path pattern starts with /"),
            "{error:#}"
        );
    }
}
