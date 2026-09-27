//! One ephemeral DuckDB session: restricted file views, in-memory S3 secrets and atomic local exports.
pub mod connection;
mod handles;
mod parquet_summary;
mod result;
mod sql;
mod transfer;

use crate::domain::types::{
    Credentials,
    dataset::{
        DataError, DataFormat, DataLimits, DataPayload, DataRequest, DataSource, EngineInfo,
        ExportInfo, InputFile, InvalidInput, ScanContext, sql_string,
    },
};
use connection::{Cancellation, Connection};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Instant,
};
pub use transfer::Transfer;
use zeroize::Zeroizing;

pub struct S3Access {
    pub credentials: Credentials,
    /// Every bucket read, and the region it is read in.
    pub regions: BTreeMap<String, String>,
    #[cfg(feature = "test-fakes")]
    pub endpoint: Option<String>,
}

pub struct Analysis {
    pub sources: Vec<DataSource>,
    pub files: Vec<InputFile>,
    pub limits: DataLimits,
    pub request: DataRequest,
    pub s3: Option<S3Access>,
}

pub struct CompletedAnalysis {
    pub result: DataPayload,
    pub export: Option<(tempfile::NamedTempFile, PathBuf)>,
    /// What httpfs moved, when the engine's HTTP log could be read.
    pub transfer: Option<Transfer>,
}

/// The columns a request names, published as soon as the statement is parsed.
///
/// The worker runs on its own thread and its result is discarded when a wait
/// is cancelled, so this is the only way a timeout can still say which columns
/// the query was reading.
#[derive(Default)]
pub struct ScannedColumns(Mutex<Vec<String>>);

impl ScannedColumns {
    fn publish(&self, columns: Vec<String>) {
        *self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = columns;
    }

    /// What the run was doing, for an error that ends a wait.
    pub fn context(&self, start: Instant) -> ScanContext {
        ScanContext {
            elapsed_ms: start.elapsed().as_millis() as u64,
            columns: self
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
        }
    }
}

pub fn execute(
    analysis: Analysis,
    cancel: Arc<Cancellation>,
    scanned: &ScannedColumns,
) -> Result<CompletedAnalysis, DataError> {
    let session = tempfile::Builder::new().prefix("kurama-data-").tempdir()?;
    let destination = analysis.request.args().export.as_deref().map(Path::new);
    let temporary = prepare_export(destination)?;
    let connection = Connection::open(
        &engine_settings(&analysis.limits, session.path()),
        cancel.clone(),
    )?;
    connection.execute("SET enable_progress_bar=false", "output settings")?;
    // The engine's own HTTP log is the only place the range reads it issues
    // can be counted; nothing else is logged, and a local run logs nothing.
    let measured = analysis.s3.is_some()
        && match transfer::start_logging(&connection) {
            Ok(()) => true,
            Err(error) => {
                tracing::debug!(%error, "HTTP accounting could not be enabled");
                false
            }
        };
    if !measured {
        connection.execute("SET enable_logging=false", "output settings")?;
    }
    if let Some(access) = &analysis.s3 {
        load_s3(
            &connection,
            session.path(),
            access,
            analysis.limits.query_timeout_secs,
        )?;
    }
    restrict_paths(&connection, &analysis.files, temporary.as_ref())?;
    for source in &analysis.sources {
        cancel.check()?;
        connection.execute(&sql::view_sql(source, &analysis.files), "source schema")?;
    }
    if let DataRequest::Query(args) = &analysis.request {
        let statement = connection.validate_select(
            args.sql.as_deref().expect("validated inline or file SQL"),
            &analysis
                .sources
                .iter()
                .map(|source| source.name.clone())
                .collect::<Vec<_>>(),
        )?;
        scanned.publish(sql::referenced_columns(&statement));
    } else if let DataRequest::Preview(args) = &analysis.request {
        scanned.publish(args.columns.clone());
    }
    let query = sql::query_sql(&analysis.request);
    let (result, export) = if let (Some(file), Some(destination)) = (&temporary, destination) {
        (
            None,
            Some(export_query(&connection, &query, file, destination)?),
        )
    } else {
        (
            Some(result::collect(&connection, &query, &analysis.limits)?),
            None,
        )
    };
    cancel.check()?;
    let parquet = describe_parquet(&connection, &analysis)?;
    cancel.check()?;
    verify_local_inputs(&analysis.files)?;
    let transfer = measured
        .then(|| transfer::read(&connection, &analysis.limits))
        .flatten();
    Ok(CompletedAnalysis {
        transfer,
        result: DataPayload {
            result,
            export,
            parquet,
            engine: EngineInfo {
                name: "duckdb".into(),
                version: Connection::version(),
            },
        },
        export: temporary.zip(destination.map(Path::to_path_buf)),
    })
}

/// `--describe` returns a logical schema. For Parquet that says nothing about
/// what the next query costs, so the footers of the described source are read
/// as well.
fn describe_parquet(
    connection: &Connection,
    analysis: &Analysis,
) -> Result<Option<crate::domain::types::dataset::ParquetSummary>, DataError> {
    let DataRequest::Describe(args) = &analysis.request else {
        return Ok(None);
    };
    let table = args.table.as_deref().expect("validated table");
    // DuckDB resolves an identifier without case; source names are unique the
    // same way, so this match is the one the view was created under.
    let described = analysis
        .sources
        .iter()
        .find(|source| source.name.eq_ignore_ascii_case(table));
    if described.is_none_or(|source| !matches!(source.format, Some(DataFormat::Parquet))) {
        return Ok(None);
    }
    let files = analysis
        .files
        .iter()
        .filter(|file| file.source.eq_ignore_ascii_case(table))
        .cloned()
        .collect::<Vec<_>>();
    if files.is_empty() {
        return Ok(None);
    }
    parquet_summary::read(connection, &files, &analysis.limits).map(Some)
}

