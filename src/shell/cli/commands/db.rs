//! Database CLI: resolve the database, open one connection for one statement,
//! and publish one bounded result.
use super::db_command::{DbCommand, is_sqlite_file};
use super::db_render::print_result;
use crate::adapters::aws::ssm_tunnel::OpenTunnel;
use crate::adapters::config::{Config, DbConnection};
use crate::adapters::database::{DbSession, Listing, Page};
use crate::domain::types::database::{
    CursorTarget, DbCursor, DbEngineInfo, DbError, DbFailure, DbKind, DbLimits, DbMeta,
    DbNextAction, DbOperation, DbOutcome, DbOutput, DbRequest, DbResult, DbStatementResult,
    DbStopReason, InvalidDb,
};
use crate::shell::agent_policy::{check_db_commit, is_agent_run};
use crate::shell::db_connection::{Waited, open_target, resolve, wait_for_statement};
use serde_json::Value;
use std::time::Instant;
use tokio::signal::unix::{SignalKind, signal};

pub async fn run(command: DbCommand, config: Config) -> anyhow::Result<()> {
    // An agent or a pipe gets the usage error instead of a blank screen.
    if command.names_nothing_to_run() && crate::shell::tui::terminal::supports_tui() {
        let connection = resolve(&command.database, &config)?;
        return crate::shell::tui::database::handle_db_explorer(
            &command.database,
            connection,
            &config,
        )
        .await;
    }
    let request = command.read_request()?;
    note_sql(&request);
    let connection = resolve(&command.database, &config)?;
    // The document calls a SQLite file `ad-hoc`, but two files are two
    // listings: the cursor binds to the database, never to the display name.
    let (target, cursor_target) = if is_sqlite_file(&command.database) {
        (
            "ad-hoc".to_owned(),
            CursorTarget::new(&connection.database()),
        )
    } else {
        (
            command.database.clone(),
            CursorTarget::new(&command.database),
        )
    };
    let operation = request.operation();
    let limits = request.limits(connection.limits());
    // A write is refused by the configuration before anything is opened, so a
    // database nobody meant to change is never even connected to.
    if operation.writes() {
        if !connection.allow_write() {
            return Err(DbError::from(InvalidDb::WriteNotAllowed).into());
        }
        if command.commit.is_none() {
            return Err(DbError::from(InvalidDb::CommitOrRollbackRequired).into());
        }
        // `allow_write` is the database's permission; keeping an agent's
        // change also needs a person's.
        if command.commit == Some(true) && !command.dry_run {
            check_db_commit(is_agent_run(), command.confirm)?;
        }
    } else if command.commit.is_some() {
        return Err(DbError::from(InvalidDb::ExecuteOptionsOnly).into());
    }
    let mut output = DbOutput {
        schema_version: 1,
        kind: DbKind::Db,
        operation,
        target: target.clone(),
        meta: DbMeta::planned(
            operation,
            limits.clone(),
            planned_engine(&connection),
            connection.database(),
            connection.host(),
        ),
        dry_run: None,
        result: None,
        statements: None,
        elapsed_ms: None,
    };
    if command.dry_run {
        output.dry_run = Some(true);
        return publish(&output, &command, false);
    }
    // The cursor binds to this database and this listing before a connection
    // is opened: a cursor from another sequence names rows that are not here.
    let page = match request.args().cursor.as_deref() {
        Some(cursor) => Some(DbCursor::decode(cursor, &cursor_target, operation)?),
        None => None,
    };

    // One listener, registered before anything starts: `ctrl_c()` registers
    // only when first polled, so a SIGINT sent before the wait polled it was
    // lost (ARCH-043).
    let mut interrupt = signal(SignalKind::interrupt())?;
    let start = Instant::now();
    // Every secret is resolved before a socket is opened, so a 1Password
    // prompt that nobody answers cannot be mistaken for an unreachable server.
    let (opened, tunnel) = open_target(&connection, &config).await?;
    output.meta.tunnel = tunnel.as_ref().map(OpenTunnel::reported);
    let mut session = match DbSession::open(opened, operation.writes()).await {
        Ok(session) => session,
        Err(error) => {
            // A tunnel that opened must be closed even when nothing used it.
            if let Some(tunnel) = tunnel {
                tunnel.close().await;
            }
            return Err(error.into());
        }
    };
    output.meta.engine = session.engine();
    output.meta.transaction = session.transaction();
    let cancel = session.cancellation();
    let Waited { outcome, abandoned } = wait_for_statement(
        execute(
            &mut session,
            &request,
            &limits,
            page,
            command.commit == Some(true),
        ),
        &cancel,
        limits.query_timeout_secs,
        async {
            interrupt.recv().await;
        },
        operation.writes(),
    )
    .await;
    if abandoned {
        // The statement still holds the connection, so there is nothing to
        // say goodbye on: the socket closes with the session.
        drop(session);
    } else {
        session.close().await;
    }
    if let Some(tunnel) = tunnel {
        tunnel.close().await;
    }
    let run = match outcome {
        Ok(run) => run,
        Err(error) => {
            // A write that did not finish leaves the transaction named, so a
            // caller can tell "nothing was kept" from "nobody knows".
            if operation.writes() {
                output.meta.transaction.outcome = error.transaction_outcome();
            }
            return Err(error.into());
        }
    };

    output.elapsed_ms = Some(start.elapsed().as_millis() as u64);
    output.meta.observed_at = Some(chrono::Utc::now().to_rfc3339());
    let incomplete = record_run(&mut output, run, &cursor_target, operation);
    publish(&output, &command, incomplete)
}

