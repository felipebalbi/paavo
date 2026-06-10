//! Error type for paavo-db.

use thiserror::Error;

/// Errors returned by paavo-db operations.
#[derive(Debug, Error)]
pub enum DbError {
    /// Underlying SQLite error.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Migration application failed.
    #[error("migration: {0}")]
    Migration(#[from] refinery::Error),
    /// JSON column failed to (de)serialize.
    #[error("json column: {0}")]
    Json(#[from] serde_json::Error),
    /// Row found but a CHECK-constrained string value was unrecognized.
    #[error("unknown enum variant for column {column}: {value}")]
    UnknownEnum {
        /// SQL column name.
        column: &'static str,
        /// Value pulled from the row.
        value: String,
    },
}

/// `Result` alias used throughout paavo-db.
pub type Result<T, E = DbError> = std::result::Result<T, E>;
