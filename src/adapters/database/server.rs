//! One connection to a PostgreSQL or MySQL server, for one statement.
//!
//! The read guard is a read-only transaction, the statement is prepared so the
//! server refuses a second one, and stopping it needs a second connection:
//! dropping the future leaves the server running the statement.
use super::decode::DecodeError;
use super::rows::{self, Cell, ColumnInfo};
pub use super::server_access::{ServerAccess, ServerPassword};
use crate::adapters::config::DbEngine;
use crate::domain::types::database::{
    DbEncoding, DbEngineInfo, DbError, DbFailure, DbLimits, DbOutcome, DbResult, DbStatement,
    DbStatementResult, DbTransaction, InvalidDb, Quoting, ServerError, StatementEnd, after_commit,
};
use futures_util::TryStreamExt;
use serde_json::Value;
use sqlx::mysql::{MySqlConnection, MySqlRow};
use sqlx::postgres::{PgConnection, PgRow};
use sqlx::{
    AssertSqlSafe, Column, Connection, Executor, Row, SqlSafeStr, Statement, TypeInfo, ValueRef,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) enum Wire {
    Postgres(Box<PgConnection>),
    MySql(Box<MySqlConnection>),
}

/// One body for both engines, where the statement and what is done with the
/// answer do not differ: `$connection` is the engine's connection and, in the
/// second form, `$db` its sqlx database and `$cell` its value decoder. Where
/// the engines differ the arms are written out.
macro_rules! on_either_engine {
    ($wire:expr, |$connection:ident| $body:expr) => {
        match $wire {
            Wire::Postgres($connection) => $body,
            Wire::MySql($connection) => $body,
        }
    };
    ($wire:expr, |$connection:ident, $db:ident, $cell:ident| $body:expr) => {
        match $wire {
            Wire::Postgres($connection) => {
                type $db = sqlx::Postgres;
                let $cell = postgres_cell;
                $body
            }
            Wire::MySql($connection) => {
                type $db = sqlx::MySql;
                let $cell = mysql_cell;
                $body
            }
        }
    };
}
pub(super) use on_either_engine;

impl Wire {
    /// A connection that cannot be closed cleanly has already given its rows,
    /// and the server drops it when the socket does.
    async fn close(self) {
        on_either_engine!(self, |connection| {
            let _ = connection.close().await;
        })
    }
}

pub struct ServerSession {
    pub(super) wire: Wire,
    pub(super) access: ServerAccess,
    version: String,
    /// The backend process (PostgreSQL) or connection (MySQL) running this
    /// session's statement, so another connection can stop it. Aurora DSQL
    /// has neither `pg_backend_pid` nor `pg_cancel_backend`, so it has none.
    backend: Option<i64>,
    /// Set once the read guard is open, so the session knows what to roll back.
    guarded: bool,
    /// Set by the `Canceller` when the caller asked to stop. A server stops
    /// the statement it is running and knows nothing of the next one, and
    /// Aurora DSQL stops none, so the session reads this before each
    /// statement of a change and before its COMMIT.
    stop_asked: Arc<AtomicBool>,
}

/// Stops the statement of a session that is busy running it. It holds its own
/// copy of the access, because the session itself is borrowed by the statement.
#[derive(Clone)]
pub struct Canceller {
    access: ServerAccess,
    backend: Option<i64>,
    stop_asked: Arc<AtomicBool>,
}

