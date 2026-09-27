//! Explorer messages: keys and the results of its effects.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use crossterm::event::KeyEvent;

use super::history::HistoryEntry;
use crate::domain::types::api_spec::ApiSpec;

#[derive(Debug, Clone)]
pub enum ExplorerMessage {
    Key(KeyEvent),
    Resize,
    /// The clock, read by the runtime on every tick.
    Tick(DateTime<Utc>),
    /// When the stored OAuth token of the API's source ends, if one is stored.
    TokenRead(Option<DateTime<Utc>>),
    /// The description, or why it could not be loaded.
    SpecLoaded(Result<Arc<ApiSpec>, String>),
    /// The API's answer, or why the request failed (with its hint).
    ResponseReceived(Result<ResponseInfo, String>),
    /// The body after `$EDITOR`.
    BodyEdited(Result<String, String>),
    Copied(Result<(), String>),
    /// The jq path `CopyPath` put on the clipboard, or why it could not.
    PathCopied(Result<String, String>),
    /// The lines a jq filter produced on the response body.
    JqApplied(Result<Vec<String>, String>),
    JqPreviewed(Result<Vec<String>, String>),
    JqPreviewUnavailable(&'static str),
    /// The API's request history, oldest first.
    HistoryLoaded(Vec<HistoryEntry>),
    /// A line for the header, from an effect.
    Notice(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseInfo {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub elapsed_ms: u128,
}
