#![allow(unused_imports)]
//! Split out of the monolithic `hooks.rs` (mechanical module split, no
//! behaviour change). Wired back into the `hooks` namespace by mod.rs via
//! `pub(crate) use <name>::*;`.
mod dedup;
mod documents;
mod images;
mod pdf;
use super::*;
use crate::*;
pub(crate) use dedup::*;
pub(crate) use documents::*;
pub(crate) use images::*;
pub(crate) use pdf::*;

/// Extract lines `[start, end)` (0-based, half-open) from `content`,
/// re-joined with `\n`. Clamps to the available lines. Used to (a) size
/// the anti-inflation guard and (b) snapshot the bytes a verbatim Read
/// delivered, so a later §ref to this range is recoverable via expand-ref.
pub(crate) fn slice_lines(content: &str, start: u64, end: u64) -> String {
    let start = start as usize;
    let end = end as usize;
    content
        .lines()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The line range a verbatim Read actually delivered: `[offset, offset+limit)`
/// for an explicit limit, else `[offset, offset+min(cap, remaining))`. Reads
/// the file to count lines only on the `limit==0` path.
pub(crate) fn delivered_line_range(path: &std::path::Path, offset: u64, limit: u64) -> (u64, u64) {
    if limit > 0 {
        return (offset, offset.saturating_add(limit));
    }
    // A REGULAR file, or no range at all. This function counts lines by
    // reading the whole path, and `metadata.len()` is 0 for a character
    // device — so the 50 MB ceiling further down (which exists for exactly
    // this OOM, and says so) never engages on one. MEASURED: a PreToolUse
    // `Read` of `/dev/zero` was SIGKILLed after 28 s of unbounded allocation,
    // and `/dev/urandom` never returned at all. A hook that does not return
    // stalls the tool call it was supposed to speed up.
    //
    // Returning an EMPTY range is what closes it: the caller registers
    // range-coverage only when `rend > rstart`, so its own unbounded
    // `read_to_string` never runs either. One guard, both reads.
    if !std::fs::metadata(path)
        .map(|m| m.is_file())
        .unwrap_or(false)
    {
        return (offset, offset);
    }
    let total = std::fs::read_to_string(path)
        .map(|s| s.lines().count() as u64)
        .unwrap_or(0);
    let remaining = total.saturating_sub(offset);
    (
        offset,
        offset.saturating_add(remaining.min(CLAUDE_READ_DEFAULT_LINE_CAP)),
    )
}

/// CLASS-1 unified gate: TRUE when "already in your context" claims are
/// unverifiable — a sidechain shares session_id but not context, and a
/// recent compaction voided whatever WAS there (audit V1: the whole
/// read-dedup family trusted a 900s wall-clock proxy while only replay
/// read the PreCompact marker). While true, every substitution phase
/// degrades to a full serve.
///
/// This is the compaction-recency HALF of that gate; `sidechain_window_active`
/// is the other. Still required even same-context — a PreCompact voids what the
/// model saw.
///
/// **The window MUST stay the one the dedup claim uses.** Both read
/// `read_context_recency_secs()`, and that is not a coincidence to be tidied
/// away — it is what makes the gate airtight rather than merely helpful:
///
/// * a claim is only made for a read YOUNGER than W (`range_is_covered` takes
///   the same W);
/// * a compaction in the last W silences every claim.
///
/// So a read that predates a compaction can never be claimed. Either the
/// compaction is recent and the gate fires, or the compaction is older than W —
/// in which case the read that preceded it is older still, and is not claimable
/// on age alone. Give the two paths different windows and the hole opens
/// silently in the gap between them: the model would be told a file is "still in
/// your context" after a compaction dropped it, and would be reasoning about
/// content it cannot see.
///
/// `token-optimizer` shipped exactly that bug and fixed it by CLEARING the
/// session's read rows at compaction (`fix(#101): clear live-session read cache
/// after compaction`). Their `--clear` hit an `all` branch that never touched
/// the live session. tokenade needs no clear because of the shared window —
/// which is only true while it stays shared.
pub(crate) fn compaction_recent(session_id: &str) -> bool {
    if session_id.is_empty() {
        return false;
    }
    let skey: String = session_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(40)
        .collect();
    let recency = read_context_recency_secs();
    if recency == 0 {
        return false;
    }
    let Ok(md) = std::fs::metadata(super::replay::compact_ts_path(&skey)) else {
        return false;
    };
    md.modified()
        .ok()
        .and_then(|m| std::time::SystemTime::now().duration_since(m).ok())
        .map(|age| age.as_secs() < recency)
        .unwrap_or(false)
}

/// Consecutive dedup claims tolerated before the breaker full-serves.
pub(crate) const BREAKER_TRIP: u64 = 2;

/// TRUE while a subagent was spawned recently in this session: sidechains
/// share session_id but not context, so "already in your context" claims
/// are unverifiable — substitution paths must degrade to full serves.
pub(crate) fn sidechain_window_active(session_id: &str) -> bool {
    if session_id.is_empty() {
        return false;
    }
    let skey: String = session_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(40)
        .collect();
    let p = super::read_staging::read_pending_dir().join(format!("sidechain-{skey}"));
    let (count, ts) = super::read_sidechain_state(&p);
    if count == 0 {
        return false;
    }
    // Exact window: SubagentStop decrements to zero the moment the last
    // subagent finishes. The 15-min TTL remains ONLY as a crash-safety net
    // (a killed subagent that never emitted SubagentStop must not suspend
    // claims forever).
    let now = tokenade_core::platform::now_unix_secs();
    now.saturating_sub(ts) < 900
}

/// Read the file (incl. PDF/DOCX extraction), slice by offset/limit, register
/// the canonical (file,0,0) full-file alias, and redact secrets — the producer
/// phase of `handle_read_pretool`. Returns `Ok(sliced_current)` (already
/// redacted) for the downstream dedup/diff/compact phases, or `Err(rc)` for
/// every early `return rc` (binary passthrough, native-PDF passthrough, binary
/// extraction emit). `path_key` is computed by the caller (cheap, deterministic)
/// and threaded onward; the (file,0,0) canonical alias is registered here.
pub(crate) fn read_file_sliced_and_redact(
    parsed: &serde_json::Value,
    file_path: &str,
    path: &std::path::Path,
    cwd: &std::path::Path,
    offset: u64,
    limit: u64,
    size: u64,
    started: std::time::Instant,
) -> Result<String, i32> {
    let current_content = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return serve_non_text_file(parsed, file_path, path, cwd, size, started),
    };
    // #718 — when slicing, capture the full content into `full_content`
    // so we can ALSO register it under (file, 0, 0) downstream (see the
    // registration block). Move semantics — no clone: `sliced` is a NEW
    // String built from line-iteration, so `current_content` is still
    // owned after the slice + we hand it off to `Some(...)`. In the
    // no-slice branch we move current_content into sliced_current as
    // before; `full_content` is None there because the slice IS the
    // full content and the existing path_key already equals (0, 0).
    let (sliced_current, full_content): (String, Option<String>) = if offset > 0 || limit > 0 {
        let sliced = current_content
            .lines()
            // Claude Code's Read `offset` is 1-BASED (offset=1 = first
            // line): `skip(offset)` shifted the slice by one line vs the
            // native tool result.
            .skip((offset as usize).saturating_sub(1))
            .take(if limit > 0 {
                limit as usize
            } else {
                usize::MAX
            })
            .collect::<Vec<_>>()
            .join("\n");
        (sliced, Some(current_content))
    } else {
        (current_content, None)
    };

    // #722 — register the canonical (file, 0, 0) alias EARLY so it fires
    // before any of the 4 OTHER register sites below (code-strip,
    // cross-tool dedup, auto-compact, terminal). #718 originally added
    // this only at the terminal path — paginated source-code Reads that
    // hit code-strip exited early and never got the alias. Moving here
    // means EVERY Read (paginated or whole) leaves a (file, 0, 0)
    // baseline pointing at the full file content. full_redacted goes
    // through the same redact pass as sliced_current does below, so
    // we never store cleartext secrets.
    if let Some(full) = full_content.as_ref() {
        let full_redacted = tokenade_core::redact::redact(full);
        let full_hash = tokenade_core::dedup_ref::hash_content(full_redacted.as_bytes());
        tokenade_core::dedup_ref::register(cwd, &full_hash, &full_redacted);
        ctx_tag(parsed, &full_hash);
        let canonical_key = tokenade_core::dedup_ref::path_key(file_path, 0, 0);
        tokenade_core::dedup_ref::register_path_pointer(cwd, &canonical_key, &full_hash);
    }

    // #478 — SECURITY: redact secrets ONCE here so every downstream
    // dedup_ref::register call (there are 8+ in this function) stores
    // already-scrubbed content in ~/.tokenade/dedup.db. Without this,
    // reading `.env`, `~/.aws/credentials`, a kubeconfig, etc.
    // persists the secret to disk AND re-exposes it to the agent on
    // future reads via the `expand_ref` MCP tool. The content hashes
    // computed below also become hash(redacted), keeping the
    // cross-tool cache consistent with cmd_wrap's #477 fix.
    let sliced_current = tokenade_core::redact::redact(&sliced_current);
    Ok(sliced_current)
}

