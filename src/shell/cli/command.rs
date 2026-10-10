//! `CliCommand`: every invocation after parsing, as `dispatch.rs` receives it.

use super::client::ClientKind;
use super::commands::api_command::ApiCommand;
use super::commands::logout::LogoutTarget;
use super::commands::profile::ProfileCommandConfig;
use super::commands::status::StatusRowKind;
use crate::shell::audit::Call;

/// Every CLI invocation, after parsing.
#[derive(Debug, Clone)]
pub enum CliCommand {
    /// `init zsh [--completion-only]`: the shell integration script, or
    /// only its completion function
    Init { completion_only: bool },
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
    /// `status --ready [--json]`: whether each source can be used right now; the
    /// one `agent` mode that reads configuration, as `status` does
    StatusReady { json: bool },
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
    /// `kurama mcp [--listen]`: the MCP server on stdio, or over HTTP on the
    /// `[mcp] listen` address; each tool call is kurama run again as an
    /// agent, which reads the configuration itself
    Mcp { listen: bool },
    /// `agent install [--dir DIR] [--offline] [--dry-run] [--json]`: the
    /// Agent Skills of kurama and of every API, written to a directory; it
    /// reads the configuration for the APIs
    AgentInstall(super::commands::agent_install::AgentInstall),
    /// `obsidian search|read|files [--json]`: the `[obsidian]` vault through
    /// the Obsidian CLI, inside `allow_paths` only
    Obsidian(super::commands::obsidian::ObsidianCommand),
}

impl CliCommand {
    /// Whether the command needs configuration and logging. Shell integration
    /// output runs at every shell startup and must work without either; the
    /// agent contract is read to write or fix the configuration.
    pub fn needs_bootstrap(&self) -> bool {
        match self {
            Self::Init { .. }
            | Self::AgentGuide
            | Self::AgentSkill
            | Self::AgentContract(_)
            | Self::AgentCatalog
            | Self::Inventory
            | Self::Config(_)
            | Self::Preset(_)
            | Self::Audit { .. } => false,
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
            | Self::StatusReady { .. }
            | Self::Mcp { .. }
            | Self::AgentInstall(_)
            | Self::Obsidian(_) => true,
        }
    }

    /// What this run promises about its streams, and the call the audit log
    /// records for it. One match over every command, with no catch-all: a
    /// new subcommand stops the build until someone says whether it answers
    /// with JSON and whether it is audited.
    pub fn contract(&self) -> RunContract<'_> {
        use super::commands::profile::ProfileAction;
        use crate::domain::types::OutputFormat;
        let text = RunContract::of(Output::Text);
        match self {
            Self::Api(api) => RunContract {
                // `api --dry-run --json` is one: its plan goes to stdout. `-v`
                // prints on stderr what was asked for, and so does a
                // `--dry-run` without `--json`, so they are not.
                output: if (api.output.json || api.output.jq.is_some()) && !api.output.verbose {
                    Output::JsonErrors
                } else {
                    Output::Text
                },
                audited: Some(Call {
                    command: "api",
                    target: &api.api,
                    program: None,
                }),
            },
            Self::Profile(profile) => match &profile.action {
                ProfileAction::Exec(argv) => RunContract {
                    output: Output::Text,
                    audited: Some(Call {
                        command: "exec",
                        target: &profile.profile_name,
                        program: argv.first().map(String::as_str),
                    }),
                },
                ProfileAction::Export(OutputFormat::Json) => RunContract::of(Output::JsonErrors),
                ProfileAction::Export(OutputFormat::Shell) | ProfileAction::OpenConsole => text,
            },
            Self::Status { json, .. }
            | Self::ClientStatus { json, .. }
            | Self::Token { json, .. } => RunContract::of(Output::json_errors_when(*json)),
            Self::Db(db) => RunContract {
                output: Output::document_when(db.json),
                audited: Some(Call {
                    command: "db",
                    target: &db.database,
                    program: None,
                }),
            },
            Self::Data(data) => RunContract {
                output: Output::document_when(data.json),
                audited: Some(Call {
                    command: "data",
                    target: data.workspace.as_deref().unwrap_or("ad-hoc"),
                    program: None,
                }),
            },
            Self::Obsidian(obsidian) => RunContract {
                output: Output::document_when(obsidian.json),
                audited: Some(Call {
                    command: "obsidian",
                    target: obsidian.audit_target(),
                    program: None,
                }),
            },
            Self::Preset(preset) => RunContract {
                output: Output::document_when(preset.json()),
                // Only `preset setup` sends anything, with its first read.
                audited: preset.sent_to().map(|api| Call {
                    command: "preset setup",
                    target: api,
                    program: None,
                }),
            },
            Self::Config(command) => RunContract::of(Output::document_when(command.json())),
            Self::S3(command) => RunContract::of(Output::document_when(command.json)),
            Self::StatusReady { json } => RunContract::of(Output::document_when(*json)),
            Self::AgentInstall(install) => RunContract::of(Output::document_when(install.json)),
            // Over stdio, stdout is the JSON-RPC stream; over HTTP stdout
            // carries nothing, and stderr is the request log. Each tool call
            // is kurama run again, which writes its own audit entry.
            Self::Mcp { listen } => RunContract::of(Output::document_when(!listen)),
            // `--json` prints the document, but the run reads no
            // configuration and starts no logging, so there is nothing to
            // keep off stdout.
            Self::AgentContract(_) | Self::AgentCatalog | Self::Inventory | Self::Audit { .. } => {
                text
            }
            Self::Init { .. }
            | Self::AgentGuide
            | Self::AgentSkill
            | Self::Login { .. }
            | Self::Logout(_)
            | Self::Unset
            | Self::Tui => text,
        }
    }

    /// Whether stdout carries one JSON document, so progress lines and logs
    /// have to stay off it.
    pub fn answers_with_json(&self) -> bool {
        self.contract().output == Output::JsonDocument
    }

    /// Whether this is an `api`, `status`, `env` or `token` run that answers
    /// with JSON (`JsonErrorKind`) and asked for nothing else on stderr, so
    /// stderr is left to its error document. Unlike a JSON client, these runs
    /// can wait for a person on a terminal (a login URL, a device code),
    /// which is why the caller keeps a terminal's stderr.
    pub fn leaves_stderr_to_json_errors(&self) -> bool {
        self.contract().output == Output::JsonErrors
    }
}

