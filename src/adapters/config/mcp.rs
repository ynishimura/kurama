//! `[mcp]`: what `kurama mcp` offers and, with `--listen`, where it listens and the token it accepts.
//!
//! ```toml
//! [mcp]
//! listen = "127.0.0.1:8807"                         # --listen only; loopback only
//! token = "op://Agent/kurama-mcp-token/credential"  # --listen only; a reference, never the value
//! tools = ["list_operations", "describe_operation", "call_api"]  # default: every tool
//! call_timeout = 600                                # seconds, the wait for a slot included
//! max_concurrent_calls = 2                          # --listen only
//! ```

use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

use crate::adapters::error::CoreError;
use crate::domain::functions::mcp::{TOOL_NAMES, reads_obsidian};
use crate::domain::types::SecretRef;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpConfig {
    /// The address `--listen` binds, which has to be a loopback one: the
    /// token travels in plain HTTP up to whatever terminates TLS.
    #[serde(default)]
    pub listen: Option<String>,
    /// Where the token clients send is read from.
    #[serde(default)]
    pub token: Option<SecretRef>,
    /// The tools offered; every tool when absent.
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    /// Seconds one tool call may take, the wait for a slot included.
    #[serde(default = "default_call_timeout")]
    pub call_timeout: u64,
    /// Tool calls running at once over HTTP.
    #[serde(default = "default_max_concurrent_calls")]
    pub max_concurrent_calls: usize,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            listen: None,
            token: None,
            tools: None,
            call_timeout: default_call_timeout(),
            max_concurrent_calls: default_max_concurrent_calls(),
        }
    }
}

fn default_call_timeout() -> u64 {
    600
}

fn default_max_concurrent_calls() -> usize {
    2
}

/// What `--listen` needs, checked: both keys present.
pub struct McpListen<'a> {
    pub address: SocketAddr,
    pub token: &'a SecretRef,
}

impl McpConfig {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(listen) = &self.listen {
            listen_address(listen)?;
        }
        if self
            .token
            .as_ref()
            .is_some_and(|token| !token.is_reference())
        {
            return Err(format!(
                "token is the value itself; keep it in a secret store and name it ({})",
                SecretRef::schemes()
                    .map(|scheme| format!("{scheme}://"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let Some(unknown) = self
            .tools
            .iter()
            .flatten()
            .find(|tool| !TOOL_NAMES.contains(&tool.as_str()))
        {
            return Err(format!(
                "tools names {unknown:?}, which is not one of {}",
                TOOL_NAMES.join(", ")
            ));
        }
        if self.call_timeout == 0 {
            return Err("call_timeout must be at least 1 second".to_owned());
        }
        if self.max_concurrent_calls == 0 {
            return Err("max_concurrent_calls must be at least 1".to_owned());
        }
        Ok(())
    }

    /// The tools offered; without an `[obsidian]` section, every tool but
    /// those that read it.
    pub fn exposed_tools(&self, obsidian: bool) -> Vec<String> {
        match &self.tools {
            Some(tools) => tools.clone(),
            None => TOOL_NAMES
                .into_iter()
                .filter(|tool| obsidian || !reads_obsidian(tool))
                .map(str::to_owned)
                .collect(),
        }
    }

    /// A tool `tools` names that reads a vault no `[obsidian]` section names.
    pub fn obsidian_tool_without_a_vault(&self, obsidian: bool) -> Option<&str> {
        (!obsidian)
            .then(|| {
                self.tools
                    .iter()
                    .flatten()
                    .find(|tool| reads_obsidian(tool))
            })
            .flatten()
            .map(String::as_str)
    }

    /// The address and token `--listen` uses, or the key that is missing.
    pub fn listening(&self) -> Result<McpListen<'_>, CoreError> {
        let missing = |key: &str| CoreError::config(format!("[mcp] {key} is required by --listen"));
        let listen = self.listen.as_deref().ok_or_else(|| missing("listen"))?;
        let token = self.token.as_ref().ok_or_else(|| missing("token"))?;
        let address =
            listen_address(listen).map_err(|error| CoreError::config(format!("[mcp] {error}")))?;
        Ok(McpListen { address, token })
    }
}

fn listen_address(listen: &str) -> Result<SocketAddr, String> {
    let address: SocketAddr = listen.parse().map_err(|_| {
        format!("listen {listen:?} is not an address and port such as 127.0.0.1:8807")
    })?;
    if !address.ip().is_loopback() {
        return Err(format!(
            "listen {listen:?} is not a loopback address: the token would cross the network in plain HTTP; listen on 127.0.0.1 and let Tailscale Funnel or another front terminate TLS"
        ));
    }
    Ok(address)
}

#[cfg(test)]
mod tests {
    use super::super::Config;

    fn error(content: &str) -> String {
        format!("{:#}", Config::parse(content).unwrap_err())
    }

    #[test]
    fn without_a_section_every_tool_is_offered_and_nothing_listens() {
        let config = Config::parse("").unwrap();
        assert_eq!(config.mcp.exposed_tools(false).len(), 7);
        assert_eq!(config.mcp.exposed_tools(true).len(), 10);
        assert_eq!(config.mcp.call_timeout, 600);
        assert_eq!(config.mcp.max_concurrent_calls, 2);
        let missing = config.mcp.listening().err().unwrap();
        assert!(
            missing
                .to_string()
                .contains("[mcp] listen is required by --listen")
        );
    }

    #[test]
    fn a_complete_section_listens_on_loopback_with_a_reference() {
        let config = Config::parse(
            "[mcp]\nlisten = \"127.0.0.1:0\"\ntoken = \"op://Agent/mcp/credential\"\n\
             tools = [\"call_api\"]\ncall_timeout = 25\n",
        )
        .unwrap();
        let listen = config.mcp.listening().unwrap();
        assert!(listen.address.ip().is_loopback());
        assert_eq!(config.mcp.exposed_tools(false), ["call_api"]);
        let ipv6 =
            Config::parse("[mcp]\nlisten = \"[::1]:8807\"\ntoken = \"op://a/b/c\"\n").unwrap();
        assert!(ipv6.mcp.listening().is_ok());
        let no_token = Config::parse("[mcp]\nlisten = \"127.0.0.1:8807\"\n").unwrap();
        let missing = no_token.mcp.listening().err().unwrap();
        assert!(
            missing
                .to_string()
                .contains("[mcp] token is required by --listen")
        );
    }

    #[rstest::rstest]
    #[case("listen = \"0.0.0.0:8807\"", "is not a loopback address")]
    #[case("listen = \"192.168.1.2:8807\"", "is not a loopback address")]
    #[case("listen = \"localhost:8807\"", "is not an address and port")]
    #[case("token = \"plain-token-value\"", "token is the value itself")]
    #[case("tools = [\"exec\"]", "tools names \"exec\"")]
    #[case(
        "tools = [\"obsidian_read\"]",
        "tools names \"obsidian_read\", which reads the vault an [obsidian] section names"
    )]
    #[case("call_timeout = 0", "call_timeout must be at least 1 second")]
    #[case("max_concurrent_calls = 0", "max_concurrent_calls must be at least 1")]
    fn a_value_kurama_cannot_use_is_a_configuration_error(
        #[case] line: &str,
        #[case] expected: &str,
    ) {
        let message = error(&format!("[mcp]\n{line}\n"));
        assert!(message.contains("[mcp] "), "{message}");
        assert!(message.contains(expected), "{message}");
        assert!(!message.contains("plain-token-value"), "{message}");
    }
}
