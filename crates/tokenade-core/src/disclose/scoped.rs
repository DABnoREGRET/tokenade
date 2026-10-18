//! Answering a prompt out of ONE stashed blob.
//!
//! A fold hands the agent a pointer; this is what comes back when the agent
//! asks a QUESTION of that pointer instead of expanding it whole. Retrieval is
//! BM25 over lines, packed greedily into a token budget — everything here
//! serves that, and nothing here decides WHEN to fold (that stays in the stash
//! path) or how to render a summary.

mod batch;
mod packing;
mod retrieval;
pub use batch::*;
pub use packing::*;
pub use retrieval::*;

use super::*;

/// Prompt-scoped extract for the WebFetch hook (lever #1).
///
/// Given already-distilled markdown `text` and the agent's `prompt`,
/// return the passages most relevant to the prompt: every line whose
/// BM25 score against the prompt terms clears the threshold, each
/// widened by [`SCOPED_CONTEXT_LINES`] of surrounding context and
/// merged, greedily packed highest-score-first until `budget_tokens` is
/// spent, then emitted in document order with `…` gap markers.
///
/// Returns `None` — signalling the caller to fall back to whole-page
/// serving — when:
///   * the prompt tokenises to nothing (empty / punctuation only),
///   * NO line reaches [`SCOPED_MIN_ANCHOR_SCORE`] (lexical gap: the
///     answer uses none of the prompt's words, or the prompt is vague).
///     Returning None here is deliberate: a keyword scoper that can't
///     find an anchor must not fabricate a narrow, possibly-wrong
///     excerpt — that is worse than an outline. The caller serves the
///     page instead so the answer is never silently dropped.
///
/// Decoy-safety: this returns ALL passages above threshold within
/// budget, never a single "the answer is X" line. When several numbers
/// score similarly (the decoy case) the model receives every candidate
/// plus their context and disambiguates, rather than being handed one
/// arbitrarily-chosen wrong value.
pub fn prompt_scoped_extract(text: &str, prompt: &str, budget_tokens: usize) -> Option<String> {
    scoped_extract_inner(text, prompt, budget_tokens, true)
}

/// Upper bound on questions honoured in one comma-separated `--prompt`.
///
/// Set high on purpose. The job of keeping a prose question with commas from
/// fanning out belongs entirely to the "every fragment stands alone" rule
/// below — this ceiling never did that work, and when it was low it only
/// silently truncated deliberate batches. Batching is the cheapest way to ask
/// several things (one round-trip instead of N), and our own scaffolded rules
/// tell agents to do it, so the ceiling should not be what stops them.
pub const MAX_SCOPED_PROMPTS: usize = 128;

/// Split a `--prompt` value into separate questions on commas. Fragments with
/// no alphanumeric content (stray punctuation, empty) are dropped, and the
/// count is capped at [`MAX_SCOPED_PROMPTS`]. A value with no usable comma
/// yields one element (or zero if blank) — the caller keeps the exact prior
/// single-question path in that case.
///
/// The caller wants the dropped count too, so the value-only wrapper that
/// used to sit here went with it.
///
/// The cap itself is right — a prompt with forty commas is not forty deliberate
/// questions — but dropping the tail SILENTLY is not: the reply comes back
/// numbered `Q1 … Q8` and reads as complete, so a caller that asked ten things
/// has no way to tell that two of them were never searched for. That is the
/// worst shape a cap can take: the missing part is not "more of the same", it is
/// a distinct question whose absence looks like an answer.
/// Value-only view of [`split_scoped_prompts_counted`], kept for the tests
/// that cover the split itself. `cargo build` calls it dead because it does
/// not compile the test cfg.
#[allow(dead_code)]
pub(super) fn split_scoped_prompts(prompt: &str) -> Vec<String> {
    split_scoped_prompts_counted(prompt).0
}

/// Split on commas that are actually separators.
///
/// A comma inside `()`, `[]`, `{}` or a `"`/`` ` ``-quoted span is never one:
/// it belongs to a range, a list the caller is quoting, or a parenthetical.
/// Splitting there produces "questions" like `pas de troncature)` and
/// `what tool or platform)?` — each of which then gets its OWN retrieval at
/// the FULL budget and comes back under a `### Qn ·` heading, reading exactly
/// like an answered question. Measured on 116 distinct `--prompt` values from
/// this machine's transcripts: 15 carry a nested comma, and 3 are torn at one
/// today.
///
/// The apostrophe is deliberately NOT a quote here. French prose is full of
/// `d'envoi` and `l'effet`, and treating `'` as an opener swallowed the whole
/// rest of the prompt in the same sample.
fn separator_split(prompt: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut start = 0usize;
    for (i, ch) in prompt.char_indices() {
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '`' => quote = Some(ch),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(&prompt[start..i]);
                start = i + ch.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&prompt[start..]);
    parts
}

