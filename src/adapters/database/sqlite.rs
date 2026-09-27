//! One read-only SQLite connection: open the file so nothing can be written
//! through it, prove the caller's SQL is one statement, and stream a bounded
//! result out of it.
use super::rows::{self, Cell, ColumnInfo};
use super::sqlite_statements::{self, PreparedColumn};
use super::{Listing, Page};
use crate::domain::types::database::{
    DbEncoding, DbEngineInfo, DbError, DbFailure, DbLimits, DbOutcome, DbResult, DbStatement,
    DbStatementResult, InvalidDb, Quoting, ServerError, StatementEnd, after_commit,
    quote_identifier,
};
use base64::Engine as _;
use futures_util::TryStreamExt;
use serde_json::Value;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection, SqliteRow};
use sqlx::{AssertSqlSafe, ConnectOptions, Connection, Row, Sqlite, TypeInfo, ValueRef};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// How many SQLite virtual-machine steps run between two cancellation checks.
/// Small enough that a stopped read ends in milliseconds, large enough that the
/// check costs nothing on a short one.
const PROGRESS_STEPS: i32 = 1000;

/// One row of `pragma_table_info`: the name, the declared type, whether the
/// column is NOT NULL, its default, and its place in the primary key.
type TableColumn = (String, Option<String>, i64, Option<String>, i64);

/// One row of `pragma_foreign_key_list`: the column, the table it references
/// and the column there.
type ForeignKey = (String, String, Option<String>);

pub struct SqliteSession {
    connection: SqliteConnection,
    /// Set from outside to end a running statement; the progress handler reads
    /// it. Dropping the future does not stop SQLite, this does.
    cancel: Arc<AtomicBool>,
    version: String,
}

