//! Database requests, their limits and the identifiers they name; no driver,
//! connection or I/O.
pub use super::database_error::{DbError, DbFailure, InvalidDb, ServerError, StatementEnd};
#[cfg(test)]
pub use super::database_output::DbConsistency;
pub use super::database_output::{
    DbColumn, DbEncoding, DbEngineInfo, DbKind, DbMeta, DbNextAction, DbOutcome, DbOutput,
    DbResult, DbStatementResult, DbStopReason, DbTransaction, DbTunnelInfo,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How much of a read a call may return, how long it may wait, and how much of
/// a change it may make.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default)]
pub struct DbLimits {
    #[schemars(range(min = 1))]
    pub query_timeout_secs: u64,
    #[schemars(range(min = 1))]
    pub max_rows: usize,
    #[schemars(range(min = 2))]
    pub max_result_bytes: usize,
    /// Roll back and fail when any one statement changes more rows than this.
    /// A `WHERE` nobody finished is caught before the commit, not after it.
    ///
    /// It is a limit like the others, so a `[db.*]` section that grants
    /// `allow_write` can bound every write to it and a call may only be
    /// stricter or say nothing. Left out everywhere, a write is unbounded,
    /// which is what a section that granted `allow_write` asked for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_affected_rows: Option<u64>,
}

impl Default for DbLimits {
    fn default() -> Self {
        Self {
            query_timeout_secs: 30,
            max_rows: 1000,
            max_result_bytes: 8 * 1024 * 1024,
            max_affected_rows: None,
        }
    }
}

impl DbLimits {
    pub fn validate(&self) -> Result<(), DbError> {
        if self.query_timeout_secs == 0
            || self.max_rows == 0
            || self.max_result_bytes < 2
            || self.max_affected_rows == Some(0)
        {
            return Err(InvalidDb::PositiveLimits.into());
        }
        // A deadline is a point in time: past this the addition that makes one
        // overflows, so a number nobody can wait out is a usage error and not
        // a long wait.
        if self.query_timeout_secs > limits::DB_CALL.query_timeout_secs {
            return Err(InvalidDb::QueryTimeoutRange.into());
        }
        Ok(())
    }

    /// Apply the per-call overrides a read carries.
    ///
    /// Every override goes through [`Self::applied`], so a read and a write
    /// cannot resolve the same limit differently.
    pub fn overridden(&self, args: &DbArgs) -> Self {
        self.applied(&DbOverrides {
            query_timeout_secs: args.query_timeout_secs,
            max_rows: args.max_rows,
            max_result_bytes: args.max_result_bytes,
            // A read changes nothing, so it has no write bound to report; a
            // section's own would otherwise be published as this call's.
            max_affected_rows: None,
            writes: false,
        })
    }

    /// The one place an override is resolved. Both sides are destructured
    /// without `..` on purpose: a limit added to `DbLimits` has to say here
    /// whether a call may override it, instead of silently working for one
    /// operation only.
    fn applied(&self, overrides: &DbOverrides) -> Self {
        let Self {
            query_timeout_secs: base_timeout,
            max_rows: base_rows,
            max_result_bytes: base_bytes,
            max_affected_rows: base_affected,
        } = self;
        let DbOverrides {
            query_timeout_secs,
            max_rows,
            max_result_bytes,
            max_affected_rows,
            writes,
        } = overrides;
        Self {
            query_timeout_secs: query_timeout_secs.unwrap_or(*base_timeout),
            max_rows: max_rows.unwrap_or(*base_rows),
            max_result_bytes: max_result_bytes.unwrap_or(*base_bytes),
            // A call may bound a write further than its section did, never
            // beyond it: the section is what granted the write at all. A read
            // reports no bound, because none of them applies to it.
            max_affected_rows: match (writes, max_affected_rows, *base_affected) {
                (false, _, _) => None,
                (true, Some(call), Some(section)) => Some((*call).min(section)),
                (true, call, section) => call.or(section),
            },
        }
    }
}

/// What one call asks to change about the limits of its database. Named rather
/// than positional: four of these are `Option<u64>` or `Option<usize>`, so two
/// of them swapped would compile and quietly take effect.
struct DbOverrides {
    query_timeout_secs: Option<u64>,
    max_rows: Option<usize>,
    max_result_bytes: Option<usize>,
    max_affected_rows: Option<u64>,
    /// Whether this call can change anything, which is what decides that the
    /// write bound is reported at all.
    writes: bool,
}

use super::limits;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DbOperation {
    Schemas,
    Tables,
    Describe,
    Preview,
    Query,
    Execute,
}

impl DbOperation {
    /// Operations that walk the catalog one page at a time.
    pub fn is_listing(self) -> bool {
        match self {
            Self::Schemas | Self::Tables => true,
            Self::Describe | Self::Preview | Self::Query | Self::Execute => false,
        }
    }

    /// The one operation that changes anything.
    pub fn writes(self) -> bool {
        match self {
            Self::Execute => true,
            Self::Schemas | Self::Tables | Self::Describe | Self::Preview | Self::Query => false,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Schemas => "schemas",
            Self::Tables => "tables",
            Self::Describe => "describe",
            Self::Preview => "preview",
            Self::Query => "query",
            Self::Execute => "execute",
        }
    }

    pub const REQUESTS: [Self; 6] = [
        Self::Schemas,
        Self::Tables,
        Self::Describe,
        Self::Preview,
        Self::Query,
        Self::Execute,
    ];
}

// SQL, parameters and cursors intentionally do not implement Debug.
#[derive(Clone, Deserialize, JsonSchema)]
#[serde(
    tag = "operation",
    content = "args",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum DbRequest {
    Schemas(DbArgs),
    Tables(DbArgs),
    Describe(DbArgs),
    Preview(DbArgs),
    Query(DbArgs),
    Execute(DbExecute),
}

/// A change, as one transaction. Whether it is kept is not written here: the
/// caller says `--commit` or `--rollback` on the call itself.
#[derive(Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbExecute {
    /// Run in this order, in one transaction. One failure rolls back all.
    pub statements: Vec<DbStatement>,
    /// Bound this write further than its `[db.*]` section did. The section's
    /// own `max_affected_rows` still applies, so a call can only be stricter.
    pub max_affected_rows: Option<u64>,
    #[schemars(range(min = 1))]
    pub query_timeout_secs: Option<u64>,
    #[schemars(range(min = 1))]
    pub max_rows: Option<usize>,
    #[schemars(range(min = 2))]
    pub max_result_bytes: Option<usize>,
}