// L21 cross-tool content dedup (extracted phase of `handle_read_pretool`).
// Checks whether this exact byte sequence already lives in the project's dedup
// cache under its CONTENT hash (a prior Bash `tokenade wrap`, or a prior Read of
// identical bytes) and substitutes a `§ref` marker when so. `content_hash`,
// `trimmed_hash` and `trimmed_current` are computed once by the caller and
// threaded in. Returns `Some(0)` on substitute / inflation-skip; `None`
// otherwise (caller continues, having registered nothing extra).
// The `#[allow]` attribute that lived here left with its function: it no
// longer covered anything, and the rule for an `allow` is to name the item it
// exempts.

/// Bench catch (cascade-py rep3): the agent `cat -n`'d 7 source files, then
/// Claude Code's Read-before-Edit gate forced a full Read of each — the same
/// content billed twice, and the EXISTING cross-tool dedup missed it because
/// `cat -n` (line numbers) and multi-file concatenation break byte identity
/// with the raw output. Register the ON-DISK bytes of every file a `cat`
/// serves; the later Read's content hash then matches, and a file modified
/// in between naturally stops matching (we hash disk state at cat time).
pub(crate) fn register_bash_served_files(
    parsed: &serde_json::Value,
    cwd: &std::path::Path,
    inner_cmd: &str,
) {
    for segment in inner_cmd.split([';', '\n']) {
        for part in segment.split("&&").flat_map(|p| p.split("||")) {
            // Pipes: only the LEFT side reads files.
            let part = part.split('|').next().unwrap_or("");
            let mut toks = part.split_whitespace();
            let Some(head) = toks.next() else { continue };
            let base = head.rsplit('/').next().unwrap_or(head);
            if base != "cat" && base != "bat" {
                continue;
            }
            for arg in toks {
                if arg.starts_with('-') {
                    continue;
                }
                let path = if std::path::Path::new(arg).is_absolute() {
                    std::path::PathBuf::from(arg)
                } else {
                    cwd.join(arg)
                };
                let Ok(md) = std::fs::metadata(&path) else {
                    continue;
                };
                if !md.is_file()
                    || md.len() < tokenade_core::dedup_ref::MIN_BYTES_TO_DEDUPE as u64
                    || md.len() > 2 * 1024 * 1024
                {
                    continue;
                }
                let Ok(bytes) = std::fs::read_to_string(&path) else {
                    continue; // binary / non-utf8 — Read serves those specially
                };
                let hash = tokenade_core::dedup_ref::hash_content(bytes.as_bytes());
                tokenade_core::dedup_ref::register(cwd, &hash, &bytes);
                ctx_tag(parsed, &hash);
                tokenade_core::debug_log::log(
                    "hookwrap",
                    "bash_served_file_registered",
                    serde_json::json!({"file": arg, "hash": hash, "size": md.len()}),
                );
            }
        }
    }
}