impl SqliteSession {
    /// Open the file read-only. This is the read guard: `INSERT`, `CREATE`,
    /// `ATTACH` of a new file and `PRAGMA journal_mode` are all refused by
    /// SQLite itself, and no caller statement can lift it.
    pub async fn open(path: &Path, limits: &DbLimits, write: bool) -> Result<Self, DbError> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            // Read-only unless this very call is a write the section allows.
            .read_only(!write)
            .create_if_missing(false)
            .busy_timeout(Duration::from_secs(limits.query_timeout_secs))
            .disable_statement_logging();
        let mut connection = options.connect().await.map_err(open_error)?;
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        connection
            .lock_handle()
            .await
            .map_err(driver_error)?
            .set_progress_handler(PROGRESS_STEPS, move || !flag.load(Ordering::Relaxed));
        let version: String = sqlx::query_scalar("SELECT sqlite_version()")
            .fetch_one(&mut connection)
            .await
            .map_err(driver_error)?;
        // A file that is not a database opens; only a statement that reads it
        // fails, and it must not be the caller's.
        sqlx::query("SELECT 1 FROM sqlite_schema LIMIT 1")
            .fetch_optional(&mut connection)
            .await
            .map_err(driver_error)?;
        Ok(Self {
            connection,
            cancel,
            version,
        })
    }

    /// The flag that stops a running statement. A deadline or a SIGINT sets it.
    pub fn cancellation(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    pub fn engine(&self) -> DbEngineInfo {
        DbEngineInfo {
            name: "sqlite".into(),
            server_version: Some(self.version.clone()),
        }
    }

    pub async fn close(self) {
        // The result does not change what the caller was told; a file that
        // cannot be closed cleanly has already given its rows.
        let _ = self.connection.close().await;
    }

    /// The caller's statement. It is one statement because SQLite counted it,
    /// and it returns rows because SQLite prepared it.
    pub async fn query(
        &mut self,
        sql: &str,
        params: &[Option<String>],
        limits: &DbLimits,
    ) -> Result<DbResult, DbError> {
        let columns = self.one_reading_statement(sql).await?;
        let mut query = sqlx::query::<Sqlite>(AssertSqlSafe(sql.to_owned()));
        for param in params {
            query = query.bind(param.clone());
        }
        let stream = query.fetch(&mut self.connection);
        rows::collect(stream.map_err(driver_error), columns, limits, decode_cell).await
    }

    /// The first rows of one table. kurama wrote this statement, so the row
    /// bound is pushed down to SQLite instead of read and thrown away.
    pub async fn preview(
        &mut self,
        schema: Option<&str>,
        table: &str,
        columns: &[String],
        limits: &DbLimits,
    ) -> Result<DbResult, DbError> {
        let sql = super::preview_sql(schema, table, columns, limits.max_rows, Quoting::Ansi);
        let prepared = self.one_reading_statement(&sql).await?;
        let stream = sqlx::query::<Sqlite>(AssertSqlSafe(sql)).fetch(&mut self.connection);
        rows::collect(stream.map_err(driver_error), prepared, limits, decode_cell).await
    }

    /// Every schema the connection can see: `main`, and anything attached.
    pub async fn schemas(&mut self, page: &Page) -> Result<Listing, DbError> {
        let after = page.after.as_ref().map(|(_, name)| name.clone());
        let rows: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT name, file FROM pragma_database_list \
             WHERE name > ?1 ORDER BY name LIMIT ?2",
        )
        .bind(after.unwrap_or_default())
        .bind(page.limit.saturating_add(1) as i64)
        .fetch_all(&mut self.connection)
        .await
        .map_err(driver_error)?;
        let columns = ["schema", "file"];
        let listed = |row: &(String, Option<String>)| {
            vec![
                Value::String(row.0.clone()),
                row.1.clone().map_or(Value::Null, Value::String),
            ]
        };
        Ok(rows::paged(rows, page.limit, &columns, listed, |row| {
            (row.0.clone(), row.0.clone())
        }))
    }

    /// The tables and views of one schema, or of every schema in turn.
    pub async fn tables(&mut self, schema: Option<&str>, page: &Page) -> Result<Listing, DbError> {
        let all = self.schema_names().await?;
        let wanted: Vec<String> = match schema {
            Some(name) => {
                if !all.iter().any(|schema| schema == name) {
                    return Err(DbError::Rejected(ServerError::new(
                        "1",
                        &format!("no such schema: {name}"),
                    )));
                }
                vec![name.to_owned()]
            }
            None => all,
        };
        let (from_schema, after_name) = match &page.after {
            Some((schema, name)) => (Some(schema.clone()), Some(name.clone())),
            None => (None, None),
        };
        let mut rows: Vec<(String, String, String)> = vec![];
        let mut started = from_schema.is_none();
        for schema in wanted {
            if !started {
                if Some(&schema) != from_schema.as_ref() {
                    continue;
                }
                started = true;
            }
            let after = if Some(&schema) == from_schema.as_ref() {
                after_name.clone().unwrap_or_default()
            } else {
                String::new()
            };
            let remaining = page.limit.saturating_add(1).saturating_sub(rows.len());
            if remaining == 0 {
                break;
            }
            let found: Vec<(String, String)> = sqlx::query_as(AssertSqlSafe(format!(
                "SELECT name, type FROM {}.sqlite_schema \
                 WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite@_%' ESCAPE '@' \
                 AND name > ?1 ORDER BY name LIMIT ?2",
                quote_identifier(&schema, Quoting::Ansi)
            )))
            .bind(after)
            .bind(remaining as i64)
            .fetch_all(&mut self.connection)
            .await
            .map_err(driver_error)?;
            rows.extend(
                found
                    .into_iter()
                    .map(|(name, kind)| (schema.clone(), name, kind)),
            );
            if rows.len() > page.limit {
                break;
            }
        }
        let columns = ["schema", "name", "type"];
        let listed = |row: &(String, String, String)| {
            vec![
                Value::String(row.0.clone()),
                Value::String(row.1.clone()),
                Value::String(row.2.clone()),
            ]
        };
        Ok(rows::paged(rows, page.limit, &columns, listed, |row| {
            (row.0.clone(), row.1.clone())
        }))
    }

    /// One table's columns, with the key and the reference each one carries.
    pub async fn describe(
        &mut self,
        schema: Option<&str>,
        table: &str,
    ) -> Result<DbResult, DbError> {
        let schema = schema.unwrap_or("main").to_owned();
        // A catalog answer kurama reads itself is read completely: a display
        // bound must not silently shorten what the columns are joined against.
        let columns: Vec<TableColumn> = sqlx::query_as(
            "SELECT name, type, \"notnull\", dflt_value, pk \
             FROM pragma_table_info(?1, ?2) ORDER BY cid",
        )
        .bind(table)
        .bind(&schema)
        .fetch_all(&mut self.connection)
        .await
        .map_err(driver_error)?;
        if columns.is_empty() {
            return Err(DbError::Rejected(ServerError::new(
                "1",
                &format!("no such table: {table}"),
            )));
        }
        let keys: Vec<ForeignKey> = sqlx::query_as(
            "SELECT \"from\", \"table\", \"to\" FROM pragma_foreign_key_list(?1, ?2)",
        )
        .bind(table)
        .bind(&schema)
        .fetch_all(&mut self.connection)
        .await
        .map_err(driver_error)?;
        let rows = columns
            .iter()
            .map(|(name, data_type, not_null, default, primary_key)| {
                let reference =
                    keys.iter()
                        .find(|(from, _, _)| from == name)
                        .map(|(_, table, to)| match to {
                            Some(to) => format!("{table}.{to}"),
                            None => table.clone(),
                        });
                vec![
                    Value::String(name.clone()),
                    // A column declared without a type reads back as an empty
                    // string; it has no declared type, so it reports none.
                    data_type
                        .clone()
                        .filter(|text| !text.is_empty())
                        .map_or(Value::Null, Value::String),
                    Value::String(bool_text(*not_null == 0)),
                    Value::String(bool_text(default.is_some())),
                    Value::String(bool_text(*primary_key > 0)),
                    reference.map_or(Value::Null, Value::String),
                ]
            })
            .collect();
        Ok(rows::text_result(&super::DESCRIBE_COLUMNS, rows))
    }

    async fn schema_names(&mut self) -> Result<Vec<String>, DbError> {
        sqlx::query_scalar("SELECT name FROM pragma_database_list ORDER BY name")
            .fetch_all(&mut self.connection)
            .await
            .map_err(driver_error)
    }

    /// Refuse anything that is not exactly one statement returning rows,
    /// before it is executed. SQLite runs every statement of a string in one
    /// call, so `SELECT 1; DROP TABLE t` has to be stopped here.
    async fn one_reading_statement(&mut self, sql: &str) -> Result<Vec<ColumnInfo>, DbError> {
        let columns = self.one_statement(sql).await?;
        if columns.is_empty() {
            return Err(InvalidDb::NoColumns.into());
        }
        Ok(columns)
    }

    /// Exactly one statement, whether or not it returns rows. A write may
    /// legitimately return none.
    async fn one_statement(&mut self, sql: &str) -> Result<Vec<ColumnInfo>, DbError> {
        let mut handle = self.connection.lock_handle().await.map_err(driver_error)?;
        let facts = sqlite_statements::inspect(handle.as_raw_handle(), sql)?;
        drop(handle);
        match facts.count {
            0 => Err(InvalidDb::EmptyStatement.into()),
            1 => Ok(facts.columns.into_iter().map(column_info).collect()),
            _ => Err(InvalidDb::MultipleStatements.into()),
        }
    }

    /// Every statement of one change, in one transaction. `BEGIN IMMEDIATE`
    /// takes the write lock at the start rather than partway through, so a
    /// second writer is refused before any of this ran.
    pub async fn execute(
        &mut self,
        statements: &[DbStatement],
        limits: &DbLimits,
        commit: bool,
        max_affected_rows: Option<u64>,
    ) -> Result<(Vec<DbStatementResult>, DbOutcome), DbError> {
        self.run("BEGIN IMMEDIATE").await?;
        let mut done = Vec::with_capacity(statements.len());
        for (index, statement) in statements.iter().enumerate() {
            match self
                .one_change(index, statement, limits, max_affected_rows)
                .await
            {
                Ok(result) => done.push(result),
                Err(error) => {
                    // Whatever went wrong, nothing of this change is kept.
                    let _ = self.run("ROLLBACK").await;
                    return Err(error);
                }
            }
        }
        if !commit {
            self.run("ROLLBACK").await?;
            return Ok((done, DbOutcome::RolledBack));
        }
        match after_commit(self.run("COMMIT").await) {
            Ok(outcome) => Ok((done, outcome)),
            // A refusal kept nothing, but the transaction is still open here.
            Err(error) => {
                if !matches!(error, DbError::Failed(DbFailure::CommitUnknown)) {
                    let _ = self.run("ROLLBACK").await;
                }
                Err(error)
            }
        }
    }

    async fn one_change(
        &mut self,
        index: usize,
        statement: &DbStatement,
        limits: &DbLimits,
        max_affected_rows: Option<u64>,
    ) -> Result<DbStatementResult, DbError> {
        let columns = self.one_statement(&statement.sql).await?;
        let returns_rows = !columns.is_empty();
        let mut query = sqlx::query::<Sqlite>(AssertSqlSafe(statement.sql.clone()));
        for param in &statement.params {
            query = query.bind(param.clone());
        }
        // A statement with no columns reports only what it changed; one with
        // columns is read, and every row it produced is counted even past the
        // display bound, so the row limit below weighs the whole change.
        let (result, rows_affected) = if returns_rows {
            let stream = query.fetch(&mut self.connection);
            rows::collect_counted(stream.map_err(driver_error), columns, limits, decode_cell)
                .await?
        } else {
            let done = query
                .execute(&mut self.connection)
                .await
                .map_err(driver_error)?;
            (rows::text_result(&[], vec![]), done.rows_affected())
        };
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
    async fn run(&mut self, sql: &'static str) -> Result<(), DbError> {
        sqlx::query(sql)
            .execute(&mut self.connection)
            .await
            .map(|_| ())
            .map_err(driver_error)
    }
}