/// What a run promises about its streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    /// Text for a person; progress lines and logs on stderr.
    Text,
    /// stdout carries one JSON document, so progress lines and logs stay off
    /// both streams.
    JsonDocument,
    /// A `JsonErrorKind` run under `--json` / `--jq`: stderr is left to its
    /// error document when no person reads it.
    JsonErrors,
}

impl Output {
    fn document_when(json: bool) -> Self {
        if json { Self::JsonDocument } else { Self::Text }
    }

    fn json_errors_when(json: bool) -> Self {
        if json { Self::JsonErrors } else { Self::Text }
    }
}

/// What [`CliCommand::contract`] answers: the streams, and the call the audit
/// log records (`api`, `exec`, `db`, `data`, `obsidian` and the first read of
/// `preset setup`).
pub struct RunContract<'a> {
    pub output: Output,
    pub audited: Option<Call<'a>>,
}

impl RunContract<'_> {
    fn of(output: Output) -> Self {
        Self {
            output,
            audited: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_integration_and_agent_commands_skip_bootstrap() {
        assert!(
            !CliCommand::Init {
                completion_only: false
            }
            .needs_bootstrap()
        );
        assert!(!CliCommand::AgentGuide.needs_bootstrap());
        assert!(!CliCommand::AgentSkill.needs_bootstrap());
        assert!(!CliCommand::AgentContract(ClientKind::Data).needs_bootstrap());
        assert!(!CliCommand::AgentCatalog.needs_bootstrap());
        assert!(!CliCommand::Inventory.needs_bootstrap());
        assert!(CliCommand::StatusReady { json: true }.needs_bootstrap());
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

    /// The call each run writes to the audit log: the command, its target
    /// and, for `exec`, the program.
    #[test]
    fn the_contract_names_the_call_the_audit_log_records() {
        let parse = |args: &[&str]| {
            let matches = crate::build_command()
                .try_get_matches_from(std::iter::once("kurama").chain(args.iter().copied()))
                .unwrap();
            super::super::parser::parse_cli_command(&matches)
        };
        let audited = |args: &[&str]| {
            parse(args).contract().audited.map(|call| {
                (
                    call.command,
                    call.target.to_owned(),
                    call.program.map(str::to_owned),
                )
            })
        };
        let call = |command: &'static str, target: &str, program: Option<&str>| {
            Some((command, target.to_owned(), program.map(str::to_owned)))
        };
        assert_eq!(
            audited(&["api", "github", "/user"]),
            call("api", "github", None)
        );
        assert_eq!(
            audited(&["exec", "dev", "--", "aws", "sts"]),
            call("exec", "dev", Some("aws"))
        );
        assert_eq!(
            audited(&["db", "app", "--tables", "--json"]),
            call("db", "app", None)
        );
        assert_eq!(audited(&["data", "--json"]), call("data", "ad-hoc", None));
        for args in [
            &["env", "dev"][..],
            &["env", "dev", "--json"],
            &["console", "dev"],
            &["token", "github"],
            &["status", "--json"],
            &["login", "github"],
            &["preset"],
            &["audit", "--json"],
        ] {
            assert_eq!(audited(args), None, "{args:?}");
        }
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
