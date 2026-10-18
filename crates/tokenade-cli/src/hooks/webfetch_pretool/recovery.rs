//! PostToolUse safety net: detect a native WebFetch that came back empty or
//! failed, and rebuild an answer from what we can still extract.

use super::*;

/// PostToolUse handler for `WebFetch` (`hookwrap-postwebfetch`). Fires
/// ONLY when the NATIVE WebFetch ran (an intercept is a PreToolUse deny,
/// so no PostToolUse event exists for it) and its result looks like a
/// fetch failure (network error / 403 / bot-block / near-empty). Appends
/// a one-line `additionalContext` hint pointing at `tokenade web`, which
/// retries with a browser UA profile and often succeeds where the native
/// fetcher was blocked. No automatic re-fetch — the agent decides.
pub(crate) fn cmd_hookwrap_postwebfetch() -> i32 {
    let started = std::time::Instant::now();
    if hook_disabled_by_env() {
        return 0;
    }
    let mut input = String::new();
    if crate::bounded_input::read_stdin_lossy(&mut input).is_err() {
        return 0;
    }
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&input) else {
        return 0;
    };
    if parsed.get("tool_name").and_then(|s| s.as_str()) != Some("WebFetch") {
        return 0;
    }
    let url = parsed
        .pointer("/tool_input/url")
        .and_then(|u| u.as_str())
        .unwrap_or("");
    if url.is_empty() {
        return 0;
    }
    let resp = match parsed.get("tool_response") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
        None => String::new(),
    };
    if !webfetch_native_result_looks_failed(&resp) {
        return 0;
    }
    let hint = format!(
        "WebFetch on {url} appears to have failed. `tokenade web '{url}'` often succeeds \
         where the native fetcher is blocked (browser-profile fetch + markdown distillation)."
    );
    let decision = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "additionalContext": hint,
        }
    });
    crate::log_emitted_additional_context("webfetch-recovery", "PostToolUse", &hint);
    println!("{decision}");
    tokenade_core::debug_log::log(
        "hookwrap-postwebfetch",
        "native_failure_hint",
        serde_json::json!({
            "url": url,
            "response_bytes": resp.len(),
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
    0
}

/// Heuristic: does a native WebFetch tool_response read as a FAILED
/// fetch? Conservative — a false positive injects a misleading hint on
/// a successful fetch, so we only fire on short responses that contain
/// an explicit failure marker, or on (near-)empty responses.
pub(crate) fn webfetch_native_result_looks_failed(resp: &str) -> bool {
    let t = resp.trim();
    if t.is_empty() {
        return true;
    }
    if t.len() > 600 {
        // Real page summaries are longer than any error message; never
        // flag them even if they MENTION an error.
        return false;
    }
    let l = t.to_lowercase();
    const MARKERS: &[&str] = &[
        "unable to fetch",
        "failed to fetch",
        "error fetching",
        "fetch failed",
        "request failed",
        "access denied",
        "forbidden",
        "403",
        "429",
        "too many requests",
        "timed out",
        "captcha",
        "enable javascript",
        "javascript is required",
        "bot detection",
        "blocked",
        "no content",
        "empty response",
    ];
    MARKERS.iter().any(|m| l.contains(m))
}

/// #949 — result of attempting to extract text from binary webfetch
/// bytes. `format_label` names the route taken ("pdf"/"docx"/…) for
/// telemetry + the deny-message banner. `tokens` is the token estimate
/// of the extracted text. `inlined=true` means the text is small enough
/// to inline in the deny reason; `inlined=false` means it was stashed
/// and only `stash_path` is referenced.
pub(crate) struct WebfetchExtraction {
    pub(crate) text: String,
    pub(crate) format_label: &'static str,
    pub(crate) tokens: usize,
    pub(crate) inlined: bool,
    pub(crate) stash_path: Option<String>,
}

/// #949 — V1 inline extraction. Tries to recover text from the binary
/// `body_bytes` using the kind heuristic first (infer crate detected),
/// then Content-Type as fallback. Returns Some(extraction) on success,
/// None on any failure / unsupported format / oversized input so the
/// caller falls through to the legacy generic-hint message.
///
/// Hard caps:
///   - `MAX_EXTRACT_BYTES = 20 MB` — skip extraction on huge payloads
///     (a 100 MB PDF would block the hook for many seconds, and the
///     agent can still recover the raw bytes via the stash path)
///   - `INLINE_TOKENS = 5_000` — extracted text above this stashes
///     instead of inlining, keeping deny-reason size sane
pub(crate) fn try_extract_webfetch(
    body_bytes: &[u8],
    kind: Option<&str>,
    content_type: Option<&str>,
) -> Option<WebfetchExtraction> {
    const MAX_EXTRACT_BYTES: usize = 20 * 1024 * 1024;
    // #955 v2 — inline cap raised 5k → shared 8k default (env
    // TOKENADE_WEBFETCH_INLINE_TOKENS): the forensic run showed a paper
    // PDF's extracted text landing behind a stash pointer at >5k, costing
    // the answer a second turn. Aligned with the legacy HTML route A cap.
    let inline_tokens = webfetch_inline_cap();
    if body_bytes.len() > MAX_EXTRACT_BYTES {
        return None;
    }
    // Prefer kind (more specific — infer reads magic bytes). Fall back
    // to Content-Type only when kind is unknown / generic.
    let fmt_from_kind = kind.and_then(tokenade_core::docs::Format::from_extension);
    let fmt_from_ct = content_type.and_then(|ct| {
        let ct_main = ct.split(';').next().unwrap_or("").trim();
        match ct_main {
            "application/pdf" => Some(tokenade_core::docs::Format::Pdf),
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
                Some(tokenade_core::docs::Format::Docx)
            }
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => {
                Some(tokenade_core::docs::Format::Xlsx)
            }
            "application/vnd.openxmlformats-officedocument.presentationml.presentation" => {
                Some(tokenade_core::docs::Format::Pptx)
            }
            "application/vnd.oasis.opendocument.text" => Some(tokenade_core::docs::Format::Odt),
            "application/vnd.oasis.opendocument.spreadsheet" => {
                Some(tokenade_core::docs::Format::Ods)
            }
            "application/vnd.oasis.opendocument.presentation" => {
                Some(tokenade_core::docs::Format::Odp)
            }
            "application/epub+zip" => Some(tokenade_core::docs::Format::Epub),
            _ => None,
        }
    });
    // Last resort: the BYTES. A URL with no extension served by a host that
    // sends `application/octet-stream` still hands us a real document, and
    // without this the channel simply misses it — the divergence that let the
    // zip-order fix reach `tokenade web` only. See `docs::detect_format`.
    let fmt = fmt_from_kind
        .or(fmt_from_ct)
        .or_else(|| tokenade_core::docs::detect_format(None, body_bytes))?;
    let format_label = docs_format_label(fmt);
    // docs::extract wraps pdf-extract in catch_unwind and remaps
    // typographic ligatures (#955). Returns empty string on parse failure
    // (no panic). pdf-extract 0.7 does not pollute stdout on standard
    // fixtures, so the hook's JSON output is safe; pathological encoding
    // warnings surface on stderr only.
    let text = tokenade_core::media::extract_document(body_bytes, fmt);
    // Redact at the SOURCE, exactly as `compactors::cmd_auto` does for the
    // LOCAL document path, and for the same reason it gives there: a document
    // can hold secrets — an `.env` pasted into a wiki export, an AWS key in a
    // runbook PDF — and a URL is a WORSE case than a local file, not a better
    // one, because nobody chose to open it.
    //
    // Here rather than at each consumer, because there are three and one of
    // them cannot redact for itself: `prompt_scoped_extract_dispatch` returns
    // the raw matching lines. `--prompt "aws credentials"` on such a document
    // was the exact leak the local path closed on 2026-08; this channel had
    // the same shape and no redaction anywhere along it. The other two are the
    // inline reason and the stashed `.txt`, so the stash on disk is redacted
    // too — same as the local path.
    //
    // `redact_for_model` is idempotent, so this costs nothing where a caller
    // already redacts.
    let text = tokenade_core::redact::redact_for_model(&text);
    if text.trim().is_empty() {
        return None;
    }
    let tokens = tokenade_core::tokenizer::estimate(&text);
    if tokens <= inline_tokens {
        Some(WebfetchExtraction {
            text,
            format_label,
            tokens,
            inlined: true,
            stash_path: None,
        })
    } else {
        // Too big to inline — stash the extracted TEXT separately
        // (different from the raw binary stash) so the agent can read
        // it with offset/limit. Use SourceHint::web so the disclose
        // path uses the right extension.
        let hint = tokenade_core::disclose::SourceHint::web();
        // maybe_stash_bytes returns a full disclose summary including
        // a `path=…` line. We don't actually need the summary here —
        // we just need the path so we can mention it. Call it and
        // extract the path from the returned string. Brittle but
        // avoids introducing a new public API; if maybe_stash_bytes
        // changes format the test below catches it.
        let stash_summary =
            tokenade_core::disclose::maybe_stash_bytes(text.as_bytes().to_vec(), hint);
        let stash_path = extract_stash_path(&stash_summary);
        Some(WebfetchExtraction {
            text,
            format_label,
            tokens,
            inlined: false,
            stash_path,
        })
    }
}

/// #949 — pull the absolute path out of a disclose summary's `[stashed:
/// <path> · …]` marker. Best-effort; returns None if the marker isn't
/// present. The extracted path is only used in a hint message, so a
/// None just means we omit that line.
///
/// #954 — was previously anchored on the literal substring `/stash/`,
/// which silently failed when a user customised `TOKENADE_STASH_DIR`
/// (e.g. `/var/cache/tokenade/dump/`). The marker `[stashed: ` is
/// stable regardless of stash-dir configuration, so anchor there
/// instead. The trailing path-character set covers absolute Unix paths;
/// the path ends at the first whitespace, `·`, or `]` separator that
/// disclose puts after it.
pub(crate) fn extract_stash_path(summary: &str) -> Option<String> {
    const MARKER: &str = "[stashed: ";
    let start = summary.find(MARKER)? + MARKER.len();
    let suffix = &summary[start..];
    let end = suffix
        .find(|c: char| c.is_whitespace() || c == '·' || c == ']')
        .unwrap_or(suffix.len());
    let path = suffix[..end].trim();
    if path.starts_with('/') {
        Some(path.to_string())
    } else {
        None
    }
}

/// #949 — render the deny-reason body when extraction succeeded. Two
/// shapes: inlined (text follows banner) vs path-referenced (extracted
/// text stashed separately, only its path is in the banner).
pub(crate) fn build_webfetch_extracted_reason(
    url: &str,
    raw_bytes: usize,
    ext: &WebfetchExtraction,
    raw_stash_block: &str,
) -> String {
    if ext.inlined {
        format!(
            "[tokenade:webfetch-binary-extracted] {url} → {raw_bytes} bytes of {fmt}; \
             tokenade extracted {tokens} tokens of text (heuristic — raw binary preserved \
             below in case extraction was wrong).\n\n\
             --- BEGIN EXTRACTED TEXT ({fmt}, {tokens} tokens) ---\n\
             {text}\n\
             --- END EXTRACTED TEXT ---\n\n\
             {raw_stash_block}",
            fmt = ext.format_label,
            tokens = ext.tokens,
            text = ext.text,
            raw_stash_block = raw_stash_block,
        )
    } else {
        let path_line = match &ext.stash_path {
            Some(p) => format!(
                "Extracted text is large ({tokens} tokens) and stashed at:\n  {p}\n\
                 Use Read with offset/limit to paginate, or grep for specific terms.",
                tokens = ext.tokens,
                p = p,
            ),
            None => format!(
                "Extracted text is large ({tokens} tokens) and was stashed (path not \
                 extractable from summary — see raw stash below for the binary).",
                tokens = ext.tokens,
            ),
        };
        format!(
            "[tokenade:webfetch-binary-extracted] {url} → {raw_bytes} bytes of {fmt}; \
             tokenade extracted {tokens} tokens of text (heuristic — raw binary also \
             preserved below in case extraction was wrong).\n\
             {path_line}\n\n\
             {raw_stash_block}",
            fmt = ext.format_label,
            tokens = ext.tokens,
            raw_stash_block = raw_stash_block,
        )
    }
}

/// #955 v2 — deny-reason body for a binary document whose extracted
/// text is too big to inline but whose prompt found a lexical anchor:
/// serve the scoped excerpt inline (answer in ONE turn) and point at
/// the stashed full text + raw binary for recovery.
pub(crate) fn build_webfetch_scoped_doc_reason(
    url: &str,
    raw_bytes: usize,
    ext: &WebfetchExtraction,
    extract: &str,
    prompt: &str,
    raw_stash_block: &str,
) -> String {
    let path_line = match &ext.stash_path {
        Some(p) => format!(
            "Full extracted text ({tokens} tokens) stashed at:\n  {p}\n\
             Use Read with offset/limit, or grep it for other terms.",
            tokens = ext.tokens,
        ),
        None => format!(
            "Full extracted text ({tokens} tokens) was stashed (path not extractable \
             from summary — see raw stash below for the binary).",
            tokens = ext.tokens,
        ),
    };
    format!(
        "[tokenade:webfetch-binary-extracted] {url} → {raw_bytes} bytes of {fmt}; tokenade \
         extracted {tokens} tokens of text. Below are the passage(s) matching your prompt \
         {prompt:?} — if several values appear they are ALL shown; pick the one that answers \
         the prompt; do NOT re-fetch. Need several things? Put them in `prompt` separated by \
         commas — each is answered under its own `### Q` heading.\n\
         {path_line}\n\n\
         --- BEGIN PROMPT-SCOPED EXCERPT ({fmt}) ---\n\
         {extract}\n\
         --- END PROMPT-SCOPED EXCERPT ---\n\n\
         {raw_stash_block}",
        fmt = ext.format_label,
        tokens = ext.tokens,
    )
}

/// #952 — shared lowercase label for `docs::Format`. Both #949
/// (webfetch) and #951 (read) want the same string in their deny
/// reasons ("pdf" / "docx" / "epub" …) for consistency with the
/// `infer` kind names and the user-typed `tokenade docs path=…`
/// extension. Previously #951 used `{fmt:?}` (Debug → "Pdf" capitalised)
/// while #949 had this map inline.
pub(crate) fn docs_format_label(fmt: tokenade_core::docs::Format) -> &'static str {
    match fmt {
        tokenade_core::docs::Format::Pdf => "pdf",
        tokenade_core::docs::Format::Docx => "docx",
        tokenade_core::docs::Format::Xlsx => "xlsx",
        tokenade_core::docs::Format::Pptx => "pptx",
        tokenade_core::docs::Format::Odt => "odt",
        tokenade_core::docs::Format::Ods => "ods",
        tokenade_core::docs::Format::Odp => "odp",
        tokenade_core::docs::Format::Epub => "epub",
        tokenade_core::docs::Format::Odg => "odg",
        tokenade_core::docs::Format::FlatOdf => "flat-odf",
        tokenade_core::docs::Format::Rtf => "rtf",
        tokenade_core::docs::Format::Fb2 => "fb2",
        tokenade_core::docs::Format::Xls => "xls",
        tokenade_core::docs::Format::Doc => "doc",
        tokenade_core::docs::Format::Ppt => "ppt",
        tokenade_core::docs::Format::Msg => "msg",
    }
}

