//! `CliCommand`: every invocation after parsing, as `dispatch.rs` receives it.

use super::client::ClientKind;
use super::commands::api_command::ApiCommand;
use super::commands::logout::LogoutTarget;
use super::commands::profile::ProfileCommandConfig;
use super::commands::status::StatusRowKind;

/// Every CLI invocation, after parsing.
#[derive(Debug, Clone)]
pub enum CliCommand {
    /// `init zsh`: the shell integration script
    Init,
    /// `completions zsh`: the completion script
    Completions,
    /// `agent`: the contract for agents and scripts
    AgentGuide,
    /// `agent --skill`: the Agent Skill that points at the contract
    AgentSkill,
    /// `agent --kind <KIND> --json`: the offline contract of one JSON client
    AgentContract(ClientKind),
    /// `agent --json`: the catalog of every subcommand, read off the clap
    /// definition
    AgentCatalog,
    /// `inventory` (hidden): what this binary has, for `cargo xtask inventory`
    Inventory,
    /// `agent ready [--json]`: whether each source can be used right now; the
    /// one `agent` mode that reads configuration, as `status` does
    AgentReady { json: bool },
    /// `env`, `exec` and `console`: get the source's credentials, then use them
    Profile(ProfileCommandConfig),
    /// `login <PROFILE> [--force] [--no-browser] [-v]`
    Login {
        profile: String,
        force: bool,
        no_browser: bool,
        verbose: bool,
    },
    /// `token <PROFILE> [--json] [--fingerprint] [-v]`
    Token {
        profile: String,
        json: bool,
        fingerprint: bool,
        verbose: bool,
    },
    /// `api <API> [TARGET] [OPTIONS]`
    Api(Box<ApiCommand>),
    /// `data [INPUT] [OPTIONS]`: analyze local or S3 inputs
    Data(Box<super::commands::data_command::DataCommand>),
    /// `db <DATABASE> [OPTIONS]`: read a database
    Db(Box<super::commands::db_command::DbCommand>),
    /// `status [NAME] --kind <KIND> [--json]`: what one JSON client has configured
    ClientStatus {
        kind: ClientKind,
        profile: Option<String>,
        json: bool,
    },
    /// `status [PROFILE] [--only <KIND>] [--json]`
    Status {
        profile: Option<String>,
        /// Keep only the rows of this kind; every kind when absent.
        only: Option<StatusRowKind>,
        json: bool,
    },
    /// `logout <PROFILE> | --all`
    Logout(LogoutTarget),
    /// `unset`
    Unset,
    /// No command: the TUI
    Tui,
    /// `config <check|path|list|show|add>`: each reads config.toml itself, so a
    /// file that does not load is what `check` reports and what `show` still
    /// shows, rather than a failure before they run
    Config(super::commands::config::ConfigCommand),
    /// `preset [show <ID>]`: the list reads no configuration; `show` reads
    /// config.toml itself, as `config` does
    Preset(super::commands::preset::PresetCommand),
    /// `s3 <S3> [TARGET] --buckets | --list | --search TEXT`: walk a bucket
    S3(Box<super::commands::s3_command::S3Command>),
    /// `kurama audit`: the calls the audit log recorded.
    Audit {
        since: Option<chrono::Duration>,
        json: bool,
        /// `--watch`: the activity monitor instead of the listing.
        watch: bool,
    },
    /// `kurama mcp`: the MCP server on stdio; each tool call is kurama run again
    /// as an agent, which reads the configuration itself
    Mcp,
    /// `agent install [--dir DIR] [--offline] [--dry-run] [--json]`: the
    /// Agent Skills of kurama and of every API, written to a directory; it
    /// reads the configuration for the APIs
    AgentInstall(super::commands::agent_install::AgentInstall),
}

impl CliCommand {
    /// Whether the command needs configuration and logging. Shell integration
    /// output runs at every shell startup and must work without either; the
    /// agent contract is read to write or fix the configuration.
    pub fn needs_bootstrap(&self) -> bool {
        match self {
            Self::Init
            | Self::Completions
            | Self::AgentGuide
            | Self::AgentSkill
            | Self::AgentContract(_)
            | Self::AgentCatalog
            | Self::Inventory
            | Self::Config(_)
            | Self::Preset(_)
            | Self::Audit { .. }
            | Self::Mcp => false,
            Self::Profile(_)
            | Self::Login { .. }
            | Self::Token { .. }
            | Self::Api(_)
            | Self::Data(_)
            | Self::Db(_)
            | Self::ClientStatus { .. }
            | Self::Status { .. }
            | Self::Logout(_)
            | Self::Unset
            | Self::Tui
            | Self::S3(_)
            | Self::AgentReady { .. }
            | Self::AgentInstall(_) => true,
        }
    }

