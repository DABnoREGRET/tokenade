//! **The agent re-reads a piece it has ALREADY seen.**
//!
//! Three forms of the same finding, and none can be derived from another: the
//! byte-identical slice, the slice contained in a page already delivered, and
//! the slice already served elsewhere in the SESSION.
//!
//! Each can return a diff instead of the file, and each must prove that the
//! earlier bytes are still in context — otherwise the diff talks about a base
//! the agent no longer has.

use super::*;

pub(crate) fn try_exact_slice_diff(req: &ReadRequest<'_>) -> Option<i32> {
    let ReadRequest {
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
    let looked = tokenade_core::dedup_ref::lookup_by_path_with_age(cwd, path_key)
        // Class-1: the "diff vs what you saw" claim needs positive proof
        // that THIS context saw the prior (tag written at delivery).
        .filter(|(ph, _, _)| super::read_pretool::ctx_claim_ok(parsed, ph));
    if looked.is_none() {
        tokenade_core::debug_log::log(
            "hookwrap-pretool",
            "read_diff_decline",
            serde_json::json!({"file": file_path, "why": "no-prior", "key": path_key}),
        );
    }
    if let Some((prior_hash, prior_content, dedup_age_secs)) = looked {
        let recency = read_context_recency_secs();
        if recency != 0 && dedup_age_secs > recency {
            // Prior Read is old enough that its content has likely scrolled out
            // of the model's context window. Serving a §ref diff/"unchanged"
            // here would leave the agent blind → a forced expand_ref round-trip.
            // Re-serve the FULL content instead (fall through), but refresh the
            // pointer so a *recent* subsequent re-read still dedups.
            register_and_tag(parsed, cwd, hash, sliced_current);
            tokenade_core::dedup_ref::register_path_pointer(cwd, path_key, hash);
            tokenade_core::debug_log::log(
                "hookwrap-pretool",
                "read_dedup_skipped_stale_context",
                serde_json::json!({
                    "file": file_path,
                    "age_secs": dedup_age_secs,
                    "recency_window_secs": recency,
                }),
            );
        } else {
            use tokenade_core::line_diff::DiffOutcome;
            match tokenade_core::line_diff::unified(&prior_content, sliced_current) {
                DiffOutcome::Diff {
                    text,
                    added,
                    removed,
                } => {
                    // Win-ratio guard. The diff is only worth serving if
                    // it's noticeably smaller than the full content; LCS
                    // can produce a diff that's BIGGER than the original
                    // when changes are pervasive. The floor lives on
                    // MIN_DIFF_WIN_PCT, which all three diff paths share.
                    let current_tokens = tokenade_core::tokenizer::estimate(sliced_current);
                    let diff_tokens = tokenade_core::tokenizer::estimate(&text);
                    let win_pct = win_pct(current_tokens, diff_tokens);
                    if diff_is_worth_serving(current_tokens, diff_tokens) {
                        let reason = format!(
                        "[tokenade:read-diff vs §ref:{prior_hash}§] {file_path} \
                         changed since last Read: +{added}/-{removed} lines, \
                         diff is {diff_tokens} tokens vs {current_tokens} full ({win_pct}% saved).\n\n\
                         {text}\n\
                         (Fetch the FULL new bytes via `tokenade expand-ref {hash} --raw` \
                         if the diff isn't enough. The prior content lives \
                         under hash {prior_hash}.)"
                    );
                        // L64 — disclose: above 5k tokens, swap the inline
                        // diff for a summary + stashed .diff file so the
                        // agent can grep specific hunks.
                        let reason = tokenade_core::disclose::maybe_stash_with_threshold(
                            reason,
                            tokenade_core::disclose::SourceHint::File(std::path::PathBuf::from(
                                file_path,
                            )),
                            tokenade_core::disclose::read_threshold_tokens(),
                        );
                        // C13 catch: this exact-slice branch FIRED but never
                        // BOOKED — the ledger had zero read_slice_diff rows while
                        // debug showed hits (only the paginated branch recorded).
                        if let Some(net) =
                            emit_read_compaction(parsed, file_path, offset, limit, reason, false)
                        {
                            gain_log::record("hook", "read_slice_diff", file_path, net);
                        }
                        // Register the NEW content under its content
                        // hash AND update the path pointer so the next
                        // diff has the latest version as baseline. The
                        // content-keyed forms (P5 2026-06) keep the
                        // cross-tool namespace current too — without
                        // them a Bash `cat` of the just-diffed file
                        // re-ships the bytes in full.
                        register_and_tag(parsed, cwd, hash, sliced_current);
                        register_and_tag(parsed, cwd, content_hash, sliced_current);
                        if trimmed_hash != content_hash {
                            register_and_tag(parsed, cwd, trimmed_hash, trimmed_current);
                        }
                        tokenade_core::dedup_ref::register_path_pointer(cwd, path_key, hash);
                        tokenade_core::debug_log::log(
                            "hookwrap-pretool",
                            "read_diff_hit",
                            serde_json::json!({
                                "file": file_path,
                                "added": added,
                                "removed": removed,
                                "diff_tokens": diff_tokens,
                                "full_tokens": current_tokens,
                                "saved_pct": win_pct,
                                "elapsed_ms": started.elapsed().as_millis() as u64,
                            }),
                        );
                        return Some(0);
                    }
                    // Below MIN_DIFF_WIN_PCT — diff isn't compact enough. Fall
                    // through to passthrough but still update the
                    // pointer so a future small edit benefits.
                }
                DiffOutcome::Identical => {
                    // The bytes did NOT change but the mtime/size key did
                    // (touch, no-op formatter, git checkout of identical
                    // content) — the identity cache keys on mtime so it
                    // missed. Without this arm the Read passed through in
                    // FULL even though the agent already has every byte
                    // (P5 2026-06: part of the 12% re-read waste). Serve
                    // an "unchanged" marker instead; truthful by
                    // construction — line_diff::unified compared the
                    // actual bytes.
                    let reason = format!(
                        "[tokenade:read-dedup unchanged] {file_path} — bytes identical to \
                     the prior Read (file was touched: mtime changed, content did not).\n{marker}",
                        marker =
                            tokenade_core::dedup_ref::ref_marker(&prior_hash, sliced_current.len()),
                    );
                    let content_tokens = tokenade_core::tokenizer::estimate(sliced_current);
                    let reason_tokens = tokenade_core::tokenizer::estimate(&reason);
                    // Anti-inflation guard, same rationale as the identity
                    // HIT path above.
                    if reason_tokens < content_tokens {
                        let net_sav =
                            emit_read_compaction(parsed, file_path, offset, limit, reason, false);
                        // Register under the NEW mtime-keyed identity hash
                        // so the next identical Read takes the fast path.
                        register_and_tag(parsed, cwd, hash, sliced_current);
                        if let Some(net_sav) = net_sav {
                            gain_log::record("hook", "read_touch_dedup", file_path, net_sav);
                        }
                        tokenade_core::debug_log::log(
                            "hookwrap-pretool",
                            "read_touch_dedup_hit",
                            serde_json::json!({
                                "file": file_path,
                                "size": sliced_current.len(),
                                "prior_hash": prior_hash,
                                "elapsed_ms": started.elapsed().as_millis() as u64,
                            }),
                        );
                        return Some(0);
                    }
                }
                DiffOutcome::TooLarge => {
                    // Skip the diff path entirely.
                }
            }
        }
    }
    None
}

pub(crate) fn try_paginated_slice_diff(req: &ReadRequest<'_>) -> Option<i32> {
    let ReadRequest {
        parsed,
        file_path,
        cwd,
        hash,
        sliced_current,
        offset,
        limit,
        started,
        path_key,
        ..
    } = *req;
    if offset > 0 || limit > 0 {
        let canonical_path_key = tokenade_core::dedup_ref::path_key(file_path, 0, 0);
        if let Some((prior_full_hash, prior_full)) =
            tokenade_core::dedup_ref::lookup_by_path(cwd, &canonical_path_key)
                // Class-1: positive proof this context saw the prior.
                .filter(|(ph, _)| super::read_pretool::ctx_claim_ok(parsed, ph))
        {
            let prior_slice: String = prior_full
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
            use tokenade_core::line_diff::DiffOutcome;
            match tokenade_core::line_diff::unified(&prior_slice, sliced_current) {
                DiffOutcome::Diff {
                    text,
                    added,
                    removed,
                } => {
                    let current_tokens = tokenade_core::tokenizer::estimate(sliced_current);
                    let diff_tokens = tokenade_core::tokenizer::estimate(&text);
                    let win_pct = win_pct(current_tokens, diff_tokens);
                    if diff_is_worth_serving(current_tokens, diff_tokens) {
                        let reason = format!(
                            "[tokenade:read-diff-paginated vs §ref:{prior_full_hash}§ \
                             (full-file baseline, slice offset={offset} limit={limit})] \
                             {file_path} changed: +{added}/-{removed} lines, \
                             diff is {diff_tokens} tokens vs {current_tokens} full \
                             ({win_pct}% saved).\n\n\
                             {text}\n\
                             (Full prior file: `tokenade expand-ref {prior_full_hash} --raw`. \
                             Current slice: `tokenade expand-ref {hash} --raw` after this diff.)"
                        );
                        let reason = tokenade_core::disclose::maybe_stash_with_threshold(
                            reason,
                            tokenade_core::disclose::SourceHint::File(std::path::PathBuf::from(
                                file_path,
                            )),
                            tokenade_core::disclose::read_threshold_tokens(),
                        );
                        if let Some(net) =
                            emit_read_compaction(parsed, file_path, offset, limit, reason, false)
                        {
                            gain_log::record("hook", "read_paginated_diff", file_path, net);
                        }
                        register_and_tag(parsed, cwd, hash, sliced_current);
                        tokenade_core::dedup_ref::register_path_pointer(cwd, path_key, hash);
                        tokenade_core::debug_log::log(
                            "hookwrap-pretool",
                            "read_diff_paginated_fallback_hit",
                            serde_json::json!({
                                "file": file_path,
                                "offset": offset,
                                "limit": limit,
                                "added": added,
                                "removed": removed,
                                "diff_tokens": diff_tokens,
                                "full_tokens": current_tokens,
                                "saved_pct": win_pct,
                                "elapsed_ms": started.elapsed().as_millis() as u64,
                            }),
                        );
                        return Some(0);
                    }
                }
                DiffOutcome::Identical | DiffOutcome::TooLarge => {
                    // Identical: prior (0,0) slice matches current — L21
                    // should have caught this via content hash; passthrough.
                    // TooLarge: skip.
                }
            }
        }
    }
    None
}

pub(crate) fn try_session_slice_diff(
    req: &ReadRequest<'_>,
    session_id: &str,
    range_key: &str,
) -> Option<i32> {
    let ReadRequest {
        parsed,
        file_path,
        cwd,
        hash,
        content_hash,
        sliced_current,
        offset,
        limit,
        started,
        ..
    } = *req;
    // A diff is only readable against bytes the model still holds. Its two
    // siblings in this file (`try_exact_slice_diff`, `try_paginated_slice_diff`)
    // go through `ctx_claim_ok`; this session-scoped fallback did not, so after
    // a PreCompact it could hand the agent "+3/-2 lines vs a prior Read" whose
    // baseline had just been summarised away — an unusable anchor to rebuild an
    // Edit's `old_string` from. Same guard, applied here.
    if compaction_recent(session_id) {
        return None;
    }
    if !range_dedup_disabled() && (offset > 0 || limit > 0) && !session_id.is_empty() {
        if let Some((prior_hash, prior_content)) =
            tokenade_core::dedup_ref::lookup_session_file_latest(
                session_id,
                range_key,
                read_context_recency_secs(),
            )
        {
            // Skip an identical baseline — unchanged bytes are the identity /
            // range passes' job, not the diff's.
            if prior_hash != content_hash {
                use tokenade_core::line_diff::DiffOutcome;
                if let DiffOutcome::Diff {
                    text,
                    added,
                    removed,
                } = tokenade_core::line_diff::unified(&prior_content, sliced_current)
                {
                    let current_tokens = tokenade_core::tokenizer::estimate(sliced_current);
                    let diff_tokens = tokenade_core::tokenizer::estimate(&text);
                    let win_pct = win_pct(current_tokens, diff_tokens);
                    if diff_is_worth_serving(current_tokens, diff_tokens) {
                        let reason = format!(
                            "[tokenade:read-diff-slice vs §ref:{prior_hash}§ (a prior Read of \
                             this file, this session)] {file_path} changed: +{added}/-{removed} \
                             lines, diff is {diff_tokens} tokens vs {current_tokens} full \
                             ({win_pct}% saved).\n\n{text}\n\
                             (Full current slice: `tokenade expand-ref {hash} --raw`; prior bytes: \
                             `tokenade expand-ref {prior_hash} --raw`.)"
                        );
                        let reason = tokenade_core::disclose::maybe_stash_with_threshold(
                            reason,
                            tokenade_core::disclose::SourceHint::File(std::path::PathBuf::from(
                                file_path,
                            )),
                            tokenade_core::disclose::read_threshold_tokens(),
                        );
                        let net =
                            emit_read_compaction(parsed, file_path, offset, limit, reason, false);
                        if let Some(net) = net {
                            gain_log::record("hook", "read_slice_diff", file_path, net);
                        }
                        register_and_tag(parsed, cwd, hash, sliced_current);
                        tokenade_core::debug_log::log(
                            "hookwrap-pretool",
                            "read_slice_diff_hit",
                            serde_json::json!({
                                "file": file_path,
                                "offset": offset,
                                "limit": limit,
                                "added": added,
                                "removed": removed,
                                "diff_tokens": diff_tokens,
                                "full_tokens": current_tokens,
                                "saved_pct": win_pct,
                                "elapsed_ms": started.elapsed().as_millis() as u64,
                            }),
                        );
                        return Some(0);
                    }
                }
            }
        }
    }
    None
}
