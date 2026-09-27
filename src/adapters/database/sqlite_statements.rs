//! Ask SQLite itself how many statements a piece of SQL is, and whether the
//! first one returns columns, without executing any of it.
//!
//! sqlx prepares every statement of a string and gives no count back, and a
//! hand-written lexer would have to know that `;` inside a string, inside a
//! comment and inside `CREATE TRIGGER ... BEGIN ... END` is not a separator.
//! `sqlite3_prepare_v2` already knows; this is the only place that calls it.
use crate::domain::types::database::{DbError, InvalidDb, ServerError};
use libsqlite3_sys::{
    SQLITE_OK, sqlite3, sqlite3_column_count, sqlite3_column_decltype, sqlite3_column_name,
    sqlite3_errmsg, sqlite3_finalize, sqlite3_prepare_v2, sqlite3_stmt,
};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr::{NonNull, null, null_mut};

/// What SQLite says one piece of SQL is, before anything runs.
#[derive(Debug, PartialEq, Eq)]
pub struct StatementFacts {
    pub count: usize,
    /// The columns the first statement returns. `DELETE` and `CREATE` have
    /// none, and an empty result still has to report its column names.
    pub columns: Vec<PreparedColumn>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct PreparedColumn {
    pub name: String,
    /// The declared type of the underlying column. An expression has none, and
    /// so does a column declared without a type.
    pub data_type: Option<String>,
}

/// Prepare every statement in `sql` and finalize it again. Nothing is stepped,
/// so nothing is executed and nothing is written.
pub fn inspect(handle: NonNull<sqlite3>, sql: &str) -> Result<StatementFacts, DbError> {
    // A NUL would end the string early and hide the rest from SQLite.
    let text = CString::new(sql).map_err(|_| InvalidDb::NulInStatement)?;
    let database = handle.as_ptr();
    let mut tail: *const c_char = text.as_ptr();
    let mut facts = StatementFacts {
        count: 0,
        columns: vec![],
    };
    loop {
        let mut statement: *mut sqlite3_stmt = null_mut();
        let mut next: *const c_char = null();
        // SAFETY: `database` is the live handle of the locked connection, and
        // `tail` points into `text`, which outlives the call. `-1` stops at the
        // NUL `CString` guarantees. Both out-parameters are owned locals.
        let code = unsafe { sqlite3_prepare_v2(database, tail, -1, &mut statement, &mut next) };
        if code != SQLITE_OK {
            // SAFETY: sqlite3_errmsg returns a NUL-terminated string owned by
            // the connection, valid until the next call on it.
            let message = unsafe { CStr::from_ptr(sqlite3_errmsg(database)) }
                .to_string_lossy()
                .into_owned();
            return Err(DbError::Rejected(ServerError::new(
                code.to_string(),
                &message,
            )));
        }
        if statement.is_null() {
            // Only whitespace or comments were left.
            break;
        }
        if facts.count == 0 {
            // SAFETY: `statement` was just prepared and is not yet finalized,
            // so its column metadata is valid and owned by it.
            let count = unsafe { sqlite3_column_count(statement) }.max(0);
            facts.columns = (0..count)
                .map(|index| PreparedColumn {
                    name: unsafe { owned_text(sqlite3_column_name(statement, index)) }
                        .unwrap_or_default(),
                    data_type: unsafe { owned_text(sqlite3_column_decltype(statement, index)) },
                })
                .collect();
        }
        // SAFETY: the statement was prepared here, was never stepped, and is
        // not used again.
        unsafe { sqlite3_finalize(statement) };
        facts.count += 1;
        tail = next;
        // SAFETY: `next` points at the remainder of `text` or at its NUL.
        if unsafe { *tail } == 0 {
            break;
        }
    }
    Ok(facts)
}

/// Copy a string SQLite owns, or `None` when it has none.
///
/// # Safety
/// `pointer` is null or points at a NUL-terminated string that lives at least
/// until the statement it belongs to is finalized.
unsafe fn owned_text(pointer: *const c_char) -> Option<String> {
    unsafe { (!pointer.is_null()).then(|| CStr::from_ptr(pointer).to_string_lossy().into_owned()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::{ConnectOptions, Connection};

    async fn facts(sql: &str) -> Result<StatementFacts, DbError> {
        let mut connection = sqlx::sqlite::SqliteConnectOptions::new()
            .in_memory(true)
            .connect()
            .await
            .expect("an in-memory database");
        // A trigger needs its table to exist before SQLite will prepare it.
        sqlx::query("CREATE TABLE x (id INTEGER)")
            .execute(&mut connection)
            .await
            .expect("a table to hang a trigger on");
        let mut handle = connection.lock_handle().await.expect("the raw handle");
        let facts = inspect(handle.as_raw_handle(), sql);
        drop(handle);
        connection.close().await.expect("a clean close");
        facts
    }

    #[tokio::test]
    async fn db_sqlite_counts_what_sqlite_calls_a_statement() {
        for (sql, count) in [
            ("SELECT 1", 1),
            ("SELECT 1;", 1),
            ("SELECT 1;;", 1),
            ("SELECT 1; -- trailing", 1),
            ("SELECT 1; /* trailing */", 1),
            ("SELECT ';' AS s", 1),
            ("/* ; */ SELECT 1", 1),
            (
                "CREATE TRIGGER t AFTER INSERT ON x BEGIN SELECT 1; SELECT 2; END",
                1,
            ),
            ("SELECT 1; SELECT 2", 2),
            ("SELECT 1; DROP TABLE x", 2),
            ("", 0),
            ("   ", 0),
            ("-- only a comment", 0),
        ] {
            assert_eq!(facts(sql).await.expect(sql).count, count, "{sql}");
        }
    }

    #[tokio::test]
    async fn db_sqlite_names_the_columns_of_the_first_statement_without_running_it() {
        let columns = facts("SELECT 1 AS one, 2 AS two").await.unwrap().columns;
        assert_eq!(
            columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["one", "two"]
        );
        // An expression has no declared type; only a stored column does.
        assert!(columns.iter().all(|c| c.data_type.is_none()));
        assert_eq!(facts("PRAGMA user_version").await.unwrap().columns.len(), 1);
        assert!(
            facts("CREATE TABLE t (id INTEGER)")
                .await
                .unwrap()
                .columns
                .is_empty()
        );
    }

    #[tokio::test]
    async fn db_sqlite_reports_a_syntax_error_before_anything_runs() {
        let error = facts("SELEC 1").await.unwrap_err();
        let server = error.server().expect("a server error");
        assert!(server.message.contains("SELEC"), "{server:?}");
        let error = facts("SELECT 1; SELEC 2").await.unwrap_err();
        assert!(error.server().is_some());
    }

    /// A NUL is the statement's problem, not a name's: it used to be reported
    /// as "schema, table and column names must not be empty", with a hint to
    /// run `--tables`, which cannot help whoever has a NUL in a `--file`.
    #[tokio::test]
    async fn db_sqlite_refuses_sql_with_an_embedded_nul() {
        let error = facts("SELECT 1\0; DROP TABLE t").await.unwrap_err();
        assert!(
            matches!(error, DbError::Invalid(InvalidDb::NulInStatement)),
            "{error}"
        );
        assert!(error.to_string().contains("NUL"), "{error}");
    }
}
