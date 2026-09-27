//! What a read-only SQLite session does with a real file: the guard it cannot
//! be talked out of, the cells it keeps intact, and the bounds it stops at.
use super::*;
use crate::domain::types::database::{
    DbFailure, DbOperation, DbOutcome, DbStatement, DbStopReason, InvalidDb, StatementEnd,
};
use serde_json::json;

/// The error of a call, without asking the success type to be printable: a
/// result holds database cells, which never reach a log or a panic message.
fn failure<T>(result: Result<T, DbError>) -> DbError {
    match result {
        Err(error) => error,
        Ok(_) => panic!("the call succeeded"),
    }
}

/// A database file with one table, written through a separate connection so the
/// session under test never has a way to write.
async fn fixture(statements: &[&str]) -> (tempfile::TempDir, std::path::PathBuf) {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("app.sqlite3");
    let mut writer = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .connect()
        .await
        .expect("a writable database");
    for statement in statements {
        sqlx::query::<Sqlite>(AssertSqlSafe((*statement).to_owned()))
            .execute(&mut writer)
            .await
            .unwrap_or_else(|error| panic!("{statement}: {error}"));
    }
    writer.close().await.expect("a clean close");
    (directory, path)
}

/// The row count the file really holds, through a second read-only session.
async fn count_orders(path: &std::path::Path) -> String {
    let mut session = session(path).await;
    let result = session
        .query("SELECT count(*) FROM orders", &[], &DbLimits::default())
        .await
        .expect("one row");
    session.close().await;
    result.rows[0][0].as_str().expect("a text cell").to_owned()
}

async fn session(path: &std::path::Path) -> SqliteSession {
    SqliteSession::open(path, &DbLimits::default(), false)
        .await
        .expect("an open database")
}

const ORDERS: &[&str] = &[
    "CREATE TABLE customers (id INTEGER PRIMARY KEY, name TEXT NOT NULL)",
    "CREATE TABLE orders (
        id INTEGER PRIMARY KEY,
        customer_id INTEGER REFERENCES customers(id),
        status TEXT NOT NULL DEFAULT 'open',
        amount,
        note TEXT
     )",
    "INSERT INTO customers VALUES (1, 'ACME'), (2, '日本語')",
    "INSERT INTO orders VALUES
        (1, 1, 'open', 42, 'first'),
        (2, 2, 'paid', 1.5, NULL),
        (3, 1, 'void', 'not a number', '')",
];

#[tokio::test]
async fn db_sqlite_read_only_refuses_every_way_of_writing_and_leaves_no_file() {
    let (directory, path) = fixture(ORDERS).await;
    let before: Vec<_> = std::fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    let mut session = session(&path).await;
    let escapes = [
        "INSERT INTO orders VALUES (9, 1, 'open', 1, NULL)",
        "CREATE TABLE sneaky (id INTEGER)",
        "DROP TABLE orders",
        "VACUUM",
        "PRAGMA journal_mode = WAL",
        "ATTACH DATABASE 'new.sqlite3' AS other",
    ];
    for sql in escapes {
        let error = session.query(sql, &[], &DbLimits::default()).await;
        assert!(error.is_err(), "{sql} was accepted");
    }
    // `PRAGMA query_only = 0` is accepted and changes nothing: the file handle
    // is the guard, not the setting.
    let _ = session
        .query("PRAGMA query_only = 0", &[], &DbLimits::default())
        .await;
    assert!(
        session
            .query(
                "INSERT INTO orders VALUES (9, 1, 'open', 1, NULL)",
                &[],
                &DbLimits::default()
            )
            .await
            .is_err()
    );
    session.close().await;
    let after: Vec<_> = std::fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(before, after, "the read added a file to the directory");
}

#[tokio::test]
async fn db_sqlite_refuses_more_than_one_statement_and_statements_without_columns() {
    let (_directory, path) = fixture(ORDERS).await;
    let mut session = session(&path).await;
    let limits = DbLimits::default();
    assert!(matches!(
        session.query("SELECT 1; SELECT 2", &[], &limits).await,
        Err(DbError::Invalid(InvalidDb::MultipleStatements))
    ));
    assert!(matches!(
        session
            .query("SELECT 1; DROP TABLE orders", &[], &limits)
            .await,
        Err(DbError::Invalid(InvalidDb::MultipleStatements))
    ));
    assert!(matches!(
        session.query("DELETE FROM orders", &[], &limits).await,
        Err(DbError::Invalid(InvalidDb::NoColumns))
    ));
    assert!(matches!(
        session.query("-- nothing here", &[], &limits).await,
        Err(DbError::Invalid(InvalidDb::EmptyStatement))
    ));
    // A string or a comment holding a semicolon is still one statement.
    assert!(
        session
            .query("SELECT ';' AS s /* ; */", &[], &limits)
            .await
            .is_ok()
    );
    session.close().await;
}

