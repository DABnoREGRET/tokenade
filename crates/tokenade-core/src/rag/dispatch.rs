//! **The CHOICE of retrieval engine, and the batch of questions that goes
//! through it.**
//!
//! These three functions used to live in `disclose`, the fold gate, because
//! it was the first one to call them. Choosing between the flat extractor and
//! the structural engine, however, forces you to NAME both — and the
//! structural engine requires `embedding`, so the fold gate was naming a layer
//! above itself. It was the last upward edge in the repository, and it had
//! hidden behind a `'"'` literal that the layer guard's scrubber did not
//! model.
//!
//! Moved up here, they all point downward: `rag` knows both engines,
//! `disclose` knows only one — its own. What stayed down there is what has no
//! opinion about the engine: splitting a prompt into questions, sharing the
//! budget, saying that a fragment was folded.

use crate::disclose::{
    is_record_stream, prompt_scoped_extract, scoped_budget_per_question,
    split_scoped_prompts_counted, warn_if_batch_was_declined, webfetch_scoped_budget_for,
    MAX_SCOPED_PROMPTS,
};

/// Dispatch the WebFetch scoped extract between the flat BM25 baseline
/// ([`prompt_scoped_extract`]) and the structural + dense-RAG extractor
/// ([`super::prompt_scoped_extract_rag`]).
///
/// The RAG path is ON by default since 0.8.13 — the 2026-07-09 THOL A/B
/// (flat vs RAG, 4 reps × 2 web tasks, same binary) measured RAG at
/// −18% cost on web-research-oss-inventory ($3.28 vs $3.99) and −9% on
/// web-research-deep with equal-or-better scores, reversing the earlier
/// v2 net-negative verdict (the regression was section-packing, fixed in
/// the dense-augmented-flat v3). `TOKENADE_RAG=0` (or the older
/// `TOKENADE_WEB_RAG=0`) opts back into the flat BM25 baseline. If the RAG
/// extractor declines it falls back internally, so behaviour degrades to the
/// baseline, never below it.
pub fn prompt_scoped_extract_dispatch(
    text: &str,
    prompt: &str,
    budget_tokens: usize,
) -> Option<String> {
    dispatch_answers(text, prompt, budget_tokens).0
}

/// [`prompt_scoped_extract_dispatch`] for a question the AGENT asked through a
/// tokenade command (`read`, `web`, `expand-ref`, `disclose` with `--prompt`):
/// N questions answered in one call are N−1 round-trips it did not make, booked
/// as batching (474 such calls in 30 days of local transcripts, never credited
/// before). NOT for the WebFetch hook — the native tool answers a multi-part
/// prompt in one turn too, so no turn was saved there — nor for a search that
/// asks each page the same query.
///
/// Booked by `gain::prompt_scoped_extract_asked` (the product layer, which
/// may name this one — not the other way round); this half only says what was
/// answered: the extract, and the size of each answered question.
pub fn prompt_scoped_extract_answers(
    text: &str,
    prompt: &str,
    budget_tokens: usize,
) -> (Option<String>, Vec<usize>) {
    dispatch_answers(text, prompt, budget_tokens)
}

