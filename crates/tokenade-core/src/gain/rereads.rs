//! **How many times a token in the context was re-read — counted, not assumed.**
//!
//! Every API call re-sends the whole conversation; the part already seen bills
//! as a cache read. A token tokenade folded away is therefore spared once per
//! later call, until the conversation is compacted; a token tokenade injected
//! is paid once per later call, same bound. Both used to be counted per USER
//! turn with a 12-turn ceiling, while a user turn runs 9 (interactive) to 16
//! (`/loop`) API calls (measured 2026-10-05, 30 days of local transcripts).
//!
//! Here both are counted on the session's real timeline: the calls it made
//! (time, context sent) and its compactions — the transcript records them
//! (Claude Code `compact_boundary`, Codex `context_compacted`, opencode's
//! `compaction` part).
//!
//! What cannot be known is decided against the saving (design rule:
//! under-estimate rather than over-estimate):
//!
//! * without tokenade the context would have been larger, and compacted
//!   EARLIER. That compaction is simulated exactly — the first call where the
//!   real context plus the folded tokens still resident reaches the session's
//!   compaction threshold — and a fold earns nothing past it, nor past the
//!   real compaction, whichever comes first. What the session would have done
//!   after that hypothetical compaction is unknowable, so it is not credited.
//! * injected tokens are paid until the REAL compaction: the cost is never
//!   shortened by a hypothesis.
//! * cache expiry is ignored: every re-read is priced as a cache read.

/// The label of a ledger row counted by this module. Such a row is already
/// bounded by the session's real compaction; the read-time bound that guards
/// rows priced per user turn (`scan::RebillBound`, 12 × injected) must not
/// cut it again — an injection re-read 300 times until the compaction would
/// otherwise be charged 12. The label survives the ledger rollup (it becomes
/// the bucket's `compactor`).
pub const EXACT_LABEL: &str = "exact";

/// The label of a row priced by the user-turn proxy (agents whose transcript
/// shows no timeline), bounded by its own horizon — likewise exempt.
pub const PROXY_LABEL: &str = "proxy";

/// Re-reads of what tokenade puts into a subagent's context at one of its
/// tool calls — a fold credited, an injection charged: the same count on both
/// sides. A subagent shares its parent's session id, not its context, and its
/// events cannot be told apart from a parallel sibling's, so the count is
/// the measured mean, the unbiased estimate of their sum.
///
/// MEASURED 2026-10-15 on 843 local Claude Code subagents over 30 days (61 951
/// tool calls): calls after the one that sends a tool call's result — mean
/// **94**, median 43, p25 16, p75 106 (a long subagent makes more events, so the
/// per-event mean exceeds the per-subagent one). The previous counts — 9 for a
/// fold ("half the p25"), 50 for an injection (p75 of calls per SUBAGENT) —
/// assumed an event lands midway through an average subagent, and credited a
/// tenth of the fold's re-reads. 6 of the 843 subagents compacted.
pub const SUBAGENT_EVENT_REREADS: u64 = 94;

/// Re-reads of the brief prepended to a subagent's prompt: every call after
/// its first. Same measure: 66.0 calls per subagent, mean.
pub const SUBAGENT_BRIEF_REREADS: u64 = 65;

/// Re-reads of the agent's rules file in a subagent that loads it — every
/// call after its first. Claude Code gives every subagent the CLAUDE.md
/// hierarchy, tokenade's block included, except the built-ins that opt out
/// (`omitClaudeMd`: Explore, Plan, web-fetch, comment-thread-analyst — read in
/// Claude Code 2.1.299's own agent table) and custom agents declaring it.
/// MEASURED 2026-10-15 on 678 such subagents over 30 days: 48 184 calls, mean
/// **71.1** (general-purpose 76.2, fork 119.9). Charged nowhere before: the
/// parent's `scaffold_overhead` follows the parent's calls only.
pub const SUBAGENT_RULES_REREADS: u64 = 70;

/// A session's timeline as its transcript records it.
#[derive(Debug, Default, Clone)]
pub struct Timeline {
    /// Each API call: (unix seconds, context sent in BILLED tokens).
    pub calls: Vec<(u64, u64)>,
    /// Each real compaction, unix seconds.
    pub compactions: Vec<u64>,
    /// Context size (billed tokens) at which this session compacts: observed
    /// (`preTokens` of its compactions) or the agent's window × its auto-compact
    /// share.
    pub threshold: u64,
    /// Billed tokens per estimated token, to set folded tokens (estimate
    /// units) against the context (billed).
    pub billed_per_estimated: f64,
    /// Calls that wrote their context to the cache instead of reading it
    /// (expired, or a changed prefix), unix seconds — empty where the agent
    /// does not report cache writes.
    pub rewrites: Vec<u64>,
    /// What a re-read on such a call costs, in cache reads: the model's
    /// cache-write price over its cache-read price ([`rewrite_weight`]).
    pub rewrite_weight: f64,
}

