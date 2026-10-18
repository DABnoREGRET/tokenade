//! **What is not text** — documents, media, binaries.
//!
//! A `.docx`, a `.mp4`, an image: the agent's `Read` would fail on the bytes,
//! or worse, deliver them. Each family gets here the only answer that makes
//! sense for it — the extracted text, a transcript, a summary of what the
//! file IS.
//!
//! The common rule is never to return less than a readable refusal: saying
//! "this file is a 42-minute MP4, no transcript available" beats an encoding
//! error, because the agent can act on the former and not on the latter.

use super::*;

/// What a Read of an audio/video file should return: what it IS, plus how to
/// ask what is SAID in it. Mirrors the CLI media serve, minus any transcript
/// body — the hook's job is to orient cheaply (~100 tokens), and a transcript
/// belongs behind an explicit question.
pub(crate) fn media_summary_for_hook(
    path: &std::path::Path,
    kind: tokenade_core::media::MediaKind,
    file_path: &str,
) -> String {
    let mut out = format!("[tokenade:{} {file_path}]\n", kind.as_str());
    match tokenade_core::media::probe_summary(path) {
        Some(p) => {
            out.push_str(&p);
            out.push('\n');
        }
        None => out.push_str("(no ffprobe: duration/codecs/tracks unavailable)\n"),
    }
    out.push_str(&format!(
        "\nThis is {} — reading its bytes as text is meaningless. To work with what is SAID \
         in it: `tokenade read {file_path} --prompt \"q1, q2\"` (uses a sidecar .srt/.vtt or an \
         embedded subtitle track when present; several questions at once, each answered under \
         its own heading).",
        kind.as_str()
    ));
    if let Some(nudge) = tokenade_core::media::install_nudge(kind) {
        out.push_str(&format!("\n[tokenade] {nudge}."));
    }
    out
}

/// Extract the document's text and answer with it.
pub(crate) fn serve_document_text(
    parsed: &serde_json::Value,
    file_path: &str,
    path: &std::path::Path,
    cwd: &std::path::Path,
    fmt: tokenade_core::docs::Format,
    size: u64,
    started: std::time::Instant,
) -> Option<i32> {
    if let Ok(bytes) = std::fs::read(path) {
        let text = tokenade_core::media::extract_document(&bytes, fmt);
        if !text.trim().is_empty() {
            let tokens = tokenade_core::tokenizer::estimate(&text);
            let label = docs_format_label(fmt);
            // Same content-addressed dedup as the PDF path, via
            // the shared helper: re-reading an unchanged office
            // doc (docx/xlsx/pptx/odt/odp/ods/epub/…) re-extracts
            // for nothing. Hash the RAW bytes so the identical
            // Read re-keys the same; register on first serve at
            // each return below.
            let doc_hash = tokenade_core::dedup_ref::hash_content(&bytes);
            if doc_dedup_hit(parsed, cwd, file_path, &doc_hash, tokens, size, started) {
                return Some(0);
            }
            // A spreadsheet is a map-and-script problem, not a
            // read problem, and `Read` is the channel agents
            // actually use — so the skeleton has to live here,
            // not only behind `tokenade web`. Without it this
            // path folds the table to a summary: a 5.8 MB sheet
            // extracts to 6.9M tokens and the agent gets an
            // arbitrary prefix of the rows, with no way to know
            // which rows it did NOT see. The map plus
            // `expand-ref --grep` answers exhaustively instead.
            if matches!(
                fmt,
                tokenade_core::docs::Format::Xlsx | tokenade_core::docs::Format::Ods
            ) && tokens > tokenade_core::disclose::read_threshold_tokens()
            {
                let stash_hint = {
                    let mut p = path.to_path_buf().into_os_string();
                    p.push(".txt");
                    std::path::PathBuf::from(p)
                };
                let data = tokenade_core::disclose::stash_for_reference(
                    &text,
                    &tokenade_core::disclose::SourceHint::File(stash_hint),
                );
                let data_ref = data
                    .as_ref()
                    .map(|(h, p)| (h.as_str(), p.to_string_lossy().into_owned()));
                if let Some(skel) = tokenade_core::docs::spreadsheet_skeleton(
                    &bytes,
                    fmt,
                    file_path,
                    data_ref.as_ref().map(|(h, p)| (*h, p.as_str())),
                ) {
                    emit_read_compaction(
                        parsed, file_path, 0,
                        0, /* extracted TEXT has no relation to the binary's offset/limit */
                        skel, false,
                    );
                    // A SPREADSHEET, never a PDF: the agent-only table answers
                    // `NativePdf` for Claude Code and that is a claim about
                    // PDFs alone, so asking it here booked every `.xlsx` at
                    // zero.
                    let bin_model = tokenade_core::coefficients::binary_model_for(
                        tokenade_core::detect::from_env().as_str(),
                        false,
                    );
                    let sav = tokenade_core::coefficients::binary_credit_savings(bin_model, tokens);
                    gain_log::record("hook", "read_sheet_skeleton", file_path, sav);
                    doc_dedup_register(parsed, cwd, file_path, &doc_hash);
                    return Some(0);
                }
            }
            let reason = format!(
                "[tokenade:read-binary-extracted] {file_path} → \
                 {bytes_len} bytes of {label}; tokenade \
                 extracted {tokens} tokens of text (heuristic — \
                 re-Read the path directly if extraction looks wrong).\n\n\
                 --- BEGIN EXTRACTED TEXT ({label}, {tokens} tokens) ---\n\
                 {text}\n\
                 --- END EXTRACTED TEXT ---",
                bytes_len = bytes.len(),
            );
            // Above 5k tokens, stash + reference path
            // so the agent can paginate via Read
            // offset/limit on the .txt file.
            //
            // #953 — append `.txt` to the hint path so
            // disclose stashes as `<hash>.txt` (the
            // extracted body IS text, not the PDF).
            // Without this, compose_extension picked
            // up `.pdf` from the original path and the
            // stash file ended in `.pdf` while
            // containing plain text — confused both the
            // `[disclose:summary kind=pdf]` header AND
            // any downstream tool detecting by ext.
            let stash_hint_path = {
                let mut p = path.to_path_buf().into_os_string();
                p.push(".txt");
                std::path::PathBuf::from(p)
            };
            let reason = tokenade_core::disclose::maybe_stash_with_threshold(
                reason,
                tokenade_core::disclose::SourceHint::File(stash_hint_path),
                tokenade_core::disclose::read_threshold_tokens(),
            );
            emit_read_compaction(
                parsed, file_path, 0,
                0, /* extracted TEXT has no relation to the binary's offset/limit */
                reason, false,
            );
            // Per-agent BINARY credit — never the fictional `bytes/4`.
            //
            // The comment here used to say "Claude+PDF already passed through
            // above (NativePdf → credit 0 for anything that still reaches
            // here)", state the right thing and then ask the AGENT-ONLY table,
            // which answers `NativePdf` whatever the format. So every `.xlsx`,
            // `.docx`, `.epub` and `.odt` Claude Code read through tokenade was
            // booked at zero — the one agent for which the extraction is
            // exactly what made the file readable.
            let bin_model = tokenade_core::coefficients::binary_model_for(
                tokenade_core::detect::from_env().as_str(),
                matches!(fmt, tokenade_core::docs::Format::Pdf),
            );
            let sav = tokenade_core::coefficients::binary_credit_savings(bin_model, tokens);
            gain_log::record("hook", "read_binary_extracted", file_path, sav);
            doc_dedup_register(parsed, cwd, file_path, &doc_hash);
            tokenade_core::debug_log::log(
                "hookwrap-pretool",
                "read_binary_extracted",
                serde_json::json!({
                    "file": file_path,
                    "bytes": bytes.len(),
                    "format": label,
                    "tokens": tokens,
                    "elapsed_ms": started.elapsed().as_millis() as u64,
                }),
            );
            return Some(0);
        }
    }
    None
}