fn prepare_export(
    destination: Option<&Path>,
) -> Result<Option<tempfile::NamedTempFile>, DataError> {
    let Some(path) = destination else {
        return Ok(None);
    };
    if path.exists() {
        return Err(InvalidInput::ExportExists.into());
    }
    Ok(Some(tempfile::NamedTempFile::new_in(
        path.parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )?))
}

fn engine_settings(limits: &DataLimits, directory: &Path) -> Vec<(&'static str, String)> {
    vec![
        ("memory_limit", limits.memory_limit.clone()),
        ("threads", limits.threads.to_string()),
        (
            "max_temp_directory_size",
            limits.max_temp_directory_size.clone(),
        ),
        (
            "temp_directory",
            if limits.allow_spill {
                directory.join("spill").to_string_lossy().into_owned()
            } else {
                String::new()
            },
        ),
        ("autoinstall_known_extensions", "false".into()),
        ("autoload_known_extensions", "false".into()),
        ("allow_persistent_secrets", "false".into()),
        ("allow_unredacted_secrets", "false".into()),
        (
            "secret_directory",
            directory.join("secrets").to_string_lossy().into_owned(),
        ),
    ]
}

fn restrict_paths(
    connection: &Connection,
    files: &[InputFile],
    output: Option<&tempfile::NamedTempFile>,
) -> Result<(), DataError> {
    let mut paths = files
        .iter()
        .map(|file| sql_string(&file.uri))
        .collect::<Vec<_>>();
    if let Some(file) = output {
        paths.push(sql_string(&file.path().canonicalize()?.to_string_lossy()));
    }
    connection.execute(
        &format!("SET allowed_paths = [{}]", paths.join(",")),
        "read restrictions",
    )?;
    connection.execute(
        "SET enable_external_access = false; SET lock_configuration = true",
        "configuration lock",
    )
}

fn export_query(
    connection: &Connection,
    query: &str,
    file: &tempfile::NamedTempFile,
    destination: &Path,
) -> Result<ExportInfo, DataError> {
    // DuckDB parses the validated SELECT as a view, including trailing comments.
    connection.execute(
        &format!("CREATE TEMP VIEW \"__kurama_export\" AS {query}"),
        "export query",
    )?;
    let format = if destination
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("parquet"))
    {
        "PARQUET"
    } else {
        "CSV, HEADER true"
    };
    // The owned file provides atomic publication; DuckDB needs no additional tmp_ path.
    connection.execute(
        &format!(
            "COPY \"__kurama_export\" TO {} (FORMAT {format}, USE_TMP_FILE false)",
            sql_string(&file.path().canonicalize()?.to_string_lossy())
        ),
        "export",
    )?;
    Ok(ExportInfo {
        path: destination.to_string_lossy().into_owned(),
        bytes: file.as_file().metadata()?.len(),
    })
}

fn verify_local_inputs(files: &[InputFile]) -> Result<(), DataError> {
    for input in files.iter().filter(|file| !file.uri.starts_with("s3://")) {
        let now = super::data_inputs::local_file(&input.source, Path::new(&input.uri))?;
        if now.size != input.size || now.modified != input.modified {
            return Err(DataError::Changed);
        }
    }
    Ok(())
}

fn append_literal(output: &mut String, value: &str) {
    output.push('\'');
    for character in value.chars() {
        output.push(character);
        if character == '\'' {
            output.push('\'');
        }
    }
    output.push('\'');
}

fn load_s3(
    connection: &Connection,
    directory: &Path,
    access: &S3Access,
    timeout: u64,
) -> Result<(), DataError> {
    let extension = directory.join("httpfs.duckdb_extension");
    let mut output = fs::File::create(&extension)?;
    let mut archive =
        flate2::read::GzDecoder::new(include_bytes!(env!("KURAMA_HTTPFS_ARCHIVE_PATH")).as_slice());
    std::io::copy(&mut archive, &mut output)?;
    output.flush()?;
    // DuckDB verifies the extension signature and ABI as well as our build hash.
    connection.execute(
        &format!("LOAD {}", sql_string(&extension.to_string_lossy())),
        "loading bundled httpfs",
    )?;
    connection.execute(
        &format!("SET http_retries=0; SET http_timeout={timeout}"),
        "HTTP limits",
    )?;
    for (i, (bucket, region)) in access.regions.iter().enumerate() {
        let mut sql = Zeroizing::new(format!(
            "CREATE TEMPORARY SECRET kurama_s3_{i} (TYPE s3, PROVIDER config"
        ));
        for (key, value) in [
            ("KEY_ID", access.credentials.access_key_id()),
            ("SECRET", access.credentials.secret_access_key()),
            (
                "SESSION_TOKEN",
                access.credentials.session_token().unwrap_or(""),
            ),
            ("REGION", region.as_str()),
            ("SCOPE", &format!("s3://{bucket}/")),
        ] {
            sql.push_str(", ");
            sql.push_str(key);
            sql.push(' ');
            append_literal(&mut sql, value);
        }
        sql.push_str(", URL_COMPATIBILITY_MODE true");
        #[cfg(feature = "test-fakes")]
        if let Some(endpoint) = &access.endpoint {
            sql.push_str(", ENDPOINT ");
            append_literal(&mut sql, endpoint);
            sql.push_str(", USE_SSL false, URL_STYLE 'path'");
        }
        sql.push(')');
        connection.execute(&sql, "temporary S3 credentials")?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "duckdb_tests.rs"]
mod tests;
