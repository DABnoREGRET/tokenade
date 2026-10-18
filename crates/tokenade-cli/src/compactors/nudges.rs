//! The nudges that tell an agent it is paying for a habit.
//!
//! Sequential single-target web calls, repeated one-question prompts: both
//! measured, both throttled, neither ever blocking the call.

use super::*;

/// Re-base a WEB compaction's credit onto the detected agent's model (see
/// `coefficients::agent_web_binary_model`). These compactors are the MCP /
/// direct-CLI channel (`tokenade web|serp` ← `web_html_to_markdown`/
/// `serp_compact`), whose caller varies — so the honest `before` (raw HTML vs
/// reader-clean vs native-transparent) is per-agent. `raw` carries the raw
/// tokens_before / delivered tokens_after; delivery is untouched.
pub(super) fn credit_web(raw: &Savings) -> Savings {
    let (web_model, _) = tokenade_core::coefficients::agent_web_binary_model(
        tokenade_core::detect::from_env().as_str(),
    );
    tokenade_core::coefficients::web_credit_savings(web_model, raw.tokens_before, raw.tokens_after)
}

/// Re-base a BINARY-document compaction's credit onto the detected agent's
/// model. `after_tokens` = extracted-text tokens; the credit is the turn tax
/// avoided (or 0 for native-PDF agents), never `bytes/4`.
/// Credit a BINARY payload, deciding the model with the detected agent AND
/// whether the payload is a PDF.
///
/// The agent-only table answers `NativePdf` for Claude Code, and that is a claim
/// about PDFs: Claude renders one itself, so tokenade adds nothing. Nothing
/// renders a `.xlsx`, a `.docx` or an archive natively, and there the
/// extraction is exactly what makes the file readable — crediting it zero books
/// a real saving as none. There is no format-blind variant any more, because
/// the format-blind question has no right answer.
pub(super) fn credit_binary_raw(is_pdf: bool, after_tokens: usize) -> Savings {
    let model = tokenade_core::coefficients::binary_model_for(
        tokenade_core::detect::from_env().as_str(),
        is_pdf,
    );
    tokenade_core::coefficients::binary_credit_savings(model, after_tokens)
}

/// [`credit_binary_raw`] for a caller holding a `docs::Format`.
pub(super) fn credit_binary_for(fmt: tokenade_core::docs::Format, after_tokens: usize) -> Savings {
    credit_binary_raw(
        matches!(fmt, tokenade_core::docs::Format::Pdf),
        after_tokens,
    )
}

/// Nudge the agent to BATCH its questions, fired at the moment it proves it
/// isn't: the 2nd (and later) single-question `--prompt` against the SAME
/// source in one session. Measured on the docs bench: agents issued 2-4
/// separate `--prompt` calls on one document where one comma-separated call
/// would have answered everything in a single round-trip.
///
/// Only fires when the CURRENT call carries ONE question — an agent that
/// already batched is doing the right thing and must not be nagged. State is a
/// per-(session, source) counter file in the existing read-pending dir; no
/// session id (plain CLI use) → no nudge, since we cannot tell a repeat from a
/// first call.
pub(super) fn nudge_batch_questions_if_repeated(source: &str, prompt: &str) {
    // A comma-separated ask is already batched: nothing to teach.
    if prompt.contains(',') {
        return;
    }
    let Some(skey) = tokenade_core::session::session_key() else {
        return;
    };
    let key = format!(
        "askct-{}-{}",
        skey.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(24)
            .collect::<String>(),
        &tokenade_core::dedup_ref::hash_content(source.as_bytes())[..12]
    );
    let path = crate::hooks::read_staging::read_pending_dir().join(key);
    let seen: u32 = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| t.trim().parse().ok())
        .unwrap_or(0);
    tokenade_core::platform::ensure_parent_dir(&path);
    let _ = std::fs::write(&path, (seen + 1).to_string());
    if seen == 0 {
        return; // first question on this source is legitimate
    }
    eprintln!(
        "[tokenade] that's question #{} on this source. Ask them together next time — \
         `--prompt \"q1, q2, q3\"` is the CHEAPEST option in tokens spent: it answers each \
         under its own heading in ONE call, \
         instead of one round-trip per question.",
        seen + 1
    );
    tokenade_core::gain::record_nudge(
        "batch-questions",
        "fire",
        tokenade_core::gain::NudgeMetric::Occurrence((seen + 1) as usize),
    );
}

