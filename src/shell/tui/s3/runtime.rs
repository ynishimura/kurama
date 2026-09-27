//! The S3 explorer's event loop and the task that holds the role's clients.
//!
//! As in the database explorer, a request does not hold the screen: the
//! task runs it while keys keep arriving, and sends what a search finds as
//! it finds it. `Esc` reaches the task over a channel. Each S3 request of a
//! run is awaited against that channel, so a stop drops the request in
//! flight and starts no other; what arrived before stays on screen.
//!
//! The clients are built from the role assumed before the screen opened:
//! paging, searching and previewing call neither STS nor 1Password while its
//! credentials have more than a minute left. Before a request that would
//! outlive them, the role is assumed again through the same path and the
//! clients are rebuilt with the new credentials.

use std::future::Future;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc;

use super::effects::{S3Ask, S3Effect};
use super::messages::{Answer, Answered, Failure, Found, S3Message};
use super::model::S3Model;
use super::update::{start, update};
use crate::adapters::aws::{s3_browse, s3_data::BucketClients};
use crate::adapters::clipboard::copy_to_clipboard;
use crate::adapters::error::CoreError;
use crate::domain::functions::s3_scan::{Scan, key_contains};
use crate::domain::types::Profile;
use crate::domain::types::limits::S3_READ;
use crate::domain::types::s3_browse::{S3Entry, S3Location, S3Position, S3StopReason};
use crate::ports::AwsProfileCredentials;
use crate::shell::cli::ErrorCode;
use crate::shell::cli::commands::s3_read::{self, ContentSearch, PreviewRequest};
use crate::shell::tui::event::{Event, EventHandler};
use crate::shell::tui::session::TuiApplication;
use crate::shell::tui::terminal::Terminal;

pub struct S3Runtime {
    model: S3Model,
    asks: mpsc::UnboundedSender<S3Ask>,
    stops: mpsc::UnboundedSender<()>,
    replies: mpsc::UnboundedReceiver<S3Message>,
    /// When the running request was sent; the update gets whole seconds.
    started: Option<Instant>,
}

/// The ends of the channels the task holds.
pub struct WorkerChannels {
    pub asks: mpsc::UnboundedReceiver<S3Ask>,
    pub stops: mpsc::UnboundedReceiver<()>,
    pub replies: mpsc::UnboundedSender<S3Message>,
}

impl S3Runtime {
    pub fn new(model: S3Model) -> (Self, WorkerChannels) {
        let (asks, ask_rx) = mpsc::unbounded_channel();
        let (stops, stop_rx) = mpsc::unbounded_channel();
        let (reply_tx, replies) = mpsc::unbounded_channel();
        (
            Self {
                model,
                asks,
                stops,
                replies,
                started: None,
            },
            WorkerChannels {
                asks: ask_rx,
                stops: stop_rx,
                replies: reply_tx,
            },
        )
    }

    /// Apply a message, run its effects, then draw.
    fn handle(&mut self, message: S3Message, terminal: &mut Terminal) -> Result<(), CoreError> {
        let mut pending = vec![message];
        while let Some(message) = pending.pop() {
            for effect in update(&mut self.model, message) {
                if let Some(next) = self.execute(effect) {
                    pending.push(next);
                }
            }
        }
        terminal.draw(|frame| super::view::render(frame, &self.model))
    }

    fn execute(&mut self, effect: S3Effect) -> Option<S3Message> {
        match effect {
            S3Effect::Ask(ask) => {
                self.started = Some(Instant::now());
                // The task ends only after the screen does.
                let _ = self.asks.send(ask);
                None
            }
            S3Effect::Stop => {
                let _ = self.stops.send(());
                None
            }
            S3Effect::CopyToClipboard { text } => Some(S3Message::Copied(
                copy_to_clipboard(&text).map_err(|error| error.to_string()),
            )),
        }
    }

    /// What the task sent since the last event, and the elapsed seconds of
    /// the request still running.
    fn arrived(&mut self) -> Vec<S3Message> {
        let mut messages = Vec::new();
        while let Ok(message) = self.replies.try_recv() {
            if matches!(message, S3Message::Answered(_)) {
                self.started = None;
            }
            messages.push(message);
        }
        if let (Some(started), Some(running)) = (self.started, &self.model.running) {
            let seconds = started.elapsed().as_secs();
            if seconds != running.elapsed_secs {
                messages.push(S3Message::Elapsed(seconds));
            }
        }
        messages
    }
}

#[async_trait::async_trait]
impl TuiApplication for S3Runtime {
    fn should_exit(&self) -> bool {
        self.model.should_exit
    }

    fn draw(&self, terminal: &mut Terminal) -> Result<(), CoreError> {
        terminal.draw(|frame| super::view::render(frame, &self.model))
    }