impl ServerSession {
    pub async fn open(access: ServerAccess) -> Result<Self, DbError> {
        let mut wire = access.connect().await?;
        let engine = access.database.engine;
        let (version, backend) = match &mut wire {
            Wire::Postgres(connection) => {
                let version: String = sqlx::query_scalar("SHOW server_version")
                    .fetch_one(&mut **connection)
                    .await
                    .map_err(|error| driver_error(engine, error))?;
                let backend = if reads_backend_pid(access.database.aurora_dsql) {
                    let backend: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                        .fetch_one(&mut **connection)
                        .await
                        .map_err(|error| driver_error(engine, error))?;
                    Some(backend as i64)
                } else {
                    None
                };
                (version, backend)
            }
            Wire::MySql(connection) => {
                let version: String = sqlx::query_scalar("SELECT VERSION()")
                    .fetch_one(&mut **connection)
                    .await
                    .map_err(|error| driver_error(engine, error))?;
                let backend: u64 = sqlx::query_scalar("SELECT CONNECTION_ID()")
                    .fetch_one(&mut **connection)
                    .await
                    .map_err(|error| driver_error(engine, error))?;
                (version, Some(backend as i64))
            }
        };
        Ok(Self {
            wire,
            access,
            version,
            backend,
            guarded: false,
            stop_asked: Arc::default(),
        })
    }

    pub fn engine(&self) -> DbEngineInfo {
        DbEngineInfo {
            name: self.access.database.engine.as_str().to_owned(),
            server_version: Some(self.version.clone()),
        }
    }

    /// How this engine writes an identifier kurama puts into SQL.
    fn quoting(&self) -> Quoting {
        quoting_of(self.access.database.engine)
    }

    pub fn canceller(&self) -> Canceller {
        Canceller {
            access: self.access.clone(),
            backend: self.backend,
            stop_asked: self.stop_asked.clone(),
        }
    }

    pub fn transaction(&self) -> DbTransaction {
        DbTransaction {
            read_only: true,
            isolation: Some(
                read_isolation(
                    self.access.database.engine,
                    self.access.database.aurora_dsql,
                )
                .to_owned(),
            ),
            outcome: if self.guarded {
                DbOutcome::RolledBack
            } else {
                DbOutcome::NotStarted
            },
        }
    }

    pub async fn close(self) {
        self.wire.close().await;
    }

    /// Roll back the read guard of the request that just ran. The next read
    /// opens it again from the start, `SET SESSION TRANSACTION READ ONLY`
    /// included, so a statement that turned it off leaves nothing behind.
    pub async fn end_request(&mut self) -> Result<(), DbError> {
        if !self.guarded {
            return Ok(());
        }
        self.run("ROLLBACK".to_owned()).await?;
        self.guarded = false;
        Ok(())
    }

    /// Open the read guard. One guard covers exactly one statement of the
    /// caller's: a statement can turn the guard off, and there is no second
    /// one in this connection for it to profit from.
    async fn guard(&mut self, limits: &DbLimits) -> Result<(), DbError> {
        let millis = server_limit_millis(limits.query_timeout_secs);
        let engine = self.access.database.engine;
        let timeout = postgres_statement_timeout(self.access.database.aurora_dsql, millis);
        match &mut self.wire {
            Wire::Postgres(connection) => {
                for statement in std::iter::once("BEGIN READ ONLY".to_owned()).chain(timeout) {
                    connection
                        .execute(AssertSqlSafe(statement))
                        .await
                        .map_err(|error| driver_error(engine, error))?;
                }
            }
            Wire::MySql(connection) => {
                // A transaction alone does not stop DDL in MySQL: DDL commits
                // implicitly and then runs read-write. The session setting is
                // what refuses CREATE, DROP, TRUNCATE and INSERT.
                for statement in [
                    "SET SESSION TRANSACTION READ ONLY".to_owned(),
                    format!("SET SESSION max_execution_time = {millis}"),
                    "START TRANSACTION READ ONLY".to_owned(),
                ] {
                    connection
                        .execute(AssertSqlSafe(statement))
                        .await
                        .map_err(|error| driver_error(engine, error))?;
                }
            }
        }
        self.guarded = true;
        Ok(())
    }

    /// The caller's statement, inside the guard. A second statement is refused
    /// by the server at prepare time, which is why nothing splits it here.
    pub async fn query(
        &mut self,
        sql: &str,
        params: &[Option<String>],
        limits: &DbLimits,
    ) -> Result<DbResult, DbError> {
        self.guard(limits).await?;
        self.read(sql.to_owned(), params, limits).await
    }

