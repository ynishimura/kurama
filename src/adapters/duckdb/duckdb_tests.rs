use super::*;
use crate::domain::types::dataset::DataArgs;
use arrow::array::Array;
use serde_json::{Value, json};

struct JsonAnalysis {
    result: Value,
    export: Option<(tempfile::NamedTempFile, PathBuf)>,
}

fn execute(mut analysis: Analysis, cancel: Arc<Cancellation>) -> Result<JsonAnalysis, DataError> {
    for source in &mut analysis.sources {
        source.format = Some(source.inferred_format()?);
    }
    let output = super::execute(analysis, cancel, &ScannedColumns::default())?;
    Ok(JsonAnalysis {
        result: serde_json::to_value(output.result).unwrap(),
        export: output.export,
    })
}

fn local(sql: &str) -> Analysis {
    let source = DataSource {
        name: "data".into(),
        path: concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/data/orders.csv"
        )
        .into(),
        format: None,
        types: [
            ("customer_id".into(), "VARCHAR".into()),
            ("amount".into(), "DECIMAL(18,2)".into()),
        ]
        .into(),
        header: Some(true),
        delimiter: None,
        date_format: None,
        timestamp_format: None,
        union_by_name: false,
        hive_partitioning: false,
    };
    Analysis {
        files: vec![
            crate::adapters::data_inputs::local_file(&source.name, Path::new(&source.path))
                .unwrap(),
        ],
        sources: vec![source],
        limits: DataLimits::default(),
        request: DataRequest::Query(DataArgs {
            sql: Some(sql.into()),
            ..Default::default()
        }),
        s3: None,
    }
}
#[test]
fn data_engine_jsonl_handles_nested_values_missing_fields_and_gzip() {
    let directory = tempfile::tempdir().unwrap();
    for extension in ["jsonl", "ndjson", "jsonl.gz", "ndjson.gz"] {
        let path = directory.path().join(format!("events.{extension}"));
        let input = include_bytes!("../../../tests/fixtures/data/events.jsonl");
        if extension.ends_with(".gz") {
            let mut gzip = flate2::write::GzEncoder::new(
                fs::File::create(&path).unwrap(),
                flate2::Compression::default(),
            );
            gzip.write_all(input).unwrap();
            gzip.finish().unwrap();
        } else {
            fs::write(&path, input).unwrap();
        }
        let mut analysis = local(
            "SELECT customer_id, event.name, array_length(event.tags), note, identifier FROM data ORDER BY amount",
        );
        analysis.sources[0].path = path.to_string_lossy().into_owned();
        analysis.sources[0].types.clear();
        analysis.sources[0].header = None;
        analysis.files = vec![crate::adapters::data_inputs::local_file("data", &path).unwrap()];
        let output = execute(analysis, Arc::default()).unwrap().result;
        assert_eq!(
            output["rows"],
            json!([
                ["001", "購入", "1", null, "3"],
                ["001", "購入", "2", "first\norder", "9007199254740993"],
                ["002", "return", "0", null, "2"]
            ]),
            "{extension}"
        );
    }
}

#[test]
fn data_engine_jsonl_rejects_malformed_rows() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("broken.jsonl");
    fs::write(&path, "{\"amount\":10}\n{invalid JSON}\n{\"amount\":20}\n").unwrap();
    let mut analysis = local("SELECT sum(amount) FROM data");
    analysis.sources[0].path = path.to_string_lossy().into_owned();
    analysis.sources[0].types.clear();
    analysis.sources[0].header = None;
    analysis.files = vec![crate::adapters::data_inputs::local_file("data", &path).unwrap()];
    assert!(matches!(
        execute(analysis, Arc::default()),
        Err(DataError::Engine(_))
    ));
}