#[derive(Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbStatement {
    pub sql: String,
    /// Placeholder values, in order. A value is a string or null.
    #[serde(default)]
    pub params: Vec<Option<String>>,
}

#[derive(Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbArgs {
    /// The schema a listing is restricted to, or the one a target named.
    pub schema: Option<String>,
    /// The table `describe` and `preview` read, without its schema.
    pub table: Option<String>,
    /// The columns `preview` selects; all of them when empty.
    #[serde(default)]
    pub columns: Vec<String>,
    pub sql: Option<String>,
    /// A file holding one statement; the CLI reads it before connecting.
    pub file: Option<String>,
    /// Placeholder values, in order. A value is a string or null; the server
    /// decides how to compare it with the column.
    #[serde(default)]
    pub params: Vec<Option<String>>,
    #[schemars(range(min = 1, max = 1000))]
    pub limit: Option<usize>,
    /// The `next_cursor` of the previous page of the same listing.
    pub cursor: Option<String>,
    #[schemars(range(min = 1))]
    pub query_timeout_secs: Option<u64>,
    #[schemars(range(min = 1))]
    pub max_rows: Option<usize>,
    #[schemars(range(min = 2))]
    pub max_result_bytes: Option<usize>,
}

/// The arguments a read carries. A write has its own shape, so every reader of
/// them says what it does without one.
static NO_ARGS: std::sync::LazyLock<DbArgs> = std::sync::LazyLock::new(DbArgs::default);