    pub async fn preview(
        &mut self,
        schema: Option<&str>,
        table: &str,
        columns: &[String],
        limits: &DbLimits,
    ) -> Result<DbResult, DbError> {
        let sql = super::preview_sql(schema, table, columns, limits.max_rows, self.quoting());
        self.guard(limits).await?;
        self.read(sql, &[], limits).await
    }

    async fn read(
        &mut self,
        sql: String,
        params: &[Option<String>],
        limits: &DbLimits,
    ) -> Result<DbResult, DbError> {
        let engine = self.access.database.engine;
        let columns = self.prepare(&sql, params.len()).await?;
        if columns.is_empty() {
            return Err(InvalidDb::NoColumns.into());
        }
        on_either_engine!(&mut self.wire, |connection, Db, cell| {
            let mut query = sqlx::query::<Db>(AssertSqlSafe(sql));
            for param in params {
                query = query.bind(param.clone());
            }
            let stream = query.fetch(&mut **connection);
            rows::collect(
                stream.map_err(move |error| driver_error(engine, error)),
                columns,
                limits,
                cell,
            )
            .await
        })
    }

    /// Prepare the caller's statement and report the columns it returns.
    ///
    /// This is the only place a statement is prepared, and it always declares
    /// the parameters as text, because that is what they are. Preparing
    /// without them lets PostgreSQL infer a type from where the parameter is
    /// used -- `WHERE id = $1` becomes an integer -- and the text that then
    /// arrives is read as that type's binary form and refused as a malformed
    /// message. With the declaration the mismatch is what it really is,
    /// `operator does not exist: integer = text`, which names the missing cast.
    ///
    /// Preparing is also the guard against a second statement: the server
    /// refuses a string that is two, here, before any of it runs.
    async fn prepare(&mut self, sql: &str, parameters: usize) -> Result<Vec<ColumnInfo>, DbError> {
        let engine = self.access.database.engine;
        let sql = AssertSqlSafe(sql.to_owned()).into_sql_str();
        match &mut self.wire {
            Wire::Postgres(connection) => {
                let text = <String as sqlx::Type<sqlx::Postgres>>::type_info();
                let prepared = <&mut PgConnection as Executor>::prepare_with(
                    &mut **connection,
                    sql,
                    &vec![text; parameters],
                )
                .await
                .map_err(|error| driver_error(engine, error))?;
                Ok(statement_columns(prepared.columns()))
            }
            Wire::MySql(connection) => {
                // MySQL infers nothing from where a parameter is used, so it
                // needs no declaration of their types.
                let prepared = <&mut MySqlConnection as Executor>::prepare(&mut **connection, sql)
                    .await
                    .map_err(|error| driver_error(engine, error))?;
                Ok(statement_columns(prepared.columns()))
            }
        }
    }

