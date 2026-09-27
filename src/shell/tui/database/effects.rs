//! Database explorer effects: what the runtime does for the pure update.
//!
//! Nothing here derives `Debug`: an ask can carry the SQL a person typed,
//! and SQL is never logged.

/// One request to the task that holds the connection. At most one runs.
#[derive(Clone, PartialEq, Eq)]
pub enum DbAsk {
    /// The next page of tables and views, after the `(schema, name)` the
    /// previous page ended on.
    Tables { after: Option<(String, String)> },
    /// The columns of one table.
    Describe { schema: String, table: String },
    /// The first rows of one table.
    Preview { schema: String, table: String },
    /// The statement a person typed.
    Query { sql: String },
}

impl DbAsk {
    /// How the screen names what is running.
    pub fn label(&self) -> String {
        match self {
            Self::Tables { .. } => "listing tables".to_owned(),
            Self::Describe { table, .. } => format!("reading the columns of {table}"),
            Self::Preview { table, .. } => format!("previewing {table}"),
            Self::Query { .. } => "running the statement".to_owned(),
        }
    }
}

pub enum DbEffect {
    /// Send the request to the connection's task.
    Ask(DbAsk),
    /// Stop the running statement (`Esc`).
    Stop,
    CopyToClipboard {
        text: String,
    },
    /// Open `$EDITOR` on the SQL, as a `.sql` file.
    EditSql {
        text: String,
    },
}
