#![allow(unused_imports)]
//! Split out of the monolithic `hooks.rs` (mechanical module split, no
//! behaviour change). Wired back into the `hooks` namespace by mod.rs via
//! `pub(crate) use <name>::*;`.
use super::*;
use crate::*;

mod passthrough;
mod slices;
mod strip;
pub(crate) use passthrough::*;
pub(crate) use slices::*;
pub(crate) use strip::*;

// L31 exact-slice diff (extracted phase of `handle_read_pretool`). Path-keyed
// cache HIT with different bytes ⇒ the agent re-read after an edit; serve a
// unified diff against the prior bytes (win-ratio gated) or an "unchanged"
// marker. Stale-context prior content is re-served in full (registers the
// pointer and returns `None`). Returns `Some(0)` only when a diff/unchanged
// view is emitted; `None` to continue (the no-op fall-throughs preserve their
// original "update pointer / passthrough" behaviour by returning `None`).
// The `#[allow]` attribute that lived here left with its function: it no
// longer covered anything, and the rule for an `allow` is to name the item
// it exempts.

/// A diff is served instead of the file only when it saves at least this much.
///
/// A diff that is barely smaller than the file is worse than the file: the
/// agent gets the same bytes as context lines plus markers, and still has to
/// apply the change in its head to know the current state. Below the floor the
/// read passes through so the agent gets the canonical view, and the pointer
/// is still updated.
///
/// Three call sites had each written this test out inline, and one of them
/// used **20** where the other two used 40, with nothing saying why. That is
/// not a band nobody reaches: measured 2026-09-03 on a real source file over
/// 200-, 500- and 2000-line windows, a diff lands between 20 % and 39 % when
/// 11-14 % of the lines changed (36-38 % at 11-12 %). So the same edit was
/// answered with a diff on a paginated read and with the whole file on an
/// exact one, and `main.rs` told the agent the read path "keeps the diff only
/// when it saves at least 40 %", which was true for two paths out of three.
///
/// Outside that band the distribution is steeply bimodal — 96, 88, 79, 54,
/// then 15, 1, 0 — so the floor decides nothing for most edits, which is
/// exactly why a silent disagreement could sit here unnoticed.
pub(crate) const MIN_DIFF_WIN_PCT: usize = 40;

/// True when a diff of `diff_tokens` is worth serving in place of
/// `current_tokens` of file. The one place the floor is applied.
pub(crate) fn diff_is_worth_serving(current_tokens: usize, diff_tokens: usize) -> bool {
    win_pct(current_tokens, diff_tokens) >= MIN_DIFF_WIN_PCT
}

/// Percentage of `current_tokens` saved by serving `diff_tokens` instead.
/// Saturating and division-safe: an empty file is a 0 % win, never a panic.
pub(crate) fn win_pct(current_tokens: usize, diff_tokens: usize) -> usize {
    current_tokens
        .saturating_sub(diff_tokens)
        .checked_mul(100)
        .and_then(|n| n.checked_div(current_tokens))
        .unwrap_or(0)
}

/// Register content in the project store AND tag the delivering context —
/// the tag is the positive proof later "in your context" claims require.
fn register_and_tag(parsed: &serde_json::Value, cwd: &std::path::Path, hash: &str, content: &str) {
    tokenade_core::dedup_ref::register(cwd, hash, content);
    super::read_pretool::ctx_tag(parsed, hash);
}

