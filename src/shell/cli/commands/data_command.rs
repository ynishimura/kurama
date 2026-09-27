//! CLI data arguments and JSON requests, converted to the same typed operation.
use crate::domain::functions::completion_candidates::ProfileScope;
use crate::domain::types::dataset::{DataArgs, DataError, DataRequest, InvalidInput};
use crate::shell::cli::client::{self, RequestReadFailure};
use crate::shell::cli::completion;
use clap::{Arg, ArgAction, ArgGroup, ArgMatches, Command, ValueHint};
use clap_complete::engine::ArgValueCandidates;

const STRINGS: &[&str] = &[
    "from",
    "aws-profile",
    "s3-source",
    "region",
    "file",
    "export",
    "delimiter",
    "date-format",
    "timestamp-format",
    "memory-limit",
    "max-temp-directory-size",
];
const NUMBERS: &[&str] = &[
    "threads",
    "timeout",
    "max-rows",
    "max-result-bytes",
    "max-source-objects",
    "max-source-bytes",
];
const OPERATIONS: &[&str] = &["tables", "describe", "preview", "summary", "query", "file"];

pub fn command() -> Command {
    let command = Command::new("data")
        .about("Analyze local/S3 CSV, JSONL and Parquet using embedded DuckDB (CLI)")
        .arg(
            Arg::new("workspace")
                .value_name("WORKSPACE_OR_INPUT")
                .value_hint(ValueHint::AnyPath)
                .help("Workspace name, local path/glob, s3:// URI or S3 HTTPS URL")
                .conflicts_with("from"),
        );
    let mut command = client::envelope_args(
        command,
        "Read a typed JSON request; '-' reads stdin without putting the input URL in argv",
        "Show static inputs and limits without opening files or acquiring credentials",
    )
        .arg(Arg::new("tables").long("tables").action(ArgAction::SetTrue).help("List the registered source tables"))
        .arg(
            Arg::new("explain")
                .long("explain")
                .action(ArgAction::SetTrue)
                .help("Show the plan for --query/--file instead of executing it")
                .conflicts_with("export"),
        )
        .group(ArgGroup::new("operation").args(OPERATIONS))
        .after_help("Examples:\n  kurama data ./events.jsonl\n  kurama data ./orders.parquet --describe\n  kurama data 's3://bucket/key.parquet' --aws-profile dev\n  kurama data ./events.jsonl --query 'SELECT count(*) FROM data' --jq '.rows'\n  kurama data lake --preview orders\n\nAd-hoc inputs default to a preview of at most 100 rows. Workspace names need an explicit operation.\nTABLE may be omitted when there is exactly one source. --dry-run never reads inputs.\nS3 HTTPS URLs are converted to s3://; URL credentials are discarded. An AWS profile or S3 source is still required.\nPasting a signed URL into argv can expose it in shell history; use --request - to pass it through stdin.");
    for (name, value, help) in [
        (
            "from",
            "INPUT",
            "Ad-hoc local path/glob, s3:// URI or S3 HTTPS URL; SQL table name is data",
        ),
        (
            "aws-profile",
            "PROFILE",
            "AWS profile for S3; never inferred from the bucket name or environment",
        ),
        ("s3-source", "NAME", "Use aws_profile/region from [s3.NAME]"),
        (
            "region",
            "REGION",
            "Override the configured region; otherwise use the AWS profile region",
        ),
        ("file", "SQL_FILE", "Read one SELECT from a SQL file"),
        (
            "query",
            "SQL",
            "Run one read-only SELECT over the source tables",
        ),
        (
            "export",
            "LOCAL_FILE",
            "Export the full query to a new .csv/.parquet file without overwriting",
        ),
        ("delimiter", "CHAR", "CSV field delimiter"),
        ("date-format", "FORMAT", "CSV date format, e.g. %Y-%m-%d"),
        ("timestamp-format", "FORMAT", "CSV timestamp format"),
        (
            "memory-limit",
            "SIZE",
            "DuckDB memory limit (default 1GB; not a process RSS limit)",
        ),
        (
            "max-temp-directory-size",
            "SIZE",
            "Maximum DuckDB spill size (default 10GB)",
        ),
    ] {
        // `Other` is how an argument says its value cannot be completed;
        // leaving `Unknown` would silently stop completing instead.
        let hint = match name {
            "from" => ValueHint::AnyPath,
            "file" | "export" => ValueHint::FilePath,
            _ => ValueHint::Other,
        };
        let mut arg = Arg::new(name)
            .long(name)
            .value_name(value)
            .value_hint(hint)
            .help(help);
        if name == "aws-profile" {
            arg = arg
                .value_hint(ValueHint::Unknown)
                .add(ArgValueCandidates::new(|| {
                    completion::profiles(ProfileScope::Aws)
                }));
        }
        command = command.arg(arg);
    }
    for (name, help) in [
        (
            "describe",
            "Show column names and types; TABLE is optional for a single source",
        ),
        (
            "preview",
            "Show the first rows (default at most 100); TABLE is optional for a single source",
        ),
        (
            "summary",
            "Scan all input rows for column statistics; TABLE is optional for a single source",
        ),
    ] {
        command = command.arg(
            Arg::new(name)
                .long(name)
                .value_name("TABLE")
                .num_args(0..=1)
                .default_missing_value("")
                // The table names come from the sources of this very run.
                .value_hint(ValueHint::Other)
                .help(help),
        );
    }
    for (name, help) in [
        ("threads", "DuckDB worker threads (default 2)"),
        (
            "timeout",
            "Input/query deadline in seconds after authentication (default 60)",
        ),
        (
            "max-rows",
            "Maximum returned rows (default 1000; preview defaults to at most 100)",
        ),
        (
            "max-result-bytes",
            "Maximum serialized result rows in bytes (minimum 2; default 8388608)",
        ),
        (
            "max-source-objects",
            "Maximum input candidates to examine (default 1000)",
        ),
        (
            "max-source-bytes",
            "Maximum size of the objects an operation reads in full (default 10737418240; not transfer bytes, and not applied to metadata operations or Parquet)",
        ),
    ] {
        command = command.arg(
            Arg::new(name)
                .long(name)
                .value_name("N")
                .help(help)
                .value_hint(ValueHint::Other)
                .value_parser(clap::value_parser!(u64).range(1..)),
        );
    }
    for (name, help) in [
        ("header", "Whether the CSV input has a header row"),
        (
            "allow-spill",
            "Allow DuckDB to use temporary disk space (default true)",
        ),
    ] {
        command = command.arg(
            Arg::new(name)
                .long(name)
                .help(help)
                .value_parser(clap::value_parser!(bool)),
        );
    }
    for (name, value, help) in [
        (
            "type",
            "COLUMN=TYPE",
            "Override a CSV column type; repeat for multiple columns",
        ),
        (
            "columns",
            "COLUMN",
            "Select a preview column; repeat for multiple columns",
        ),
    ] {
        command = command.arg(
            Arg::new(name)
                .long(name)
                .value_name(value)
                // Column names come from the input files of this very run.
                .value_hint(ValueHint::Other)
                .help(help)
                .action(ArgAction::Append),
        );
    }
    let conflicts: Vec<_> = STRINGS
        .iter()
        .chain(NUMBERS)
        .copied()
        .chain([
            "tables",
            "describe",
            "preview",
            "summary",
            "query",
            "explain",
            "header",
            "allow-spill",
            "type",
            "columns",
        ])
        .collect();
    command
        .mut_arg("max-result-bytes", |arg| {
            arg.value_parser(clap::value_parser!(u64).range(2..))
        })
        .mut_arg("threads", |arg| {
            arg.value_parser(clap::value_parser!(u64).range(1..=u32::MAX as u64))
        })
        .mut_arg("request", |arg| arg.conflicts_with_all(conflicts))
        .mut_arg("aws-profile", |arg| arg.conflicts_with("s3-source"))
}