#[test]
fn data_engine_preserves_precision_duplicate_columns_and_nulls() {
    let output = execute(local("SELECT 18446744073709551615::UBIGINT AS x, amount AS x, NULL AS missing, customer_id FROM data ORDER BY amount"), Arc::default()).unwrap().result;
    assert_eq!(
        output["rows"][0],
        json!(["18446744073709551615", "2.25", null, "001"])
    );
    assert_eq!(output["columns"][0]["name"], "x");
    assert_eq!(output["columns"][1]["name"], "x");
}
#[test]
fn data_engine_rejects_multiple_statements_and_writes() {
    for sql in [
        "SELECT 1; SELECT 2",
        "CREATE TABLE x (a INT)",
        "SET threads=5",
        "COPY data TO '/tmp/kurama-forbidden.csv'",
        "INSERT INTO data SELECT * FROM data",
        "UPDATE data SET amount=0",
        "DELETE FROM data",
        "ATTACH ':memory:' AS other",
        "INSTALL httpfs",
        "LOAD httpfs",
    ] {
        assert!(matches!(
            execute(local(sql), Arc::default()),
            Err(DataError::Sql)
        ));
    }
    let output = execute(local("SELECT ';' AS x /* a ; comment */"), Arc::default())
        .unwrap()
        .result;
    assert_eq!(output["rows"][0][0], ";");
}

#[test]
fn data_engine_applies_resource_limits_and_spill_choice() {
    for (threads, memory, spill_size, allow_spill) in [(1, 32, 2, true), (2, 48, 3, false)] {
        let mut analysis = local(
            "SELECT current_setting('threads'), current_setting('memory_limit'), current_setting('max_temp_directory_size'), current_setting('temp_directory') != ''",
        );
        analysis.limits.threads = threads;
        analysis.limits.memory_limit = format!("{memory} MiB");
        analysis.limits.max_temp_directory_size = format!("{spill_size} MiB");
        analysis.limits.allow_spill = allow_spill;
        let output = execute(analysis, Arc::default()).unwrap().result;
        assert_eq!(
            output["rows"][0],
            json!([
                threads.to_string(),
                format!("{memory}.0 MiB"),
                format!("{spill_size}.0 MiB"),
                allow_spill
            ])
        );
    }
}
#[test]
fn data_engine_limits_display_without_limiting_aggregate_input() {
    let mut analysis = local("SELECT sum(amount) FROM data");
    analysis.limits.max_rows = 1;
    let output = execute(analysis, Arc::default()).unwrap().result;
    assert_eq!(output["rows"], json!([["33.00"]]));
    assert_eq!(output["truncated"], false);
    let mut analysis = local("SELECT * FROM data");
    analysis.limits.max_rows = 1;
    assert_eq!(
        execute(analysis, Arc::default()).unwrap().result["truncated"],
        true
    );
}
#[test]
fn data_engine_blocks_files_outside_selected_inputs() {
    let temp = tempfile::NamedTempFile::new().unwrap();
    fs::write(temp.path(), "private\nsecret\n").unwrap();
    let sql = format!(
        "SELECT * FROM read_csv({})",
        sql_string(&temp.path().to_string_lossy())
    );
    assert!(matches!(
        execute(local(&sql), Arc::default()),
        Err(DataError::Sql)
    ));
}

#[test]
fn data_engine_cte_names_stay_in_their_declared_scope() {
    for sql in [
        "SELECT sql FROM duckdb_views WHERE 'x' IN (SELECT 'x' FROM (WITH duckdb_views AS (SELECT 1) SELECT 1))",
        "WITH x AS (SELECT * FROM duckdb_views), duckdb_views AS (SELECT 1) SELECT * FROM x",
        "WITH duckdb_views AS (SELECT * FROM duckdb_views) SELECT * FROM duckdb_views",
        "WITH duckdb_views AS (SELECT 1) SELECT sql FROM main.duckdb_views",
    ] {
        assert!(
            matches!(execute(local(sql), Arc::default()), Err(DataError::Sql)),
            "{sql}"
        );
    }
}