    /// Whether stdout carries one JSON document, so progress lines and logs
    /// have to stay off it.
    pub fn answers_with_json(&self) -> bool {
        match self {
            Self::Data(command) => command.json,
            Self::Db(command) => command.json,
            Self::Config(command) => command.json(),
            Self::Preset(command) => command.json(),
            Self::S3(command) => command.json,
            Self::AgentReady { json } => *json,
            Self::AgentInstall(install) => install.json,
            Self::Mcp => true,
            _ => false,
        }
    }

    /// Whether this is an `api`, `status`, `env` or `token` run that answers
    /// with JSON (`JsonErrorKind`) and asked for nothing else on stderr, so
    /// stderr is left to its error document. `api --dry-run --json` is one:
    /// its plan goes to stdout. `-v` prints on stderr what was asked for, and
    /// so does a `--dry-run` without `--json`, so they are not. Unlike a JSON client,
    /// these runs can wait for a person on a terminal (a login URL, a device
    /// code), which is why the caller keeps a terminal's stderr.
    pub fn leaves_stderr_to_json_errors(&self) -> bool {
        match self {
            Self::Api(api) => (api.output.json || api.output.jq.is_some()) && !api.output.verbose,
            Self::Status { json, .. }
            | Self::ClientStatus { json, .. }
            | Self::Token { json, .. } => *json,
            Self::Profile(config) => {
                config.action
                    == super::commands::profile::ProfileAction::Export(
                        crate::domain::types::OutputFormat::Json,
                    )
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_integration_and_agent_commands_skip_bootstrap() {
        assert!(!CliCommand::Init.needs_bootstrap());
        assert!(!CliCommand::Completions.needs_bootstrap());
        assert!(!CliCommand::AgentGuide.needs_bootstrap());
        assert!(!CliCommand::AgentSkill.needs_bootstrap());
        assert!(!CliCommand::AgentContract(ClientKind::Data).needs_bootstrap());
        assert!(!CliCommand::AgentCatalog.needs_bootstrap());
        assert!(!CliCommand::Inventory.needs_bootstrap());
        assert!(CliCommand::AgentReady { json: true }.needs_bootstrap());
        assert!(
            CliCommand::ClientStatus {
                kind: ClientKind::Data,
                profile: None,
                json: true
            }
            .needs_bootstrap()
        );
        assert!(CliCommand::Unset.needs_bootstrap());
        assert!(CliCommand::Logout(LogoutTarget::All).needs_bootstrap());
    }

    #[test]
    fn only_a_json_run_that_asked_for_nothing_else_leaves_stderr_to_its_error() {
        let parse = |args: &[&str]| {
            let matches = crate::build_command()
                .try_get_matches_from(std::iter::once("kurama").chain(args.iter().copied()))
                .unwrap();
            super::super::parser::parse_cli_command(&matches)
        };
        for args in [
            &["api", "github", "/user", "--json"][..],
            &["api", "github", "/user", "--jq", ".id"],
            &["api", "github", "/user", "--json", "--dry-run"],
            &["status", "--json"],
            &["status", "--kind", "data", "--json"],
            &["env", "dev", "--json"],
            &["token", "github", "--json"],
        ] {
            assert!(parse(args).leaves_stderr_to_json_errors(), "{args:?}");
        }
        for args in [
            &["api", "github", "/user"][..],
            &["api", "github", "/user", "--dry-run"],
            &["api", "github", "/user", "--json", "--dry-run", "-v"],
            &["api", "github", "/user", "--json", "-v"],
            &["status"],
            &["env", "dev"],
            &["exec", "dev", "--", "true"],
            &["token", "github"],
            &["data", "--json"],
        ] {
            assert!(!parse(args).leaves_stderr_to_json_errors(), "{args:?}");
        }
    }
}
