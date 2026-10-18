//! `read_mode` — the named read-mode ladder (`aggressive` / `task` /
//! `reference` / `entropy`), a single opt-in knob that tunes how
//! aggressively tokenade folds long listings and read output.
//!
//! Selected via the `TOKENADE_READ_MODE` environment variable, read once
//! per process and cached. The ladder is **opt-in**: the default
//! (`task`, or an unset/unknown value) is byte-for-byte identical to the
//! pre-ladder behaviour, so nothing changes unless the user asks for it.
//! That deliberate default matters — aggressive brevity has been shown to
//! degrade coding/reasoning quality, so terseness is never forced on.
//!
//! The ladder, from terse to verbose:
//!   * `aggressive` — keep far fewer lines (0.4× the head budget). Max
//!     brevity; for cheap models / tight context windows.
//!   * `task`       — the balanced default (1.0×). Current behaviour.
//!   * `reference`  — keep much more (3.0×) for "I need the whole picture"
//!     moments (reading a config, a schema, a full diff).
//!   * `entropy`    — keep the *most information-dense* lines rather than
//!     simply the first N: each candidate line is scored by Shannon
//!     entropy × length, and the top-budget lines are kept **in their
//!     original order**. Blank / near-uniform lines lose to dense ones.
//!
//! Threaded at the fold layer (`cmd::linux::git_listing::fold` /
//! `fold_paths`) so a single env knob governs every listing compactor
//! uniformly — there is one source of truth, not a per-command flag.

use std::sync::OnceLock;

/// Never fold below this many lines, regardless of an aggressive scale —
/// a floor so `aggressive` on an already-small budget can't collapse the
/// output to nothing. Capped to the base budget for tiny compactors so the
/// floor never *raises* a sub-floor budget (which would fold *less*).
const MIN_HEAD: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadMode {
    Aggressive,
    Task,
    Reference,
    Entropy,
}

impl ReadMode {
    /// Parse a mode name (case-insensitive, surrounding whitespace
    /// ignored). Unknown / empty values fall back to [`ReadMode::Task`]
    /// so a typo silently degrades to the safe default rather than erroring.
    pub fn parse(s: &str) -> ReadMode {
        Self::parse_checked(s).unwrap_or(ReadMode::Task)
    }

    /// The same parse, but saying whether the value was RECOGNISED.
    ///
    /// [`parse`](Self::parse) folds a typo into the default on purpose — no
    /// command should fail because a shell exports a misspelled knob — and the
    /// cost of that kindness is that nothing ever says the knob did nothing.
    /// MEASURED 2026-09-04: `TOKENADE_READ_MODE=agressive` (one `g`, the
    /// French spelling) left every fold at `task`, and `tokenade read-mode` —
    /// the surface that exists to make this knob discoverable — answered
    /// `active: task (TOKENADE_READ_MODE=agressive)`, echoing the value back
    /// beside a mode it did not select. The status surface asks this instead,
    /// so the parse can stay forgiving and the report can still be honest.
    ///
    /// One function, two answers: the fallback cannot drift from the list of
    /// rungs, because there is only one list.
    pub fn parse_checked(s: &str) -> Option<ReadMode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "aggressive" | "aggro" | "terse" => Some(ReadMode::Aggressive),
            "task" | "balanced" | "default" => Some(ReadMode::Task),
            "reference" | "ref" | "full" => Some(ReadMode::Reference),
            "entropy" | "dense" => Some(ReadMode::Entropy),
            _ => None,
        }
    }

    /// Every spelling this knob accepts, for a message that has to list them.
    pub const RUNG_SPELLINGS: &'static str =
        "aggressive | task | reference | entropy (aliases: aggro, terse, balanced, default, ref, full, dense)";

    /// Multiplier applied to a compactor's head budget. `task` and
    /// `entropy` keep the same *count* (entropy changes *which* lines, not
    /// how many); `aggressive` shrinks it, `reference` grows it.
    pub fn head_scale(self) -> f64 {
        match self {
            ReadMode::Aggressive => 0.4,
            ReadMode::Task => 1.0,
            ReadMode::Reference => 3.0,
            ReadMode::Entropy => 1.0,
        }
    }

    /// Lowercase canonical name (for banners / status output).
    pub fn label(self) -> &'static str {
        match self {
            ReadMode::Aggressive => "aggressive",
            ReadMode::Task => "task",
            ReadMode::Reference => "reference",
            ReadMode::Entropy => "entropy",
        }
    }

    pub fn is_entropy(self) -> bool {
        matches!(self, ReadMode::Entropy)
    }
}