#[test]
fn data_engine_rejects_unregistered_table_reference_kinds() {
    for sql in [
        "SHOW TABLES",
        "SHOW ALL TABLES",
        "DESCRIBE data",
        "SUMMARIZE data",
    ] {
        assert!(
            matches!(execute(local(sql), Arc::default()), Err(DataError::Sql)),
            "{sql}"
        );
    }
}

#[test]
fn data_engine_recursive_ctes_and_nested_table_references_work() {
    for (sql, expected) in [
        (
            "WITH RECURSIVE r(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM r WHERE i<3) SELECT sum(i) FROM r",
            json!([["6"]]),
        ),
        (
            "SELECT a.i+b.i FROM (VALUES (1)) a(i) JOIN LATERAL (SELECT a.i+1 AS i) b ON true",
            json!([["3"]]),
        ),
        (
            "WITH x AS (SELECT 2 AS i), y AS (SELECT i+1 AS i FROM x) SELECT i FROM y",
            json!([["3"]]),
        ),
        (
            "WITH z AS (SELECT 2 AS i), a AS (SELECT i+1 AS i FROM z) SELECT i FROM a",
            json!([["3"]]),
        ),
        (
            "WITH data AS (SELECT 1) SELECT sum(amount) FROM main.data",
            json!([["33.00"]]),
        ),
    ] {
        assert_eq!(
            execute(local(sql), Arc::default()).unwrap().result["rows"],
            expected,
            "{sql}"
        );
    }
}

#[test]
fn data_engine_allowed_paths_admit_only_selected_inputs() {
    let outside = tempfile::NamedTempFile::new().unwrap();
    fs::write(outside.path(), "a\nsecret\n").unwrap();
    let selected = local("SELECT 1").files[0].uri.clone();
    let connection = Connection::open(&[("threads", "1".into())], Arc::default()).unwrap();
    restrict_paths(&connection, &local("SELECT 1").files, None).unwrap();
    let mut rows = 0;
    connection
        .stream(
            &format!("SELECT * FROM read_csv({})", sql_string(&selected)),
            |batch| {
                rows += batch.len();
                Ok(true)
            },
        )
        .unwrap();
    assert_eq!(rows, 3);
    assert!(matches!(
        connection.stream(
            &format!(
                "SELECT * FROM read_csv({})",
                sql_string(&outside.path().to_string_lossy())
            ),
            |_| Ok(true)
        ),
        Err(DataError::Sql)
    ));
}

#[test]
fn data_engine_nested_values_need_exactly_one_json_parse() {
    let output = execute(local("SELECT [{'a': 1, 'more': [2, NULL]}] AS l, MAP {'k': {'x': 3}} AS m, true AS b, from_base64('AAEC/w==') AS bytes"), Arc::default()).unwrap().result;
    assert_eq!(
        serde_json::from_str::<Value>(output["rows"][0][0].as_str().unwrap()).unwrap(),
        json!([{"a":"1", "more":["2", null]}])
    );
    assert_eq!(
        serde_json::from_str::<Value>(output["rows"][0][1].as_str().unwrap()).unwrap(),
        json!([{"key":"k", "value":{"x":"3"}}])
    );
    assert_eq!(output["rows"][0][2], true);
    assert_eq!(output["rows"][0][3], "AAEC/w==");
}

#[test]
fn data_engine_unsigned_hugeint_preserves_its_upper_half() {
    let output = execute(local("SELECT 170141183460469231731687303715884105727::UHUGEINT, 170141183460469231731687303715884105728::UHUGEINT, 340282366920938463463374607431768211455::UHUGEINT, -1::HUGEINT"), Arc::default()).unwrap().result;
    assert_eq!(
        output["rows"][0],
        json!([
            "170141183460469231731687303715884105727",
            "170141183460469231731687303715884105728",
            "340282366920938463463374607431768211455",
            "-1"
        ])
    );
}

