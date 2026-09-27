//! S3 browsing: a bucket and prefix as written, the operation, the resumable cursor, the result envelope and its failures; no I/O.
use super::s3_object::{S3Match, S3NextAction, S3ObjectHead, S3Preview, S3Searched, S3Skipped};
use base64::Engine as _;
use serde::{Deserialize, Serialize};

/// Keys and common prefixes one `ListObjectsV2` page asks for, unless the
/// `[s3.*]` says otherwise.
pub const DEFAULT_PAGE_SIZE: u32 = 200;
/// The most `ListObjectsV2` returns in one page.
pub const MAX_PAGE_SIZE: u32 = 1000;
/// Entries a search examines in one run unless `--max-objects` says otherwise.
pub const DEFAULT_SEARCH_OBJECTS: u64 = 1000;
/// The most entries one run examines: everything it keeps is held in memory.
pub const MAX_OBJECTS: u64 = 100_000;
/// Seconds one S3 request may take unless the `[s3.*]` says otherwise.
pub const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 30;

/// A bucket and a key prefix, exactly as written after `s3://`: `%`, `+`,
/// spaces, `..` and repeated `/` are part of the key, never decoded or
/// normalized.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct S3Location {
    pub bucket: String,
    pub prefix: String,
}

impl S3Location {
    pub fn parse(url: &str) -> Result<Self, S3Invalid> {
        let rest = url.strip_prefix("s3://").ok_or(S3Invalid::Url)?;
        let (bucket, prefix) = rest.split_once('/').unwrap_or((rest, ""));
        if !is_bucket_name(bucket) {
            return Err(S3Invalid::Url);
        }
        Ok(Self {
            bucket: bucket.to_owned(),
            prefix: prefix.to_owned(),
        })
    }
}

/// The characters a bucket name is made of; S3 itself judges the rest.
pub fn is_bucket_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.'))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum S3Operation {
    /// `ListBuckets`, every page.
    Buckets,
    /// One level (common prefixes and objects), or every key with `recursive`.
    List,
    /// Every key under the prefix, kept when it contains the text.
    Search,
    /// One object's metadata (`HeadObject`).
    Head,
    /// A bounded range of one object's bytes, as text or a hex head.
    Preview,
    /// Every object under the prefix, read to the bounds, each line that
    /// contains the text.
    ContentSearch,
}

impl S3Operation {
    /// Every operation, in the order the contract lists them.
    pub const ALL: [Self; 6] = [
        Self::Buckets,
        Self::List,
        Self::Search,
        Self::Head,
        Self::Preview,
        Self::ContentSearch,
    ];
}

/// Where a listing goes on: the page a continuation token names, and how
/// many entries of that page were already examined. A run that stops in the
/// middle of a page asks for the same page again and skips what it saw, so
/// no entry is skipped or repeated at a page boundary.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct S3Position {
    pub token: Option<String>,
    pub skip: usize,
}

/// A position, and everything the listing it continues was asked with. The
/// page size is the one the token was issued for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct S3Cursor {
    pub bucket: String,
    pub prefix: String,
    pub operation: S3Operation,
    pub recursive: bool,
    pub search: Option<String>,
    pub page_size: u32,
    pub position: S3Position,
}

impl S3Cursor {
    pub fn encode(&self) -> String {
        let json = serde_json::to_vec(self).expect("a cursor serializes");
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
    }

    pub fn decode(text: &str) -> Result<Self, S3Invalid> {
        let json = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(text)
            .map_err(|_| S3Invalid::Cursor)?;
        let cursor: Self = serde_json::from_slice(&json).map_err(|_| S3Invalid::Cursor)?;
        if !(1..=MAX_PAGE_SIZE).contains(&cursor.page_size) {
            return Err(S3Invalid::Cursor);
        }
        Ok(cursor)
    }

    /// Whether this cursor continues the listing the run asks for.
    pub fn continues(
        &self,
        location: &S3Location,
        operation: S3Operation,
        recursive: bool,
        search: Option<&str>,
    ) -> bool {
        self.bucket == location.bucket
            && self.prefix == location.prefix
            && self.operation == operation
            && self.recursive == recursive
            && self.search.as_deref() == search
    }
}

/// One entry of a listing page, in S3's order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum S3Entry {
    /// A common prefix: a level below, ending with the delimiter.
    Prefix(String),
    Object(S3Object),
}

