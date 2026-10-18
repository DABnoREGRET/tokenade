//! The non-text branch of the WebFetch hook: extract what we can from a
//! document, stash the raw bytes, and answer in ONE message.
//!
//! Split out of `handle_webfetch_pretool`, which was carrying this whole
//! decision tree inline. The fields the branch needs are gathered into
//! [`BinaryResponse`] at the call site, while the response is still in hand.

use super::*;

/// A fetched response the classifier called binary, plus the context the
/// answer needs. Borrowed where the text path below still needs the value.
pub(crate) struct BinaryResponse<'a> {
    pub(crate) url: &'a str,
    pub(crate) prompt: String,
    pub(crate) bytes: &'a [u8],
    pub(crate) content_type: Option<String>,
    /// `content_type` reduced to its bare mime, per `web_fetch`.
    pub(crate) normalized_type: Option<String>,
    /// What the bytes themselves look like, when the header did not say.
    pub(crate) detected_kind: Option<String>,
    pub(crate) disclosure: &'a Disclosure,
    pub(crate) started: std::time::Instant,
}

/// Answer a binary response. Always returns 0 — the agent is served through
/// the deny-as-success channel, exactly like the markdown path.
pub(crate) fn serve(response: BinaryResponse<'_>) -> i32 {
    let BinaryResponse {
        url,
        prompt,
        bytes: body_bytes,
        content_type,
        normalized_type: ct_normalized,
        detected_kind,
        disclosure,
        started,
    } = response;
    let web_hint = match &content_type {
        Some(ct) if !ct.is_empty() => {
            tokenade_core::disclose::SourceHint::web_with_content_type(ct.clone())
        }
        _ => tokenade_core::disclose::SourceHint::web(),
    };
    // #949 — V1 inline extraction pipeline.
    // When the detected kind (or CT) maps to a docs::Format we can
    // extract (PDF / DOCX / XLSX / PPTX / ODT / ODS / ODP / EPUB),
    // RUN the extraction synchronously and put the result directly
    // into the deny reason. Previously the agent had to take a
    // second tool-call ("tokenade docs path=<stash>") to get the
    // text; that's the roundtrip we observed agents skipping in
    // favor of WebSearch, losing the source.
    //
    // Decision tree per call:
    //   - bytes > MAX_EXTRACT_BYTES (20 MB) → skip extraction
    //   - kind/ct unmapped to Format → skip
    //   - extracted text empty → skip
    //   - extracted text ≤ INLINE_TOKENS (5 000) → inline
    //   - else → stash extracted text + reference its path
    // In every "skip"/failure case we fall through to the #947
    // generic hint + raw stash, preserving backward compat.
    let extraction = try_extract_webfetch(
        body_bytes,
        detected_kind.as_deref(),
        ct_normalized.as_deref(),
    );
    // #947 — actionable hint: tell the agent the exact command to
    // recover text from the stash, instead of just suggesting
    // `file` / `xxd` / `hexdump`. Used when extraction is unavailable.
    let kind_hint = pick_webfetch_recovery_hint(detected_kind.as_deref(), ct_normalized.as_deref());
    let stash_block = tokenade_core::disclose::maybe_stash_bytes(body_bytes.to_vec(), web_hint);
    // #955 v2 — A/B logic on the EXTRACTED document text too:
    //   ≤ inline cap → whole text inline (ext.inlined, handled by
    //   build_webfetch_extracted_reason);
    //   > cap + anchored prompt → prompt-scoped excerpt inline +
    //   stashed-text pointer (the answer reaches the model in ONE
    //   turn instead of a stash pointer forcing a second);
    //   > cap, no anchor → legacy path-referenced message.
    let scoped_doc = match &extraction {
        Some(ext) if !ext.inlined && !prompt.trim().is_empty() => {
            tokenade_core::rag::prompt_scoped_extract_dispatch(
                &ext.text,
                &prompt,
                tokenade_core::disclose::webfetch_scoped_budget_for(
                    tokenade_core::tokenizer::estimate(&ext.text),
                ),
            )
        }
        _ => None,
    };
    let body = match (&extraction, &scoped_doc) {
        (Some(ext), Some(extract)) => build_webfetch_scoped_doc_reason(
            url,
            body_bytes.len(),
            ext,
            extract,
            &prompt,
            &stash_block,
        ),
        (Some(ext), None) => {
            build_webfetch_extracted_reason(url, body_bytes.len(), ext, &stash_block)
        }
        (None, _) => {
            let banner = format!(
                "[tokenade:webfetch-binary] {url} returned binary content ({} bytes)\n\
                 {kind_hint}",
                body_bytes.len(),
            );
            format!("{banner}\n\n{stash_block}")
        }
    };
    // Same PreToolUse-deny-as-success channel as the markdown path: lead
    // with an unambiguous success marker so the <error> wrapper Claude Code
    // adds around a deny reason doesn't read as a failed fetch + retry.
    // Same status-aware marker as the markdown path: a PDF URL that
    // answered 404 must not be announced as a success either.
    let reason = format!("{}\n{body}", disclosure.lead_marker());
    let decision = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    });
    println!("{decision}");
    // #950 — credit the savings in gain.jsonl when extraction
    // succeeded so dashboards (gain / dashboard / statusline)
    // reflect what tokenade actually saved the agent. The text path
    // a few lines below already does this for `webfetch_compact`;
    // the binary-extraction path was missing the equivalent until
    // now, leaving the new V1 pipeline invisible to telemetry.
    //
    // Framing: before = raw bytes ÷ 4 (the cost an agent would pay
    // if they naively cat-piped the binary into context — same
    // ratio docs::extract_with_savings uses); after = extracted
    // text tokens. Op `webfetch_binary_extracted` is distinct from
    // `webfetch_compact` so the dashboard can attribute the two
    // routes independently. Skipped when extraction was unavailable
    // (no Format mapped / oversized / parse failed) — falling
    // through to the generic-hint path means no real savings were
    // generated this turn.
    if let Some(ext) = &extraction {
        // BINARY credit on the WebFetch-URL path — NOT the fictional
        // `bytes/4` (up to ~553k for a PDF the agent never ingests raw).
        // NOTE: ALWAYS ExtractOrFail here, regardless of the per-agent
        // table: `NativePdf` (Claude) only covers the LOCAL Read path
        // (document block, `pages`). The native WebFetch on a PDF *URL*
        // was MEASURED to fail — the server-side model receives raw
        // FlateDecode bytes and answers "unable to answer… encoded/
        // compressed data". So even for Claude, serving the extraction
        // saves the failed attempt + rescue turns (BINARY_TURN_TAX).
        // Delivery is untouched — this only fixes the credit assiette.
        let sav = tokenade_core::coefficients::binary_credit_savings(
            tokenade_core::coefficients::BinaryCreditModel::ExtractOrFail,
            ext.tokens,
        );
        gain_log::record("hook", "webfetch_binary_extracted", url, sav);
    }
    tokenade_core::debug_log::log(
        "hookwrap-pretool",
        "webfetch_binary_short_circuit",
        serde_json::json!({
            "url": url,
            "bytes": body_bytes.len(),
            "content_type": content_type,
            "extracted_format": extraction.as_ref().map(|e| e.format_label),
            "extracted_tokens": extraction.as_ref().map(|e| e.tokens),
            "extracted_inlined": extraction.as_ref().map(|e| e.inlined),
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
    0
}
