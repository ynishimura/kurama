//! `kurama s3 <S3>` on a terminal: the S3 explorer.
//!
//! The same TEA shape as the database explorer (`database/`): `update` and
//! `view` are pure, `runtime` runs the effects and holds the task that
//! makes the S3 requests. The role is assumed by the caller before the
//! screen opens, so an MFA prompt is never under the alternate screen, and
//! every request of the session uses those clients until their credentials
//! are about to end; then the task assumes the role again.

pub mod effects;
pub mod messages;
pub mod model;
pub mod runtime;
#[cfg(test)]
pub mod testing;
pub mod update;
pub mod view;

use anyhow::Result;

use crate::adapters::aws::s3_data::BucketClients;
use crate::domain::types::s3_browse::S3Location;
use crate::shell::tui::session::TuiSession;
use model::{S3Model, S3Summary};
pub use runtime::Role;
use runtime::{S3Runtime, Worker};

pub async fn handle_s3_explorer(
    summary: S3Summary,
    start: Option<S3Location>,
    clients: BucketClients,
    role: Role,
) -> Result<()> {
    let page_size = summary.page_size;
    let (mut app, channels) = S3Runtime::new(S3Model::new(summary, start));
    let worker = tokio::spawn(
        Worker {
            clients,
            page_size,
            role,
        }
        .serve(channels),
    );
    let shown = match TuiSession::new() {
        Ok(mut screen) => screen.run(&mut app).await,
        Err(error) => Err(error),
    };
    // Whatever ended the screen, the task reads the closed channels as a
    // stop and ends; nothing it read is kept.
    drop(app);
    let _ = worker.await;
    shown?;
    Ok(())
}