/// #947 — recovery-hint for the webfetch-binary deny message. Maps the
/// detected kind (or fallback Content-Type) to the exact in-toolkit
/// command the agent should run on the stashed file. Without this, the
/// only guidance was `file <path>, xxd <path>, hexdump -C <path>` — all
/// of which leave the agent with raw bytes still to interpret. Some
/// agents then pivot to WebSearch (losing the source) instead of
/// finding the toolkit path themselves.
pub(crate) fn pick_webfetch_recovery_hint(
    kind: Option<&str>,
    content_type: Option<&str>,
) -> String {
    let k = kind.unwrap_or("");
    let ct = content_type.unwrap_or("");
    if k == "pdf" || ct == "application/pdf" {
        "Extract text: `tokenade web <stash file shown below>` (handles PDF, returns markdown)."
            .to_string()
    } else if k == "html"
        || k == "htm"
        || ct.starts_with("text/html")
        || ct == "application/xhtml+xml"
    {
        "Parse as HTML: `tokenade web <stash file shown below>` \
         (HTML → compact markdown). The bytewise heuristic \
         was triggered by noisy inline content (SVG / base64 / minified JS), \
         but the source is text/html and `tokenade web` handles it."
            .to_string()
    } else if k.starts_with("image/")
        || ct.starts_with("image/")
        || k == "png"
        || k == "jpg"
        || k == "jpeg"
        || k == "gif"
        || k == "webp"
        || k == "svg"
    {
        "Image content — no text extraction available. Inspect bytes with: file <stash file shown below>".to_string()
    } else if k == "zip" || k == "gz" || k == "tar" || ct.contains("zip") || ct.contains("tar") {
        "Archive content — extract with: `tar -tf <stash file shown below>` (list) or `unzip -l <stash file shown below>`.".to_string()
    } else {
        "(Binary content — not shown as text. Inspect with: file <stash file shown below>, xxd <stash file shown below> | head, \
         hexdump -C <stash file shown below> | head)".to_string()
    }
}