/// One Read the agent is about to perform, as the three slice-diff strategies
/// see it.
///
/// The three `try_*_slice_diff` functions took **13, 9 and 12 parameters**, of
/// which eight were the same eight every time. Eight values threaded through
/// three call sites are not parameters, they are the request itself — the same
/// finding, and the same fix, as `hookwrap::PostToolInvocation`.
///
/// What stays an argument is what varies per STRATEGY: the dedup key each one
/// looks under, and the trimmed pair only the exact matcher uses. Folding those
/// in would make this struct two things at once.
///
/// Destructured at the top of each function rather than read as `req.x`
/// throughout: the bodies stay byte-for-byte what they were, and inline format
/// captures (`{file_path}`) keep working — `{req.file_path}` is not valid Rust.
/// Register a delivered view under every hash a later Read — or a Bash `cat` of
/// the same file — could arrive under, then credit the saving.
///
/// Six call sites carried these nine lines verbatim and the gain op was the
/// only thing that varied. The parameter list is flat rather than a context
/// struct because the six callers already are: building a struct at each site
/// would cost more lines than the block it replaces.
///
/// `try_exact_slice_diff` deliberately does LESS (a slice is not the whole
/// file, so it registers no path pointer) and is NOT folded in here.
fn register_view_and_credit(
    req: &ReadRequest<'_>,
    net_sav: Option<tokenade_core::Savings>,
    op: &str,
) {
    let ReadRequest {
        parsed,
        file_path,
        cwd,
        hash,
        content_hash,
        sliced_current,
        trimmed_hash,
        trimmed_current,
        path_key,
        ..
    } = *req;
    register_and_tag(parsed, cwd, hash, sliced_current);
    register_and_tag(parsed, cwd, content_hash, sliced_current);
    if trimmed_hash != content_hash {
        register_and_tag(parsed, cwd, trimmed_hash, trimmed_current);
    }
    tokenade_core::dedup_ref::register_path_pointer(cwd, path_key, content_hash);
    if let Some(net_sav) = net_sav {
        gain_log::record("hook", op, file_path, net_sav);
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ReadRequest<'a> {
    pub(crate) parsed: &'a serde_json::Value,
    pub(crate) file_path: &'a str,
    pub(crate) cwd: &'a std::path::Path,
    /// Hash of the SLICE the agent asked for.
    pub(crate) hash: &'a str,
    /// Hash of the whole file's content.
    pub(crate) content_hash: &'a str,
    pub(crate) sliced_current: &'a str,
    /// Hash of the slice with trailing newlines trimmed — a `cat` of the same
    /// file and a Read of it do not always agree on the final newline.
    pub(crate) trimmed_hash: &'a str,
    pub(crate) trimmed_current: &'a str,
    /// Dedup key for the path itself, independent of any slice.
    pub(crate) path_key: &'a str,
    pub(crate) offset: u64,
    pub(crate) limit: u64,
    pub(crate) started: std::time::Instant,
}

#[cfg(test)]
mod skeleton_precedence {
    /// The ladder returned as soon as one mode cleared its bar, so on a large,
    /// lightly commented file the skeleton was never reached.
    /// MEASURED on **50 043 real code files larger than 6 kB**:
    /// 246 364 926 tokens delivered out of 270 123 743 (−8 %), and **14 526 of
    /// them returned more than twice what the skeleton would have left** —
    /// 54 366 802 tokens too many. Under the rule: 191 998 124 (−28 %).
    #[test]
    fn a_skeleton_takes_over_only_when_it_wins_by_far() {
        use tokenade_core::read_view::a_skeleton_that_wins_by_far as wins;
        // Twice as good is NOT enough: it must be strictly better than twice,
        // because the skeleton omits bodies and the other modes do not.
        assert!(!wins(500, 1_000), "exactly half is not enough");
        assert!(wins(499, 1_000));
        // The measured case: `client_adapters.rs`, 8 815 vs 108 001.
        assert!(wins(8_815, 108_001));
        // A skeleton that wins almost nothing takes nobody's place.
        assert!(!wins(95_000, 108_001));
        // **Overflow must SATURATE.** `usize::MAX` does not tell the two
        // arithmetics apart (wrapping gives MAX−1, still too large): we need
        // the value that wraps to ZERO, where a non-saturating multiplication
        // would let the largest possible skeleton win against the smallest
        // fold.
        assert!(!wins(usize::MAX / 2 + 1, 10), "doubling must saturate");
        assert!(!wins(usize::MAX, 10));
    }

    /// The skeleton must beat the BETTER of the two ordinary modes. Compared
    /// to the first one that clears its bar, a mediocre comments mode is
    /// enough to let it in ahead of a lite mode that did better.
    #[test]
    fn the_skeleton_must_beat_the_better_of_the_two_ordinary_modes() {
        use tokenade_core::read_view::the_better_ordinary_mode as better;
        assert_eq!(
            better(900, 400),
            400,
            "lite was winning, it is the reference"
        );
        assert_eq!(better(400, 900), 400);
        assert_eq!(better(400, 400), 400);
        // The distinguishing case: the skeleton beats 900 but not 400, so it
        // must NOT go first.
        use tokenade_core::read_view::a_skeleton_that_wins_by_far as wins;
        assert!(wins(300, 900), "it would beat the weaker mode");
        assert!(
            !wins(300, better(900, 400)),
            "but not the better of the two"
        );
    }
}
