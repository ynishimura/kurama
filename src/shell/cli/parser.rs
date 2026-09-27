//! `ArgMatches` -> `CliCommand`.

use clap::ArgMatches;

use crate::domain::OutputFormat;

use super::client::ClientKind;
use super::command::CliCommand;
use super::commands::api_command::ApiCommand;
use super::commands::logout::LogoutTarget;
use super::commands::profile::{ProfileAction, ProfileCommandConfig};
use super::commands::status::StatusRowKind;

/// Map the matches of `args::build_command` to a command.
pub fn parse_cli_command(matches: &ArgMatches) -> CliCommand {
    match matches.subcommand() {
        Some(("init", _)) => CliCommand::Init,
        Some(("completions", _)) => CliCommand::Completions,
        Some(("inventory", _)) => CliCommand::Inventory,
        Some(("config", config)) => {
            CliCommand::Config(super::commands::config::ConfigCommand::parse(config))
        }
        Some(("agent", agent)) => {
            if let Some(("ready", ready)) = agent.subcommand() {
                CliCommand::AgentReady {
                    json: ready.get_flag("json"),
                }
            } else if let Some(("install", install)) = agent.subcommand() {
                CliCommand::AgentInstall(super::commands::agent_install::AgentInstall {
                    dir: install.get_one::<std::path::PathBuf>("dir").cloned(),
                    offline: install.get_flag("offline"),
                    dry_run: install.get_flag("dry-run"),
                    json: install.get_flag("json"),
                })
            } else if let Some(kind) = client_kind(agent) {
                CliCommand::AgentContract(kind)
            } else if agent.get_flag("json") {
                CliCommand::AgentCatalog
            } else if agent.get_flag("skill") {
                CliCommand::AgentSkill
            } else {
                CliCommand::AgentGuide
            }
        }
        Some(("env", env)) => {
            let format = if env.get_flag("json") {
                OutputFormat::Json
            } else {
                OutputFormat::Shell
            };
            CliCommand::Profile(profile_command(
                env,
                ProfileAction::Export(format),
                env.get_flag("verbose"),
            ))
        }
        Some(("exec", exec)) => {
            let command = exec
                .get_many::<String>("command")
                .expect("clap requires COMMAND")
                .cloned()
                .collect();
            CliCommand::Profile(profile_command(
                exec,
                ProfileAction::Exec(command),
                exec.get_flag("verbose"),
            ))
        }
        Some(("console", console)) => {
            CliCommand::Profile(profile_command(console, ProfileAction::OpenConsole, false))
        }
        Some(("login", login)) => CliCommand::Login {
            profile: login
                .get_one::<String>("profile")
                .expect("clap requires PROFILE")
                .clone(),
            force: login.get_flag("force"),
            no_browser: login.get_flag("no-browser"),
            verbose: login.get_flag("verbose"),
        },
        Some(("token", token)) => CliCommand::Token {
            profile: token
                .get_one::<String>("profile")
                .expect("clap requires PROFILE")
                .clone(),
            json: token.get_flag("json"),
            fingerprint: token.get_flag("fingerprint"),
            verbose: token.get_flag("verbose"),
        },
        Some(("api", api)) => CliCommand::Api(Box::new(ApiCommand::parse(api))),
        Some(("data", data)) => CliCommand::Data(Box::new(
            super::commands::data_command::DataCommand::parse(data),
        )),
        Some(("db", db)) => {
            CliCommand::Db(Box::new(super::commands::db_command::DbCommand::parse(db)))
        }
        Some(("status", status)) if client_kind(status).is_some() => CliCommand::ClientStatus {
            kind: client_kind(status).expect("the guard matched a kind"),
            profile: status.get_one::<String>("profile").cloned(),
            json: status.get_flag("json"),
        },
        Some(("status", status)) => CliCommand::Status {
            profile: status.get_one::<String>("profile").cloned(),
            only: status
                .get_one::<String>("only")
                .map(|value| StatusRowKind::parse(value).expect("clap checked the value")),
            json: status.get_flag("json"),
        },
        Some(("logout", logout)) => CliCommand::Logout(match logout.get_one::<String>("profile") {
            Some(name) => LogoutTarget::Profile(name.clone()),
            None => LogoutTarget::All,
        }),
        Some(("unset", _)) => CliCommand::Unset,
        Some(("preset", preset)) => {
            CliCommand::Preset(super::commands::preset::PresetCommand::parse(preset))
        }
        Some(("s3", s3)) => {
            CliCommand::S3(Box::new(super::commands::s3_command::S3Command::parse(s3)))
        }
        Some(("audit", audit)) => CliCommand::Audit {
            since: audit.get_one::<chrono::Duration>("since").copied(),
            json: audit.get_flag("json"),
            watch: audit.get_flag("watch"),
        },
        Some(("mcp", _)) => CliCommand::Mcp,
        _ => CliCommand::Tui,
    }
}