impl DbRequest {
    pub fn args(&self) -> &DbArgs {
        match self {
            Self::Schemas(a)
            | Self::Tables(a)
            | Self::Describe(a)
            | Self::Preview(a)
            | Self::Query(a) => a,
            Self::Execute(_) => &NO_ARGS,
        }
    }

    pub fn execute(&self) -> Option<&DbExecute> {
        match self {
            Self::Execute(execute) => Some(execute),
            _ => None,
        }
    }

    /// The limits this call runs under, whatever shape its arguments have.
    pub fn limits(&self, base: &DbLimits) -> DbLimits {
        match self {
            Self::Execute(execute) => base.applied(&DbOverrides {
                query_timeout_secs: execute.query_timeout_secs,
                max_rows: execute.max_rows,
                max_result_bytes: execute.max_result_bytes,
                max_affected_rows: execute.max_affected_rows,
                writes: true,
            }),
            _ => base.overridden(self.args()),
        }
    }

    pub fn operation(&self) -> DbOperation {
        match self {
            Self::Schemas(_) => DbOperation::Schemas,
            Self::Tables(_) => DbOperation::Tables,
            Self::Describe(_) => DbOperation::Describe,
            Self::Preview(_) => DbOperation::Preview,
            Self::Query(_) => DbOperation::Query,
            Self::Execute(_) => DbOperation::Execute,
        }
    }

    pub fn validate(&self) -> Result<(), DbError> {
        if let Self::Execute(execute) = self {
            if execute.statements.is_empty() {
                return Err(InvalidDb::NoStatements.into());
            }
            if execute.statements.len() > crate::domain::types::limits::WRITE.statements {
                return Err(InvalidDb::TooManyStatements.into());
            }
            if execute
                .statements
                .iter()
                .any(|one| one.sql.trim().is_empty())
            {
                return Err(InvalidDb::EmptyStatement.into());
            }
            return self.limits(&DbLimits::default()).validate();
        }
        let args = self.args();
        let operation = self.operation();
        if matches!(self, Self::Query(_)) {
            if args.sql.is_some() == args.file.is_some() {
                return Err(InvalidDb::QuerySqlRequired.into());
            }
        } else if args.sql.is_some() || args.file.is_some() || !args.params.is_empty() {
            return Err(InvalidDb::QueryOptionsOnly.into());
        }
        if !matches!(self, Self::Preview(_)) && !args.columns.is_empty() {
            return Err(InvalidDb::PreviewColumnsOnly.into());
        }
        if matches!(self, Self::Describe(_) | Self::Preview(_)) && args.table.is_none() {
            return Err(InvalidDb::TableRequired.into());
        }
        if !matches!(self, Self::Describe(_) | Self::Preview(_)) && args.table.is_some() {
            return Err(InvalidDb::UnexpectedTable.into());
        }
        if !operation.is_listing() && (args.limit.is_some() || args.cursor.is_some()) {
            return Err(InvalidDb::ListingOptionsOnly.into());
        }
        if args
            .limit
            .is_some_and(|limit| limit == 0 || limit > limits::DB_CALL.max_list_page)
        {
            return Err(InvalidDb::ListLimitRange.into());
        }
        if matches!(self, Self::Schemas(_)) && args.schema.is_some() {
            return Err(InvalidDb::UnexpectedSchema.into());
        }
        for name in [&args.schema, &args.table]
            .into_iter()
            .flatten()
            .chain(&args.columns)
        {
            if name.is_empty() || name.contains('\0') {
                return Err(InvalidDb::InvalidIdentifier.into());
            }
        }
        DbLimits::default().overridden(args).validate()?;
        Ok(())
    }

