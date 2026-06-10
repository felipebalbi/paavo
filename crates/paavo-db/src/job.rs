//! Job table helpers (filled in by Task 1.3.c).

/// Row representation of the `job` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRow;

/// Insert-time job representation (no state, no outcome).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewJob;

/// Captured terminal outcome to record when transitioning to a terminal state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutcomeRecord;
