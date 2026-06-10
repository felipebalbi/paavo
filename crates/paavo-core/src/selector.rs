//! Selector-matches-any helper used at enqueue time.

use paavo_proto::{BoardSelector, BoardSpec};

/// Returns `true` if at least one board in `inventory` satisfies `sel`.
/// Ignores health (per spec §5.5: rejection is for impossible selectors,
/// not for transient un-availability).
pub fn selector_matches_any(sel: &BoardSelector, inventory: &[BoardSpec]) -> bool {
    inventory.iter().any(|b| sel.matches(b))
}
