//! `review_context` — a token-budgeted code-review context builder.
//!
//! The problem (lifted from the `graphsift` competitor analysis): when an agent reviews a change, naively dumping
//! every touched file plus its neighbourhood blows the context window. What
//! you actually want is the *most review-relevant* files greedily packed into
//! a hard token budget — changed files first, then their direct graph
//! neighbours, then a few high-importance hubs, stopping the moment the budget
//! is spent.
//!
//! This module is the **pure** core: scoring a file from its relationship to
//! the change set, and the greedy knapsack-style fit. All IO (running `git
//! diff`, loading the symbol index, reading + tokenizing files) lives in the
//! CLI wrapper `cmd_review_context`, so the ranking math here is deterministic
//! and unit-testable in isolation.
//!
//! Design choices that keep it coherent with the rest of tokenade:
//!   * importance is the import-graph **in-degree** (how many repo files
//!     import this one) — derived from the index tokenade already builds, so
//!     there is no new model, weight store, or dependency.
//!   * the greedy fit keeps scanning after an over-budget file is skipped, so
//!     a cheap high-value file still lands even when a big one is dropped.

use std::cmp::Ordering;

/// Why a file was pulled into the review context, highest tier first. The
/// tiers are a total order: a `Changed` file always outranks any related
/// file, which always outranks a pure importance hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The file appears in the diff — the review is *about* this file.
    Changed,
    /// A dependency the changed code imports — needed to understand the change.
    DependencyOfChanged,
    /// A dependent that imports the changed code — the change's blast radius.
    DependentOnChanged,
    /// A historically central file (high import-graph in-degree), included
    /// only if budget remains after the change-related files.
    ImportanceHub,
}

impl Reason {
    /// Short tag for the human/JSON rendering.
    pub fn tag(self) -> &'static str {
        match self {
            Reason::Changed => "changed",
            Reason::DependencyOfChanged => "dependency",
            Reason::DependentOnChanged => "dependent",
            Reason::ImportanceHub => "hub",
        }
    }
}

/// A single review-context candidate file with its computed relevance and
/// measured token cost.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub path: String,
    /// Relevance in `[0.0, 1.0]`, higher = more review-relevant.
    pub score: f64,
    /// Estimated tokens the file body costs if included.
    pub tokens: usize,
    pub reason: Reason,
}

/// The outcome of fitting candidates into a budget: what made the cut, what
/// got dropped, and the running token total of the included set.
#[derive(Debug, Clone, Default)]
pub struct ReviewPlan {
    pub budget: usize,
    pub included: Vec<Candidate>,
    pub dropped: Vec<Candidate>,
    pub total_tokens: usize,
}

impl ReviewPlan {
    /// Tokens the full candidate set *would* have cost — the baseline a naive
    /// "include everything" review would pay. Used for the savings story.
    pub fn candidate_tokens(&self) -> usize {
        self.total_tokens + self.dropped.iter().map(|c| c.tokens).sum::<usize>()
    }
}

/// Score a file from its relationship to the change set. Returns `None` when
/// the file is unrelated to the change *and* has no importance — such files
/// are not even candidates and should never be read/tokenized.
///
/// `importance` is the file's normalized in-degree in `[0.0, 1.0]` (1.0 = the
/// most-imported file in the repo). The reason tiers are mutually exclusive
/// and checked in priority order, so a changed file that is *also* a hub still
/// scores as `Changed` (1.0) — its primacy is never diluted.
pub fn score_file(
    is_changed: bool,
    imports_changed: bool,
    imported_by_changed: bool,
    importance: f64,
) -> Option<(Reason, f64)> {
    let imp = importance.clamp(0.0, 1.0);
    if is_changed {
        return Some((Reason::Changed, 1.0));
    }
    if imported_by_changed {
        // A dependency the change leans on — bounded to (0.7, 0.9].
        return Some((Reason::DependencyOfChanged, 0.7 + 0.2 * imp));
    }
    if imports_changed {
        // A dependent affected by the change — bounded to (0.5, 0.7].
        return Some((Reason::DependentOnChanged, 0.5 + 0.2 * imp));
    }
    if imp > 0.0 {
        // A central hub with no direct tie to the change — capped at 0.3 so it
        // never outranks a change-related file.
        return Some((Reason::ImportanceHub, 0.3 * imp));
    }
    None
}

