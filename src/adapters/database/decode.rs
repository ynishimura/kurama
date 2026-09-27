//! What every engine's value reader answers with, and how a failure to read
//! one is named.
use crate::domain::types::database::{DbEncoding, DbError, DbFailure};

/// One value, and whether it had to be base64 to stay intact.
pub type Decoded = (String, DbEncoding);

#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// kurama has no reader for this type; the caller can cast it in the
    /// statement.
    Unsupported,
    /// The server sent a value of a type kurama reads, in a shape it does not
    /// have. Silently reporting an empty cell would hide it.
    Unreadable,
}

impl DecodeError {
    pub fn named(self, column: &str, data_type: &str) -> DbError {
        match self {
            // The column name and the type name are the database's own text.
            Self::Unsupported => DbFailure::UnsupportedType {
                column: column.into(),
                data_type: data_type.into(),
            },
            Self::Unreadable => DbFailure::UnreadableValue {
                column: column.into(),
                data_type: data_type.into(),
            },
        }
        .into()
    }
}