    async fn initialize(
        &mut self,
        _terminal: &mut Terminal,
        _events: &mut EventHandler,
    ) -> Result<(), CoreError> {
        for effect in start(&mut self.model) {
            self.execute(effect);
        }
        Ok(())
    }

    async fn handle_event(
        &mut self,
        event: Event,
        terminal: &mut Terminal,
        _events: &mut EventHandler,
    ) -> Result<(), CoreError> {
        for message in self.arrived() {
            self.handle(message, terminal)?;
        }
        match event {
            Event::Key(key) => self.handle(S3Message::Key(key), terminal),
            Event::Resize => self.handle(S3Message::Resize, terminal),
            Event::Tick => Ok(()),
        }
    }
}

/// The task that holds the role's clients: one request at a time, each
/// stopped by `Esc`.
pub struct Worker {
    pub clients: BucketClients,
    pub page_size: u32,
    pub role: Role,
}

/// Where the clients' credentials come from again.
pub struct Role {
    /// The cache the credentials before the screen came through
    /// (`AssumedRoles`): it hands them back while they have time left and
    /// assumes the role again when they do not.
    pub credentials: Arc<dyn AwsProfileCredentials>,
    pub profile: Profile,
}

/// The request was stopped before it ended.
struct Stopped;

enum Ended {
    Stopped,
    Failed(anyhow::Error),
}

impl From<Stopped> for Ended {
    fn from(_: Stopped) -> Self {
        Self::Stopped
    }
}

impl<E: Into<anyhow::Error>> From<E> for Ended {
    fn from(error: E) -> Self {
        Self::Failed(error.into())
    }
}

impl Worker {
    /// Answer requests until the screen closes.
    pub async fn serve(mut self, mut channels: WorkerChannels) {
        while let Some(ask) = channels.asks.recv().await {
            // A stop that arrived after the previous answer was meant for it.
            while channels.stops.try_recv().is_ok() {}
            let outcome = match self.renew().await {
                Ok(()) => self.run(&ask, &mut channels.stops, &channels.replies).await,
                Err(error) => Err(Ended::Failed(error)),
            };
            let outcome = outcome.map_err(|ended| match ended {
                Ended::Stopped => Failure {
                    message: "stopped".to_owned(),
                    stopped: true,
                },
                Ended::Failed(error) => Failure {
                    message: describe(&error),
                    stopped: false,
                },
            });
            let answer = S3Message::Answered(Answer { ask, outcome });
            if channels.replies.send(answer).is_err() {
                break;
            }
        }
    }

    /// The credentials the next request is signed with: the ones held while
    /// they have time left, else the role assumed again.
    async fn renew(&mut self) -> anyhow::Result<()> {
        let credentials = self
            .role
            .credentials
            .assume_role(&self.role.profile)
            .await?;
        self.clients.renew(&credentials);
        Ok(())
    }

    async fn run(
        &mut self,
        ask: &S3Ask,
        stops: &mut mpsc::UnboundedReceiver<()>,
        replies: &mpsc::UnboundedSender<S3Message>,
    ) -> Result<Answered, Ended> {
        let page_size = self.page_size;
        let clients = &mut self.clients;
        match ask {
            S3Ask::Buckets => {
                let buckets =
                    until_stopped(stops, s3_browse::list_buckets(clients, page_size)).await??;
                Ok(Answered::Buckets(buckets))
            }
            S3Ask::Level { location, start } => {
                let mut scan = Scan::new(start.clone(), u64::from(page_size));
                while let Some(token) = scan.next_page() {
                    let page = until_stopped(
                        stops,
                        s3_browse::list_page(
                            clients,
                            &location.bucket,
                            &location.prefix,
                            true,
                            page_size,
                            token,
                        ),
                    )
                    .await??;
                    scan.accept(page, |_| true);
                }
                let scanned = scan.finish();
                Ok(Answered::Level {
                    prefixes: scanned.prefixes,
                    objects: scanned.objects,
                    scanned: scanned.scanned,
                    next: scanned.resume,
                })
            }
            S3Ask::KeySearch {
                location,
                text,
                max_objects,
            } => {
                let mut scan = Scan::new(S3Position::default(), *max_objects);
                let mut sent = 0;
                while let Some(token) = scan.next_page() {
                    let page = until_stopped(
                        stops,
                        s3_browse::list_page(
                            clients,
                            &location.bucket,
                            &location.prefix,
                            false,
                            page_size,
                            token,
                        ),
                    )
                    .await??;
                    scan.accept(page, |entry| key_contains(entry, text));
                    let _ = replies.send(S3Message::Found(Found {
                        objects: scan.objects()[sent..].to_vec(),
                        matches: Vec::new(),
                        scanned: scan.scanned(),
                        read: 0,
                    }));
                    sent = scan.objects().len();
                }
                let complete = scan.finish().resume.is_none();
                Ok(Answered::Searched {
                    complete,
                    stop_reason: (!complete).then_some(S3StopReason::MaxObjects),
                    skipped: 0,
                })
            }
            S3Ask::ContentSearch {
                location,
                text,
                max_objects,
            } => {
                content_search(
                    clients,
                    stops,
                    replies,
                    location,
                    text,
                    *max_objects,
                    page_size,
                )
                .await
            }
            S3Ask::Preview {
                bucket,
                key,
                offset,
                if_match,
            } => {
                let request = PreviewRequest {
                    bucket,
                    key,
                    bytes: S3_READ.preview_bytes,
                    offset: *offset,
                    if_match: if_match.as_deref(),
                };
                let result = until_stopped(stops, s3_read::preview(clients, request)).await??;
                Ok(Answered::Preview(Box::new(result)))
            }
        }
    }
}