    /// Every statement of one change, in one read-write transaction.
    pub async fn execute(
        &mut self,
        statements: &[DbStatement],
        limits: &DbLimits,
        commit: bool,
        max_affected_rows: Option<u64>,
    ) -> Result<(Vec<DbStatementResult>, DbOutcome), DbError> {
        let millis = server_limit_millis(limits.query_timeout_secs);
        let start: &[String] = &match self.access.database.engine {
            DbEngine::Postgresql => std::iter::once("BEGIN".to_owned())
                .chain(postgres_statement_timeout(
                    self.access.database.aurora_dsql,
                    millis,
                ))
                .collect(),
            // `max_execution_time` bounds read-only SELECTs only, so it is
            // not sent here: it would read as a guard on the write and be
            // none. What a long write really waits for is a lock, and that
            // has its own bound. Ending the statement itself is the client
            // deadline and the cancel from the second connection.
            DbEngine::Mysql | DbEngine::Sqlite => vec![
                format!(
                    "SET SESSION innodb_lock_wait_timeout = {}",
                    limits.query_timeout_secs.max(1)
                ),
                "START TRANSACTION".to_owned(),
            ],
        };
        for statement in start {
            self.run(statement.clone()).await?;
        }
        self.guarded = true;
        let mut done = Vec::with_capacity(statements.len());
        for (index, statement) in statements.iter().enumerate() {
            let changed = match self.refuse_after_a_stop() {
                Ok(()) => {
                    self.one_change(index, statement, limits, max_affected_rows)
                        .await
                }
                Err(stopped) => Err(stopped),
            };
            match changed {
                Ok(result) => done.push(result),
                Err(error) => {
                    // Whatever went wrong, nothing of this change is kept.
                    let _ = self.run("ROLLBACK".to_owned()).await;
                    return Err(error);
                }
            }
        }
        if !commit {
            self.run("ROLLBACK".to_owned()).await?;
            return Ok((done, DbOutcome::RolledBack));
        }
        // The last moment a stop can still mean "keep nothing".
        if let Err(stopped) = self.refuse_after_a_stop() {
            let _ = self.run("ROLLBACK".to_owned()).await;
            return Err(stopped);
        }
        match after_commit(self.run("COMMIT".to_owned()).await) {
            Ok(outcome) => Ok((done, outcome)),
            Err(error) => {
                if !matches!(error, DbError::Failed(DbFailure::CommitUnknown)) {
                    let _ = self.run("ROLLBACK".to_owned()).await;
                }
                Err(error)
            }
        }
    }

    /// A caller that asked to stop gets no further statement and no COMMIT,
    /// whether or not the server could be told: the caller rolls back, so
    /// nothing is running and nothing is kept.
    ///
    /// The statement that ran before this is over, and nothing here says the
    /// database stopped it -- on Aurora DSQL nothing could -- so what the
    /// failure names is the rollback that follows it.
    fn refuse_after_a_stop(&self) -> Result<(), DbError> {
        if stop_was_asked(&self.stop_asked) {
            return Err(DbFailure::Interrupted {
                end: StatementEnd::RolledBack,
            }
            .into());
        }
        Ok(())
    }

    async fn one_change(
        &mut self,
        index: usize,
        statement: &DbStatement,
        limits: &DbLimits,
        max_affected_rows: Option<u64>,
    ) -> Result<DbStatementResult, DbError> {
        let engine = self.access.database.engine;
        let columns = self.prepare(&statement.sql, statement.params.len()).await?;
        // A write that returns nothing is the ordinary case, so no column is
        // not a refusal here.
        let returns_rows = !columns.is_empty();
        let sql = statement.sql.clone();
        let (result, rows_affected) = on_either_engine!(&mut self.wire, |connection, Db, cell| {
            let mut query = sqlx::query::<Db>(AssertSqlSafe(sql));
            for param in &statement.params {
                query = query.bind(param.clone());
            }
            if returns_rows {
                let stream = query.fetch(&mut **connection);
                rows::collect_counted(
                    stream.map_err(move |error| driver_error(engine, error)),
                    columns,
                    limits,
                    cell,
                )
                .await?
            } else {
                let done = query
                    .execute(&mut **connection)
                    .await
                    .map_err(|error| driver_error(engine, error))?;
                (rows::text_result(&[], vec![]), done.rows_affected())
            }
        });
        if let Some(max) = max_affected_rows
            && rows_affected > max
        {
            return Err(DbFailure::AffectedRowsLimit {
                index,
                rows_affected,
                max,
            }
            .into());
        }
        Ok(DbStatementResult {
            index,
            rows_affected,
            result: returns_rows.then_some(result),
        })
    }

