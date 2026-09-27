//! The activity monitor's event loop: the audit log read again on the tick after it changed, and the clipboard.
//!
//! The log is only read, never written. Each tick (every 250 ms) compares the
//! current file's length and modification time with the ones last read, so
//! an idle log costs one `stat` a tick and draws nothing, and an appended
//! entry is on screen within the next tick.

use std::path::PathBuf;

use super::update::{ActivityEffect, ActivityMessage, ActivityModel, update};
use crate::adapters::audit_log::{log_version, read_entries};
use crate::adapters::clipboard::copy_to_clipboard;
use crate::adapters::error::CoreError;
use crate::shell::tui::event::{Event, EventHandler};
use crate::shell::tui::session::TuiApplication;
use crate::shell::tui::terminal::Terminal;

type Version = Option<(u64, std::time::SystemTime)>;

pub struct ActivityRuntime {
    model: ActivityModel,
    file: PathBuf,
    /// The version of the log last read; `None` before the first read.
    read: Option<Version>,
}

impl ActivityRuntime {
    pub fn new(model: ActivityModel, file: PathBuf) -> Self {
        Self {
            model,
            file,
            read: None,
        }
    }

    /// The log, when it changed since it was last read.
    fn read_if_changed(&mut self) -> Option<ActivityMessage> {
        let version = log_version(&self.file);
        if self.read == Some(version) {
            return None;
        }
        self.read = Some(version);
        Some(ActivityMessage::Read(
            read_entries(&self.file).map_err(|error| error.to_string()),
        ))
    }

    /// Apply a message and the messages its effects produce, then draw.
    fn handle(
        &mut self,
        message: ActivityMessage,
        terminal: &mut Terminal,
    ) -> Result<(), CoreError> {
        let mut pending = vec![message];
        while let Some(message) = pending.pop() {
            for effect in update(&mut self.model, message) {
                match effect {
                    ActivityEffect::CopyToClipboard { text } => {
                        pending.push(ActivityMessage::Copied(
                            copy_to_clipboard(&text)
                                .map(|()| text)
                                .map_err(|error| error.to_string()),
                        ))
                    }
                }
            }
        }
        self.draw(terminal)
    }
}

#[async_trait::async_trait]
impl TuiApplication for ActivityRuntime {
    fn should_exit(&self) -> bool {
        self.model.should_exit
    }

    fn draw(&self, terminal: &mut Terminal) -> Result<(), CoreError> {
        terminal.draw(|frame| super::view::render(frame, &self.model))
    }

    async fn initialize(
        &mut self,
        terminal: &mut Terminal,
        _events: &mut EventHandler,
    ) -> Result<(), CoreError> {
        match self.read_if_changed() {
            Some(message) => self.handle(message, terminal),
            None => Ok(()),
        }
    }

    async fn handle_event(
        &mut self,
        event: Event,
        terminal: &mut Terminal,
        _events: &mut EventHandler,
    ) -> Result<(), CoreError> {
        match event {
            Event::Key(key) => self.handle(ActivityMessage::Key(key), terminal),
            Event::Resize => self.draw(terminal),
            Event::Tick => match self.read_if_changed() {
                Some(message) => self.handle(message, terminal),
                None => Ok(()),
            },
        }
    }
}
