//! Typed database failures: what was sent when they happened, and what a
//! caller has to change.
use super::limits;
use schemars::JsonSchema;
use serde::Serialize;
use std::path::PathBuf;

/// What the database itself said. The text is the server's, so it is cut to one
/// line and a bounded length before it is published, and it never reaches the
/// logs.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ServerError {
    /// SQLSTATE (PostgreSQL and MySQL) or the SQLite result code. MySQL has
    /// its own error numbers too, but the driver reports SQLSTATE, so that is
    /// the one namespace every arm below reads.
    pub code: String,
    pub message: String,
}

/// The longest server message kurama repeats.
const SERVER_MESSAGE_BYTES: usize = 512;

/// Untrusted text on its way to an error line: one line, no control
/// characters, bounded length.
///
/// A driver, an SDK and a child process all hand kurama text nobody wrote for
/// an `error[CODE]:` line. Making it a type rather than a call means the
/// sanitizing cannot be forgotten at one of the places that builds such an
/// error, and it is written here once instead of in each adapter.
#[derive(Clone, Debug)]
pub struct Detail(String);

impl Detail {
    pub fn new(text: &str) -> Self {
        Self(bounded_one_line(text))
    }
}

impl std::fmt::Display for Detail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for Detail {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}

impl From<String> for Detail {
    fn from(text: String) -> Self {
        Self::new(&text)
    }
}

/// One line, no control characters, at most [`SERVER_MESSAGE_BYTES`].
fn bounded_one_line(message: &str) -> String {
    let mut text: String = message
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if text.len() > SERVER_MESSAGE_BYTES {
        let end = (0..=SERVER_MESSAGE_BYTES)
            .rev()
            .find(|i| text.is_char_boundary(*i))
            .unwrap_or(0);
        text.truncate(end);
    }
    text.trim().to_owned()
}

impl ServerError {
    /// Take a driver's message as untrusted text: one line, no control
    /// characters, bounded length.
    pub fn new(code: impl Into<String>, message: &str) -> Self {
        Self {
            code: code.into(),
            message: bounded_one_line(message),
        }
    }

    /// Whether PostgreSQL refused the statement because a parameter is text
    /// and the column it is compared with is not. The server names the types
    /// but not the remedy, so the hint does.
    ///
    /// SQLSTATE 42883 is `undefined_function`, which also covers a function
    /// nobody defined: `SELECT my_typo(1)` returns it, and telling whoever
    /// wrote that to add `$1::int` sends them the wrong way. What PostgreSQL
    /// writes for the type mismatch is the operator form, so that is what
    /// this reads.
    pub fn needs_a_parameter_cast(&self) -> bool {
        self.code == "42883" && self.message.contains("operator does not exist")
    }

