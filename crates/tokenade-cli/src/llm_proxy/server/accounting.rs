//! What a turn through the proxy books in the gain ledger.

use super::*;

/// Book a turn in the gain ledger, for the agent this port serves.
///
/// * first fold of a block → a measured saving;
/// * the same block folded again on a later call → a re-read spared, exact
///   (a real call re-sent it), and stopped where the context would have
///   reached the model's compaction threshold without the folds — the rule of
///   `gain::fold_rereads`, decided against the saving. No usage, no credit;
/// * the style preamble the proxy injected → charged on every call carrying it.
///
/// Only a turn the provider answered (2xx) is booked.
pub struct LedgerAccounting;
impl Accounting for LedgerAccounting {
    fn record(&self, t: &TurnRecord) {
        DebugLogAccounting.record(t);
        if !(200..300).contains(&t.status) {
            return;
        }
        let model = t.model.as_deref();
        let book = || {
            use tokenade_core::{gain, Savings};
            let (before, after) = t.first_fold;
            if before > after {
                gain::record(
                    "llm-proxy",
                    "llm-proxy:fold",
                    "tool-result",
                    Savings::new(before as usize, after as usize),
                );
            }
            if t.recovery_offered > 0 {
                gain::record_recovery("offered", t.recovery_offered as usize);
            }
            // A call that wrote its context to the cache instead of reading it
            // paid each re-sent token at the write price.
            let w = |n: u64| match t.usage {
                Some(u) if u.cache_write * 2 > u.context() => {
                    (n as f64 * gain::rewrite_weight(model.unwrap_or(""))).round() as usize
                }
                _ => n as usize,
            };
            if t.tokens_already_credited > 0 && below_compaction(t, model) {
                gain::record_estimated(
                    "llm-proxy",
                    "compounding",
                    gain::EXACT_LABEL,
                    "compounding",
                    Savings::new(w(t.tokens_already_credited), 0),
                );
            }
            if t.preamble_tokens > 0 {
                gain::record_estimated_io(
                    "llm-proxy",
                    "overhead_rebill",
                    gain::EXACT_LABEL,
                    "overhead",
                    "cache",
                    Savings::new(w(t.preamble_tokens), 0),
                );
            }
        };
        match t.agent {
            Some(a) => tokenade_core::gain::as_agent(a, model, book),
            None => tokenade_core::gain::as_agent("unknown", model, book),
        }
    }
}

/// Would this call still have been under the compaction threshold with every
/// folded token back in the context? Past it the agent would have compacted
/// earlier without tokenade, and what it would have re-read is unknowable.
fn below_compaction(t: &TurnRecord, model: Option<&str>) -> bool {
    let Some(u) = t.usage else { return false };
    let model = model.unwrap_or("");
    let threshold = tokenade_core::autocompact::compaction_threshold(
        tokenade_core::autocompact::estimator_window_for_model(model),
        tokenade_core::autocompact::DEFAULT_PCT,
    ) as f64;
    let ctx = u.context() as f64;
    let resident = (t.tokens_saved + t.tokens_already_credited) as f64
        * tokenade_core::tokenizer::claude_tokens_per_estimated(model);
    ctx > 0.0 && ctx + resident < threshold
}