/// Put what the call produced into the envelope with the advice it calls for,
/// and say whether it is partial.
fn record_run(
    output: &mut DbOutput,
    run: DbRun,
    cursor_target: &CursorTarget,
    operation: DbOperation,
) -> bool {
    // A read and a write report different things, so each arm builds its own
    // half of the envelope: neither can be left holding the other's.
    let incomplete = match run {
        DbRun::Read { result, next } => {
            output.meta.returned_rows = Some(result.row_count);
            output.meta.returned_result_bytes = Some(result.result_bytes);
            output.meta.next_cursor =
                next.map(|key| DbCursor::new(cursor_target, operation, key).encode());
            if output.meta.next_cursor.is_some() {
                output.meta.next_actions.push(DbNextAction {
                    kind: DbKind::Db,
                    operation,
                    message:
                        "More rows remain; pass meta.next_cursor to --cursor for the next page."
                            .into(),
                });
            }
            // A preview that filled its window is a window, not a failure. Any
            // other truncation means the answer is not the whole answer.
            let incomplete = result.truncated()
                && !(operation == DbOperation::Preview
                    && result.stop_reason() == Some(DbStopReason::MaxRows));
            if result.truncated() && !incomplete {
                output.meta.next_actions.push(DbNextAction {
                    kind: DbKind::Db,
                    operation: DbOperation::Query,
                    message: "The preview stopped at max_rows; use --query with a narrower statement for the rest.".into(),
                });
            }
            output.result = Some(result);
            incomplete
        }
        DbRun::Written {
            statements,
            outcome,
        } => {
            output.meta.transaction.read_only = false;
            output.meta.transaction.outcome = outcome;
            // A write reports its statements, so the row counts of one result
            // table would name nothing.
            output.meta.returned_rows = None;
            output.meta.returned_result_bytes = None;
            // A cut `RETURNING` is as partial as a cut read: the change was
            // made, and what came back is not all of what it changed.
            let incomplete = statements
                .iter()
                .any(|statement| statement.result.as_ref().is_some_and(DbResult::truncated));
            output.statements = Some(statements);
            incomplete
        }
    };
    if incomplete {
        output.meta.next_actions.push(DbNextAction {
            kind: DbKind::Db,
            operation: DbOperation::Query,
            message:
                "Narrow the statement or raise --max-rows / --max-result-bytes; there is no continuation cursor for a statement."
                    .into(),
        });
    }
    incomplete
}

