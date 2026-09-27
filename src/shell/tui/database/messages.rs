//! Database explorer messages: keys, the clock of a running request and the
//! results of effects. Nothing here derives `Debug`: SQL and rows are never
//! logged.

use crossterm::event::KeyEvent;

use super::effects::DbAsk;
use crate::domain::types::database::DbResult;

pub enum DbMessage {
    Key(KeyEvent),
    Resize,
    /// Whole seconds the running request has taken; sent once a second
    /// while one runs.
    Elapsed(u64),
    /// The answer to the running request.
    Answered(Answer),
    Copied(Result<(), String>),
    /// The SQL after `$EDITOR`.
    SqlEdited(Result<String, String>),
}

pub struct Answer {
    pub ask: DbAsk,
    pub outcome: Result<Answered, Failure>,
    pub elapsed_ms: u64,
}

pub enum Answered {
    /// A page of the table list and the key the next page starts after.
    Listing {
        result: DbResult,
        next: Option<(String, String)>,
    },
    Rows(DbResult),
}

/// Why a request failed, as the screen shows it.
pub struct Failure {
    /// The code, the message and the hint.
    pub message: String,
    /// The person asked for the stop (`Esc`): the failure is what they
    /// wanted, so it is a line under the result and not an error box.
    pub stopped: bool,
}