    /// The operation that answers this rejection: a name the caller got wrong
    /// is found by listing, a column by describing the table.
    pub fn next_operation(&self) -> Option<super::database::DbOperation> {
        use super::database::DbOperation;
        let message = self.message.to_ascii_lowercase();
        match self.code.as_str() {
            "42P01" | "42S02" => Some(DbOperation::Tables),
            "42703" | "42S22" => Some(DbOperation::Describe),
            // SQLite has result codes only, so its two common name errors are
            // told apart by the message the engine writes for them.
            _ if message.contains("no such table") => Some(DbOperation::Tables),
            _ if message.contains("no such column") => Some(DbOperation::Describe),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error(transparent)]
    Invalid(#[from] InvalidDb),
    /// Nothing was sent: the connection was never opened.
    ///
    /// The driver's text is carried in the `Detail` rather than as a
    /// `#[source]`: a driver error's own `Display` already embeds its cause,
    /// so attaching it as a source prints that cause twice, which is what
    /// `db_server_that_does_not_answer_is_unreachable_after_its_secret_is_read`
    /// refuses. `Detail` is what keeps it to one bounded line.
    #[error("cannot reach the database: {0}")]
    Unreachable(Detail),
    /// Nothing was sent, and the database is not what could not be reached:
    /// the bastion is. They need different things done about them, so they are
    /// different failures.
    #[error("cannot reach the bastion: {0}")]
    TunnelUnavailable(Detail),
    /// The database refused what was sent: credentials, permissions, SQL or a
    /// constraint.
    #[error("the database rejected the statement ({}): {}", .0.code, .0.message)]
    Rejected(ServerError),
    /// A statement was sent and the read did not finish.
    #[error(transparent)]
    Failed(#[from] DbFailure),
    #[error("database file operation failed for {}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl DbError {
    /// Nothing was sent: the connection was never opened.
    pub fn unreachable(detail: impl Into<Detail>) -> Self {
        Self::Unreachable(detail.into())
    }

    /// Nothing was sent, and the bastion is what could not be reached.
    pub fn tunnel_unavailable(detail: impl Into<Detail>) -> Self {
        Self::TunnelUnavailable(detail.into())
    }
}

/// How a statement a caller asked to stop really ended. The three are what
/// can be said about it and they exclude each other, so no failure can claim
/// a stop the database never made: a client that gave up says nothing about
/// what the server is still doing, and a change kurama rolled back is not a
/// statement anyone stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatementEnd {
    /// The engine stopped the statement and said so.
    Stopped,
    /// Nobody confirmed a stop, so the server may still be running it. Aurora
    /// DSQL, which has no cancel at all, can never say anything else.
    NotConfirmed,
    /// The statement ran to its end and the session rolled the change back
    /// before its next statement or its COMMIT, so nothing was kept.
    RolledBack,
}

impl StatementEnd {
    /// What a cancel that did or did not reach the engine means. Only these
    /// two ends can come of asking: a rollback is what the session does after.
    pub fn after_a_cancel(confirmed: bool) -> Self {
        if confirmed {
            Self::Stopped
        } else {
            Self::NotConfirmed
        }
    }
}

/// What each end is called, written once, so a timeout and an interrupt
/// cannot describe the same end differently.
fn statement_end_text(end: StatementEnd) -> &'static str {
    match end {
        StatementEnd::Stopped => "the database confirmed it stopped",
        StatementEnd::NotConfirmed => "the database was not confirmed to have stopped it",
        StatementEnd::RolledBack => "the change was rolled back, so nothing was kept",
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DbFailure {
    #[error("the statement timed out after {seconds} seconds and {}; narrow it or raise --timeout", statement_end_text(*end))]
    Timeout { seconds: u64, end: StatementEnd },
    #[error("the statement was interrupted and {}", statement_end_text(*end))]
    Interrupted { end: StatementEnd },
    #[error("the database file stayed locked for the whole wait; no statement was executed")]
    Locked,
    #[error("result display limit reached; narrow the statement or raise the display limits")]
    Incomplete,
    #[error(
        "column {column} has type {data_type}, which kurama cannot represent; cast it in the statement"
    )]
    UnsupportedType { column: Detail, data_type: Detail },
    #[error("column {column} holds a {data_type} value kurama could not read")]
    UnreadableValue { column: Detail, data_type: Detail },
    #[error("the database connection ended during the read")]
    Disconnected,
    #[error(
        "statement {index} would change {rows_affected} rows, more than the {max} it was allowed; nothing was kept"
    )]
    AffectedRowsLimit {
        index: usize,
        rows_affected: u64,
        max: u64,
    },
    #[error(
        "the commit was sent and its answer never arrived, so what the database kept is unknown"
    )]
    CommitUnknown,
}

impl DbError {
    /// Whether the failure is known to have sent no statement, so repeating a
    /// read changes nothing. Only the two failures whose type says so qualify.
    pub fn read_only_retry(&self) -> bool {
        matches!(
            self,
            Self::Unreachable(_) | Self::TunnelUnavailable(_) | Self::Failed(DbFailure::Locked)
        )
    }

