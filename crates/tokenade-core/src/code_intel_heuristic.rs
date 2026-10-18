//! Heuristic savings for code-intelligence MCP tools.
//!
//! Tools like [`semantic_search`], [`symbol_find`], [`structure_map`],
//! [`skeleton`], and [`impact_of`] don't have a directly-measurable
//! "savings" number the way a compactor does. When a compactor turns a
//! 5000-token git diff into a 800-token summary, the saving is the
//! arithmetic difference. But when an agent calls `symbol_find`, the
//! counterfactual — *the Grep+Read sequence the agent would have run
//! instead* — never happens. We can't time-travel and measure it.
//!
//! ## Where the multiplier comes from
//!
//! Until we run our own controlled benchmark across a representative
//! corpus, we use CodeGraph 0.9.4's published headline number:
//!
//! > "Average: 35% cheaper · **57% fewer tokens** · 46% faster · 71%
//! > fewer tool calls" — across 7 codebases × 4 runs each (median).
//! > Source: <https://github.com/colbymchenry/codegraph> (2026-05-24).
//!
//! That's the closest peer-reviewed datapoint we have: another local-
//! first code-intelligence MCP, tested with a real headless Claude Code
//! agent on real architecture questions. Their suite covers VS Code,
//! Excalidraw, Django, Tokio, OkHttp, Gin, Alamofire — a fair sample.
//!
//! If 57 % of the agent's would-be tokens are saved, then for a
//! response of size `r` the *baseline* (no Tokenade) would have been
//! `r / (1 - 0.57) ≈ r × 2.33`. We log code-intel ops as
//! `before = r × 2.33`, `after = r`, so the dashboard shows the
//! estimated savings without inventing per-tool fudge factors.
//!
//! ## Why this is honest enough to ship
//!
//! - The number is **published, dated, and cited**, not invented.
//! - It's a **single conservative average** — no per-tool numerology.
//! - The dashboard surfaces it with a clear "heuristic, based on
//!   CodeGraph benchmark" label so a screenshot is never misleading.
//! - When we run our own benchmark, we replace one constant in one
//!   file and every estimate updates uniformly.
//!
//! ## When this is NOT honest enough
//!
//! - For per-call attribution ("this `symbol_find` saved 1234 tokens"):
//!   the number is a statistical average, not a per-call truth.
//! - For unusually small/large responses: a 50-token response would
//!   imply a 100-token baseline, which is implausibly small for a real
//!   agent search. We don't apply the multiplier when the response is
//!   below [`MIN_RESPONSE_TOKENS_FOR_HEURISTIC`].

/// Headline savings ratio from CodeGraph 0.9.4 published benchmark
/// (mean of 7 codebases × 4 runs, "fewer tokens" column). Replace with
/// our own number when we publish our benchmark.
pub const HEURISTIC_SAVINGS_PCT: f64 = 0.57;

/// Citation block surfaced by the dashboard so readers can verify the
/// origin of the estimate.
pub const HEURISTIC_SOURCE_LABEL: &str =
    "CodeGraph 0.9.4 benchmark · 7 codebases × 4 runs · published 2026-05-24";

/// Smallest response (in tokens) we'll apply the multiplier to. Below
/// this, the implied baseline is implausibly small and the estimate
/// risks looking like noise.
///
/// History:
/// - **was 50** under the assumption "real agent searches return ≥50
///   tokens of context." Production telemetry (2026-05-31) refuted
///   this: `symbol_find` hits routinely return 14-41 tokens (a single
///   `file:line kind name` line), well under the floor, so 26
///   calls/session attributed 0 savings to a tool that VISIBLY
///   displaces 200-2000-token greps.
/// - **now 10** — a 10-token response expands to a 23-token baseline,
///   conservative even at the lower end (a single-hit grep wouldn't
///   be much smaller than that). Larger displacements remain
///   underreported, which is the right side to err on.
pub const MIN_RESPONSE_TOKENS_FOR_HEURISTIC: u64 = 10;

