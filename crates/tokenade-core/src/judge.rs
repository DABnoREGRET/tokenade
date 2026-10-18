//! Answer-groundedness judge (sigmap-inspired, flagged 2026-06-09).
//!
//! The offline accuracy bench proves "no quality loss" over a *fixed corpus*.
//! This module proves it for **one artifact on demand**: given a question, a
//! raw output, and a compacted output, it answers the question against each
//! side and reports whether the compaction preserved the answer. It is the
//! shippable counterpart to `tokenade-bench`'s `accuracy` harness — the same
//! `ANTHROPIC_API_KEY` + `TOKENADE_BENCH_MODEL` contract — so a paid user can
//! verify a single compaction on real content, not just trust the corpus.
//!
//! Two grounding modes:
//!   * **span mode** (`--expect a,b`): the answer is grounded iff every
//!     expected span survives in the model's reply — the exact predicate the
//!     offline gate uses (`re:` prefix = regex).
//!   * **referee mode** (no spans): a second model call judges whether the
//!     compacted-side answer is *equivalent* to the raw-side answer. This is
//!     sigmap's groundedness check generalized to free-form answers.
//!
//! Every network call shells out to `curl` (no HTTP crate), and any failure
//! (missing key, missing curl, network/parse error) degrades to `None` so the
//! judge is never a hard dependency.

use crate::tokenizer;

/// Model used for the answer + referee calls. Defaults to Haiku (cheap; the
/// task is extraction/equivalence, not generation). Shared with the bench so a
/// user can pin one model across both via `TOKENADE_BENCH_MODEL`.
/// **The ONLY reader of `TOKENADE_BENCH_MODEL`.** The measurement binary
/// carried a second copy, same literal default: two places answering the same
/// question end up answering differently.
pub fn judge_model() -> String {
    std::env::var("TOKENADE_JUDGE_MODEL")
        .or_else(|_| std::env::var("TOKENADE_BENCH_MODEL"))
        .unwrap_or_else(|_| "claude-haiku-4-5".to_string())
}