#[derive(Clone)]
pub struct DataCommand {
    pub workspace: Option<String>,
    pub json: bool,
    pub jq: Option<String>,
    pub dry_run: bool,
    request_file: Option<String>,
    request: Option<DataRequest>,
    type_overrides: Vec<String>,
}
impl std::fmt::Debug for DataCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DataCommand { input: [REDACTED] }")
    }
}
impl DataCommand {
    pub fn parse(matches: &ArgMatches) -> Self {
        let string = |name: &str| matches.get_one::<String>(name).cloned();
        let number = |name: &str| matches.get_one::<u64>(name).copied();
        let mut args = DataArgs {
            from: string("from"),
            aws_profile: string("aws-profile"),
            s3_source: string("s3-source"),
            region: string("region"),
            file: string("file"),
            export: string("export"),
            delimiter: string("delimiter"),
            date_format: string("date-format"),
            timestamp_format: string("timestamp-format"),
            memory_limit: string("memory-limit"),
            max_temp_directory_size: string("max-temp-directory-size"),
            threads: number("threads").map(|v| v as u32),
            query_timeout_secs: number("timeout"),
            max_rows: number("max-rows").map(|v| v as usize),
            max_result_bytes: number("max-result-bytes").map(|v| v as usize),
            max_source_objects: number("max-source-objects").map(|v| v as usize),
            max_source_bytes: number("max-source-bytes"),
            header: matches.get_one::<bool>("header").copied(),
            allow_spill: matches.get_one::<bool>("allow-spill").copied(),
            ..DataArgs::default()
        };
        let mut workspace = matches.get_one::<String>("workspace").cloned();
        if workspace.as_deref().is_some_and(is_input) {
            args.from = workspace.take();
        }
        args.explain = matches.get_flag("explain");
        args.columns = matches
            .get_many::<String>("columns")
            .map(|v| v.cloned().collect())
            .unwrap_or_default();
        args.sql = matches.get_one::<String>("query").cloned();
        let request = if matches.get_flag("tables") {
            Some(DataRequest::Tables(args))
        } else if let Some(table) = matches.get_one::<String>("describe") {
            args.table = (!table.is_empty()).then(|| table.clone());
            Some(DataRequest::Describe(args))
        } else if let Some(table) = matches.get_one::<String>("preview") {
            args.table = (!table.is_empty()).then(|| table.clone());
            Some(DataRequest::Preview(args))
        } else if let Some(table) = matches.get_one::<String>("summary") {
            args.table = (!table.is_empty()).then(|| table.clone());
            Some(DataRequest::Summary(args))
        } else if args.sql.is_some() || args.file.is_some() {
            Some(DataRequest::Query(args))
        } else if matches.get_flag("dry-run") {
            Some(DataRequest::Tables(args))
        } else if args.from.is_some() {
            Some(DataRequest::Preview(args))
        } else {
            None
        };
        Self {
            workspace,
            json: matches.get_flag("json") || matches.contains_id("jq"),
            jq: matches.get_one::<String>("jq").cloned(),
            dry_run: matches.get_flag("dry-run"),
            request_file: matches.get_one::<String>("request").cloned(),
            request,
            type_overrides: matches
                .get_many::<String>("type")
                .map(|v| v.cloned().collect())
                .unwrap_or_default(),
        }
    }

