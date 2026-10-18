//! Not serving the same bytes twice.
//!
//! Three shapes of repeat: the identical read, the range already covered by
//! an earlier one, and content already delivered through another tool. Each
//! is keyed on the transcript context so a compaction resets them together.

use super::*;

/// Range-coverage dedup opt-out (mirrors the other per-lever gates).
pub(crate) fn range_dedup_disabled() -> bool {
    tokenade_core::env_compat::is_truthy_one("TOKENADE_NO_RANGE_DEDUP")
}

/// Exact-identity dedup hit (extracted phase of `handle_read_pretool`). The
/// metadata-identity `hash` (path+mtime+size+slice) matched a prior Read, so the
/// bytes are already in the agent's earlier turn — substitute a `§ref` marker
/// instead of re-shipping, guarded against inflation. Returns `Some(0)` on hit
/// (substitute / inflation-skip / read-failure), `None` on lookup miss.
pub(crate) fn handle_identity_dedup_hit(
    parsed: &serde_json::Value,
    file_path: &str,
    path: &std::path::Path,
    cwd: &std::path::Path,
    hash: &str,
    offset: u64,
    limit: u64,
    size: u64,
    mtime: u64,
    started: std::time::Instant,
) -> Option<i32> {
    if tokenade_core::dedup_ref::lookup(cwd, hash).is_some() && ctx_claim_ok(parsed, hash) {
        // Cache HIT — substitute. The reason field becomes the
        // content the model sees as the "Read" result.
        // #708 — drop the duplicated `expand_ref` mention; ref_marker
        // above already prints it inline. Footer keeps the metadata
        // (size/mtime/"prior Read in this project") that the marker
        // doesn't have — ~25 tokens saved per dedup-hit.
        let reason = format!(
            "[tokenade:read-dedup unchanged]\n{marker}\n\n\
             (file `{file_path}` matches a prior Read in this project — \
             size={size} mtime={mtime}. The bytes are already in your earlier turn.)",
            marker = tokenade_core::dedup_ref::ref_marker(hash, size as usize),
        );
        // Anti-inflation guard: read what would have been served vs
        // the marker, in tokens. If the marker is bigger, passthrough.
        // The "agent already saw it" rationale only matters when we
        // shrink — otherwise we charge the model more tokens for less
        // information.
        let Ok(actual_bytes) = std::fs::read_to_string(path) else {
            return Some(0);
        };
        let original_tokens = tokenade_core::tokenizer::estimate(&actual_bytes);
        let reason_tokens = tokenade_core::tokenizer::estimate(&reason);
        if reason_tokens >= original_tokens {
            tokenade_core::debug_log::log(
                "hookwrap-pretool",
                "read_dedup_skipped_inflation",
                serde_json::json!({
                    "file": file_path,
                    "original_tokens": original_tokens,
                    "reason_tokens": reason_tokens,
                    "elapsed_ms": started.elapsed().as_millis() as u64,
                }),
            );
            return Some(0);
        }
        // Double-count audit (inverse): this substitution never CREDITED
        // its emit — mirror of the D1 booking fix.
        if let Some(net) = emit_read_compaction(parsed, file_path, offset, limit, reason, false) {
            gain_log::record("hook", "read_identity_dedup", file_path, net);
        }
        tokenade_core::debug_log::log(
            "hookwrap-pretool",
            "read_dedup_hit",
            serde_json::json!({
                "file": file_path,
                "size": size,
                "hash": hash,
                "elapsed_ms": started.elapsed().as_millis() as u64,
            }),
        );
        return Some(0);
    }
    None
}

/// Per-CONTEXT dedup discriminator. Claude Code gives hooks no context
/// identity in stdin, but it DOES pass `transcript_path` — and a spawned
/// subagent writes to a DIFFERENT transcript file than its parent. Folding
/// a short hash of it into the session key means a subagent's reads dedup
/// only against ITS OWN context, never against the parent's (the false
/// "already in your context" class) — recovering multi-agent gain WITHOUT
/// suspending anything. Empty transcript_path (rare) → session_id alone,
/// where the sidechain counter still guards.
pub(crate) fn context_key(parsed: &serde_json::Value) -> String {
    let sid = crate::hooks::session_id_of(parsed).unwrap_or("");
    let base: String = sid
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(40)
        .collect();
    match parsed.get("transcript_path").and_then(|t| t.as_str()) {
        Some(tp) if !tp.is_empty() => {
            format!(
                "{base}@{}",
                &tokenade_core::dedup_ref::hash_content(tp.as_bytes())[..8]
            )
        }
        _ => base,
    }
}