/// List the objects under the prefix to the bound, then read them one at a
/// time, sending each object's matching lines as they are found.
async fn content_search(
    clients: &mut BucketClients,
    stops: &mut mpsc::UnboundedReceiver<()>,
    replies: &mpsc::UnboundedSender<S3Message>,
    location: &S3Location,
    text: &str,
    max_objects: u64,
    page_size: u32,
) -> Result<Answered, Ended> {
    let mut scan = Scan::new(S3Position::default(), max_objects);
    while let Some(token) = scan.next_page() {
        let page = until_stopped(
            stops,
            s3_browse::list_page(
                clients,
                &location.bucket,
                &location.prefix,
                false,
                page_size,
                token,
            ),
        )
        .await??;
        scan.accept(page, |entry| matches!(entry, S3Entry::Object(_)));
    }
    let listed = scan.finish();
    let mut run = ContentSearch::default();
    let now = || chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    for object in &listed.objects {
        let before = run.matches.len();
        until_stopped(
            stops,
            run.read(clients, &location.bucket, object, text, now),
        )
        .await??;
        let _ = replies.send(S3Message::Found(Found {
            objects: Vec::new(),
            matches: run.matches[before..].to_vec(),
            scanned: listed.scanned,
            read: run.searched.len(),
        }));
    }
    let complete = listed.resume.is_none()
        && run.stop.is_none()
        && run.skipped.is_empty()
        && run.searched.iter().all(|searched| searched.full);
    let listing_stop = listed.resume.map(|_| S3StopReason::MaxObjects);
    Ok(Answered::Searched {
        complete,
        stop_reason: run.stop.or(listing_stop),
        skipped: run.skipped.len() + run.searched.iter().filter(|s| !s.full).count(),
    })
}

/// Await `request` unless a stop arrives first; the request is then
/// dropped, which cancels it. A closed channel is the screen going away,
/// which stops it too.
async fn until_stopped<T>(
    stops: &mut mpsc::UnboundedReceiver<()>,
    request: impl Future<Output = T>,
) -> Result<T, Stopped> {
    tokio::select! {
        biased;
        _ = stops.recv() => Err(Stopped),
        value = request => Ok(value),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::Credentials;
    use crate::ports::aws_credentials::MockAwsProfileCredentials;
    use crate::shell::aws_profile_credentials::AssumedRoles;

    fn expiring_in(seconds: i64) -> Credentials {
        Credentials::new(
            "ASIA".into(),
            "SECRET".into(),
            None,
            Some(chrono::Utc::now() + chrono::Duration::seconds(seconds)),
        )
    }

    /// The role assumed before the screen opened is reused while it has time
    /// left, and assumed again through the same path once it is about to end.
    async fn assumed_before_each_request(lifetime: i64) -> usize {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = calls.clone();
        let mut inner = MockAwsProfileCredentials::new();
        inner.expect_assume_role().returning(move |_| {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(expiring_in(lifetime))
        });
        let roles = Arc::new(AssumedRoles::new(Arc::new(inner)));
        let profile = Profile::new("dev");
        let credentials = roles.assume_role(&profile).await.unwrap();
        let mut worker = Worker {
            clients: BucketClients::new(&credentials, "ap-northeast-1", true, 30),
            page_size: 10,
            role: Role {
                credentials: roles,
                profile,
            },
        };
        for _ in 0..2 {
            worker.renew().await.unwrap();
        }
        calls.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[tokio::test]
    async fn s3_explorer_renews_the_role_only_when_it_is_about_to_expire() {
        assert_eq!(assumed_before_each_request(3600).await, 1);
        assert_eq!(assumed_before_each_request(30).await, 3);
    }
}