#[tokio::test]
async fn db_sqlite_returns_every_storage_class_as_the_value_it_stored() {
    let (_directory, path) = fixture(ORDERS).await;
    let mut session = session(&path).await;
    let result = session
        .query(
            "SELECT id, amount, note FROM orders ORDER BY id",
            &[],
            &DbLimits::default(),
        )
        .await
        .expect("three rows");
    assert_eq!(result.row_count, 3);
    assert_eq!(
        result.rows,
        vec![
            vec![json!("1"), json!("42"), json!("first")],
            // A REAL keeps its decimal point, so it is not read back as 1.
            vec![json!("2"), json!("1.5"), Value::Null],
            // A column declared without a type holds whatever was stored.
            vec![json!("3"), json!("not a number"), json!("")],
        ]
    );
    let max = session
        .query(
            "SELECT 9223372036854775807 AS big, CAST('日本' AS BLOB) AS raw",
            &[],
            &DbLimits::default(),
        )
        .await
        .expect("one row");
    assert_eq!(max.rows[0][0], json!("9223372036854775807"));
    assert_eq!(max.rows[0][1], json!("5pel5pys"));
    assert_eq!(max.columns[0].encoding, DbEncoding::Text);
    assert_eq!(max.columns[1].encoding, DbEncoding::Base64);
    session.close().await;
}

/// SQLite does not check that a TEXT value is UTF-8, so a column can hold
/// bytes `try_get::<String, _>` refuses. That is one value kurama could not
/// read, and it used to be reported as "the database connection ended during
/// the read" -- of a connection that is this process and never ended.
#[tokio::test]
async fn db_sqlite_names_a_value_it_cannot_read_instead_of_blaming_the_connection() {
    let (_directory, path) = fixture(&["CREATE TABLE t (v TEXT)"]).await;
    let mut session = session(&path).await;
    let error = session
        .query(
            "SELECT CAST(x'ff' AS TEXT) AS v FROM t UNION ALL SELECT CAST(x'ff' AS TEXT)",
            &[],
            &DbLimits::default(),
        )
        .await
        .err()
        .expect("a TEXT value that is not UTF-8 is not read");
    assert!(
        matches!(
            &error,
            DbError::Failed(DbFailure::UnreadableValue { column, .. })
                if column.to_string() == "v"
        ),
        "{error}"
    );
    assert!(
        !error.to_string().contains("connection ended"),
        "the connection is this process, and it did not end: {error}"
    );
    session.close().await;
}

#[tokio::test]
async fn db_sqlite_names_the_columns_of_a_result_with_no_rows() {
    let (_directory, path) = fixture(ORDERS).await;
    let mut session = session(&path).await;
    let result = session
        .query(
            "SELECT id, status FROM orders WHERE id = 999",
            &[],
            &DbLimits::default(),
        )
        .await
        .expect("an empty result");
    assert_eq!(result.row_count, 0);
    assert_eq!(
        result
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>(),
        ["id", "status"]
    );
    assert_eq!(result.columns[1].data_type.as_deref(), Some("TEXT"));
    assert!(!result.truncated());
    session.close().await;
}

#[tokio::test]
async fn db_sqlite_keeps_columns_that_share_a_name() {
    let (_directory, path) = fixture(ORDERS).await;
    let mut session = session(&path).await;
    let result = session
        .query("SELECT 1 AS a, 2 AS a", &[], &DbLimits::default())
        .await
        .expect("two columns");
    assert_eq!(result.columns.len(), 2);
    assert_eq!(result.rows[0], vec![json!("1"), json!("2")]);
    session.close().await;
}