/// Rejoins fragments that are too short to their NEIGHBOUR, instead of refusing the batch.
///
/// A comma inside ONE question must not fan out into
/// several. Measured: « what is the revenue, in euros, for Q3 » went out as
/// three searches, two of which (« in euros », « for Q3 ») are noise, and
/// the caller received up to three times the requested budget.
///
/// The first rule was « ALL fragments must stand alone, otherwise
/// none » — and **a single short fragment refused the whole batch**. Measured on
/// local transcripts on 2026-09-09: **31 refusals**, two of them the
/// same day, and the pattern is almost always two real questions plus a
/// two-word shorthand: « which agents not covered, which proxy
/// ideas, which blockers » went out again as ONE search. The refusal message
/// already said what to do — « fold it into its neighbour » — and made the
/// human do it.
///
/// Rejoining returns exactly the old result on the case that motivated the rule
/// (« in euros » and « for Q3 » join « what is the revenue », so a
/// single question) and a real battery on the over-refused case. A short
/// fragment AT THE HEAD has no left neighbour: it joins the next one.
pub(super) fn fold_short_fragments(parts: &[&str]) -> Vec<String> {
    let long: Vec<bool> = parts
        .iter()
        .map(|p| p.split_whitespace().count() >= MIN_WORDS_PER_QUESTION)
        .collect();
    // Without ANY standalone fragment, there is no batch: a single question.
    if !long.iter().any(|b| *b) {
        return vec![parts.join(", ")];
    }
    // Each group carries exactly one standalone fragment. A short one rejoins
    // the last standalone SEEN; those preceding the first rejoin the
    // next. That is what « Combien, au total, coûte le projet ? » requires:
    // the two short heads belong to the question that follows them, and a
    // leftward rejoin would have made « Combien, au total » of it — three
    // words, so standalone, so a second search on half a sentence.
    let mut out: Vec<String> = Vec::new();
    let mut lead: Vec<&str> = Vec::new();
    for (p, is_long) in parts.iter().zip(&long) {
        if *is_long {
            let mut group = lead.join(", ");
            if !group.is_empty() {
                group.push_str(", ");
            }
            group.push_str(p);
            lead.clear();
            out.push(group);
        } else if out.is_empty() {
            lead.push(p);
        } else {
            let last = out.last_mut().expect("non-empty");
            last.push_str(", ");
            last.push_str(p);
        }
    }
    out
}

pub fn split_scoped_prompts_counted(prompt: &str) -> (Vec<String>, usize) {
    let parts: Vec<&str> = separator_split(prompt)
        .into_iter()
        .map(str::trim)
        .filter(|s| s.chars().any(char::is_alphanumeric))
        .collect();
    if parts.len() < 2 {
        return (parts.into_iter().map(str::to_string).collect(), 0);
    }
    // A comma inside ONE question does not fan out into several:
    // `fold_short_fragments` rejoins fragments that are too short to their neighbour.
    // The rule and its justification live there, once.
    let mut v = fold_short_fragments(&parts);
    let dropped = v.len().saturating_sub(MAX_SCOPED_PROMPTS);
    v.truncate(MAX_SCOPED_PROMPTS);
    (v, dropped)
}

// `answer_over_blob` — « the ONE retrieval core behind every
// ask-a-stash entry » — moved to `rag` with the dispatcher it
// calls. See `rag::answer_over_blob`.

/// Divisor mapping the distilled input size to the adaptive scoped
/// budget. A page just over the 12k intercept gate gets 12 000/16 =
/// 750 tokens — the same size class as a native WebFetch extract
/// (~700 tokens median, MEASURED) — and only genuinely huge pages
/// (≥24k distilled) reach the [`WEBFETCH_SCOPED_BUDGET_TOKENS`]
/// ceiling. The old fixed 1500 made every intercepted reply ~3× the
/// native extract even when the useful content was small.
pub const WEBFETCH_SCOPED_BUDGET_DIVISOR: usize = 16;

/// Floor for the adaptive scoped budget. Callers also scope extracted
/// documents (PDF/DOCX/…) barely over the 5k inline cap, where
/// `input/16` alone would starve the excerpt below one useful passage.
pub const WEBFETCH_SCOPED_BUDGET_FLOOR_TOKENS: usize = 500;

/// Adaptive scoped-extract budget for an input of `input_tokens`:
/// `clamp(input/16, 500, 1500)`. `TOKENADE_WEBFETCH_SCOPED_TOKENS`
/// (the existing fixed-budget override) pins the budget exactly and
/// bypasses adaptivity — measurement / kill-switch knob.
pub fn webfetch_scoped_budget_for(input_tokens: usize) -> usize {
    if let Some(n) = std::env::var("TOKENADE_WEBFETCH_SCOPED_TOKENS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
    {
        return n;
    }
    (input_tokens / WEBFETCH_SCOPED_BUDGET_DIVISOR).clamp(
        WEBFETCH_SCOPED_BUDGET_FLOOR_TOKENS,
        WEBFETCH_SCOPED_BUDGET_TOKENS,
    )
}
