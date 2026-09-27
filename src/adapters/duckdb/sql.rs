//! Pure DuckDB statement generation and scoped table-reference validation.
use crate::domain::types::dataset::{
    DataFormat, DataRequest, DataSource, InputFile, sql_identifier, sql_string,
};
use serde_json::Value;
use std::collections::BTreeSet;

pub(super) fn view_sql(source: &DataSource, files: &[InputFile]) -> String {
    let paths = files
        .iter()
        .filter(|f| f.source == source.name)
        .map(|f| sql_string(&f.uri))
        .collect::<Vec<_>>()
        .join(",");
    let mut options = vec![
        format!("union_by_name={}", source.union_by_name),
        format!("hive_partitioning={}", source.hive_partitioning),
    ];
    let reader = match source.format.expect("resolved input format") {
        DataFormat::Parquet => "read_parquet",
        DataFormat::Jsonl => {
            options.push("ignore_errors=false".into());
            "read_ndjson_auto"
        }
        DataFormat::Csv => {
            options.push("ignore_errors=false".into());
            if let Some(header) = source.header {
                options.push(format!("header={header}"));
            }
            for (key, value) in [
                ("delim", &source.delimiter),
                ("dateformat", &source.date_format),
                ("timestampformat", &source.timestamp_format),
            ] {
                if let Some(value) = value {
                    options.push(format!("{key}={}", sql_string(value)));
                }
            }
            if !source.types.is_empty() {
                options.push(format!(
                    "types={{{}}}",
                    source
                        .types
                        .iter()
                        .map(|(k, v)| format!("{}:{}", sql_string(k), sql_string(v)))
                        .collect::<Vec<_>>()
                        .join(",")
                ));
            }
            "read_csv"
        }
    };
    format!(
        "CREATE VIEW {} AS SELECT * FROM {reader}([{paths}], {})",
        sql_identifier(&source.name),
        options.join(",")
    )
}

pub(super) fn query_sql(request: &DataRequest) -> String {
    match request {
        DataRequest::Tables(_) => "SELECT table_name AS name FROM information_schema.tables WHERE table_schema='main' ORDER BY table_name".into(),
        DataRequest::Describe(args) => format!("DESCRIBE {}", sql_identifier(args.table.as_deref().expect("validated table"))),
        DataRequest::Preview(args) => format!("SELECT {} FROM {}", if args.columns.is_empty() { "*".into() } else { args.columns.iter().map(|c| sql_identifier(c)).collect::<Vec<_>>().join(",") }, sql_identifier(args.table.as_deref().expect("validated table"))),
        DataRequest::Summary(args) => format!("SUMMARIZE {}", sql_identifier(args.table.as_deref().expect("validated table"))),
        DataRequest::Query(args) => {
            let sql = args.sql.as_deref().expect("validated inline or file SQL");
            if args.explain { format!("EXPLAIN {sql}") } else { sql.into() }
        }
    }
}

/// The column names a serialized statement references, ordered and unique.
///
/// A qualified reference keeps only its last part: `t.amount` is the column
/// `amount`. `SELECT *` references no column by name and yields nothing, which
/// is what "as far as the parser shows" means.
pub(super) fn referenced_columns(statement: &Value) -> Vec<String> {
    let mut names = BTreeSet::new();
    collect_columns(statement, &mut names);
    names.into_iter().collect()
}

fn collect_columns(value: &Value, names: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            if map.get("class").and_then(Value::as_str) == Some("COLUMN_REF")
                && let Some(name) = map
                    .get("column_names")
                    .and_then(Value::as_array)
                    .and_then(|parts| parts.last())
                    .and_then(Value::as_str)
            {
                names.insert(name.to_owned());
            }
            for child in map.values() {
                collect_columns(child, names);
            }
        }
        Value::Array(array) => array.iter().for_each(|child| collect_columns(child, names)),
        _ => {}
    }
}

/// CTE declarations become visible in order, only inside their declaring node.
/// A recursive CTE exposes its own name to the recursive right branch only.
pub(super) fn allowed_tables(value: &Value, registered: &BTreeSet<String>) -> bool {
    scoped_tables(value, registered, &BTreeSet::new())
}

