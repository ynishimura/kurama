//! The database explorer's event loop and the task that holds the
//! connection.
//!
//! Unlike the API explorer, a request does not hold the screen: the task
//! runs it while keys keep arriving, so `Esc` can stop it. The screen sends
//! one request at a time over a channel and reads the answer on the next
//! event (a key, or the tick that also redraws the elapsed seconds).
//!
//! The task keeps one connection for the whole session and, before every
//! request, forgets an earlier stop; after it, the session ends the request
//! (a server's read guard is rolled back, and the next read opens it again).
//! A connection that was lost, or whose statement nobody could stop, is
//! dropped and opened again on the next request, once; the statement is
//! never run again on its own.

use std::time::Instant;

use tokio::sync::mpsc;

use super::effects::{DbAsk, DbEffect};
use super::messages::{Answer, Answered, DbMessage, Failure};
use super::model::DbModel;
use super::update::{start, update};
use crate::adapters::clipboard::copy_to_clipboard;
use crate::adapters::database::{DbSession, DbTarget, Page};
use crate::adapters::editor::edit_text;
use crate::adapters::error::CoreError;
use crate::domain::types::database::{DbError, DbFailure, DbLimits};
use crate::domain::types::limits;
use crate::shell::cli::ErrorCode;
use crate::shell::db_connection::{Waited, wait_for_statement};
use crate::shell::tui::event::{Event, EventHandler};
use crate::shell::tui::session::TuiApplication;
use crate::shell::tui::terminal::Terminal;

pub struct DbRuntime {
    model: DbModel,
    asks: mpsc::UnboundedSender<DbAsk>,
    stops: mpsc::UnboundedSender<()>,
    answers: mpsc::UnboundedReceiver<Answer>,
    /// When the running request was sent; the update gets whole seconds.
    started: Option<Instant>,
}

/// The ends of the channels the connection's task holds.
pub struct WorkerChannels {
    pub asks: mpsc::UnboundedReceiver<DbAsk>,
    pub stops: mpsc::UnboundedReceiver<()>,
    pub answers: mpsc::UnboundedSender<Answer>,
}

impl DbRuntime {
    pub fn new(model: DbModel) -> (Self, WorkerChannels) {
        let (asks, ask_rx) = mpsc::unbounded_channel();
        let (stops, stop_rx) = mpsc::unbounded_channel();
        let (answer_tx, answers) = mpsc::unbounded_channel();
        (
            Self {
                model,
                asks,
                stops,
                answers,
                started: None,
            },
            WorkerChannels {
                asks: ask_rx,
                stops: stop_rx,
                answers: answer_tx,
            },
        )
    }

    /// Apply a message, run its effects (and the messages they produce),
    /// then draw.
    async fn handle(
        &mut self,
        message: DbMessage,
        terminal: &mut Terminal,
        events: &mut EventHandler,
    ) -> Result<(), CoreError> {
        let mut pending = vec![message];
        while let Some(message) = pending.pop() {
            let effects = update(&mut self.model, message);
            for effect in effects {
                if let Some(next) = self.execute(effect, terminal, events)? {
                    pending.push(next);
                }
            }
        }
        terminal.draw(|frame| super::view::render(frame, &self.model))
    }

    fn execute(
        &mut self,
        effect: DbEffect,
        terminal: &mut Terminal,
        events: &mut EventHandler,
    ) -> Result<Option<DbMessage>, CoreError> {
        Ok(match effect {
            DbEffect::Ask(ask) => {
                self.started = Some(Instant::now());
                // The task ends only after the screen does, so the send
                // cannot fail while the screen runs.
                let _ = self.asks.send(ask);
                None
            }
            DbEffect::Stop => {
                let _ = self.stops.send(());
                None
            }
            DbEffect::CopyToClipboard { text } => Some(DbMessage::Copied(
                copy_to_clipboard(&text).map_err(|error| error.to_string()),
            )),
            DbEffect::EditSql { text } => {
                // The editor owns the terminal and stdin meanwhile.
                events.pause();
                terminal.suspend()?;
                let edited = edit_text(&text, "sql");
                let resumed = terminal.resume();
                events.resume();
                resumed?;
                Some(DbMessage::SqlEdited(
                    edited.map_err(|error| error.to_string()),
                ))
            }
        })
    }

    /// The answers that arrived since the last event, and the elapsed
    /// seconds of the one still running.
    fn arrived(&mut self) -> Vec<DbMessage> {
        let mut messages = Vec::new();
        while let Ok(answer) = self.answers.try_recv() {
            self.started = None;
            messages.push(DbMessage::Answered(answer));
        }
        if let (Some(started), Some(running)) = (self.started, &self.model.running) {
            let seconds = started.elapsed().as_secs();
            if seconds != running.elapsed_secs {
                messages.push(DbMessage::Elapsed(seconds));
            }
        }
        messages
    }
}

