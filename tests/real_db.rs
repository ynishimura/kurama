//! What a real PostgreSQL and a real MySQL do, which no fake can stand in for.
//!
//! These tests need the databases `cargo xtask db-up` starts and run only with
//! `KURAMA_TEST_DB=1`; without it each one says it did not run and returns, so
//! the gate never depends on Docker. Everything here is a fact about the
//! server: the bytes it sends for each type, what its read guard refuses, what
//! it does with a second statement, and whether a cancel from another
//! connection reaches the statement.
use kurama::adapters::config::{DbAuth, DbEngine, DbTls, ServerDatabase};
use kurama::adapters::database::Page;
use kurama::adapters::database::server::{ServerAccess, ServerPassword, ServerSession};
use kurama::domain::types::SecretRef;
use kurama::domain::types::database::{
    DbError, DbFailure, DbLimits, DbResult, DbStatement, DbStopReason, InvalidDb,
};
use std::path::PathBuf;
/// The tunnel tests set `PATH` and `KURAMA_*` for the whole process, so they
/// take turns. Without this they ran beside each other and each saw the
/// other's endpoint and plugin mode. It is held across awaits, so it is the
/// async lock and not the standard one.
static TUNNEL_ENVIRONMENT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// `std::env::set_var` is unsafe since edition 2024, and the argument that
/// makes it sound is the same at every call site here, so it is made once:
/// every caller holds [`TUNNEL_ENVIRONMENT`], which is what already kept these
/// tests from seeing each other's endpoint and plugin mode.
mod test_env {
    use std::ffi::OsStr;

    pub fn set(key: &str, value: impl AsRef<OsStr>) {
        unsafe { std::env::set_var(key, value) }
    }

    pub fn remove(key: &str) {
        unsafe { std::env::remove_var(key) }
    }
}

fn put_the_fake_plugin_on_the_path() {
    let fakes = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fakes");
    let path = std::env::var("PATH").unwrap_or_default();
    if !path.starts_with(&format!("{}:", fakes.display())) {
        test_env::set("PATH", format!("{}:{path}", fakes.display()));
    }
}

/// The password `tests/fakes/op` prints, which is what `up.sh` gave the
/// servers: a scenario and these tests then use the same secret.
const PASSWORD: &str = "fake-client-secret";

fn enabled(test: &str) -> bool {
    if std::env::var("KURAMA_TEST_DB").is_ok() {
        return true;
    }
    eprintln!(
        "{test}: not run; start the databases with `cargo xtask db-up` and set KURAMA_TEST_DB=1"
    );
    false
}

fn tls_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/db/tls")
}

fn port(name: &str, default: u16) -> u16 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Server {
    Postgres17,
    Postgres18,
    MySql,
}

/// The engines these tests really run against. `tests/architecture/` reads
/// this list, not the file: every engine name also appears in prose, so a
/// whole-file search said MySQL was covered after it had been dropped.
const REAL_ENGINES: [&str; 2] = ["postgresql", "mysql"];

impl Server {
    fn every() -> [Self; 3] {
        [Self::Postgres17, Self::Postgres18, Self::MySql]
    }

    /// Which entry of [`REAL_ENGINES`] this server is one of.
    fn engine(self) -> &'static str {
        match self {
            Self::Postgres17 | Self::Postgres18 => "postgresql",
            Self::MySql => "mysql",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Postgres17 => "postgresql 17",
            Self::Postgres18 => "postgresql 18",
            Self::MySql => "mysql 8.4",
        }
    }

    fn is_postgres(self) -> bool {
        !matches!(self, Self::MySql)
    }

    /// The placeholder this engine writes.
    fn placeholder(self, index: usize) -> String {
        if self.is_postgres() {
            format!("${index}")
        } else {
            "?".to_owned()
        }
    }

    fn access(self) -> ServerAccess {
        let (engine, port, ca) = match self {
            Self::Postgres17 => (
                DbEngine::Postgresql,
                port("KURAMA_TEST_PG17_PORT", 55432),
                "ca.pem",
            ),
            Self::Postgres18 => (
                DbEngine::Postgresql,
                port("KURAMA_TEST_PG18_PORT", 55433),
                "ca.pem",
            ),
            Self::MySql => (
                DbEngine::Mysql,
                port("KURAMA_TEST_MYSQL_PORT", 55306),
                "mysql-ca.pem",
            ),
        };
        ServerAccess {
            database: ServerDatabase {
                engine,
                host: "127.0.0.1".into(),
                port,
                database: "app".into(),
                username: SecretRef::Literal("kurama".into()),
                auth: DbAuth::Password(SecretRef::OnePassword("op://Agent/db/password".into())),
                // The certificate says `db.test` and nothing connects by that
                // name, which is the situation verify-ca exists for.
                tls: DbTls::VerifyCa,
                ca_file: Some(tls_dir().join(ca)),
                connect_timeout_secs: 10,
                allow_write: true,
                tunnel: None,
                limits: DbLimits::default(),
                aurora_dsql: false,
            },
            host: "127.0.0.1".into(),
            port,
            username: "kurama".into(),
            password: ServerPassword::Fixed(PASSWORD.into()),
        }
    }
}

async fn connect(server: Server) -> ServerSession {
    ServerSession::open(server.access())
        .await
        .unwrap_or_else(|error| panic!("{}: {error}", server.name()))
}

fn failure<T>(result: Result<T, DbError>, what: &str) -> DbError {
    match result {
        Err(error) => error,
        Ok(_) => panic!("{what} was accepted"),
    }
}

/// The cells of one row, by column name.
fn row_of(result: &DbResult) -> Vec<(String, Option<String>)> {
    result
        .columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            (
                column.name.clone(),
                result.rows[0][index].as_str().map(str::to_owned),
            )
        })
        .collect()
}

