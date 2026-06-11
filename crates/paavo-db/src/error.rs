//! Error type for paavo-db.

use thiserror::Error;

/// Errors returned by paavo-db operations.
#[derive(Debug, Error)]
pub enum DbError {
    /// Underlying SQLite error (catch-all for low-level rusqlite failures
    /// we don't yet pattern-match into a typed variant).
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
    /// A typed entity was looked up or mutated by id but did not exist.
    /// Surfaces to HTTP as `404 Not Found`.
    #[error("{entity} not found: {id}")]
    NotFound {
        /// Logical entity name (e.g. `"board"`, `"job"`).
        entity: &'static str,
        /// Id we looked for.
        id: String,
    },
    /// A typed entity was inserted but its primary key already exists.
    /// Surfaces to HTTP as `409 Conflict`.
    #[error("{entity} already exists: {id}")]
    AlreadyExists {
        /// Logical entity name.
        entity: &'static str,
        /// The duplicate id.
        id: String,
    },
    /// A typed entity could not be mutated because it conflicts with
    /// existing state (e.g. delete refused while related rows exist).
    /// Surfaces to HTTP as `409 Conflict`.
    #[error("{entity} {id} conflicts with existing state: {reason}")]
    Conflict {
        /// Logical entity name.
        entity: &'static str,
        /// Id of the row.
        id: String,
        /// Human-readable reason.
        reason: String,
    },
}

/// `Result` alias used throughout paavo-db.
pub type Result<T, E = DbError> = std::result::Result<T, E>;
