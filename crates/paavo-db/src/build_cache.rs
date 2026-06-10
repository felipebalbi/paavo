//! build_cache table helpers (filled in by Task 1.3.e).

/// Row representation of the `build_cache` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildCacheEntry;

/// Aggregate stats for the build-cache LRU policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildCacheStats;