#[async_trait::async_trait]
impl TuiApplication for DbRuntime {
    fn should_exit(&self) -> bool {
        self.model.should_exit
    }

    fn draw(&self, terminal: &mut Terminal) -> Result<(), CoreError> {
        terminal.draw(|frame| super::view::render(frame, &self.model))
    }

    async fn initialize(
        &mut self,
        terminal: &mut Terminal,
        events: &mut EventHandler,
    ) -> Result<(), CoreError> {
        for effect in start(&mut self.model) {
            self.execute(effect, terminal, events)?;
        }
        Ok(())
    }

    async fn handle_event(
        &mut self,
        event: Event,
        terminal: &mut Terminal,
        events: &mut EventHandler,
    ) -> Result<(), CoreError> {
        // Each arrived message is handled and drawn; an idle tick draws
        // nothing.
        for message in self.arrived() {
            self.handle(message, terminal, events).await?;
        }
        match event {
            Event::Key(key) => self.handle(DbMessage::Key(key), terminal, events).await,
            Event::Resize => self.handle(DbMessage::Resize, terminal, events).await,
            Event::Tick => Ok(()),
        }
    }
}

/// The task that holds the connection: one request at a time, each under
/// the deadline of the database and stopped by `Esc`.
pub struct Worker {
    pub target: DbTarget,
    pub session: Option<DbSession>,
    pub limits: DbLimits,
}

impl Worker {
    /// Answer requests until the screen closes, then close the connection.
    pub async fn serve(mut self, mut channels: WorkerChannels) {
        while let Some(ask) = channels.asks.recv().await {
            let answer = self.answer(ask, &mut channels.stops).await;
            if channels.answers.send(answer).is_err() {
                break;
            }
        }
        if let Some(session) = self.session.take() {
            session.close().await;
        }
    }

    async fn answer(&mut self, ask: DbAsk, stops: &mut mpsc::UnboundedReceiver<()>) -> Answer {
        let started = Instant::now();
        // A stop that arrived after the previous answer was meant for it.
        while stops.try_recv().is_ok() {}
        let outcome = self.run(&ask, stops).await.map_err(|error| Failure {
            stopped: matches!(error, DbError::Failed(DbFailure::Interrupted { .. })),
            message: describe(&error.into()),
        });
        Answer {
            ask,
            outcome,
            elapsed_ms: started.elapsed().as_millis() as u64,
        }
    }

    async fn run(
        &mut self,
        ask: &DbAsk,
        stops: &mut mpsc::UnboundedReceiver<()>,
    ) -> Result<Answered, DbError> {
        if self.session.is_none() {
            self.session = Some(DbSession::open(self.target.clone(), false).await?);
        }
        let session = self.session.as_mut().expect("opened above");
        let cancel = session.cancellation();
        cancel.rearm();
        let Waited { outcome, abandoned } = wait_for_statement(
            perform(session, ask, &self.limits),
            &cancel,
            self.limits.query_timeout_secs,
            async {
                // A closed channel is the screen going away: stop too.
                let _ = stops.recv().await;
            },
            false,
        )
        .await;
        let lost = matches!(outcome, Err(DbError::Failed(DbFailure::Disconnected)));
        if abandoned || lost {
            // The statement nobody could stop still holds this connection,
            // and a lost one holds nothing: the next request opens another.
            self.session = None;
        } else if session.end_request().await.is_err() {
            self.session = None;
        }
        outcome
    }
}

/// One request on the open session; the same calls the CLI makes.
async fn perform(
    session: &mut DbSession,
    ask: &DbAsk,
    limits: &DbLimits,
) -> Result<Answered, DbError> {
    match ask {
        DbAsk::Tables { after } => {
            let page = Page {
                after: after.clone(),
                limit: limits::DB_CALL.list_page,
            };
            let listing = session.tables(None, &page).await?;
            Ok(Answered::Listing {
                result: listing.result,
                next: listing.next,
            })
        }
        DbAsk::Describe { schema, table } => session
            .describe(Some(schema), table)
            .await
            .map(Answered::Rows),
        DbAsk::Preview { schema, table } => session
            .preview(Some(schema), table, &[], limits)
            .await
            .map(Answered::Rows),
        DbAsk::Query { sql } => session.query(sql, &[], limits).await.map(Answered::Rows),
    }
}

/// An error as the screen shows it: the code, the message, the hint.
fn describe(error: &anyhow::Error) -> String {
    let code = ErrorCode::classify(error);
    let mut text = format!("{code}: {error:#}");
    if let Some(hint) = code.hint(error) {
        text.push_str(&format!("\nhint: {hint}"));
    }
    text
}
