//! Read a result stream up to the bounds the caller asked for, whatever engine
//! produced it, and build the result envelope from what was read.
use crate::domain::types::database::{
    DbColumn, DbEncoding, DbError, DbLimits, DbResult, DbStopReason,
};
use futures_util::{Stream, TryStreamExt};
use serde_json::Value;

/// What a prepared statement says about one of its columns, before any row has
/// arrived. An empty result still has to name its columns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: Option<String>,
}

/// One cell, and whether it had to be base64 to stay intact.
pub type Cell = (Value, DbEncoding);

/// Read rows until the stream ends or a bound is reached. The bound that
/// stopped it is named, so a caller can tell a full window from a cut answer.
pub async fn collect<R, S>(
    stream: S,
    columns: Vec<ColumnInfo>,
    limits: &DbLimits,
    decode: impl Fn(&R, usize, &str) -> Result<Cell, DbError>,
) -> Result<DbResult, DbError>
where
    S: Stream<Item = Result<R, DbError>>,
{
    read(stream, columns, limits, decode, false)
        .await
        .map(|(result, _)| result)
}

/// The same bounded read, and the number of rows the statement really
/// produced.
///
/// A write needs both: the display bound says what a caller sees, and
/// `max_affected_rows` has to weigh the whole change. Stopping the count at
/// the display bound would let a large change through a small bound, so the
/// rows past it are read and counted even though none of them is kept.
pub async fn collect_counted<R, S>(
    stream: S,
    columns: Vec<ColumnInfo>,
    limits: &DbLimits,
    decode: impl Fn(&R, usize, &str) -> Result<Cell, DbError>,
) -> Result<(DbResult, u64), DbError>
where
    S: Stream<Item = Result<R, DbError>>,
{
    read(stream, columns, limits, decode, true).await
}

async fn read<R, S>(
    stream: S,
    columns: Vec<ColumnInfo>,
    limits: &DbLimits,
    decode: impl Fn(&R, usize, &str) -> Result<Cell, DbError>,
    count_every_row: bool,
) -> Result<(DbResult, u64), DbError>
where
    S: Stream<Item = Result<R, DbError>>,
{
    let mut encodings = vec![DbEncoding::Text; columns.len()];
    let mut rows: Vec<Vec<Value>> = vec![];
    let mut result_bytes = 0;
    let mut stop_reason: Option<DbStopReason> = None;
    let mut seen = 0u64;
    let mut stream = std::pin::pin!(stream);
    loop {
        let Some(row) = stream.try_next().await? else {
            break;
        };
        seen += 1;
        if stop_reason.is_some() {
            // Past the bound: the row is counted and nothing else.
            continue;
        }
        if rows.len() >= limits.max_rows {
            stop_reason = Some(DbStopReason::MaxRows);
            if count_every_row {
                continue;
            }
            break;
        }
        let mut values = Vec::with_capacity(columns.len());
        for (index, column) in columns.iter().enumerate() {
            let (value, encoding) = decode(&row, index, &column.name)?;
            if encoding == DbEncoding::Base64 {
                encodings[index] = DbEncoding::Base64;
            }
            values.push(value);
        }
        let bytes = row_bytes(&values);
        if result_bytes + bytes > limits.max_result_bytes {
            stop_reason = Some(DbStopReason::MaxResultBytes);
            if count_every_row {
                continue;
            }
            break;
        }
        result_bytes += bytes;
        rows.push(values);
    }
    Ok((
        DbResult::new(
            columns
                .into_iter()
                .zip(encodings)
                .map(|(column, encoding)| DbColumn {
                    name: column.name,
                    data_type: column.data_type,
                    encoding,
                })
                .collect(),
            rows,
            stop_reason,
            result_bytes,
        ),
        seen,
    ))
}

/// A result kurama built itself out of text cells: a catalog listing, or the
/// columns of a table.
pub fn text_result(columns: &[&str], rows: Vec<Vec<Value>>) -> DbResult {
    let result_bytes = rows.iter().map(row_bytes).sum();
    DbResult::new(
        columns
            .iter()
            .map(|name| DbColumn {
                name: (*name).to_owned(),
                data_type: None,
                encoding: DbEncoding::Text,
            })
            .collect(),
        rows,
        None,
        result_bytes,
    )
}

fn row_bytes(row: &Vec<Value>) -> usize {
    serde_json::to_string(row).map_or(0, |text| text.len())
}

