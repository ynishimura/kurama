//! CLI database arguments and JSON requests, converted to the same typed
//! operation.
use crate::domain::types::database::{
    DbArgs, DbError, DbExecute, DbLimits, DbRequest, DbStatement, InvalidDb, split_table_target,
};
use crate::domain::types::limits;
use crate::shell::cli::client::{self, RequestReadFailure};
use crate::shell::cli::completion;
use clap::{Arg, ArgAction, ArgGroup, ArgMatches, Command, ValueHint};
use clap_complete::engine::ArgValueCandidates;
use std::path::PathBuf;

const OPERATIONS: &[&str] = &[
    "schemas", "tables", "describe", "preview", "query", "file", "execute",
];
/// Everything a `--request` document carries instead, so the two cannot
/// describe two different operations in one call.
const REQUEST_CONFLICTS: &[&str] = &[
    "execute",
    "max-affected-rows",
    "schemas",
    "tables",
    "describe",
    "preview",
    "query",
    "file",
    "schema",
    "columns",
    "param",
    "limit",
    "cursor",
    "timeout",
    "max-rows",
    "max-result-bytes",
];

pub fn command() -> Command {
    let command = Command::new("db")
        .about(
            "Read a configured database or a SQLite file: structure, preview and one SQL statement",
        )
        .arg(
            Arg::new("database")
                .value_name("DATABASE")
                .required(true)
                .value_hint(ValueHint::AnyPath)
                .help("A [db.*] name, or a path to a SQLite file")
                .add(ArgValueCandidates::new(completion::databases)),
        );
    let mut command = client::envelope_args(
        command,
        "Read a typed JSON request; '-' reads stdin, so SQL stays out of argv",
        "Show the connection and the planned operation without connecting",
    )
    .mut_arg("request", |arg| arg.conflicts_with_all(REQUEST_CONFLICTS))
    .arg(
        Arg::new("schemas")
            .long("schemas")
            .action(ArgAction::SetTrue)
            .help("List the schemas of the database"),
    )
    .arg(
        Arg::new("tables")
            .long("tables")
            .action(ArgAction::SetTrue)
            .help("List the tables and views, newest page first with --cursor"),
    )
    .group(ArgGroup::new("operation").args(OPERATIONS))
    .after_help(
        "Examples:\n  \
             kurama db app --tables --json\n  \
             kurama db app --describe public.orders\n  \
             kurama db app --preview orders --max-rows 20\n  \
             kurama db app --query 'SELECT status, count(*) FROM orders GROUP BY status'\n  \
             kurama db app --query 'SELECT * FROM orders WHERE id = ?' --param 42\n  \
             kurama db ./fixtures/app.sqlite3 --preview orders\n\n\
             DATABASE is required and so is one operation. A path with a / or a .db / .sqlite /\n\
             .sqlite3 extension is read as a SQLite file with no configuration, read-only.\n\
             One call runs one statement: the database refuses a second one.\n\
             --dry-run connects to nothing and prints no SQL.",
    );
    for (name, value, help) in [
        (
            "describe",
            "TABLE",
            "Show the columns, keys and references of TABLE (name or schema.name)",
        ),
        (
            "preview",
            "TABLE",
            "Show the first rows of TABLE (name or schema.name)",
        ),
        ("query", "SQL", "Run one statement that returns rows"),
        (
            "execute",
            "SQL",
            "Run one statement that changes rows; needs --rollback or --commit",
        ),
        ("file", "SQL_FILE", "Read that one statement from a file"),
        ("schema", "NAME", "Restrict --tables to one schema"),
        (
            "cursor",
            "CURSOR",
            "Continue a --schemas or --tables listing from its next_cursor",
        ),
    ] {
        let hint = match name {
            "file" => ValueHint::FilePath,
            // A table, a schema, SQL and a cursor come from this very database.
            _ => ValueHint::Other,
        };
        command = command.arg(
            Arg::new(name)
                .long(name)
                .value_name(value)
                .value_hint(hint)
                .help(help),
        );
    }
    for (name, value, help) in [
        (
            "param",
            "VALUE",
            "Bind the next placeholder; repeat in order. A value is never SQL",
        ),
        (
            "columns",
            "COLUMN",
            "Select a preview column; repeat for multiple columns",
        ),
    ] {
        command = command.arg(
            Arg::new(name)
                .long(name)
                .value_name(value)
                .value_hint(ValueHint::Other)
                .action(ArgAction::Append)
                .help(help),
        );
    }
    // The help text and the accepted range are built from the same constants
    // the code enforces, so moving a bound cannot leave the line a person
    // reads saying the old number.
    let defaults = DbLimits::default();
    for (name, help, max) in [
        (
            "limit",
            format!(
                "Rows per listing page (default {}, at most {})",
                limits::DB_CALL.list_page,
                limits::DB_CALL.max_list_page
            ),
            limits::DB_CALL.max_list_page as u64,
        ),
        (
            "timeout",
            format!(
                "Statement deadline in seconds, counted after authentication (default {}, at most {})",
                defaults.query_timeout_secs, limits::DB_CALL.query_timeout_secs
            ),
            limits::DB_CALL.query_timeout_secs,
        ),
        (
            "max-rows",
            format!("Maximum returned rows (default {})", defaults.max_rows),
            u64::MAX,
        ),
        (
            "max-result-bytes",
            format!(
                "Maximum serialized result rows in bytes (minimum 2; default {})",
                defaults.max_result_bytes
            ),
            u64::MAX,
        ),
        (
            "max-affected-rows",
            "Roll back and fail when a statement of an execute changes more rows than this; the [db.*] section may already bound it, and this can only be stricter".to_owned(),
            u64::MAX,
        ),
    ] {
        let minimum = if name == "max-result-bytes" { 2 } else { 1 };
        command = command.arg(
            Arg::new(name)
                .long(name)
                .value_name("N")
                .value_hint(ValueHint::Other)
                .value_parser(clap::value_parser!(u64).range(minimum..=max))
                .help(help),
        );
    }
    for (name, help) in [
        (
            "rollback",
            "Run the change and keep nothing; the locks are still taken",
        ),
        ("commit", "Keep the change when every statement succeeded"),
        (
            "confirm",
            "A person agreed to this --commit: a KURAMA_AGENT run needs it to keep a change",
        ),
    ] {
        command = command.arg(
            Arg::new(name)
                .long(name)
                .action(ArgAction::SetTrue)
                .help(help),
        );
    }
    // An option belongs to the operation that has it. A read already refuses
    // the ones it does not use (`ListingOptionsOnly`, `PreviewColumnsOnly`);
    // a write dropped them silently, so a listing or preview option written
    // beside `--execute` looked applied and did nothing.
    //
    // `--max-affected-rows` is refused in `read_request` instead: clap's
    // `requires` is not enforced for it, and the refusal belongs with the
    // other execute-only option anyway.
    command
        .mut_arg("rollback", |arg| arg.conflicts_with("commit"))
        .mut_arg("confirm", |arg| arg.requires("commit"))
        .mut_arg("limit", |arg| arg.conflicts_with("execute"))
        .mut_arg("cursor", |arg| arg.conflicts_with("execute"))
        .mut_arg("schema", |arg| arg.conflicts_with("execute"))
        .mut_arg("columns", |arg| arg.conflicts_with("execute"))
}

