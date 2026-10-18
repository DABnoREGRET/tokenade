//! Bounded Levenshtein edit distance for typo-tolerant symbol search.
//!
//! A standard bounded-Levenshtein edit distance.
//! A user typing `creatUser` should still find `createUser`. Without a
//! bound, full Levenshtein is O(m·n) — too slow on a 30k-symbol index
//! when called per candidate. The bounded variant rejects pairs that
//! can't possibly fit within `max` edits using row-min pruning, and
//! exits early when the lower bound on any row exceeds `max`.
//!
//! Returns `Some(distance)` when distance ≤ `max`, else `None`. Caller
//! treats `None` as "not similar enough — don't suggest".

/// Compute the Levenshtein edit distance between `a` and `b`, but only
/// up to `max`. Returns `None` if the actual distance exceeds `max` so
/// the caller can early-exit without examining ranked siblings.
pub fn bounded(a: &str, b: &str, max: usize) -> Option<usize> {
    let av: Vec<char> = a.chars().collect();
    let bv: Vec<char> = b.chars().collect();
    let (m, n) = (av.len(), bv.len());

    // Fast-path: length difference alone exceeds the budget.
    if m.abs_diff(n) > max {
        return None;
    }
    // Empty cases.
    if m == 0 {
        return (n <= max).then_some(n);
    }
    if n == 0 {
        return (m <= max).then_some(m);
    }

    // Two-row DP. `prev[j]` = distance from a[..i-1] to b[..j].
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr: Vec<usize> = vec![0; n + 1];

    for i in 1..=m {
        curr[0] = i;
        let mut row_min = usize::MAX;
        for j in 1..=n {
            let cost = if av[i - 1] == bv[j - 1] { 0 } else { 1 };
            curr[j] = (curr[j - 1] + 1).min(prev[j] + 1).min(prev[j - 1] + cost);
            row_min = row_min.min(curr[j]);
        }
        // If even the minimum cell on this row is already over budget,
        // every subsequent cell will be ≥ that minimum (DP monotonicity),
        // so we know the final answer cannot fit. Bail.
        if row_min > max {
            return None;
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    let d = prev[n];
    (d <= max).then_some(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_is_zero() {
        assert_eq!(bounded("foo", "foo", 2), Some(0));
    }

    #[test]
    fn empty_strings() {
        assert_eq!(bounded("", "", 0), Some(0));
        assert_eq!(bounded("", "abc", 5), Some(3));
        assert_eq!(bounded("abc", "", 5), Some(3));
        assert_eq!(bounded("abc", "", 2), None);
    }

    #[test]
    fn typical_typo_within_budget() {
        // create → creat (deletion at end)
        assert_eq!(bounded("create", "creat", 1), Some(1));
        // creatUser → createUser (insertion in middle)
        assert_eq!(bounded("creatUser", "createUser", 1), Some(1));
        // teh → the (transposition counts as 2 in plain Levenshtein)
        assert_eq!(bounded("teh", "the", 2), Some(2));
    }

    #[test]
    fn length_diff_pruning() {
        assert_eq!(bounded("a", "abcdefghij", 3), None);
    }

    #[test]
    fn row_min_pruning() {
        // Totally different strings of equal length: distance ≈ length.
        assert_eq!(bounded("zzzz", "aaaa", 2), None);
    }

    #[test]
    fn unicode_is_per_char() {
        // 2 char insertions, both 4-byte sequences. Byte length would
        // be 8 but char-level distance is 2.
        assert_eq!(bounded("a", "a🦀🦀", 2), Some(2));
        assert_eq!(bounded("a", "a🦀🦀", 1), None);
    }
}
