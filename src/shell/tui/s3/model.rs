//! The S3 explorer's model: where it is (the bucket list, a level, a key or content search), the rows that arrived, the local filter, the search form, the preview, the running request, and what the view reads off them. No I/O, no clock.

use super::effects::S3Ask;
use crate::domain::types::s3_browse::{S3Bucket, S3Location, S3Object, S3Position, S3StopReason};
use crate::domain::types::s3_object::{S3Match, S3NextAction};
use crate::shell::tui::components::LineInput;

/// What the header says about the connection.
pub struct S3Summary {
    /// The `[s3.*]` name.
    pub name: String,
    pub aws_profile: String,
    /// Where the first request is signed; a bucket elsewhere is followed.
    pub region: String,
    /// Keys a listing page asks for.
    pub page_size: u32,
}

/// One row of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Bucket(S3Bucket),
    /// A common prefix: a level below.
    Prefix(String),
    Object(S3Object),
    /// A line a content search found.
    Match(S3Match),
}

impl Row {
    /// The text the local filter reads and the list shows first.
    pub fn name(&self) -> &str {
        match self {
            Self::Bucket(bucket) => &bucket.name,
            Self::Prefix(prefix) => prefix,
            Self::Object(object) => &object.key,
            Self::Match(found) => &found.key,
        }
    }
}

/// What the list shows. The three ways of narrowing it are told apart on
/// screen: the local filter (`/`) reads only the rows here, a key search
/// (`s`) lists the whole prefix, a content search (`g`) reads the objects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum View {
    Buckets,
    Level,
    KeySearch(String),
    ContentSearch(String),
}

/// How the rows on screen came to an end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum End {
    /// Nothing is left: an empty list means none.
    Complete,
    /// A level has more pages; moving to the last row loads the next.
    More,
    /// A bound stopped it; more may exist.
    Bound(S3StopReason),
    /// `Esc` stopped it: what arrived before stays.
    Stopped,
    /// S3 refused or did not answer; the error box said why.
    Failed,
}

pub struct Listing {
    pub view: View,
    /// `None` for the bucket list.
    pub location: Option<S3Location>,
    pub rows: Vec<Row>,
    /// Keys and prefixes examined (buckets for the bucket list).
    pub scanned: u64,
    /// Objects a content search read.
    pub read: usize,
    /// Objects a content search did not read, or not in full.
    pub skipped: usize,
    /// Where the next page of a level starts.
    pub next: Option<S3Position>,
    /// `None` while it is still arriving.
    pub end: Option<End>,
}

impl Listing {
    pub fn new(view: View, location: Option<S3Location>) -> Self {
        Self {
            view,
            location,
            rows: Vec::new(),
            scanned: 0,
            read: 0,
            skipped: 0,
            next: None,
            end: None,
        }
    }
}

/// The form a key or content search starts from: nothing is read until
/// Enter confirms it.
pub struct SearchForm {
    /// A content search when true, a key search otherwise.
    pub content: bool,
    pub location: S3Location,
    pub text: LineInput,
    pub max_objects: LineInput,
    /// 0: the text, 1: the bound.
    pub field: usize,
    pub error: Option<String>,
}

/// What was read of one object.
pub struct PreviewPane {
    pub bucket: String,
    pub key: String,
    pub result: crate::domain::types::s3_browse::S3Result,
    /// The lines shown, terminal-safe.
    pub lines: Vec<String>,
    pub scroll: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    List,
    Preview,
}

pub struct Running {
    pub ask: S3Ask,
    pub elapsed_secs: u64,
    /// `Esc` was pressed; the answer is on its way.
    pub stopping: bool,
}

pub enum S3Modal {
    Help,
    Error(String),
    /// The `kurama data` request that opens an object; nothing runs it.
    Handoff(S3NextAction),
}

pub struct S3Model {
    pub summary: S3Summary,
    pub listing: Listing,
    /// Indices into `listing.rows` that match the filter.
    pub filtered: Vec<usize>,
    /// Index into `filtered`.
    pub selected: usize,
    pub filter: LineInput,
    pub filtering: bool,
    pub form: Option<SearchForm>,
    pub preview: Option<PreviewPane>,
    pub focus: Focus,
    pub running: Option<Running>,
    pub modal: Option<S3Modal>,
    /// One line in the header until the next key.
    pub notice: Option<String>,
    pub should_exit: bool,
}