    pub fn server(&self) -> Option<&ServerError> {
        match self {
            Self::Rejected(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InvalidDb {
    #[error("unknown database; use status --kind db")]
    UnknownDatabase,
    #[error("choose one operation: --schemas, --tables, --describe, --preview, --query or --file")]
    OperationRequired,
    #[error("resource limits must be positive, with max_result_bytes at least 2")]
    PositiveLimits,
    #[error("query needs exactly one of sql or file")]
    QuerySqlRequired,
    #[error("sql, file and params are query options")]
    QueryOptionsOnly,
    #[error("columns is a preview option")]
    PreviewColumnsOnly,
    #[error("describe and preview need a TABLE, as name or schema.name")]
    TableRequired,
    #[error("table is only used by describe and preview")]
    UnexpectedTable,
    #[error("limit and cursor are options of --schemas and --tables")]
    ListingOptionsOnly,
    #[error("limit must be between 1 and {}", limits::DB_CALL.max_list_page)]
    ListLimitRange,
    /// A deadline is a point in time, and past this bound the addition that
    /// makes one overflows. Saying so is the difference between a usage error
    /// and a panic.
    #[error(
        "query timeout must be between 1 and {} seconds",
        limits::DB_CALL.query_timeout_secs
    )]
    QueryTimeoutRange,
    #[error("schema is not a --schemas option")]
    UnexpectedSchema,
    #[error("schema, table and column names must not be empty")]
    InvalidIdentifier,
    /// A NUL ends a C string, so SQLite would prepare only the part before it
    /// and count the statements of something nobody wrote.
    #[error("the statement contains a NUL byte, which would end it early")]
    NulInStatement,
    #[error("the request is not a valid JSON database request")]
    InvalidRequest,
    #[error("cannot read the file {}", path.display())]
    RequestFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the cursor is not a cursor this listing produced")]
    InvalidCursor,
    #[error("the cursor belongs to another database or operation")]
    CursorTarget,
    #[error("the statement must be exactly one statement")]
    MultipleStatements,
    #[error("the statement has no statement to run")]
    EmptyStatement,
    #[error("execute needs at least one statement")]
    NoStatements,
    #[error(
        "execute takes at most {} statements in one transaction",
        limits::WRITE.statements
    )]
    TooManyStatements,
    #[error("this database has no allow_write = true, so it takes no execute")]
    WriteNotAllowed,
    #[error("execute needs exactly one of --rollback and --commit")]
    CommitOrRollbackRequired,
    #[error("--rollback, --commit and --max-affected-rows are execute options")]
    ExecuteOptionsOnly,
    #[error("the statement returns no rows; a read needs a statement with columns")]
    NoColumns,
    #[error("engine = \"sqlite\" needs path")]
    SqlitePathRequired,
    #[error(
        "engine = \"sqlite\" is a file: host, port, database, username, password, iam, tls, ca_file and connect_timeout_secs belong to a server"
    )]
    SqliteServerSettings,
    #[error("path is a sqlite setting; a server is reached by host and database")]
    ServerPath,
    #[error("a server needs host")]
    ServerHostRequired,
    #[error("a server needs database")]
    ServerDatabaseRequired,
    #[error("a server needs username")]
    ServerUsernameRequired,
    #[error("a server needs password, or an [iam] section when its user authenticates with IAM")]
    ServerPasswordRequired,
    #[error("a server authenticates with password or with [iam], not both")]
    PasswordAndIam,
    #[error("iam needs aws_profile")]
    IamProfileRequired,
    #[error("an iam region must not be empty")]
    IamRegionEmpty,
    #[error(
        "an IAM token is a password and RDS takes it over TLS only, so tls must not be \"disable\""
    )]
    IamNeedsTls,
    #[error("the region to sign the IAM token for cannot be told from the host or the AWS profile")]
    IamRegionUnknown,
    #[error("an IAM token is signed for the host name, and this host is not one")]
    IamHostInvalid,
    #[error(
        "an Aurora DSQL host takes engine = \"postgresql\" and an [iam] section, never a password"
    )]
    AuroraDsqlSettings,
    #[error("cannot sign an IAM token: {0}")]
    IamTokenUnsignable(Detail),
    #[error(
        "password must be a reference (op://, aws-secrets://, aws-ssm://), never the password itself"
    )]
    PasswordMustBeAReference,
    #[error("ca_file must be an absolute file path")]
    CaFileMustBeAbsolute,
    #[error("ca_file has nothing to verify with tls = \"disable\"")]
    CaFileNeedsTls,
    #[error(
        "a tunnel makes the connection target a local port, so tls must be \"verify-ca\" or \"disable\""
    )]
    TunnelNeedsVerifyCa,
    #[error("a tunnel needs aws_profile")]
    TunnelProfileRequired,
    #[error("a tunnel names the bastion by instance_name or by instance_id, not both")]
    TunnelInstanceConflict,
    #[error("a tunnel needs instance_name or instance_id")]
    TunnelInstanceRequired,
    #[error("a tunnel region must not be empty")]
    TunnelRegionEmpty,
    #[error(
        "session-manager-plugin {found} is too old; 1.2.536.0 is the first that takes the session response out of the command line"
    )]
    PluginTooOld { found: Detail },
    #[error("session-manager-plugin is not installed or cannot be run")]
    PluginUnavailable,
    #[error("path must be an absolute file path")]
    SqlitePathMustBeAbsolute,
    #[error("the database file does not exist or cannot be opened")]
    FileUnavailable,
    #[error("the file is not a SQLite database")]
    NotADatabase,
    #[error("--jq must evaluate to exactly one JSON value; use an array for multiple results")]
    InvalidProjection,
}

