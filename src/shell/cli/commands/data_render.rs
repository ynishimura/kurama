//! Render typed data output as JSON or terminal-safe text without running analysis.
use crate::domain::{
    functions::parquet_advice,
    types::dataset::{DataOperation, DataOutput, ParquetSummary},
};
use crate::shell::cli::client::{json_line, safe_text, tab_separated};

/// One document on stdout: JSON, or the rows as text.
pub(super) fn print_result(output: &DataOutput, json: bool) {
    if json {
        println!("{}", json_line(output));
        return;
    }
    print!("{}", render_text_result(output));
    if let Some(result) = output.payload.as_ref().and_then(|p| p.result.as_ref())
        && result.truncated
        && output.operation != DataOperation::Preview
    {
        crate::console::progress!(
            "# Result truncated; narrow the query, raise the display limits, or use --query with --export for all rows."
        );
    }
}

fn render_text_result(output: &DataOutput) -> String {
    let Some(payload) = &output.payload else {
        return format!(
            "{}\n",
            serde_json::to_string_pretty(output).expect("typed data plan serializes")
        );
    };
    if let Some(export) = &payload.export {
        return format!(
            "Exported {} bytes to {}\n",
            export.bytes,
            safe_text(&export.path)
        );
    }
    let Some(result) = &payload.result else {
        return String::new();
    };
    let mut text = tab_separated(result.columns.iter().map(|c| c.name.as_str()), &result.rows);
    if let Some(parquet) = &payload.parquet {
        text.push('\n');
        text.push_str(&render_parquet(parquet));
    }
    text
}

/// One line under the column table. The per-column detail is in `--json`.
///
/// Which column is the heaviest and which cannot be skipped are decided in
/// `domain::functions::parquet_advice`, the same place `meta.next_actions`
/// reads them from, so the two output modes cannot disagree.
fn render_parquet(parquet: &ParquetSummary) -> String {
    let heaviest = parquet_advice::heaviest_column(parquet)
        .map(|column| {
            format!(
                "; largest column {} at {} compressed bytes",
                safe_text(&column.name),
                column.compressed_bytes
            )
        })
        .unwrap_or_default();
    let statistics = match parquet_advice::columns_without_statistics(parquet).len() {
        0 => String::new(),
        count => format!("; {count} column(s) without min/max statistics"),
    };
    let omitted = match parquet.files_omitted {
        0 => String::new(),
        count => format!(" (+{count} not read)"),
    };
    let columns = match parquet.columns_omitted {
        0 => String::new(),
        count => format!("; {count} column(s) not measured"),
    };
    format!(
        "parquet: {} file(s){omitted}, {} row group(s) (at most {} per file), {} rows{heaviest}{statistics}{columns}\n",
        parquet.files, parquet.row_groups, parquet.max_row_groups_per_file, parquet.rows
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::dataset::{
        DataKind, DataLimits, DataMeta, DataPayload, EngineInfo, ExportInfo, ParquetColumn,
    };

    #[test]
    fn data_text_parquet_summary_escapes_a_column_name_from_the_file() {
        let parquet = ParquetSummary {
            files: 2,
            files_omitted: 1,
            columns_omitted: 3,
            row_groups: 4,
            max_row_groups_per_file: 3,
            rows: 7000,
            columns: vec![
                ParquetColumn {
                    name: "body\u{1b}[31m".into(),
                    physical_type: "BYTE_ARRAY".into(),
                    compressed_bytes: 900,
                    uncompressed_bytes: 1000,
                    has_statistics: false,
                },
                ParquetColumn {
                    name: "id".into(),
                    physical_type: "INT64".into(),
                    compressed_bytes: 10,
                    uncompressed_bytes: 10,
                    has_statistics: true,
                },
            ],
        };
        assert_eq!(
            render_parquet(&parquet),
            concat!(
                "parquet: 2 file(s) (+1 not read), 4 row group(s) (at most 3 per file), 7000 rows",
                "; largest column body\\u{1b}[31m at 900 compressed bytes",
                "; 1 column(s) without min/max statistics",
                "; 3 column(s) not measured\n"
            )
        );
    }

    #[test]
    fn data_text_escapes_control_characters_without_quoting_strings() {
        assert_eq!(safe_text("a\tb\n\x1b[31m日本語"), r"a\tb\n\u{1b}[31m日本語");
    }

    #[test]
    fn data_text_export_is_one_acknowledgement_line() {
        let output = DataOutput {
            schema_version: 1,
            kind: DataKind::Data,
            operation: DataOperation::Export,
            target: "ad-hoc".into(),
            meta: DataMeta::planned(vec![], DataLimits::default(), None, None),
            dry_run: None,
            elapsed_ms: Some(1),
            payload: Some(DataPayload {
                parquet: None,
                result: None,
                export: Some(ExportInfo {
                    path: "out\nfile.csv".into(),
                    bytes: 42,
                }),
                engine: EngineInfo {
                    name: "duckdb".into(),
                    version: "test".into(),
                },
            }),
        };
        assert_eq!(
            render_text_result(&output),
            "Exported 42 bytes to out\\nfile.csv\n"
        );
    }
}
