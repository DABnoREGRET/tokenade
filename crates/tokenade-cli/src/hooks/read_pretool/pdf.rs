//! **PDF, the only format some agents display THEMSELVES.**
//!
//! Intercepting there is net-harmful: the extracted text is added ON TOP of
//! the native read, and rewriting `updatedInput` drops the `pages` parameter
//! — `Read(pdf, pages:"1-12")` loses its pagination and the agent loops.
//! MEASURED (doc-pdf bench): native control **4 turns / $0.159**, when
//! intercepting **13 turns / $0.210**.
//!
//! This file therefore exists to STEP ASIDE at the right moment, which is the
//! opposite of what the rest of the directory does — and that is why it
//! stands alone.

use super::*;

/// PDFs on an agent that renders them NATIVELY: step aside, or redirect.
///
/// Intercepting there is net-harmful — the extracted text is injected ON TOP of
/// the native read AND the staged `updatedInput` rewrite strips the agent's
/// `pages` param, so `Read(pdf, pages:"1-12")` loses its pagination and the
/// agent loops. Measured (doc-pdf bench): native control 4 turns/$0.159,
/// intercepting 13 turns/$0.210.
pub(crate) fn serve_native_pdf(
    parsed: &serde_json::Value,
    file_path: &str,
    path: &std::path::Path,
    cwd: &std::path::Path,
    fmt: tokenade_core::docs::Format,
    size: u64,
    started: std::time::Instant,
) -> Option<i32> {
    // Native image read costs ~1.5k tokens/page, reloaded every
    // turn. For a PROSE PDF the extracted text is a faithful,
    // far cheaper stand-in (measured: a 20-page guide, ~30k tok
    // native vs ~8.8k tok text, with the granular detail — a
    // flow-rate figure the native+delegate path dropped —
    // preserved). So only KEEP the native read for PDFs the text
    // cannot represent: scans and diagram-heavy pages
    // (`pdf_is_text_rich` = false), or when the agent paginated
    // explicitly (`pages`/`offset`/`limit`) — replacing that
    // with a .txt strips the pagination and loops (the failure
    // the old blanket passthrough was guarding against). A
    // text-rich, un-paginated PDF falls through to extraction.
    // Extract ONCE. Judge text-density from that text plus a
    // cheap page count (not a second extraction), then bound it.
    let bytes = std::fs::read(path).ok();
    let redirect = bytes.as_deref().and_then(|b| {
        let text = tokenade_core::media::extract_document(b, fmt);
        if text.trim().is_empty() {
            return None;
        }
        let pages = tokenade_core::docs::pdf_page_count(b);
        // Keep the native image read for PDFs the text can't
        // represent (scans, diagrams). Otherwise REDIRECT the
        // Read to a stashed .txt of the extracted text, like the
        // image-downscale path rewrites `file_path`: the agent
        // reads faithful text (~3× cheaper than page images,
        // granular detail preserved). Pagination does NOT gate
        // this — Claude Code reads PDFs with `pages`/`limit` BY
        // DEFAULT, so gating on it disabled the redirect on
        // essentially every real read; the loop hazard it once
        // guarded (rewriting the PDF to `limit:20` with `pages`
        // still set) is gone because a .txt has no pages.
        if !tokenade_core::docs::text_is_pdf_text_rich(&text, pages) {
            return None;
        }
        // The tout-vs-RAG threshold (the ~5k we settled on for
        // the web path): under it, serving the whole text is
        // cheaper and loses nothing; over it, the document is
        // too big to load whole. Serving its HEAD would be waste
        // — the answer is rarely in the first pages, and the
        // agent has to run a retrieval anyway — so serve NOTHING
        // but a pointer: its size and how to retrieve any part.
        // A book-length PDF (measured: 290k words → ~495k tokens)
        // thus costs ~50 tokens here, not 495k, and `--prompt`
        // still reaches every section on demand.
        let full_tokens = tokenade_core::tokenizer::estimate(&text);
        let served = if full_tokens > tokenade_core::disclose::threshold_tokens() {
            format!(
                "[tokenade: this PDF's text is ~{full_tokens} tokens — too large to \
                 load whole. Ask it what you need (RAG): \
                 `tokenade read {file_path} --prompt \"<what you need>\"` returns \
                 just the relevant passages. Have several questions about this \
                 document? Putting them ALL in ONE call, comma-separated — \
                 `--prompt \"q1, q2, q3\"` — is the CHEAPEST way to read it: each \
                 is answered under its own heading, one round-trip instead of N. Read it whole only if you genuinely need \
                 every page.]"
            )
        } else {
            text
        };
        // stash_for_reference redacts at write time, so a secret
        // in the PDF never reaches the model via the redirect
        // (same guarantee as the other doc paths).
        let hint = {
            let mut p = path.to_path_buf().into_os_string();
            p.push(".txt");
            std::path::PathBuf::from(p)
        };
        tokenade_core::disclose::stash_for_reference(
            &served,
            &tokenade_core::disclose::SourceHint::File(hint),
        )
        .map(|(_, stash_path)| stash_path)
    });
    let text_rich = redirect.is_some();
    if let Some(stash_path) = redirect {
        // Dedup through the SAME primitives the image / identity
        // paths use — no new mechanism. If this exact PDF was
        // already served this session AND its text is still in
        // context (`ctx_claim_ok`, now generation-scoped so it
        // also holds INSIDE a subagent for that subagent's own
        // earlier read), don't re-serve it: a re-read of an
        // unchanged document is pure waste. Deny with a pointer
        // back (the text is in context above) plus the --prompt
        // nudge. The ctx_claim_ok circuit breaker full-serves
        // the 3rd retry, so a stuck agent self-heals.
        let pdf_hash = bytes.as_deref().map(tokenade_core::dedup_ref::hash_content);
        if let Some(h) = pdf_hash.as_deref() {
            let would_serve = std::fs::read_to_string(&stash_path)
                .map(|s| tokenade_core::tokenizer::estimate(&s))
                .unwrap_or(0);
            if doc_dedup_hit(parsed, cwd, file_path, h, would_serve, size, started) {
                return Some(0);
            }
        }
        let redirect_s = stash_path.to_string_lossy().into_owned();
        // Start from the original tool_input, point it at the
        // .txt and strip the page/line selectors — they don't
        // apply to the extracted text and would truncate it.
        let mut updated = parsed
            .get("tool_input")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        if let Some(obj) = updated.as_object_mut() {
            obj.insert("file_path".into(), serde_json::json!(redirect_s));
            obj.remove("pages");
            obj.remove("limit");
            obj.remove("offset");
        }
        // Nudge toward targeted retrieval. The redirect serves
        // the FULL text (safe: nothing lost, and already ~3×
        // What was served varies (full text under the
        // threshold, a pointer over it), so the hint asserts
        // neither. It ENCOURAGES a targeted question — cheaper
        // in most cases — without forbidding anything: the agent
        // may still read it whole or summarise it if that is what
        // it needs. No prompt is guessed on its behalf.
        // State the SIZE IN TOKENS: given the number the agent
        // picks the right verb by itself (small → just read it;
        // large → obviously ask), which steers better than any
        // amount of nudge wording.
        let served_tokens = tokenade_core::tokenizer::estimate(
            &std::fs::read_to_string(&stash_path).unwrap_or_default(),
        );
        let hint = format!(
            "tokenade: this is a text PDF, served as text (~{served_tokens} \
             tokens; page images would cost ~3× that). If you're after \
             something specific, a targeted question usually costs far less \
             than reading it whole (RAG): \
             `tokenade read {file_path} --prompt \"<what you need>\"` \
             returns just the relevant passages. Several questions about this \
             document? Putting them ALL in ONE call, comma-separated — \
             `--prompt \"q1, q2, q3\"` — is the CHEAPEST way to read it: each is \
             answered under its own heading, one round-trip instead of N. For the original page images, Read \
             `{file_path}` with a `pages` range.",
        );
        let decision = serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow",
                "permissionDecisionReason":
                    "tokenade: text-PDF served as extracted text, not page images",
                "updatedInput": updated,
                "additionalContext": hint,
            }
        });
        crate::log_emitted_additional_context("read-pretool-pdf", "PreToolUse", &hint);
        println!("{decision}");
        // First sighting: register identity so a re-read dedups.
        if let Some(h) = pdf_hash.as_deref() {
            doc_dedup_register(parsed, cwd, file_path, h);
        }
        tokenade_core::debug_log::log(
            "hookwrap-pretool",
            "read_pdf_text_redirect",
            serde_json::json!({
                "file": file_path,
                "size": size,
                "elapsed_ms": started.elapsed().as_millis() as u64,
            }),
        );
        return Some(0);
    }
    // image-poor or extraction/stash failed → native read is
    // the right (or only) answer.
    tokenade_core::debug_log::log(
        "hookwrap-pretool",
        "read_binary_pdf_passthrough_native",
        serde_json::json!({
            "file": file_path,
            "size": size,
            "reason": if !text_rich { "image_poor" } else { "extract_failed" },
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
    Some(0)
}