/// Greedily fit candidates into `budget` tokens, most-relevant first.
///
/// Ordering: score descending, then **tokens ascending** (pack cheap files
/// first so more of them fit at equal relevance), then path for determinism.
/// The scan does not stop at the first over-budget file — it keeps going so a
/// small high-value file still lands after a big one is skipped (a bounded,
/// order-preserving knapsack approximation, not a true 0/1 solve).
pub fn greedy_fit(mut candidates: Vec<Candidate>, budget: usize) -> ReviewPlan {
    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(Ordering::Equal)
            .then(a.tokens.cmp(&b.tokens))
            .then(a.path.cmp(&b.path))
    });

    let mut included = Vec::new();
    let mut dropped = Vec::new();
    let mut total = 0usize;
    for c in candidates {
        if total + c.tokens <= budget {
            total += c.tokens;
            included.push(c);
        } else {
            dropped.push(c);
        }
    }

    ReviewPlan {
        budget,
        included,
        dropped,
        total_tokens: total,
    }
}

/// Normalize a raw count against the maximum observed, yielding `[0.0, 1.0]`.
/// Returns 0.0 when `max == 0` (avoids divide-by-zero on an empty graph).
pub fn normalize(count: usize, max: usize) -> f64 {
    if max == 0 {
        return 0.0;
    }
    count as f64 / max as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(path: &str, score: f64, tokens: usize, reason: Reason) -> Candidate {
        Candidate {
            path: path.into(),
            score,
            tokens,
            reason,
        }
    }

    #[test]
    fn score_tiers_are_totally_ordered() {
        let changed = score_file(true, true, true, 1.0).unwrap();
        let dep = score_file(false, false, true, 1.0).unwrap();
        let dependent = score_file(false, true, false, 1.0).unwrap();
        let hub = score_file(false, false, false, 1.0).unwrap();
        assert_eq!(changed.0, Reason::Changed);
        assert_eq!(dep.0, Reason::DependencyOfChanged);
        assert_eq!(dependent.0, Reason::DependentOnChanged);
        assert_eq!(hub.0, Reason::ImportanceHub);
        // Strictly decreasing across tiers regardless of importance.
        assert!(changed.1 > dep.1);
        assert!(dep.1 > dependent.1);
        assert!(dependent.1 > hub.1);
    }

    #[test]
    fn changed_beats_hub_even_when_also_a_hub() {
        // A file that is both changed AND the top hub still scores as Changed.
        let (reason, score) = score_file(true, false, false, 1.0).unwrap();
        assert_eq!(reason, Reason::Changed);
        assert_eq!(score, 1.0);
    }

    #[test]
    fn unrelated_zero_importance_is_not_a_candidate() {
        assert!(score_file(false, false, false, 0.0).is_none());
    }

    #[test]
    fn importance_modulates_within_tier_but_stays_bounded() {
        let lo = score_file(false, false, true, 0.0).unwrap().1;
        let hi = score_file(false, false, true, 1.0).unwrap().1;
        assert!(lo < hi);
        assert!(lo > 0.5 && hi <= 0.9);
    }

    #[test]
    fn greedy_fit_respects_budget_and_packs_cheap_first() {
        let cands = vec![
            cand("big.rs", 0.9, 100, Reason::Changed),
            cand("small.rs", 0.9, 10, Reason::Changed),
            cand("mid.rs", 0.5, 40, Reason::DependentOnChanged),
        ];
        // Budget fits small (10) + mid (40) = 50, but not big (100).
        let plan = greedy_fit(cands, 60);
        let kept: Vec<&str> = plan.included.iter().map(|c| c.path.as_str()).collect();
        assert!(kept.contains(&"small.rs"), "cheap high-value file must fit");
        assert!(
            kept.contains(&"mid.rs"),
            "small file fits after big skipped"
        );
        assert!(!kept.contains(&"big.rs"), "over-budget file dropped");
        assert_eq!(plan.total_tokens, 50);
        assert_eq!(plan.dropped.len(), 1);
    }

    #[test]
    fn greedy_fit_keeps_scanning_after_overflow() {
        // Highest-score file is too big; the next-best cheap file must still
        // be picked up rather than aborting at the first overflow.
        let cands = vec![
            cand("huge.rs", 1.0, 1000, Reason::Changed),
            cand("tiny.rs", 0.4, 5, Reason::ImportanceHub),
        ];
        let plan = greedy_fit(cands, 50);
        assert_eq!(plan.included.len(), 1);
        assert_eq!(plan.included[0].path, "tiny.rs");
    }

    #[test]
    fn candidate_tokens_sums_included_and_dropped() {
        let plan = greedy_fit(
            vec![
                cand("a", 0.9, 30, Reason::Changed),
                cand("b", 0.8, 80, Reason::Changed),
            ],
            50,
        );
        assert_eq!(plan.total_tokens, 30);
        assert_eq!(plan.candidate_tokens(), 110);
    }

    #[test]
    fn normalize_is_safe_and_bounded() {
        assert_eq!(normalize(0, 0), 0.0);
        assert_eq!(normalize(5, 10), 0.5);
        assert_eq!(normalize(10, 10), 1.0);
    }
}
