//! S3 explorer messages: keys, the clock of a running request, what a search found so far, and the answers of the task.

use crossterm::event::KeyEvent;

use super::effects::S3Ask;
use crate::domain::types::s3_browse::{S3Bucket, S3Object, S3Position, S3Result, S3StopReason};
use crate::domain::types::s3_object::S3Match;

pub enum S3Message {
    Key(KeyEvent),
    Resize,
    /// Whole seconds the running request has taken.
    Elapsed(u64),
    /// Part of what the running search found; more may follow.
    Found(Found),
    /// The running request ended.
    Answered(Answer),
    Copied(Result<(), String>),
}

/// What a search found since the last `Found`, and how far it got.
pub struct Found {
    pub objects: Vec<S3Object>,
    pub matches: Vec<S3Match>,
    /// Keys and prefixes the listing examined so far.
    pub scanned: u64,
    /// Objects a content search has read so far.
    pub read: usize,
}

pub struct Answer {
    pub ask: S3Ask,
    pub outcome: Result<Answered, Failure>,
}

pub enum Answered {
    Buckets(Vec<S3Bucket>),
    /// One page of a level and where the next one starts.
    Level {
        prefixes: Vec<String>,
        objects: Vec<S3Object>,
        scanned: u64,
        next: Option<S3Position>,
    },
    /// A search ran to its end: whether nothing was left unread, and the
    /// bound that stopped it.
    Searched {
        complete: bool,
        stop_reason: Option<S3StopReason>,
        skipped: usize,
    },
    Preview(Box<S3Result>),
}

/// Why a request failed, as the screen shows it.
pub struct Failure {
    /// The code, the message and the hint.
    pub message: String,
    /// The person asked for the stop (`Esc`): what arrived stays, and the
    /// stop is a line under it, not an error box.
    pub stopped: bool,
}