/// A file whose bytes are a DOCUMENT (pdf/docx/xlsx/…): extract its text and
/// answer with that, or step aside.
///
/// `Some(rc)` = answered here. `None` = this format was not served (native-PDF
/// passthrough, extraction failure, …) and the caller should keep looking.
#[allow(clippy::too_many_arguments)]
pub(crate) fn serve_extracted_document(
    parsed: &serde_json::Value,
    file_path: &str,
    path: &std::path::Path,
    cwd: &std::path::Path,
    fmt: tokenade_core::docs::Format,
    size: u64,
    started: std::time::Instant,
) -> Option<i32> {
    // PDFs, for agents that read them NATIVELY (e.g. Claude Code
    // 2.x: visual render + built-in pagination via the `pages`
    // param), must pass through untouched. Intercepting them there
    // is net-harmful: our extracted text is injected as
    // additionalContext ON TOP of the native read, AND the staged
    // updatedInput rewrite (`{file_path, limit}`) STRIPS the
    // agent's `pages` param — so `Read(pdf, pages:"1-12")` loses
    // its pagination, the agent re-emits "12 pages, too many to
    // read at once", and loops. Measured (doc-pdf bench, same
    // content as doc-digest): native control reads in 4 turns/
    // $0.159; intercepting cost 13 turns/$0.210 (+32%, +225% turns,
    // 1 claude_error).
    //
    // But agents that do NOT render PDFs natively (Codex, Gemini,
    // Cline, …) DO need our extraction — without it they just get
    // an unreadable-binary error. So gate the passthrough on the
    // calling agent (detect::reads_pdf_natively); everyone else
    // falls through to extraction below. Same for docx/xlsx/pptx,
    // which no agent renders natively → always extracted.
    if matches!(fmt, tokenade_core::docs::Format::Pdf)
        && tokenade_core::detect::from_env().reads_pdf_natively()
    {
        if let Some(rc) = serve_native_pdf(parsed, file_path, path, cwd, fmt, size, started) {
            return Some(rc);
        }
    }
    // Pagination does NOT gate extraction. The old guard skipped a
    // paginated Read on the theory that "the agent likely already
    // pre-read the file and wants a specific slice" — which is
    // false for a BINARY document: no agent can ever have usefully
    // read a .pdf/.docx, so honouring `offset`/`limit` just hands
    // it raw bytes. Measured on the hooked non-Claude agents
    // (opencode/kilocode/copilot/cline/hermes/gemini): a plain Read
    // of a PDF was served as text, but the SAME Read with
    // `limit:100` or `offset:1` fell through to the binary. It is
    // the same trap the Claude PDF redirect already had to remove
    // (Claude Code paginates PDFs by default); this branch kept it.
    // Extract regardless, and let the delivery path bound the text.
    serve_document_text(parsed, file_path, path, cwd, fmt, size, started)
}

