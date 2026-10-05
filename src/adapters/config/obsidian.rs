//! `[obsidian]`: the vault `kurama obsidian` reads through the official Obsidian CLI, the folders it may read, and the bounds of a read.
//!
//! ```toml
//! [obsidian]
//! vault = "obsidian-brain"           # the vault name (or id) the CLI targets
//! allow_paths = ["Wiki/", "Daily/"]  # folders an agent may search and read; required
//! cli_path = "/Applications/Obsidian.app/Contents/MacOS/obsidian"  # default "obsidian"
//! max_read_bytes = 65536             # a longer note is cut and says so
//! timeout = 20                       # seconds per CLI call
//! ```

use serde::{Deserialize, Serialize};

use crate::domain::functions::obsidian::allowed_folder;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObsidianConfig {
    /// The vault the CLI targets, by name or id.
    pub vault: String,
    /// Folders relative to the vault root that may be searched and read.
    pub allow_paths: Vec<String>,
    /// The CLI; a LaunchAgent's PATH has no `/Applications/Obsidian.app/Contents/MacOS`.
    #[serde(default = "default_cli_path")]
    pub cli_path: String,
    /// The most of one note `read` returns.
    #[serde(default = "default_max_read_bytes")]
    pub max_read_bytes: usize,
    /// Seconds before a CLI call is killed.
    #[serde(default = "default_timeout")]
    pub timeout: u64,
}

fn default_cli_path() -> String {
    "obsidian".to_owned()
}

fn default_max_read_bytes() -> usize {
    65536
}

fn default_timeout() -> u64 {
    20
}

impl ObsidianConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.vault.is_empty() {
            return Err("vault must name a vault".to_owned());
        }
        if self.allow_paths.is_empty() {
            return Err("allow_paths must name at least one folder".to_owned());
        }
        for entry in &self.allow_paths {
            allowed_folder(entry)?;
        }
        if self.max_read_bytes == 0 {
            return Err("max_read_bytes must be at least 1".to_owned());
        }
        if self.timeout == 0 {
            return Err("timeout must be at least 1 second".to_owned());
        }
        Ok(())
    }

    /// The allowed folders, each ending in `/`. Only called on a validated
    /// section, so every entry is one.
    pub fn allowed_folders(&self) -> Vec<String> {
        self.allow_paths
            .iter()
            .filter_map(|entry| allowed_folder(entry).ok())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::super::Config;

    #[test]
    fn a_section_with_vault_and_folders_takes_the_defaults() {
        let config =
            Config::parse("[obsidian]\nvault = \"brain\"\nallow_paths = [\"Wiki\", \"Daily/\"]\n")
                .unwrap();
        let obsidian = config.obsidian.unwrap();
        assert_eq!(obsidian.allowed_folders(), ["Wiki/", "Daily/"]);
        assert_eq!(obsidian.cli_path, "obsidian");
        assert_eq!(obsidian.max_read_bytes, 65536);
        assert_eq!(obsidian.timeout, 20);
        assert!(Config::parse("").unwrap().obsidian.is_none());
    }

    #[rstest::rstest]
    #[case("vault = \"b\"", "missing field `allow_paths`")]
    #[case("allow_paths = [\"Wiki/\"]", "missing field `vault`")]
    #[case(
        "vault = \"b\"\nallow_paths = []",
        "allow_paths must name at least one folder"
    )]
    #[case(
        "vault = \"b\"\nallow_paths = [\"/Wiki/\"]",
        "\"/Wiki/\" is not a folder"
    )]
    #[case(
        "vault = \"b\"\nallow_paths = [\"Wiki/../x\"]",
        "\"Wiki/../x\" is not a folder"
    )]
    #[case("vault = \"\"\nallow_paths = [\"Wiki/\"]", "vault must name a vault")]
    #[case(
        "vault = \"b\"\nallow_paths = [\"Wiki/\"]\ntimeout = 0",
        "timeout must be at least 1 second"
    )]
    fn a_section_kurama_cannot_use_is_a_configuration_error(
        #[case] lines: &str,
        #[case] expected: &str,
    ) {
        let message = format!(
            "{:#}",
            Config::parse(&format!("[obsidian]\n{lines}\n")).unwrap_err()
        );
        assert!(message.contains(expected), "{message}");
    }
}