/// The process-wide active mode, read from `TOKENADE_READ_MODE` once and
/// cached. Cached (not re-read per call) so a fold-heavy run is cheap and
/// the mode is stable for the whole process even if the env mutates.
pub fn active() -> ReadMode {
    static MODE: OnceLock<ReadMode> = OnceLock::new();
    *MODE.get_or_init(|| {
        std::env::var("TOKENADE_READ_MODE")
            .map(|v| ReadMode::parse(&v))
            .unwrap_or(ReadMode::Task)
    })
}

/// Scale a compactor's base head budget by the active mode. `task` returns
/// the base unchanged (identity fast-path — guarantees byte-identical
/// output when the ladder is unused). Other modes scale then clamp into
/// `[min(MIN_HEAD, base), base*4]` so the result is always sane.
pub fn effective_head(base: usize) -> usize {
    scale_with(active(), base)
}

/// Pure core of [`effective_head`]: scale `base` by an explicit `mode`,
/// no env / cache involved (so it unit-tests deterministically).
pub fn scale_with(mode: ReadMode, base: usize) -> usize {
    if mode == ReadMode::Task {
        return base; // identity — the overwhelmingly common path.
    }
    let scaled = (base as f64 * mode.head_scale()).round() as usize;
    let floor = MIN_HEAD.min(base);
    let ceil = base.saturating_mul(4).max(base);
    scaled.clamp(floor, ceil)
}

/// Banner mode-tag suffix: empty for `task` (so default banners are
/// unchanged), `·<mode>` otherwise — makes an opted-in mode visible in the
/// folded output's header without touching the default format.
pub fn banner_tag() -> &'static str {
    match active() {
        ReadMode::Task => "",
        ReadMode::Aggressive => "·aggressive",
        ReadMode::Reference => "·reference",
        ReadMode::Entropy => "·entropy",
    }
}

/// Information score for a single line: Shannon entropy (bits/byte) of its
/// trimmed byte distribution times its length, i.e. total information
/// content in bits. Blank / whitespace-only lines score 0; long lines with
/// varied bytes score highest. Pure + deterministic.
pub fn line_score(line: &str) -> f64 {
    let bytes = line.trim().as_bytes();
    if bytes.is_empty() {
        return 0.0;
    }
    let mut freq = [0u32; 256];
    for &b in bytes {
        freq[b as usize] += 1;
    }
    let n = bytes.len() as f64;
    let mut entropy = 0.0f64;
    for &f in freq.iter() {
        if f > 0 {
            let p = f as f64 / n;
            entropy -= p * p.log2();
        }
    }
    entropy * n
}

/// Partition `lines` into (kept, tail) index lists for a given kept-budget
/// `head`, honouring the active mode. Both lists are returned **sorted by
/// original position** so callers render in document order.
///
/// * non-entropy modes → kept = `0..head`, tail = `head..` (cheap, and
///   byte-identical to the historical "first N" behaviour).
/// * entropy mode → kept = the `head` highest-[`line_score`] indices
///   (ties broken by earliest position), tail = the remainder.
///
/// Caller guarantees `head < lines.len()` (the fold short-circuits when
/// `total <= head`), so both partitions are non-empty.
pub fn partition_indices(lines: &[&str], head: usize) -> (Vec<usize>, Vec<usize>) {
    partition_with(active(), lines, head)
}