fn bool_text(value: bool) -> String {
    if value { "true" } else { "false" }.to_owned()
}

/// One cell, as the string the database stored. SQLite types values and not
/// columns, so the storage class is read per cell.
fn column_info(column: PreparedColumn) -> ColumnInfo {
    ColumnInfo {
        name: column.name,
        data_type: column.data_type,
    }
}

fn decode_cell(row: &SqliteRow, index: usize, name: &str) -> Result<Cell, DbError> {
    let raw = row.try_get_raw(index).map_err(driver_error)?;
    if raw.is_null() {
        return Ok((Value::Null, DbEncoding::Text));
    }
    let kind = raw.type_info().name().to_owned();
    let unsupported = || DbFailure::UnsupportedType {
        column: name.into(),
        data_type: kind.as_str().into(),
    };
    // A value kurama has a reader for but cannot read is the value's problem,
    // not the connection's. `driver_error` calls everything it cannot classify
    // a lost connection, which SQLite -- running in this process -- does not
    // have: a TEXT column holding bytes that are not UTF-8 is a value SQLite
    // stores and `try_get::<String, _>` refuses, and it was reported as "the
    // database connection ended during the read".
    let unreadable = |_: sqlx::Error| {
        DbError::from(DbFailure::UnreadableValue {
            column: name.into(),
            data_type: kind.as_str().into(),
        })
    };
    let text = match kind.as_str() {
        "INTEGER" => row
            .try_get::<i64, _>(index)
            .map_err(unreadable)?
            .to_string(),
        // `{:?}` keeps the decimal point, so a REAL 1.0 is not reported as the
        // integer 1.
        "REAL" => format!("{:?}", row.try_get::<f64, _>(index).map_err(unreadable)?),
        "TEXT" => row.try_get::<String, _>(index).map_err(unreadable)?,
        "BLOB" => {
            let bytes: Vec<u8> = row.try_get(index).map_err(unreadable)?;
            return Ok((
                Value::String(base64::engine::general_purpose::STANDARD.encode(bytes)),
                DbEncoding::Base64,
            ));
        }
        _ => return Err(unsupported().into()),
    };
    Ok((Value::String(text), DbEncoding::Text))
}