/// Estimate the baseline (no-Tokenade) token cost for a code-intel
/// response of `response_tokens` tokens. Returns `response_tokens`
/// unchanged when the response is too small to apply the heuristic
/// honestly (caller sees `before == after`, i.e. no estimated saving).
pub fn estimated_baseline(response_tokens: u64) -> u64 {
    if response_tokens < MIN_RESPONSE_TOKENS_FOR_HEURISTIC {
        return response_tokens;
    }
    // saved% = (baseline - response) / baseline = HEURISTIC_SAVINGS_PCT
    // → baseline = response / (1 - HEURISTIC_SAVINGS_PCT)
    let factor = 1.0 / (1.0 - HEURISTIC_SAVINGS_PCT);
    ((response_tokens as f64) * factor).round() as u64
}

/// The set of MCP tool names we treat as code-intelligence. Used by
/// both the gain logger (to apply the heuristic) and the dashboard
/// (to partition them into their own section).
pub const CODE_INTEL_TOOLS: &[&str] = &[
    "semantic_search",
    "symbol_find",
    "structure_map",
    "skeleton",
    "impact_of",
    // Added 2026-05-29: call_hierarchy logs `Savings::new(tok, tok)`
    // expecting the heuristic to expand its baseline (it can't measure
    // the avoided grep+read sequence). Without this entry the MCP
    // `log_mcp` wrapper skips the expansion and the dashboard shows
    // 0% savings forever.
    "call_hierarchy",
    // #501 — `expand_fold` returns lines from a stashed reference; it
    // substitutes for re-reading the original blob, which is exactly
    // the code-intel value proposition. Previously logged with the
    // `Savings::new(tok, tok)` pattern but missed the heuristic, so
    // small responses got silently dropped by the log_mcp guard.
    "expand_fold",
];

