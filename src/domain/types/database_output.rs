//! The serializable database result envelope, shared by every operation and
//! by the static plan `--dry-run` prints.
use super::database::{DbLimits, DbOperation};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DbKind {
    Db,
}

#[derive(Serialize, JsonSchema)]
pub struct DbOutput {
    #[schemars(range(min = 1, max = 1))]
    pub schema_version: u8,
    pub kind: DbKind,
    pub operation: DbOperation,
    /// The configured name, or `ad-hoc` for a SQLite file given as a path.
    pub target: String,
    pub meta: DbMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<bool>,
    #[serde(flatten)]
    pub result: Option<DbResult>,
    /// One entry per statement of a write, in the order they were sent. The
    /// statements themselves are not here: an index names them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub statements: Option<Vec<DbStatementResult>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
}

/// What one statement of a write did. The SQL and its parameters are not
/// reported: `index` is how a caller refers to them.
#[derive(Serialize, JsonSchema)]
pub struct DbStatementResult {
    pub index: usize,
    pub rows_affected: u64,
    /// The rows a statement returned, for `RETURNING` and for a `SELECT`
    /// inside the transaction. Bounded like any other result.
    #[serde(flatten)]
    pub result: Option<DbResult>,
}

#[derive(Serialize, JsonSchema)]
pub struct DbMeta {
    pub limits: DbLimits,
    pub engine: DbEngineInfo,
    /// Absent for SQLite, which is a file and not a server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// The database name, or the file path for SQLite.
    pub database: String,
    /// The bastion the connection went through, when it went through one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel: Option<DbTunnelInfo>,
    pub transaction: DbTransaction,
    /// What holds between the things one call returned.
    pub consistency: DbConsistency,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
    pub returned_rows: Option<usize>,
    pub returned_result_bytes: Option<usize>,
    /// The cursor that continues a listing, when one has more pages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub next_actions: Vec<DbNextAction>,
}

impl DbMeta {
    /// What is known before a connection is opened; `--dry-run` stops here.
    pub fn planned(
        operation: DbOperation,
        limits: DbLimits,
        engine: DbEngineInfo,
        database: String,
        host: Option<String>,
    ) -> Self {
        Self {
            limits,
            engine,
            host,
            database,
            tunnel: None,
            transaction: DbTransaction::planned(),
            consistency: DbConsistency::of(operation),
            observed_at: None,
            returned_rows: None,
            returned_result_bytes: None,
            next_cursor: None,
            next_actions: vec![],
        }
    }
}

/// The bastion a tunnelled connection went through. The local port is not
/// here: it is temporary, so it names nothing after the call.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct DbTunnelInfo {
    pub kind: String,
    pub instance_id: String,
    pub region: String,
}

#[derive(Serialize, JsonSchema)]
pub struct DbEngineInfo {
    pub name: String,
    /// Absent until a connection reports it, so `--dry-run` claims no version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_version: Option<String>,
}

/// How the read was guarded and how it ended. A read always ends without
/// leaving anything behind.
#[derive(Serialize, JsonSchema)]
pub struct DbTransaction {
    pub read_only: bool,
    /// Absent for SQLite, whose read guard is the read-only file handle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub isolation: Option<String>,
    pub outcome: DbOutcome,
}

impl DbTransaction {
    pub fn planned() -> Self {
        Self {
            read_only: true,
            isolation: None,
            outcome: DbOutcome::NotStarted,
        }
    }
}

/// What holds between the things one call returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DbConsistency {
    /// A read is one statement, so nothing it returned can be inconsistent
    /// with anything else it returned.
    SingleStatement,
    /// A write is one transaction of up to `limits::WRITE.statements`
    /// statements: consistent inside it, and all or nothing from outside.
    SingleTransaction,
}

impl DbConsistency {
    pub fn of(operation: DbOperation) -> Self {
        if operation.writes() {
            Self::SingleTransaction
        } else {
            Self::SingleStatement
        }
    }
}

#[derive(Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DbOutcome {
    NotStarted,
    RolledBack,
    Committed,
    /// The commit was sent and its answer never arrived.
    Unknown,
}

#[derive(Serialize, JsonSchema)]
pub struct DbNextAction {
    pub kind: DbKind,
    pub operation: DbOperation,
    pub message: String,
}

/// Why a result stopped before the statement ran out of rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DbStopReason {
    MaxRows,
    MaxResultBytes,
}

#[derive(Serialize, JsonSchema)]
pub struct DbResult {
    pub columns: Vec<DbColumn>,
    /// Every cell is a string or JSON null: an integer, a DECIMAL and a
    /// timestamp reach the caller exactly as the database stored them.
    pub rows: Vec<Vec<Value>>,
    pub row_count: usize,
    /// Whether the stream was cut. It is `stop_reason` said as a bool, and
    /// both are set by [`DbResult::new`], so a reader of the JSON cannot find
    /// one saying the answer is whole while the other names where it stopped.
    truncated: bool,
    stop_reason: Option<DbStopReason>,
    pub result_bytes: usize,
}

impl DbResult {
    /// The only way to build one. `truncated` is derived here rather than
    /// passed in, so no later caller can set the two apart.
    pub fn new(
        columns: Vec<DbColumn>,
        rows: Vec<Vec<Value>>,
        stop_reason: Option<DbStopReason>,
        result_bytes: usize,
    ) -> Self {
        Self {
            columns,
            row_count: rows.len(),
            rows,
            truncated: stop_reason.is_some(),
            stop_reason,
            result_bytes,
        }
    }

    /// Whether this is the whole answer.
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    pub fn stop_reason(&self) -> Option<DbStopReason> {
        self.stop_reason
    }
}

#[derive(Serialize, JsonSchema)]
pub struct DbColumn {
    pub name: String,
    /// The declared type. SQLite columns without a declaration have none.
    pub data_type: Option<String>,
    pub encoding: DbEncoding,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DbEncoding {
    Text,
    Base64,
}