/// The `--kind` of a command that addresses one JSON client. clap already
/// restricted the value to the kinds that exist.
fn client_kind(matches: &ArgMatches) -> Option<ClientKind> {
    matches
        .get_one::<String>("kind")
        .and_then(|kind| ClientKind::parse(kind))
}

fn profile_command(
    matches: &ArgMatches,
    action: ProfileAction,
    verbose: bool,
) -> ProfileCommandConfig {
    // `--confirm` is an `exec` option only.
    let confirm = matches!(action, ProfileAction::Exec(_)) && matches.get_flag("confirm");
    ProfileCommandConfig {
        profile_name: matches
            .get_one::<String>("profile")
            .expect("clap requires PROFILE")
            .clone(),
        readonly: matches.get_flag("readonly"),
        confirm,
        verbose,
        action,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::cli::args::build_command;

    fn parse(args: &[&str]) -> Result<CliCommand, clap::Error> {
        build_command()
            .try_get_matches_from(std::iter::once("kurama").chain(args.iter().copied()))
            .map(|matches| parse_cli_command(&matches))
    }

    fn profile_command_of(args: &[&str]) -> ProfileCommandConfig {
        match parse(args).unwrap() {
            CliCommand::Profile(config) => config,
            other => panic!("expected a profile command, got {other:?}"),
        }
    }

    #[test]
    fn env_exports_shell_script_by_default() {
        let config = profile_command_of(&["env", "dev"]);
        assert_eq!(config.profile_name, "dev");
        assert!(!config.readonly);
        assert_eq!(config.action, ProfileAction::Export(OutputFormat::Shell));
    }

    #[test]
    fn env_json_and_readonly() {
        let config = profile_command_of(&["env", "dev", "--json", "-r"]);
        assert!(config.readonly);
        assert_eq!(config.action, ProfileAction::Export(OutputFormat::Json));
    }

    #[test]
    fn console_opens_the_console_only() {
        let config = profile_command_of(&["console", "prod", "--readonly"]);
        assert_eq!(config.profile_name, "prod");
        assert!(config.readonly);
        assert_eq!(config.action, ProfileAction::OpenConsole);
    }

    #[test]
    fn exec_takes_the_command_after_double_dash() {
        let config =
            profile_command_of(&["exec", "dev", "-r", "--", "aws", "s3", "ls", "--recursive"]);
        assert!(config.readonly);
        assert_eq!(
            config.action,
            ProfileAction::Exec(vec![
                "aws".into(),
                "s3".into(),
                "ls".into(),
                "--recursive".into()
            ])
        );
        assert!(parse(&["exec", "dev"]).is_err());
        assert!(parse(&["exec", "dev", "aws"]).is_err());
    }

    #[test]
    fn login_with_and_without_force() {
        assert!(matches!(
            parse(&["login", "ops"]).unwrap(),
            CliCommand::Login { profile, force: false, no_browser: false, verbose: false } if profile == "ops"
        ));
        assert!(matches!(
            parse(&["login", "ops", "--force", "--no-browser", "-v"]).unwrap(),
            CliCommand::Login {
                force: true,
                no_browser: true,
                verbose: true,
                ..
            }
        ));
        assert!(parse(&["login"]).is_err());
    }

    #[test]
    fn inventory_is_its_own_command() {
        assert!(matches!(
            parse(&["inventory"]).unwrap(),
            CliCommand::Inventory
        ));
    }

    #[test]
    fn agent_prints_the_guide_or_the_skill() {
        assert!(matches!(parse(&["agent"]).unwrap(), CliCommand::AgentGuide));
        assert!(matches!(
            parse(&["agent", "--skill"]).unwrap(),
            CliCommand::AgentSkill
        ));
        // `--json` without a kind is the catalog of the whole CLI.
        assert!(matches!(
            parse(&["agent", "--json"]).unwrap(),
            CliCommand::AgentCatalog
        ));
        assert!(parse(&["agent", "--json", "--skill"]).is_err());
        // The catalog is one document to pipe into jq; there is no `--jq`.
        assert!(parse(&["agent", "--json", "--jq", ".commands"]).is_err());
    }

    #[test]
    fn a_kind_addresses_one_json_client_in_agent_and_status() {
        assert!(matches!(
            parse(&["agent", "--kind", "data", "--json"]).unwrap(),
            CliCommand::AgentContract(ClientKind::Data)
        ));
        assert!(matches!(
            parse(&["status", "--kind", "data", "--json"]).unwrap(),
            CliCommand::ClientStatus {
                kind: ClientKind::Data,
                profile: None,
                json: true
            }
        ));
        assert!(matches!(
            parse(&["status", "lake", "--kind", "data"]).unwrap(),
            CliCommand::ClientStatus { kind: ClientKind::Data, profile: Some(name), json: false } if name == "lake"
        ));
        assert!(matches!(
            parse(&["agent", "--kind", "db", "--json"]).unwrap(),
            CliCommand::AgentContract(ClientKind::Db)
        ));
        assert!(matches!(
            parse(&["status", "--kind", "db"]).unwrap(),
            CliCommand::ClientStatus {
                kind: ClientKind::Db,
                ..
            }
        ));
        assert!(matches!(
            parse(&["agent", "--kind", "s3", "--json"]).unwrap(),
            CliCommand::AgentContract(ClientKind::S3)
        ));
        assert!(matches!(
            parse(&["status", "--kind", "s3", "--json"]).unwrap(),
            CliCommand::ClientStatus {
                kind: ClientKind::S3,
                json: true,
                ..
            }
        ));
        // A kind the binary does not have never reaches the parser.
        assert!(parse(&["agent", "--kind", "api", "--json"]).is_err());
        assert!(parse(&["status", "--kind", "aws"]).is_err());
        // The contract is JSON only; the guide has no kind.
        assert!(parse(&["agent", "--kind", "data"]).is_err());
    }

    #[test]
    fn token_takes_a_profile_and_json() {
        assert!(matches!(
            parse(&["token", "github"]).unwrap(),
            CliCommand::Token { profile, json: false, fingerprint: false, verbose: false } if profile == "github"
        ));
        assert!(matches!(
            parse(&["token", "github", "--verbose"]).unwrap(),
            CliCommand::Token { verbose: true, .. }
        ));
        assert!(matches!(
            parse(&["token", "github", "--fingerprint", "--json"]).unwrap(),
            CliCommand::Token {
                json: true,
                fingerprint: true,
                ..
            }
        ));
        assert!(matches!(
            parse(&["token", "github", "--json"]).unwrap(),
            CliCommand::Token { json: true, .. }
        ));
        assert!(parse(&["token"]).is_err());
    }

    #[test]
    fn api_defaults_and_every_option() {
        let CliCommand::Api(command) = parse(&["api", "github"]).unwrap() else {
            panic!("expected api");
        };
        assert_eq!(command.api, "github");
        assert_eq!(command.target, None);
        assert_eq!(command.connection.timeout_secs, 60);
        assert!(command.request.headers.is_empty());
        assert!(!command.output.json);
        assert!(!command.output.dry_run);
        assert!(!command.output.verbose);
        assert!(!command.connection.insecure);
        assert_eq!(
            (command.signing.service, command.signing.region),
            (None, None)
        );
        assert!(command.request.params.is_empty());
        assert_eq!(
            (
                command.spec.ops,
                command.spec.describe,
                command.spec.refresh
            ),
            (None, None, false)
        );

        let CliCommand::Api(command) = parse(&[
            "api",
            "github",
            "/repos/o/r/issues",
            "-X",
            "POST",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "X-Trace: 1",
            "-d",
            "{\"title\":\"x\"}",
            "--jq",
            ".number",
            "--json",
            "--dry-run",
            "-v",
            "-k",
            "--timeout",
            "5",
            "--service",
            "execute-api",
            "--region",
            "ap-northeast-1",
            "-P",
            "owner=o",
            "--param",
            "repo=r",
            "--refresh-spec",
        ])
        .unwrap() else {
            panic!("expected api");
        };
        assert_eq!(command.request.params, ["owner=o", "repo=r"]);
        assert!(command.spec.refresh);
        assert_eq!(command.signing.service.as_deref(), Some("execute-api"));
        assert_eq!(command.signing.region.as_deref(), Some("ap-northeast-1"));
        assert_eq!(command.target.as_deref(), Some("/repos/o/r/issues"));
        assert_eq!(command.request.method.as_deref(), Some("POST"));
        assert_eq!(
            command.request.headers,
            ["Accept: application/vnd.github+json", "X-Trace: 1"]
        );
        assert_eq!(command.request.data.as_deref(), Some("{\"title\":\"x\"}"));
        assert_eq!(command.output.jq.as_deref(), Some(".number"));
        assert!(command.output.json);
        assert!(command.output.dry_run);
        assert!(command.output.verbose);
        assert!(command.connection.insecure);
        assert_eq!(command.connection.timeout_secs, 5);
        assert!(parse(&["api"]).is_err());
        assert!(parse(&["api", "github", "--timeout", "0"]).is_err());
    }

    #[test]
    fn api_ops_describe_and_targets_are_exclusive() {
        let CliCommand::Api(command) = parse(&["api", "github", "--ops"]).unwrap() else {
            panic!("expected api");
        };
        assert_eq!(command.spec.ops.as_deref(), Some(""));
        let CliCommand::Api(command) =
            parse(&["api", "github", "--ops", "issues", "--json"]).unwrap()
        else {
            panic!("expected api");
        };
        assert_eq!(command.spec.ops.as_deref(), Some("issues"));
        assert!(command.output.json);
        let CliCommand::Api(command) =
            parse(&["api", "github", "--describe", "issues/create"]).unwrap()
        else {
            panic!("expected api");
        };
        assert_eq!(command.spec.describe.as_deref(), Some("issues/create"));
        assert!(parse(&["api", "github", "/user", "--ops"]).is_err());
        assert!(parse(&["api", "github", "/user", "--describe", "x"]).is_err());
        assert!(parse(&["api", "github", "--ops", "--describe", "x"]).is_err());
        let CliCommand::Api(command) = parse(&[
            "api",
            "github",
            "issues/create",
            "-P",
            "owner=o",
            "--refresh-spec",
        ])
        .unwrap() else {
            panic!("expected api");
        };
        assert_eq!(command.target.as_deref(), Some("issues/create"));
        assert!(command.spec.refresh);
    }

    #[test]
    fn status_with_and_without_profile() {
        assert!(matches!(
            parse(&["status"]).unwrap(),
            CliCommand::Status {
                profile: None,
                only: None,
                json: false
            }
        ));
        assert!(matches!(
            parse(&["status", "dev", "--json"]).unwrap(),
            CliCommand::Status { profile: Some(name), only: None, json: true } if name == "dev"
        ));
    }

    /// `--only` narrows the rows of `status`; `--kind` is a different command,
    /// so the two cannot be given together and an unknown kind is a usage
    /// failure rather than an empty table.
    #[test]
    fn status_only_selects_one_kind_of_row() {
        assert!(matches!(
            parse(&["status", "--only", "api"]).unwrap(),
            CliCommand::Status {
                only: Some(StatusRowKind::Api),
                ..
            }
        ));
        assert!(matches!(
            parse(&["status", "--only", "auth", "--json"]).unwrap(),
            CliCommand::Status {
                only: Some(StatusRowKind::Auth),
                json: true,
                ..
            }
        ));
        assert!(parse(&["status", "--only", "db"]).is_err());
        assert!(parse(&["status", "--only", "nothing"]).is_err());
        assert!(parse(&["status", "--only", "api", "--kind", "data"]).is_err());
    }

    #[test]
    fn logout_takes_a_profile_or_all() {
        assert!(matches!(
            parse(&["logout", "ops"]).unwrap(),
            CliCommand::Logout(LogoutTarget::Profile(name)) if name == "ops"
        ));
        assert!(matches!(
            parse(&["logout", "--all"]).unwrap(),
            CliCommand::Logout(LogoutTarget::All)
        ));
        assert!(parse(&["logout"]).is_err());
        assert!(parse(&["logout", "ops", "--all"]).is_err());
    }

    #[test]
    fn no_command_opens_the_tui() {
        assert!(matches!(parse(&[]).unwrap(), CliCommand::Tui));
    }

    #[test]
    fn profile_commands_require_a_profile() {
        assert!(parse(&["env"]).is_err());
        assert!(parse(&["exec", "--", "true"]).is_err());
        assert!(parse(&["console"]).is_err());
    }

    #[test]
    fn removed_flags_and_commands_are_rejected() {
        for args in [
            &["-p", "dev"][..],
            &["--list-profiles"],
            &["-r"],
            &["session", "clear"],
            &["show"],
            &["unset", "--quiet"],
        ] {
            assert!(parse(args).is_err(), "{args:?} was accepted");
        }
    }

    #[test]
    fn parses_init_and_completions_for_zsh_only() {
        assert!(matches!(parse(&["init", "zsh"]).unwrap(), CliCommand::Init));
        assert!(matches!(
            parse(&["completions", "zsh"]).unwrap(),
            CliCommand::Completions
        ));
        assert!(parse(&["init", "bash"]).is_err());
        assert!(parse(&["init"]).is_err());
        assert!(parse(&["completions", "fish"]).is_err());
    }
}