/// Nudge an agent that fetches/searches ONE target at a time toward the batch
/// form (D13). Counts single-target web-calls per session across BOTH channels
/// (the WebFetch/WebSearch hook AND the `tokenade web`/`search` CLI) via a
/// SHARED session counter; on the 2nd+ in a row it fires ONCE per channel, then
/// stays quiet. A batch call resets the counter — the agent learned. The counter
/// is per-session (each agent its own session), so no cross-process coordination
/// is needed, unlike the breaker file (D12).
///
/// `channel` is the unique caller identity (`cli-web`, `hook-search`, …) used
/// only for the once-per-channel fired marker; `kind` is `fetch` or `search`,
/// used only for the wording.
/// `2` → `2nd`. The nudge printed "that's your 2th single web fetch", which is
/// a small thing that reads as carelessness in a tool whose whole pitch is
/// that its numbers are right.
pub(super) fn ordinal(n: u32) -> String {
    let suffix = match (n % 100, n % 10) {
        (11..=13, _) => "th",
        (_, 1) => "st",
        (_, 2) => "nd",
        (_, 3) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

pub(crate) fn nudge_sequential_web_calls(channel: &str, kind: &str, is_batch: bool) {
    let Some(skey) = tokenade_core::session::session_key() else {
        return;
    };
    let tag: String = skey
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(24)
        .collect();
    let dir = crate::hooks::read_staging::read_pending_dir();
    let _ = std::fs::create_dir_all(&dir);
    let count_path = dir.join(format!("webseq-{tag}"));
    if is_batch {
        // The agent batched: reset so a later single call starts the count over.
        let _ = std::fs::remove_file(&count_path);
        return;
    }
    let seen: u32 = std::fs::read_to_string(&count_path)
        .ok()
        .and_then(|t| t.trim().parse().ok())
        .unwrap_or(0);
    let _ = std::fs::write(&count_path, (seen + 1).to_string());
    if seen == 0 {
        return; // the first single call this session is legitimate
    }
    let fired_path = dir.join(format!("webseq-fired-{channel}-{tag}"));
    if fired_path.exists() {
        return; // at most once per session per channel — teach without nagging
    }
    let _ = std::fs::write(&fired_path, "1");
    eprintln!(
        "[tokenade] that's your {} single web {kind} this session — the next time you \
         need several, do them in ONE call: `tokenade web url1 url2 …` / \
         `tokenade search \"q1\" \"q2\" …` run concurrently — ONE round-trip instead \
         of N, and the context each extra turn would re-send is never re-sent.",
        ordinal(seen + 1)
    );
    tokenade_core::gain::record_nudge(
        "batch-web-calls",
        "fire",
        tokenade_core::gain::NudgeMetric::Occurrence((seen + 1) as usize),
    );
}

#[cfg(test)]
mod binary_credit_tests {
    use super::*;
    use tokenade_core::docs::Format;

    /// `credit_binary_for` must pass the FORMAT through, not a constant.
    ///
    /// Asserted against `credit_binary_raw` rather than against a number, so
    /// the test needs no agent in the environment and still pins the one thing
    /// this wrapper does: only a PDF is a PDF.
    #[test]
    fn only_a_pdf_is_treated_as_natively_rendered() {
        let n = 4_000;
        assert_eq!(
            credit_binary_for(Format::Pdf, n).tokens_before,
            credit_binary_raw(true, n).tokens_before,
            "a PDF must take the native-rendering path"
        );
        for fmt in [
            Format::Xlsx,
            Format::Docx,
            Format::Epub,
            Format::Odt,
            Format::Rtf,
        ] {
            assert_eq!(
                credit_binary_for(fmt, n).tokens_before,
                credit_binary_raw(false, n).tokens_before,
                "{fmt:?} was treated as natively rendered"
            );
        }
        // Non-vacuity: on an agent that renders PDFs the two answers differ,
        // so the equalities above are not both trivially the same number.
        let pdf = tokenade_core::coefficients::binary_model_for("claude-code", true);
        let other = tokenade_core::coefficients::binary_model_for("claude-code", false);
        assert_ne!(pdf, other, "the distinction this wrapper carries is gone");
    }
}