/// Cache-write price over cache-read price for `model`: what one token
/// re-sent on a call that missed the cache costs, counted in the cache reads
/// every re-read row is priced at. Anthropic bills a cache write 1.25 × input;
/// elsewhere a miss is billed as input.
pub fn rewrite_weight(model: &str) -> f64 {
    let r = crate::pricing::rates_for_model(model);
    let write = if model.to_ascii_lowercase().contains("claude") {
        r.input * 1.25
    } else {
        r.input
    };
    if r.cache > 0.0 {
        (write / r.cache).max(1.0)
    } else {
        1.0
    }
}

impl Timeline {
    /// Add the compactions the context sent shows but the recorded list lacks.
    ///
    /// A compaction rewrites the context: the call after it sends a fraction
    /// of what the call before it sent. MEASURED 2026-10-15 on 30 days of
    /// local Claude Code transcripts: a drop below half of a ≥ 50 k context,
    /// held by the next two calls, matches **285 of 289** `compact_boundary`
    /// records, with 6 drops where none was written. The cached timeline had
    /// lost records in 5 of 22 sessions (one with 1 of 4), and every injection
    /// before a lost compaction was charged up to the next one: 238 M re-billed
    /// for 123 M re-read. Both sides of the ledger end at the drop.
    pub fn with_inferred_compactions(mut self) -> Self {
        const FLOOR: u64 = 50_000;
        let c = &self.calls;
        let mut inferred = Vec::new();
        for i in 1..c.len() {
            let (prev, (ts, ctx)) = (c[i - 1].1, c[i]);
            let held = c[i + 1..].iter().take(2).all(|&(_, n)| n * 2 < prev);
            if prev >= FLOOR
                && ctx * 2 < prev
                && held
                && !self.compactions.iter().any(|&k| k > c[i - 1].0 && k <= ts)
            {
                inferred.push(ts);
            }
        }
        self.compactions.extend(inferred);
        self.compactions.sort_unstable();
        self
    }

    fn next_compaction_after(&self, ts: u64) -> u64 {
        self.compactions
            .iter()
            .copied()
            .filter(|&c| c > ts)
            .min()
            .unwrap_or(u64::MAX)
    }

    /// What one token re-sent on the call at `ts` costs, in cache reads.
    fn weight_at(&self, ts: u64) -> f64 {
        if self.rewrites.binary_search(&ts).is_ok() {
            self.rewrite_weight.max(1.0)
        } else {
            1.0
        }
    }
}

/// Re-reads spared by `folds` — (unix seconds, tokens removed, estimate
/// units) — summed over the calls each stayed out of: Σ tokens × calls.
pub fn fold_rereads(folds: &[(u64, u64)], tl: &Timeline) -> u64 {
    let mut folds: Vec<(u64, u64)> = folds.to_vec();
    folds.sort_unstable();
    // (tokens, real compaction that ends it, not sent yet)
    let mut live: Vec<(u64, u64, bool)> = Vec::new();
    let mut next = 0usize;
    let mut total = 0u64;
    for &(t, ctx) in &tl.calls {
        while next < folds.len() && folds[next].0 < t {
            let (ft, n) = folds[next];
            live.push((n, tl.next_compaction_after(ft), true));
            next += 1;
        }
        live.retain(|&(_, end, _)| t < end);
        let resident: u64 = live.iter().map(|&(n, _, _)| n).sum();
        if resident > 0
            && tl.threshold > 0
            && ctx as f64 + resident as f64 * tl.billed_per_estimated >= tl.threshold as f64
        {
            // The compaction tokenade's folds postponed: past it, nothing is
            // credited.
            live.clear();
            continue;
        }
        // The first call to carry a fold is the send its measured row already
        // prices (as input): a re-read starts on the call after.
        let w = tl.weight_at(t);
        for (n, _, first) in live.iter_mut() {
            if std::mem::take(first) {
                continue;
            }
            total = total.saturating_add((*n as f64 * w).round() as u64);
        }
    }
    total
}