#[tokio::test]
async fn db_sqlite_binds_parameters_instead_of_interpolating_them() {
    let (_directory, path) = fixture(ORDERS).await;
    let mut session = session(&path).await;
    let result = session
        .query(
            "SELECT id FROM orders WHERE status = ?",
            &[Some("paid".into())],
            &DbLimits::default(),
        )
        .await
        .expect("one row");
    assert_eq!(result.rows, vec![vec![json!("2")]]);
    // A parameter is a value, never SQL.
    let result = session
        .query(
            "SELECT id FROM orders WHERE status = ?",
            &[Some("paid' OR '1'='1".into())],
            &DbLimits::default(),
        )
        .await
        .expect("no rows");
    assert_eq!(result.row_count, 0);
    // A null parameter stays null.
    let result = session
        .query(
            "SELECT id FROM orders WHERE note IS ?",
            &[None],
            &DbLimits::default(),
        )
        .await
        .expect("one row");
    assert_eq!(result.rows, vec![vec![json!("2")]]);
    session.close().await;
}

#[tokio::test]
async fn db_sqlite_stops_at_the_row_and_byte_bounds_and_says_which() {
    let (_directory, path) = fixture(ORDERS).await;
    let mut session = session(&path).await;
    let rows = session
        .query(
            "SELECT id FROM orders ORDER BY id",
            &[],
            &DbLimits {
                max_rows: 2,
                ..DbLimits::default()
            },
        )
        .await
        .expect("a bounded result");
    assert_eq!(rows.row_count, 2);
    assert!(rows.truncated());
    assert_eq!(rows.stop_reason(), Some(DbStopReason::MaxRows));
    let bytes = session
        .query(
            "SELECT id FROM orders ORDER BY id",
            &[],
            &DbLimits {
                max_result_bytes: 8,
                ..DbLimits::default()
            },
        )
        .await
        .expect("a bounded result");
    assert!(bytes.truncated());
    assert_eq!(bytes.stop_reason(), Some(DbStopReason::MaxResultBytes));
    assert!(bytes.result_bytes <= 8);
    // Exactly as many rows as the bound allows is not a truncation.
    let exact = session
        .query(
            "SELECT id FROM orders ORDER BY id",
            &[],
            &DbLimits {
                max_rows: 3,
                ..DbLimits::default()
            },
        )
        .await
        .expect("a full result");
    assert_eq!(exact.row_count, 3);
    assert!(!exact.truncated());
    session.close().await;
}

#[tokio::test]
async fn db_sqlite_preview_pushes_the_row_bound_into_the_statement() {
    let (_directory, path) = fixture(ORDERS).await;
    let mut session = session(&path).await;
    let limits = DbLimits {
        max_rows: 2,
        ..DbLimits::default()
    };
    let result = session
        .preview(None, "orders", &[], &limits)
        .await
        .expect("two rows");
    assert_eq!(result.row_count, 2);
    assert!(result.truncated());
    assert_eq!(result.stop_reason(), Some(DbStopReason::MaxRows));
    let projected = session
        .preview(Some("main"), "orders", &["status".into()], &limits)
        .await
        .expect("one column");
    assert_eq!(
        projected
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>(),
        ["status"]
    );
    // An identifier is quoted, never interpolated.
    assert!(
        session
            .preview(None, "orders\"; DROP TABLE orders --", &[], &limits)
            .await
            .is_err()
    );
    session.close().await;
}

#[tokio::test]
async fn db_sqlite_describes_a_table_with_its_keys_and_references() {
    let (_directory, path) = fixture(ORDERS).await;
    let mut session = session(&path).await;
    let result = session
        .describe(None, "orders")
        .await
        .expect("five columns");
    assert_eq!(
        result
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>(),
        [
            "column",
            "declared_type",
            "nullable",
            "has_default",
            "primary_key",
            "references"
        ]
    );
    assert_eq!(
        result.rows[0],
        vec![
            json!("id"),
            json!("INTEGER"),
            json!("true"),
            json!("false"),
            json!("true"),
            Value::Null
        ]
    );
    assert_eq!(
        result.rows[1],
        vec![
            json!("customer_id"),
            json!("INTEGER"),
            json!("true"),
            json!("false"),
            json!("false"),
            json!("customers.id")
        ]
    );
    // status is NOT NULL with a default; amount has no declared type.
    assert_eq!(result.rows[2][2], json!("false"));
    assert_eq!(result.rows[2][3], json!("true"));
    assert_eq!(result.rows[3][1], Value::Null);
    let missing = failure(session.describe(None, "nope").await);
    assert_eq!(
        missing.server().map(|error| error.next_operation()),
        Some(Some(DbOperation::Tables))
    );
    session.close().await;
}