/// Pure core of [`partition_indices`]: partition under an explicit `mode`,
/// no env / cache (so the entropy path unit-tests deterministically).
pub fn partition_with(mode: ReadMode, lines: &[&str], head: usize) -> (Vec<usize>, Vec<usize>) {
    let total = lines.len();
    if !mode.is_entropy() || head >= total {
        let kept: Vec<usize> = (0..head.min(total)).collect();
        let tail: Vec<usize> = (head.min(total)..total).collect();
        return (kept, tail);
    }
    // Rank by score desc, tie-break by earliest index for determinism.
    let mut ranked: Vec<usize> = (0..total).collect();
    ranked.sort_by(|&a, &b| {
        line_score(lines[b])
            .partial_cmp(&line_score(lines[a]))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.cmp(&b))
    });
    let mut kept: Vec<usize> = ranked[..head].to_vec();
    let mut tail: Vec<usize> = ranked[head..].to_vec();
    kept.sort_unstable();
    tail.sort_unstable();
    (kept, tail)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A value the ladder does not know is REPORTABLE, not just absorbed.
    ///
    /// MEASURED 2026-09-04: `TOKENADE_READ_MODE=agressive` (one `g`) left
    /// every fold at `task` while `tokenade read-mode` echoed the value beside
    /// the mode it had not selected.
    #[test]
    fn an_unknown_rung_is_recognised_as_unknown() {
        for good in [
            "aggressive",
            "AGGRESSIVE",
            " terse ",
            "aggro",
            "task",
            "balanced",
            "default",
            "reference",
            "ref",
            "full",
            "entropy",
            "dense",
        ] {
            assert!(
                ReadMode::parse_checked(good).is_some(),
                "a real rung was rejected: {good:?}"
            );
        }
        for bad in ["agressive", "", "  ", "verbose", "referance", "0", "true"] {
            assert!(
                ReadMode::parse_checked(bad).is_none(),
                "an unknown value passed for a rung: {bad:?}"
            );
            // …and the forgiving parse still answers, because no command may
            // fail over a misspelled export.
            assert_eq!(ReadMode::parse(bad), ReadMode::Task, "{bad:?}");
        }
        // The two cannot drift: one list, two answers.
        for s in [
            "aggressive",
            "terse",
            "ref",
            "dense",
            "task",
            "nonsense",
            "",
        ] {
            assert_eq!(
                ReadMode::parse(s),
                ReadMode::parse_checked(s).unwrap_or(ReadMode::Task),
                "the fallback and the checked parse disagree on {s:?}"
            );
        }
    }

    #[test]
    fn parse_is_case_insensitive_with_aliases() {
        assert_eq!(ReadMode::parse("AGGRESSIVE"), ReadMode::Aggressive);
        assert_eq!(ReadMode::parse("  aggro "), ReadMode::Aggressive);
        assert_eq!(ReadMode::parse("Reference"), ReadMode::Reference);
        assert_eq!(ReadMode::parse("full"), ReadMode::Reference);
        assert_eq!(ReadMode::parse("entropy"), ReadMode::Entropy);
        assert_eq!(ReadMode::parse("dense"), ReadMode::Entropy);
        assert_eq!(ReadMode::parse("task"), ReadMode::Task);
        // unknown / empty → safe default.
        assert_eq!(ReadMode::parse("nonsense"), ReadMode::Task);
        assert_eq!(ReadMode::parse(""), ReadMode::Task);
    }

    #[test]
    fn head_scale_ladder_is_monotone() {
        assert!(ReadMode::Aggressive.head_scale() < ReadMode::Task.head_scale());
        assert!(ReadMode::Task.head_scale() <= ReadMode::Reference.head_scale());
        assert_eq!(ReadMode::Task.head_scale(), 1.0);
    }

    #[test]
    fn effective_head_task_is_identity() {
        // Task path must not perturb the budget at all (byte-identity).
        assert_eq!(super::scale_with(ReadMode::Task, 60), 60);
        assert_eq!(super::scale_with(ReadMode::Task, 5), 5);
    }

    #[test]
    fn effective_head_aggressive_shrinks_with_floor() {
        assert_eq!(super::scale_with(ReadMode::Aggressive, 60), 24); // 0.4×
                                                                     // tiny base: floor = min(8, base) so it never grows a small budget.
        assert_eq!(super::scale_with(ReadMode::Aggressive, 5), 5);
        assert_eq!(super::scale_with(ReadMode::Aggressive, 100), 40);
    }

    #[test]
    fn effective_head_reference_grows_capped() {
        assert_eq!(super::scale_with(ReadMode::Reference, 60), 180); // 3.0×
    }

    #[test]
    fn partition_non_entropy_is_first_n_in_order() {
        let lines = vec!["a", "b", "c", "d", "e"];
        let (kept, tail) = partition_with(ReadMode::Task, &lines, 2);
        assert_eq!(kept, vec![0, 1]);
        assert_eq!(tail, vec![2, 3, 4]);
    }

    #[test]
    fn partition_entropy_keeps_densest_in_document_order() {
        // Lines 1 and 3 are blank / near-uniform (low score); the dense
        // code lines (0, 2, 4) should win and be returned IN ORDER.
        let lines = vec![
            "fn parse(input: &str) -> Result<Ast, Err>",
            "   ",
            "let checksum = compute(buffer, seed ^ 0x9e3779b9);",
            "aaaa",
            "struct Config { retries: u32, backoff_ms: u64 }",
        ];
        let (kept, _tail) = partition_with(ReadMode::Entropy, &lines, 3);
        assert_eq!(kept, vec![0, 2, 4], "densest lines, original order");
    }

    #[test]
    fn line_score_ranks_dense_over_blank() {
        assert_eq!(line_score("   "), 0.0);
        assert!(line_score("fn compute_checksum(buf: &[u8]) -> u64") > line_score("aaaaaa"));
        assert!(line_score("aaaaaa") < line_score("abcdef"));
    }
}
