//! What a Parquet footer summary says about the next query: one statement of it, for every output mode.
use crate::domain::types::dataset::{
    DataKind, DataOperation, NextAction, ParquetColumn, ParquetSummary,
};

/// The column that dominates the read, when naming one is advice at all.
///
/// A single-column object leaves nothing to drop, so it has no heaviest
/// column to speak of.
pub fn heaviest_column(parquet: &ParquetSummary) -> Option<&ParquetColumn> {
    parquet
        .columns
        .iter()
        .max_by_key(|column| column.compressed_bytes)
        .filter(|_| parquet.columns.len() > 1)
}

/// The columns a filter cannot skip row groups on, in the order reported.
pub fn columns_without_statistics(parquet: &ParquetSummary) -> Vec<&str> {
    parquet
        .columns
        .iter()
        .filter(|column| !column.has_statistics)
        .map(|column| column.name.as_str())
        .collect()
}

/// The typed advice a describe of a Parquet source publishes.
///
/// Naming fewer columns really does move fewer bytes, locally and over S3
/// alike: `data_s3_requests_and_transferred_bytes_are_measured` reads one
/// column of a 1 MB object and measures 97 KB transferred.
pub fn projection_advice(parquet: &ParquetSummary) -> Vec<NextAction> {
    let mut actions = vec![];
    if let Some(heaviest) = heaviest_column(parquet) {
        actions.push(NextAction {
            kind: DataKind::Data,
            operation: DataOperation::Preview,
            message: format!(
                "Column '{}' is the largest at {} compressed bytes; name the columns you need instead of selecting every one.",
                heaviest.name, heaviest.compressed_bytes
            ),
        });
    }
    let unskippable = columns_without_statistics(parquet);
    if !unskippable.is_empty() {
        actions.push(NextAction {
            kind: DataKind::Data,
            operation: DataOperation::Query,
            message: format!(
                "No min/max statistics on {}: a WHERE clause on those columns reads every row group instead of skipping some.",
                named(&unskippable)
            ),
        });
    }
    actions
}

/// At most three names, then how many were left out.
pub fn named(names: &[&str]) -> String {
    let shown = names.iter().take(3).copied().collect::<Vec<_>>().join(", ");
    match names.len().saturating_sub(3) {
        0 => shown,
        rest => format!("{shown} and {rest} more"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(columns: &[(&str, u64, bool)]) -> ParquetSummary {
        ParquetSummary {
            files: 1,
            files_omitted: 0,
            columns_omitted: 0,
            row_groups: 1,
            max_row_groups_per_file: 1,
            rows: 1,
            columns: columns
                .iter()
                .map(|(name, bytes, statistics)| ParquetColumn {
                    name: (*name).into(),
                    physical_type: "BYTE_ARRAY".into(),
                    compressed_bytes: *bytes,
                    uncompressed_bytes: *bytes,
                    has_statistics: *statistics,
                })
                .collect(),
        }
    }

    #[test]
    fn data_advice_names_the_column_that_dominates_the_read() {
        let parquet = summary(&[("id", 10, true), ("body_text", 9000, true)]);
        let actions = projection_advice(&parquet);
        assert_eq!(actions.len(), 1);
        assert!(
            actions[0]
                .message
                .contains("Column 'body_text' is the largest at 9000 compressed bytes")
        );
    }

    #[test]
    fn data_advice_is_silent_about_a_single_column() {
        assert!(heaviest_column(&summary(&[("only", 10, true)])).is_none());
        assert!(projection_advice(&summary(&[("only", 10, true)])).is_empty());
    }

    #[test]
    fn data_columns_without_statistics_keeps_the_reported_order() {
        let parquet = summary(&[("z", 1, false), ("a", 1, true), ("m", 1, false)]);
        assert_eq!(columns_without_statistics(&parquet), ["z", "m"]);
        assert!(columns_without_statistics(&summary(&[("a", 1, true)])).is_empty());
    }

    #[test]
    fn data_advice_names_the_columns_a_filter_cannot_skip() {
        let parquet = summary(&[
            ("a", 1, false),
            ("b", 1, false),
            ("c", 1, false),
            ("d", 1, false),
            ("e", 1, true),
        ]);
        let actions = projection_advice(&parquet);
        let message = &actions.last().expect("statistics advice").message;
        assert!(
            message.contains("No min/max statistics on a, b, c and 1 more"),
            "{message}"
        );
    }

    #[test]
    fn data_advice_lists_up_to_three_names_before_counting_the_rest() {
        assert_eq!(named(&[]), "");
        assert_eq!(named(&["a"]), "a");
        assert_eq!(named(&["a", "b", "c"]), "a, b, c");
        assert_eq!(named(&["a", "b", "c", "d"]), "a, b, c and 1 more");
        assert_eq!(named(&["a", "b", "c", "d", "e"]), "a, b, c and 2 more");
    }
}