#[tokio::test]
async fn db_sqlite_lists_schemas_and_tables_one_page_at_a_time() {
    let (_directory, path) = fixture(ORDERS).await;
    let mut session = session(&path).await;
    let schemas = session
        .schemas(&Page {
            after: None,
            limit: 10,
        })
        .await
        .expect("one schema");
    assert_eq!(schemas.result.rows[0][0], json!("main"));
    assert!(schemas.next.is_none());

    let first = session
        .tables(
            None,
            &Page {
                after: None,
                limit: 1,
            },
        )
        .await
        .expect("one table");
    assert_eq!(
        first.result.rows,
        vec![vec![json!("main"), json!("customers"), json!("table")]]
    );
    let next = first.next.expect("a second page");
    assert_eq!(next, ("main".to_owned(), "customers".to_owned()));
    let second = session
        .tables(
            None,
            &Page {
                after: Some(next),
                limit: 1,
            },
        )
        .await
        .expect("the next table");
    assert_eq!(second.result.rows[0][1], json!("orders"));
    assert!(second.next.is_none(), "the listing ended");
    // A schema that does not exist is a rejection, not an empty page.
    assert!(
        session
            .tables(
                Some("nope"),
                &Page {
                    after: None,
                    limit: 10
                }
            )
            .await
            .is_err()
    );
    session.close().await;
}

#[tokio::test]
async fn db_sqlite_cancellation_ends_a_long_read_and_the_file_stays_readable() {
    let (_directory, path) = fixture(ORDERS).await;
    let mut session = session(&path).await;
    let cancel = session.cancellation();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancel.store(true, Ordering::Relaxed);
    });
    let error = failure(
        session
            .query(
                "WITH RECURSIVE forever(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM forever) \
                 SELECT count(*) FROM forever",
                &[],
                &DbLimits::default(),
            )
            .await,
    );
    assert!(
        matches!(
            error,
            DbError::Failed(DbFailure::Interrupted {
                end: StatementEnd::Stopped
            })
        ),
        "{error}"
    );
    session.close().await;
}

#[tokio::test]
async fn db_sqlite_execute_keeps_a_change_only_when_it_was_told_to() {
    let (_directory, path) = fixture(ORDERS).await;
    let limits = DbLimits::default();
    let statement = |sql: &str, param: &str| DbStatement {
        sql: sql.to_owned(),
        params: vec![Some(param.to_owned())],
    };
    assert_eq!(count_orders(&path).await, "3");

    // A rollback runs every statement and keeps nothing.
    let mut writer = SqliteSession::open(&path, &limits, true).await.unwrap();
    let (done, outcome) = writer
        .execute(
            &[statement("DELETE FROM orders WHERE status = ?", "void")],
            &limits,
            false,
            None,
        )
        .await
        .expect("a rolled back change");
    writer.close().await;
    assert_eq!(done[0].rows_affected, 1);
    assert_eq!(done[0].index, 0);
    assert!(done[0].result.is_none(), "a DELETE returns no rows");
    assert!(matches!(outcome, DbOutcome::RolledBack));
    assert_eq!(count_orders(&path).await, "3", "a rollback kept nothing");

    // A commit keeps it, and a statement that returns rows reports them.
    let mut writer = SqliteSession::open(&path, &limits, true).await.unwrap();
    let (done, outcome) = writer
        .execute(
            &[
                statement("DELETE FROM orders WHERE status = ?", "void"),
                DbStatement {
                    sql: "SELECT count(*) AS left_over FROM orders".to_owned(),
                    params: vec![],
                },
            ],
            &limits,
            true,
            None,
        )
        .await
        .expect("a committed change");
    writer.close().await;
    assert!(matches!(outcome, DbOutcome::Committed));
    assert_eq!(done[1].result.as_ref().unwrap().rows[0][0], json!("2"));
    assert_eq!(count_orders(&path).await, "2");
}