/// Filename-safe context id (context_key with '@' flattened).
pub(crate) fn ctx_file_id(parsed: &serde_json::Value) -> String {
    context_key(parsed).replace('@', "-")
}

/// POSITIVE context proof — the definitive class-1 fix. Every delivery
/// tags (context, content-hash); a claim about "already in your context"
/// is served only on proof. No suspension windows: a sibling context
/// simply has no tag and gets a full serve.
/// Sidechain generation: 0 on the main thread, ≥1 while N subagents are active
/// (the `sidechain-{skey}` counter the Task/Agent hook maintains). Folded into
/// the seen-tag below. A subagent shares the parent's session_id and — per live
/// audit (3×) — its transcript_path, so a bare context tag cannot tell a
/// subagent's OWN read from parent content it merely inherited. Scoping the tag
/// by generation draws exactly that line: parent (gen 0) and each subagent
/// depth get distinct tags, so a subagent dedups the reads IT made and never
/// claims the parent's. Cost: two subagents reading identical bytes at the same
/// generation could cross-claim — rare, and the circuit breaker full-serves it.
pub(super) fn ctx_generation(parsed: &serde_json::Value) -> u64 {
    let sid = crate::hooks::session_id_of(parsed).unwrap_or("");
    if sid.is_empty() {
        return 0;
    }
    let skey: String = sid
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(40)
        .collect();
    sidechain_generation(&skey)
}

/// The same generation, addressed by session KEY rather than by hook payload.
///
/// #289 — `wrap` runs as the Bash tool's child and never sees the payload, so
/// the bash side could not reach the reasoning above and its proof stayed
/// session-scoped. Extracted rather than copied: the counter's filename is now
/// decided in ONE place, and the two dedup paths cannot drift apart on it.
pub(crate) use tokenade_core::session::sidechain_generation;

/// Shared path for the positive context proof — written by [`ctx_tag`], read by
/// [`ctx_saw`]. One builder so tag and check can never drift. `None` when the
/// context has no identity (empty key).
pub(super) fn ctx_seen_path(parsed: &serde_json::Value, hash: &str) -> Option<std::path::PathBuf> {
    let ctx = ctx_file_id(parsed);
    if ctx.is_empty() {
        return None;
    }
    let gen = ctx_generation(parsed);
    Some(super::read_staging::read_pending_dir().join(format!(
        "ctxseen-{ctx}-g{gen}-{}",
        &hash[..hash.len().min(16)]
    )))
}

pub(crate) fn ctx_tag(parsed: &serde_json::Value, hash: &str) {
    if let Some(p) = ctx_seen_path(parsed, hash) {
        tokenade_core::platform::ensure_parent_dir(&p);
        let _ = std::fs::write(&p, b"1");
    }
}

pub(super) fn ctx_saw(parsed: &serde_json::Value, hash: &str) -> bool {
    ctx_seen_path(parsed, hash)
        .map(|p| p.exists())
        .unwrap_or(false)
}

