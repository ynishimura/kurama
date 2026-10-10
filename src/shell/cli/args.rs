//! Command-line definition (clap builder).
//!
//! Every command names the credential source (an AWS profile or an
//! `[auth.*]` source) or the API as a positional argument. Without a
//! command, kurama opens the TUI.

use clap::{Arg, ArgAction, Command};
use clap_complete::engine::ArgValueCandidates;

use crate::domain::functions::completion_candidates::ProfileScope;

use super::completion;

const AFTER_HELP: &str = "\
Without a command, kurama opens the TUI; it needs a terminal.

PROFILE is an AWS profile in ~/.aws/config or an [auth.*] source in
~/.config/kurama/config.toml; API is an [api.*] profile there.";

const AGENTS_HELP: &str = "\
Agents and scripts: `kurama agent` prints the contract (commands, JSON, exit
codes, setup); `kurama agent --json` the catalog of every subcommand;
`kurama agent --skill` an Agent Skill that points at the contract.";

/// The text after the options: the exit codes come from `ErrorCode::EXITS`,
/// so `--help` and the catalog cannot describe them differently.
fn after_help() -> String {
    let exits: String = super::error_code::ErrorCode::EXITS
        .iter()
        .map(|(exit, meaning)| format!("\n  {exit}  {meaning}"))
        .collect();
    format!("{AFTER_HELP}\n\nExit codes:{exits}\n\n{AGENTS_HELP}")
}

pub fn build_command() -> Command {
    Command::new("kurama")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Credential switcher and API client for AWS profiles and OAuth sources")
        .after_help(after_help())
        .subcommand(build_env_command())
        .subcommand(build_exec_command())
        .subcommand(build_console_command())
        .subcommand(build_login_command())
        .subcommand(build_logout_command())
        .subcommand(build_token_command())
        .subcommand(super::commands::api_command::command())
        .subcommand(super::commands::data_command::command())
        .subcommand(super::commands::db_command::command())
        .subcommand(build_status_command())
        .subcommand(
            Command::new("unset")
                .about("Print unset lines for every variable `kurama env` exports"),
        )
        .subcommand(build_agent_command())
        .subcommand(build_init_command())
        .subcommand(build_inventory_command())
        .subcommand(super::commands::config::command())
        .subcommand(super::commands::preset::command())
        .subcommand(super::commands::s3_command::command())
        .subcommand(super::commands::audit::command())
        .subcommand(super::commands::mcp::command())
        .subcommand(super::commands::obsidian::command())
}

const SOURCE_HELP: &str = "AWS profile in ~/.aws/config, or an [auth.*] source in config.toml";

fn profile_arg(help: &'static str, scope: ProfileScope) -> Arg {
    Arg::new("profile")
        .value_name("PROFILE")
        .help(help)
        .add(ArgValueCandidates::new(move || completion::profiles(scope)))
        .required(true)
}

fn readonly_arg() -> Arg {
    Arg::new("readonly")
        .long("readonly")
        .short('r')
        .help("Attach the ReadOnlyAccess policy to the role session")
        .action(ArgAction::SetTrue)
}

/// `-v` on the commands that read a credential: one `< secret read:` line
/// per store read, as `api -v` prints them.
fn verbose_arg() -> Arg {
    Arg::new("verbose")
        .short('v')
        .long("verbose")
        .help("Print each secret store read on stderr (never the value)")
        .action(ArgAction::SetTrue)
}

fn build_env_command() -> Command {
    Command::new("env")
        .about("Export the source's credentials into the current shell")
        .long_about(
            "Export the source's credentials into the current shell.\n\n\
             An AWS profile is assumed and its role credentials exported as AWS_*; an [auth.*]\n\
             source's token goes into its env_var (default KURAMA_TOKEN). The shell function\n\
             from `kurama init zsh` sources the export script. Without it, the script goes to\n\
             stdout: eval \"$(command kurama env <PROFILE>)\".",
        )
        .arg(profile_arg(SOURCE_HELP, ProfileScope::All))
        .arg(readonly_arg())
        .arg(verbose_arg())
        .arg(
            Arg::new("json")
                .long("json")
                .help("Print JSON on stdout instead of the export script (credential_process for AWS)")
                .action(ArgAction::SetTrue),
        )
}

fn build_exec_command() -> Command {
    Command::new("exec")
        .about("Run a command with the source's credentials in its environment")
        .long_about(
            "Run a command with the source's credentials in its environment.\n\n\
             kurama replaces itself with the command, so its exit code and signals are the\n\
             command's own. The current shell is not changed.",
        )
        .arg(profile_arg(SOURCE_HELP, ProfileScope::All))
        .arg(readonly_arg())
        .arg(
            Arg::new("confirm")
                .long("confirm")
                .help("A person agreed: in a KURAMA_AGENT run, keep the role's own permissions instead of the ReadOnlyAccess [agent] exec_readonly attaches")
                .action(ArgAction::SetTrue),
        )
        .arg(verbose_arg())
        .arg(
            Arg::new("command")
                .value_name("COMMAND")
                .help("Command and arguments, after --")
                .required(true)
                .num_args(1..)
                .value_hint(clap::ValueHint::AnyPath)
                .last(true),
        )
}

