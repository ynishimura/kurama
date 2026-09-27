//! TUI presentation layer
//!
//! This module provides the terminal user interface for KURAMA: the TEA
//! runtime in `tea/`, the shared terminal session and input event loop, plus
//! the design system every screen is built from: `theme` (colors, styles,
//! borders), `layout` (breakpoints and regions) and `components` (widgets).
//! `testing` holds the render helpers, fixtures and snapshot assertions used
//! by the view tests.

pub mod activity;
pub mod components;
pub mod database;
pub mod event;
pub mod explorer;
pub mod layout;
pub mod s3;
pub mod session;
pub mod tea;
pub mod terminal;
#[cfg(test)]
pub mod testing;
pub mod theme;

use crate::adapters::config::Config;
use anyhow::Result;

use session::TuiSession;
use tea::update::Handoff;

/// Run the TUI application. A row that hands off to another command
/// (`login`, the API or database explorer) leaves the screen, runs that
/// command as `kurama <args>` would, and opens the screen again on the same
/// tab; its failure ends kurama like the command's own. The palette's
/// operation or history entry opens the explorer on that form.
pub async fn handle_tui_command(config: Config) -> Result<()> {
    let mut tab = tea::sources::Tab::default();
    loop {
        // The CLI and the tests use the same TEA implementation. The Runtime
        // is created per selected profile so source-profile credentials stay
        // correct.
        let mut app = tea::TeaRuntime::with_config(config.clone()).on_tab(tab);
        let mut session = TuiSession::new()?;
        session.run(&mut app).await?;
        session.close();

        // stdout or KURAMA_ENV_SCRIPT must be written after leaving the
        // alternate screen so shell output is not hidden by terminal
        // restoration.
        app.flush_output()?;

        let Some((handoff, from)) = app.handoff() else {
            return Ok(());
        };
        tab = from;
        match handoff {
            Handoff::Run(args) => run_handoff(&args, config.clone()).await?,
            Handoff::Explore { api, entry } => {
                crate::shell::cli::commands::api::handle_explorer_at(&api, entry, config.clone())
                    .await?
            }
        }
    }
}

/// `kurama <args>`, parsed and dispatched as the command line would be.
async fn run_handoff(args: &[String], config: Config) -> Result<()> {
    let matches = crate::shell::cli::build_command()
        .try_get_matches_from(std::iter::once("kurama").chain(args.iter().map(String::as_str)))?;
    let command = crate::shell::cli::parser::parse_cli_command(&matches);
    Box::pin(crate::shell::cli::dispatch::execute_command(
        command, config,
    ))
    .await
}