#[derive(Clone)]
pub struct DbCommand {
    pub database: String,
    pub json: bool,
    pub jq: Option<String>,
    pub dry_run: bool,
    /// `Some(true)` keeps the change, `Some(false)` rolls it back, `None` is a
    /// call that says nothing about a transaction.
    pub commit: Option<bool>,
    /// `--confirm`: a person agreed to the `--commit` of an agent's run.
    pub confirm: bool,
    request_file: Option<String>,
    /// `--max-affected-rows` on a call that changes nothing.
    write_bound_without_a_write: bool,
    request: Option<DbRequest>,
}

impl std::fmt::Debug for DbCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DbCommand { statement: [REDACTED] }")
    }
}

impl DbCommand {
    pub fn parse(matches: &ArgMatches) -> Self {
        let string = |name: &str| matches.get_one::<String>(name).cloned();
        let number = |name: &str| matches.get_one::<u64>(name).copied();
        let many = |name: &str| -> Vec<String> {
            matches
                .get_many::<String>(name)
                .map(|values| values.cloned().collect())
                .unwrap_or_default()
        };
        let args = DbArgs {
            schema: string("schema"),
            columns: many("columns"),
            sql: string("query"),
            file: string("file"),
            params: many("param").into_iter().map(Some).collect(),
            limit: number("limit").map(|limit| limit as usize),
            cursor: string("cursor"),
            query_timeout_secs: number("timeout"),
            max_rows: number("max-rows").map(|rows| rows as usize),
            max_result_bytes: number("max-result-bytes").map(|bytes| bytes as usize),
            table: None,
        };
        let request = if matches.get_flag("schemas") {
            Some(DbRequest::Schemas(args))
        } else if matches.get_flag("tables") {
            Some(DbRequest::Tables(args))
        } else if let Some(target) = string("describe") {
            Some(DbRequest::Describe(with_target(args, target)))
        } else if let Some(target) = string("preview") {
            Some(DbRequest::Preview(with_target(args, target)))
        } else if let Some(sql) = string("execute") {
            Some(DbRequest::Execute(DbExecute {
                statements: vec![DbStatement {
                    sql,
                    params: args.params.clone(),
                }],
                max_affected_rows: number("max-affected-rows"),
                query_timeout_secs: args.query_timeout_secs,
                max_rows: args.max_rows,
                max_result_bytes: args.max_result_bytes,
            }))
        } else if args.sql.is_some() || args.file.is_some() {
            Some(DbRequest::Query(args))
        } else if matches.get_flag("dry-run") {
            // Nothing runs, so the cheapest catalog read stands in for the
            // operation the next call will name.
            Some(DbRequest::Schemas(args))
        } else {
            None
        };
        Self {
            database: matches
                .get_one::<String>("database")
                .expect("clap requires DATABASE")
                .clone(),
            json: matches.get_flag("json") || matches.contains_id("jq"),
            jq: matches.get_one::<String>("jq").cloned(),
            dry_run: matches.get_flag("dry-run"),
            commit: match (matches.get_flag("commit"), matches.get_flag("rollback")) {
                (true, false) => Some(true),
                (false, true) => Some(false),
                _ => None,
            },
            confirm: matches.get_flag("confirm"),
            request_file: matches.get_one::<String>("request").cloned(),
            // A bound written for a call that cannot change anything bounds
            // nothing, and looks applied. It is refused like `--commit` is.
            write_bound_without_a_write: number("max-affected-rows").is_some()
                && !request
                    .as_ref()
                    .is_some_and(|request| request.operation().writes()),
            request,
        }
    }

