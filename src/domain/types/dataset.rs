//! File-analysis inputs, bounded results and errors; no engine or I/O operations.
pub use super::dataset_error::{DataError, InvalidInput, ScanContext};
pub use super::dataset_output::{
    DataKind, DataMeta, DataOutput, DataPayload, EngineInfo, ExportInfo, NextAction, ParquetColumn,
    ParquetSummary, RegionSource,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DataSource {
    pub name: String,
    pub path: String,
    pub format: Option<DataFormat>,
    #[serde(default)]
    pub types: BTreeMap<String, String>,
    pub header: Option<bool>,
    pub delimiter: Option<String>,
    pub date_format: Option<String>,
    pub timestamp_format: Option<String>,
    #[serde(default)]
    pub union_by_name: bool,
    #[serde(default)]
    pub hive_partitioning: bool,
}

/// Published on both the effective limits and the request that can set them.
/// `tests/architecture/` checks the rule itself against the table in
/// `docs/development/data.md`.
const SOURCE_BYTES: &str = "Total size of the objects a request reads end to end. Object size is not transfer: it bounds a CSV or JSONL scan, and a Parquet one only when the request reads every column (`summary`, or `preview` without `columns`). It never bounds tables/describe/explain.";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DataFormat {
    Csv,
    Parquet,
    Jsonl,
}

impl DataFormat {
    /// The format a path's extension names, case-insensitively; `None` for
    /// one `kurama data` does not read.
    pub fn from_path(path: &str) -> Option<Self> {
        let path = path.to_ascii_lowercase();
        if path.ends_with(".parquet") {
            Some(Self::Parquet)
        } else if path.ends_with(".csv") || path.ends_with(".csv.gz") {
            Some(Self::Csv)
        } else if [".jsonl", ".ndjson", ".jsonl.gz", ".ndjson.gz"]
            .iter()
            .any(|extension| path.ends_with(extension))
        {
            Some(Self::Jsonl)
        } else {
            None
        }
    }

    /// Whether a scan has to read the object end to end. A row format has no
    /// footer to read and no column ranges to skip, so its object size is its
    /// transfer; Parquet's is not.
    pub fn reads_whole_object(self) -> bool {
        match self {
            Self::Csv | Self::Jsonl => true,
            Self::Parquet => false,
        }
    }
}

impl DataSource {
    pub fn inferred_format(&self) -> Result<DataFormat, DataError> {
        if let Some(format) = self.format {
            return Ok(format);
        }
        DataFormat::from_path(&self.path).ok_or(DataError::Invalid(InvalidInput::UnsupportedFormat))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default)]
pub struct DataLimits {
    #[schemars(regex(pattern = r"^0*[1-9][0-9]*\s*([kKmMgGtT][iI]?[bB]|[bB])\s*$"))]
    #[schemars(
        description = "Size with a unit; the integer component must be in 1..=18446744073709551615 (u64). The pattern checks syntax; runtime validation enforces the numeric bound."
    )]
    pub memory_limit: String,
    #[schemars(range(min = 1))]
    pub threads: u32,
    #[schemars(range(min = 1))]
    pub query_timeout_secs: u64,
    #[schemars(range(min = 1))]
    pub max_rows: usize,
    #[schemars(range(min = 2))]
    pub max_result_bytes: usize,
    #[schemars(regex(pattern = r"^0*[1-9][0-9]*\s*([kKmMgGtT][iI]?[bB]|[bB])\s*$"))]
    #[schemars(
        description = "Size with a unit; the integer component must be in 1..=18446744073709551615 (u64). The pattern checks syntax; runtime validation enforces the numeric bound."
    )]
    pub max_temp_directory_size: String,
    pub allow_spill: bool,
    #[schemars(range(min = 1))]
    pub max_source_objects: usize,
    #[schemars(range(min = 1))]
    #[schemars(
        description = SOURCE_BYTES
    )]
    pub max_source_bytes: u64,
}
impl Default for DataLimits {
    fn default() -> Self {
        Self {
            memory_limit: "1GB".into(),
            threads: 2,
            query_timeout_secs: 60,
            max_rows: 1000,
            max_result_bytes: 8 * 1024 * 1024,
            max_temp_directory_size: "10GB".into(),
            allow_spill: true,
            max_source_objects: 1000,
            max_source_bytes: 10 * 1024 * 1024 * 1024,
        }
    }
}
impl DataLimits {
    pub fn validate(&self) -> Result<(), DataError> {
        if self.threads == 0
            || self.query_timeout_secs == 0
            || self.max_rows == 0
            || self.max_result_bytes < 2
            || self.max_source_objects == 0
            || self.max_source_bytes == 0
        {
            return Err(DataError::Invalid(InvalidInput::PositiveLimits));
        }
        for value in [&self.memory_limit, &self.max_temp_directory_size] {
            let split = value
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(value.len());
            if value[..split].parse::<u64>().ok().is_none_or(|v| v == 0)
                || !matches!(
                    value[split..].trim().to_ascii_uppercase().as_str(),
                    "B" | "KB" | "MB" | "GB" | "TB" | "KIB" | "MIB" | "GIB" | "TIB"
                )
            {
                return Err(DataError::Invalid(InvalidInput::InvalidMemorySize));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DataOperation {
    Tables,
    Describe,
    Preview,
    Summary,
    Query,
    Explain,
    Export,
}

impl DataOperation {
    /// Whether the operation reads the rows of an object, rather than the
    /// catalog or a footer. Object size bounds only the first kind.
    pub fn scans_input(self) -> bool {
        match self {
            Self::Tables | Self::Describe | Self::Explain => false,
            Self::Preview | Self::Summary | Self::Query | Self::Export => true,
        }
    }
    pub const REQUESTS: [Self; 5] = [
        Self::Tables,
        Self::Describe,
        Self::Preview,
        Self::Summary,
        Self::Query,
    ];
    pub const RESULTS: [Self; 7] = [
        Self::Tables,
        Self::Describe,
        Self::Preview,
        Self::Summary,
        Self::Query,
        Self::Explain,
        Self::Export,
    ];
}

// SQL and request values intentionally do not implement Debug.
#[derive(Clone, Deserialize, JsonSchema)]
#[serde(
    tag = "operation",
    content = "args",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum DataRequest {
    Tables(DataArgs),
    Describe(DataArgs),
    Preview(DataArgs),
    Summary(DataArgs),
    Query(DataArgs),
}

#[derive(Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DataArgs {
    pub from: Option<String>,
    pub aws_profile: Option<String>,
    pub s3_source: Option<String>,
    pub region: Option<String>,
    pub table: Option<String>,
    #[serde(default)]
    pub columns: Vec<String>,
    pub sql: Option<String>,
    pub file: Option<String>,
    #[serde(default)]
    pub explain: bool,
    pub export: Option<String>,
    #[serde(default)]
    pub types: BTreeMap<String, String>,
    pub delimiter: Option<String>,
    pub header: Option<bool>,
    pub date_format: Option<String>,
    pub timestamp_format: Option<String>,
    #[schemars(regex(pattern = r"^0*[1-9][0-9]*\s*([kKmMgGtT][iI]?[bB]|[bB])\s*$"))]
    #[schemars(
        description = "Size with a unit; the integer component must be in 1..=18446744073709551615 (u64). The pattern checks syntax; runtime validation enforces the numeric bound."
    )]
    pub memory_limit: Option<String>,
    #[schemars(range(min = 1))]
    pub threads: Option<u32>,
    #[schemars(range(min = 1))]
    pub query_timeout_secs: Option<u64>,
    #[schemars(range(min = 1))]
    pub max_rows: Option<usize>,
    #[schemars(range(min = 2))]
    pub max_result_bytes: Option<usize>,
    #[schemars(regex(pattern = r"^0*[1-9][0-9]*\s*([kKmMgGtT][iI]?[bB]|[bB])\s*$"))]
    #[schemars(
        description = "Size with a unit; the integer component must be in 1..=18446744073709551615 (u64). The pattern checks syntax; runtime validation enforces the numeric bound."
    )]
    pub max_temp_directory_size: Option<String>,
    pub allow_spill: Option<bool>,
    #[schemars(range(min = 1))]
    pub max_source_objects: Option<usize>,
    #[schemars(range(min = 1))]
    #[schemars(
        description = SOURCE_BYTES
    )]
    pub max_source_bytes: Option<u64>,
}
impl DataRequest {
    pub fn args(&self) -> &DataArgs {
        match self {
            Self::Tables(a)
            | Self::Describe(a)
            | Self::Preview(a)
            | Self::Summary(a)
            | Self::Query(a) => a,
        }
    }
    pub fn operation(&self) -> DataOperation {
        match self {
            Self::Tables(_) => DataOperation::Tables,
            Self::Describe(_) => DataOperation::Describe,
            Self::Preview(_) => DataOperation::Preview,
            Self::Summary(_) => DataOperation::Summary,
            Self::Query(a) if a.export.is_some() => DataOperation::Export,
            Self::Query(a) if a.explain => DataOperation::Explain,
            Self::Query(_) => DataOperation::Query,
        }
    }
    /// Whether this request is known to read every column of every row.
    ///
    /// `SUMMARIZE` computes statistics for all of them and an unprojected
    /// preview selects all of them, so a columnar format saves nothing and its
    /// object size is its transfer. Arbitrary SQL is not assumed to: refusing
    /// it for size would refuse `count(*)`, which reads statistics only.
    pub fn reads_every_column(&self) -> bool {
        match self {
            Self::Summary(_) => true,
            Self::Preview(args) => args.columns.is_empty(),
            Self::Tables(_) | Self::Describe(_) | Self::Query(_) => false,
        }
    }

