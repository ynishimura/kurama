//! Serializable analysis output contract, shared by execution and schema discovery.
use super::dataset::{DataLimits, DataOperation, DataResult, DataSource, InputFile};
use schemars::JsonSchema;
use serde::Serialize;

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DataKind {
    Data,
}

#[derive(Serialize, JsonSchema)]
pub struct DataOutput {
    #[schemars(range(min = 1, max = 1))]
    pub schema_version: u8,
    pub kind: DataKind,
    pub operation: DataOperation,
    pub target: String,
    pub meta: DataMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<bool>,
    #[serde(flatten)]
    pub payload: Option<DataPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
}

#[derive(Serialize, JsonSchema)]
pub struct DataPayload {
    #[serde(flatten)]
    pub result: Option<DataResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub export: Option<ExportInfo>,
    /// Present only when `describe` read Parquet footers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parquet: Option<ParquetSummary>,
    pub engine: EngineInfo,
}

/// What a Parquet input costs to read, aggregated over the objects whose
/// footer was read: a logical schema does not say whether a column is a
/// gigabyte of text or whether a filter on it can skip row groups.
#[derive(Serialize, JsonSchema)]
pub struct ParquetSummary {
    /// Objects whose footer was read.
    pub files: usize,
    /// Objects left unread because the footer bound was reached.
    pub files_omitted: usize,
    /// Columns left out of `columns` because the column bound was reached.
    /// The advice below is derived from what was read, so a non-zero count
    /// says the advice covers part of the schema.
    pub columns_omitted: usize,
    pub row_groups: u64,
    /// The largest row-group count in any one of those objects.
    pub max_row_groups_per_file: u64,
    pub rows: u64,
    pub columns: Vec<ParquetColumn>,
}

#[derive(Serialize, JsonSchema)]
pub struct ParquetColumn {
    /// The column's path in the Parquet schema; a nested field is dotted.
    pub name: String,
    /// The Parquet physical type. The logical type is in the describe rows.
    pub physical_type: String,
    pub compressed_bytes: u64,
    pub uncompressed_bytes: u64,
    /// Whether any row group carries min/max statistics for the column. A
    /// column without them is read whole whatever the query filters on.
    pub has_statistics: bool,
}

#[derive(Serialize, JsonSchema)]
pub struct ExportInfo {
    pub path: String,
    pub bytes: u64,
}

#[derive(Serialize, JsonSchema)]
pub struct EngineInfo {
    pub name: String,
    pub version: String,
}

#[derive(Serialize, JsonSchema)]
pub struct NextAction {
    pub kind: DataKind,
    pub operation: DataOperation,
    pub message: String,
}

/// Where `meta.region` came from.
#[derive(Serialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RegionSource {
    /// `--region`, the workspace, `[s3.*]` or a regional URL named it; it is
    /// used as it is and never corrected.
    Named,
    /// The AWS profile's region: where the first request is signed, not yet
    /// confirmed as where a bucket lives (a dry run stops here).
    Profile,
    /// Where S3 answered for each bucket read.
    Bucket,
}

#[derive(Serialize, JsonSchema)]
pub struct DataMeta {
    pub limits: DataLimits,
    pub aws_profile: Option<String>,
    pub region: Option<String>,
    /// Whether `region` is where the buckets were read or only where the
    /// first request is signed; null when no S3 input needs a region.
    pub region_source: Option<RegionSource>,
    pub sources: Vec<DataSource>,
    pub consistency: String,
    pub bytes_transferred: Option<u64>,
    pub request_count: Option<u64>,
    pub next_actions: Vec<NextAction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inputs: Option<Vec<InputFile>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inputs_omitted: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
    pub returned_rows: Option<usize>,
    pub returned_result_bytes: Option<usize>,
}

impl DataMeta {
    pub fn planned(
        sources: Vec<DataSource>,
        limits: DataLimits,
        aws_profile: Option<String>,
        region: Option<String>,
    ) -> Self {
        Self {
            sources,
            limits,
            aws_profile,
            region,
            region_source: None,
            consistency: "best_effort".into(),
            bytes_transferred: None,
            request_count: None,
            next_actions: vec![],
            input_count: None,
            input_bytes: None,
            inputs: None,
            inputs_omitted: None,
            observed_at: None,
            returned_rows: None,
            returned_result_bytes: None,
        }
    }
}
