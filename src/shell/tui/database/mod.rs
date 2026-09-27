//! `kurama db <DB>` on a terminal: the database explorer.
//!
//! The same TEA shape as the API explorer (`explorer/`): `update` and `view`
//! are pure, `runtime` runs the effects. Everything that can fail before a
//! person has anything to look at -- the secrets, the tunnel, the first
//! connection -- happens here, before the screen opens, so it ends as the
//! CLI's own `error[CODE]` and a 1Password prompt is never under the
//! alternate screen. The connection and the tunnel are closed however the
//! screen ends.

pub mod effects;
pub mod messages;
pub mod model;
pub mod runtime;
#[cfg(test)]
pub mod testing;
pub mod update;
pub mod view;

use anyhow::Result;

use crate::adapters::config::{Config, DbConnection};
use crate::adapters::database::DbSession;
use crate::shell::db_connection::open_target;
use crate::shell::tui::session::TuiSession;
use model::{DbModel, DbSummary};
use runtime::{DbRuntime, Worker};

pub async fn handle_db_explorer(
    name: &str,
    connection: DbConnection,
    config: &Config,
) -> Result<()> {
    let (target, tunnel) = open_target(&connection, config).await?;
    let session = match DbSession::open(target.clone(), false).await {
        Ok(session) => session,
        Err(error) => {
            if let Some(tunnel) = tunnel {
                tunnel.close().await;
            }
            return Err(error.into());
        }
    };
    let engine = session.engine();
    let summary = DbSummary {
        name: name.to_owned(),
        engine: match engine.server_version {
            Some(version) => format!("{} {version}", engine.name),
            None => engine.name,
        },
        database: connection.database(),
        tunnel: tunnel
            .as_ref()
            .map(|tunnel| format!("via ssm {}", tunnel.instance_id)),
    };
    let (mut app, channels) = DbRuntime::new(DbModel::new(summary));
    let worker = tokio::spawn(
        Worker {
            target,
            session: Some(session),
            limits: connection.limits().clone(),
        }
        .serve(channels),
    );
    let shown = match TuiSession::new() {
        Ok(mut screen) => screen.run(&mut app).await,
        Err(error) => Err(error),
    };
    // Whatever ended the screen, the running statement is stopped (the task
    // reads the closed stop channel as a stop), the connection closed and
    // the tunnel's session terminated.
    drop(app);
    let _ = worker.await;
    if let Some(tunnel) = tunnel {
        tunnel.close().await;
    }
    shown?;
    Ok(())
}