fn scoped_tables(value: &Value, registered: &BTreeSet<String>, ctes: &BTreeSet<String>) -> bool {
    let mut scope = ctes.clone();
    if let Some(entries) = value
        .get("cte_map")
        .and_then(|map| map.get("map"))
        .and_then(Value::as_array)
    {
        for entry in entries {
            if !scoped_tables(&entry["value"], registered, &scope) {
                return false;
            }
            let Some(name) = entry["key"].as_str() else {
                return false;
            };
            scope.insert(name.to_lowercase());
        }
    }
    if !allowed_reference(value, registered, &scope) {
        return false;
    }
    match value {
        Value::Object(map) => {
            map.iter()
                .filter(|(key, _)| key.as_str() != "cte_map")
                .all(|(key, child)| {
                    if value["type"] == "RECURSIVE_CTE_NODE" && key == "right" {
                        let Some(name) = value["cte_name"].as_str() else {
                            return false;
                        };
                        let mut recursive = scope.clone();
                        recursive.insert(name.to_lowercase());
                        scoped_tables(child, registered, &recursive)
                    } else {
                        scoped_tables(child, registered, &scope)
                    }
                })
        }
        Value::Array(array) => array
            .iter()
            .all(|child| scoped_tables(child, registered, &scope)),
        _ => true,
    }
}

fn allowed_reference(
    value: &Value,
    registered: &BTreeSet<String>,
    ctes: &BTreeSet<String>,
) -> bool {
    // DuckDB serializes both alias and sample on table references. Expression
    // and query nodes have at most one. New reference kinds fail closed.
    if value.get("alias").is_none() || value.get("sample").is_none() {
        return true;
    }
    match value["type"].as_str() {
        Some("BASE_TABLE") => {
            let Some(name) = value["table_name"].as_str() else {
                return false;
            };
            let name = name.to_lowercase();
            let schema = value["schema_name"].as_str().unwrap_or("");
            // DuckDB never resolves a schema-qualified name to a CTE. Keep
            // registered views separate so a CTE cannot authorize main.system_view.
            let visible = if schema.is_empty() {
                registered.contains(&name) || ctes.contains(&name)
            } else {
                schema.eq_ignore_ascii_case("main") && registered.contains(&name)
            };
            visible && value["catalog_name"].as_str().is_none_or(str::is_empty)
        }
        Some("TABLE_FUNCTION") => matches!(
            value["function"]["function_name"].as_str(),
            Some("range" | "generate_series" | "unnest")
        ),
        Some("SUBQUERY" | "JOIN" | "EXPRESSION_LIST" | "EMPTY" | "PIVOT") => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn data_sql_csv_options_keep_literals_and_identifiers_separate() {
        let source: DataSource = serde_json::from_value(json!({
            "name":"a\"b", "path":"rows.csv", "format":"csv", "header":false,
            "delimiter":"'", "date_format":"%d/%m/%Y", "timestamp_format":"%Y-%m-%d %H:%M:%S",
            "types":{"amount":"DECIMAL(18,2)"}, "union_by_name":true, "hive_partitioning":true
        }))
        .unwrap();
        let files = vec![InputFile {
            source: source.name.clone(),
            uri: "/tmp/it's.csv".into(),
            size: 1,
            etag: None,
            version_id: None,
            modified: None,
        }];
        assert_eq!(
            view_sql(&source, &files),
            "CREATE VIEW \"a\"\"b\" AS SELECT * FROM read_csv(['/tmp/it''s.csv'], union_by_name=true,hive_partitioning=true,ignore_errors=false,header=false,delim='''',dateformat='%d/%m/%Y',timestampformat='%Y-%m-%d %H:%M:%S',types={'amount':'DECIMAL(18,2)'})"
        );
    }

    #[test]
    fn data_sql_referenced_columns_drop_qualifiers_and_repeats() {
        let statement = json!({"select_list": [
            {"class": "COLUMN_REF", "column_names": ["o", "amount"]},
            {"class": "COLUMN_REF", "column_names": ["amount"]},
            {"class": "FUNCTION", "children": [
                {"class": "COLUMN_REF", "column_names": ["main", "o", "body_text"]}
            ]},
            {"class": "CONSTANT", "column_names": ["not_a_reference"]}
        ]});
        assert_eq!(referenced_columns(&statement), ["amount", "body_text"]);
    }

    #[test]
    fn data_sql_unknown_table_reference_kind_fails_closed() {
        let unknown = json!({"type":"FUTURE_REF", "alias":"", "sample":null});
        assert!(!allowed_tables(&unknown, &BTreeSet::new()));
        assert!(allowed_tables(
            &json!({"type":"EMPTY", "alias":"", "sample":null}),
            &BTreeSet::new()
        ));
    }
}
