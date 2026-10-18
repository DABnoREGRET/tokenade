//! Levenshtein edit distance — used by the CLI for "did you mean?" hints on
//! typo'd command/profile/agent/tool names (#414, #614, #622, #638, #639, #640).
//!
//! Single-vector dynamic programming so the allocation footprint stays at
//! O(min(a, b)) words. Tight enough for the typo-hint hot path where we
//! score the user's input against a small set of candidate names.
//!
//! Consolidated in #641: cli/main.rs and tokenade-mcp/src/lib.rs each had
//! an identical 20-line impl. One source of truth keeps any future tweak
//! (UTF-8 grapheme awareness, damerau-levenshtein, etc.) from drifting.

/// Edit distance (insertions, deletions, substitutions count 1) between
/// `a` and `b`. Empty strings handled. Byte-level (not grapheme-aware) —
/// the only call sites compare ASCII identifiers, so this is enough.
pub fn levenshtein(a: &str, b: &str) -> usize {
    let (ab, bb) = (a.as_bytes(), b.as_bytes());
    if ab.is_empty() {
        return bb.len();
    }
    if bb.is_empty() {
        return ab.len();
    }
    let mut prev: Vec<usize> = (0..=bb.len()).collect();
    let mut curr: Vec<usize> = vec![0; bb.len() + 1];
    for (i, &ai) in ab.iter().enumerate() {
        curr[0] = i + 1;
        for (j, &bj) in bb.iter().enumerate() {
            let cost = if ai == bj { 0 } else { 1 };
            curr[j + 1] = (curr[j] + 1).min(prev[j + 1] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[bb.len()]
}

/// Typo-distance: min(levenshtein(typed, candidate), levenshtein(typed, candidate[..len(typed)+1])).
///
/// The prefix-window trick makes long candidate names (e.g. `--install-path`)
/// not penalise a short mis-typed prefix (e.g. `--isntall`) more than necessary.
/// Empty `typed` falls back to plain Levenshtein — the window optimisation would
/// otherwise return distance 1 for every candidate.
pub fn typo_distance(typed: &str, candidate: &str) -> usize {
    let full = levenshtein(typed, candidate);
    if typed.is_empty() {
        return full;
    }
    let window = typed.len() + 1;
    if let Some(prefix) = candidate.get(..window) {
        return full.min(levenshtein(typed, prefix));
    }
    full
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_empty_strings() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", ""), 3);
    }

    #[test]
    fn handles_exact_match() {
        assert_eq!(levenshtein("auto_compact", "auto_compact"), 0);
    }

    #[test]
    fn one_char_substitution_is_distance_one() {
        // dash-vs-underscore typo — the common CLI mis-key.
        assert_eq!(levenshtein("auto-compact", "auto_compact"), 1);
        // single substitution.
        assert_eq!(levenshtein("cat", "bat"), 1);
    }

    #[test]
    fn deletion_and_insertion_distance() {
        assert_eq!(levenshtein("autocompact", "auto_compact"), 1);
        assert_eq!(levenshtein("auto_compact", "autocompact"), 1);
    }

    #[test]
    fn transposition_costs_two_substitutions() {
        // levenshtein (not damerau) treats AB→BA as 2 substitutions.
        assert_eq!(levenshtein("ab", "ba"), 2);
    }
}