fn expect_cells(server: Server, result: &DbResult, expected: &[(&str, Option<&str>)]) {
    let found = row_of(result);
    for (name, value) in expected {
        let (_, actual) = found
            .iter()
            .find(|(column, _)| column == name)
            .unwrap_or_else(|| panic!("{}: no column {name}", server.name()));
        assert_eq!(
            actual.as_deref(),
            *value,
            "{}: column {name}",
            server.name()
        );
    }
}

#[tokio::test]
async fn db_real_reads_every_value_as_the_server_stored_it() {
    if !enabled("db_real_reads_every_value_as_the_server_stored_it") {
        return;
    }
    for server in Server::every() {
        let mut session = connect(server).await;
        let result = session
            .query(
                "SELECT * FROM types WHERE id = 1",
                &[],
                &DbLimits::default(),
            )
            .await
            .unwrap_or_else(|error| panic!("{}: {error}", server.name()));
        let expected: &[(&str, Option<&str>)] = if server.is_postgres() {
            &[
                ("a_bool", Some("true")),
                ("a_int2", Some("-32768")),
                ("a_int4", Some("-1")),
                ("a_int8", Some("9223372036854775807")),
                ("a_float4", Some("0.1")),
                ("a_float8", Some("0.1")),
                ("a_numeric", Some("12345678901234567890.123456789")),
                ("a_text", Some("日本語")),
                ("a_varchar", Some("")),
                ("a_bpchar", Some("ab  ")),
                ("a_uuid", Some("12345678-9abc-def0-1234-56789abcdef0")),
                ("a_json", Some("{\"a\": 1}")),
                ("a_jsonb", Some("{\"a\": 1}")),
                ("a_date", Some("1999-12-31")),
                ("a_time", Some("12:00:00.5")),
                ("a_timestamp", Some("2026-09-21 12:34:56.75")),
                ("a_timestamptz", Some("2026-09-21 12:34:56+00")),
                ("a_bytea", Some("AAEC")),
                ("a_int4_array", Some("{1,NULL,3}")),
                ("a_text_array", Some("{\"a,b\",\"\",\"NULL\",plain}")),
                ("a_matrix", Some("{{1,2},{3,4}}")),
            ]
        } else {
            &[
                ("a_tinyint", Some("-1")),
                ("a_bool", Some("1")),
                ("a_smallint", Some("-2")),
                ("a_mediumint", Some("-3")),
                ("a_int", Some("-4")),
                ("a_bigint", Some("-9223372036854775808")),
                ("a_uint", Some("18446744073709551615")),
                ("a_float", Some("0.1")),
                ("a_double", Some("0.1")),
                ("a_decimal", Some("12345678901234567890.123456789")),
                ("a_varchar", Some("日本語")),
                ("a_char", Some("ab")),
                ("a_text", Some("")),
                ("a_json", Some("{\"a\": 1}")),
                ("a_enum", Some("open")),
                ("a_set", Some("a,b")),
                ("a_date", Some("2026-09-21")),
                ("a_time", Some("12:00:00.5")),
                ("a_datetime", Some("2026-09-21 12:34:56.75")),
                ("a_timestamp", Some("2026-09-21 12:34:56")),
                ("a_blob", Some("AAEC")),
                ("a_binary", Some("AAEC")),
                ("a_bit", Some("256")),
                ("a_year", Some("2026")),
            ]
        };
        expect_cells(server, &result, expected);
        // A binary column says how it is written; a text one does not.
        let binary = if server.is_postgres() {
            "a_bytea"
        } else {
            "a_blob"
        };
        let encoding = result
            .columns
            .iter()
            .find(|column| column.name == binary)
            .map(|column| column.encoding);
        assert_eq!(
            encoding,
            Some(kurama::domain::types::database::DbEncoding::Base64),
            "{}",
            server.name()
        );

        // A null of every type is a null, not an empty string or a zero.
        let nulls = session
            .query(
                "SELECT * FROM types WHERE id = 2",
                &[],
                &DbLimits::default(),
            )
            .await
            .expect("the all-null row");
        assert!(
            nulls.rows[0].iter().skip(1).all(serde_json::Value::is_null),
            "{}: a null came back as something else",
            server.name()
        );
        session.close().await;
    }
}

#[tokio::test]
async fn db_real_postgres_keeps_the_values_no_decimal_type_holds() {
    if !enabled("db_real_postgres_keeps_the_values_no_decimal_type_holds") {
        return;
    }
    for server in [Server::Postgres17, Server::Postgres18] {
        let mut session = connect(server).await;
        let result = session
            .query(
                "SELECT a_numeric, a_float8 FROM types WHERE id = 3",
                &[],
                &DbLimits::default(),
            )
            .await
            .unwrap_or_else(|error| panic!("{}: {error}", server.name()));
        expect_cells(
            server,
            &result,
            &[("a_numeric", Some("NaN")), ("a_float8", Some("NaN"))],
        );
        session.close().await;
    }
}