    /// The request, and the region a regional S3 HTTPS input named.
    pub fn read_request(&self) -> Result<(DataRequest, Option<String>), DataError> {
        let mut request = if let Some(path) = &self.request_file {
            if self
                .request
                .as_ref()
                .is_some_and(|r| r.args().from.is_some())
            {
                return Err(DataError::Invalid(InvalidInput::PositionalRequestConflict));
            }
            let text = client::read_request_text(path).map_err(|failure| match failure {
                RequestReadFailure::Stdin(source) => DataError::from(source),
                RequestReadFailure::File(source) => request_file_error(path, source),
            })?;
            serde_json::from_str(&text)
                .map_err(|_| DataError::Invalid(InvalidInput::InvalidRequest))?
        } else {
            self.request
                .clone()
                .ok_or(DataError::Invalid(InvalidInput::OperationRequired))?
        };
        let args = match &mut request {
            DataRequest::Tables(a)
            | DataRequest::Describe(a)
            | DataRequest::Preview(a)
            | DataRequest::Summary(a)
            | DataRequest::Query(a) => a,
        };
        let mut url_region = None;
        if let Some(input) = &args.from {
            let (path, region) = normalize_input(input)?;
            args.from = Some(path);
            url_region = region;
        }
        for spec in &self.type_overrides {
            let (name, ty) = spec
                .split_once('=')
                .filter(|(k, v)| !k.is_empty() && !v.is_empty())
                .ok_or(DataError::Invalid(InvalidInput::InvalidTypeOverride))?;
            if args.types.insert(name.into(), ty.into()).is_some() {
                return Err(DataError::Invalid(InvalidInput::DuplicateTypeOverride));
            }
        }
        request.validate()?;
        // SQL files are request input; resolve their errors before starting the
        // engine. A static plan does not read them.
        if !self.dry_run
            && let DataRequest::Query(args) = &mut request
            && let Some(path) = args.file.take()
        {
            args.sql = Some(read_request_file(&path)?);
        }
        Ok((request, url_region))
    }
}