/// Claim admissibility for content-addressed ("in your context") dedup.
/// Transcript-scoped contexts use the positive tag; sessions without a
/// transcript_path (older/foreign agents) keep the counter fallback.
pub(crate) fn ctx_claim_ok(parsed: &serde_json::Value, hash: &str) -> bool {
    let sid = crate::hooks::session_id_of(parsed).unwrap_or("");
    if compaction_recent(sid) {
        return false;
    }
    // Deny-audit (CRITICAL, reproduced live 3x): real subagents inherit the
    // PARENT's transcript_path, so the positive proof once could NOT tell a
    // subagent's own read from parent content it inherited — hence the old
    // blanket denial for the whole subagent window. The seen-tag is now
    // GENERATION-scoped (see `ctx_generation`), which draws that line exactly:
    // a subagent matches only reads IT made. So the transcript-proof path no
    // longer blanket-denies during a window — a subagent gets the SAME dedup as
    // the main thread, scoped to its own context (the capability parity Paul
    // asked for). The no-transcript fallback (older/foreign agents: no proof to
    // scope) keeps the window guard as its only safeguard.
    let claim = if context_key(parsed).contains('@') {
        ctx_saw(parsed, hash)
    } else {
        !sidechain_window_active(sid)
    };
    if !claim {
        return false;
    }
    // Deny-audit circuit breaker: if the agent re-Reads the SAME identity
    // right after two consecutive dedup claims, our "it's above" answer
    // isn't landing (whatever the cause) — full-serve the 3rd attempt
    // instead of looping the agent forever on an unusable pointer.
    let breaker = super::read_staging::read_pending_dir().join(format!(
        "denyct-{}-{}",
        ctx_file_id(parsed),
        &hash[..16.min(hash.len())]
    ));
    // Counted by LENGTH, one appended byte per hit — not read-parse-write.
    // Parallel Read calls run their PreToolUse hooks concurrently, and a
    // read-modify-write loses increments under exactly that; losing them here
    // means the breaker never reaches 2 and the agent loops forever on the
    // unusable pointer this breaker exists to escape. A sub-PIPE_BUF O_APPEND
    // write is atomic, so concurrent hits all land. (A counter file left by an
    // older build holds the digit "1"/"2" = 1 byte, so it reads as one hit and
    // self-corrects on the next append; these live in the transient
    // read-pending dir.)
    let hits = breaker_hits(&breaker);
    if hits >= BREAKER_TRIP {
        let _ = std::fs::remove_file(&breaker);
        tokenade_core::debug_log::log(
            "hookwrap-pretool",
            "dedup_circuit_breaker_full_serve",
            serde_json::json!({"hash": hash, "hits": hits}),
        );
        return false;
    }
    breaker_bump(&breaker);
    true
}

/// Hits recorded so far for this (file, identity) pair.
pub(crate) fn breaker_hits(path: &std::path::Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Record one hit. See the invariant note at the call site: one appended byte,
/// never read-modify-write, so concurrent PreToolUse hooks cannot lose counts.
pub(crate) fn breaker_bump(path: &std::path::Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, b"x"));
}