impl InvalidDb {
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            Self::UnknownDatabase => {
                Some("run `kurama status --kind db` to list the configured databases")
            }
            Self::OperationRequired => Some(
                "choose an operation, for example `--tables`, `--describe TABLE` or `--query SQL`",
            ),
            Self::TableRequired | Self::InvalidIdentifier => {
                Some("run the same database with --tables, then pass a listed name or schema.name")
            }
            Self::NulInStatement => {
                Some("remove the NUL byte from the statement, or from the --file it was read from")
            }
            Self::MultipleStatements | Self::EmptyStatement => {
                Some("send exactly one statement; the engine refuses a second one in the same call")
            }
            Self::NoColumns => Some(
                "a read needs a statement that returns rows; `--query` cannot run one that does not",
            ),
            Self::FileUnavailable | Self::SqlitePathRequired | Self::SqlitePathMustBeAbsolute => {
                Some(
                    "check the SQLite file path and its read permissions; kurama never creates one",
                )
            }
            Self::PasswordMustBeAReference => Some(
                "write the reference of where the password is kept -- op://<vault>/<item>/<field>, aws-secrets://<aws-profile>/<secret-id>[#<json-key>] or aws-ssm://<aws-profile>/<parameter-name>; `kurama agent --kind db --json` shows the section",
            ),
            Self::SqliteServerSettings
            | Self::ServerPath
            | Self::ServerHostRequired
            | Self::ServerDatabaseRequired
            | Self::ServerUsernameRequired
            | Self::ServerPasswordRequired
            | Self::PasswordAndIam
            | Self::IamProfileRequired
            | Self::IamRegionEmpty
            | Self::IamNeedsTls
            | Self::IamHostInvalid
            | Self::AuroraDsqlSettings
            | Self::IamTokenUnsignable(_)
            | Self::CaFileMustBeAbsolute
            | Self::CaFileNeedsTls
            | Self::TunnelNeedsVerifyCa
            | Self::TunnelProfileRequired
            | Self::TunnelInstanceConflict
            | Self::TunnelInstanceRequired
            | Self::TunnelRegionEmpty => {
                Some("`kurama agent --kind db --json` shows the settings each engine has")
            }
            Self::IamRegionUnknown => {
                Some("add region to the [db.<name>.iam] section, or to the AWS profile")
            }
            Self::PluginTooOld { .. } | Self::PluginUnavailable => Some(
                "install or update the Session Manager plugin: `brew install --cask session-manager-plugin`",
            ),
            Self::NotADatabase => Some("point the path at a SQLite database file"),
            Self::WriteNotAllowed => Some(
                "add allow_write = true to the [db.*] section of a database that is meant to be written",
            ),
            Self::CommitOrRollbackRequired | Self::ExecuteOptionsOnly => Some(
                "say what to do with the transaction: --rollback runs it and keeps nothing, --commit keeps it",
            ),
            Self::NoStatements | Self::TooManyStatements => {
                Some("one execute is one transaction; split a longer change into several")
            }
            Self::InvalidCursor | Self::CursorTarget => {
                Some("pass the meta.next_cursor of the previous page of the same listing")
            }
            Self::RequestFile { .. } => {
                Some("check the --request or --file path and its read permissions")
            }
            Self::InvalidRequest => {
                Some("run `kurama agent --kind db --json` for the request schema")
            }
            Self::PositiveLimits | Self::ListLimitRange | Self::QueryTimeoutRange => {
                Some("check the effective limits with --dry-run")
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::database::DbOperation;

    /// Every `error[CODE]:` line carries one line. `Detail` is the only way to
    /// put text nobody wrote for that line into a `DbError`, so the rule is
    /// kept by the type and not by whoever builds the error.
    #[test]
    fn db_untrusted_detail_is_one_bounded_line_whoever_built_it() {
        assert_eq!(Detail::new("a\nb\tc\u{1b}d").to_string(), "a b c d");
        assert_eq!(Detail::new("  spaced  ").to_string(), "spaced");
        assert!(Detail::new(&"日".repeat(1000)).to_string().len() <= SERVER_MESSAGE_BYTES);
        // The two failures that carry a driver's or an SDK's own text build it
        // the same way, and neither can be built with anything else.
        for error in [
            DbError::unreachable("postgres\n(connection refused)"),
            DbError::tunnel_unavailable("SSM\tStartSession: denied"),
        ] {
            assert!(
                !error.to_string().contains(['\n', '\t', '\u{1b}']),
                "{error}"
            );
        }
        // The driver's text is in the `Detail` and not under a `#[source]`.
        // A driver error's own `Display` already embeds its cause, so a source
        // would print that cause a second time, which is what the harness
        // check "error messages do not repeat causes" refuses. Nothing under
        // these two means nothing to repeat.
        for error in [
            DbError::unreachable("postgresql (connection refused (os error 61))"),
            DbError::tunnel_unavailable("SSM StartSession: denied"),
        ] {
            assert!(std::error::Error::source(&error).is_none(), "{error}");
        }
    }

    #[test]
    fn db_server_messages_are_one_bounded_line_of_untrusted_text() {
        let error = ServerError::new("42S22", " no such column:\n\x1b[31mid\ttail ");
        assert_eq!(error.message, "no such column:  [31mid tail");
        let long = ServerError::new("1", &"日".repeat(1000));
        assert!(long.message.len() <= SERVER_MESSAGE_BYTES);
        assert!(long.message.chars().all(|c| c == '日'));
    }

    #[test]
    fn db_name_rejections_point_at_the_operation_that_answers_them() {
        for (code, message, expected) in [
            ("42P01", "", Some(DbOperation::Tables)),
            ("42S02", "", Some(DbOperation::Tables)),
            ("42703", "", Some(DbOperation::Describe)),
            ("42S22", "", Some(DbOperation::Describe)),
            ("1", "no such table: orders", Some(DbOperation::Tables)),
            ("1", "no such column: id", Some(DbOperation::Describe)),
            ("23505", "duplicate key", None),
        ] {
            assert_eq!(
                ServerError::new(code, message).next_operation(),
                expected,
                "{code} {message}"
            );
        }
    }

    #[test]
    fn db_postgres_names_the_types_of_a_comparison_it_has_no_operator_for() {
        // What PostgreSQL 17 and 18 really answer for `WHERE id = $1` when the
        // parameter is text, which is how every parameter is sent.
        let error = ServerError::new("42883", "operator does not exist: integer = text");
        assert!(error.needs_a_parameter_cast());
        assert!(!ServerError::new("42P01", "no such table").needs_a_parameter_cast());
    }

    #[test]
    fn db_read_only_retry_is_only_for_failures_that_sent_nothing() {
        assert!(DbError::unreachable("connection refused").read_only_retry());
        assert!(DbError::tunnel_unavailable("the agent is offline").read_only_retry());
        assert!(DbError::Failed(DbFailure::Locked).read_only_retry());
        for sent in [
            DbError::Failed(DbFailure::Timeout {
                seconds: 30,
                end: StatementEnd::Stopped,
            }),
            DbError::Failed(DbFailure::Interrupted {
                end: StatementEnd::RolledBack,
            }),
            DbError::Failed(DbFailure::Incomplete),
            DbError::Failed(DbFailure::Disconnected),
            DbError::Rejected(ServerError::new("42P01", "no such table")),
            DbError::Invalid(InvalidDb::UnknownDatabase),
        ] {
            assert!(!sent.read_only_retry(), "{sent}");
        }
    }

    #[test]
    fn db_pass_through_variants_print_their_cause_once() {
        // The database's own words are on the line, not only in the JSON: a
        // rejection whose reason is hidden tells a person nothing.
        let rejected = DbError::Rejected(ServerError::new("1", "no such table: ordrs"));
        assert_eq!(
            format!("{rejected}"),
            "the database rejected the statement (1): no such table: ordrs"
        );
        let error = anyhow::Error::from(DbError::Invalid(InvalidDb::UnknownDatabase));
        assert_eq!(
            format!("{error:#}"),
            "unknown database; use status --kind db"
        );
        let error = anyhow::Error::from(DbError::Failed(DbFailure::Interrupted {
            end: StatementEnd::NotConfirmed,
        }));
        assert_eq!(
            format!("{error:#}"),
            "the statement was interrupted and the database was not confirmed to have stopped it"
        );
    }

    /// Each end says what happened to the statement and to the change, and no
    /// two of them say the same thing: a confirmed stop and a change kurama
    /// rolled back were both `stopped: true` until a real Aurora DSQL cluster
    /// reported a timeout it had stopped nothing about as a stop the database
    /// had confirmed.
    #[test]
    fn db_each_end_of_a_stopped_statement_says_something_different() {
        let texts = [
            StatementEnd::Stopped,
            StatementEnd::NotConfirmed,
            StatementEnd::RolledBack,
        ]
        .map(statement_end_text);
        assert_eq!(
            texts,
            [
                "the database confirmed it stopped",
                "the database was not confirmed to have stopped it",
                "the change was rolled back, so nothing was kept",
            ]
        );
        // Asking a cancel produces two of the three; the third is what the
        // session does after the statement ended by itself.
        assert_eq!(StatementEnd::after_a_cancel(true), StatementEnd::Stopped);
        assert_eq!(
            StatementEnd::after_a_cancel(false),
            StatementEnd::NotConfirmed
        );
    }
}