#[test]
fn data_engine_and_bundled_httpfs_versions_match() {
    assert_eq!(
        Connection::version(),
        format!("v{}", env!("KURAMA_HTTPFS_VERSION"))
    );
}

#[test]
fn data_engine_accepts_ctes_and_windows_but_rejects_secret_introspection() {
    let output = execute(local("WITH totals AS (SELECT customer_id, sum(amount) AS total FROM data GROUP BY customer_id) SELECT customer_id, total, row_number() OVER (ORDER BY customer_id) FROM totals ORDER BY customer_id"), Arc::default()).unwrap().result;
    assert_eq!(
        output["rows"],
        json!([["001", "12.50", "1"], ["002", "20.50", "2"]])
    );
    for sql in [
        "SELECT * FROM duckdb_secrets()",
        "SELECT * FROM query('SELECT 1')",
        "SELECT * FROM information_schema.tables",
    ] {
        assert!(matches!(
            execute(local(sql), Arc::default()),
            Err(DataError::Sql)
        ));
    }
}

#[test]
fn data_engine_export_is_complete_and_parquet_round_trips() {
    let directory = tempfile::tempdir().unwrap();
    for extension in ["csv", "parquet"] {
        let path = directory.path().join(format!("export.{extension}"));
        let mut analysis = local("SELECT * FROM data ORDER BY amount; -- trailing comment");
        analysis.limits.max_rows = 1;
        if let DataRequest::Query(args) = &mut analysis.request {
            args.export = Some(path.to_string_lossy().into_owned());
        }
        let mut output = execute(analysis, Arc::default()).unwrap();
        assert!(
            !path.exists(),
            "output is unpublished until input verification finishes"
        );
        let (temporary, destination) = output.export.take().unwrap();
        temporary.persist_noclobber(&destination).unwrap();
        let mut analysis = local("SELECT count(*), sum(amount) FROM data");
        analysis.sources[0].path = path.to_string_lossy().into_owned();
        analysis.files = vec![crate::adapters::data_inputs::local_file("data", &path).unwrap()];
        let result = execute(analysis, Arc::default()).unwrap().result;
        assert_eq!(result["rows"], json!([["3", "33.00"]]));
        let mut analysis = local("SELECT * FROM data");
        if let DataRequest::Query(args) = &mut analysis.request {
            args.export = Some(path.to_string_lossy().into_owned());
        }
        assert!(matches!(
            execute(analysis, Arc::default()),
            Err(DataError::Invalid(_))
        ));
    }
}

#[test]
fn data_engine_gzip_and_result_byte_limit() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("data.csv.gz");
    let mut gzip = flate2::write::GzEncoder::new(
        fs::File::create(&path).unwrap(),
        flate2::Compression::default(),
    );
    gzip.write_all(include_bytes!("../../../tests/fixtures/data/orders.csv"))
        .unwrap();
    gzip.finish().unwrap();
    let mut analysis = local("SELECT customer_id, note FROM data ORDER BY amount");
    analysis.sources[0].path = path.to_string_lossy().into_owned();
    analysis.files = vec![crate::adapters::data_inputs::local_file("data", &path).unwrap()];
    let output = execute(analysis, Arc::default()).unwrap().result;
    assert_eq!(output["rows"][1][1], "first\norder");
    assert_eq!(output["rows"][2][1], "日本語");
    let mut analysis = local("SELECT repeat('a', 100) FROM data");
    analysis.limits.max_result_bytes = 20;
    let output = execute(analysis, Arc::default()).unwrap().result;
    assert_eq!(output["rows"], json!([]));
    assert_eq!(output["stop_reason"], "max_result_bytes");
}

