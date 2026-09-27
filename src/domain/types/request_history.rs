//! One line of the explorer's request history: the operation, the parameter
//! values that are not secret, the body, and a name for a favorite. Never
//! the response and never a credential.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// The operation id (`Operation::id`).
    pub operation: String,
    #[serde(default)]
    pub params: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Set on a favorite.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}
