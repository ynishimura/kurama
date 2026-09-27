//! One line of the audit log: what a call of `api`, `exec`, `db` or `data` was and how it ended, never a secret, a query string, a header or a body.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A call, as `kurama audit` lists it. Every field is something kurama
/// decided or was named on the command line and holds no value a person
/// would keep secret: an `exec` keeps its program and not its arguments,
/// `db` / `data` keep the SHA-256 of their SQL and not the SQL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub time: DateTime<Utc>,
    /// `api`, `exec`, `db` or `data`.
    pub command: String,
    /// The API, credential source, database or workspace named.
    pub target: String,
    /// Whether `KURAMA_AGENT` marked the run as an agent's.
    pub agent: bool,
    /// The HTTP method of the request `api` built.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// Its URL path, without the query.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The HTTP status of the last response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// `exec`: the program, argv[0].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    /// `db` / `data`: `sha256:<hex>` of the SQL written on the command line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sql_sha256: Option<String>,
    /// `None` when kurama replaced itself with the `exec` command, whose own
    /// exit code kurama never sees.
    pub exit_code: Option<u8>,
    /// The `error[CODE]` a failed call ended with, such as
    /// `AGENT_POLICY_DENIED`: a name, never the message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    pub duration_ms: u64,
}