#[test]
fn data_engine_late_fetch_failure_is_not_end_of_results() {
    let connection = Connection::open(&[("threads", "1".into())], Arc::default()).unwrap();
    let mut rows = 0;
    let result = connection.stream("SELECT CAST(CASE WHEN i = 1000000 THEN 'invalid' ELSE i::VARCHAR END AS BIGINT) FROM range(2000000) r(i)", |batch| { rows += batch.len(); Ok(true) });
    assert!(
        rows > 0,
        "the fixture must fail after at least one successful chunk"
    );
    assert!(result.is_err());
}

#[test]
fn data_engine_interrupt_stops_running_query() {
    let cancellation = Arc::new(Cancellation::default());
    let worker_cancel = cancellation.clone();
    let (send, receive) = std::sync::mpsc::channel();
    let (finished, result) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let connection = Connection::open(&[("threads", "1".into())], worker_cancel).unwrap();
        send.send(()).unwrap();
        let result = connection.stream(
            "SELECT sum(a.i*b.i) FROM range(100000000) a(i), range(100000000) b(i)",
            |_| Ok(true),
        );
        finished.send(result.map(|_| ())).unwrap();
    });
    receive
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));
    cancellation.cancel();
    assert!(matches!(
        result
            .recv_timeout(std::time::Duration::from_secs(30))
            .unwrap(),
        Err(DataError::Interrupted(_))
    ));
    worker.join().unwrap();
}

#[test]
fn data_engine_secret_literal_escapes_without_plaintext_temporaries() {
    let mut sql = Zeroizing::new(String::new());
    append_literal(&mut sql, "a'b");
    assert_eq!(sql.as_str(), "'a''b'");
    let bytes = super::handles::cstring(&sql).unwrap();
    assert_eq!(bytes.as_slice(), b"'a''b'\0");
    assert!(matches!(
        super::handles::cstring("a\0b"),
        Err(DataError::Invalid(InvalidInput::NulByte))
    ));
}

#[test]
fn data_engine_nested_unsigned_hugeint_preserves_its_upper_half() {
    let output = execute(local("SELECT [340282366920938463463374607431768211455::UHUGEINT], {'n': 340282366920938463463374607431768211455::UHUGEINT}, MAP {'k': 340282366920938463463374607431768211455::UHUGEINT}, [340282366920938463463374607431768211455::UHUGEINT]::UHUGEINT[1]"), Arc::default()).unwrap().result;
    let cells: Vec<Value> = output["rows"][0]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| serde_json::from_str(value.as_str().unwrap()).unwrap())
        .collect();
    assert_eq!(
        cells,
        vec![
            json!(["340282366920938463463374607431768211455"]),
            json!({"n":"340282366920938463463374607431768211455"}),
            json!([{"key":"k","value":"340282366920938463463374607431768211455"}]),
            json!(["340282366920938463463374607431768211455"])
        ]
    );
}

#[test]
fn data_engine_union_values_keep_numeric_precision_and_nested_shape() {
    let output = execute(local("SELECT union_value(v := 340282366920938463463374607431768211455::UHUGEINT), union_value(v := [{'n': 1}]), 'x'::ENUM('x', 'y')"), Arc::default()).unwrap().result;
    assert_eq!(
        output["rows"][0][0],
        "340282366920938463463374607431768211455"
    );
    assert_eq!(
        serde_json::from_str::<Value>(output["rows"][0][1].as_str().unwrap()).unwrap(),
        json!([{"n":"1"}])
    );
    assert_eq!(output["rows"][0][2], "x");
}