/// Case-insensitive substring match; an entry prefixed `re:` is a regex.
/// Mirrors `accuracy::contains_ci` so span scoring is identical across the
/// on-demand judge and the offline corpus gate.
pub fn contains_ci(haystack: &str, needle: &str) -> bool {
    if let Some(pat) = needle.strip_prefix("re:") {
        return regex::Regex::new(pat)
            .map(|re| re.is_match(haystack))
            .unwrap_or(false);
    }
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// True iff `answer` contains every expected span.
pub fn grounded_in_spans(answer: &str, expect: &[String]) -> bool {
    expect.iter().all(|m| contains_ci(answer, m))
}

/// POST a single user `prompt` to the Anthropic Messages API over the shared
/// in-process HTTP client. Returns the concatenated text content, or `None` on
/// any failure.
fn anthropic_text(prompt: &str, max_tokens: u32) -> Option<String> {
    let key = std::env::var("ANTHROPIC_API_KEY").ok()?;
    let body = serde_json::json!({
        "model": judge_model(),
        "max_tokens": max_tokens,
        "messages": [{ "role": "user", "content": prompt }],
    });
    let resp = crate::net::http_post_json(
        "https://api.anthropic.com/v1/messages",
        &[
            ("anthropic-version", "2023-06-01"),
            ("x-api-key", key.as_str()),
        ],
        &body,
        120,
    )?;
    let v: serde_json::Value = resp.json()?;
    let text = v
        .get("content")?
        .as_array()?
        .iter()
        .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
        .collect::<Vec<_>>()
        .join("");
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Ask `question` using ONLY `context`. Returns the model's terse answer.
pub fn ask_llm(context: &str, question: &str) -> Option<String> {
    let prompt = format!(
        "You are given the output of a command. Answer the question using ONLY that output, as briefly as possible.\n\n--- OUTPUT ---\n{context}\n--- END OUTPUT ---\n\nQuestion: {question}"
    );
    anthropic_text(&prompt, 256)
}

/// Referee call: does `candidate` convey the same answer as `reference` for
/// `question`? Used in referee mode (no expected spans). Returns `Some(true)`
/// / `Some(false)`, or `None` if the call/parse failed.
pub fn equivalent(question: &str, reference: &str, candidate: &str) -> Option<bool> {
    let prompt = format!(
        "Two answers were given to the same question. Decide if the SECOND answer conveys the same essential information as the FIRST (ignore wording, length, and formatting differences). Reply with exactly one word: YES or NO.\n\nQuestion: {question}\n\nFIRST (reference):\n{reference}\n\nSECOND (candidate):\n{candidate}"
    );
    let reply = anthropic_text(&prompt, 8)?;
    let head = reply.trim().to_lowercase();
    if head.starts_with("yes") {
        Some(true)
    } else if head.starts_with("no") {
        Some(false)
    } else {
        None
    }
}

/// Grounding of one side (raw or compacted) of a judged comparison.
#[derive(Debug, Clone)]
pub struct Grounding {
    /// The model's answer against this side's context (None on call failure).
    pub answer: Option<String>,
    /// In span mode: every expected span survived. In referee mode this is
    /// left `true` for the reference (raw) side and set by the referee for the
    /// candidate (compacted) side.
    pub grounded: bool,
}

/// Full verdict for one (question, raw, compacted) triple.
#[derive(Debug, Clone)]
pub struct Verdict {
    pub question: String,
    pub tokens_raw: usize,
    pub tokens_compacted: usize,
    pub savings_pct: f64,
    pub raw: Grounding,
    pub compacted: Grounding,
    /// Headline "no quality loss" signal: the compacted context preserved the
    /// answer. `None` when the judge could not run (no key / call failed).
    pub preserved: Option<bool>,
    /// Which mode produced the verdict, for display.
    pub mode: Mode,
}

/// How groundedness was decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Expected spans supplied — exact corpus-style gate.
    Spans,
    /// No spans — a referee model judged answer equivalence.
    Referee,
}

fn savings_pct(before: usize, after: usize) -> f64 {
    if before == 0 {
        return 0.0;
    }
    (before.saturating_sub(after) as f64) * 100.0 / before as f64
}

/// Judge whether `compacted` preserves the answer to `question` that `raw`
/// supports. When `expect` is non-empty, span mode is used; otherwise referee
/// mode. Network failures leave `preserved = None` (inconclusive, never a hard
/// error) — the same "unrunnable ⇒ don't fail" discipline as the live bench.
pub fn judge(question: &str, raw: &str, compacted: &str, expect: &[String]) -> Verdict {
    let tokens_raw = tokenizer::estimate(raw);
    let tokens_compacted = tokenizer::estimate(compacted);
    let raw_answer = ask_llm(raw, question);
    let compacted_answer = ask_llm(compacted, question);

    if !expect.is_empty() {
        let raw_g = Grounding {
            grounded: raw_answer
                .as_deref()
                .is_some_and(|a| grounded_in_spans(a, expect)),
            answer: raw_answer,
        };
        let cmp_g = Grounding {
            grounded: compacted_answer
                .as_deref()
                .is_some_and(|a| grounded_in_spans(a, expect)),
            answer: compacted_answer,
        };
        // Preserved iff the compacted side grounds the spans at least as well
        // as raw did (raw is the practical ceiling on a synthetic-free input).
        let preserved = match (cmp_g.answer.is_some(), raw_g.answer.is_some()) {
            (true, _) => Some(cmp_g.grounded || (!raw_g.grounded)),
            _ => None,
        };
        return Verdict {
            question: question.to_string(),
            tokens_raw,
            tokens_compacted,
            savings_pct: savings_pct(tokens_raw, tokens_compacted),
            raw: raw_g,
            compacted: cmp_g,
            preserved,
            mode: Mode::Spans,
        };
    }

    // Referee mode: need both answers, then a third equivalence call.
    let preserved = match (&raw_answer, &compacted_answer) {
        (Some(r), Some(c)) => equivalent(question, r, c),
        _ => None,
    };
    Verdict {
        question: question.to_string(),
        tokens_raw,
        tokens_compacted,
        savings_pct: savings_pct(tokens_raw, tokens_compacted),
        raw: Grounding {
            grounded: raw_answer.is_some(),
            answer: raw_answer,
        },
        compacted: Grounding {
            grounded: preserved.unwrap_or(false),
            answer: compacted_answer,
        },
        preserved,
        mode: Mode::Referee,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_ci_plain_and_regex() {
        assert!(contains_ci("The STATUS is CrashLoopBackOff", "crashloop"));
        assert!(contains_ci("exit code 137", "re:exit code \\d+"));
        assert!(!contains_ci("all good", "re:error \\d+"));
    }

    #[test]
    fn grounded_requires_every_span() {
        let spans = vec!["pod-7".to_string(), "OOMKilled".to_string()];
        assert!(grounded_in_spans("pod-7 was OOMKilled", &spans));
        assert!(!grounded_in_spans("pod-7 is Running", &spans));
    }

    #[test]
    fn savings_pct_basic() {
        assert_eq!(savings_pct(100, 25), 75.0);
        assert_eq!(savings_pct(0, 0), 0.0);
        // never panics / goes negative when compacted is somehow larger
        assert_eq!(savings_pct(10, 40), 0.0);
    }

    #[test]
    fn judge_is_inconclusive_without_key() {
        // With no ANTHROPIC_API_KEY the calls fail and preserved is None,
        // but token accounting still works and never panics.
        if std::env::var("ANTHROPIC_API_KEY").is_ok() {
            return; // skip when a real key is present (would make a live call)
        }
        let v = judge(
            "what is the status?",
            "STATUS: Running\n".repeat(40).as_str(),
            "STATUS: Running (x40)",
            &[],
        );
        assert!(v.tokens_raw > v.tokens_compacted);
        assert_eq!(v.preserved, None);
        assert_eq!(v.mode, Mode::Referee);
    }
}