/// Cut a listing that asked for one row past its bound into a page and the key
/// the next page starts after.
pub fn paged<T>(
    mut rows: Vec<T>,
    limit: usize,
    columns: &[&str],
    cell: impl Fn(&T) -> Vec<Value>,
    key: impl Fn(&T) -> (String, String),
) -> super::Listing {
    let next = (rows.len() > limit).then(|| {
        rows.truncate(limit);
        key(rows.last().expect("a full page has a last row"))
    });
    let values: Vec<Vec<Value>> = rows.iter().map(&cell).collect();
    super::Listing {
        result: text_result(columns, values),
        next,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rows(count: usize) -> impl Stream<Item = Result<usize, DbError>> {
        futures_util::stream::iter((0..count).map(Ok))
    }

    fn one_column() -> Vec<ColumnInfo> {
        vec![ColumnInfo {
            name: "n".into(),
            data_type: Some("INT".into()),
        }]
    }

    fn decode(row: &usize, _index: usize, _name: &str) -> Result<Cell, DbError> {
        Ok((Value::String(row.to_string()), DbEncoding::Text))
    }

    #[tokio::test]
    async fn db_rows_stop_at_the_bound_that_was_reached_first() {
        let limits = |max_rows, max_result_bytes| DbLimits {
            max_rows,
            max_result_bytes,
            ..DbLimits::default()
        };
        let full = collect(rows(3), one_column(), &limits(3, 1000), decode)
            .await
            .unwrap();
        assert_eq!(full.row_count, 3);
        assert!(
            !full.truncated(),
            "as many rows as the bound allows is not a cut"
        );
        assert_eq!(full.rows[0], vec![json!("0")]);

        let cut = collect(rows(4), one_column(), &limits(2, 1000), decode)
            .await
            .unwrap();
        assert_eq!(cut.row_count, 2);
        assert_eq!(cut.stop_reason(), Some(DbStopReason::MaxRows));

        let bytes = collect(rows(4), one_column(), &limits(9, 8), decode)
            .await
            .unwrap();
        assert_eq!(bytes.stop_reason(), Some(DbStopReason::MaxResultBytes));
        assert!(bytes.result_bytes <= 8);
    }

    #[tokio::test]
    async fn db_rows_name_the_columns_of_an_empty_result() {
        let empty = collect(rows(0), one_column(), &DbLimits::default(), decode)
            .await
            .unwrap();
        assert_eq!(empty.row_count, 0);
        assert_eq!(empty.columns[0].name, "n");
        assert_eq!(empty.columns[0].data_type.as_deref(), Some("INT"));
        assert_eq!(empty.columns[0].encoding, DbEncoding::Text);
    }

    #[tokio::test]
    async fn db_rows_mark_a_column_base64_when_one_of_its_cells_is_binary() {
        let binary = |row: &usize, _: usize, _: &str| -> Result<Cell, DbError> {
            Ok(if *row == 1 {
                (json!("AAEC"), DbEncoding::Base64)
            } else {
                (json!("plain"), DbEncoding::Text)
            })
        };
        let result = collect(rows(3), one_column(), &DbLimits::default(), binary)
            .await
            .unwrap();
        assert_eq!(result.columns[0].encoding, DbEncoding::Base64);
    }

    #[tokio::test]
    async fn db_rows_count_every_row_of_a_change_even_past_the_bound() {
        let limits = DbLimits {
            max_rows: 1,
            ..DbLimits::default()
        };
        let (result, seen) = collect_counted(rows(5), one_column(), &limits, decode)
            .await
            .unwrap();
        assert_eq!(result.row_count, 1, "one row is what a caller sees");
        assert!(result.truncated());
        assert_eq!(
            seen, 5,
            "a display bound must not let a large change through a small one"
        );
    }

    #[test]
    fn db_rows_page_on_the_row_past_the_bound() {
        let items = vec![("main", "a"), ("main", "b"), ("main", "c")];
        let cell = |row: &(&str, &str)| vec![json!(row.0), json!(row.1)];
        let key = |row: &(&str, &str)| (row.0.to_owned(), row.1.to_owned());
        let page = paged(items.clone(), 2, &["schema", "name"], cell, key);
        assert_eq!(page.result.row_count, 2);
        assert_eq!(page.next, Some(("main".to_owned(), "b".to_owned())));
        let last = paged(items[..2].to_vec(), 2, &["schema", "name"], cell, key);
        assert_eq!(last.result.row_count, 2);
        assert_eq!(last.next, None, "a page that is not full ends the listing");
    }
}
