//! The explorer's event loop and effects: load the description, send the
//! request through `ApiRuntime`, run `$EDITOR`, the clipboard, the browser
//! and jq. Draws once after every message and once more after its
//! effects, never while idle. An applied jq filter runs on its own thread
//! and is polled on every event, so keys still act while it runs.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use super::effects::ExplorerEffect;
use super::jq_input::{
    APPLY_TIMEOUT, APPLY_TIMEOUT_MESSAGE, PREVIEW_TIMEOUT, PREVIEW_TIMEOUT_MESSAGE,
};
use super::messages::{ExplorerMessage, ResponseInfo};
use super::model::{ApiSummary, ExplorerModel};
use super::update::update;
use crate::adapters::browser::open_url;
use crate::adapters::clipboard::copy_to_clipboard;
use crate::adapters::config::ApiProfile;
use crate::adapters::editor::edit_text;
use crate::adapters::error::CoreError;
use crate::adapters::jq::{ParsedInput, apply_filter_parsed, apply_filter_until, parse_input};
use crate::adapters::request_history::{append_history, history_file, read_history};
use crate::domain::functions::api_request::body_as_json;
use crate::domain::types::auth_source::AuthSource;
use crate::domain::types::request_history::HistoryEntry;
use crate::shell::api_runtime::ApiRuntime;
use crate::shell::cli::ErrorCode;
use crate::shell::cli::commands::api_spec::print_warnings;
use crate::shell::spec_loader::load_spec_verbose;
use crate::shell::tui::event::{Event, EventHandler};
use crate::shell::tui::session::TuiApplication;
use crate::shell::tui::terminal::Terminal;

pub struct ExplorerRuntime {
    model: ExplorerModel,
    api: ApiProfile,
    runtime: ApiRuntime,
    verbose: bool,
    preview_input: Option<(Arc<serde_json::Value>, Arc<ParsedInput>)>,
    pending_jq: Option<PendingJq>,
}

/// An applied jq filter running on its own thread. The thread is never
/// joined, so quitting does not wait for it; dropping this raises the flag
/// that ends an endless filter.
struct PendingJq {
    result: mpsc::Receiver<Result<Vec<String>, String>>,
    stop: Arc<AtomicBool>,
    deadline: Instant,
}

impl PendingJq {
    /// Run `filter` on `body`; `wake` gets a `Tick` once the lines are
    /// there, so they are shown without waiting for the next tick.
    fn start(filter: String, body: Vec<u8>, timeout: Duration, wake: mpsc::Sender<Event>) -> Self {
        let (sender, result) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        std::thread::spawn(move || {
            let lines = match body_as_json(&body) {
                Some(value) => apply_filter_until(&filter, &value, &flag),
                None => Err("the response body is not JSON".into()),
            };
            // The receiver is gone once the deadline passed.
            let _ = sender.send(lines);
            let _ = wake.send(Event::Tick);
        });
        Self {
            result,
            stop,
            deadline: Instant::now() + timeout,
        }
    }

    /// The filter's lines once they are there, that it stopped once the
    /// deadline has passed, `None` while it runs.
    fn poll(&self, now: Instant) -> Option<Result<Vec<String>, String>> {
        match self.result.try_recv() {
            Ok(lines) => Some(lines),
            Err(_) if now >= self.deadline => Some(Err(APPLY_TIMEOUT_MESSAGE.into())),
            Err(_) => None,
        }
    }
}

impl Drop for PendingJq {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl ExplorerRuntime {
    pub fn new(api: ApiProfile, runtime: ApiRuntime, verbose: bool) -> Self {
        let mut model = ExplorerModel::new(ApiSummary {
            name: api.name.clone(),
            base_url: api.base_url.clone(),
            auth: api.auth.clone(),
            aws_profile: api.aws_profile.clone(),
            headers: api.headers.clone(),
        });
        // The first frame already counts from now, not from the first tick.
        model.now = chrono::Utc::now();
        Self {
            model,
            api,
            runtime,
            verbose,
            preview_input: None,
            pending_jq: None,
        }
    }

    /// Open the form of `start` once the description is loaded.
    pub fn open_on(&mut self, start: Option<HistoryEntry>) {
        self.model.start = start;
    }