/// Print exactly one document, and say whether the result it holds is partial.
fn publish(output: &DbOutput, command: &DbCommand, incomplete: bool) -> anyhow::Result<()> {
    match &command.jq {
        Some(filter) => println!("{}", project(output, filter)?),
        None => print_result(output, command.json),
    }
    if incomplete {
        return Err(DbError::Failed(DbFailure::Incomplete).into());
    }
    Ok(())
}

fn project(output: &DbOutput, filter: &str) -> Result<Value, DbError> {
    crate::shell::cli::client::project_document(output, filter)
        .map_err(|_| DbError::from(InvalidDb::InvalidProjection))
}

/// What one call produced. A read and a write report different shapes, so
/// this is one enum rather than a returned result next to an out parameter:
/// a dummy result stood in for a write's, and the post-processing written for
/// a read then read the dummy's bounds instead of the write's.
enum DbRun {
    Read {
        result: DbResult,
        /// The key the next page of this listing starts after.
        next: Option<(String, String)>,
    },
    Written {
        statements: Vec<DbStatementResult>,
        outcome: DbOutcome,
    },
}

/// The one operation this call runs.
async fn execute(
    session: &mut DbSession,
    request: &DbRequest,
    limits: &DbLimits,
    page: Option<(String, String)>,
    commit: bool,
) -> Result<DbRun, DbError> {
    if let Some(write) = request.execute() {
        let (statements, outcome) = session
            .execute(&write.statements, limits, commit, limits.max_affected_rows)
            .await?;
        return Ok(DbRun::Written {
            statements,
            outcome,
        });
    }
    let args = request.args();
    let page = Page {
        after: page,
        limit: request.list_limit(),
    };
    let rows = |result: DbResult| DbRun::Read { result, next: None };
    let listed = |listing: Listing| DbRun::Read {
        result: listing.result,
        next: listing.next,
    };
    match request {
        DbRequest::Schemas(_) => session.schemas(&page).await.map(listed),
        DbRequest::Tables(_) => session
            .tables(args.schema.as_deref(), &page)
            .await
            .map(listed),
        DbRequest::Describe(_) => session
            .describe(args.schema.as_deref(), table(args)?)
            .await
            .map(rows),
        DbRequest::Preview(_) => session
            .preview(args.schema.as_deref(), table(args)?, &args.columns, limits)
            .await
            .map(rows),
        DbRequest::Query(_) => {
            let sql = args.sql.as_deref().ok_or(InvalidDb::QuerySqlRequired)?;
            session.query(sql, &args.params, limits).await.map(rows)
        }
        // Handled before this match, because a write reports statements.
        DbRequest::Execute(_) => Err(InvalidDb::CommitOrRollbackRequired.into()),
    }
}

fn table(args: &crate::domain::types::database::DbArgs) -> Result<&str, DbError> {
    args.table
        .as_deref()
        .ok_or_else(|| InvalidDb::TableRequired.into())
}

/// What is known about the engine before a connection reports its version.
fn planned_engine(connection: &DbConnection) -> DbEngineInfo {
    DbEngineInfo {
        name: connection.engine().as_str().to_owned(),
        server_version: None,
    }
}

/// The SQL of a query or a change, for the audit log's fingerprint; a
/// listing has none.
fn note_sql(request: &DbRequest) {
    let sql = match request.execute() {
        Some(execute) => Some(
            execute
                .statements
                .iter()
                .map(|statement| statement.sql.as_str())
                .collect::<Vec<_>>()
                .join(";\n"),
        ),
        None => request.args().sql.clone(),
    };
    if let Some(sql) = sql {
        crate::shell::audit::note_sql(&sql);
    }
}
