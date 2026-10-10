//! Command dispatch: one `CliCommand` in, one handler out.
//!
//! ```text
//! ArgMatches -> parser::parse_cli_command -> CliCommand -> execute_command
//! ```

use anyhow::Result;
use tracing::debug;

use super::client::ClientKind;
use super::command::CliCommand;
use super::commands::agent::{AGENT_GUIDE, AGENT_SKILL};
use super::commands::api::handle_api_command;
use super::commands::login::handle_login_command;
use super::commands::logout::handle_logout_command;
use super::commands::profile::{
    apply_agent_policy, check_iam_user_profile, handle_auth_profile_command,
    handle_aws_profile_command,
};
use super::commands::source::{CredentialSource, resolve_source};
use super::commands::status::handle_status_command;
use super::commands::token::handle_token_command;
use super::commands::{handle_unset_command, zsh_completions, zsh_init_script};
use super::executor::CliExecutorError;
use crate::adapters::config::Config;
use crate::shell::agent_policy::is_agent_run;
use crate::shell::runtime::Runtime;
use crate::shell::tui::handle_tui_command;

pub async fn execute_command(command: CliCommand, config: Config) -> Result<()> {
    match command {
        CliCommand::AgentContract(kind) => {
            println!(
                "{}",
                match kind {
                    ClientKind::Data => super::commands::data_contract::capabilities(),
                    ClientKind::Db => super::commands::db_contract::capabilities(),
                    ClientKind::S3 => super::commands::s3_contract::capabilities(),
                }
            );
            Ok(())
        }
        CliCommand::AgentCatalog => {
            println!("{}", super::commands::agent_catalog::catalog());
            Ok(())
        }
        CliCommand::Data(command) => super::commands::data::run(*command, config).await,
        CliCommand::Db(command) => super::commands::db::run(*command, config).await,
        CliCommand::ClientStatus {
            kind,
            profile,
            json,
        } => match kind {
            ClientKind::Data => {
                super::commands::data_contract::print_status(&config, profile.as_deref(), json)
            }
            ClientKind::Db => {
                super::commands::db_contract::print_status(&config, profile.as_deref(), json)
            }
            ClientKind::S3 => {
                super::commands::s3_contract::print_status(&config, profile.as_deref(), json)
            }
        },
        CliCommand::Init { completion_only } => {
            let binary = std::env::current_exe()?;
            if completion_only {
                print!("{}", zsh_completions(&binary));
            } else {
                print!("{}", zsh_init_script(&binary));
            }
            Ok(())
        }
        CliCommand::AgentGuide => {
            print!("{AGENT_GUIDE}");
            Ok(())
        }
        CliCommand::AgentSkill => {
            print!("{AGENT_SKILL}");
            Ok(())
        }
        CliCommand::Inventory => {
            println!("{}", super::commands::inventory::document());
            Ok(())
        }
        CliCommand::Profile(profile) => {
            debug!(?profile, "Executing profile command");
            match resolve_source(&config, &profile.profile_name).await? {
                CredentialSource::Aws(aws_profile) => {
                    let profile =
                        apply_agent_policy(profile, &config, is_agent_run(), &aws_profile)?;
                    check_iam_user_profile(&profile, &aws_profile)?;
                    let runtime = Runtime::from_config(config, &aws_profile).await;
                    handle_aws_profile_command(profile, &runtime).await
                }
                CredentialSource::Auth(source) => {
                    handle_auth_profile_command(profile, source, config).await
                }
            }
        }
        CliCommand::Login {
            profile,
            force,
            no_browser,
            verbose,
        } => handle_login_command(&profile, force, no_browser, verbose, config).await,
        CliCommand::Token {
            profile,
            json,
            fingerprint,
            verbose,
        } => handle_token_command(&profile, json, fingerprint, verbose, config).await,
        CliCommand::Api(command) => handle_api_command(*command, config).await,
        CliCommand::Status {
            profile,
            only,
            json,
        } => handle_status_command(profile.as_deref(), only, json, &config).await,
        CliCommand::Logout(target) => handle_logout_command(&target, &config).await,
        CliCommand::Unset => handle_unset_command(&config),
        CliCommand::Config(command) => command.run().await,
        CliCommand::Preset(command) => command.run().await,
        CliCommand::Tui => {
            // An agent or a pipe must get an answer instead of a blank TUI.
            if !crate::shell::tui::terminal::supports_tui() {
                return Err(CliExecutorError::TerminalRequired.into());
            }
            handle_tui_command(config).await
        }
        CliCommand::S3(command) => super::commands::s3::run(*command, config).await,
        CliCommand::StatusReady { json } => super::commands::status_ready::run(json, &config).await,
        CliCommand::Audit { watch: true, .. } => {
            crate::shell::tui::activity::watch_audit_log().await
        }
        CliCommand::Audit { since, json, .. } => super::commands::audit::run(since, json),
        CliCommand::Mcp { listen } => super::commands::mcp::run(listen, &config).await,
        CliCommand::AgentInstall(install) => {
            super::commands::agent_install::run(&install, config).await
        }
        CliCommand::Obsidian(command) => super::commands::obsidian::run(command, &config).await,
    }
}