pub(crate) fn handle_read_pretool(parsed: &serde_json::Value, started: std::time::Instant) -> i32 {
    if hook_disabled_by_env() {
        return 0;
    }
    let file_path = tool_input_str(parsed, "file_path");
    if file_path.is_empty() {
        return 0;
    }
    // A `Read(…)` deny/ask rule is judged on the path this hook RETURNS: a
    // redirect to a converted copy would carry a denied file past it.
    let cwd = parsed
        .get("cwd")
        .and_then(|c| c.as_str())
        .map(std::path::Path::new);
    if crate::hooks::permission_rules::a_user_rule_may_guard_path(file_path, cwd) {
        return 0;
    }
    // Our own compacted-view temp files (a prior specific-compactor redirect)
    // must read back verbatim — re-compacting would loop and they are already
    // the minimal view. Pass through before any work.
    if is_read_temp_file(file_path) {
        return 0;
    }
    // Image downscaling: an oversized image costs vision tokens by AREA.
    // Redirect the Read to a downscaled copy (cache-stable, under
    // .tokenade/reads/), leave the original on disk, and point the agent at a
    // full-resolution copy if it needs the detail. Covers every agent sharing
    // this PreToolUse handler — Claude Code, Codex, Grok. Disable with
    // TOKENADE_IMAGE_MAX_EDGE=0.
    if let Some(code) = handle_image_downscale_pretool(parsed, file_path, started) {
        return code;
    }
    let offset = parsed
        .get("tool_input")
        .and_then(|i| i.get("offset"))
        .and_then(|n| n.as_u64())
        .unwrap_or(0);
    let limit = parsed
        .get("tool_input")
        .and_then(|i| i.get("limit"))
        .and_then(|n| n.as_u64())
        .unwrap_or(0);

    // Cheap stat — bail if not a regular file or doesn't exist.
    let path = std::path::PathBuf::from(file_path);
    let meta = match std::fs::metadata(&path) {
        Ok(m) if m.is_file() => m,
        _ => return 0,
    };
    let size = meta.len();

    // Claude Code auto-memory interceptor was retired alongside the
    // custom memory subsystem — agents now read their own memory dirs
    // without tokenade mediating. The Read still flows through the
    // standard dedup path below (which handles repeat-reads cleanly).

    if size < tokenade_core::dedup_ref::MIN_BYTES_TO_DEDUPE as u64 {
        return 0; // too small — UI cost would exceed savings
    }
    // Cap at 50 MB to prevent OOM. Below, two `std::fs::read_to_string`
    // calls load the WHOLE file (for dedup-hit inflation check + diff/
    // compact paths). An agent Reading a multi-GB log file would OOM
    // the hook before Claude Code's own Read tool applies its
    // truncation. Bail to let Claude Code handle the read natively.
    const MAX_READ_FILE_BYTES: u64 = 50 * 1024 * 1024;
    if size > MAX_READ_FILE_BYTES {
        tokenade_core::debug_log::log(
            "hookwrap-pretool",
            "read_skipped_too_large",
            serde_json::json!({
                "file": file_path,
                "size": size,
                "cap_bytes": MAX_READ_FILE_BYTES,
                "elapsed_ms": started.elapsed().as_millis() as u64,
            }),
        );
        return 0;
    }
    // Deny-audit: SECOND granularity let two same-second writes of equal
    // size share an identity — a Read after the 2nd write matched the
    // marker from the 1st and served a STALE §ref. Nanos close it (ext4/
    // NTFS/APFS all carry sub-second mtimes; filesystems that don't just
    // keep the old exposure, no worse).
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);

    // Dedup key encodes path + slice + identity (mtime + size).
    // Changing any of those = different bytes = different key.
    let dedup_key_input = format!("{file_path}|{mtime}|{size}|{offset}|{limit}");
    let hash = tokenade_core::dedup_ref::hash_content(dedup_key_input.as_bytes());
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    // ── Image re-read dedup ─────────────────────────────────────────────
    // Claude Code (and other vision agents) deliver an image Read as a
    // base64 VISION block, NOT text — so the text dedup/extract paths below
    // (`read_to_string`) never touch it, and a byte-identical RE-read in the
    // same session re-ships the entire base64 image the model is already
    // looking at upthread (measured: the same slide `.jpg` read 3× in field
    // data). Dedup on the SAME path+mtime+size identity already encoded in
    // `hash`, reusing the exact `lookup`/`register` store as text dedup — so
    // this inherits text dedup's session + freshness semantics (an edited
    // image gets a new mtime/size → new hash → not deduped, correctly
    // re-loaded). The first sighting registers + passes through (the model
    // SEES the image); a later identical read is substituted with a pointer
    // to the prior view. No `expand_ref` marker — we can't re-serve an image
    // as text, and we don't need to: it's still in context above.
    // CLASS-1 gate (audit V1-V4): while in-context claims are unverifiable
    // (sidechain window / recent compaction), EVERY substitution phase below
    // is skipped — full serves only. Registration resumes when safe.
    // Class-1 endgame: per-claim POSITIVE proofs replaced the blanket
    // claims_unsafe gate (kept only inside ctx_claim_ok as the no-transcript
    // fallback + the compaction-recency check).

    if let Some(rc) = handle_image_dedup_or_register(
        parsed, file_path, &cwd, &hash, offset, limit, size, mtime, started,
    ) {
        return rc;
    }

    if let Some(rc) = {
        handle_identity_dedup_hit(
            parsed, file_path, &path, &cwd, &hash, offset, limit, size, mtime, started,
        )
    } {
        return rc;
    }

    // ── Range-coverage dedup (slice paging) ─────────────────────────
    // The exact-hash check missed (different offset/limit/version), but
    // this slice may be FULLY CONTAINED in bytes already delivered
    // VERBATIM earlier in THIS session — so still in the agent's context.
    // Serve a §ref instead of re-shipping. Session-scoped + version-keyed
    // (`range_is_covered`) is what guarantees "still in context"; only an
    // explicit `limit>0` slice can be a coverage *consumer* (a whole-file
    // read's contained sub-reads are the win). Recovery via expand-ref.
    // Context-scoped key (transcript_path folded in): a subagent's ranges
    // never collide with the parent's — the cause-level fix for the false
    // "already delivered" claim, with no suspended gain.
    let ctx_key = context_key(parsed);
    let range_version = format!("{mtime}|{size}");
    let range_key = tokenade_core::dedup_ref::range_file_key(file_path);
    if let Some(rc) = handle_range_coverage_dedup(
        parsed,
        file_path,
        &path,
        offset,
        limit,
        &ctx_key,
        &range_version,
        &range_key,
        started,
    ) {
        return rc;
    }

    // Identity MISS path. Two sub-cases:
    //   (a) path-keyed cache HIT — same file+slice, different bytes.
    //       Agent is re-reading after an edit. Compute a unified
    //       diff against the prior bytes and serve THAT as the read
    //       result, then update the path pointer.
    //   (b) path-keyed cache MISS — first time we see this slice.
    //       Register and passthrough.
    let path_key = tokenade_core::dedup_ref::path_key(file_path, offset, limit);
    let sliced_current = match read_file_sliced_and_redact(
        parsed, file_path, &path, &cwd, offset, limit, size, started,
    ) {
        Ok(s) => s,
        Err(rc) => return rc,
    };

    // L21 — Cross-tool content dedup. Before the diff / auto-compact
    // paths, check whether this exact byte sequence already lives in
    // the project's dedup cache under its CONTENT hash (registered
    // by a prior Bash run via `tokenade wrap`, by a prior Read of a
    // different file with identical bytes, etc.). When it does we
    // substitute with the existing §ref§ marker — the bytes are
    // already in the agent's earlier turn, no need to re-ship them.
    //
    // This unifies the Read+Bash dedup namespaces: a `cat foo.json`
    // followed by `Read foo.json` (or the reverse) collapses to a
    // single full transfer + N tiny refs across both tools.
    //
    // Normalisation: Bash's `tokenade wrap` cache often holds the
    // file content WITHOUT trailing newline (some pipes drop it,
    // some compactors trim). Read sees the file with whatever
    // newlines are on disk. So we try BOTH `sliced_current` AND
    // `sliced_current.trim_end_matches('\n')` — whichever hits wins.
    let trimmed_current = sliced_current.trim_end_matches('\n');
    let content_hash = tokenade_core::dedup_ref::hash_content(sliced_current.as_bytes());
    let trimmed_hash = tokenade_core::dedup_ref::hash_content(trimmed_current.as_bytes());
    // Built here rather than at the three slice-diff calls below: this is the
    // first strategy that needs it, and one construction serves all four.
    let req = crate::hooks::read_diffs::ReadRequest {
        parsed,
        file_path,
        cwd: &cwd,
        hash: &hash,
        content_hash: &content_hash,
        sliced_current: &sliced_current,
        trimmed_hash: &trimmed_hash,
        trimmed_current,
        path_key: &path_key,
        offset,
        limit,
        started,
    };
    if let Some(rc) = handle_cross_tool_content_dedup(&req) {
        return rc;
    }

    if let Some(rc) = try_exact_slice_diff(&req) {
        return rc;
    }
    if let Some(rc) = try_paginated_slice_diff(&req) {
        return rc;
    }
    if let Some(rc) = try_session_slice_diff(&req, &ctx_key, &range_key) {
        return rc;
    }

    // L22 — Source-code Comments strip (offset==0 && limit==0 only,
    // file >= 5 KB). For full-file Reads of known languages
    // (rs/py/ts/js/go/...), apply `code_strip(Comments)` which
    // drops `//` `/* */` `#` ordinary comments BUT preserves
    // doc-comments (`///`, `/** */`, `"""…"""`). Skipped when the
    // agent passes offset/limit because comment-stripping shifts
    // line numbers and slices are how Edits target a line. Bypass
    // via `TOKENADE_NO_CODE_STRIP=1`.
    //
    // Why this is worth the line-shift trade-off: full-file Reads
    // of large source are usually exploratory (the agent wants
    // structure, not exact lines). The banner tells them to
    // re-Read with offset/limit when they need precision, and the
    // raw bytes stay accessible via `expand_ref`.
    // P1c (flow analysis 2026-06) — editable-source guard. A compacted
    // Read of a source file is what the agent bases its next Edit on:
    // stripped comments / collapsed blanks / head-tail truncation all
    // produce `old_string`s that no longer match the bytes on disk.
    // For files inside a git work tree (the ones the agent edits) and
    // below EDITABLE_SOURCE_MAX_TOKENS, the savings don't cover that
    // risk — serve raw bytes. Above the ceiling the Read is almost
    // always exploratory and compaction wins. The dedup / diff passes
    // above stay active either way: identical bytes can't break an
    // Edit.
    let editable_source_guard = path
        .extension()
        .and_then(|e| e.to_str())
        .and_then(tokenade_core::code_strip::language_for_extension)
        .is_some()
        && tokenade_core::tokenizer::estimate(&sliced_current) < EDITABLE_SOURCE_MAX_TOKENS
        && inside_git_work_tree(&path);
    if let Some(rc) = try_code_strip(&req, &path, size, editable_source_guard) {
        return rc;
    }

    // L16 — last-chance pass: format-specific lossless auto-compact. Also
    // computes `read_kind_folds_values`, which the terminal disclose phase
    // consumes (value-folding kinds must pass through, never disclose-stash).
    let (auto_rc, read_kind_folds_values) = try_auto_compact(&req, editable_source_guard);
    if let Some(rc) = auto_rc {
        return rc;
    }

    emit_passthrough_with_disclosure(
        parsed,
        file_path,
        &path,
        &cwd,
        &hash,
        &content_hash,
        &trimmed_hash,
        trimmed_current,
        &path_key,
        &sliced_current,
        &ctx_key,
        &range_version,
        &range_key,
        offset,
        limit,
        size,
        editable_source_guard,
        read_kind_folds_values,
        started,
    )
}