#[tokio::test]
async fn db_real_read_guard_refuses_every_way_of_writing() {
    if !enabled("db_real_read_guard_refuses_every_way_of_writing") {
        return;
    }
    for server in Server::every() {
        for sql in [
            "INSERT INTO orders VALUES (9, 1, 'open', 1, NULL)",
            "DELETE FROM orders",
            "UPDATE orders SET status = 'open'",
            "CREATE TABLE sneaky (id integer)",
            "DROP TABLE orders",
        ] {
            let mut session = connect(server).await;
            let error = failure(
                session.query(sql, &[], &DbLimits::default()).await,
                &format!("{}: {sql}", server.name()),
            );
            // Either the read guard refuses it, or it never was a read: both
            // stop it before it runs.
            assert!(
                error.server().is_some() || matches!(error, DbError::Invalid(_)),
                "{}: {sql} -> {error}",
                server.name()
            );
            session.close().await;
        }
        // And the third guard is named, not merely implied by the first: a
        // statement with no columns is not a read, whatever the transaction
        // would have done with it. `error.server().is_some()` above passes on
        // the read-only transaction alone, so deleting this guard left every
        // assertion green.
        //
        // MySQL 8.4.11 refuses these already at PREPARE inside the read-only
        // transaction (SQLSTATE 25006), before kurama reads the columns; 8.4.9
        // prepared them. So on MySQL the refusal is pinned as the server's,
        // and the column guard is pinned by the PostgreSQL servers.
        for sql in ["DELETE FROM orders", "UPDATE orders SET status = 'open'"] {
            let mut session = connect(server).await;
            let error = failure(
                session.query(sql, &[], &DbLimits::default()).await,
                &format!("{}: {sql} has no columns", server.name()),
            );
            let refused = match server {
                Server::MySql => error.server().is_some_and(|server| server.code == "25006"),
                Server::Postgres17 | Server::Postgres18 => {
                    matches!(error, DbError::Invalid(InvalidDb::NoColumns))
                }
            };
            assert!(refused, "{}: {sql} -> {error}", server.name());
            session.close().await;
        }
        // Nothing of that reached the table.
        let mut session = connect(server).await;
        let left = session
            .query("SELECT count(*) FROM orders", &[], &DbLimits::default())
            .await
            .expect("the orders are still there");
        assert_eq!(
            left.rows[0][0].as_str(),
            Some("4"),
            "{}: a write got through",
            server.name()
        );
        session.close().await;
    }
}

#[tokio::test]
async fn db_real_refuses_a_second_statement_and_one_that_returns_nothing() {
    if !enabled("db_real_refuses_a_second_statement_and_one_that_returns_nothing") {
        return;
    }
    for server in Server::every() {
        let mut session = connect(server).await;
        for sql in [
            "SELECT 1; SELECT 2",
            "SELECT 1; DROP TABLE orders",
            "COMMIT; CREATE TABLE sneaky (id integer)",
        ] {
            let error = failure(
                session.query(sql, &[], &DbLimits::default()).await,
                &format!("{}: {sql}", server.name()),
            );
            assert!(
                error.server().is_some(),
                "{}: {sql} was not refused by the server -> {error}",
                server.name()
            );
        }
        session.close().await;
        // `orders` is proof the escape did not run.
        let mut session = connect(server).await;
        assert!(
            session
                .query("SELECT count(*) FROM orders", &[], &DbLimits::default())
                .await
                .is_ok()
        );
        session.close().await;
    }
}

#[tokio::test]
async fn db_real_names_the_columns_of_a_result_with_no_rows() {
    if !enabled("db_real_names_the_columns_of_a_result_with_no_rows") {
        return;
    }
    for server in Server::every() {
        let mut session = connect(server).await;
        let result = session
            .query(
                "SELECT id, status FROM orders WHERE id = 999",
                &[],
                &DbLimits::default(),
            )
            .await
            .unwrap_or_else(|error| panic!("{}: {error}", server.name()));
        assert_eq!(result.row_count, 0, "{}", server.name());
        assert_eq!(
            result
                .columns
                .iter()
                .map(|column| column.name.as_str())
                .collect::<Vec<_>>(),
            ["id", "status"],
            "{}",
            server.name()
        );
        // Two columns of one name are two columns, not one.
        let same = session
            .query("SELECT 1 AS a, 2 AS a", &[], &DbLimits::default())
            .await
            .expect("two columns");
        assert_eq!(same.columns.len(), 2, "{}", server.name());
        session.close().await;
    }
}

#[tokio::test]
async fn db_real_binds_parameters_and_postgres_needs_the_cast_the_contract_names() {
    if !enabled("db_real_binds_parameters_and_postgres_needs_the_cast_the_contract_names") {
        return;
    }
    for server in Server::every() {
        let mut session = connect(server).await;
        let text = format!(
            "SELECT id FROM orders WHERE status = {}",
            server.placeholder(1)
        );
        let result = session
            .query(&text, &[Some("paid".into())], &DbLimits::default())
            .await
            .unwrap_or_else(|error| panic!("{}: {error}", server.name()));
        assert_eq!(result.rows[0][0].as_str(), Some("2"), "{}", server.name());

        // A parameter is a value, never SQL.
        let injected = session
            .query(
                &text,
                &[Some("paid' OR '1'='1".into())],
                &DbLimits::default(),
            )
            .await
            .expect("no rows");
        assert_eq!(injected.row_count, 0, "{}", server.name());

        // A string parameter against an integer column: PostgreSQL asks for a
        // cast and says so with 42883, MySQL compares it as it is.
        let numeric = format!("SELECT id FROM orders WHERE id = {}", server.placeholder(1));
        let answer = session
            .query(&numeric, &[Some("2".into())], &DbLimits::default())
            .await;
        if server.is_postgres() {
            let error = failure(answer, "an uncast text parameter");
            // The parameter is declared text, so the refusal names the
            // comparison that has no operator rather than a malformed message.
            assert_eq!(
                error.server().map(|server| server.code.as_str()),
                Some("42883"),
                "{}: {error}",
                server.name()
            );
            assert!(
                error
                    .server()
                    .is_some_and(|server| server.message.contains("integer = text")),
                "{}: {error}",
                server.name()
            );
            // The failure aborted the transaction the read guard opened, and a
            // guard covers one statement: the next one is its own connection,
            // which is what the CLI does for every call.
            session.close().await;
            session = connect(server).await;
            let cast = format!(
                "SELECT id FROM orders WHERE id = {}::int",
                server.placeholder(1)
            );
            let result = session
                .query(&cast, &[Some("2".into())], &DbLimits::default())
                .await
                .expect("the cast makes it work");
            assert_eq!(result.rows[0][0].as_str(), Some("2"));
        } else {
            assert_eq!(
                answer.expect("mysql compares it").rows[0][0].as_str(),
                Some("2")
            );
        }
        session.close().await;
    }
}

