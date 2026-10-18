//! **Let it through, and SAY so.**
//!
//! When no view wins, the file goes out whole — but not silently. The agent
//! must know it has just paid full price, and which path gets it back cheaper
//! next time.
//!
//! A passthrough without disclosure is indistinguishable from a successful
//! fold: that is exactly what keeps a degraded path degraded for months.

use super::*;

/// Terminal passthrough phase of `handle_read_pretool`: no upstream pass
/// substituted. Register the bytes under the metadata + content (+ trimmed)
/// hashes and the path pointer, apply the universal L64 disclose fallback for
/// >5k-token raw content (unless value-folding or editable source), snapshot the
/// > delivered line range for range-coverage dedup, log the miss, and return the
/// > final code (0 = pass through, or 0 from a disclose emit).
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_passthrough_with_disclosure(
    parsed: &serde_json::Value,
    file_path: &str,
    path: &std::path::Path,
    cwd: &std::path::Path,
    hash: &str,
    content_hash: &str,
    trimmed_hash: &str,
    trimmed_current: &str,
    path_key: &str,
    sliced_current: &str,
    session_id: &str,
    range_version: &str,
    range_key: &str,
    offset: u64,
    limit: u64,
    size: u64,
    editable_source_guard: bool,
    read_kind_folds_values: bool,
    started: std::time::Instant,
) -> i32 {
    // Path MISS or diff-not-worth-it — register the content under
    // BOTH the metadata-identity hash (fast HIT path next time) AND
    // the content hash (cross-tool dedup, so a later Bash producing
    // these exact bytes hits §ref§ instead of re-shipping). Plus
    // the TRIMMED content hash — `tokenade wrap` strips trailing
    // newlines from compactor output, so a future Bash `cat file`
    // hashes the no-trailing-newline form. Registering both ensures
    // the reverse direction (Read → Bash) cross-hits symmetrically
    // with the forward direction (Bash → Read) that's already on
    // the lookup side. Path pointer points at the content hash so
    // L13 diff baselining works the same way.
    // Audit V6 (class 2): Claude Code refuses Reads over ~25k tokens AFTER
    // this hook — registering identity/content baselines for a delivery
    // that will be refused feeds the class-1 "already in your earlier
    // turn" false claim. Same predictive gate as register_read_range.
    let delivery_plausible = tokenade_core::tokenizer::estimate(sliced_current) <= 24_000;
    if delivery_plausible {
        register_and_tag(parsed, cwd, hash, sliced_current);
        register_and_tag(parsed, cwd, content_hash, sliced_current);
        if trimmed_hash != content_hash {
            register_and_tag(parsed, cwd, trimmed_hash, trimmed_current);
        }
    }
    if delivery_plausible {
        tokenade_core::dedup_ref::register_path_pointer(cwd, path_key, content_hash);
    }
    // #722 — canonical (file, 0, 0) alias was originally added here in
    // #718, but the 4 OTHER register sites in this function (code-strip,
    // auto-compact, cross-tool dedup, etc.) exited early without ever
    // reaching this terminal path. Moved to right after full_content
    // is captured (above) so it fires before any early-exit. Single
    // registration site, universal coverage.
    // L64 — universal disclosure fallback. Even if no upstream pass
    // recognised the format or won enough to substitute, the raw
    // bytes Claude Code is about to read may still be > 5k tokens.
    // Above the threshold, swap in a stash + summary so we never
    // dump > 5k of unmodelled prose / config / data into context.
    let raw_tokens = tokenade_core::tokenizer::estimate(sliced_current);
    // Value-folding kinds (the log family) PASS THROUGH on the Read path even
    // when large: the raw file is cheap `cache_read` and a competent agent
    // greps it directly (control does exactly this and wins). Stashing a marker
    // here would re-introduce the head-slice loss — a pointer in place of
    // grep-able content. They were never reaching disclose before (the
    // now-gated auto-compact block pre-empted them); excluding them keeps the
    // passthrough true. See `kind_lossless_read_view` for the bench evidence
    // (log-needle-zh +60% when its 54k log was substituted).
    if raw_tokens >= tokenade_core::disclose::read_threshold_tokens()
        && !editable_source_guard
        && !read_kind_folds_values
    {
        let summary = tokenade_core::disclose::maybe_stash_with_threshold(
            sliced_current.to_string(),
            tokenade_core::disclose::SourceHint::File(std::path::PathBuf::from(file_path)),
            tokenade_core::disclose::read_threshold_tokens(),
        );
        // `maybe_stash` returns the original bytes unchanged when the
        // tokenizer estimate was below threshold OR the stash failed.
        // We re-check by comparing length: a stashed return is the
        // summary marker, far shorter than the raw input.
        if summary.len() < sliced_current.len() {
            let banner = format!(
                "[tokenade:read-disclose] {file_path} ({raw_tokens} tokens above {threshold} read threshold)\n\n",
                threshold = tokenade_core::disclose::read_threshold_tokens()
            );
            let reason = format!("{banner}{summary}");
            let reason_tokens = tokenade_core::tokenizer::estimate(&reason);
            let net_sav = emit_read_compaction(parsed, file_path, offset, limit, reason, false);
            if let Some(net_sav) = net_sav {
                gain_log::record("hook", "read_disclose", file_path, net_sav);
            }
            tokenade_core::debug_log::log(
                "hookwrap-pretool",
                "read_disclose_emit",
                serde_json::json!({
                    "file": file_path,
                    "size": size,
                    "raw_tokens": raw_tokens,
                    "inject_tokens": reason_tokens,
                    "elapsed_ms": started.elapsed().as_millis() as u64,
                }),
            );
            return 0;
        }
    }
    // Register the line range Claude Code is about to deliver VERBATIM for
    // range-coverage dedup (a later contained slice this session → §ref).
    // Snapshot the delivered bytes under their own hash so the §ref stays
    // recoverable via expand-ref. Reaching here means no substitution fired,
    // so the agent gets the real [offset, limit) slice. Gated + session-scoped.
    if !range_dedup_disabled() && !session_id.is_empty() {
        // `offset` is Claude Code's 1-based line number; the ranges are 0-based.
        let (rstart, rend) = delivered_line_range(path, offset.saturating_sub(1), limit);
        if rend > rstart {
            if let Ok(full) = std::fs::read_to_string(path) {
                let slice = slice_lines(&full, rstart, rend);
                // U2 fleet catch: Claude Code REFUSES Reads over ~25k tokens
                // AFTER our pretool ran — registering "about to deliver" then
                // claimed a never-delivered range as covered. Deterministic
                // failure is predictable: skip registration near the cap.
                // (Full fix = move registration to PostToolUse on success.)
                if !slice.is_empty() && tokenade_core::tokenizer::estimate(&slice) <= 24_000 {
                    let h = tokenade_core::dedup_ref::hash_content(slice.as_bytes());
                    register_and_tag(parsed, cwd, &h, &slice);
                    crate::wrap::register_read_slice(parsed, path, rstart, rend, &h);
                    tokenade_core::dedup_ref::register_read_range(
                        session_id,
                        range_key,
                        range_version,
                        rstart,
                        rend,
                        &h,
                    );
                }
            }
        }
    }
    tokenade_core::debug_log::log(
        "hookwrap-pretool",
        "read_dedup_miss_registered",
        // #715b — include offset/limit so we can correlate against
        // `compose_edit_diff_hint.skipped_no_snapshot` events: if the
        // hint always looks up `(file, 0, 0)` but Reads register under
        // a different key, the mismatch shows up here directly.
        serde_json::json!({
            "file": file_path,
            "size": size,
            "offset": offset,
            "limit": limit,
            "metadata_hash": hash,
            "content_hash": content_hash,
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
    0 // pass through — Claude Code reads normally
}
