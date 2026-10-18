//! **The gate: stash, or let through.**
//!
//! All the forms of `maybe_stash`, with the installable gate that lets
//! a caller decide otherwise. It is the only place in the module that talks
//! to the ledger, and it is what makes the LAST layer-debt edge of the
//! repo — declared, with its reason, in `layers_only_point_downwards`.

use super::*;

pub fn maybe_stash(text: String, hint: SourceHint) -> String {
    maybe_stash_with_threshold(text, hint, threshold_tokens())
}

/// Threshold-parametrised variant of [`maybe_stash`]. Callers that know
/// the bytes came from a DELIBERATE source (a `Read` of a file the agent
/// chose to open) pass [`read_threshold_tokens`] so the file is served
/// inline up to a higher bar — see that function for the rationale.
/// Incidental callers (Bash/curl/MCP output) keep calling the plain
/// [`maybe_stash`], which forwards [`threshold_tokens`].
pub fn maybe_stash_with_threshold(text: String, hint: SourceHint, threshold: usize) -> String {
    maybe_stash_reporting_with_threshold(text, hint, threshold).0
}

/// [`maybe_stash`], plus the FACT of whether the returned text carries a
/// pointer this call put there.
///
/// A caller that needs to know must be told. Searching the returned text for
/// `[stashed:` or `[tokenade:` reads a claim that the CONTENT can make: a
/// fetched page, a log line, a file quoting this tool's own output all carry
/// those bytes perfectly legitimately, and then a caller deciding "a pointer is
/// already here" is deciding on the content's say-so. Measured 2026-09-06: one
/// page of twelve carrying the literal `[tokenade:` in its body suppressed the
/// whole-batch fold, 1 875 bytes becoming 18 997.
pub fn maybe_stash_reporting(text: String, hint: SourceHint) -> (String, bool) {
    maybe_stash_reporting_with_threshold(text, hint, threshold_tokens())
}

/// Threshold-parametrised [`maybe_stash_reporting`].
pub fn maybe_stash_reporting_with_threshold(
    text: String,
    hint: SourceHint,
    threshold: usize,
) -> (String, bool) {
    stash_reporting_impl(text, hint, threshold, None)
}

/// [`maybe_stash`], with the summary BODY supplied by the caller.
///
/// The stash carries the full text and the retrieval pointer is
/// unchanged: only the delivered preview comes from elsewhere. Nothing changes when the
/// text falls under the threshold — it is then returned whole, and the proposed body
/// is not used.
pub fn maybe_stash_with_body(text: String, hint: SourceHint, body: String) -> String {
    stash_reporting_impl(text, hint, threshold_tokens(), Some(body)).0
}

/// [`maybe_stash_with_body`], ALSO saying whether a pointer was set.
///
/// The web route by BATCH needs both halves: the body to serve, and the
/// fact that a pointer exists — it is what decides whether the delivery can
/// send the agent back to the stash.
pub fn maybe_stash_reporting_with_body(
    text: String,
    hint: SourceHint,
    body: String,
) -> (String, bool) {
    stash_reporting_impl(text, hint, threshold_tokens(), Some(body))
}

pub(super) fn stash_reporting_impl(
    text: String,
    hint: SourceHint,
    threshold: usize,
    body: Option<String>,
) -> (String, bool) {
    // Opportunistic TTL sweep — once per process. `tokenade gc` and
    // `project_scaffold::ensure` already trigger this, but heavy
    // agent sessions that never re-scaffold and never call `gc`
    // would accumulate stash files forever. Each Bash hook is a
    // fresh process, so "once per process" gives natural cadence
    // (one sweep per stash-triggering command) without spamming.
    // 1. Binary short-circuit (L69). Binary content is unreadable as
    //    text regardless of who asked for it, so the threshold never
    //    applies here — always stash, return metadata only.
    if let Some(bin_kind) = detect_binary(text.as_bytes()) {
        return (stash_as_binary(text.as_bytes(), &bin_kind, &hint), true);
    }
    // 2. Text + threshold.
    let tokens = crate::tokenizer::estimate(&text);
    if tokens < threshold {
        // #475 (extended) — SECURITY: redact secrets even on the sub-threshold
        // passthrough. Callers that delegate their redaction to the stash
        // (notably MCP: auto::compact does not redact, then hands small
        // results here) would otherwise leak below the stash threshold.
        // Redact is idempotent, so callers that already scrubbed (cmd_wrap,
        // cmd_auto, filter) pay only a no-op second pass.
        return (crate::redact::redact(&text), false);
    }
    // #475 — SECURITY: redact secrets before writing to disk. Upstream
    // pipelines (cmd_wrap, MCP handlers) sometimes redact the BANNER
    // / LABEL but pass the raw output body through. Without this guard
    // an `env`, `cat .env`, or `kubectl get secrets -o yaml` whose
    // output crosses the stash threshold would land in
    // ~/.cache/tokenade/stash/ in cleartext. Defense-in-depth: the
    // stash layer is the LAST chance to scrub before bytes hit disk.
    // Hash AFTER redaction so equal-redacted inputs dedup correctly.
    let text = crate::redact::redact(&text);
    let ext = compose_extension(&text, &hint);
    let hash = crate::dedup_ref::hash_content(text.as_bytes());
    let path = match ensure_stashed(&text, &hash, &ext) {
        Ok(p) => p,
        // Fail-open, but never SILENTLY. When the write fails the caller gets
        // the text back with no pointer — the agent simply loses the recovery
        // path, and until now nothing anywhere said why. That is the exact
        // failure mode the licence guard eight lines above is at pains to
        // avoid ("a diagnostic so the fail-open is never invisible"), and this
        // path had none. First real cost: a one-off red test whose recovery
        // pointer was missing, with nothing on disk to explain it.
        Err(e) => {
            crate::debug_log::log(
                "disclose",
                "stash_write_failed",
                serde_json::json!({
                    "hash": hash,
                    "bytes": text.len(),
                    "ext": ext,
                    "error": e.to_string(),
                }),
            );
            return (text, false);
        }
    };
    // The stash landed and the caller is about to get a pointer instead of the
    // content: this is the exact moment `tokens` stop being delivered. Recorded
    // here rather than at any call site because this is the one choke point all
    // eight of them go through.
    //
    // It is NOT the only offer, and reading it as one made the ratio
    // impossible. `wrap::nudges::attach_drop_recovery` makes a second kind —
    // a pointer to what the fold DROPPED rather than to what the pointer
    // REPLACED — and it is the larger of the two. Both record `offered`; see
    // the note there.
    crate::gain::record_recovery("offered", tokens);
    (
        build_summary_with_body(&text, tokens, &path, &ext, &hash, &hint, body),
        true,
    )
}