/// True when `op` is a code-intelligence tool. Matches either the
/// exact name from [`CODE_INTEL_TOOLS`] or a colon-suffixed variant
/// of one (`semantic_search:file`, `semantic_search:global`, …).
///
/// #501 — the colon-variant arm was added because the MCP layer logs
/// `semantic_search:file` and `semantic_search:global` as distinct
/// telemetry buckets. Without it, those rows were treated as plain
/// no-savings MCP work, dropped by the log_mcp zero-saved guard, and
/// excluded from the dashboard code-intel partition — a silent
/// undercount of every semantic_search scoped to a file or repo.
pub fn is_code_intel(op: &str) -> bool {
    CODE_INTEL_TOOLS.iter().any(|tool| {
        op == *tool
            || op
                .strip_prefix(tool)
                .is_some_and(|rest| rest.starts_with(':'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimated_baseline_above_threshold_applies_multiplier() {
        // 1000 → 1000 / (1 - 0.57) = 2326 (rounded).
        let b = estimated_baseline(1000);
        assert!(b > 2300 && b < 2360, "expected ≈2326, got {b}");
        // Saved = b - 1000 = 1326. Pct = 1326/2326 ≈ 57%.
        let saved = b - 1000;
        let pct = 100.0 * (saved as f64) / (b as f64);
        assert!((pct - 57.0).abs() < 1.0);
    }

    #[test]
    fn estimated_baseline_below_threshold_returns_unchanged() {
        assert_eq!(estimated_baseline(0), 0);
        assert_eq!(estimated_baseline(MIN_RESPONSE_TOKENS_FOR_HEURISTIC - 1), 9);
        // Sanity: any token count strictly below the threshold is
        // returned as-is (no heuristic expansion).
        for n in 0..MIN_RESPONSE_TOKENS_FOR_HEURISTIC {
            assert_eq!(estimated_baseline(n), n, "expected passthrough at n={n}");
        }
    }

    #[test]
    fn small_symbol_find_response_now_gets_credited() {
        // A single-hit symbol_find response (~14 tokens) used to
        // return 14 → 14 (no savings) under the old 50-token floor.
        // Now it expands to a plausible baseline.
        let baseline = estimated_baseline(14);
        assert!(
            baseline > 14,
            "14-token response should now get a baseline expansion, got {baseline}"
        );
        let saved = baseline.saturating_sub(14);
        let pct = 100.0 * saved as f64 / baseline as f64;
        assert!(
            (pct - 57.0).abs() < 2.0,
            "expansion should track the 57% heuristic, got {pct:.1}%"
        );
    }

    #[test]
    fn estimated_baseline_at_threshold_applies_multiplier() {
        let b = estimated_baseline(MIN_RESPONSE_TOKENS_FOR_HEURISTIC);
        assert!(b > MIN_RESPONSE_TOKENS_FOR_HEURISTIC);
    }

    #[test]
    fn is_code_intel_matches_known_tools() {
        assert!(is_code_intel("symbol_find"));
        assert!(is_code_intel("semantic_search"));
        assert!(!is_code_intel("json_compact"));
        assert!(!is_code_intel(""));
    }

    // #501 — variant arms (`semantic_search:file`, `:global`) and
    // `expand_fold` were previously misclassified as non-code-intel,
    // silently dropping their telemetry. Pin the new behaviour.
    #[test]
    fn is_code_intel_recognises_variant_suffixes() {
        assert!(is_code_intel("semantic_search:file"));
        assert!(is_code_intel("semantic_search:global"));
        assert!(is_code_intel("symbol_find:fuzzy"));
        assert!(is_code_intel("expand_fold"));
    }

    #[test]
    fn is_code_intel_rejects_close_but_different_names() {
        // Collision safety: prefix without colon-separator must NOT match.
        // If we ever add `skeleton_alt` as a NON-code-intel tool, the
        // current matcher correctly rejects it because `_` follows the
        // prefix instead of `:`.
        assert!(!is_code_intel("skeleton_alt"));
        assert!(!is_code_intel("semantic_search_failed"));
        assert!(!is_code_intel("symbol_find_v2"));
        // Empty colon-suffix still requires the colon char.
        assert!(is_code_intel("skeleton:")); // explicit empty variant is fine
    }

    #[test]
    fn citation_present_and_dated() {
        assert!(HEURISTIC_SOURCE_LABEL.contains("CodeGraph"));
        assert!(HEURISTIC_SOURCE_LABEL.contains("2026"));
    }
}

/// True when a code-intel row's BASELINE came out of the published ratio
/// above rather than out of anything this machine read. Two shapes reach
/// the same place:
///
/// * the writer already expanded it and stamped `class: "ci_heuristic"`;
/// * it arrived unmeasurable (`before == after`) and the reader multiplies it.
///
/// Everything else in the bucket had a real input — `skeleton` tokenises the
/// file it was handed, and only the ALTERNATIVE ("you would have read it
/// whole") is assumed. Reporting the two as one number claims the same
/// confidence for both, and on this install the measured-input half is the
/// LARGER one: 73 % of the bucket.
///
/// It lives HERE, next to the ratio it is about, because the terminal
/// dashboard was the only surface that knew. `gain --json` did not, and the
/// value ranking that reads that JSON to CHOOSE each iteration's target
/// therefore listed `structure_map` at −56 % among fifteen compactors whose
/// rates are measured — a number that is the published ratio itself, on an
/// op that folds nothing. A rule that decides what counts as measured cannot
/// live in one consumer.
pub fn baseline_is_borrowed(class: Option<&str>, before: u64, after: u64) -> bool {
    crate::coefficients::baseline_is_borrowed(class, "", before, after)
}

#[cfg(test)]
mod borrowed_baseline_tests {
    use super::*;

    #[test]
    fn a_stamped_row_and_a_flat_row_are_both_borrowed() {
        assert!(baseline_is_borrowed(Some("ci_heuristic"), 2326, 1000));
        assert!(baseline_is_borrowed(None, 1000, 1000));
    }

    #[test]
    fn a_row_with_a_real_input_is_not_borrowed() {
        // `skeleton` tokenises the file it was handed: before != after and
        // no stamp. Folding it into the borrowed half would disclaim the
        // larger, measured part of the bucket.
        assert!(!baseline_is_borrowed(None, 1_707_116, 98_430));
        assert!(!baseline_is_borrowed(Some("other"), 2326, 1000));
    }
}