impl S3Model {
    /// A model that starts at `location`, or at the bucket list.
    pub fn new(summary: S3Summary, location: Option<S3Location>) -> Self {
        let view = if location.is_some() {
            View::Level
        } else {
            View::Buckets
        };
        Self {
            summary,
            listing: Listing::new(view, location),
            filtered: Vec::new(),
            selected: 0,
            filter: LineInput::default(),
            filtering: false,
            form: None,
            preview: None,
            focus: Focus::List,
            running: None,
            modal: None,
            notice: None,
            should_exit: false,
        }
    }

    pub fn selected_row(&self) -> Option<&Row> {
        self.listing.rows.get(*self.filtered.get(self.selected)?)
    }

    /// The filter is on the rows already here; S3 is not asked.
    pub(super) fn apply_filter(&mut self) {
        let query = self.filter.as_str().to_lowercase();
        self.filtered = self
            .listing
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| query.is_empty() || row.name().to_lowercase().contains(&query))
            .map(|(index, _)| index)
            .collect();
        self.selected = self.selected.min(self.filtered.len().saturating_sub(1));
    }
}

/// `s3://bucket/prefix` as written.
pub fn location_uri(location: &S3Location) -> String {
    format!("s3://{}/{}", location.bucket, location.prefix)
}

/// The level above: the prefix without its last segment, or the bucket
/// list above a bucket's root.
pub fn parent(location: &S3Location) -> Option<S3Location> {
    if location.prefix.is_empty() {
        return None;
    }
    let trimmed = location
        .prefix
        .strip_suffix('/')
        .unwrap_or(&location.prefix);
    let prefix = match trimmed.rfind('/') {
        Some(at) => trimmed[..=at].to_owned(),
        None => String::new(),
    };
    Some(S3Location {
        bucket: location.bucket.clone(),
        prefix,
    })
}

/// A bound as the screen names it.
pub fn stop_name(reason: S3StopReason) -> &'static str {
    match reason {
        S3StopReason::MaxObjects => "max_objects",
        S3StopReason::MaxBytes => "max_bytes",
        S3StopReason::MaxTransfer => "max_transfer",
        S3StopReason::MaxMatches => "max_matches",
        S3StopReason::InvalidGzip => "invalid_gzip",
    }
}

/// The line under the list: how far the rows go and how they ended.
pub fn listing_status(listing: &Listing) -> String {
    let counted = match listing.view {
        View::Buckets => format!("{} buckets", listing.rows.len()),
        View::ContentSearch(_) => format!(
            "{} lines · {}/{} objects read",
            listing.rows.len(),
            listing.read,
            listing.scanned
        ),
        _ => format!("{} scanned", listing.scanned),
    };
    let end = match &listing.end {
        None => "arriving…".to_owned(),
        Some(End::Complete) if listing.skipped > 0 => {
            format!("{} not read in full", listing.skipped)
        }
        Some(End::Complete) => "complete".to_owned(),
        Some(End::More) => "more below".to_owned(),
        Some(End::Bound(reason)) => format!("stopped at {}; more may exist", stop_name(*reason)),
        Some(End::Stopped) => "stopped by Esc; partial result".to_owned(),
        Some(End::Failed) => "failed; partial result".to_owned(),
    };
    format!("{counted} · {end}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(prefix: &str) -> S3Location {
        S3Location {
            bucket: "b".into(),
            prefix: prefix.into(),
        }
    }

    #[test]
    fn s3_parent_drops_the_last_segment_as_written() {
        assert_eq!(parent(&at("a/b/")), Some(at("a/")));
        assert_eq!(parent(&at("a/")), Some(at("")));
        assert_eq!(parent(&at("a//")), Some(at("a/")));
        assert_eq!(parent(&at("logs")), Some(at("")));
        assert_eq!(parent(&at("")), None);
    }
}
