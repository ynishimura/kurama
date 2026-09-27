//! Physical Parquet facts behind `--describe`: row groups, per-column compressed size and whether min/max statistics exist.
use super::{connection::Connection, result};
use crate::domain::types::{
    dataset::{DataError, DataLimits, InputFile, ParquetColumn, ParquetSummary, sql_string},
    limits::INPUT_LIST,
};
use serde_json::Value;

/// Read the footers of at most `INPUT_LIST.listed` objects and aggregate them.
///
/// These table functions are reachable only from here. The SQL validator's
/// allowlist is unchanged, so a user's `--query` still cannot call them.
///
/// Footer facts are not what a person asked to see, so the display limits do
/// not shape them: the queries run under `footer_limits`, which bounds the
/// column list at `INPUT_LIST.listed` and reports the rest as omitted.
pub(super) fn read(
    connection: &Connection,
    files: &[InputFile],
    limits: &DataLimits,
) -> Result<ParquetSummary, DataError> {
    let (read, files_omitted) = INPUT_LIST.split(files);
    let paths = read
        .iter()
        .map(|file| sql_string(&file.uri))
        .collect::<Vec<_>>()
        .join(",");
    let bounds = footer_limits(limits);
    let totals = result::collect_complete(
        connection,
        &format!(
            "SELECT sum(num_row_groups), max(num_row_groups), sum(num_rows) \
             FROM parquet_file_metadata([{paths}])"
        ),
        &bounds,
    )?;
    let totals = totals
        .first()
        .ok_or(DataError::Engine("parquet file metadata"))?;
    // One row past the bound tells the reader the list is partial without
    // making the reader pay for the whole schema.
    let counted = result::collect_complete(
        connection,
        &format!("SELECT count(DISTINCT path_in_schema) FROM parquet_metadata([{paths}])"),
        &bounds,
    )?;
    let total_columns = counted
        .first()
        .and_then(|row| result::number(row.first()))
        .ok_or(DataError::Engine("parquet column metadata"))? as usize;
    let columns = result::collect_complete(
        connection,
        &format!(
            "SELECT path_in_schema, any_value(type), sum(total_compressed_size), \
             sum(total_uncompressed_size), \
             bool_or(stats_min_value IS NOT NULL OR stats_max_value IS NOT NULL) \
             FROM parquet_metadata([{paths}]) GROUP BY path_in_schema \
             ORDER BY path_in_schema LIMIT {}",
            INPUT_LIST.listed
        ),
        &bounds,
    )?;
    let listed = columns
        .iter()
        .map(|row| {
            Ok(ParquetColumn {
                name: text(row.first()),
                physical_type: text(row.get(1)),
                compressed_bytes: number(row.get(2))?,
                uncompressed_bytes: number(row.get(3))?,
                has_statistics: row.get(4).and_then(Value::as_bool).unwrap_or(false),
            })
        })
        .collect::<Result<Vec<_>, DataError>>()?;
    Ok(ParquetSummary {
        files: read.len(),
        files_omitted,
        columns_omitted: total_columns.saturating_sub(listed.len()),
        row_groups: number(totals.first())?,
        max_row_groups_per_file: number(totals.get(1))?,
        rows: number(totals.get(2))?,
        columns: listed,
    })
}

/// The display limits do not bound a footer read. The column list is cut to
/// the same number as the input list, and the byte budget is whatever it takes
/// to carry that many short rows.
fn footer_limits(limits: &DataLimits) -> DataLimits {
    DataLimits {
        max_rows: INPUT_LIST.listed + 1,
        max_result_bytes: limits.max_result_bytes.max(1 << 20),
        ..limits.clone()
    }
}

/// A number the footer must carry. Refusing is the honest answer when the
/// engine stops printing one the way this module reads it: a published zero
/// would be indistinguishable from a measured zero.
fn number(value: Option<&Value>) -> Result<u64, DataError> {
    result::number(value).ok_or(DataError::Engine("parquet file metadata"))
}

fn text(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).unwrap_or_default().to_owned()
}