    /// One statement kurama wrote itself, with nothing to return.
    async fn run(&mut self, sql: String) -> Result<(), DbError> {
        let engine = self.access.database.engine;
        on_either_engine!(&mut self.wire, |connection| connection
            .execute(AssertSqlSafe(sql))
            .await
            .map(|_| ())
            .map_err(|error| driver_error(engine, error)))
    }
}

impl Canceller {
    /// Forget an earlier stop, before the session runs its next request.
    pub fn rearm(&self) {
        self.stop_asked.store(false, Ordering::Relaxed);
    }

    /// Stop the statement from a second connection. Dropping the first future
    /// does not reach the server; this does, or says it did not.
    pub async fn cancel(&self) -> bool {
        // First, and whatever follows: the session sends nothing more.
        self.stop_asked.store(true, Ordering::Relaxed);
        // Nothing to name the statement by is nothing to stop it with, and
        // saying so is the answer: the caller reports a stop nobody confirmed.
        let Some(backend) = self.backend else {
            return false;
        };
        let Ok(mut wire) = self.access.connect().await else {
            return false;
        };
        let stopped = match &mut wire {
            // `false` is PostgreSQL saying it could not signal the backend:
            // the statement ran, and the stop did not happen.
            Wire::Postgres(connection) => {
                sqlx::query_scalar::<_, bool>("SELECT pg_cancel_backend($1)")
                    .bind(backend as i32)
                    .fetch_one(&mut **connection)
                    .await
                    .is_ok_and(|signalled| signalled)
            }
            Wire::MySql(connection) => sqlx::query(AssertSqlSafe(format!("KILL QUERY {backend}")))
                .execute(&mut **connection)
                .await
                .is_ok(),
        };
        wire.close().await;
        stopped
    }
}

fn stop_was_asked(flag: &AtomicBool) -> bool {
    flag.load(Ordering::Relaxed)
}

/// Whether the session can name its backend for a later cancel. Aurora DSQL
/// has neither `pg_backend_pid` nor `pg_cancel_backend` (0A000 for each).
fn reads_backend_pid(aurora_dsql: bool) -> bool {
    !aurora_dsql
}

/// The isolation level the read guard runs at, which the envelope reports.
fn read_isolation(engine: DbEngine, aurora_dsql: bool) -> &'static str {
    match engine {
        // The one level Aurora DSQL has.
        DbEngine::Postgresql if aurora_dsql => "repeatable read",
        DbEngine::Postgresql => "read committed",
        DbEngine::Mysql | DbEngine::Sqlite => "repeatable read",
    }
}

/// The server's own limit on a statement, in milliseconds: a backstop for a
/// client that went away, one second after the client's deadline so that the
/// deadline is what ends a statement, and a cancel is what stops it.
fn server_limit_millis(timeout_secs: u64) -> u64 {
    timeout_secs.saturating_add(1).saturating_mul(1000)
}

/// The server-side deadline of one PostgreSQL transaction. Aurora DSQL refuses
/// the setting (0A000), which would fail every call before its statement ran.
fn postgres_statement_timeout(aurora_dsql: bool, millis: u64) -> Option<String> {
    (!aurora_dsql).then(|| format!("SET LOCAL statement_timeout = {millis}"))
}

/// How this engine writes an identifier kurama puts into SQL.
///
/// A free function so the gate can check it: MySQL had PostgreSQL's quote for
/// a while, which made `--preview` a syntax error on every MySQL table, and
/// only a real server said so -- a test that needs Docker, outside the gate.
fn quoting_of(engine: DbEngine) -> Quoting {
    // No catch-all: an engine added here has to say which quote it reads,
    // because the wrong one is a syntax error.
    match engine {
        DbEngine::Mysql => Quoting::MySql,
        DbEngine::Postgresql | DbEngine::Sqlite => Quoting::Ansi,
    }
}

/// The columns of a prepared statement, for a write, where returning none is
/// the ordinary case.
fn statement_columns<C: Column>(columns: &[C]) -> Vec<ColumnInfo> {
    columns
        .iter()
        .map(|column| ColumnInfo {
            name: column.name().to_owned(),
            data_type: Some(column.type_info().name().to_owned()),
        })
        .collect()
}

