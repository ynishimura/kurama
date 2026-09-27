//! Typed analysis failures and the concrete input corrections they require.
use std::path::PathBuf;

/// What a wait was doing when it ended. A timeout otherwise says nothing about
/// which columns the query was reading, which is the difference between a scan
/// that touched an identifier and one that touched a gigabyte of text.
#[derive(Debug, Default)]
pub struct ScanContext {
    /// Wall clock for the whole command, credential acquisition included.
    /// The query deadline starts later, after MFA and AssumeRole, so this is
    /// not the number to compare with `--timeout`.
    pub elapsed_ms: u64,
    /// The columns the request names, as far as the parsed statement shows.
    /// Empty when it names none of them: `SELECT *`, a full preview, a
    /// summary, or a query that never reached validation.
    pub columns: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum DataError {
    #[error(transparent)]
    Invalid(#[from] InvalidInput),
    #[error("SQL must be one read-only SELECT; check syntax and column names with --describe")]
    Sql,
    #[error("data operation failed during {0}; check input format, memory and disk limits")]
    Engine(&'static str),
    #[error("data operation timed out; narrow the input or increase --timeout")]
    Timeout(ScanContext),
    #[error("data operation was interrupted")]
    Interrupted(ScanContext),
    #[error(
        "result display limit reached; narrow the query, raise the display limits, or query with --export for all rows"
    )]
    Incomplete,
    #[error("S3 rejected the data operation ({0})")]
    S3Rejected(String),
    /// S3 answered 301 with the bucket's own region to a request signed for
    /// the region the request named, which kurama does not correct.
    #[error(
        "S3 answers for this bucket in {bucket_region}, not in {region}, the region the request named"
    )]
    S3OtherRegion {
        region: String,
        bucket_region: String,
    },
    /// S3 answered 301 to a request signed for `region` without naming the
    /// bucket's own region: a region or endpoint mismatch, not a permission.
    #[error(
        "S3 redirected a request signed for {region} (HTTP 301): the bucket is not in {region}, and S3 did not say where it is"
    )]
    S3Redirected { region: String },
    #[error("S3 data request failed")]
    S3Failed,
    #[error("an input changed during the operation; the result was discarded")]
    Changed,
    #[error("data file operation failed")]
    Io(#[from] std::io::Error),
    #[error("data input file operation failed for {}", path.display())]
    InputIo {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum InvalidInput {
    #[error(
        "source must be CSV, JSONL/NDJSON (optionally gzip) or Parquet; use a supported extension or configure the source format in a workspace"
    )]
    UnsupportedFormat,
    #[error("resource limits must be positive, with max_result_bytes at least 2")]
    PositiveLimits,
    #[error("memory and spill sizes need a positive integer and unit, for example 1GB")]
    InvalidMemorySize,
    #[error("query needs exactly one of sql or file")]
    QuerySqlRequired,
    #[error("sql, file, explain and export are query options")]
    QueryOptionsOnly,
    #[error("explain and export are exclusive")]
    ExplainExportConflict,
    #[error("columns is a preview option")]
    PreviewColumnsOnly,
    #[error("table is only used by describe, preview and summary")]
    UnexpectedTable,
    #[error("aws_profile and s3_source are exclusive")]
    CredentialSourceConflict,
    #[error("multiple sources; specify a table name from --tables")]
    TableRequired,
    #[error("choose a workspace or --from, exclusively")]
    WorkspaceOrInput,
    #[error("unknown data workspace; use status --kind data")]
    UnknownWorkspace,
    #[error("workspace CSV options belong in its source configuration")]
    WorkspaceCsvOptions,
    #[error("unknown s3_source")]
    UnknownS3Source,
    #[error("unknown source table; use --tables")]
    UnknownTable,
    #[error("export needs a local .csv or .parquet path")]
    InvalidExportPath,
    #[error("S3 input requires aws_profile or s3_source")]
    S3CredentialsRequired,
    #[error("S3 input requires a region")]
    S3RegionRequired,
    #[error("--jq must evaluate to exactly one JSON value; use an array for multiple results")]
    InvalidProjection,
    #[error("positional inputs cannot be mixed with --request; put from in the request")]
    PositionalRequestConflict,
    #[error("invalid data request; see agent --kind data --json for the schema")]
    InvalidRequest,
    #[error(
        "choose --tables, --describe, --preview, --summary or --query; data TUI is not implemented"
    )]
    OperationRequired,
    #[error("--type needs column=TYPE")]
    InvalidTypeOverride,
    #[error("duplicate --type column")]
    DuplicateTypeOverride,
    #[error("unsupported S3 HTTPS object URL; use s3://bucket/key with an AWS profile")]
    UnsupportedS3Url,
    #[error("versionId and partNumber in S3 URLs are not supported")]
    UnsupportedS3UrlVersion,
    #[error("S3 HTTPS URLs must name a single key without glob characters")]
    S3UrlGlob,
    #[error("cannot read data request or SQL file {}", path.display())]
    RequestFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("export destination already exists")]
    ExportExists,
    #[error("input contains a NUL byte")]
    NulByte,
    #[error("invalid DuckDB setting: {name}")]
    EngineSetting { name: String },
    #[error("S3 input must be s3://bucket/key")]
    InvalidS3Uri,
    #[error("S3 input needs a nonempty bucket and key")]
    InvalidS3BucketOrKey,
    #[error("invalid input glob for {source_name}: {pattern}")]
    InvalidGlob {
        source_name: String,
        pattern: String,
    },
    #[error("input is not a regular file: {path}")]
    InputNotRegularFile { path: String },
    #[error("input filename contains glob characters unsupported by DuckDB: {path}")]
    InputContainsGlob { path: String },
    #[error("input pattern matched no files for {source_name}: {pattern}")]
    NoMatchingInputs {
        source_name: String,
        pattern: String,
    },
    #[error("max_source_objects exceeded; narrow the input pattern")]
    SourceObjectLimit,
    #[error(
        "max_source_bytes exceeded by the inputs this operation reads in full; narrow the input pattern"
    )]
    SourceByteLimit,
    #[error("data workspace needs at least one source")]
    WorkspaceNeedsSource,
    #[error("root_dir must be an absolute path")]
    RootDirMustBeAbsolute,
    #[error("source names must be nonempty and unique ignoring case")]
    InvalidSourceNames,
    #[error("source path must be nonempty and contain no NUL byte")]
    InvalidSourcePath,
    #[error("relative source paths need an absolute root_dir")]
    RelativeSourceNeedsRoot,
    #[error("only local and S3 inputs are supported")]
    UnsupportedSourceProtocol,
    #[error("aws_profile must be nonempty")]
    EmptyAwsProfile,
    #[error("region must be nonempty")]
    EmptyRegion,
    #[error("bucket must be a bucket name: letters, digits, - and .")]
    S3BucketName,
    #[error("prefix needs a bucket")]
    S3PrefixNeedsBucket,
    #[error("page_size must be between 1 and 1000")]
    S3PageSize,
    #[error("request_timeout_secs must be at least 1")]
    S3RequestTimeout,
}