impl S3Entry {
    pub fn name(&self) -> &str {
        match self {
            Self::Prefix(prefix) => prefix,
            Self::Object(object) => &object.key,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct S3Object {
    pub key: String,
    pub size: u64,
    pub last_modified: Option<String>,
    pub etag: Option<String>,
    pub storage_class: Option<String>,
}

/// One `ListObjectsV2` page: common prefixes and keys merged in key order,
/// and the token of the next page when there is one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct S3Page {
    pub entries: Vec<S3Entry>,
    pub next_token: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct S3Bucket {
    pub name: String,
    pub created: Option<String>,
    pub region: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum S3StopReason {
    /// The run examined `--max-objects` entries; `cursor` continues it.
    MaxObjects,
    /// A preview showed `--bytes`, or a content search read as many bytes as
    /// one run may transfer or decode.
    MaxBytes,
    /// A gzip preview transferred as many compressed bytes as it may before
    /// the stream ended.
    MaxTransfer,
    /// A content search found as many matching lines as one run reports.
    MaxMatches,
    /// A gzip stream stopped decoding: what decoded before is shown, and the
    /// object is not a valid gzip as a whole.
    InvalidGzip,
}

/// What one run of `kurama s3` answers, or would ask with `--dry-run`.
#[derive(Clone, Debug, Serialize)]
pub struct S3Output {
    pub schema_version: u8,
    pub kind: &'static str,
    pub operation: S3Operation,
    /// The `[s3.*]` the run used.
    pub s3: String,
    pub bucket: Option<String>,
    pub prefix: Option<String>,
    pub recursive: bool,
    pub search: Option<String>,
    pub aws_profile: String,
    /// Where the bucket was read, or where the first request is signed.
    pub region: String,
    pub page_size: u32,
    /// Entries one run examines; `null` for `--buckets`, which reads every page.
    pub max_objects: Option<u64>,
    /// `--preview`: the most bytes shown, and where the range starts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset_bytes: Option<u64>,
    /// The ETag the preview reads only while the object still has.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub if_match: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<bool>,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub result: Option<S3Result>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct S3Result {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buckets: Option<Vec<S3Bucket>>,
    pub prefixes: Vec<String>,
    pub objects: Vec<S3Object>,
    /// Keys and common prefixes examined (buckets for `--buckets`).
    pub scanned_objects: u64,
    /// Nothing is left under the prefix: an empty result then means none.
    pub complete: bool,
    pub stop_reason: Option<S3StopReason>,
    /// Pass it to `--cursor` with the same target and operation to go on.
    pub cursor: Option<String>,
    /// `--head` and `--preview`: the object as `HeadObject` described it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object: Option<S3ObjectHead>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<S3Preview>,
    /// `--search-content`: the matching lines, the objects read, and those
    /// not read or not read in full, with why.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matches: Option<Vec<S3Match>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub searched: Option<Vec<S3Searched>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<Vec<S3Skipped>>,
    /// Bytes transferred from S3, and the bytes they decoded to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_read: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_decoded: Option<u64>,
    /// `--head` and `--preview`: the `kurama data` request that opens the
    /// object, when it is a format `data` reads; never started on its own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_actions: Option<Vec<S3NextAction>>,
}

#[derive(Debug, thiserror::Error)]
pub enum S3Error {
    #[error(transparent)]
    Invalid(#[from] S3Invalid),
    #[error("S3 rejected the request: {0}")]
    Rejected(String),
    #[error("S3 refused to read the object: {0}")]
    ReadRejected(String),
    #[error("the object changed: its ETag is no longer the one it was read with")]
    Changed,
    #[error("S3 answered that the bucket is not in {region} without naming its region")]
    Redirected { region: String },
    #[error("the S3 request got no answer (network or timeout)")]
    Failed,
}

#[derive(Debug, thiserror::Error)]
pub enum S3Invalid {
    #[error("no [s3.{0}] in the configuration")]
    UnknownConnection(String),
    #[error(
        "no operation: pass --buckets, --list or --search TEXT, or --head, --preview or --search-content TEXT"
    )]
    OperationRequired,
    #[error("--list and --search need a bucket")]
    BucketRequired,
    #[error("the region is unknown")]
    RegionRequired,
    #[error("TARGET must be s3://bucket or s3://bucket/prefix")]
    Url,
    #[error("--search and --search-content need a nonempty text")]
    EmptySearch,
    #[error("--head and --preview need an object key, not a prefix")]
    KeyRequired,
    #[error("a gzip object is decoded from its start: --offset-bytes cannot start inside it")]
    GzipOffset,
    #[error("--offset-bytes {offset} is past the end of the object ({size} bytes)")]
    OffsetPastEnd { offset: u64, size: u64 },
    #[error("--cursor is not a cursor kurama issued")]
    Cursor,
    #[error("--cursor continues another target or operation")]
    CursorMismatch,
}

impl S3Invalid {
    pub fn hint(&self) -> &'static str {
        match self {
            Self::UnknownConnection(_) => {
                "add an [s3.<name>] with aws_profile to config.toml; `kurama config list` shows what it has"
            }
            Self::OperationRequired => {
                "for example: kurama s3 <S3> --list, kurama s3 <S3> s3://bucket/prefix/ --search TEXT, kurama s3 <S3> --buckets"
            }
            Self::BucketRequired => {
                "pass s3://bucket/prefix as TARGET, or set bucket in the [s3.*]; --buckets lists what the role may see"
            }
            Self::RegionRequired => {
                "pass --region, or set region in the [s3.*] or in the AWS profile"
            }
            Self::Url => "write the bucket after s3://, for example s3://bucket/reports/",
            Self::EmptySearch => "pass the text a key or a line has to contain",
            Self::KeyRequired => {
                "pass s3://bucket/key as TARGET; --list shows the keys under a prefix"
            }
            Self::GzipOffset => {
                "drop --offset-bytes and raise --bytes (up to 1048576) to see further into the decoded text"
            }
            Self::OffsetPastEnd { .. } => {
                "pass an --offset-bytes below the object's size; --head reports the size"
            }
            Self::Cursor => "pass the cursor a previous run printed, unchanged",
            Self::CursorMismatch => {
                "pass the same TARGET, operation, --recursive and --search the cursor was printed for"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s3_location_keeps_the_key_as_written() {
        for (url, bucket, prefix) in [
            ("s3://bucket", "bucket", ""),
            ("s3://bucket/", "bucket", ""),
            ("s3://bucket/a//../b+c%20 d/", "bucket", "a//../b+c%20 d/"),
            ("s3://my.bucket-1/日本語 /", "my.bucket-1", "日本語 /"),
        ] {
            assert_eq!(
                S3Location::parse(url).unwrap(),
                S3Location {
                    bucket: bucket.into(),
                    prefix: prefix.into()
                }
            );
        }
        for url in [
            "bucket/key",
            "s3://",
            "s3:///key",
            "s3://bu_cket/",
            "https://b/k",
        ] {
            assert!(
                matches!(S3Location::parse(url), Err(S3Invalid::Url)),
                "{url}"
            );
        }
    }

    fn cursor() -> S3Cursor {
        S3Cursor {
            bucket: "bucket".into(),
            prefix: "日本語 +%//".into(),
            operation: S3Operation::Search,
            recursive: false,
            search: Some("inv".into()),
            page_size: 2,
            position: S3Position {
                token: Some("t/+=".into()),
                skip: 1,
            },
        }
    }

    #[test]
    fn s3_cursor_round_trips_and_refuses_anything_else() {
        let cursor = cursor();
        let text = cursor.encode();
        assert!(
            text.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')),
            "{text}"
        );
        assert_eq!(S3Cursor::decode(&text).unwrap(), cursor);
        let zero_page = S3Cursor {
            page_size: 0,
            ..cursor.clone()
        };
        for text in ["", "not a cursor", "e30", &zero_page.encode()] {
            assert!(
                matches!(S3Cursor::decode(text), Err(S3Invalid::Cursor)),
                "{text}"
            );
        }
    }

    #[test]
    fn s3_cursor_continues_only_the_listing_it_was_issued_for() {
        let cursor = cursor();
        let location = S3Location {
            bucket: "bucket".into(),
            prefix: "日本語 +%//".into(),
        };
        assert!(cursor.continues(&location, S3Operation::Search, false, Some("inv")));
        let other = S3Location {
            prefix: "日本語 +%/".into(),
            ..location.clone()
        };
        assert!(!cursor.continues(&other, S3Operation::Search, false, Some("inv")));
        assert!(!cursor.continues(&location, S3Operation::List, false, None));
        assert!(!cursor.continues(&location, S3Operation::Search, false, Some("in")));
        assert!(!cursor.continues(&location, S3Operation::Search, true, Some("inv")));
    }
}