fn postgres_cell(row: &PgRow, index: usize, name: &str) -> Result<Cell, DbError> {
    let raw = row
        .try_get_raw(index)
        .map_err(|error| driver_error(DbEngine::Postgresql, error))?;
    if raw.is_null() {
        return Ok((Value::Null, DbEncoding::Text));
    }
    let type_name = raw.type_info().name().to_owned();
    let bytes = raw
        .as_bytes()
        .map_err(|_| DecodeError::Unreadable.named(name, &type_name))?;
    super::postgres_decode::decode(&type_name, bytes)
        .map(|(text, encoding)| (Value::String(text), encoding))
        .map_err(|error| error.named(name, &type_name))
}

fn mysql_cell(row: &MySqlRow, index: usize, name: &str) -> Result<Cell, DbError> {
    let raw = row
        .try_get_raw(index)
        .map_err(|error| driver_error(DbEngine::Mysql, error))?;
    if raw.is_null() {
        return Ok((Value::Null, DbEncoding::Text));
    }
    let type_name = raw.type_info().name().to_owned();
    // The raw bytes are not a public API on MySQL, but an unchecked decode to
    // `Vec<u8>` skips the type check and hands them over unchanged.
    let bytes: Vec<u8> = row
        .try_get_unchecked(index)
        .map_err(|_| DecodeError::Unreadable.named(name, &type_name))?;
    super::mysql_decode::decode(&type_name, &bytes)
        .map(|(text, encoding)| (Value::String(text), encoding))
        .map_err(|error| error.named(name, &type_name))
}