/// The SQLite result code of a driver error, when it came from the database.
fn code_of(error: &sqlx::Error) -> Option<String> {
    match error {
        sqlx::Error::Database(database) => database.code().map(|code| code.into_owned()),
        _ => None,
    }
}

/// A failure to open the file. Nothing was sent, so nothing was read: for a
/// file database every one of them is the same answer, that it could not be
/// opened.
fn open_error(error: sqlx::Error) -> DbError {
    match code_of(&error).as_deref() {
        Some("26") => InvalidDb::NotADatabase.into(),
        // SQLITE_CANTOPEN and everything else: no such file, no permission,
        // or a path that is not a file kurama may read.
        _ => InvalidDb::FileUnavailable.into(),
    }
}

/// A failure while the connection was open. The database's own words are
/// repeated for the caller and never logged.
fn driver_error(error: sqlx::Error) -> DbError {
    if let Some(code) = code_of(&error) {
        // SQLITE_INTERRUPT: the progress handler stopped the statement, which
        // only happens when the deadline or a signal asked it to.
        if code == "9" {
            // SQLite stops in the process kurama is in: it either stopped or
            // it never got the request.
            return DbFailure::Interrupted {
                end: StatementEnd::Stopped,
            }
            .into();
        }
        // SQLITE_BUSY: another process held the write lock for the whole wait.
        if code == "5" {
            return DbFailure::Locked.into();
        }
        // SQLITE_NOTADB: the file opens and holds something else. It surfaces
        // at the first statement that reads it, whichever one that is.
        if code == "26" {
            return InvalidDb::NotADatabase.into();
        }
        let message = match &error {
            sqlx::Error::Database(database) => database.message().to_owned(),
            other => other.to_string(),
        };
        return DbError::Rejected(ServerError::new(code, &message));
    }
    // The connection was open, so whatever ended it ended a read.
    DbFailure::Disconnected.into()
}

#[cfg(test)]
#[path = "sqlite_tests.rs"]
mod tests;