#[tokio::test]
async fn db_real_catalog_lists_what_the_database_has() {
    if !enabled("db_real_catalog_lists_what_the_database_has") {
        return;
    }
    for server in Server::every() {
        let mut session = connect(server).await;
        let schemas = session
            .schemas(&Page {
                after: None,
                limit: 100,
            })
            .await
            .unwrap_or_else(|error| panic!("{}: {error}", server.name()));
        let names: Vec<&str> = schemas
            .result
            .rows
            .iter()
            .filter_map(|row| row[0].as_str())
            .collect();
        let expected = if server.is_postgres() {
            "public"
        } else {
            "app"
        };
        assert!(
            names.contains(&expected),
            "{}: {names:?} has no {expected}",
            server.name()
        );

        let tables = session
            .tables(
                None,
                &Page {
                    after: None,
                    limit: 100,
                },
            )
            .await
            .expect("the tables");
        let found: Vec<&str> = tables
            .result
            .rows
            .iter()
            .filter_map(|row| row[1].as_str())
            .collect();
        for name in ["customers", "orders", "open_orders", "types"] {
            assert!(found.contains(&name), "{}: {found:?}", server.name());
        }

        // One page at a time, continuing after exactly the last row.
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
        assert_eq!(first.result.row_count, 1, "{}", server.name());
        let next = first.next.expect("a second page");
        let second = session
            .tables(
                None,
                &Page {
                    after: Some(next.clone()),
                    limit: 1,
                },
            )
            .await
            .expect("the next table");
        assert_ne!(
            second.result.rows[0][1],
            first.result.rows[0][1],
            "{}: the cursor repeated a row",
            server.name()
        );

        let described = session
            .describe(None, "orders")
            .await
            .expect("the columns of orders");
        let columns: Vec<&str> = described
            .rows
            .iter()
            .filter_map(|row| row[0].as_str())
            .collect();
        assert_eq!(
            columns,
            ["id", "customer_id", "status", "amount", "note"],
            "{}",
            server.name()
        );
        let id = &described.rows[0];
        assert_eq!(
            id[2].as_str(),
            Some("false"),
            "{}: id is NOT NULL",
            server.name()
        );
        assert_eq!(
            id[4].as_str(),
            Some("true"),
            "{}: id is the key",
            server.name()
        );
        let customer = &described.rows[1];
        assert_eq!(
            customer[5].as_str(),
            Some("customers.id"),
            "{}: the reference",
            server.name()
        );
        let status = &described.rows[2];
        assert_eq!(
            status[3].as_str(),
            Some("true"),
            "{}: status has a default",
            server.name()
        );
        session.close().await;
    }
}

#[tokio::test]
async fn db_real_stops_at_the_bounds_and_says_which_one() {
    if !enabled("db_real_stops_at_the_bounds_and_says_which_one") {
        return;
    }
    for server in Server::every() {
        let mut session = connect(server).await;
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
            .unwrap_or_else(|error| panic!("{}: {error}", server.name()));
        assert_eq!(rows.row_count, 2, "{}", server.name());
        assert_eq!(
            rows.stop_reason(),
            Some(DbStopReason::MaxRows),
            "{}",
            server.name()
        );
        session.close().await;

        let mut session = connect(server).await;
        let preview = session
            .preview(
                None,
                "orders",
                &["id".into()],
                &DbLimits {
                    max_rows: 2,
                    ..DbLimits::default()
                },
            )
            .await
            .expect("a preview");
        assert_eq!(preview.row_count, 2, "{}", server.name());
        assert!(preview.truncated(), "{}", server.name());
        session.close().await;
    }
}

#[tokio::test]
async fn db_real_a_cancel_from_a_second_connection_stops_the_statement() {
    if !enabled("db_real_a_cancel_from_a_second_connection_stops_the_statement") {
        return;
    }
    for server in Server::every() {
        let mut session = connect(server).await;
        let canceller = session.canceller();
        // MySQL's SLEEP() is not interrupted by KILL QUERY: it returns as if
        // it had finished. A statement that really works is what a cancel has
        // to stop, so MySQL gets one that does work.
        let sleep = if server.is_postgres() {
            "SELECT pg_sleep(30)".to_owned()
        } else {
            "SELECT count(*) FROM information_schema.columns a, \
             information_schema.columns b, information_schema.columns c"
                .to_owned()
        };
        let started = std::time::Instant::now();
        let stopped = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            canceller.cancel().await
        });
        let error = failure(
            session.query(&sleep, &[], &DbLimits::default()).await,
            &format!("{}: a sleeping statement", server.name()),
        );
        let confirmed = stopped.await.expect("the cancel finished");
        assert!(confirmed, "{}: the cancel did not reach it", server.name());
        assert!(
            started.elapsed() < std::time::Duration::from_secs(20),
            "{}: the statement ran to its end",
            server.name()
        );
        assert!(
            matches!(error, DbError::Failed(DbFailure::Interrupted { .. })),
            "{}: {error} / {:?}",
            server.name(),
            error.server()
        );
        session.close().await;
    }
}