    /// Apply a message, draw, run its effects (and the messages they
    /// produce), draw again.
    pub async fn handle(
        &mut self,
        message: ExplorerMessage,
        terminal: &mut Terminal,
        events: &mut EventHandler,
    ) -> Result<(), CoreError> {
        let mut queue = VecDeque::from([message]);
        while let Some(message) = queue.pop_front() {
            let (model, effects) = update(self.model.clone(), message);
            self.model = model;
            // Show the new state before its effects run, so the Sending
            // modal is on screen while the API answers.
            terminal.draw(|frame| super::view::render(frame, &self.model))?;
            for effect in effects {
                if let Some(next) = self.execute(effect, terminal, events).await? {
                    queue.push_back(next);
                }
            }
        }
        terminal.draw(|frame| super::view::render(frame, &self.model))?;
        Ok(())
    }

    async fn run_effects(
        &mut self,
        effects: Vec<ExplorerEffect>,
        terminal: &mut Terminal,
        events: &mut EventHandler,
    ) -> Result<(), CoreError> {
        for effect in effects {
            if let Some(message) = self.execute(effect, terminal, events).await? {
                self.handle(message, terminal, events).await?;
            }
        }
        Ok(())
    }

    /// Run one effect; the message it answers with, if any.
    async fn execute(
        &mut self,
        effect: ExplorerEffect,
        terminal: &mut Terminal,
        events: &mut EventHandler,
    ) -> Result<Option<ExplorerMessage>, CoreError> {
        let message = match effect {
            ExplorerEffect::LoadSpec => {
                let result = load_spec_verbose(
                    &self.runtime,
                    &self.api,
                    false,
                    "the explorer",
                    crate::shell::spec_loader::SpecAccess::WithCredential,
                    self.verbose,
                )
                .await;
                // Keys typed while loading were meant for the loading screen.
                events.drain();
                ExplorerMessage::SpecLoaded(
                    result
                        .map(|loaded| {
                            // Held while the TUI owns the terminal: printed after exit.
                            print_warnings(&loaded.spec);
                            loaded.spec
                        })
                        .map_err(|error| describe(&error)),
                )
            }
            ExplorerEffect::ReadTokenExpiry => {
                ExplorerMessage::TokenRead(token_expiry(&self.runtime, &self.api).await)
            }
            ExplorerEffect::SendRequest(request) => {
                let started = Instant::now();
                let result = self.runtime.call(&self.api, request).await;
                // The Sending modal ignores keys; the ones typed meanwhile
                // must not act on the result.
                events.drain();
                ExplorerMessage::ResponseReceived(
                    result
                        .map(|response| ResponseInfo {
                            status: response.status,
                            headers: response.headers,
                            body: response.body,
                            elapsed_ms: started.elapsed().as_millis(),
                        })
                        .map_err(|error| describe(&error)),
                )
            }
            ExplorerEffect::EditBody { text, suffix } => {
                // The editor owns the terminal and stdin meanwhile.
                events.pause();
                terminal.suspend()?;
                let edited = edit_text(&text, suffix);
                let resumed = terminal.resume();
                events.resume();
                resumed?;
                ExplorerMessage::BodyEdited(edited.map_err(|error| error.to_string()))
            }
            ExplorerEffect::CopyToClipboard { text } => {
                ExplorerMessage::Copied(copy_to_clipboard(&text).map_err(|error| error.to_string()))
            }
            ExplorerEffect::CopyPath { path } => ExplorerMessage::PathCopied(
                copy_to_clipboard(&path)
                    .map(|()| path)
                    .map_err(|error| error.to_string()),
            ),
            ExplorerEffect::OpenBrowser { url } => ExplorerMessage::Notice(match open_url(&url) {
                Ok(()) => format!("opened {url}"),
                Err(error) => format!("browser: {error}"),
            }),
            ExplorerEffect::ApplyJq { filter, body } => {
                // Replacing a running filter drops it, which stops it.
                self.pending_jq = Some(PendingJq::start(
                    filter,
                    body,
                    APPLY_TIMEOUT,
                    events.waker(),
                ));
                return Ok(None);
            }
            ExplorerEffect::PreviewJq { filter, body } => {
                let cached = self
                    .preview_input
                    .as_ref()
                    .and_then(|(cached_body, input)| {
                        Arc::ptr_eq(cached_body, &body).then(|| Arc::clone(input))
                    });
                let input = match cached {
                    Some(input) => input,
                    None => {
                        let input = match parse_input(&body) {
                            Ok(input) => Arc::new(input),
                            Err(error) => {
                                return Ok(Some(ExplorerMessage::JqPreviewed(Err(error))));
                            }
                        };
                        self.preview_input = Some((Arc::clone(&body), Arc::clone(&input)));
                        input
                    }
                };
                match tokio::time::timeout(
                    PREVIEW_TIMEOUT,
                    tokio::task::spawn_blocking(move || {
                        apply_filter_parsed(
                            &filter,
                            &input,
                            Some(crate::shell::tui::layout::MAX_JQ_PREVIEW_ROWS + 1),
                        )
                    }),
                )
                .await
                {
                    Err(_) => ExplorerMessage::JqPreviewUnavailable(PREVIEW_TIMEOUT_MESSAGE),
                    Ok(Ok(result)) => ExplorerMessage::JqPreviewed(result),
                    Ok(Err(error)) => ExplorerMessage::JqPreviewed(Err(error.to_string())),
                }
            }
            ExplorerEffect::LoadHistory => {
                match history_file(&self.api.name).and_then(|path| Ok(read_history(&path)?)) {
                    Ok(entries) => ExplorerMessage::HistoryLoaded(entries),
                    Err(error) => ExplorerMessage::Notice(format!("history: {error}")),
                }
            }
            ExplorerEffect::AppendHistory(entry) => {
                match history_file(&self.api.name)
                    .and_then(|path| Ok(append_history(&path, &entry)?))
                {
                    Ok(()) => return Ok(None),
                    Err(error) => ExplorerMessage::Notice(format!("history: {error}")),
                }
            }
            ExplorerEffect::Exit => return Ok(None),
        };
        Ok(Some(message))
    }
}

#[async_trait::async_trait]
impl TuiApplication for ExplorerRuntime {
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
        self.run_effects(vec![ExplorerEffect::LoadSpec], terminal, events)
            .await
    }

