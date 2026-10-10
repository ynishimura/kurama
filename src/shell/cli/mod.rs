//! Command-line interface: argument definition, parsing, bootstrap and
//! dispatch, plus the effect-based command handlers.
//!
//! ```text
//! main -> build_command (args) -> run:
//!   parser::parse_cli_command -> CliCommand
//!   bootstrap::bootstrap        -> Config (skipped for init)
//!   dispatch::execute_command   -> commands::* -> effects -> executor
//! ```
//!
//! Errors propagate as `anyhow::Error` with the typed cause kept in the
//! chain; `main` turns them into `error[CODE]` lines through `ErrorCode`.

pub mod args;
pub mod arguments;
pub mod bootstrap;
pub mod client;
mod client_error;
pub mod command;
pub mod commands;
mod completion;
pub mod dispatch;
pub mod effects;
pub mod error_code;
pub mod executor;
pub mod parser;

pub use args::build_command;
pub use client::{ClientInvocation, ClientKind, JsonErrorKind, classify_invocation};
pub use error_code::ErrorCode;

use crate::adapters::config::Config;

/// Parse, bootstrap when the command needs it, execute.
pub async fn run(matches: &clap::ArgMatches) -> anyhow::Result<()> {
    let command = parser::parse_cli_command(matches);

    // `init` runs at every shell startup, and `agent` is what an
    // agent reads to write the configuration: no config or logging for any.
    // A run under the JSON error contract keeps stderr to its error document
    // too, unless a person may have to read a login prompt there.
    let quiet = command.answers_with_json()
        || (command.leaves_stderr_to_json_errors()
            && !crate::shell::tui::terminal::stderr_is_a_terminal());
    let _quiet = quiet.then(crate::console::silence);
    let config = if command.needs_bootstrap() {
        bootstrap::bootstrap().await?
    } else {
        Config::default()
    };

    tracing::debug!("Parsed command: {:?}", command);
    if let Some(call) = command.contract().audited {
        crate::shell::audit::begin(call, &config, crate::shell::agent_policy::is_agent_run());
    }
    let result = dispatch::execute_command(command, config).await;
    let code = result.as_ref().err().map(ErrorCode::classify);
    crate::shell::audit::finish(
        Some(code.map_or(0, ErrorCode::exit_code)),
        code.map(ErrorCode::as_str),
    );
    result
}