/// Content-addressed dedup for any EXTRACTED document we're about to serve
/// (PDF text, docx/odt/pptx/xlsx/ods/…), reusing the shared read-dedup
/// primitives — NOT a new mechanism. Returns true (and emits a deny that points
/// the agent back to the copy already in context, plus the `--prompt` nudge)
/// when this exact document was already served this session and is still in
/// context. `ctx_claim_ok` is generation-scoped, so this fires on the main
/// thread AND inside a subagent for that subagent's OWN earlier read. `false`
/// on a miss — the caller serves normally, then calls [`doc_dedup_register`].
pub(super) fn doc_dedup_hit(
    parsed: &serde_json::Value,
    cwd: &std::path::Path,
    file_path: &str,
    hash: &str,
    would_serve_tokens: usize,
    size: u64,
    started: std::time::Instant,
) -> bool {
    if tokenade_core::dedup_ref::lookup(cwd, hash).is_none() || !ctx_claim_ok(parsed, hash) {
        return false;
    }
    let reason = format!(
        "[tokenade:doc-dedup unchanged] `{file_path}` was already served as text earlier in \
         this session — it is still in your context above, so it was not re-served. To pull \
         something specific without re-reading, ask: `tokenade read {file_path} --prompt \
         \"<what you need>\"` (several questions at once, comma-separated). Re-read only if the \
         file changed."
    );
    let after = tokenade_core::tokenizer::estimate(&reason);
    // Refusing has to be cheaper than re-serving. The gate upstream compares
    // the FILE's byte size against `MIN_BYTES_TO_DEDUPE`, which is the wrong
    // quantity for a document: a container is large on disk and small once
    // extracted. Measured 2026-09-02 on `pptx.pptx` — 33 467 bytes holding 62
    // tokens of text — the deny message costs 102 tokens, so saying "you
    // already have it" made the context 40 tokens BIGGER than re-serving the
    // whole deck. Same doctrine as the compactor's own safety net: never
    // return more than doing nothing would.
    if would_serve_tokens <= after {
        return false;
    }
    let decision = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    });
    println!("{decision}");
    gain_log::record(
        "hook",
        "read_doc_dedup",
        file_path,
        tokenade_core::Savings::new(would_serve_tokens, after),
    );
    tokenade_core::debug_log::log(
        "hookwrap-pretool",
        "read_doc_dedup_hit",
        serde_json::json!({
            "file": file_path,
            "size": size,
            "hash": hash,
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
    true
}

/// Register a served document's identity so the NEXT read of it dedups. Same
/// first-sighting primitives as the image path (register + generation-scoped
/// ctx_tag + path pointer); empty body — `lookup` only needs existence.
pub(super) fn doc_dedup_register(
    parsed: &serde_json::Value,
    cwd: &std::path::Path,
    file_path: &str,
    hash: &str,
) {
    tokenade_core::dedup_ref::register(cwd, hash, "");
    ctx_tag(parsed, hash);
    let pk = tokenade_core::dedup_ref::path_key(file_path, 0, 0);
    tokenade_core::dedup_ref::register_path_pointer(cwd, &pk, hash);
}

/// Range-coverage dedup / slice paging (extracted phase of
/// `handle_read_pretool`). The exact-hash check missed, but this `limit>0` slice
/// may be FULLY CONTAINED in bytes delivered verbatim earlier THIS session, so
/// still in the agent's context — serve a `§ref` instead of re-shipping. Returns
/// `Some(0)` only when it actually substitutes (passing the anti-inflation
/// guard); `None` otherwise.
pub(crate) fn handle_range_coverage_dedup(
    parsed: &serde_json::Value,
    file_path: &str,
    path: &std::path::Path,
    offset: u64,
    limit: u64,
    session_id: &str,
    range_version: &str,
    range_key: &str,
    started: std::time::Instant,
) -> Option<i32> {
    // Range table is CONTEXT-scoped (transcript folded into the key), so
    // its claims are self-proving; the counter fallback only guards keys
    // without a transcript discriminator.
    if !session_id.contains('@') && sidechain_window_active(session_id) {
        return None;
    }
    // Context-scoping proves WE delivered those lines; it does NOT prove the
    // model still holds them. A PreCompact summarises the transcript away, and
    // this path then tells the agent "scroll up, they're still in your context"
    // about bytes that are gone — the one thing a compaction claim must never
    // do. `ctx_claim_ok` applies this same check to the five other dedup paths
    // (identity, exact-slice, paginated-slice, cross-tool, doc); the range
    // table was the hole. See `compaction_recent`: required even same-context.
    if compaction_recent(session_id) {
        return None;
    }
    if !range_dedup_disabled() && limit > 0 && !session_id.is_empty() {
        // `offset` is Claude Code's 1-based line number; the ranges are 0-based.
        let rstart = offset.saturating_sub(1);
        let rend = rstart.saturating_add(limit);
        if let Some(cover_hash) = tokenade_core::dedup_ref::range_is_covered(
            session_id,
            range_key,
            range_version,
            rstart,
            rend,
            // Same window as `compaction_recent`, deliberately — see the
            // invariant on that function. A claim younger than W plus a gate
            // covering the last W is what makes a pre-compaction read
            // unclaimable. Two different windows would open a silent hole.
            read_context_recency_secs(),
        ) {
            if let Ok(full) = std::fs::read_to_string(path) {
                let slice = slice_lines(&full, rstart, rend);
                let slice_tokens = tokenade_core::tokenizer::estimate(&slice);
                let marker = tokenade_core::dedup_ref::ref_marker(&cover_hash, slice.len());
                let reason = format!(
                    "[tokenade:read-range-dedup unchanged]\n{marker}\n\n\
                     (lines {}–{rend} of `{file_path}` were already delivered \
                     verbatim earlier this session and are still in your context — \
                     scroll up, or run `tokenade expand-ref {cover_hash}` to refetch.)",
                    rstart + 1
                );
                // Anti-inflation guard (same rule as identity dedup): only
                // substitute when the marker is genuinely smaller.
                if tokenade_core::tokenizer::estimate(&reason) < slice_tokens {
                    let net = emit_read_compaction(parsed, file_path, offset, limit, reason, false);
                    if let Some(net) = net {
                        gain_log::record("hook", "read_range_dedup", file_path, net);
                    }
                    tokenade_core::debug_log::log(
                        "hookwrap-pretool",
                        "read_range_dedup_hit",
                        serde_json::json!({
                            "file": file_path,
                            "start": rstart,
                            "end": rend,
                            "cover_hash": cover_hash,
                            "elapsed_ms": started.elapsed().as_millis() as u64,
                        }),
                    );
                    return Some(0);
                }
            }
        }
    }
    None
}

pub(crate) fn handle_cross_tool_content_dedup(
    req: &crate::hooks::read_diffs::ReadRequest<'_>,
) -> Option<i32> {
    let crate::hooks::read_diffs::ReadRequest {
        parsed,
        file_path,
        cwd,
        hash,
        content_hash,
        sliced_current,
        offset,
        limit,
        started,
        trimmed_hash,
        trimmed_current,
        path_key,
        ..
    } = *req;
    // The session is not read here: `ctx_claim_ok` takes `parsed` and derives
    // it itself, so a second copy could only ever disagree with the first.
    let cross_hit = tokenade_core::dedup_ref::lookup(cwd, content_hash)
        .map(|p| (content_hash.to_string(), p))
        .or_else(|| {
            if trimmed_hash != content_hash {
                tokenade_core::dedup_ref::lookup(cwd, trimmed_hash)
                    .map(|p| (trimmed_hash.to_string(), p))
            } else {
                None
            }
        });
    // Class-1 definitive: claim only on POSITIVE proof that THIS context
    // saw those bytes (ctx tag written at delivery time). No window.
    let cross_hit = cross_hit.filter(|(h, _)| ctx_claim_ok(parsed, h));
    if let Some((matched_hash, prior)) = cross_hit {
        let expected = if matched_hash == trimmed_hash {
            trimmed_current
        } else {
            sliced_current
        };
        if prior == expected {
            let content_hash = matched_hash;
            // #708 — drop duplicated `expand_ref` mention; ref_marker
            // already names the recovery path. The lead sentence is
            // the unique info here (which tool produced these bytes).
            let reason = format!(
                "[tokenade:read-content-dedup cross-tool] {file_path} — these bytes are \
                 identical to output another tool (Bash, prior Read of a different file, …) \
                 already produced in this project.\n{marker}",
                marker = tokenade_core::dedup_ref::ref_marker(&content_hash, sliced_current.len()),
            );
            // Anti-inflation guard: the boilerplate text in `reason`
            // is ~40 tokens of fixed overhead + path + hash. For tiny
            // files the marker can be LONGER than the content itself
            // — observed in production at 71→144t (+73 tokens). When
            // that happens, don't substitute — let Claude Code read
            // the bytes normally. We still REGISTER the content so
            // future bigger reads can dedup against it.
            let content_tokens = tokenade_core::tokenizer::estimate(sliced_current);
            let reason_tokens = tokenade_core::tokenizer::estimate(&reason);
            if reason_tokens >= content_tokens {
                tokenade_core::dedup_ref::register(cwd, hash, sliced_current);
                ctx_tag(parsed, hash);
                ctx_tag(parsed, &content_hash);
                tokenade_core::dedup_ref::register_path_pointer(cwd, path_key, &content_hash);
                tokenade_core::debug_log::log(
                    "hookwrap-pretool",
                    "read_content_dedup_skipped_inflation",
                    serde_json::json!({
                        "file": file_path,
                        "content_tokens": content_tokens,
                        "reason_tokens": reason_tokens,
                        "elapsed_ms": started.elapsed().as_millis() as u64,
                    }),
                );
                return Some(0);
            }
            let net_sav = emit_read_compaction(parsed, file_path, offset, limit, reason, false);
            // Refresh the metadata pointer for the fast HIT path next time.
            tokenade_core::dedup_ref::register(cwd, hash, sliced_current);
            tokenade_core::dedup_ref::register_path_pointer(cwd, path_key, &content_hash);
            // Log the REALISED savings (raw → head-slice + injected
            // additionalContext), as measured centrally in emit_read_compaction.
            // `None` means emit passed the Read through — nothing was saved.
            if let Some(net_sav) = net_sav {
                gain_log::record("hook", "read_content_dedup", file_path, net_sav);
            }
            tokenade_core::debug_log::log(
                "hookwrap-pretool",
                "read_content_dedup_hit",
                serde_json::json!({
                    "file": file_path,
                    "content_hash": content_hash,
                    "size": sliced_current.len(),
                    "elapsed_ms": started.elapsed().as_millis() as u64,
                }),
            );
            return Some(0);
        }
    }
    None
}
