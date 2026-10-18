//! Central truncation caps + helpers.
//!
//! Ported from rtk `src/core/truncate.rs:1-21`. Compactors used to
//! hardcode their own per-format caps (`200` for errors here, `60` for
//! list entries there, `50` for "top items" yonder), which made it
//! impossible to tune the agent's verbose-vs-terse balance without
//! grepping for magic numbers across 50 files. This module centralises
//! the four tiers everyone agreed on and adds the `reduced()` helper
//! for lower-priority data classes within a single compactor.
//!
//! ## Tier semantics
//!
//! - **`CAP_ERRORS = 200`** — error-class items. Things the agent
//!   absolutely needs to see to fix the problem. Truncation here means
//!   "we hit the cap; the rest are likely similar to the last shown".
//! - **`CAP_WARNINGS = 100`** — warnings + non-fatal noise. Useful but
//!   the agent can usually proceed without seeing all of them.
//! - **`CAP_LIST = 60`** — generic list outputs (`ls -la`, package
//!   names, file paths). Beyond 60, ellipsis the rest.
//! - **`CAP_INVENTORY = 50`** — top-N inventories (top files by symbol
//!   count, top routes by handler size). 50 is enough to navigate a
//!   medium repo; "show all" is a follow-up query.
//!
//! ## Reduction helper
//!
//! When one compactor surfaces several data classes (e.g., logs has
//! ERROR + WARN + INFO), the lower-priority class gets `reduced(cap, n)`
//! so the agent sees relatively fewer of them. Saturating subtract
//! prevents accidentally returning 0 (would silently hide the entire
//! class — a worse failure mode than showing too few).

pub const CAP_ERRORS: usize = 200;
pub const CAP_WARNINGS: usize = 100;
pub const CAP_LIST: usize = 60;
pub const CAP_INVENTORY: usize = 50;

/// Reduce a cap by `offset`, with a floor of `1`. Used when a compactor
/// has a clear priority order between data classes — the lower-priority
/// one gets less room without disappearing entirely.
///
/// ```
/// # use tk_fold::caps::{CAP_ERRORS, reduced};
/// assert_eq!(reduced(CAP_ERRORS, 50), 150);
/// // Floor at 1, never 0.
/// assert_eq!(reduced(10, 99), 1);
/// ```
pub const fn reduced(cap: usize, offset: usize) -> usize {
    if cap > offset + 1 {
        cap - offset
    } else {
        1
    }
}

/// Truncate `s` to at most `max_bytes` bytes, respecting UTF-8 char boundaries.
pub fn truncate_at_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_values_match_documentation() {
        assert_eq!(CAP_ERRORS, 200);
        assert_eq!(CAP_WARNINGS, 100);
        assert_eq!(CAP_LIST, 60);
        assert_eq!(CAP_INVENTORY, 50);
    }

    #[test]
    fn truncate_at_char_boundary_never_splits_multibyte() {
        // A byte cap mid-codepoint must back off, not panic, on non-Latin text.
        let cjk = "令牌优化".repeat(50); // 3-byte chars
        let emoji = "😀🎉".repeat(50); // 4-byte chars
        for sample in [&cjk, &emoji] {
            for cap in 0..=24 {
                let out = truncate_at_char_boundary(sample, cap); // must not panic
                assert!(sample.starts_with(out));
                assert!(out.len() <= cap);
            }
        }
        // ASCII and already-short inputs pass through unchanged.
        assert_eq!(truncate_at_char_boundary("hello", 3), "hel");
        assert_eq!(truncate_at_char_boundary("hi", 99), "hi");
    }

    #[test]
    fn reduced_clamps_to_one() {
        assert_eq!(reduced(10, 0), 10);
        assert_eq!(reduced(10, 5), 5);
        assert_eq!(reduced(10, 9), 1);
        assert_eq!(reduced(10, 999), 1);
        assert_eq!(reduced(0, 0), 1);
    }
}