    /// The page size a listing uses, bounded by what the contract publishes.
    pub fn list_limit(&self) -> usize {
        self.args()
            .limit
            .unwrap_or(limits::DB_CALL.list_page)
            .min(limits::DB_CALL.max_list_page)
    }
}

/// A `schema.table` target as the command line writes it. An unqualified name
/// leaves the schema to the engine's default.
pub fn split_table_target(target: &str) -> Result<(Option<String>, String), DbError> {
    let invalid = || DbError::from(InvalidDb::InvalidIdentifier);
    let (schema, table) = match target.split_once('.') {
        Some((schema, table)) => (Some(schema.to_owned()), table.to_owned()),
        None => (None, target.to_owned()),
    };
    if table.is_empty()
        || table.contains(['.', '\0'])
        || schema.as_ref().is_some_and(|s| s.is_empty())
    {
        return Err(invalid());
    }
    Ok((schema, table))
}

/// What a commit's answer means, in one place, because two engines send the
/// same commit and must read the same answer.
///
/// A commit that the database refused kept nothing, and saying so is safe. A
/// commit that was sent and never answered is the one case where nothing here
/// knows what the database did: only a read can tell, so the failure says
/// exactly that instead of guessing either way.
pub fn after_commit(answer: Result<(), DbError>) -> Result<DbOutcome, DbError> {
    match answer {
        Ok(()) => Ok(DbOutcome::Committed),
        Err(DbError::Failed(_)) => Err(DbFailure::CommitUnknown.into()),
        Err(refused) => Err(refused),
    }
}

impl DbError {
    /// What a write that ended in this failure left behind. Everything but an
    /// unanswered commit rolled back, which is what the transaction is for.
    pub fn transaction_outcome(&self) -> DbOutcome {
        match self {
            Self::Failed(DbFailure::CommitUnknown) => DbOutcome::Unknown,
            _ => DbOutcome::RolledBack,
        }
    }
}

/// Where a listing stopped, so its next page starts after exactly that row and
/// only on the listing that produced it.
#[derive(Deserialize, Serialize)]
pub struct DbCursor {
    pub target: String,
    pub operation: String,
    pub schema: String,
    pub after: String,
}

/// What a cursor is bound to: the sequence of rows it continues.
///
/// The envelope calls a SQLite file `ad-hoc`, which is a word and not a
/// database: every file got the same one, so a cursor from one file was
/// accepted by another and its rows were silently skipped. A cursor binds to
/// what identifies the sequence, never to what the document displays.
pub struct CursorTarget(String);

impl CursorTarget {
    pub fn new(target: &str) -> Self {
        Self(target.to_owned())
    }
}

impl DbCursor {
    pub fn new(target: &CursorTarget, operation: DbOperation, key: (String, String)) -> Self {
        Self {
            target: target.0.clone(),
            operation: operation.as_str().to_owned(),
            schema: key.0,
            after: key.1,
        }
    }

    pub fn encode(&self) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(self).expect("a typed cursor serializes"))
    }

    /// The `(schema, name)` the next page starts after, or a refusal: a cursor
    /// from another database or another listing describes another sequence.
    pub fn decode(
        text: &str,
        target: &CursorTarget,
        operation: DbOperation,
    ) -> Result<(String, String), DbError> {
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(text)
            .map_err(|_| InvalidDb::InvalidCursor)?;
        let cursor: Self = serde_json::from_slice(&bytes).map_err(|_| InvalidDb::InvalidCursor)?;
        if cursor.target != target.0 || cursor.operation != operation.as_str() {
            return Err(InvalidDb::CursorTarget.into());
        }
        Ok((cursor.schema, cursor.after))
    }
}

/// How an engine writes an identifier. MySQL's own quote is the backtick, and
/// it reads a double-quoted name as a string unless someone set `ANSI_QUOTES`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quoting {
    Ansi,
    MySql,
}

