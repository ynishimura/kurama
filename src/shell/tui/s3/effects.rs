//! S3 explorer effects: what the runtime does for the pure update.

use crate::domain::types::s3_browse::{S3Location, S3Position};

/// One request to the task that holds the role's clients. At most one runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum S3Ask {
    /// Every bucket `ListBuckets` answers.
    Buckets,
    /// One page of one level, from `start`.
    Level {
        location: S3Location,
        start: S3Position,
    },
    /// Every key under the prefix that contains `text`, to `max_objects`
    /// examined; what it keeps arrives as it is found.
    KeySearch {
        location: S3Location,
        text: String,
        max_objects: u64,
    },
    /// The objects under the prefix, to `max_objects` listed, read one at a
    /// time; the matching lines arrive object by object.
    ContentSearch {
        location: S3Location,
        text: String,
        max_objects: u64,
    },
    /// One range of one object, from `offset`, while its ETag is `if_match`.
    Preview {
        bucket: String,
        key: String,
        offset: u64,
        if_match: Option<String>,
    },
}

impl S3Ask {
    /// How the screen names what is running.
    pub fn label(&self) -> String {
        match self {
            Self::Buckets => "listing the buckets".to_owned(),
            Self::Level { .. } => "listing".to_owned(),
            Self::KeySearch { .. } => "searching the keys".to_owned(),
            Self::ContentSearch { .. } => "searching the contents".to_owned(),
            Self::Preview { .. } => "reading the object".to_owned(),
        }
    }
}

pub enum S3Effect {
    /// Send the request to the task.
    Ask(S3Ask),
    /// Stop the running request (`Esc`): no further request starts, and the
    /// one in flight is dropped.
    Stop,
    CopyToClipboard {
        text: String,
    },
}
