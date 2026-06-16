//! In-memory jobs index: lightweight rows + fuzzy search, refreshed by
//! the background poller. The index is the single observation point for
//! the live jobs feed and the search endpoint.
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use paavo_proto::JobListItem;

/// In-memory jobs index: list items plus a lowercased fuzzy haystack each.
#[derive(Default, Clone)]
pub struct JobIndex {
    items: Vec<JobListItem>,
    haystacks: Vec<String>,
}

impl JobIndex {
    /// Build an index from newest-first list items.
    pub fn from_items(items: Vec<JobListItem>) -> Self {
        let haystacks = items.iter().map(haystack).collect();
        Self { items, haystacks }
    }

    /// Return `(page_items, total)`. Blank `q` => time-ordered (the items
    /// are already newest-first), optionally pinned to `submitted_at <= as_of`.
    /// Non-blank `q` => fuzzy-ranked (score desc, newest-first tiebreak).
    pub fn search(
        &self,
        q: &str,
        as_of: Option<i64>,
        page: u32,
        per_page: u32,
    ) -> (Vec<JobListItem>, u64) {
        let matched: Vec<&JobListItem> = if q.trim().is_empty() {
            self.items
                .iter()
                .filter(|it| as_of.is_none_or(|t| it.submitted_at <= t))
                .collect()
        } else {
            let m = SkimMatcherV2::default();
            let mut scored: Vec<(i64, usize, &JobListItem)> = self
                .items
                .iter()
                .enumerate()
                .filter_map(|(i, it)| m.fuzzy_match(&self.haystacks[i], q).map(|s| (s, i, it)))
                .collect();
            // score desc; stable tiebreak by original index (already newest-first)
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            scored.into_iter().map(|(_, _, it)| it).collect()
        };
        let total = matched.len() as u64;
        let start = (page.saturating_sub(1) as usize) * per_page as usize;
        let items = matched
            .into_iter()
            .skip(start)
            .take(per_page as usize)
            .cloned()
            .collect();
        (items, total)
    }

    /// Count of jobs newer than `as_of` (drives the "N new" pill). 0 when
    /// `as_of` is None.
    pub fn new_count(&self, as_of: Option<i64>) -> u64 {
        match as_of {
            Some(t) => self.items.iter().filter(|it| it.submitted_at > t).count() as u64,
            None => 0,
        }
    }
}

fn haystack(it: &JobListItem) -> String {
    format!(
        "{} {} {:?} {}",
        it.id,
        it.submitter,
        it.state,
        it.board_id.as_deref().unwrap_or("")
    )
    .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use paavo_proto::{JobId, JobState, Priority};

    fn item(submitter: &str, board: &str, state: JobState, submitted_at: i64) -> JobListItem {
        JobListItem {
            id: JobId::new(),
            state,
            priority: Priority::Interactive,
            submitter: submitter.into(),
            board_id: Some(board.into()),
            submitted_at,
        }
    }

    /// Newest-first fixture: alice (newest) / bob / cron (oldest).
    fn sample_index() -> JobIndex {
        JobIndex::from_items(vec![
            item("alice", "mcxa266-01", JobState::Running, 3_000),
            item("bob", "mcxa266-02", JobState::Passed, 2_000),
            item("cron", "mcxa266-03", JobState::Building, 1_000),
        ])
    }

    #[test]
    fn blank_query_returns_all_in_order() {
        let idx = sample_index();
        let (items, total) = idx.search("", None, 1, 50);
        assert_eq!(total, 3);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].submitter, "alice");
        assert_eq!(items[2].submitter, "cron");
    }

    #[test]
    fn fuzzy_query_ranks_match_first() {
        let idx = sample_index();
        let (items, total) = idx.search("alice", None, 1, 50);
        assert_eq!(total, 1);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].submitter, "alice");
    }

    #[test]
    fn blank_query_pages() {
        let idx = sample_index();
        // page 2 of size 2 over 3 items => the single remaining (oldest) row.
        let (items, total) = idx.search("", None, 2, 2);
        assert_eq!(total, 3);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].submitter, "cron");
    }

    #[test]
    fn new_count_counts_strictly_newer() {
        let idx = sample_index();
        assert_eq!(idx.new_count(None), 0);
        assert_eq!(idx.new_count(Some(3_000)), 0, "boundary is exclusive");
        assert_eq!(idx.new_count(Some(2_000)), 1);
        assert_eq!(idx.new_count(Some(0)), 3);
    }
}