fn read_request_file(path: &str) -> Result<String, DataError> {
    std::fs::read_to_string(path).map_err(|source| request_file_error(path, source))
}

/// A request or SQL file the run was told to read and could not.
fn request_file_error(path: &str, source: std::io::Error) -> DataError {
    if client::is_unopenable(&source) {
        InvalidInput::RequestFile {
            path: path.into(),
            source,
        }
        .into()
    } else {
        DataError::InputIo {
            path: path.into(),
            source,
        }
    }
}

fn is_input(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.contains('/')
        || [
            ".csv",
            ".csv.gz",
            ".parquet",
            ".jsonl",
            ".jsonl.gz",
            ".ndjson",
            ".ndjson.gz",
        ]
        .iter()
        .any(|ext| value.ends_with(ext))
}

/// Interpret an S3 HTTPS URL as a key and, for a regional endpoint, as the
/// region the bucket lives in; never as a source of credentials.
fn normalize_input(input: &str) -> Result<(String, Option<String>), DataError> {
    if !input.starts_with("https://") {
        return Ok((input.into(), None));
    }
    let invalid = || DataError::Invalid(InvalidInput::UnsupportedS3Url);
    let url = url::Url::parse(input).map_err(|_| invalid())?;
    if !url.username().is_empty() || url.password().is_some() || url.port().is_some() {
        return Err(invalid());
    }
    if url
        .query_pairs()
        .any(|(name, _)| matches!(name.as_ref(), "versionId" | "partNumber"))
    {
        return Err(DataError::Invalid(InvalidInput::UnsupportedS3UrlVersion));
    }
    let host = url.host_str().ok_or_else(invalid)?;
    let host = host
        .strip_suffix(".amazonaws.com")
        .or_else(|| host.strip_suffix(".amazonaws.com.cn"))
        .ok_or_else(invalid)?;
    // Use the original path: URL parsers normalize dot segments, which are valid S3 keys.
    let path = input
        .split(['?', '#'])
        .next()
        .unwrap()
        .strip_prefix("https://")
        .and_then(|s| s.split_once('/'))
        .map(|(_, path)| path)
        .ok_or_else(invalid)?;
    // `s3.<region>` and `<bucket>.s3.<region>`; the global `s3` names no region.
    let (bucket, key, region) = if is_s3_endpoint(host) {
        let (bucket, key) = path.split_once('/').ok_or_else(invalid)?;
        (bucket, key, host.strip_prefix("s3."))
    } else {
        let (bucket, suffix) = host.rsplit_once(".s3").ok_or_else(invalid)?;
        if !is_s3_endpoint(&format!("s3{suffix}")) {
            return Err(invalid());
        }
        (bucket, path, suffix.strip_prefix('.'))
    };
    let key = percent_encoding::percent_decode_str(key)
        .decode_utf8()
        .map_err(|_| invalid())?;
    if key.contains(['*', '?', '[', '\0']) {
        return Err(DataError::Invalid(InvalidInput::S3UrlGlob));
    }
    let uri = format!("s3://{bucket}/{key}");
    crate::adapters::aws::s3_data::split_uri(&uri)?;
    Ok((uri, region.map(str::to_owned)))
}

fn is_s3_endpoint(host: &str) -> bool {
    host == "s3"
        || host.strip_prefix("s3.").is_some_and(|region| {
            region
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                && region.rsplit_once('-').is_some_and(|(prefix, number)| {
                    prefix.contains('-')
                        && !number.is_empty()
                        && number.bytes().all(|b| b.is_ascii_digit())
                })
        })
}

#[cfg(test)]
#[path = "data_command_tests.rs"]
mod tests;