/// One Parquet file per requested row count, written by the engine under test.
/// Different counts make the row-group sum and the per-file maximum differ.
fn parquet_analysis(directory: &Path, rows: &[usize]) -> Analysis {
    let connection = Connection::open(
        &[
            ("threads", "1".into()),
            ("autoload_known_extensions", "false".into()),
        ],
        Arc::default(),
    )
    .unwrap();
    let mut inputs = vec![];
    for (index, count) in rows.iter().enumerate() {
        let path = directory.join(format!("orders-{index}.parquet"));
        connection
            .execute(
                &format!(
                    "COPY (SELECT ('c' || (i % 3))::VARCHAR AS customer_id, i::BIGINT AS amount FROM range({count}) t(i)) TO {} (FORMAT PARQUET, ROW_GROUP_SIZE 2048)",
                    sql_string(&path.to_string_lossy())
                ),
                "fixture",
            )
            .unwrap();
        inputs.push(crate::adapters::data_inputs::local_file("data", &path).unwrap());
    }
    let source = DataSource {
        name: "data".into(),
        path: directory
            .join("orders-*.parquet")
            .to_string_lossy()
            .into_owned(),
        format: Some(crate::domain::types::dataset::DataFormat::Parquet),
        types: Default::default(),
        header: None,
        delimiter: None,
        date_format: None,
        timestamp_format: None,
        union_by_name: false,
        hive_partitioning: false,
    };
    Analysis {
        files: inputs,
        sources: vec![source],
        limits: DataLimits::default(),
        request: DataRequest::Describe(DataArgs {
            table: Some("data".into()),
            ..Default::default()
        }),
        s3: None,
    }
}

#[test]
fn data_describe_reports_parquet_row_groups_statistics_and_column_sizes() {
    let directory = tempfile::tempdir().unwrap();
    // 5,000 rows fill three 2,048-row groups; 2,000 rows fill one.
    let output = execute(
        parquet_analysis(directory.path(), &[5000, 2000]),
        Arc::default(),
    )
    .unwrap()
    .result;
    let parquet = &output["parquet"];
    assert_eq!(parquet["files"], 2);
    assert_eq!(parquet["files_omitted"], 0);
    assert_eq!(parquet["rows"], 7000);
    assert_eq!(parquet["row_groups"], 4);
    assert_eq!(parquet["max_row_groups_per_file"], 3);
    let columns = parquet["columns"].as_array().unwrap();
    assert_eq!(
        columns
            .iter()
            .map(|c| c["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["amount", "customer_id"]
    );
    assert_eq!(columns[0]["physical_type"], "INT64");
    assert_eq!(columns[0]["has_statistics"], true);
    assert!(columns[0]["compressed_bytes"].as_u64().unwrap() > 0);
    assert!(
        columns[0]["uncompressed_bytes"].as_u64().unwrap()
            >= columns[0]["compressed_bytes"].as_u64().unwrap()
    );
    // The logical schema is still the describe result itself.
    assert_eq!(output["rows"][0][0], "customer_id");
}

#[test]
fn data_describe_of_a_row_format_has_no_physical_block() {
    let mut analysis = local("");
    analysis.request = DataRequest::Describe(DataArgs {
        table: Some("data".into()),
        ..Default::default()
    });
    let output = execute(analysis, Arc::default()).unwrap().result;
    assert_eq!(output["parquet"], Value::Null);
    assert_eq!(output["rows"][0][0], "customer_id");
}

#[test]
fn data_engine_referenced_columns_come_from_the_parser_that_runs_the_query() {
    let analysis = local("SELECT 1");
    let connection = Connection::open(
        &[
            ("threads", "1".into()),
            ("autoload_known_extensions", "false".into()),
        ],
        Arc::default(),
    )
    .unwrap();
    let mut source = analysis.sources[0].clone();
    source.format = Some(source.inferred_format().unwrap());
    connection
        .execute(&sql::view_sql(&source, &analysis.files), "source schema")
        .unwrap();
    let tables = vec!["data".to_string()];
    let columns =
        |query: &str| sql::referenced_columns(&connection.validate_select(query, &tables).unwrap());
    assert_eq!(columns("SELECT amount FROM data"), ["amount"]);
    assert_eq!(
        columns("SELECT d.amount FROM data d WHERE d.customer_id = '1'"),
        ["amount", "customer_id"]
    );
    // A star names no column, which is what the error document reports as an
    // empty list. This pins DuckDB's own serialization, not an assumption.
    assert!(columns("SELECT * FROM data").is_empty());
}