/// Re-reads paid for `injected` — (unix seconds, tokens, persistent) — each
/// until the session's next real compaction; a persistent one (the agent's
/// rules file, reloaded after every compaction) on every later call.
pub fn injected_rereads(injected: &[(u64, u64, bool)], tl: &Timeline) -> u64 {
    injected
        .iter()
        .map(|&(t, n, persistent)| {
            // The rules file is in the context from the session's FIRST call,
            // whenever tokenade got to book it (on a bridge, its first tool
            // call — after the call that sent it). MEASURED on a real pi
            // session: booked at the second of the first reply, which the
            // `c > t` rule then took for a re-read-free send.
            let weighed = |calls: &mut dyn Iterator<Item = u64>| -> u64 {
                (calls.map(|c| tl.weight_at(c)).sum::<f64>() * n as f64).round() as u64
            };
            if persistent {
                return weighed(&mut tl.calls.iter().skip(1).map(|&(c, _)| c));
            }
            let end = tl.next_compaction_after(t);
            // The first call after the injection sends it (its `injected_context`
            // row, priced as input); the re-reads are the calls after that.
            weighed(
                &mut tl
                    .calls
                    .iter()
                    .filter(|&&(c, _)| c > t && c < end)
                    .skip(1)
                    .map(|&(c, _)| c),
            )
        })
        .fold(0u64, u64::saturating_add)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tl(calls: &[(u64, u64)], compactions: &[u64], threshold: u64) -> Timeline {
        Timeline {
            calls: calls.to_vec(),
            compactions: compactions.to_vec(),
            threshold,
            billed_per_estimated: 1.0,
            ..Default::default()
        }
    }

    /// A fold is spared once per call after the one that sends it, up to the
    /// real compaction.
    #[test]
    fn a_fold_counts_every_later_call_until_the_compaction() {
        let t = tl(
            &[(10, 100), (20, 100), (30, 100), (40, 100), (50, 100)],
            &[35],
            1_000_000,
        );
        // Fold at 15: call 20 sends it, call 30 re-reads it, then the
        // compaction at 35 ends it.
        assert_eq!(fold_rereads(&[(15, 1_000)], &t), 1_000);
        // No compaction: calls 30..50.
        let t = tl(
            &[(10, 100), (20, 100), (30, 100), (40, 100), (50, 100)],
            &[],
            1_000_000,
        );
        assert_eq!(fold_rereads(&[(15, 1_000)], &t), 3_000);
    }

    /// Without the fold the context would have reached the threshold sooner:
    /// the credit stops at that simulated compaction (under-estimate).
    #[test]
    fn the_compaction_the_fold_postponed_ends_its_credit() {
        // Real context 500, 600, 700; the 300 folded tokens make 800, 900, 1000.
        let t = tl(&[(10, 400), (20, 500), (30, 600), (40, 700)], &[], 1_000);
        assert_eq!(fold_rereads(&[(15, 300)], &t), 300);
    }

    /// A compaction the timeline did not record still ends both sides: the
    /// context sent shows it. A one-call dip is not one.
    #[test]
    fn a_held_context_drop_is_a_compaction() {
        let calls = [
            (10, 600_000),
            (20, 650_000),
            (30, 80_000),
            (40, 90_000),
            (50, 95_000),
        ];
        let t = tl(&calls, &[], u64::MAX).with_inferred_compactions();
        assert_eq!(t.compactions, vec![30]);
        assert_eq!(injected_rereads(&[(5, 10, false)], &t), 10);
        // Already recorded: not added twice.
        assert_eq!(
            tl(&calls, &[25], u64::MAX)
                .with_inferred_compactions()
                .compactions,
            vec![25]
        );
        // A dip the next call does not hold.
        let dip = [(10, 600_000), (20, 100_000), (30, 610_000), (40, 620_000)];
        assert!(tl(&dip, &[], u64::MAX)
            .with_inferred_compactions()
            .compactions
            .is_empty());
        // Below the floor: noise.
        let small = [(10, 40_000), (20, 5_000), (30, 6_000), (40, 7_000)];
        assert!(tl(&small, &[], u64::MAX)
            .with_inferred_compactions()
            .compactions
            .is_empty());
    }

    /// Injected tokens are paid until the REAL compaction, never shortened.
    #[test]
    fn injected_tokens_are_paid_until_the_real_compaction() {
        let t = tl(&[(10, 900), (20, 990), (30, 999), (40, 100)], &[35], 1_000);
        assert_eq!(injected_rereads(&[(5, 50, false)], &t), 100);
        assert_eq!(injected_rereads(&[(36, 50, false)], &t), 0);
    }

    /// A call that missed the cache paid every re-sent token at the
    /// cache-write price: its re-reads weigh `rewrite_weight` cache reads, on
    /// both sides.
    #[test]
    fn a_call_that_missed_the_cache_weighs_its_write_price() {
        let mut t = tl(
            &[(10, 100), (20, 100), (30, 100), (40, 100)],
            &[],
            1_000_000,
        );
        t.rewrites = vec![30];
        t.rewrite_weight = 12.5;
        // Fold at 15: sent at 20, re-read at 30 (a miss) and 40.
        assert_eq!(fold_rereads(&[(15, 100)], &t), 1_250 + 100);
        // Injected at 15: the same calls.
        assert_eq!(injected_rereads(&[(15, 100, false)], &t), 1_250 + 100);
        // Anthropic: write 1.25 × input over a read of 0.1 × input.
        assert!((rewrite_weight("claude-opus-4-5") - 12.5).abs() < 1e-9);
    }

    /// The rules file is reloaded after a compaction: re-read on every call.
    #[test]
    fn a_persistent_injection_outlives_the_compaction() {
        let t = tl(&[(10, 900), (20, 990), (30, 999), (40, 100)], &[35], 1_000);
        assert_eq!(injected_rereads(&[(5, 50, true)], &t), 150);
        // Booked late (at the third call): still sent by the first.
        assert_eq!(injected_rereads(&[(25, 50, true)], &t), 150);
    }
}