// ── Read-compaction delivery (flow analysis 2026-06) ────────────────
//
// Historically every Read compaction above was delivered as a
// PreToolUse DENY whose `permissionDecisionReason` carried the
// compacted content. Claude Code surfaces a deny as a tool ERROR, so
// the file was never marked "read" — and the agent's next Edit failed
// with "File has not been read yet" (469× in the May–June 2026
// transcript corpus), costing a recovery Read that often re-shipped
// the very bytes the compaction saved.
//
// Default delivery is now two-step:
//   1. PreToolUse replies ALLOW with `updatedInput` shrinking the Read
//      to a `READ_HEAD_LINES` head slice — the file is genuinely read
//      (and marked as such) at a ~20-line cost;
//   2. the compacted content is staged in ~/.tokenade/read-pending/
//      and the `hookwrap-postread` PostToolUse hook attaches it to the
//      SAME tool call via `additionalContext`.
//
// The deny path survives as a fallback — `TOKENADE_READ_FLOW=deny`,
// postread hook not installed (marker absent), or a staging failure —
// and now carries an explicit "file NOT marked as read" hint.

#[cfg(test)]
mod ctx_generation_tests {
    use super::*;
    use serde_json::json;

    // Env-based pending dir is process-global; serialize the env-touching tests.
    /// ALIAS for [`tokenade_core::platform::ENV_LOCK`], not a second lock. Process
    /// env has exactly one owner: two locks over it do not exclude each other.
    /// Measured as a real flake on 2026-08-27 between the capability lock and
    /// `ScopedEnv` — a test that passed alone failed in the full suite. An earlier
    /// fix named `ENV_LOCK` the single home; these were the copies it missed.
    static ENV_LOCK: &std::sync::Mutex<()> = &tokenade_core::platform::ENV_LOCK;

