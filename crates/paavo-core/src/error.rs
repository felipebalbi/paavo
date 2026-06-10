//! Errors returned by paavo-core operations.

use thiserror::Error;

/// All errors surfaced by paavo-core public API.
#[derive(Debug, Error)]
pub enum CoreError {
    /// paavo-db error.
    #[error("db: {0}")]
    Db(#[from] paavo_db::DbError),
    /// paavo-build error.
    #[error("build: {0}")]
    Build(#[from] paavo_build::BuildError),
    /// Selector matched no possible board in the inventory (per spec §5.5,
    /// rejected at enqueue time, not silently queued).
    #[error("selector matches no board in inventory: {0:?}")]
    SelectorNeverMatches(paavo_proto::BoardSelector),
    /// Requested hard-max exceeds the daemon ceiling.
    #[error("requested hard_max_ms {requested} exceeds daemon ceiling {ceiling}")]
    OverCeiling {
        /// What was asked.
        requested: u64,
        /// Daemon-configured ceiling.
        ceiling: u64,
    },
    /// Cancel was issued in a state where it doesn't apply.
    #[error("cannot cancel job in state {state:?}")]
    NotCancellable {
        /// State the job was in.
        state: paavo_proto::JobState,
    },
}

/// Result alias.
pub type Result<T, E = CoreError> = std::result::Result<T, E>;