fn build_login_command() -> Command {
    Command::new("login")
        .about("Log in: cache an MFA session (AWS) or store an OAuth token ([auth.*])")
        .long_about(
            "Log in: cache an MFA session (AWS) or store an OAuth token ([auth.*]).\n\n\
             Nothing is requested while a cached session or stored token is still valid. Every\n\
             AWS profile that uses the same MFA device shares the session. An [auth.*] source\n\
             runs its grant: authorization_code opens the browser, device_code shows a code,\n\
             client_credentials needs nobody.",
        )
        .arg(profile_arg(SOURCE_HELP, ProfileScope::All))
        .arg(
            Arg::new("force")
                .long("force")
                .help("Get a new session or token even if the stored one is still valid")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("no-browser")
                .long("no-browser")
                .help("[auth.*] only: print the authorization URL instead of opening the browser; the login still waits in this terminal for the callback")
                .action(ArgAction::SetTrue),
        )
        .arg(verbose_arg())
}

fn build_token_command() -> Command {
    Command::new("token")
        .about("Print the access token of an [auth.*] source on stdout")
        .long_about(
            "Print the access token of an [auth.*] source on stdout.\n\n\
             The stored token is refreshed or replaced when needed. Without a terminal, a\n\
             grant that needs a person stops with exit code 3 and a hint to run `kurama login`.",
        )
        .arg(profile_arg(
            "[auth.*] source in config.toml",
            ProfileScope::Auth,
        ))
        .arg(
            Arg::new("json")
                .long("json")
                .help(
                    "Print {access_token, token_type, expires_at, scope} instead of the bare token",
                )
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("fingerprint")
                .long("fingerprint")
                .help("Print sha256:<hex> of the credential instead of the credential, to tell whether two sources hold one value; with --json, {fingerprint}")
                .action(ArgAction::SetTrue),
        )
        .arg(verbose_arg())
}

fn build_console_command() -> Command {
    Command::new("console")
        .about("Assume the profile's role and open the AWS console in the browser")
        .arg(profile_arg(
            "AWS profile in ~/.aws/config",
            ProfileScope::Aws,
        ))
        .arg(readonly_arg())
}

fn build_status_command() -> Command {
    Command::new("status")
        .arg(
            Arg::new("kind")
                .long("kind")
                .value_parser(super::client::ClientKind::values())
                .help("List what one JSON client has configured: data workspaces, databases or S3 connections"),
        )
        .arg(
            Arg::new("only")
                .long("only")
                .value_name("KIND")
                .value_parser(super::commands::status::StatusRowKind::selectable_values())
                // --kind is a different command, and silently dropping this
                // flag there would be worse than refusing the pair.
                .conflicts_with("kind")
                .help("Only rows of this kind (databases: --kind db)"),
        )
        .about(
            "List AWS profiles, [auth.*] sources and [api.*] profiles with their state; no network",
        )
        .arg(
            Arg::new("profile")
                .value_name("PROFILE")
                .help("Only this profile, source or API")
                .add(ArgValueCandidates::new(|| {
                    completion::profiles(ProfileScope::All)
                })),
        )
        .arg(
            Arg::new("ready")
                .long("ready")
                .action(ArgAction::SetTrue)
                .conflicts_with_all(["kind", "only", "profile"])
                .help("Say whether each source can be used right now (ready, will_prompt, needs_human, misconfigured) and what a person runs when not"),
        )
        .arg(
            Arg::new("json")
                .long("json")
                .help("Print one JSON array (with --ready, one JSON document)")
                .action(ArgAction::SetTrue),
        )
}

fn build_logout_command() -> Command {
    Command::new("logout")
        .about("Remove the cached MFA session (AWS) or the stored token ([auth.*])")
        .arg(
            Arg::new("profile")
                .value_name("PROFILE")
                .help("AWS profile in ~/.aws/config, or an [auth.*] source in config.toml")
                .add(ArgValueCandidates::new(|| {
                    completion::profiles(ProfileScope::All)
                }))
                .required_unless_present("all")
                .conflicts_with("all"),
        )
        .arg(
            Arg::new("all")
                .long("all")
                .help("Remove every MFA session and every stored token")
                .action(ArgAction::SetTrue),
        )
}

/// Shell name positional argument (zsh only)
fn build_shell_arg() -> Arg {
    Arg::new("shell")
        .help("Shell type")
        .value_name("SHELL")
        .value_parser(["zsh"])
        .required(true)
}

