//! Resolve local patterns to bounded input files, with size and modification evidence.
use crate::domain::types::dataset::{
    DataError, DataLimits, DataRequest, DataSource, InputFile, InvalidInput,
};
use std::{collections::BTreeSet, fs, path::Path};

pub fn local_file(source: &str, path: &Path) -> Result<InputFile, DataError> {
    let path = path.canonicalize().map_err(|error| DataError::InputIo {
        path: path.to_owned(),
        source: error,
    })?;
    let metadata = fs::metadata(&path).map_err(|error| DataError::InputIo {
        path: path.clone(),
        source: error,
    })?;
    if !metadata.is_file() {
        return Err(InvalidInput::InputNotRegularFile {
            path: path.to_string_lossy().into_owned(),
        }
        .into());
    }
    let uri = path.to_string_lossy().into_owned();
    validate_resolved_path(&uri)?;
    Ok(InputFile {
        source: source.into(),
        uri,
        size: metadata.len(),
        etag: None,
        version_id: None,
        modified: metadata
            .modified()
            .ok()
            .map(|time| chrono::DateTime::<chrono::Utc>::from(time).to_rfc3339()),
    })
}

/// `examined` counts the candidates of the whole request, so the limit stops
/// resolution before the first candidate past it is opened.
pub fn resolve_local(
    source: &DataSource,
    limits: &DataLimits,
    examined: &mut usize,
) -> Result<Vec<InputFile>, DataError> {
    let mut files = Vec::new();
    let paths = glob::glob(&source.path).map_err(|_| InvalidInput::InvalidGlob {
        source_name: source.name.clone(),
        pattern: source.path.clone(),
    })?;
    for path in paths {
        if *examined == limits.max_source_objects {
            return Err(InvalidInput::SourceObjectLimit.into());
        }
        *examined += 1;
        let path = path.map_err(|error| DataError::InputIo {
            path: error.path().to_owned(),
            source: error.into(),
        })?;
        files.push(local_file(&source.name, &path)?);
    }
    if files.is_empty() {
        return Err(InvalidInput::NoMatchingInputs {
            source_name: source.name.clone(),
            pattern: source.path.clone(),
        }
        .into());
    }
    Ok(files)
}

/// DuckDB expands glob characters even after kurama has resolved an exact file.
pub fn validate_resolved_path(path: &str) -> Result<(), DataError> {
    if path.contains(['*', '?', '[']) {
        return Err(InvalidInput::InputContainsGlob { path: path.into() }.into());
    }
    Ok(())
}