/// The scoped extract, and the size of each question it ANSWERED — a question
/// with no passage bought no turn.
fn dispatch_answers(
    text: &str,
    prompt: &str,
    budget_tokens: usize,
) -> (Option<String>, Vec<usize>) {
    // Multi-question: an agent can ask several things in ONE call by separating
    // them with commas. Each sub-question runs its own retrieval and gets its
    // own labelled block, so the agent can tell which passages answer which
    // question — and pays one tool round-trip instead of N. A value with no
    // comma is the common case and stays byte-for-byte as before: no split, no
    // header, so single-question callers are untouched.
    let (parts, dropped) = split_scoped_prompts_counted(prompt);
    if parts.len() <= 1 {
        warn_if_batch_was_declined(prompt);
        return (dispatch_scoped_one(text, prompt, budget_tokens), Vec::new());
    }
    // A dropped question must never pass for an answered one. Say it on BOTH
    // channels: stdout carries it to an MCP caller (which only ever sees
    // stdout), stderr carries it when every answered question missed and the
    // document below is discarded by the `any` gate.
    if dropped > 0 {
        eprintln!(
            "[tokenade] only the first {MAX_SCOPED_PROMPTS} questions of that prompt were \
             searched for — {dropped} more were not asked. Split them across two calls."
        );
    }
    let per_question = scoped_budget_per_question(budget_tokens, parts.len());
    let mut out = String::new();
    let mut any = false;
    let mut answered: Vec<usize> = Vec::new();
    for (i, q) in parts.iter().enumerate() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        // Echo the question in the header so the reader can map passages back to
        // it. Each question gets the FULL budget: N batched questions are the
        // honest equivalent of N separate calls, and the win is fewer round-
        // trips, not thinner answers.
        out.push_str(&format!("### Q{} · {q}\n", i + 1));
        match dispatch_scoped_one(text, q, per_question) {
            Some(block) => {
                any = true;
                answered.push(crate::tokenizer::estimate(block.trim()));
                out.push_str(block.trim());
            }
            None => out.push_str("_(no passage in this document matched this question)_"),
        }
    }
    if dropped > 0 {
        out.push_str(&format!(
            "\n\n_({dropped} further question(s) in that prompt were not searched for — \
             at most {MAX_SCOPED_PROMPTS} run in one call. Ask the rest in a second call.)_"
        ));
    }
    // Every question missed → behave exactly like a single miss, so the caller's
    // fallback (serve the compacted text / a hint) fires unchanged.
    (any.then_some(out), answered)
}

/// One question against the scoped extractor (RAG, or flat BM25 when
/// `TOKENADE_RAG=0` / the older `TOKENADE_WEB_RAG=0` opts back into it — kept
/// working because it was the documented escape hatch and may sit in someone's
/// script; a rename must not silently turn a user's opt-out back on). If the
/// RAG extractor declines it falls back internally, so behaviour degrades to
/// the baseline, never below it.
fn dispatch_scoped_one(text: &str, prompt: &str, budget_tokens: usize) -> Option<String> {
    let rag_on = std::env::var("TOKENADE_RAG")
        .or_else(|_| std::env::var("TOKENADE_WEB_RAG"))
        .map(|v| {
            let v = v.trim();
            !(v == "0" || v.eq_ignore_ascii_case("false") || v.eq_ignore_ascii_case("off"))
        })
        .unwrap_or(true);
    // A record stream gets the LINE extractor even with RAG on. Block
    // retrieval assumes a passage has a topic, and over `grep -r` output or a
    // log it does not: the neighbours of a hit belong to unrelated files, so
    // the block around the answer is mostly noise the agent still pays for.
    //
    // MEASURED on a 14 044-line grep stash, asking `value_is_code`:
    //   block retrieval  5 561 chars, answer on line 60 of 67
    //   line retrieval     754 chars, answer on line 4 of 9
    // and on a natural-language question neither finds it, but the line path
    // costs 5 590 chars against 6 934. Cheaper when it wins, cheaper when it
    // loses. Prose is untouched — it is not a record stream.
    if rag_on && !is_record_stream(text) {
        super::prompt_scoped_extract_rag(text, prompt, budget_tokens)
    } else {
        prompt_scoped_extract(text, prompt, budget_tokens)
    }
}