/// What the explorer does with one connection: request after request, each
/// ending with `end_request` (the read guard rolled back) and the next read
/// opening the guard again from the start. A failed statement, an attempt at
/// turning the guard off and a cancelled statement all leave the same
/// connection usable, and a write after each of them is still refused with
/// 25006 -- the guard was opened again, not left to whatever came before.
#[tokio::test]
async fn db_real_one_connection_reissues_the_read_guard_for_every_request() {
    if !enabled("db_real_one_connection_reissues_the_read_guard_for_every_request") {
        return;
    }
    for server in Server::every() {
        let name = server.name();
        let limits = DbLimits::default();
        let mut session = connect(server).await;
        // A write that returns rows, so only the transaction can refuse it.
        let write = if server.is_postgres() {
            "WITH gone AS (DELETE FROM orders RETURNING id) SELECT count(*) FROM gone"
        } else {
            "DELETE FROM orders"
        };
        let refused = |error: &DbError| error.server().is_some_and(|s| s.code == "25006");
        let attempts: &[&str] = if server.is_postgres() {
            // The one way a read could try to turn its own guard off.
            &[
                "SELECT set_config('transaction_read_only', 'off', false)",
                "SELECT no_such_column FROM orders",
            ]
        } else {
            &["SELECT no_such_column FROM orders"]
        };
        for attempt in attempts {
            let _ = session.query(attempt, &[], &limits).await;
            session
                .end_request()
                .await
                .unwrap_or_else(|error| panic!("{name}: ending {attempt}: {error}"));
            let error = failure(
                session.query(write, &[], &limits).await,
                &format!("{name}: a write after {attempt}"),
            );
            assert!(refused(&error), "{name}: after {attempt}: {error}");
            session.end_request().await.expect("the guard rolls back");
        }
        // A cancelled statement leaves the connection usable too.
        let canceller = session.canceller();
        let sleep = if server.is_postgres() {
            "SELECT pg_sleep(30)"
        } else {
            "SELECT count(*) FROM information_schema.columns a, \
             information_schema.columns b, information_schema.columns c"
        };
        let stopped = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            canceller.cancel().await
        });
        let _ = failure(
            session.query(sleep, &[], &limits).await,
            &format!("{name}: a cancelled statement"),
        );
        assert!(stopped.await.expect("the cancel finished"), "{name}");
        session.end_request().await.expect("the guard rolls back");
        session.canceller().rearm();
        let error = failure(
            session.query(write, &[], &limits).await,
            &format!("{name}: a write after a cancel"),
        );
        assert!(refused(&error), "{name}: after a cancel: {error}");
        session.end_request().await.expect("the guard rolls back");
        let left = session
            .query("SELECT count(*) FROM orders", &[], &limits)
            .await
            .unwrap_or_else(|error| panic!("{name}: the connection is still usable: {error}"));
        assert_eq!(
            left.rows[0][0].as_str(),
            Some("4"),
            "{name}: a write got through"
        );
        session.close().await;
    }
}

#[tokio::test]
async fn db_real_execute_rolls_back_commits_and_stops_a_change_that_reaches_too_far() {
    if !enabled("db_real_execute_rolls_back_commits_and_stops_a_change_that_reaches_too_far") {
        return;
    }
    for server in Server::every() {
        let limits = DbLimits::default();
        let count = |server: Server| async move {
            let mut session = connect(server).await;
            let result = session
                .query("SELECT count(*) FROM scratch", &[], &DbLimits::default())
                .await
                .expect("the scratch rows");
            session.close().await;
            result.rows[0][0].as_str().expect("a count").to_owned()
        };
        let statement = |sql: &str| DbStatement {
            sql: sql.to_owned(),
            params: vec![],
        };
        // A table of this test's own, so the shared fixture stays as it is.
        let mut writer = connect(server).await;
        writer
            .execute(
                &[
                    statement("DROP TABLE IF EXISTS scratch"),
                    statement("CREATE TABLE scratch (id integer)"),
                    statement("INSERT INTO scratch VALUES (1), (2), (3)"),
                ],
                &limits,
                true,
                None,
            )
            .await
            .unwrap_or_else(|error| panic!("{}: {error}", server.name()));
        writer.close().await;
        assert_eq!(count(server).await, "3", "{}", server.name());

        // A rollback runs every statement and keeps nothing.
        let mut writer = connect(server).await;
        let (done, outcome) = writer
            .execute(&[statement("DELETE FROM scratch")], &limits, false, None)
            .await
            .expect("a rolled back change");
        writer.close().await;
        assert_eq!(done[0].rows_affected, 3, "{}", server.name());
        assert!(
            matches!(
                outcome,
                kurama::domain::types::database::DbOutcome::RolledBack
            ),
            "{}",
            server.name()
        );
        assert_eq!(
            count(server).await,
            "3",
            "{}: a rollback kept something",
            server.name()
        );

        // A change that reaches further than allowed keeps nothing either.
        let mut writer = connect(server).await;
        let error = failure(
            writer
                .execute(&[statement("DELETE FROM scratch")], &limits, true, Some(2))
                .await,
            "a change past its bound",
        );
        writer.close().await;
        assert!(
            matches!(
                error,
                DbError::Failed(DbFailure::AffectedRowsLimit {
                    rows_affected: 3,
                    max: 2,
                    ..
                })
            ),
            "{}: {error}",
            server.name()
        );
        assert_eq!(count(server).await, "3", "{}", server.name());

        // A stop asked between statements stops no statement, because none
        // is running, and Aurora DSQL can be told of none at all. The change
        // is not committed all the same: the session reads the request.
        let mut writer = connect(server).await;
        let _ = writer.canceller().cancel().await;
        let error = failure(
            writer
                .execute(&[statement("DELETE FROM scratch")], &limits, true, None)
                .await,
            "a change after a stop",
        );
        writer.close().await;
        assert!(
            matches!(error, DbError::Failed(DbFailure::Interrupted { .. })),
            "{}: {error}",
            server.name()
        );
        assert_eq!(
            count(server).await,
            "3",
            "{}: a stopped change was committed",
            server.name()
        );

        // A second statement that fails takes the first one with it.
        let mut writer = connect(server).await;
        let error = failure(
            writer
                .execute(
                    &[
                        statement("DELETE FROM scratch WHERE id = 1"),
                        statement("DELETE FROM nope"),
                    ],
                    &limits,
                    true,
                    None,
                )
                .await,
            "a failing second statement",
        );
        writer.close().await;
        assert!(error.server().is_some(), "{}: {error}", server.name());
        assert_eq!(
            count(server).await,
            "3",
            "{}: the first statement was kept",
            server.name()
        );

        // And a commit keeps what every statement did.
        let mut writer = connect(server).await;
        let (_, outcome) = writer
            .execute(
                &[statement("DELETE FROM scratch WHERE id = 1")],
                &limits,
                true,
                None,
            )
            .await
            .expect("a committed change");
        writer.close().await;
        assert!(
            matches!(
                outcome,
                kurama::domain::types::database::DbOutcome::Committed
            ),
            "{}",
            server.name()
        );
        assert_eq!(count(server).await, "2", "{}", server.name());
    }
}