#[tokio::test]
async fn db_sqlite_execute_keeps_nothing_when_one_statement_fails_or_goes_too_far() {
    let (_directory, path) = fixture(ORDERS).await;
    let limits = DbLimits::default();
    let delete = DbStatement {
        sql: "DELETE FROM orders WHERE status = 'void'".to_owned(),
        params: vec![],
    };
    let broken = DbStatement {
        sql: "DELETE FROM nope".to_owned(),
        params: vec![],
    };
    let mut writer = SqliteSession::open(&path, &limits, true).await.unwrap();
    let error = failure(
        writer
            .execute(&[delete.clone(), broken], &limits, true, None)
            .await,
    );
    assert!(error.server().is_some(), "{error}");
    writer.close().await;

    // The row bound catches a change that reaches further than it should.
    let mut writer = SqliteSession::open(&path, &limits, true).await.unwrap();
    let error = failure(
        writer
            .execute(
                &[DbStatement {
                    sql: "DELETE FROM orders".to_owned(),
                    params: vec![],
                }],
                &limits,
                true,
                Some(1),
            )
            .await,
    );
    assert!(
        matches!(
            error,
            DbError::Failed(DbFailure::AffectedRowsLimit {
                index: 0,
                rows_affected: 3,
                max: 1
            })
        ),
        "{error}"
    );
    writer.close().await;

    let mut session = session(&path).await;
    let left = session
        .query("SELECT count(*) FROM orders", &[], &limits)
        .await
        .expect("one row");
    assert_eq!(left.rows[0][0], json!("3"), "neither change was kept");
    session.close().await;
}

#[tokio::test]
async fn db_sqlite_counts_a_returning_change_past_the_rows_a_caller_sees() {
    let (_directory, path) = fixture(ORDERS).await;
    // A display bound must not let a large change through: the rows past it
    // are counted even though they are not shown.
    let limits = DbLimits {
        max_rows: 1,
        ..DbLimits::default()
    };
    let mut writer = SqliteSession::open(&path, &limits, true).await.unwrap();
    let error = failure(
        writer
            .execute(
                &[DbStatement {
                    sql: "DELETE FROM orders RETURNING id".to_owned(),
                    params: vec![],
                }],
                &limits,
                true,
                Some(2),
            )
            .await,
    );
    assert!(
        matches!(
            error,
            DbError::Failed(DbFailure::AffectedRowsLimit {
                rows_affected: 3,
                max: 2,
                ..
            })
        ),
        "{error}"
    );
    writer.close().await;
    assert_eq!(count_orders(&path).await, "3", "nothing was kept");
}

#[tokio::test]
async fn db_sqlite_a_file_another_writer_holds_is_a_failure_that_wrote_nothing() {
    let (_directory, path) = fixture(ORDERS).await;
    // Another process is in the middle of a write and keeps the lock.
    let mut holder = SqliteConnectOptions::new()
        .filename(&path)
        .connect()
        .await
        .expect("a second writer");
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut holder)
        .await
        .expect("it takes the write lock");

    // The wait is the query deadline, so this one gives up in a second.
    let limits = DbLimits {
        query_timeout_secs: 1,
        ..DbLimits::default()
    };
    let mut writer = SqliteSession::open(&path, &limits, true)
        .await
        .expect("opening is not blocked by a writer");
    let waited = std::time::Instant::now();
    let error = failure(
        writer
            .execute(
                &[DbStatement {
                    sql: "DELETE FROM orders".to_owned(),
                    params: vec![],
                }],
                &limits,
                true,
                None,
            )
            .await,
    );
    writer.close().await;
    assert!(
        matches!(error, DbError::Failed(DbFailure::Locked)),
        "{error}"
    );
    assert!(
        error.read_only_retry(),
        "a lock it never got is a failure that wrote nothing"
    );
    assert!(
        waited.elapsed() >= std::time::Duration::from_millis(500),
        "it waited for the lock rather than giving up at once"
    );

    sqlx::query("ROLLBACK")
        .execute(&mut holder)
        .await
        .expect("the holder lets go");
    holder.close().await.expect("a clean close");
    assert_eq!(count_orders(&path).await, "3", "nothing was written");
}

#[tokio::test]
async fn db_sqlite_refuses_a_missing_file_and_a_file_that_is_not_a_database() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let missing = directory.path().join("absent.sqlite3");
    assert!(matches!(
        SqliteSession::open(&missing, &DbLimits::default(), false).await,
        Err(DbError::Invalid(InvalidDb::FileUnavailable))
    ));
    assert!(!missing.exists(), "opening created the file");

    let text = directory.path().join("notes.txt");
    std::fs::write(&text, b"this is not a database").expect("a text file");
    let error = failure(SqliteSession::open(&text, &DbLimits::default(), false).await);
    assert!(
        matches!(error, DbError::Invalid(InvalidDb::NotADatabase)),
        "{error:?} / {error}"
    );
}
