//! What `kurama s3` reports about one object's contents: its metadata, a bounded preview, content-search matches and why an object was not read.
use serde::Serialize;

/// What `HeadObject` says about one object.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct S3ObjectHead {
    pub key: String,
    pub size: u64,
    pub content_type: Option<String>,
    pub content_encoding: Option<String>,
    pub last_modified: Option<String>,
    pub etag: Option<String>,
    /// S3 names every class but `STANDARD`, which is what it means by none.
    pub storage_class: String,
    /// `x-amz-restore` as S3 wrote it, for an archived object.
    pub restore: Option<String>,
    /// The Intelligent-Tiering archive tier an object sits in.
    pub archive_status: Option<String>,
}

impl S3ObjectHead {
    /// Whether the contents cannot be read until the object is restored.
    pub fn archived(&self) -> bool {
        self.archive_status.is_some()
            || (is_archive_class(&self.storage_class)
                && !self
                    .restore
                    .as_deref()
                    .is_some_and(|r| r.contains("ongoing-request=\"false\"")))
    }

    /// Whether the bytes are a gzip stream to decode before they are text.
    pub fn gzip(&self) -> bool {
        is_gzip(
            &self.key,
            self.content_type.as_deref(),
            self.content_encoding.as_deref(),
        )
    }
}

/// The storage classes whose objects have to be restored before a GET.
pub fn is_archive_class(class: &str) -> bool {
    matches!(class, "GLACIER" | "DEEP_ARCHIVE")
}

/// A gzip stream: the encoding, the type or the `.gz` name says so.
pub fn is_gzip(key: &str, content_type: Option<&str>, content_encoding: Option<&str>) -> bool {
    content_encoding.is_some_and(|e| e.eq_ignore_ascii_case("gzip"))
        || content_type.is_some_and(|t| matches!(t, "application/gzip" | "application/x-gzip"))
        || key.ends_with(".gz")
}

/// How a preview's text is laid out, from the key and the type; the bytes
/// are shown as text whatever it says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum S3TextFormat {
    Json,
    Jsonl,
    Yaml,
    Csv,
    Text,
}

/// What a preview found in the bytes it read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum S3PreviewKind {
    /// The object has no bytes; nothing was fetched.
    Empty,
    /// UTF-8 text, in `text`.
    Text,
    /// Not UTF-8, or a NUL: only the first bytes, in `hex`.
    Binary,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct S3Preview {
    pub kind: S3PreviewKind,
    pub format: S3TextFormat,
    /// `gzip` when the bytes were decoded from the start, else `identity`.
    pub encoding: &'static str,
    /// The byte range of the object that was transferred, end exclusive.
    pub range_start: u64,
    pub range_end: u64,
    /// Where the next `--offset-bytes` starts: after the last whole UTF-8
    /// character shown. `null` at the end of the object, and for gzip.
    pub next_offset: Option<u64>,
    /// Bytes skipped at the start because the offset fell inside a character.
    pub utf8_head_skipped: usize,
    /// Bytes held back at the end because the range cut a character.
    pub utf8_tail_cut: usize,
    /// For JSON: whether the text shown is one whole document. For JSONL:
    /// whether every whole line is one. `null` for other formats.
    pub well_formed: Option<bool>,
    pub text: Option<String>,
    pub hex: Option<String>,
}

/// Why an object's contents were not read, or not read in full.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum S3SkipReason {
    /// An archive storage class that has not been restored.
    Archived,
    /// Larger than one object may transfer.
    TooLarge,
    /// Not UTF-8 text once decoded.
    Binary,
    /// A gzip stream that does not decode.
    InvalidGzip,
    /// Its ETag is no longer the one it was listed with.
    Changed,
    /// The search stopped before it reached this object.
    NotReached,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct S3Skipped {
    pub key: String,
    pub reason: S3SkipReason,
}

/// One object a content search read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct S3Searched {
    pub key: String,
    pub etag: Option<String>,
    /// Every byte of the object was searched; `false` when it decoded to more
    /// than one object may, or the match bound stopped the search in it.
    pub full: bool,
    pub bytes_read: u64,
    pub bytes_decoded: u64,
    pub matches: usize,
}

/// One matching line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct S3Match {
    pub bucket: String,
    pub key: String,
    /// 1-based line of the decoded text.
    pub line: u64,
    /// Where the line starts in the decoded text.
    pub byte_offset: u64,
    pub excerpt: String,
    /// When the object's bytes were fetched (RFC 3339, UTC).
    pub fetched_at: String,
}

/// One next action: a `kurama data` request, never started on its own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct S3NextAction {
    /// Always `data`: the subcommand that takes the request.
    pub kind: &'static str,
    /// Always `describe`: the columns, which read the least of the object.
    pub operation: &'static str,
    /// The arguments of the `kurama data --request` document.
    pub args: DataHandoffArgs,
    /// What `kurama s3` saw of the object. `kurama data` reads it again and
    /// does not assume it is the same version.
    pub provenance: S3Provenance,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DataHandoffArgs {
    /// `s3://bucket/key`, the key exactly as listed.
    pub from: String,
    /// The `[s3.*]` whose AWS profile and region sign the read.
    pub s3_source: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct S3Provenance {
    pub etag: Option<String>,
    pub size: u64,
    pub last_modified: Option<String>,
}
