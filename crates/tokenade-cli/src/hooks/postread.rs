//! **What we do AFTER the tool has returned.**
//!
//! `Read`, `Grep`, `Glob`: the payload is already produced, the question is
//! no longer avoiding it but reducing it without lying about what was
//! delivered.
//!
//! Hence the two bounds declared here rather than guessed: the line cap that
//! `Read` applies without `limit`, which bounds what a read COULD have put in
//! context, and the token cap under which a source file from a git repository
//! is served raw. An overestimated bound would claim lines that were never
//! delivered.

use super::*;

/// Claude Code's `Read` delivers at most this many lines when no explicit
/// `limit` is given (documented default). Used ONLY to bound what a
/// `limit=0` read could have placed in context — never an over-estimate, so
/// range-coverage can't claim undelivered lines.
pub(crate) const CLAUDE_READ_DEFAULT_LINE_CAP: u64 = 2000;

/// `Glob` PostToolUse: drop vendored/build paths and say what was dropped.
///
/// Replaces the result via `updatedToolOutput`, the only channel that removes
/// the raw listing rather than sitting next to it. Claude Code < 2.1.121
/// ignores the field, so an old host simply keeps its full listing — fail-open
/// by protocol, same posture as the WebSearch hook.
///
/// Two `tool_response` shapes are handled because the host has used both: a
/// bare newline-separated string, and an object carrying `filenames`. Anything
/// else prints nothing and the listing rides through untouched: a file listing
/// we do not fully understand is the last place to start guessing.
pub(crate) fn handle_glob_posttool(parsed: &serde_json::Value, started: std::time::Instant) -> i32 {
    if tokenade_core::env_compat::is_truthy_one("TOKENADE_NO_GLOB") {
        return 0;
    }
    let pattern = parsed
        .pointer("/tool_input/pattern")
        .and_then(|p| p.as_str())
        .unwrap_or("");
    let Some(resp) = parsed
        .get("tool_response")
        .or_else(|| parsed.get("toolResponse"))
    else {
        return 0;
    };

    let mut updated = resp.clone();
    let (before, after, footer) = match &mut updated {
        serde_json::Value::String(s) => {
            let Some(folded) = tokenade_core::glob_filter::fold_text(s, pattern) else {
                return 0;
            };
            // SECURITY: the same scrub every other served channel applies.
            // `websearch_post` states the rule — "same scrub as every other
            // served channel" — and this one was the exception: a Glob result
            // is a list of PATHS, and a path carries whatever the filesystem
            // was told to call it. Measured on the published 1.1.8 and on this
            // build, a path segment shaped like an AWS key reached the model
            // verbatim through this hook while `wrap`, `read`, `filter`,
            // `exec`, `web`, `expand-ref` (raw included) and both other hooks
            // all scrubbed it.
            let folded = tokenade_core::redact::redact_for_model(&folded);
            let b = s.len();
            let a = folded.len();
            // The footer is already a line of this text — `fold_text` appends
            // it — so it reaches the model with the listing and must not be
            // sent a second time.
            *s = folded;
            (b, a, String::new())
        }
        serde_json::Value::Object(obj) => {
            let Some(list) = obj.get("filenames").and_then(|f| f.as_array()) else {
                return 0;
            };
            let paths: Vec<String> = list
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            if paths.len() != list.len() || paths.is_empty() {
                return 0; // not the shape we think it is
            }
            let f = tokenade_core::glob_filter::fold(&paths, pattern);
            if f.dropped.is_empty() {
                return 0;
            }
            // SECURITY: scrub each path we are about to serve, and the footer
            // built from them. See the note on the string branch above.
            let kept_redacted: Vec<String> = f
                .kept
                .iter()
                .map(|p| tokenade_core::redact::redact_for_model(p))
                .collect();
            let b = paths.len();
            let a = f.kept.len();
            obj.insert(
                "filenames".to_string(),
                serde_json::Value::Array(
                    kept_redacted
                        .iter()
                        .map(|p| serde_json::Value::String(p.clone()))
                        .collect(),
                ),
            );
            // `numFiles` (when present) must agree with the list we just
            // rewrote, or the agent reads a count it cannot reconcile.
            if obj.contains_key("numFiles") {
                obj.insert("numFiles".to_string(), serde_json::json!(f.kept.len()));
            }
            (
                b,
                a,
                tokenade_core::redact::redact_for_model(&tokenade_core::glob_filter::footer(&f)),
            )
        }
        _ => return 0,
    };
    // The footer travels as `additionalContext`, NOT as a field of the
    // replacement. It used to be `updatedToolOutput.tokenadeGlobFooter`, and
    // the host never showed it: measured on the shipped Claude Code 2.1.263,
    // a Glob result reaches the model as
    // `content: [...e.filenames, ...(e.truncated?[note]:[])].join("\n")` —
    // `filenames` and nothing else. So the object shape dropped 60 of 66 paths
    // and rewrote `numFiles` to 6 to agree with the short list, leaving the
    // agent a shortened listing, a corroborating count, and no statement that
    // anything had been hidden. A fold that does not announce itself is the one
    // thing this product does not do. The string shape carried its footer
    // inside the text and did announce, so the same command told the truth or
    // not depending on which response shape the host handed us.
    //
    // `additionalContext` is part of the PostToolUse output schema and is
    // honoured by every host, where an invented field of the replacement is
    // honoured by none — the same conclusion `token-optimizer` reached for its
    // nudges on 2026-09-06. Emitted in the SAME envelope, and never duplicated
    // inside `updatedToolOutput`.
    let mut hook_out = serde_json::json!({
        "hookEventName": "PostToolUse",
        "updatedToolOutput": updated,
    });
    if !footer.is_empty() {
        hook_out["additionalContext"] = serde_json::Value::String(footer.clone());
        crate::log_emitted_additional_context("glob_fold", "posttool", &footer);
    }
    println!("{}", serde_json::json!({ "hookSpecificOutput": hook_out }));
    tokenade_core::debug_log::log(
        "hookwrap-postread",
        "glob_fold",
        serde_json::json!({
            "before": before,
            "after": after,
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
    0
}

pub(crate) fn cmd_hookwrap_postread() -> i32 {
    let started = std::time::Instant::now();
    let mut input = String::new();
    if crate::bounded_input::read_stdin_lossy(&mut input).is_err() {
        return 0;
    }
    let Some(parsed) = crate::hooks::stdin::parse_event(&input) else {
        return 0;
    };
    let tool = parsed
        .get("tool_name")
        .and_then(|s| s.as_str())
        .unwrap_or("");
    if tool_is(tool, "Glob") {
        return handle_glob_posttool(&parsed, started);
    }
    // Read AND Grep share this delivery: PreToolUse staged a compacted view
    // (keyed by file_path for Read, by the grep signature for Grep) and capped
    // the native tool result; here we deliver the staged view as
    // additionalContext. `file_path` doubles as the staging key + the steer
    // label (the grep pattern for Grep).
    let ti = parsed.get("tool_input");
    // `tool_is`, not a bare `match` on the string. The PreToolUse half decides
    // with `tool_is` — case-INSENSITIVE — so it staged a compacted view, and
    // shrank the caller's Read to a head slice (`updatedInput { limit: 20 }`),
    // for any spelling of the name. This half compared the raw string, so on
    // `read` or `READ` it fell through and delivered NOTHING. Measured on a
    // 4 000-line file: `Read` gets 20 lines plus 1 050 bytes of compacted
    // content; `read` gets 20 lines and nothing else — while the permission
    // reason says, in both cases and word for word, "compacted content follows
    // as additionalContext".
    //
    // That is the one failure this product must not have: a fold that names a
    // way back, and then does not deliver it. The two halves of one protocol
    // decide with ONE spelling now.
    let (key_input, file_path): (String, String) = if tool_is(tool, "Read") {
        {
            let fp = ti
                .and_then(|i| i.get("file_path"))
                .and_then(|p| p.as_str())
                .unwrap_or("")
                .to_string();
            (fp.clone(), fp)
        }
    } else if tool_is(tool, "Grep") {
        {
            let Some(ti) = ti else { return 0 };
            let label = format!(
                "Grep:{}",
                ti.get("pattern").and_then(|v| v.as_str()).unwrap_or("")
            );
            (grep_pending_sig(ti), label)
        }
    } else {
        return 0;
    };
    if key_input.is_empty() {
        return 0;
    }
    let session_id = crate::hooks::session_id_of(&parsed).unwrap_or("");
    let (p_off, p_lim) = if tool_is(tool, "Read") {
        (
            ti.and_then(|i| i.get("offset"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            ti.and_then(|i| i.get("limit"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
        )
    } else {
        (0, 0)
    };
    let key = read_pending_key(session_id, &key_input, p_off, p_lim);
    let pending_path = read_pending_dir().join(format!("{key}.json"));
    let Ok(bytes) = std::fs::read_to_string(&pending_path) else {
        return 0;
    };
    let _ = std::fs::remove_file(&pending_path);
    let Ok(record) = serde_json::from_str::<serde_json::Value>(&bytes) else {
        return 0;
    };
    let ts = record.get("ts").and_then(|n| n.as_u64()).unwrap_or(0);
    let now = tokenade_core::platform::now_unix_secs();
    if now.saturating_sub(ts) > READ_PENDING_TTL_SECS {
        return 0;
    }
    let context = record.get("context").and_then(|s| s.as_str()).unwrap_or("");
    if context.is_empty() {
        return 0;
    }
    // S3 — Verify the invariant before injecting. The staged compaction is
    // ADDITIVE: it only pays off if the native tool_result was actually
    // bounded by our PreToolUse updatedInput (a head slice of <=20 lines).
    // If the agent received a large native output anyway — updatedInput not
    // applied, a format rendered natively (PDF in vision), or any future leak
    // we haven't imagined — injecting our stash on top is pure inflation.
    // Measure the REAL tool_response and skip injection when it is already
    // large. Fail-open: if the field is absent we inject as before.
    let resp_str = match parsed.get("tool_response") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
        None => String::new(),
    };
    let expected_head = parsed
        .get("tool_input")
        .and_then(|i| i.get("limit"))
        .and_then(|l| l.as_u64())
        .unwrap_or(READ_HEAD_LINES)
        .max(1) as usize;
    if native_read_breaks_invariant(&resp_str, expected_head) {
        tokenade_core::debug_log::log(
            "hookwrap-postread",
            "invariant_broken_skip_injection",
            serde_json::json!({
                "file": file_path,
                "native_lines": resp_str.lines().count(),
                "native_tokens": tokenade_core::tokenizer::estimate(&resp_str),
                "expected_head": expected_head,
            }),
        );
        return 0;
    }
    let delivered = cap_postread_additional_context(context, &file_path);
    let decision = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "additionalContext": delivered,
        }
    });
    log_emitted_additional_context("hookwrap-postread", "PostToolUse", &delivered);
    println!("{decision}");
    tokenade_core::debug_log::log(
        "hookwrap-postread",
        "delivered",
        serde_json::json!({
            "file": file_path,
            "context_bytes": delivered.len(),
            "staged_bytes": context.len(),
            "capped": context.len() > POSTREAD_INLINE_CAP_BYTES,
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
    0
}

/// P1c (flow analysis 2026-06) — editable-source guard support. Walks
/// up from the file looking for a `.git` entry (dir for work trees,
/// file for worktrees/submodules). Capped to keep the hook O(depth).
pub(crate) fn inside_git_work_tree(path: &std::path::Path) -> bool {
    let mut dir = path.parent();
    for _ in 0..64 {
        let Some(d) = dir else { return false };
        if d.join(".git").exists() {
            return true;
        }
        dir = d.parent();
    }
    false
}

/// Token ceiling under which a source file inside a git work tree is
/// served raw (see `editable_source_guard` in `handle_read_pretool`).
pub(crate) const EDITABLE_SOURCE_MAX_TOKENS: usize = 20_000;