#[tokio::test]
async fn db_real_refuses_a_wrong_password_and_an_unknown_certificate_authority() {
    if !enabled("db_real_refuses_a_wrong_password_and_an_unknown_certificate_authority") {
        return;
    }
    for server in Server::every() {
        let mut access = server.access();
        access.password = ServerPassword::Fixed("not-the-password".into());
        let error = failure(
            ServerSession::open(access).await,
            &format!("{}: a wrong password", server.name()),
        );
        assert!(
            error.server().is_some(),
            "{}: a refused login is the server's answer, not a tool failure -> {error}",
            server.name()
        );

        // A certificate signed by someone else is not the server this trusts.
        let mut access = server.access();
        let other = if server.is_postgres() {
            "mysql-ca.pem"
        } else {
            "ca.pem"
        };
        access.database.ca_file = Some(tls_dir().join(other));
        let error = failure(
            ServerSession::open(access).await,
            &format!("{}: another certificate authority", server.name()),
        );
        assert!(
            matches!(error, DbError::Unreachable(_)),
            "{}: {error}",
            server.name()
        );
    }
}

/// The tunnel, end to end, with everything but AWS and the plugin itself real.
///
/// A fake SSM answers the two calls kurama makes, `tests/fakes/session-manager-plugin`
/// stands in for the plugin, and a proxy this test runs forwards the port it
/// reports to the PostgreSQL that `db-up` started. What is verified is
/// everything between: the call sequence, that the session token reaches the
/// plugin through the environment and never through its command line, that the
/// port kurama reads is the port it connects to, that a statement really runs
/// through it, and that nothing of it is left behind.
#[tokio::test]
async fn db_real_a_tunnel_carries_the_connection_and_leaves_nothing_behind() {
    if !enabled("db_real_a_tunnel_carries_the_connection_and_leaves_nothing_behind") {
        return;
    }
    use kurama::adapters::aws::ssm_tunnel::{OpenTunnel, TunnelRequest, require_usable_plugin};
    use kurama::adapters::config::InstanceRef;
    use kurama::domain::types::Credentials;
    use wiremock::matchers::{header, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const TOKEN: &str = "SECRET-SESSION-TOKEN";
    let postgres = Server::Postgres17.access().port;

    // A port of this test's own, forwarding to the database the way the
    // bastion would.
    let proxy = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a local port");
    let local = proxy.local_addr().expect("its number").port();
    tokio::spawn(async move {
        while let Ok((mut client, _)) = proxy.accept().await {
            tokio::spawn(async move {
                if let Ok(mut server) =
                    tokio::net::TcpStream::connect(("127.0.0.1", postgres)).await
                {
                    let _ = tokio::io::copy_bidirectional(&mut client, &mut server).await;
                }
            });
        }
    });

    let ssm = MockServer::start().await;
    let answer = |target: &str, body: serde_json::Value| {
        Mock::given(method("POST"))
            .and(header("x-amz-target", target))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
    };
    ssm.register(answer(
        "AmazonSSM.DescribeInstanceInformation",
        serde_json::json!({"InstanceInformationList": [
            {"InstanceId": "i-0123456789abcdef0", "PingStatus": "Online"}
        ]}),
    ))
    .await;
    ssm.register(answer(
        "AmazonSSM.StartSession",
        serde_json::json!({
            "SessionId": "s-kurama", "TokenValue": TOKEN, "StreamUrl": "wss://example.invalid/"
        }),
    ))
    .await;
    ssm.register(answer(
        "AmazonSSM.TerminateSession",
        serde_json::json!({"SessionId": "s-kurama"}),
    ))
    .await;

    let _guard = TUNNEL_ENVIRONMENT.lock().await;
    let directory = tempfile::tempdir().expect("a place for the plugin's log");
    let log = directory.path().join("plugin.log");
    put_the_fake_plugin_on_the_path();
    test_env::set("KURAMA_TEST_SSM_ENDPOINT", ssm.uri());
    test_env::set("KURAMA_FAKE_PLUGIN_PORT", local.to_string());
    test_env::set("KURAMA_FAKE_PLUGIN_LOG", &log);
    test_env::remove("KURAMA_FAKE_PLUGIN_MODE");

    let ready = require_usable_plugin()
        .await
        .expect("the installed plugin is new enough");
    let credentials = Credentials::new(
        "AKIAFAKE".into(),
        "secret".into(),
        Some("token".into()),
        None,
    );
    let tunnel = OpenTunnel::open(
        TunnelRequest {
            credentials: &credentials,
            region: "ap-northeast-1".into(),
            instance: InstanceRef::Name("bastion".into()),
            remote_host: "db.internal.example.com".into(),
            remote_port: 5432,
            connect_timeout_secs: 10,
        },
        ready,
    )
    .await
    .expect("the tunnel opens");
    assert_eq!(
        tunnel.local_port, local,
        "kurama connects to the port the plugin said it opened"
    );
    assert_eq!(tunnel.reported().instance_id, "i-0123456789abcdef0");

    // The plugin was given the response out of band, not on its command line.
    let argv = std::fs::read_to_string(&log).expect("the plugin logged its arguments");
    assert!(
        !argv.contains(TOKEN),
        "the session token was in the plugin's command line: {argv}"
    );
    assert!(
        argv.contains("AWS_SSM_START_SESSION_RESPONSE"),
        "the plugin was told which variable holds the response: {argv}"
    );
    assert!(
        argv.contains("response-in-environment"),
        "the response really was in the plugin's environment: {argv}"
    );

    // A statement really runs through it.
    let mut access = Server::Postgres17.access();
    access.host = "127.0.0.1".into();
    access.port = tunnel.local_port;
    let mut session = ServerSession::open(access)
        .await
        .expect("a connection through the tunnel");
    let result = session
        .query("SELECT count(*) FROM orders", &[], &DbLimits::default())
        .await
        .expect("a statement through the tunnel");
    assert_eq!(result.rows[0][0].as_str(), Some("4"));
    session.close().await;

    // Closing ends the session at AWS and leaves no plugin behind.
    tunnel.close().await;
    let calls: Vec<String> = ssm
        .received_requests()
        .await
        .expect("the calls kurama made")
        .iter()
        .filter_map(|request| {
            request
                .headers
                .get("x-amz-target")
                .and_then(|value| value.to_str().ok())
                .map(|target| target.trim_start_matches("AmazonSSM.").to_owned())
        })
        .collect();
    assert_eq!(
        calls,
        [
            "DescribeInstanceInformation",
            "StartSession",
            "TerminateSession"
        ],
        "the call sequence"
    );
    // The port belongs to this test's proxy, which stands in for what the
    // plugin forwards to, so it says nothing about the plugin; the plugin's
    // own process is what must be gone.
    let pid = argv
        .lines()
        .find_map(|line| line.strip_prefix("pid "))
        .expect("the plugin logged its process id");
    let alive = std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .expect("kill -0")
        .success();
    assert!(!alive, "the plugin (pid {pid}) outlived the call");
}

/// The tunnel through a real bastion, with nothing faked.
///
/// `tests/db/bastion.yaml` creates one; this runs only when
/// `KURAMA_TEST_BASTION` names it and the ambient environment carries AWS
/// credentials for the account it lives in. What it adds over the faked
/// tunnel is the part that is AWS's own: a real Session Manager session and
/// the real plugin's port forwarding.
#[tokio::test]
async fn db_real_a_tunnel_through_a_real_bastion_carries_the_connection() {
    let Ok(bastion) = std::env::var("KURAMA_TEST_BASTION") else {
        eprintln!(
            "db_real_a_tunnel_through_a_real_bastion_carries_the_connection: not run; \
             create one with tests/db/bastion-up.sh and set KURAMA_TEST_BASTION to its name"
        );
        return;
    };
    use kurama::adapters::aws::ssm_tunnel::{OpenTunnel, TunnelRequest, require_usable_plugin};
    use kurama::adapters::config::InstanceRef;
    use kurama::domain::types::Credentials;

    let credentials = Credentials::new(
        std::env::var("AWS_ACCESS_KEY_ID").expect("AWS credentials in the environment"),
        std::env::var("AWS_SECRET_ACCESS_KEY").expect("AWS credentials in the environment"),
        std::env::var("AWS_SESSION_TOKEN").ok(),
        None,
    );
    let ready = require_usable_plugin()
        .await
        .expect("the plugin is installed");
    let tunnel = OpenTunnel::open(
        TunnelRequest {
            credentials: &credentials,
            region: std::env::var("AWS_DEFAULT_REGION").unwrap_or_else(|_| "ap-northeast-1".into()),
            instance: InstanceRef::Name(bastion),
            // The database as the bastion reaches it, which is its own host.
            remote_host: "localhost".into(),
            remote_port: 5432,
            connect_timeout_secs: 30,
        },
        ready,
    )
    .await
    .expect("the tunnel opens through the real bastion");
    assert!(tunnel.local_port > 0);
    assert!(tunnel.reported().instance_id.starts_with("i-"));
    let tunnel_port = tunnel.local_port;

    // The certificate the bastion presents is for `db.test`, and the
    // connection goes to a local port: `verify-ca` is the most a tunnel can
    // check, which is what the configuration rules say.
    let through_the_tunnel = || {
        let mut access = Server::Postgres17.access();
        access.host = "127.0.0.1".into();
        access.port = tunnel.local_port;
        access.username = "postgres".into();
        // The bastion trusts localhost, so no secret travels for this check.
        access.password = ServerPassword::Fixed("unused".into());
        access.database.database = "postgres".into();
        access.database.tls = DbTls::VerifyCa;
        access.database.ca_file = std::env::var("KURAMA_TEST_BASTION_CA")
            .ok()
            .map(PathBuf::from);
        access
    };
    assert!(
        through_the_tunnel().database.ca_file.is_some(),
        "set KURAMA_TEST_BASTION_CA to the certificate authority of the bastion"
    );
    let mut session = ServerSession::open(through_the_tunnel())
        .await
        .expect("a connection through the real tunnel");
    let result = session
        .query("SELECT count(*) FROM orders", &[], &DbLimits::default())
        .await
        .expect("a statement through the real tunnel");
    assert_eq!(result.rows[0][0].as_str(), Some("4"));
    session.close().await;

    // And the rule that a tunnel cannot check a host name is not a guess: the
    // same connection with `verify-full` is refused by the certificate.
    let mut strict = through_the_tunnel();
    strict.database.tls = DbTls::VerifyFull;
    let error = failure(
        ServerSession::open(strict).await,
        "verify-full through a tunnel",
    );
    assert!(
        matches!(error, DbError::Unreachable(_)),
        "a name that cannot match is a connection that cannot be made: {error}"
    );
    tunnel.close().await;

    assert!(
        !tokio::net::TcpStream::connect(("127.0.0.1", tunnel_port))
            .await
            .is_ok(),
        "a plugin outlived the call and is still listening on its port"
    );
}

/// What a tunnel does when it cannot open, which needs no database and no
/// Docker: only the fake plugin and a Session Manager that answers.
///
/// It runs in the gate, unlike everything else in this file. The two failures
/// it covers are the ones a person actually meets -- a plugin that cannot
/// reach AWS, and one that never becomes ready -- and neither was exercised
/// anywhere: the fake's `exit` and `silent` modes existed and no test set
/// them. A hang instead of a classified failure would have gone unnoticed.
#[tokio::test]
async fn db_tunnel_failures_are_classified_and_close_the_session_they_opened() {
    use kurama::adapters::aws::ssm_tunnel::{OpenTunnel, TunnelRequest, require_usable_plugin};
    use kurama::adapters::config::InstanceRef;
    use kurama::domain::types::Credentials;
    use wiremock::matchers::{header, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let _guard = TUNNEL_ENVIRONMENT.lock().await;
    let credentials = Credentials::new(
        "AKIAFAKE".into(),
        "secret".into(),
        Some("token".into()),
        None,
    );

    for (mode, expected) in [
        // The plugin ended. What it said on stderr is the only thing that
        // tells this from an unreachable stream, and the hint for the failure
        // points at a bastion `find_bastion` has already found online.
        ("exit", "Reached maximum retries"),
        // The plugin is alive and never ready: the deadline is what ends it,
        // and a hang here would be a call nobody can interrupt.
        ("silent", "did not open within 1s"),
    ] {
        let ssm = MockServer::start().await;
        let answer = |target: &str, body: serde_json::Value| {
            Mock::given(method("POST"))
                .and(header("x-amz-target", target))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
        };
        ssm.register(answer(
            "AmazonSSM.DescribeInstanceInformation",
            serde_json::json!({"InstanceInformationList": [
                {"InstanceId": "i-0123456789abcdef0", "PingStatus": "Online"}
            ]}),
        ))
        .await;
        ssm.register(answer(
            "AmazonSSM.StartSession",
            serde_json::json!({
                "SessionId": "s-kurama", "TokenValue": "T", "StreamUrl": "wss://example.invalid/"
            }),
        ))
        .await;
        ssm.register(answer(
            "AmazonSSM.TerminateSession",
            serde_json::json!({"SessionId": "s-kurama"}),
        ))
        .await;

        put_the_fake_plugin_on_the_path();
        test_env::set("KURAMA_TEST_SSM_ENDPOINT", ssm.uri());
        test_env::set("KURAMA_FAKE_PLUGIN_MODE", mode);
        test_env::remove("KURAMA_FAKE_PLUGIN_LOG");

        let ready = require_usable_plugin()
            .await
            .expect("the fake plugin is new enough");
        let error = OpenTunnel::open(
            TunnelRequest {
                credentials: &credentials,
                region: "ap-northeast-1".into(),
                instance: InstanceRef::Name("bastion".into()),
                remote_host: "db.internal.example.com".into(),
                remote_port: 5432,
                connect_timeout_secs: 1,
            },
            ready,
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("{mode}: a tunnel that cannot open is a failure"));

        assert!(
            matches!(error, DbError::TunnelUnavailable(_)),
            "{mode}: {error}"
        );
        assert!(
            error.to_string().contains(expected),
            "{mode}: the failure says why: {error}"
        );
        // One line, whatever the plugin wrote.
        assert!(!error.to_string().contains('\n'), "{mode}: {error}");

        // A session that opened and had nothing to carry is still closed.
        let calls: Vec<String> = ssm
            .received_requests()
            .await
            .expect("the calls kurama made")
            .iter()
            .filter_map(|request| {
                request
                    .headers
                    .get("x-amz-target")
                    .and_then(|value| value.to_str().ok())
                    .map(|target| target.trim_start_matches("AmazonSSM.").to_owned())
            })
            .collect();
        assert_eq!(
            calls,
            [
                "DescribeInstanceInformation",
                "StartSession",
                "TerminateSession"
            ],
            "{mode}: the call sequence"
        );
    }
    test_env::remove("KURAMA_FAKE_PLUGIN_MODE");
}

/// `REAL_ENGINES` is what `every()` really runs, so the architecture rule that
/// reads it is reading the truth.
#[test]
fn db_real_engines_names_exactly_the_servers_these_tests_run() {
    let mut covered: Vec<&str> = Server::every().iter().map(|s| s.engine()).collect();
    covered.sort_unstable();
    covered.dedup();
    let mut declared = REAL_ENGINES.to_vec();
    declared.sort_unstable();
    assert_eq!(covered, declared);
}