/// Quote an identifier kurama itself puts into SQL. Values are always bound.
pub fn quote_identifier(value: &str, quoting: Quoting) -> String {
    match quoting {
        Ansi => format!("\"{}\"", value.replace('"', "\"\"")),
        MySql => format!("`{}`", value.replace('`', "``")),
    }
}
use Quoting::{Ansi, MySql};

#[cfg(test)]
mod tests {
    use super::*;

    fn request(json: serde_json::Value) -> Result<DbRequest, DbError> {
        let request: DbRequest = serde_json::from_value(json).expect("a typed request");
        request.validate().map(|()| request)
    }

    #[test]
    fn db_requests_keep_each_option_with_the_operation_that_has_it() {
        assert!(request(serde_json::json!({"operation":"tables","args":{}})).is_ok());
        assert!(request(
            serde_json::json!({"operation":"query","args":{"sql":"SELECT 1","params":["a",null]}})
        )
        .is_ok());
        assert!(request(
            serde_json::json!({"operation":"preview","args":{"table":"orders","columns":["id"]}})
        )
        .is_ok());
        for invalid in [
            // A query needs exactly one statement source.
            serde_json::json!({"operation":"query","args":{}}),
            serde_json::json!({"operation":"query","args":{"sql":"SELECT 1","file":"/q.sql"}}),
            // SQL, params and columns belong to the operations that take them.
            serde_json::json!({"operation":"tables","args":{"sql":"SELECT 1"}}),
            serde_json::json!({"operation":"tables","args":{"params":["a"]}}),
            serde_json::json!({"operation":"describe","args":{"table":"t","columns":["id"]}}),
            // describe and preview name a table; the others do not.
            serde_json::json!({"operation":"describe","args":{}}),
            serde_json::json!({"operation":"preview","args":{}}),
            serde_json::json!({"operation":"tables","args":{"table":"orders"}}),
            // Paging belongs to the listings.
            serde_json::json!({"operation":"preview","args":{"table":"t","limit":10}}),
            serde_json::json!({"operation":"query","args":{"sql":"SELECT 1","cursor":"x"}}),
            serde_json::json!({"operation":"tables","args":{"limit":0}}),
            serde_json::json!({"operation":"tables","args":{"limit":1001}}),
            serde_json::json!({"operation":"schemas","args":{"schema":"main"}}),
            // Limits stay positive.
            serde_json::json!({"operation":"tables","args":{"max_rows":0}}),
            serde_json::json!({"operation":"tables","args":{"max_result_bytes":1}}),
            serde_json::json!({"operation":"tables","args":{"query_timeout_secs":0}}),
            // Identifiers are names, not empty strings.
            serde_json::json!({"operation":"describe","args":{"table":""}}),
        ] {
            assert!(request(invalid.clone()).is_err(), "{invalid}");
        }
    }