/// A file whose bytes are AUDIO or VIDEO: answer with what it IS and how to ask
/// what is SAID in it (~100 tokens), never with its bytes.
#[allow(clippy::too_many_arguments)]
pub(crate) fn serve_media_summary(
    parsed: &serde_json::Value,
    file_path: &str,
    path: &std::path::Path,
    cwd: &std::path::Path,
    kind: tokenade_core::media::MediaKind,
    size: u64,
    started: std::time::Instant,
) -> Option<i32> {
    // Bounded identity — never slurp a 4 GB video inside a hook.
    let media_hash = tokenade_core::media::cheap_media_identity(path);
    let summary = media_summary_for_hook(path, kind, file_path);
    if let Some(h) = media_hash.as_deref() {
        if doc_dedup_hit(
            parsed,
            cwd,
            file_path,
            h,
            tokenade_core::tokenizer::estimate(&summary),
            size,
            started,
        ) {
            return Some(0);
        }
    }
    // DENY, not the head-slice/additionalContext flow: that flow
    // shrinks the Read to a head slice of the ORIGINAL file, which
    // on media means 20 lines of raw binary delivered to the model.
    // The image path denies for the same reason — the bytes cannot
    // be text, so substituting the reason IS the answer.
    let decision = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": summary,
        }
    });
    println!("{decision}");
    // Shared bounded credit — see `media::plausible_media_credit`
    // for why file size must never be the basis.
    let (before, delivered) = tokenade_core::media::plausible_media_credit(
        size,
        tokenade_core::tokenizer::estimate(&summary),
    );
    gain_log::record(
        "hook",
        "read_media_summary",
        file_path,
        tokenade_core::Savings::new(before, delivered),
    );
    if let Some(h) = media_hash.as_deref() {
        doc_dedup_register(parsed, cwd, file_path, h);
    }
    tokenade_core::debug_log::log(
        "hookwrap-pretool",
        "read_media_summary",
        serde_json::json!({
            "file": file_path,
            "size": size,
            "kind": kind.as_str(),
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
    Some(0)
}

/// The file is not UTF-8 text: extract it, summarise it, or step aside.
///
/// Split out of `read_file_sliced_and_redact`, where it was a 442-line `Err(_)`
/// match arm — half of that function, and about a different subject: the text
/// path slices and redacts, this one decides what a non-text file even IS.
///
/// Never yields content to the caller: every path here answers the agent
/// directly (extracted document, media summary, or passthrough) and returns
/// `Err(0)` so the hook stops.
#[allow(clippy::too_many_arguments)]
pub(crate) fn serve_non_text_file(
    parsed: &serde_json::Value,
    file_path: &str,
    path: &std::path::Path,
    cwd: &std::path::Path,
    size: u64,
    started: std::time::Instant,
) -> Result<String, i32> {
    // #951 — read_to_string failed = file is not UTF-8 text.
    // Most common reason: binary file (PDF, DOCX, image, …).
    // If the extension maps to docs::Format we can extract,
    // serve the extracted text via the same pattern as the
    // webfetch hook (#949). Otherwise return 0 (passthrough)
    // so Claude Code's own error surfaces — but ALSO log it so
    // we can see how often this happens in real sessions.
    // Detect through the shared entry point, so a file whose name lies
    // (or has no extension) is still identified from its bytes — the way
    // every other channel does it. `detect_format_at` checks the name
    // first and, failing that, reads only a 1 KiB header and seeks the
    // zip directory: identifying a file must never mean reading it.
    if let Some(fmt) = tokenade_core::docs::detect_format_at(path) {
        if let Some(rc) = serve_extracted_document(parsed, file_path, path, cwd, fmt, size, started)
        {
            return Err(rc);
        }
    }
    // Audio/video: the agent gets an unreadable-binary error otherwise.
    // Serve what it actually needs — what the file IS (ffprobe) and how
    // to ask what is SAID in it — for ~100 tokens. Same dedup as the
    // document paths: re-reading an unchanged media file re-probes for
    // nothing.
    if let Some(kind) = tokenade_core::media::detect_media_at(path) {
        if let Some(rc) = serve_media_summary(parsed, file_path, path, cwd, kind, size, started) {
            return Err(rc);
        }
    }
    tokenade_core::debug_log::log(
        "hookwrap-pretool",
        "read_binary_passthrough",
        serde_json::json!({
            "file": file_path,
            "size": size,
            "had_known_doc_ext": tokenade_core::docs::Format::from_filename(file_path).is_some(),
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
    Err(0)
}
