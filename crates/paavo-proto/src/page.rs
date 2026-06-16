//! Generic pagination envelope for list endpoints.
use serde::{Deserialize, Serialize};

/// One page of a list endpoint plus the metadata the client needs for
/// pagination + live updates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page<T> {
    /// The rows on this page.
    pub items: Vec<T>,
    /// Total rows across all pages (after filtering, before paging).
    pub total: u64,
    /// 1-based page number echoed back.
    pub page: u32,
    /// Page size echoed back.
    pub per_page: u32,
    /// Resource revision at query time (for live de-dup).
    pub revision: u64,
    /// Jobs newer than `as_of` (0 for boards/schedules/search mode).
    pub new_count: u64,
    /// Echoed jobs cursor (epoch-ms); None for boards/schedules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_of: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn page_roundtrips() {
        let p = Page {
            items: vec![1u32, 2, 3],
            total: 3,
            page: 1,
            per_page: 50,
            revision: 7,
            new_count: 0,
            as_of: Some(123),
        };
        let j = serde_json::to_string(&p).unwrap();
        assert_eq!(p, serde_json::from_str::<Page<u32>>(&j).unwrap());
    }
}