    #[test]
    fn db_unknown_request_keys_are_refused() {
        assert!(
            serde_json::from_value::<DbRequest>(
                serde_json::json!({"operation":"tables","args":{"nope":1}})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<DbRequest>(
                serde_json::json!({"operation":"vacuum","args":{}})
            )
            .is_err()
        );
    }

    #[test]
    fn db_table_targets_split_on_the_first_dot_only() {
        assert_eq!(
            split_table_target("public.orders").unwrap(),
            (Some("public".into()), "orders".into())
        );
        assert_eq!(
            split_table_target("orders").unwrap(),
            (None, "orders".into())
        );
        for invalid in ["", ".orders", "a.b.c", "orders.", "or\0ders"] {
            assert!(split_table_target(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn db_a_commit_that_was_never_answered_is_the_only_unknown_one() {
        assert!(matches!(after_commit(Ok(())), Ok(DbOutcome::Committed)));
        // The database refused the commit: it kept nothing, and it said so.
        let refused = DbError::Rejected(ServerError::new("40001", "deadlock detected"));
        assert!(matches!(
            after_commit(Err(refused)),
            Err(DbError::Rejected(_))
        ));
        // The commit went out and nothing came back.
        for ended in [
            DbFailure::Disconnected,
            DbFailure::Timeout {
                seconds: 30,
                end: StatementEnd::NotConfirmed,
            },
        ] {
            let unknown = after_commit(Err(DbError::Failed(ended)));
            assert!(
                matches!(unknown, Err(DbError::Failed(DbFailure::CommitUnknown))),
                "a commit whose answer never arrived is not a rollback"
            );
        }
        // And that is the one failure a write reports as unknown.
        assert!(matches!(
            DbError::Failed(DbFailure::CommitUnknown).transaction_outcome(),
            DbOutcome::Unknown
        ));
        for kept_nothing in [
            DbError::Failed(DbFailure::Locked),
            DbError::Failed(DbFailure::Interrupted {
                end: StatementEnd::RolledBack,
            }),
            DbError::Rejected(ServerError::new("23505", "duplicate key")),
            DbError::Invalid(InvalidDb::WriteNotAllowed),
        ] {
            assert!(
                matches!(kept_nothing.transaction_outcome(), DbOutcome::RolledBack),
                "{kept_nothing}"
            );
        }
    }

    #[test]
    fn db_cursors_continue_only_the_listing_that_made_them() {
        let app = CursorTarget::new("app");
        let cursor = DbCursor::new(&app, DbOperation::Tables, ("main".into(), "orders".into()));
        let text = cursor.encode();
        assert_eq!(
            DbCursor::decode(&text, &app, DbOperation::Tables).unwrap(),
            ("main".to_owned(), "orders".to_owned())
        );
        // Another database, or another listing, is another sequence.
        assert!(matches!(
            DbCursor::decode(&text, &CursorTarget::new("other"), DbOperation::Tables),
            Err(DbError::Invalid(InvalidDb::CursorTarget))
        ));
        assert!(matches!(
            DbCursor::decode(&text, &app, DbOperation::Schemas),
            Err(DbError::Invalid(InvalidDb::CursorTarget))
        ));
        for invalid in ["", "not base64!", "eyJhIjoxfQ"] {
            assert!(matches!(
                DbCursor::decode(invalid, &app, DbOperation::Tables),
                Err(DbError::Invalid(InvalidDb::InvalidCursor))
            ));
        }
        // Two SQLite files are two listings even though the envelope calls
        // both of them `ad-hoc`: the cursor binds to the file.
        let one = CursorTarget::new("/tmp/one.sqlite3");
        let other = CursorTarget::new("/tmp/other.sqlite3");
        let page = DbCursor::new(&one, DbOperation::Tables, ("main".into(), "a".into())).encode();
        assert!(matches!(
            DbCursor::decode(&page, &other, DbOperation::Tables),
            Err(DbError::Invalid(InvalidDb::CursorTarget))
        ));
    }

    #[test]
    fn db_identifiers_are_quoted_the_way_the_engine_reads_them() {
        assert_eq!(quote_identifier("orders", Quoting::Ansi), "\"orders\"");
        assert_eq!(quote_identifier("orders", Quoting::MySql), "`orders`");
        // The quote character of each engine closes only when it is doubled.
        assert_eq!(
            quote_identifier("a\"; DROP TABLE t --", Quoting::Ansi),
            "\"a\"\"; DROP TABLE t --\""
        );
        assert_eq!(
            quote_identifier("a`; DROP TABLE t --", Quoting::MySql),
            "`a``; DROP TABLE t --`"
        );
    }

    /// A deadline is `Instant + Duration`, which panics near the end of the
    /// clock. Before the bound moved to `limits.rs` the CLI took `u64::MAX`
    /// and the process ended with a panic and exit 101 instead of one
    /// `error[DB_INVALID]` line.
    #[test]
    fn db_a_deadline_nobody_can_wait_out_is_refused() {
        let at = |secs| DbLimits {
            query_timeout_secs: secs,
            ..DbLimits::default()
        };
        assert!(at(limits::DB_CALL.query_timeout_secs).validate().is_ok());
        for refused in [limits::DB_CALL.query_timeout_secs + 1, u64::MAX] {
            assert!(
                matches!(
                    at(refused).validate(),
                    Err(DbError::Invalid(InvalidDb::QueryTimeoutRange))
                ),
                "{refused}"
            );
        }
        // And the request carrying it says so before anything is opened.
        assert!(
            request(
                serde_json::json!({"operation":"tables","args":{"query_timeout_secs":u64::MAX}})
            )
            .is_err()
        );
        assert!(
            request(serde_json::json!({
                "operation":"execute",
                "args":{"statements":[{"sql":"DELETE FROM t"}],"query_timeout_secs":u64::MAX}
            }))
            .is_err()
        );
    }

    /// A section that grants `allow_write` may bound every write it allows,
    /// and a call may only be stricter than it.
    #[test]
    fn db_a_write_is_bounded_by_its_section_and_the_call_may_only_narrow_it() {
        let section = DbLimits {
            max_affected_rows: Some(10),
            ..DbLimits::default()
        };
        let write = |max: Option<u64>| {
            DbRequest::Execute(DbExecute {
                statements: vec![DbStatement {
                    sql: "DELETE FROM orders".into(),
                    params: vec![],
                }],
                max_affected_rows: max,
                query_timeout_secs: None,
                max_rows: None,
                max_result_bytes: None,
            })
        };
        // The section's bound applies to a call that names none.
        assert_eq!(write(None).limits(&section).max_affected_rows, Some(10));
        // A stricter call wins; a wider one does not lift the section's bound.
        assert_eq!(write(Some(3)).limits(&section).max_affected_rows, Some(3));
        assert_eq!(
            write(Some(100)).limits(&section).max_affected_rows,
            Some(10)
        );
        // A section that bounded nothing leaves the call to say.
        let open = DbLimits::default();
        assert_eq!(write(None).limits(&open).max_affected_rows, None);
        assert_eq!(write(Some(3)).limits(&open).max_affected_rows, Some(3));
        // Zero is not a bound, it is a refusal to run anything.
        assert!(write(Some(0)).validate().is_err());
        // A read changes nothing, so it reports no write bound: publishing the
        // section's own as this call's effective limit would name a bound that
        // applies to nothing the call does.
        let read = DbRequest::Tables(DbArgs::default());
        assert_eq!(read.limits(&section).max_affected_rows, None);
        assert!(
            !serde_json::to_string(&read.limits(&section))
                .expect("limits serialize")
                .contains("max_affected_rows"),
            "a read's effective limits name only what bounds a read"
        );
    }

    /// The published schema and the bound the code enforces are the same
    /// number: an agent builds its request from the schema.
    #[test]
    fn db_the_published_schema_names_the_bound_it_enforces() {
        let schema = serde_json::to_value(schemars::schema_for!(DbRequest))
            .expect("the request schema serializes");
        let text = schema.to_string();
        assert!(
            text.contains(&format!("\"maximum\":{}", limits::DB_CALL.max_list_page))
                || text.contains(&format!("\"maximum\":{}.0", limits::DB_CALL.max_list_page)),
            "the schema must publish the list page bound it enforces: {text}"
        );
    }

    #[test]
    fn db_limits_take_the_request_overrides_and_stay_positive() {
        let args = DbArgs {
            max_rows: Some(5),
            ..DbArgs::default()
        };
        let limits = DbLimits::default().overridden(&args);
        assert_eq!(limits.max_rows, 5);
        assert_eq!(limits.query_timeout_secs, 30);
        assert!(limits.validate().is_ok());
        assert!(
            DbLimits {
                max_rows: 0,
                ..DbLimits::default()
            }
            .validate()
            .is_err()
        );
    }
}