    /// A call that names no operation, no request and no output mode: on a
    /// terminal it opens the explorer, anywhere else it is refused as before.
    pub fn names_nothing_to_run(&self) -> bool {
        self.request.is_none()
            && self.request_file.is_none()
            && !self.json
            && !self.dry_run
            && self.commit.is_none()
            && !self.write_bound_without_a_write
    }

    /// The request this call runs, from the arguments or from the document.
    pub fn read_request(&self) -> Result<DbRequest, DbError> {
        if self.write_bound_without_a_write {
            return Err(InvalidDb::ExecuteOptionsOnly.into());
        }
        let mut request = match &self.request_file {
            Some(path) => {
                let text = client::read_request_text(path).map_err(|failure| match failure {
                    RequestReadFailure::Stdin(source) => DbError::Io {
                        path: PathBuf::from("-"),
                        source,
                    },
                    RequestReadFailure::File(source) => request_file_error(path, source),
                })?;
                serde_json::from_str(&text).map_err(|_| InvalidDb::InvalidRequest)?
            }
            None => self.request.clone().ok_or(InvalidDb::OperationRequired)?,
        };
        // A target written as `schema.table` on the command line is already
        // split; one that arrived in a request has its own fields.
        request.validate()?;
        // A SQL file is request input: a missing one is a usage failure, not a
        // database failure, and a plan never reads it.
        if !self.dry_run
            && let DbRequest::Query(args) = &mut request
            && let Some(path) = args.file.take()
        {
            args.sql = Some(read_sql_file(&path)?);
        }
        Ok(request)
    }
}

/// Split `schema.table` once, here, so every later reader sees two names.
fn with_target(mut args: DbArgs, target: String) -> DbArgs {
    match split_table_target(&target) {
        Ok((schema, table)) => {
            if schema.is_some() {
                args.schema = schema;
            }
            args.table = Some(table);
        }
        // An unusable target is reported by `validate`, with the message that
        // says what a target looks like.
        Err(_) => args.table = Some(String::new()),
    }
    args
}

fn read_sql_file(path: &str) -> Result<String, DbError> {
    std::fs::read_to_string(path).map_err(|source| request_file_error(path, source))
}

fn request_file_error(path: &str, source: std::io::Error) -> DbError {
    if client::is_unopenable(&source) {
        InvalidDb::RequestFile {
            path: path.into(),
            source,
        }
        .into()
    } else {
        DbError::Io {
            path: path.into(),
            source,
        }
    }
}

/// Whether the positional argument is a SQLite file rather than a `[db.*]`
/// name. The same judgement `data` makes about its positional input.
pub fn is_sqlite_file(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.contains('/')
        || [".db", ".sqlite", ".sqlite3"]
            .iter()
            .any(|e| value.ends_with(e))
}

#[cfg(test)]
#[path = "db_command_tests.rs"]
mod tests;