/// A failure while the connection was open.
pub(super) fn driver_error(engine: DbEngine, error: sqlx::Error) -> DbError {
    if let sqlx::Error::Database(database) = &error {
        let code = database
            .code()
            .map(|code| code.into_owned())
            .unwrap_or_default();
        // The statement was stopped: by `statement_timeout`, by
        // `max_execution_time`, or by the cancel from the second connection.
        //
        // PostgreSQL names that in SQLSTATE (57014), and so does MySQL for a
        // KILL (70100). But the SQLSTATE of a `max_execution_time` stop is the
        // catch-all HY000, which means everything else too, so that one is
        // only knowable from the error number the driver keeps beside it.
        // `code()` is SQLSTATE for both engines: matching 1317 or 3024 there
        // matched nothing at all.
        let stopped_by_mysql = database
            .try_downcast_ref::<sqlx::mysql::MySqlDatabaseError>()
            .is_some_and(|mysql| matches!(mysql.number(), 1317 | 3024));
        if matches!(code.as_str(), "57014" | "70100") || stopped_by_mysql {
            // The server is the one saying it, so this is the one end that is
            // confirmed.
            return DbFailure::Interrupted {
                end: StatementEnd::Stopped,
            }
            .into();
        }
        return DbError::Rejected(ServerError::new(code, database.message()));
    }
    let _ = engine;
    DbFailure::Disconnected.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::config::DbAuth;
    use crate::adapters::config::{DbTls, ServerDatabase};
    use crate::adapters::database::qualified;
    use crate::adapters::database::server_access::takes_cleartext_token;
    use crate::domain::types::SecretRef;
    use std::time::Duration;

    fn access(host: &str) -> ServerAccess {
        ServerAccess {
            database: ServerDatabase {
                engine: DbEngine::Postgresql,
                host: host.to_owned(),
                // Nothing listens here, so a test that connects fails.
                port: 1,
                database: "postgres".into(),
                username: SecretRef::Literal("admin".into()),
                auth: DbAuth::Password(SecretRef::OnePassword("op://Agent/db/password".into())),
                tls: DbTls::VerifyFull,
                ca_file: None,
                connect_timeout_secs: 1,
                allow_write: false,
                tunnel: None,
                limits: DbLimits::default(),
                aurora_dsql: crate::domain::functions::signing_target::names_aurora_dsql(host),
            },
            host: "127.0.0.1".into(),
            port: 1,
            username: "admin".into(),
            password: ServerPassword::Fixed("unused".into()),
        }
    }

    /// The server's own limit is a backstop for a client that went away. Were
    /// it the same as `--timeout`, it would race the client deadline: the
    /// engine would stop the statement first, no cancel would be sent, the
    /// failure would read as an interrupt, and MySQL's `SLEEP()` would answer
    /// with a row. One second later, the deadline is always the one that ends it.
    #[test]
    fn db_server_limit_ends_after_the_client_deadline() {
        assert_eq!(server_limit_millis(1), 2000);
        assert_eq!(server_limit_millis(86_400), 86_401_000);
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/adapters/database/server.rs"),
        )
        .expect("this file");
        let production = &source[..source.find("#[cfg(test)]").expect("the tests")];
        // Both the read guard and a change take the limit from it.
        assert_eq!(
            production
                .matches("server_limit_millis(limits.query_timeout_secs)")
                .count(),
            2,
            "every server limit is set through server_limit_millis"
        );
        assert!(!production.contains("query_timeout_secs.saturating_mul(1000)"));
    }

    /// Measured against a real cluster on 2026-09-21: Aurora DSQL answers
    /// 0A000 to `SET LOCAL statement_timeout`, `pg_backend_pid()` and
    /// `pg_cancel_backend()`, and the first of them failed every call.
    #[test]
    fn db_server_sends_aurora_dsql_no_setting_it_refuses() {
        assert_eq!(postgres_statement_timeout(true, 2000), None);
        assert_eq!(
            postgres_statement_timeout(false, 2000).as_deref(),
            Some("SET LOCAL statement_timeout = 2000")
        );
        assert!(!reads_backend_pid(true));
        assert!(reads_backend_pid(false));
        // The one level DSQL has, and what the envelope says of the others.
        assert_eq!(
            read_isolation(DbEngine::Postgresql, true),
            "repeatable read"
        );
        assert_eq!(
            read_isolation(DbEngine::Postgresql, false),
            "read committed"
        );
        assert_eq!(read_isolation(DbEngine::Mysql, false), "repeatable read");
    }

    /// The cleartext plugin hands the password to whoever asks for it, so it
    /// is on for the one user that has to send a token that way.
    #[test]
    fn db_server_offers_the_cleartext_plugin_to_an_iam_user_only() {
        let iam = DbAuth::Iam(crate::adapters::config::IamSection {
            aws_profile: "dev".into(),
            region: None,
        });
        assert!(takes_cleartext_token(&iam));
        assert!(!takes_cleartext_token(&access("db").database.auth));
    }

    /// A stop the server cannot be told about -- Aurora DSQL has no cancel,
    /// and a cancel between two statements stops neither -- still has to keep
    /// the change from being committed. The session reads the flag the
    /// canceller sets; a real server shows it in `tests/real_db.rs`, and this
    /// keeps the check in front of every statement and of the COMMIT.
    #[tokio::test]
    async fn db_server_asked_to_stop_sends_no_further_statement_and_no_commit() {
        let canceller = Canceller {
            access: access("abcdefghijklmnopqrst.dsql.ap-northeast-1.on.aws"),
            backend: None,
            stop_asked: Arc::default(),
        };
        assert!(!stop_was_asked(&canceller.stop_asked));
        assert!(!canceller.cancel().await, "nothing confirmed the stop");
        assert!(stop_was_asked(&canceller.stop_asked));
        // The explorer keeps the session, so the next request forgets it.
        canceller.rearm();
        assert!(!stop_was_asked(&canceller.stop_asked));

        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/adapters/database/server.rs"),
        )
        .expect("this file");
        let body = source
            .split("pub async fn execute(")
            .nth(1)
            .expect("ServerSession::execute");
        let body = &body[..body.find("\n    }\n").expect("the end of execute")];
        let position = |text: &str| body.find(text).unwrap_or_else(|| panic!("{text}"));
        // Once in front of each statement, and once more between the last
        // of them and the COMMIT.
        let (statements, commit) = body.split_at(position(".one_change("));
        assert!(statements.contains("refuse_after_a_stop"), "{body}");
        let before_commit = &commit[..commit.find("\"COMMIT\"").expect("a COMMIT")];
        assert!(
            before_commit.contains("refuse_after_a_stop"),
            "COMMIT is sent only after the stop flag was read:\n{body}"
        );
    }

    #[tokio::test]
    async fn db_server_without_a_backend_to_name_opens_no_connection_to_stop_it() {
        // A listener, so an attempt to connect is seen instead of refused:
        // a refused connection answers `false` as well.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut access = access("abcdefghijklmnopqrst.dsql.ap-northeast-1.on.aws");
        access.port = listener.local_addr().unwrap().port();
        let named = Canceller {
            access: access.clone(),
            backend: Some(1),
            stop_asked: Arc::default(),
        };
        let unnamed = Canceller {
            access,
            backend: None,
            stop_asked: Arc::default(),
        };
        async fn knocked(listener: &tokio::net::TcpListener) -> bool {
            tokio::time::timeout(Duration::from_millis(200), listener.accept())
                .await
                .is_ok()
        }
        assert!(!unnamed.cancel().await);
        assert!(!knocked(&listener).await, "it connected to stop nothing");
        // The same listener does see the canceller that has a backend to name.
        // Its connection is dropped at once, which is a stop nobody confirmed.
        let (stopped, seen) = tokio::join!(named.cancel(), async {
            let seen = knocked(&listener).await;
            drop(listener);
            seen
        });
        assert!(!stopped && seen);
    }

    /// What the two engines disagree about in the SQL kurama writes, checked
    /// where no Docker is needed.
    ///
    /// `tests/real_db.rs` covers these against real servers and is outside the
    /// gate, so a regression in either of them passed `cargo xtask check`.
    #[test]
    fn db_server_quotes_each_engine_the_way_it_reads_identifiers() {
        assert_eq!(quoting_of(DbEngine::Mysql), Quoting::MySql);
        assert_eq!(quoting_of(DbEngine::Postgresql), Quoting::Ansi);
        assert_eq!(
            qualified(Some("app"), "orders", quoting_of(DbEngine::Mysql)),
            "`app`.`orders`"
        );
        assert_eq!(
            qualified(Some("public"), "orders", quoting_of(DbEngine::Postgresql)),
            "\"public\".\"orders\""
        );
        assert_eq!(
            qualified(None, "orders", quoting_of(DbEngine::Mysql)),
            "`orders`"
        );
        // The quote of each engine closes only when it is doubled.
        assert_eq!(
            qualified(None, "a`; DROP TABLE t --", quoting_of(DbEngine::Mysql)),
            "`a``; DROP TABLE t --`"
        );
    }

    /// A prepared statement declares its parameters as text. Preparing a
    /// second time, to learn the column names, dropped those declarations:
    /// PostgreSQL then inferred each parameter's type from where it was used
    /// and refused the text that arrived. `tests/architecture/` keeps the
    /// preparing in one place; this keeps that one place declaring.
    #[test]
    fn db_server_declares_every_parameter_it_prepares() {
        let prepare = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/adapters/database/server.rs"),
        )
        .expect("this file");
        let body = prepare
            .split("async fn prepare(")
            .nth(1)
            .expect("ServerSession::prepare");
        let body = &body[..body.find("\n    }\n").expect("the end of prepare")];
        assert!(
            body.contains("parameters") && body.contains("text"),
            "prepare declares the parameter types it sends:\n{body}"
        );
    }
}
