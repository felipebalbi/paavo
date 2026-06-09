//! Stable identifier types.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// A job identifier. ULID under the hood: lexicographically sortable by
/// creation time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JobId(ulid::Ulid);

impl JobId {
    /// Generate a new job id from the current system time.
    pub fn new() -> Self {
        Self(ulid::Ulid::new())
    }

    /// Return the underlying ULID.
    pub fn as_ulid(&self) -> ulid::Ulid {
        self.0
    }
}

impl Default for JobId {
    /// Generates a fresh ULID. Beware: this draws randomness on every call,
    /// so `#[derive(Default)]` on a struct that embeds a `JobId` will assign
    /// a new id on each default-construction. That's the intent for top-level
    /// `JobId::default()`, but is rarely what you want in aggregate types.
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for JobId {
    type Err = ulid::DecodeError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(ulid::Ulid::from_str(s)?))
    }
}