    async fn handle_event(
        &mut self,
        event: Event,
        terminal: &mut Terminal,
        events: &mut EventHandler,
    ) -> Result<(), CoreError> {
        if let Some(lines) = self
            .pending_jq
            .as_ref()
            .and_then(|jq| jq.poll(Instant::now()))
        {
            self.pending_jq = None;
            self.handle(ExplorerMessage::JqApplied(lines), terminal, events)
                .await?;
        }
        let message = match event {
            Event::Key(key) => ExplorerMessage::Key(key),
            Event::Resize => ExplorerMessage::Resize,
            Event::Tick => ExplorerMessage::Tick(chrono::Utc::now()),
        };
        self.handle(message, terminal, events).await
    }
}

/// When the stored token of the API's OAuth source ends. A token that
/// cannot be read shows no time: the header is not where that is reported,
/// the call that needs the token is.
async fn token_expiry(
    runtime: &ApiRuntime,
    api: &ApiProfile,
) -> Option<chrono::DateTime<chrono::Utc>> {
    let Ok(Some(AuthSource::OAuth(client))) = runtime.auth_source(api) else {
        return None;
    };
    runtime
        .token_store
        .load(&client.name)
        .await
        .ok()
        .flatten()?
        .expires_at
}

/// An error as the modal shows it: the code, the message, the hint.
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
    use crate::shell::api_error::ApiError;

    #[test]
    fn an_applied_filter_answers_with_its_lines() {
        let (wake, woken) = mpsc::channel();
        let jq = PendingJq::start(
            ".a".into(),
            br#"{"a": 1}"#.to_vec(),
            Duration::from_secs(60),
            wake,
        );
        // The loop is woken once the lines are there.
        assert!(matches!(
            woken.recv_timeout(Duration::from_secs(10)),
            Ok(Event::Tick)
        ));
        assert_eq!(jq.poll(Instant::now()), Some(Ok(vec!["1".to_string()])));
    }

    #[test]
    fn a_filter_past_its_deadline_answers_that_it_stopped() {
        let started = Instant::now();
        let (wake, _woken) = mpsc::channel();
        let jq = PendingJq::start(
            "repeat(.)".into(),
            b"1".to_vec(),
            Duration::from_millis(50),
            wake,
        );
        assert_eq!(jq.poll(started), None, "still running");
        assert_eq!(
            jq.poll(started + Duration::from_secs(1)),
            Some(Err(APPLY_TIMEOUT_MESSAGE.to_string()))
        );
        // Dropping it raises the stop flag the endless filter reads.
        let stop = Arc::clone(&jq.stop);
        drop(jq);
        assert!(stop.load(Ordering::SeqCst));
    }

    #[test]
    fn errors_are_described_with_their_code_and_hint() {
        let error: anyhow::Error = ApiError::SpecRequired {
            api: "pets".into(),
            needed: "the explorer".into(),
        }
        .into();
        assert_eq!(
            describe(&error),
            "API_SPEC_REQUIRED: [api.pets] has no openapi description; the explorer needs one\n\
             hint: add openapi = \"<URL or file>\" under [api.pets] in config.toml"
        );
        assert_eq!(describe(&anyhow::anyhow!("boom")), "INTERNAL: boom");
    }
}
