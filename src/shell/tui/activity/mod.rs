//! `kurama audit --watch`: the activity monitor, the audit log shown live as calls are appended to it.
//!
//! The same TEA shape as the explorers: `update` and `view` are pure,
//! `runtime` reads the log and runs the clipboard. The screen reads the log
//! and never writes it; the configuration is read only for the `base_url`
//! of each `[api.*]`, so the command it copies names the path the way
//! `kurama api` takes it, and a configuration that does not load leaves the
//! whole path in it.

pub mod runtime;
#[cfg(test)]
pub mod testing;
pub mod update;
pub mod view;
#[cfg(test)]
mod view_snapshot_tests;

use anyhow::Result;

use crate::adapters::audit_log::audit_file;
use crate::adapters::config::Config;
use crate::shell::cli::executor::CliExecutorError;
use crate::shell::tui::session::TuiSession;
use runtime::ActivityRuntime;
use update::ActivityModel;

pub async fn watch_audit_log() -> Result<()> {
    // An agent or a pipe must get an answer instead of a blank screen.
    if !crate::shell::tui::terminal::supports_tui() {
        return Err(CliExecutorError::TerminalRequired.into());
    }
    let file = audit_file()?;
    let config = Config::load().await.unwrap_or_default();
    let base_paths = config
        .api_profiles()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|api| {
            let path = url::Url::parse(&api.base_url).ok()?.path().to_owned();
            Some((api.name, path))
        })
        .collect();
    let mut app = ActivityRuntime::new(
        ActivityModel {
            base_paths,
            ..Default::default()
        },
        file,
    );
    TuiSession::new()?.run(&mut app).await?;
    Ok(())
}