/// Bytes-aware variant of [`maybe_stash`]. Use this when the caller
/// has the raw response bytes (e.g. `Vec<u8>` from a curl handle, a
/// Playwright screenshot, or a Bash command whose stdout might be
/// binary). The binary short-circuit picks up image / audio / video
/// / archive / executable signatures that would be lost through a
/// `String::from_utf8_lossy` round-trip, and the stash gets the
/// EXACT bytes the upstream tool produced.
///
/// Text content is forwarded to the regular [`maybe_stash`] after a
/// lossy UTF-8 decode for the summary path — the lossy view is fine
/// for the threshold check and the rendered summary, and the stash
/// itself uses the raw bytes.
pub fn maybe_stash_bytes(bytes: Vec<u8>, hint: SourceHint) -> String {
    maybe_stash_bytes_reporting(bytes, hint).0
}

/// [`maybe_stash_bytes`], plus whether a pointer was put there — the same FACT
/// [`maybe_stash_reporting`] carries, for the same reason.
pub fn maybe_stash_bytes_reporting(bytes: Vec<u8>, hint: SourceHint) -> (String, bool) {
    if let Some(bin_kind) = detect_binary(&bytes) {
        return (stash_as_binary(&bytes, &bin_kind, &hint), true);
    }
    // Text path: hand off to the standard summary pipeline.
    let text = String::from_utf8_lossy(&bytes).to_string();
    let tokens = crate::tokenizer::estimate(&text);
    if tokens < threshold_tokens() {
        return (text, false);
    }
    // #475 — SECURITY: redact secrets before writing to disk. See the
    // same-numbered note in `maybe_stash` for the full rationale.
    // The earlier `detect_binary` short-circuit already routed any
    // actual binary (image, archive, executable) through
    // `stash_as_binary`, so anything reaching here is text — safe to
    // round-trip through redact's UTF-8 regex pass. We also lose the
    // "exotic encoding" fidelity the old comment mentioned, but that
    // trade-off favours security: a misdecoded byte sequence is rare,
    // a leaked credential is not.
    let text = crate::redact::redact(&text);
    let ext = compose_extension(&text, &hint);
    let redacted_bytes = text.as_bytes().to_vec();
    let hash = crate::dedup_ref::hash_content(&redacted_bytes);
    let path = match ensure_stashed_bytes(&redacted_bytes, &hash, &ext) {
        Ok(p) => p,
        Err(_) => return (text, false),
    };
    (
        build_summary(&text, tokens, &path, &ext, &hash, &hint),
        true,
    )
}

pub(super) fn ensure_stashed_bytes(
    bytes: &[u8],
    hash: &str,
    ext: &str,
) -> std::io::Result<PathBuf> {
    // Sweep where the growth is. This used to sit at the TOP of `maybe_stash`
    // / `maybe_stash_bytes` / `stash_for_reference`, i.e. BEFORE the size
    // threshold that declines to stash at all — so a three-byte `echo hi` paid
    // a full walk of the stash dir. Here it runs only on the paths that
    // actually add a file, which is the only reason the sweep exists, and it
    // covers every writer including `stash_as_binary`.
    opportunistic_evict();
    let dir = stash_dir();
    std::fs::create_dir_all(&dir)?;
    let path = stash_path(hash, ext);
    note_session_ref(hash);
    let content: std::borrow::Cow<'_, [u8]> = if bytes.len() <= STASH_MAX_BYTES {
        bytes.into()
    } else {
        let half_cap = STASH_MAX_BYTES / 2;
        let mut combined = Vec::with_capacity(STASH_MAX_BYTES + 256);
        combined.extend_from_slice(&bytes[..half_cap]);
        let marker = format!(
            "\n[tokenade:stash truncated — original was {} bytes, kept first {} + last {}]\n",
            bytes.len(),
            half_cap,
            half_cap,
        );
        combined.extend_from_slice(marker.as_bytes());
        let tail_start = bytes.len().saturating_sub(half_cap);
        combined.extend_from_slice(&bytes[tail_start..]);
        combined.into()
    };
    super::store::keep_blob(&path, &content)?;
    Ok(path)
}
