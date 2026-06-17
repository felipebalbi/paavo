//! Shared `LIKE` helpers: wildcard escaping and the subsequence pattern
//! builder used by the fuzzy-search queries in `job.rs` and the fleet
//! filter in `board.rs`. Kept in one place so the escape rules (`%`, `_`,
//! `\`, paired with `ESCAPE '\'`) never drift between call sites.

/// Escape `LIKE` wildcards so `%`, `_`, and `\` match literally. Pair the
/// result with `ESCAPE '\'` in the query.
pub(crate) fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '%' || c == '_' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Build a subsequence `LIKE` pattern from `query`: each character is
/// escaped and separated by `%`, so `"almcx"` becomes `"%a%l%m%c%x%"`.
/// `LIKE` against this pattern is true iff the characters appear in order
/// — the same membership test `SkimMatcherV2` uses. An empty query yields
/// `"%"` (matches everything). The caller lowercases `query` first so the
/// pattern and the `fuzzy_score` needle agree.
// Introduced ahead of its consumer: the only non-test caller is the
// fuzzy-search query in `job.rs`, added later in this series. Until that
// lands, the non-test build sees this helper as dead code, so allow it
// explicitly to keep `clippy -D warnings` green; drop the attribute once
// `job.rs` calls it.
#[allow(dead_code)]
pub(crate) fn subsequence_pattern(query: &str) -> String {
    let mut p = String::from("%");
    for c in query.chars() {
        if c == '%' || c == '_' || c == '\\' {
            p.push('\\');
        }
        p.push(c);
        p.push('%');
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_like_escapes_wildcards() {
        assert_eq!(escape_like("a%b_c\\d"), "a\\%b\\_c\\\\d");
        assert_eq!(escape_like("plain"), "plain");
    }

    #[test]
    fn subsequence_pattern_interleaves_percents() {
        assert_eq!(subsequence_pattern("almcx"), "%a%l%m%c%x%");
        assert_eq!(subsequence_pattern(""), "%");
        assert_eq!(subsequence_pattern("a%"), "%a%\\%%");
    }
}