    /// Whether `max_source_bytes` bounds a source of `format` for this
    /// request. Object size stands in for transfer only where the whole
    /// object is read; `docs/development/data.md` publishes this as a table,
    /// and `tests/architecture/` checks the table against this function.
    pub fn spends_byte_budget(&self, format: DataFormat) -> bool {
        self.operation().scans_input() && (format.reads_whole_object() || self.reads_every_column())
    }

    pub fn validate(&self) -> Result<(), DataError> {
        let a = self.args();
        let query = matches!(self, Self::Query(_));
        if query && (a.sql.is_some() == a.file.is_some()) {
            return Err(DataError::Invalid(InvalidInput::QuerySqlRequired));
        }
        if !query && (a.sql.is_some() || a.file.is_some() || a.explain || a.export.is_some()) {
            return Err(DataError::Invalid(InvalidInput::QueryOptionsOnly));
        }
        if a.explain && a.export.is_some() {
            return Err(DataError::Invalid(InvalidInput::ExplainExportConflict));
        }
        if !matches!(self, Self::Preview(_)) && !a.columns.is_empty() {
            return Err(DataError::Invalid(InvalidInput::PreviewColumnsOnly));
        }
        if (query || matches!(self, Self::Tables(_))) && a.table.is_some() {
            return Err(DataError::Invalid(InvalidInput::UnexpectedTable));
        }
        if a.aws_profile.is_some() && a.s3_source.is_some() {
            return Err(DataError::Invalid(InvalidInput::CredentialSourceConflict));
        }
        Ok(())
    }