    fn sidechain_file(dir: &std::path::Path, sid: &str) -> std::path::PathBuf {
        let skey: String = sid
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(40)
            .collect();
        dir.join(format!("sidechain-{skey}"))
    }

    /// A subagent shares the parent's session_id AND transcript_path, yet must
    /// dedup only the reads IT made — never falsely claim content the parent
    /// read. Generation-scoping the seen-tag draws exactly that line.
    #[test]
    fn subagent_dedups_its_own_reads_not_the_parents() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("tok-ctxgen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: serialized by ENV_LOCK; test-only override.
        unsafe {
            std::env::set_var("TOKENADE_READ_PENDING_DIR", &dir);
        }

        // Same payload for parent and subagent — they are indistinguishable
        // except for the sidechain generation counter.
        let parsed = json!({"session_id": "sessABC", "transcript_path": "/tmp/t.jsonl"});
        let h_parent = "a".repeat(64); // parent-only read
        let h_sub = "b".repeat(64); // read made inside the subagent

        // Parent (generation 0) reads h_parent.
        ctx_tag(&parsed, &h_parent);
        assert!(
            ctx_saw(&parsed, &h_parent),
            "parent must see its own gen-0 read"
        );

        // A subagent spawns → generation 1.
        std::fs::write(sidechain_file(&dir, "sessABC"), "1:1700000000").unwrap();
        assert_eq!(ctx_generation(&parsed), 1);

        // The subagent must NOT claim the parent's read (different generation).
        assert!(
            !ctx_saw(&parsed, &h_parent),
            "subagent must not claim parent content"
        );
        assert!(
            !ctx_claim_ok(&parsed, &h_parent),
            "claim on inherited content must be denied"
        );

        // The subagent reads h_sub, then re-reads it → dedup fires for ITS own read.
        ctx_tag(&parsed, &h_sub);
        assert!(
            ctx_saw(&parsed, &h_sub),
            "subagent must dedup its own re-read"
        );
        assert!(
            ctx_claim_ok(&parsed, &h_sub),
            "own-read claim must be admitted in a subagent"
        );

        // Subagent finishes → generation back to 0. Parent still sees its read;
        // the subagent's gen-1 read is (correctly) not in the parent's context.
        let _ = std::fs::remove_file(sidechain_file(&dir, "sessABC"));
        assert_eq!(ctx_generation(&parsed), 0);
        assert!(
            ctx_saw(&parsed, &h_parent),
            "parent still sees its gen-0 read after subagent"
        );
        assert!(
            !ctx_saw(&parsed, &h_sub),
            "parent must not inherit the subagent's read"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The dedup circuit breaker is the only thing that stops an agent looping on
/// a pointer it cannot use, and PreToolUse hooks for parallel Read calls run
/// concurrently. A read-parse-write counter loses increments under exactly
/// that — the breaker would then never reach its trip point, and the loop it
/// exists to break would run forever.
#[cfg(test)]
mod breaker_tests {
    use super::{breaker_bump, breaker_hits, BREAKER_TRIP};

    #[test]
    fn concurrent_hits_are_never_lost() {
        let dir = std::env::temp_dir().join(format!("tk-breaker-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("denyct-x");

        const N: u64 = 64;
        std::thread::scope(|s| {
            for _ in 0..N {
                s.spawn(|| breaker_bump(&path));
            }
        });

        assert_eq!(
            breaker_hits(&path),
            N,
            "increments were lost: a read-modify-write counter is back"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_counter_left_by_an_older_build_still_trips() {
        // Older builds wrote the digit itself. One byte reads as one hit, so
        // the breaker under-counts once and then self-corrects — it must never
        // read as already-tripped or as never-tripping.
        let dir = std::env::temp_dir().join(format!("tk-breaker-legacy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("denyct-legacy");
        std::fs::write(&path, "2").unwrap();

        assert_eq!(breaker_hits(&path), 1, "legacy file must read as one hit");
        breaker_bump(&path);
        assert!(
            breaker_hits(&path) >= BREAKER_TRIP,
            "the next hit must trip the breaker"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
