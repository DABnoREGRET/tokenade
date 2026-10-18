//! **Several questions, ONE budget.**
//!
//! Asking ten questions at once must not cost ten times: the budget is
//! SHARED. But shared all the way, it leaves crumbs — hence a
//! floor per question, beyond which the batch deliberately spends more
//! than one budget.
//!
//! And a refused batch SAYS so. The failure this sharing exists to prevent:
//! sixteen questions assembled beyond the delivery bound came back with
//! SIX headings, ten questions gone and nothing signalling it.

use super::*;

/// How many words a comma-separated fragment needs to count as a question of
/// its own. Written twice before — once as the rule, once as the prose in the
/// notice that quotes it — which is how the two drift apart.
pub const MIN_WORDS_PER_QUESTION: usize = 3;

/// Say so when a prompt that LOOKS batched was treated as one question.
///
/// Every surface that advertises the batched form — the repeated-question
/// nudge, the scaffolded rules file — promises "each answered under its own
/// heading". The split is declined when a fragment is too short to stand
/// alone, which is the right call (a prose aside must not fan out into three
/// retrievals), but it happened SILENTLY: ask `"windows arm64, secret
/// redaction, verbose output"` and you get one merged retrieval, no headings,
/// and no way to tell which behaviour you got. The advice was printed; its one
/// condition was not.
///
/// stderr, once per process: stdout stays byte-identical so nothing downstream
/// (dedup, cache, the caller's own diffing) sees a different payload — the same
/// rule the head-slice advisory follows.
pub fn warn_if_batch_was_declined(prompt: &str) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static SAID: AtomicBool = AtomicBool::new(false);
    let Some(msg) = batch_declined_notice(prompt) else {
        return;
    };
    if SAID.swap(true, Ordering::Relaxed) {
        return;
    }
    eprintln!("{msg}");
}

/// The notice itself, as a value. Inline in the `eprintln!` it was reachable
/// only by a process that writes to a real stderr, which is why the wording
/// had no test at all (what cannot be called cannot be asserted).
pub fn batch_declined_notice(prompt: &str) -> Option<String> {
    let parts: Vec<&str> = separator_split(prompt)
        .into_iter()
        .map(str::trim)
        .filter(|s| s.chars().any(char::is_alphanumeric))
        .collect();
    if parts.len() < 2 {
        return None;
    }
    let short = |p: &str| p.split_whitespace().count() < MIN_WORDS_PER_QUESTION;
    let n_short = parts.iter().filter(|p| short(p)).count();
    if n_short == 0 {
        return None;
    }
    // Name the FACT, not the rule. The notice used to state the condition and
    // leave the caller to find which part failed it. Quoting that fragment
    // turns "somewhere in there is a short part" into an edit.
    //
    // And the number of questions comes from the FOLD itself, not from a second
    // reading of the rule: that is how the message and the decision
    // drift apart (the const `MIN_WORDS_PER_QUESTION` already carries that scar).
    let culprit = parts.iter().find(|p| short(p)).copied().unwrap_or_default();
    let kept = fold_short_fragments(&parts).len();
    Some(format!(
        "[tokenade] \"{culprit}\" is too short to stand on its own as a question \
         ({n_short} of the {} comma-separated parts fall under \
         {MIN_WORDS_PER_QUESTION} words), so it was folded into its neighbour — \
         that prompt was searched as {kept} question(s). An aside like \
         \"in euros\" must not become its own search; give a part \
         {MIN_WORDS_PER_QUESTION} words to keep it separate.",
        parts.len(),
    ))
}

// `prompt_scoped_extract_dispatch` — the CHOICE of engine — lives in `rag`.
// It could not live here: choosing between the flat extractor and the
// structural engine requires NAMING both, and `rag` requires
// `embedding`, so the fold gate named a layer above itself.
// What remains here is what has no opinion on the engine: splitting a
// prompt into questions, sharing the budget, saying that a fragment was
// folded.

/// The smallest answer budget a single question of a batch is given.
///
/// Below roughly this, a retrieved passage is a fragment rather than an answer,
/// so a very long batch spends more than one budget in total — deliberately,
/// and bounded by [`MAX_SCOPED_PROMPTS`].
pub const MIN_SCOPED_BUDGET_PER_QUESTION: usize = 150;

/// How much of the answer budget ONE question of a batch may spend.
///
/// Every question used to get the WHOLE budget, so a batch cost the budget N
/// times over. Measured 2026-09-04 on a project changelog, which
/// `tokenade read` delivers whole in 4,699 bytes: one question came back at
/// 7,053 bytes, four at 30,592 and twelve at **82,056** — seventeen times the
/// document, from the flag whose whole purpose is to cost less than reading it
/// and which the shipped guidance recommends for exactly that reason.
///
/// Worse at the far end: sixteen questions assembled past the delivery bound,
/// which cut the answer back to an outline carrying **six** `### Q` headings
/// and no mention of the ten questions that had vanished — a question asked and
/// silently dropped, which is the one failure this whole path exists to avoid.
///
/// Sharing the budget fixes both: the batch costs about one budget however many
/// questions it carries, and nothing downstream has to truncate it.
pub fn scoped_budget_per_question(budget_tokens: usize, questions: usize) -> usize {
    if questions <= 1 {
        return budget_tokens;
    }
    (budget_tokens / questions).max(MIN_SCOPED_BUDGET_PER_QUESTION)
}

#[cfg(test)]
mod shared_budget {
    use super::*;

    #[test]
    fn a_batch_costs_about_one_budget_however_many_questions_it_carries() {
        assert_eq!(
            scoped_budget_per_question(1500, 1),
            1500,
            "one question keeps the whole budget — it is not a batch"
        );
        for n in [2usize, 4, 8, 10] {
            let per = scoped_budget_per_question(1500, n);
            assert!(
                per * n <= 1500,
                "{n} questions at {per} each spend more than the one budget"
            );
            assert!(per >= MIN_SCOPED_BUDGET_PER_QUESTION);
        }
        // Past the point where sharing would leave fragments, the floor wins
        // and the batch deliberately spends more than one budget.
        assert_eq!(
            scoped_budget_per_question(1500, 100),
            MIN_SCOPED_BUDGET_PER_QUESTION
        );
        // And it is a SHARE, not a constant: twice the budget is twice the
        // room for the same batch.
        assert!(scoped_budget_per_question(3000, 4) > scoped_budget_per_question(1500, 4));
    }

    // The end-to-end test of the sharing — sixteen questions, sixteen
    // headings — goes through the dispatcher, so it lives with it, in
    // `rag`. What remains here is the arithmetic of the share.
}
