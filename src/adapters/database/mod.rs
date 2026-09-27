//! The database drivers, one engine per file, bundled behind one session type.
//!
//! There is no port trait here: `commands/db.rs` is the only caller, the
//! scenarios exercise it through the binary, and nothing mocks a connection.
pub mod decode;
pub mod mysql_decode;
pub mod postgres_decode;
pub mod rows;
pub mod server;
mod server_access;
mod server_catalog;
pub mod sqlite;
pub mod sqlite_statements;

/// One page of a catalog listing.
pub struct Page {
    /// The `(schema, name)` the previous page ended on.
    pub after: Option<(String, String)>,
    pub limit: usize,
}

/// The header of `describe` on every engine.
const DESCRIBE_COLUMNS: [&str; 6] = [
    "column",
    "declared_type",
    "nullable",
    "has_default",
    "primary_key",
    "references",
];

/// The first rows of one table, and one past `max_rows`, which tells the
/// caller the table has more. kurama writes this statement, so the bound is
/// the server's to apply.
fn preview_sql(
    schema: Option<&str>,
    table: &str,
    columns: &[String],
    max_rows: usize,
    quoting: Quoting,
) -> String {
    let selection = if columns.is_empty() {
        "*".to_owned()
    } else {
        columns
            .iter()
            .map(|column| quote_identifier(column, quoting))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "SELECT {selection} FROM {} LIMIT {}",
        qualified(schema, table, quoting),
        max_rows.saturating_add(1)
    )
}

fn qualified(schema: Option<&str>, table: &str, quoting: Quoting) -> String {
    match schema {
        Some(schema) => format!(
            "{}.{}",
            quote_identifier(schema, quoting),
            quote_identifier(table, quoting)
        ),
        None => quote_identifier(table, quoting),
    }
}

/// A page of rows and the key the next one starts after.
pub struct Listing {
    pub result: DbResult,
    pub next: Option<(String, String)>,
}

use crate::adapters::config::SqliteDatabase;
use crate::domain::types::database::{
    DbEngineInfo, DbError, DbLimits, DbOutcome, DbResult, DbStatement, DbStatementResult,
    DbTransaction, Quoting, quote_identifier,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The database to open, with every secret it needs already resolved. The
/// explorer keeps it to open the database again after a lost connection.
#[derive(Clone)]
pub enum DbTarget {
    Sqlite(SqliteDatabase),
    Server(Box<server::ServerAccess>),
}

/// How a running statement is stopped. Dropping the future does not do it on
/// any engine.
pub enum Cancellation {
    /// SQLite runs inside this process: its progress handler reads this flag.
    Flag(Arc<AtomicBool>),
    /// A server keeps running the statement after the client stops listening,
    /// so a second connection has to tell it to stop.
    Server(Box<server::Canceller>),
}

impl Cancellation {
    /// Stop the statement, and say whether the engine confirmed it stopped.
    pub async fn stop(&self) -> bool {
        match self {
            Self::Flag(flag) => {
                flag.store(true, Ordering::Relaxed);
                true
            }
            Self::Server(canceller) => canceller.cancel().await,
        }
    }

    /// Forget a stop that was asked for an earlier statement, so a session
    /// that stays open runs the next one. The CLI runs one statement and
    /// never needs this; the explorer calls it before every request.
    pub fn rearm(&self) {
        match self {
            Self::Flag(flag) => flag.store(false, Ordering::Relaxed),
            Self::Server(canceller) => canceller.rearm(),
        }
    }
}

/// One open database, for the length of one call.
pub enum DbSession {
    Sqlite(sqlite::SqliteSession),
    Server(Box<server::ServerSession>),
}

impl DbSession {
    pub async fn open(target: DbTarget, write: bool) -> Result<Self, DbError> {
        match target {
            DbTarget::Sqlite(database) => {
                sqlite::SqliteSession::open(&database.path, &database.limits, write)
                    .await
                    .map(Self::Sqlite)
            }
            DbTarget::Server(access) => server::ServerSession::open(*access)
                .await
                .map(|session| Self::Server(Box::new(session))),
        }
    }

    /// What ends a running statement, taken before the statement starts: the
    /// session itself is busy running it by then.
    pub fn cancellation(&self) -> Cancellation {
        match self {
            Self::Sqlite(session) => Cancellation::Flag(session.cancellation()),
            Self::Server(session) => Cancellation::Server(Box::new(session.canceller())),
        }
    }

    pub fn engine(&self) -> DbEngineInfo {
        match self {
            Self::Sqlite(session) => session.engine(),
            Self::Server(session) => session.engine(),
        }
    }

    /// How the read was guarded and how it ended. A read leaves nothing behind
    /// whichever engine it ran on.
    pub fn transaction(&self) -> DbTransaction {
        match self {
            // The read-only file handle is the guard; there is no transaction
            // to name an isolation level for and none to roll back.
            Self::Sqlite(_) => DbTransaction {
                read_only: true,
                isolation: None,
                outcome: DbOutcome::NotStarted,
            },
            Self::Server(session) => session.transaction(),
        }
    }

    pub async fn close(self) {
        match self {
            Self::Sqlite(session) => session.close().await,
            Self::Server(session) => session.close().await,
        }
    }

    /// End the request that just ran, so the next one starts from nothing: a
    /// server's read guard is rolled back here and opened again by the next
    /// read, because a statement may have turned it off. A SQLite file is
    /// read-only for the life of the connection and needs nothing.
    pub async fn end_request(&mut self) -> Result<(), DbError> {
        match self {
            Self::Sqlite(_) => Ok(()),
            Self::Server(session) => session.end_request().await,
        }
    }

    pub async fn schemas(&mut self, page: &Page) -> Result<Listing, DbError> {
        match self {
            Self::Sqlite(session) => session.schemas(page).await,
            Self::Server(session) => session.schemas(page).await,
        }
    }

    pub async fn tables(&mut self, schema: Option<&str>, page: &Page) -> Result<Listing, DbError> {
        match self {
            Self::Sqlite(session) => session.tables(schema, page).await,
            Self::Server(session) => session.tables(schema, page).await,
        }
    }

    pub async fn describe(
        &mut self,
        schema: Option<&str>,
        table: &str,
    ) -> Result<DbResult, DbError> {
        match self {
            Self::Sqlite(session) => session.describe(schema, table).await,
            Self::Server(session) => session.describe(schema, table).await,
        }
    }

    pub async fn preview(
        &mut self,
        schema: Option<&str>,
        table: &str,
        columns: &[String],
        limits: &DbLimits,
    ) -> Result<DbResult, DbError> {
        match self {
            Self::Sqlite(session) => session.preview(schema, table, columns, limits).await,
            Self::Server(session) => session.preview(schema, table, columns, limits).await,
        }
    }

    pub async fn query(
        &mut self,
        sql: &str,
        params: &[Option<String>],
        limits: &DbLimits,
    ) -> Result<DbResult, DbError> {
        match self {
            Self::Sqlite(session) => session.query(sql, params, limits).await,
            Self::Server(session) => session.query(sql, params, limits).await,
        }
    }

    pub async fn execute(
        &mut self,
        statements: &[DbStatement],
        limits: &DbLimits,
        commit: bool,
        max_affected_rows: Option<u64>,
    ) -> Result<(Vec<DbStatementResult>, DbOutcome), DbError> {
        match self {
            Self::Sqlite(session) => {
                session
                    .execute(statements, limits, commit, max_affected_rows)
                    .await
            }
            Self::Server(session) => {
                session
                    .execute(statements, limits, commit, max_affected_rows)
                    .await
            }
        }
    }
}