    /// Resolve an omitted table only when the selected workspace has one source.
    pub fn select_table(&mut self, sources: &[DataSource]) -> Result<(), DataError> {
        if let Self::Describe(a) | Self::Preview(a) | Self::Summary(a) = self
            && a.table.is_none()
        {
            let [source] = sources else {
                return Err(DataError::Invalid(InvalidInput::TableRequired));
            };
            a.table = Some(source.name.clone());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct InputFile {
    pub source: String,
    pub uri: String,
    pub size: u64,
    pub etag: Option<String>,
    pub version_id: Option<String>,
    pub modified: Option<String>,
}
#[derive(Serialize, JsonSchema)]
pub struct DataColumn {
    pub name: String,
    pub data_type: String,
}
#[derive(Serialize, JsonSchema)]
pub struct DataResult {
    pub columns: Vec<DataColumn>,
    pub rows: Vec<Vec<Value>>,
    pub row_count: usize,
    pub truncated: bool,
    pub stop_reason: Option<String>,
    pub result_bytes: usize,
}

pub fn sql_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
pub fn sql_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_formats_accept_uppercase_extensions() {
        for path in [
            "ORDERS.CSV",
            "ORDERS.CSV.GZ",
            "EVENTS.NDJSON",
            "DATA.PARQUET",
        ] {
            let source: DataSource =
                serde_json::from_value(serde_json::json!({"name":"data", "path":path})).unwrap();
            assert!(source.inferred_format().is_ok(), "{path}");
        }
    }

    #[test]
    fn data_formats_reject_unsupported_extensions_and_use_explicit_format() {
        for path in ["/etc/hosts", "orders.txt", "data.json", "orders.CSV.zip"] {
            let mut source: DataSource =
                serde_json::from_value(serde_json::json!({"name":"data", "path":path})).unwrap();
            assert!(
                matches!(
                    source.inferred_format(),
                    Err(DataError::Invalid(InvalidInput::UnsupportedFormat))
                ),
                "{path}"
            );
            source.format = Some(DataFormat::Csv);
            assert!(
                matches!(source.inferred_format(), Ok(DataFormat::Csv)),
                "{path}"
            );
            source.path = "events.parquet".into();
            assert!(matches!(source.inferred_format(), Ok(DataFormat::Csv)));
        }
    }

    #[test]
    fn data_size_schemas_document_the_runtime_integer_bound() {
        for schema in [
            serde_json::to_value(schemars::schema_for!(DataArgs)).unwrap(),
            serde_json::to_value(schemars::schema_for!(DataLimits)).unwrap(),
        ] {
            for field in ["memory_limit", "max_temp_directory_size"] {
                let description = schema["properties"][field]["description"]
                    .as_str()
                    .unwrap_or("");
                assert!(
                    description.contains("1..=18446744073709551615"),
                    "{field}: {description}"
                );
            }
        }
    }

    #[test]
    fn data_schema_limits_require_positive_values_and_match_defaults() {
        let schema = serde_json::to_value(schemars::schema_for!(DataArgs)).unwrap();
        let defaults = serde_json::to_value(DataLimits::default()).unwrap();
        for (name, value) in defaults.as_object().unwrap() {
            let property = &schema["properties"][name];
            assert!(
                !property.is_null(),
                "default {name} cannot be set in a request"
            );
            if value.is_number() {
                assert_eq!(
                    property["minimum"],
                    if name == "max_result_bytes" { 2 } else { 1 },
                    "{name}"
                );
            }
        }
    }

    #[test]
    fn data_request_options_validate_all_operation_boundaries() {
        for operation in ["tables", "describe", "preview", "summary", "query"] {
            for (field, value) in [
                ("sql", serde_json::json!("SELECT 1")),
                ("file", serde_json::json!("query.sql")),
                ("explain", serde_json::json!(true)),
                ("export", serde_json::json!("out.csv")),
                ("table", serde_json::json!("data")),
                ("columns", serde_json::json!(["id"])),
            ] {
                let mut args = serde_json::json!({});
                if operation == "query" && field != "file" {
                    args["sql"] = "SELECT 1".into();
                }
                args[field] = value;
                let request: DataRequest =
                    serde_json::from_value(serde_json::json!({"operation":operation,"args":args}))
                        .unwrap();
                let expected = match field {
                    "columns" => operation == "preview",
                    "table" => matches!(operation, "describe" | "preview" | "summary"),
                    _ => operation == "query",
                };
                assert_eq!(request.validate().is_ok(), expected, "{operation}: {field}");
            }
        }
    }

    #[test]
    fn data_limits_validate_every_numeric_limit_and_size_unit() {
        let numeric = [
            "threads",
            "query_timeout_secs",
            "max_rows",
            "max_result_bytes",
            "max_source_objects",
            "max_source_bytes",
        ];
        let sizes = ["memory_limit", "max_temp_directory_size"];
        let defaults = serde_json::to_value(DataLimits::default()).unwrap();
        let covered = numeric
            .iter()
            .chain(sizes.iter())
            .copied()
            .chain(["allow_spill"])
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            defaults
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>(),
            covered,
            "every limit needs a validation case when a field is added"
        );
        for allow_spill in [false, true] {
            assert!(
                DataLimits {
                    allow_spill,
                    ..Default::default()
                }
                .validate()
                .is_ok()
            );
        }
        for field in numeric {
            for number in [0, 1, 2] {
                let mut value = serde_json::to_value(DataLimits::default()).unwrap();
                value[field] = number.into();
                let limits: DataLimits = serde_json::from_value(value).unwrap();
                assert_eq!(
                    limits.validate().is_ok(),
                    number >= if field == "max_result_bytes" { 2 } else { 1 },
                    "{field} = {number}"
                );
            }
        }
        for field in sizes {
            for (value, valid) in [
                ("1GB", true),
                ("2 mib", true),
                ("001 kb", true),
                ("1 B ", true),
                ("0GB", false),
                ("-1GB", false),
                ("1.5GB", false),
                ("GB", false),
                ("1", false),
                ("1XB", false),
                ("18446744073709551615GB", true),
                ("18446744073709551616GB", false),
            ] {
                let mut limits = serde_json::to_value(DataLimits::default()).unwrap();
                limits[field] = value.into();
                let limits: DataLimits = serde_json::from_value(limits).unwrap();
                assert_eq!(limits.validate().is_ok(), valid, "{field} = {value}");
            }
        }
    }

    #[test]
    fn data_request_rejects_conflicting_options() {
        for args in [
            serde_json::json!({"sql":"SELECT 1", "file":"a.sql"}),
            serde_json::json!({"sql":"SELECT 1", "explain":true, "export":"out.csv"}),
            serde_json::json!({"sql":"SELECT 1", "aws_profile":"dev", "s3_source":"lake"}),
            serde_json::json!({}),
        ] {
            let request: DataRequest =
                serde_json::from_value(serde_json::json!({"operation":"query", "args":args}))
                    .unwrap();
            assert!(request.validate().is_err());
        }
    }
}
