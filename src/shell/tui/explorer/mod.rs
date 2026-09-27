//! `kurama api <API>` on a terminal: the OpenAPI explorer.
//!
//! The same TEA shape as the home screen (`tea/`): `update` and `view` are
//! pure, `runtime` runs the effects with the `ApiRuntime` the CLI uses, so
//! a call from the explorer is the call `kurama api <API> <OP> -P ...`
//! makes, and `c` copies exactly that command.

pub mod effects;
pub mod history;
pub mod history_view;
pub mod jq_input;
pub mod jq_view;
pub mod json_tree;
pub mod messages;
pub mod model;
pub mod runtime;
#[cfg(test)]
pub mod testing;
pub mod update;
pub mod view;

use anyhow::Result;

use crate::adapters::config::ApiProfile;
use crate::domain::types::request_history::HistoryEntry;
use crate::shell::api_error::ApiError;
use crate::shell::api_runtime::ApiRuntime;
use crate::shell::tui::session::TuiSession;

pub async fn handle_explorer_command(
    api: ApiProfile,
    mut runtime: ApiRuntime,
    verbose: bool,
    start: Option<HistoryEntry>,
) -> Result<()> {
    if api.spec.is_none() {
        return Err(ApiError::SpecRequired {
            api: api.name,
            needed: "the explorer".into(),
        }
        .into());
    }
    // A grant that needs a person is reported with its login hint instead
    // of prompting under the alternate screen.
    runtime.set_interactive(false);

    let mut app = runtime::ExplorerRuntime::new(api, runtime, verbose);
    app.open_on(start);
    let mut session = TuiSession::new()?;
    session.run(&mut app).await?;
    Ok(())
}