/// Answer question(s) against an already-recovered blob — the ONE retrieval
/// core behind every "ask a stash/ref" entry point (`expand-ref --prompt`, the
/// `search-stash` alias, the MCP `disclose search` twin), so they cannot drift.
///
/// **The whole blob is served only when searching cannot save anything.**
///
/// The rule used to be "below the whole-vs-RAG threshold, serve everything" —
/// the same threshold as the no-question route. MEASURED on 2026-09-14 on **19
/// questions actually asked by agents**, replayed on the blobs still cached
/// (`rag_eval --stash`): **8 of the 19 returned the WHOLE blob**, between 1,759
/// and 4,767 tokens, even though the agent had ASKED a question. Searching
/// anyway returns **3,506 characters on average instead of 7,063** — and the
/// recall of the tokens the question NAMES (`updatedToolOutput`,
/// `access-by-path`: checkable without a judge) does not move, **45/46 in both
/// cases**.
///
/// The fold threshold answers "does this text deserve to be folded?". Here the
/// question is a different one: "is this text bigger than what I can return?".
/// The right bound is therefore the BUDGET, not the threshold: below it,
/// extraction would return the whole text anyway; above it, it has something
/// to choose. `None` when nothing matches.
pub fn answer_over_blob(text: &str, prompt: &str) -> Option<String> {
    let toks = crate::tokenizer::estimate(text);
    let budget = webfetch_scoped_budget_for(toks);
    if toks <= budget {
        return Some(text.to_string());
    }
    prompt_scoped_extract_dispatch(text, prompt, budget)
}

/// [`answer_over_blob`] for a question the agent asked through a tokenade
/// command, with the size of each answered question (booked by
/// `gain::answer_over_blob_asked`). A blob served whole answers every
/// question at once, as the first of N sequential calls would have: no turn
/// saved, nothing answered to book.
pub fn answer_over_blob_answers(text: &str, prompt: &str) -> (Option<String>, Vec<usize>) {
    let toks = crate::tokenizer::estimate(text);
    let budget = webfetch_scoped_budget_for(toks);
    if toks <= budget {
        return (Some(text.to_string()), Vec::new());
    }
    prompt_scoped_extract_answers(text, prompt, budget)
}

#[cfg(test)]
mod shared_budget {
    use super::*;

    /// The failure the share exists to prevent, end to end: sixteen questions
    /// used to assemble past the delivery bound and come back with SIX
    /// headings, ten questions gone and nothing saying so.
    #[test]
    fn every_question_of_a_long_batch_comes_back_with_its_own_heading() {
        let doc = (0..400)
            .map(|i| {
                format!(
                    "## Section {i}\nThe retention policy for records of class {i} keeps them for \
                     {} days, after which the collector archives them to cold storage and the \
                     dashboard stops counting them.\n\n",
                    i * 3 + 30
                )
            })
            .collect::<String>();
        let questions: Vec<String> = (0..16)
            .map(|i| format!("how long are records of class {} kept", i * 7))
            .collect();
        let one = prompt_scoped_extract_dispatch(&doc, &questions[0], 1500)
            .expect("a single question is answered");
        let many = prompt_scoped_extract_dispatch(&doc, &questions.join(", "), 1500)
            .expect("a batch is answered");
        for i in 1..=16 {
            assert!(
                many.contains(&format!("### Q{i} ·")),
                "question {i} of 16 came back with no heading of its own"
            );
        }
        assert!(
            many.len() < one.len() * 4,
            "sixteen questions cost {} bytes where one costs {} — the budget is \
             being spent once per question",
            many.len(),
            one.len()
        );
    }
}

#[cfg(test)]
mod turns_saved {
    use super::*;

    /// What the batching credit is computed from: one size per ANSWERED
    /// question, none for a miss, none for a single question.
    #[test]
    fn only_the_answered_questions_of_a_batch_count() {
        let doc = (0..200)
            .map(|i| format!("## Section {i}\nThe retention policy for records of class {i} keeps them for {} days before archiving.\n\n", i + 30))
            .collect::<String>();
        let (out, sizes) = dispatch_answers(&doc, "how long are records of class 7 kept, how long are records of class 90 kept, zebra quokka narwhal migration", 1500);
        let out = out.expect("answered");
        assert_eq!(out.matches("### Q").count(), 3, "{out}");
        assert_eq!(sizes.len(), 2, "{out}");
        assert!(sizes.iter().all(|&s| s > 0));
        assert!(
            dispatch_answers(&doc, "how long are records of class 7 kept", 1500)
                .1
                .is_empty()
        );
    }
}
