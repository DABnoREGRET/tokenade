//! **When the right answer is to tell the agent to do it DIFFERENTLY.**
//!
//! A 40,000-line `Read` has no good view: folding it returns an
//! approximation, letting it through costs the window. The third answer is to
//! name the tool that actually answers the question — `search`, or a bounded
//! read.
//!
//! The unknown parameter falls under the same caution: a `Read` with an
//! argument we do not understand must not be folded on an assumption.

use super::*;

/// Deliver a Read compaction to the agent — allow+updatedInput flow by
/// default, legacy deny as fallback. `reason` is the full banner +
/// compacted content a deny would have carried.
/// S1 — first key on a Read's `tool_input` that tokenade cannot faithfully
/// reproduce after a rewrite. The compaction exits rewrite `updatedInput` to
/// `{file_path,limit,offset}` (staged) or substitute the whole output (legacy
/// deny); any OTHER key (`pages` on a PDF today, a future param tomorrow)
/// would be silently dropped, mutilating the agent's call and triggering a
/// re-Read spiral. `None` ⇒ every key is known ⇒ the rewrite is safe. This
/// generalises `reads_pdf_natively` to any param / format / agent.
pub(crate) fn unknown_read_param(tool_input: &serde_json::Value) -> Option<String> {
    const KNOWN_READ_KEYS: &[&str] = &["file_path", "limit", "offset"];
    tool_input
        .as_object()?
        .keys()
        .find(|k| !KNOWN_READ_KEYS.contains(&k.as_str()))
        .cloned()
}

/// S3 — does the agent's REAL post-rewrite `tool_response` show the invariant
/// ("the native read was bounded to a head slice") is broken? The staged
/// compaction is ADDITIVE: it only pays off when the native output is small.
/// If it is large (updatedInput not applied, a format rendered natively like
/// a PDF in vision, or any future leak), injecting our stash on top is pure
/// inflation, so the caller must skip it. Generous line slack absorbs
/// line-number prefixes / wrapper text; the token cap also catches the
/// few-lines-but-huge case (base64 image blobs). `expected_head` is the line
/// limit we asked for (≤ READ_HEAD_LINES on the staged path). Empty response
/// ⇒ fail-open (nothing measurable ⇒ inject as before).
pub(crate) fn native_read_breaks_invariant(resp_str: &str, expected_head: usize) -> bool {
    if resp_str.is_empty() {
        return false;
    }
    let lines_blown = resp_str.lines().count() > expected_head.saturating_mul(3) + 20;
    let tokens_blown = tokenade_core::tokenizer::estimate(resp_str) > 2500;
    lines_blown || tokens_blown
}

/// Inline token budget for a compacted Read view. Above this we inject NO
/// compacted content at all — a digest this large is not worth re-serving on
/// every later turn (it rides in `additionalContext`, so its cost is paid each
/// turn it stays in context), and for needle/extraction tasks it is pure noise.
/// Measured: log-needle-zh, a 145k→20k-token CJK log re-served as
/// additionalContext drove session cost +83% vs a control that just grepped.
pub(crate) const READ_INLINE_TOKEN_CAP: usize = 5_000;

/// The steer delivered in place of a too-large compacted view: keep the small
/// native head (format) and point the agent at the cheap way to interrogate the
/// file — a script (only its stdout returns) or grep-like tools — instead of
/// pulling the whole thing into context.
pub(crate) fn read_too_large_steer(file_path: &str, compacted_tokens: usize) -> String {
    format!(
        "[tokenade: {file_path} is too large to pull into context — even compacted it is ~{compacted_tokens} \
         tokens, over the inline budget. Reading it whole (or re-reading a saved \
         tool-results/*-additionalContext.txt) would flood the session — don't. Analyse it with a script: \
         `tokenade exec --lang python --script '<code that prints only the answer>'` runs a capped \
         subprocess with your permissions and returns only its stdout. Or pull specific lines with grep-like tools (`grep -n PATTERN {file_path}`). \
         The native head shown above is there for format.]"
    )
}

/// Generic large-file Read steer threshold: only nudge toward grep/exec when
/// the source spans far more lines than the [`READ_HEAD_LINES`] head slice, so
/// we never nag on a small file whose view is delivered whole.
pub(crate) const READ_SEARCH_STEER_MIN_LINES: usize = 200;

/// Internal A/B gate + decision for the generic large-file Read search steer.
/// Fires ONLY for a generic (non-specific-compactor) Read whose compacted view
/// was kept inline — NOT already swapped for `read_too_large_steer` — and whose
/// source spans many lines. That is the exact regime where the 20-line head
/// slice can drop a mid-file needle AND the line-dedup'd view may have folded
/// the one-off line the agent is hunting, with nothing telling it to grep (the
/// Bash path has `emit_head_slice_advisory`; the Read path had no equivalent →
/// re-Read/re-grep spiral, measured on log-needle). Off with
/// `TOKENADE_NO_READ_SEARCH_STEER` (escape hatch / A/B).
pub(crate) fn should_search_steer(
    from_specific_compactor: bool,
    too_large_steered: bool,
    total_lines: usize,
) -> bool {
    if from_specific_compactor || too_large_steered {
        return false;
    }
    if std::env::var_os("TOKENADE_NO_READ_SEARCH_STEER").is_some() {
        return false;
    }
    total_lines > READ_SEARCH_STEER_MIN_LINES
}

/// Steer appended to a generic large-file Read's compacted view: the head
/// slice + line-dedup can hide a specific one-off value, so point the agent at
/// grep / `tokenade exec` for the exact line instead of a blind re-Read.
pub(crate) fn read_search_steer(file_path: &str, total_lines: usize) -> String {
    format!(
        "\n\n[tokenade: {file_path} has {total_lines} lines and the view above folds \
         repeated lines, so a specific one-off value you may be searching for (a single \
         timestamp, id, request, or error) can be absent here even though it IS in the \
         file. If what you need is NOT above, pull the exact line with \
         `grep -n PATTERN {file_path}` or `tokenade exec --lang python --script '<code>'` \
         — don't re-Read the whole file.]"
    )
}