/// `max_source_objects` bounds enumeration for every operation.
/// `max_source_bytes` stands in for transfer, so it bounds only the objects
/// this request reads end to end. `DataRequest::spends_byte_budget` is the one
/// statement of that rule; this function only sums what it selects.
pub fn validate_inputs(
    files: &[InputFile],
    sources: &[DataSource],
    limits: &DataLimits,
    request: &DataRequest,
) -> Result<(), DataError> {
    if files.len() > limits.max_source_objects {
        return Err(InvalidInput::SourceObjectLimit.into());
    }
    let spending = sources
        .iter()
        .filter(|source| request.spends_byte_budget(source.format.expect("resolved input format")))
        .map(|source| source.name.as_str())
        .collect::<BTreeSet<_>>();
    let bytes = files
        .iter()
        .filter(|file| spending.contains(file.source.as_str()))
        .try_fold(0_u64, |size, file| size.checked_add(file.size));
    if bytes.is_none_or(|bytes| bytes > limits.max_source_bytes) {
        return Err(InvalidInput::SourceByteLimit.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(path: String) -> DataSource {
        serde_json::from_value(serde_json::json!({"name": "orders", "path": path})).unwrap()
    }

    fn typed_source(name: &str, format: &str) -> DataSource {
        serde_json::from_value(
            serde_json::json!({"name": name, "path": format!("x.{format}"), "format": format}),
        )
        .unwrap()
    }

    fn input(source: &str, size: u64) -> InputFile {
        InputFile {
            source: source.into(),
            uri: format!("/tmp/{source}"),
            size,
            etag: None,
            version_id: None,
            modified: None,
        }
    }

    fn budget(bytes: u64) -> DataLimits {
        DataLimits {
            max_source_bytes: bytes,
            ..DataLimits::default()
        }
    }

    fn request(json: serde_json::Value) -> DataRequest {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn data_metadata_operations_ignore_object_size() {
        let sources = vec![
            typed_source("orders", "parquet"),
            typed_source("rows", "csv"),
        ];
        let files = vec![input("orders", 20), input("rows", 20)];
        for operation in ["tables", "describe"] {
            let request = request(serde_json::json!({"operation": operation, "args": {}}));
            assert!(validate_inputs(&files, &sources, &budget(10), &request).is_ok());
        }
        let explain = request(
            serde_json::json!({"operation": "query", "args": {"sql": "SELECT 1", "explain": true}}),
        );
        assert!(validate_inputs(&files, &sources, &budget(10), &explain).is_ok());
    }

    #[test]
    fn data_a_query_spends_the_budget_on_row_formats_only() {
        let sources = vec![
            typed_source("orders", "parquet"),
            typed_source("rows", "csv"),
        ];
        let query = request(
            serde_json::json!({"operation": "query", "args": {"sql": "SELECT count(*) FROM orders"}}),
        );
        // Arbitrary SQL may read statistics only, so a columnar object is not
        // refused for its size: that is what `count(*)` on a large input needs.
        assert!(validate_inputs(&[input("orders", 20)], &sources, &budget(10), &query).is_ok());
        let error =
            validate_inputs(&[input("rows", 20)], &sources, &budget(10), &query).unwrap_err();
        assert!(error.to_string().contains("max_source_bytes"));
    }

    #[test]
    fn data_reading_every_column_spends_the_budget_on_parquet_too() {
        let sources = vec![typed_source("orders", "parquet")];
        let files = vec![input("orders", 20)];
        // SUMMARIZE computes statistics for every column of every row, and an
        // unprojected preview selects every column: the object is read whole.
        for json in [
            serde_json::json!({"operation": "summary", "args": {"table": "orders"}}),
            serde_json::json!({"operation": "preview", "args": {"table": "orders"}}),
        ] {
            let error = validate_inputs(&files, &sources, &budget(10), &request(json))
                .expect_err("an operation that reads every column spends the budget");
            assert!(error.to_string().contains("max_source_bytes"));
        }
        // A projected preview reads the named column ranges only.
        let projected = request(
            serde_json::json!({"operation": "preview", "args": {"table": "orders", "columns": ["amount"]}}),
        );
        assert!(validate_inputs(&files, &sources, &budget(10), &projected).is_ok());
    }

    #[test]
    fn data_object_count_stops_every_operation() {
        let sources = vec![typed_source("orders", "parquet")];
        let files = vec![input("orders", 1), input("orders", 1)];
        let limits = DataLimits {
            max_source_objects: 1,
            ..DataLimits::default()
        };
        for json in [
            serde_json::json!({"operation": "describe", "args": {"table": "orders"}}),
            serde_json::json!({"operation": "query", "args": {"sql": "SELECT 1"}}),
        ] {
            let error = validate_inputs(&files, &sources, &limits, &request(json)).unwrap_err();
            assert!(error.to_string().contains("max_source_objects"));
        }
    }

    #[test]
    fn data_local_input_errors_preserve_the_path() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing.csv");
        let error = local_file("orders", &missing).unwrap_err();
        assert!(error.to_string().contains(missing.to_str().unwrap()));
        assert!(
            std::error::Error::source(&error)
                .and_then(|error| error.downcast_ref::<std::io::Error>())
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        );
    }

    #[test]
    fn data_local_glob_errors_name_the_source_and_pattern() {
        let directory = tempfile::tempdir().unwrap();
        let pattern = directory
            .path()
            .join("missing-*.csv")
            .to_string_lossy()
            .into_owned();
        let error =
            resolve_local(&source(pattern.clone()), &DataLimits::default(), &mut 0).unwrap_err();
        assert!(error.to_string().contains("orders"));
        assert!(error.to_string().contains(&pattern));
    }

    #[test]
    fn data_local_object_limit_is_shared_by_every_source_of_the_request() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("a.csv");
        fs::write(&file, "amount\n1\n").unwrap();
        let limits = DataLimits {
            max_source_objects: 1,
            ..DataLimits::default()
        };
        let mut examined = 0;
        let found = resolve_local(
            &source(file.to_string_lossy().into_owned()),
            &limits,
            &mut examined,
        )
        .unwrap();
        assert_eq!((found.len(), examined), (1, 1));
        // A directory would fail as "not a regular file" if it were resolved.
        let error = resolve_local(
            &source(directory.path().to_string_lossy().into_owned()),
            &limits,
            &mut examined,
        )
        .unwrap_err();
        assert!(error.to_string().contains("max_source_objects"), "{error}");
        assert_eq!(examined, 1);
    }

    #[test]
    fn data_local_resolved_glob_characters_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        for name in ["a[1].csv", "a?.csv", "a*.csv"] {
            let path = directory.path().join(name);
            fs::write(&path, "amount\n1\n").unwrap();
            let error = local_file("orders", &path).unwrap_err();
            assert!(error.to_string().contains(name));
        }
    }
}