impl DataError {
    /// What the run was doing when it ended, for the errors that end a wait.
    pub fn scan_context(&self) -> Option<&ScanContext> {
        match self {
            Self::Timeout(context) | Self::Interrupted(context) => Some(context),
            _ => None,
        }
    }
}

impl InvalidInput {
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            Self::CredentialSourceConflict => {
                Some("pass only one of --aws-profile and --s3-source")
            }
            Self::SourceObjectLimit | Self::NoMatchingInputs { .. } => Some(
                "check the input path or narrow the glob; raise the source limit only when the full input is intended",
            ),
            Self::SourceByteLimit => Some(
                "object size bounds only row formats read end to end; run --tables, --describe or --explain to read metadata instead, narrow the glob, or raise --max-source-bytes when the full input is intended",
            ),
            Self::UnknownWorkspace => {
                Some("run `kurama status --kind data` to list configured workspaces")
            }
            Self::UnknownTable | Self::TableRequired => {
                Some("run the same data input with --tables, then choose a listed table")
            }
            Self::ExportExists => {
                Some("choose a new local export filename; existing files are never overwritten")
            }
            Self::RequestFile { .. } => {
                Some("check the --request or --file path and its read permissions")
            }
            Self::S3CredentialsRequired | Self::UnknownS3Source => {
                Some("pass --aws-profile PROFILE or configure and select --s3-source NAME")
            }
            Self::S3RegionRequired | Self::EmptyRegion => {
                Some("pass --region REGION or configure the AWS profile region")
            }
            Self::InvalidRequest => {
                Some("run `kurama agent --kind data --json` for the request schema")
            }
            Self::PositiveLimits | Self::InvalidMemorySize | Self::EngineSetting { .. } => {
                Some("check the effective resource limits with --dry-run")
            }
            _ => None,
        }
    }
}