fn build_init_command() -> Command {
    Command::new("init")
        .about("Print shell integration script (use: eval \"$(kurama init zsh)\")")
        .arg(build_shell_arg())
        .arg(
            Arg::new("completion-only")
                .long("completion-only")
                .action(ArgAction::SetTrue)
                .help("Print only the completion function, for a completer kept in fpath"),
        )
}

fn build_agent_command() -> Command {
    Command::new("agent")
        .arg(Arg::new("kind").long("kind").value_parser(super::client::ClientKind::values()).requires("json").conflicts_with("skill").help("Select the contract of one JSON client: data, db or s3"))
        .arg(Arg::new("json").long("json").action(ArgAction::SetTrue).conflicts_with("skill").help("Print the selected contract as JSON; without --kind, the catalog of every subcommand (arguments, exit codes, which subcommands answer errors as JSON)"))
        .about("Print the contract for agents and scripts: commands, JSON fields, exit codes, setup of [auth.*] and [api.*]")
        .long_about(
            "Print the contract for agents and scripts: the commands and their stdout, the\n\
             `status --json` fields, `kurama api`, the exit codes, and how to add an [auth.*]\n\
             source or an [api.*] profile to config.toml. It is docs/agents/kurama/, embedded,\n\
             so it describes this binary. Reads no configuration.\n\n\
             --skill prints an Agent Skill (SKILL.md) that tells an agent to read the contract\n\
             first. `kurama agent install` writes it, and one Skill per [api.*] with a\n\
             description, under ~/.claude/skills.",
        )
        .arg(
            Arg::new("skill")
                .long("skill")
                .help("Print the Agent Skill (SKILL.md) that points at the contract instead")
                .action(ArgAction::SetTrue),
        )
        .args_conflicts_with_subcommands(true)
        .subcommand(
            Command::new("install")
                .about("Write kurama's Agent Skill and one per [api.*] with a description under ~/.claude/skills")
                .long_about(
                    "Write kurama's Agent Skill (what `agent --skill` prints) as kurama/SKILL.md and,\n\
                     for each [api.*] with openapi, discovery or graphql, what `api <name> --skill`\n\
                     prints as kurama-api-<name>/SKILL.md, under ~/.claude/skills or --dir. A file\n\
                     that already holds the same bytes is not touched, one that differs is replaced,\n\
                     and nothing else is written or removed. An API without a description, or whose\n\
                     description cannot be loaded, is skipped with the reason. Descriptions load as\n\
                     `api --skill` loads them: the cached copy, fetched when missing or due. Prints\n\
                     one row per Skill: written, unchanged or skipped.",
                )
                .arg(
                    Arg::new("dir")
                        .long("dir")
                        .value_name("DIR")
                        .value_hint(clap::ValueHint::DirPath)
                        .value_parser(clap::value_parser!(std::path::PathBuf))
                        .help("Where the Skill directories go (default ~/.claude/skills)"),
                )
                .arg(
                    Arg::new("offline")
                        .long("offline")
                        .action(ArgAction::SetTrue)
                        .help("Read descriptions from the cache only; fetch nothing"),
                )
                .arg(
                    Arg::new("dry-run")
                        .long("dry-run")
                        .action(ArgAction::SetTrue)
                        .help("Report what would be written and write nothing; descriptions are read from the cache only, as with --offline"),
                )
                .arg(
                    Arg::new("json")
                        .long("json")
                        .action(ArgAction::SetTrue)
                        .help("Print one JSON document"),
                ),
        )
}

/// Hidden: `cargo xtask inventory` reads it; a person or an agent has
/// `--help` and `kurama agent`.
fn build_inventory_command() -> Command {
    Command::new("inventory")
        .hide(true)
        .about("Print the commands, options, config keys and enum values this binary has, as JSON")
        .long_about(
            "Print what this binary has, as one JSON document: every command and option\n\
             (from the clap definition), every config.toml key with its kind and the values\n\
             an enum key accepts (from the config types), the secret reference schemes and\n\
             the JSON client kinds. Reads no configuration and no AWS environment. It is what\n\
             `cargo xtask inventory` compares the feature map and the verification cases against.",
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_is_hidden_and_takes_nothing() {
        let command = build_command();
        let inventory = command.find_subcommand("inventory").unwrap();
        assert!(inventory.is_hide_set());
        assert_eq!(inventory.get_arguments().count(), 0);
    }

    #[test]
    fn exec_command_keeps_path_completion_after_the_double_dash() {
        let command = build_command();
        let exec = command.find_subcommand("exec").unwrap();
        let command = exec
            .get_arguments()
            .find(|argument| argument.get_id().as_str() == "command")
            .unwrap();
        assert_eq!(command.get_value_hint(), clap::ValueHint::AnyPath);
    }
}
