#![allow(unused_imports)]
//! Split out of the monolithic `hooks.rs` (mechanical module split, no
//! behaviour change). Wired back into the `hooks` namespace by mod.rs via
//! `pub(crate) use <name>::*;`.
use super::*;
use crate::*;

/// Below this many tokens a Grep result isn't worth compacting — the
/// head-anchor + injected-compaction overhead would eat the win. One measured
/// user's median grep was ~50 tok; the value lives in the ~13 greps ≥300 tok.
pub(crate) const GREP_MIN_TOKENS: usize = 300;

/// How many matches the native Grep is capped to (via `head_limit`) once we
/// decide to compact — a tiny anchor so the raw tool_result is cache-stable
/// but does NOT duplicate the full result. THIS is the fix for the abandoned
/// first attempt: previously the raw Grep AND the compacted view both sat in
/// context (additive → double cost). Bounding the native result to an anchor
/// makes raw+compacted ≈ compacted.
pub(crate) const GREP_HEAD_ANCHOR: u64 = 3;

/// Grep/Glob output-compaction opt-out.
pub(crate) fn grep_compact_disabled() -> bool {
    tokenade_core::env_compat::is_truthy_one("TOKENADE_NO_GREP")
}

/// A staging key for a Grep call, reproducible from the same `tool_input` in
/// BOTH the PreToolUse (stage) and PostToolUse (deliver) hooks. Encodes the
/// params that determine the result set.
pub(crate) fn grep_pending_sig(ti: &serde_json::Value) -> String {
    let g = |k: &str| ti.get(k).and_then(|v| v.as_str()).unwrap_or("");
    // Flags in the key: two greps differing only in -i/-n/context used to
    // collide and pop each other's staged view (wrong-slice delivery).
    let b = |k: &str| ti.get(k).and_then(|v| v.as_bool()).unwrap_or(false) as u8;
    let n = |k: &str| ti.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
    format!(
        "grep::{}::{}::{}::{}::i{}n{}m{}A{}B{}C{}",
        g("pattern"),
        g("path"),
        g("glob"),
        g("type"),
        b("-i"),
        b("-n"),
        b("multiline"),
        n("-A"),
        n("-B"),
        n("-C")
    )
}

/// Reconstruct a ripgrep argv from Claude Code's `Grep` tool_input
/// (`output_mode == "content"` only). Returns `None` on any param we don't
/// faithfully support, so the caller passes the Grep through untouched rather
/// than risk serving a result that differs from Claude Code's own ripgrep.
pub(crate) fn build_rg_argv(ti: &serde_json::Value) -> Option<Vec<String>> {
    let pattern = ti
        .get("pattern")
        .and_then(|v| v.as_str())
        .filter(|p| !p.is_empty())?;
    // Match Claude Code's content-mode rendering: filenames on, colour off.
    let mut argv = vec!["--color=never".to_string(), "--with-filename".to_string()];
    if ti.get("-n").and_then(|v| v.as_bool()).unwrap_or(false) {
        argv.push("--line-number".into());
    }
    if ti.get("-i").and_then(|v| v.as_bool()).unwrap_or(false) {
        argv.push("--ignore-case".into());
    }
    if ti
        .get("multiline")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        argv.push("--multiline".into());
        argv.push("--multiline-dotall".into());
    }
    for (k, flag) in [
        ("-A", "--after-context"),
        ("-B", "--before-context"),
        ("-C", "--context"),
    ] {
        if let Some(n) = ti.get(k).and_then(|v| v.as_u64()) {
            argv.push(flag.into());
            argv.push(n.to_string());
        }
    }
    if let Some(glob) = ti
        .get("glob")
        .and_then(|v| v.as_str())
        .filter(|g| !g.is_empty())
    {
        argv.push("--glob".into());
        argv.push(glob.into());
    }
    if let Some(ty) = ti
        .get("type")
        .and_then(|v| v.as_str())
        .filter(|t| !t.is_empty())
    {
        argv.push("--type".into());
        argv.push(ty.into());
    }
    argv.push("--".into());
    argv.push(pattern.to_string());
    if let Some(path) = ti
        .get("path")
        .and_then(|v| v.as_str())
        .filter(|p| !p.is_empty())
    {
        argv.push(path.to_string());
    }
    Some(argv)
}

/// PreToolUse handler for Claude Code's `Grep` tool. Compacts the (verbose)
/// `content`-mode output the SAME way the Read staging does, fixing the
/// additive double-cost that shelved the first attempt:
///   1. run the identical ripgrep query ourselves to get the full result,
///   2. fold it losslessly (`searchout::compact`),
///   3. cap the NATIVE Grep to a 3-match anchor via `updatedInput.head_limit`
///      (so the raw tool_result is tiny — no longer a full-size duplicate),
///   4. stage the compacted full view; the PostToolUse hook delivers it as
///      `additionalContext`, exactly like Read.
/// Net in context: tiny anchor + compacted full ≪ raw full. Gated
/// `TOKENADE_NO_GREP`; passes through on any unsupported param / missing rg /
/// small result / net loss.
pub(crate) fn handle_grep_pretool(parsed: &serde_json::Value, started: std::time::Instant) -> i32 {
    if hook_disabled_by_env() || grep_compact_disabled() {
        return 0;
    }
    let Some(ti) = parsed.get("tool_input") else {
        return 0;
    };
    // Our own `rg` walks the whole tree and its matches reach the agent: a
    // file a `Read(…)` deny/ask rule covers could be quoted. Any such rule ⇒
    // the native Grep alone, under Claude Code's own rules.
    let cwd = parsed
        .get("cwd")
        .and_then(|c| c.as_str())
        .map(std::path::Path::new);
    if crate::hooks::permission_rules::any_user_rule_for("Read", cwd) {
        return 0;
    }
    // Only the verbose content mode is worth it; files_with_matches / count
    // are already terse. And never fight an agent that already bounded it.
    // Claude Code's Grep DEFAULT is files_with_matches — treating a missing
    // output_mode as "content" re-ran rg in content mode and injected a big
    // view the agent never asked for (while clamping its file list).
    if ti
        .get("output_mode")
        .and_then(|v| v.as_str())
        .unwrap_or("files_with_matches")
        != "content"
    {
        return 0;
    }
    if ti.get("head_limit").is_some() {
        return 0;
    }
    let session_id = crate::hooks::session_id_of(parsed).unwrap_or("");
    if session_id.is_empty() {
        return 0;
    }
    let Some(argv) = build_rg_argv(ti) else {
        return 0;
    };
    // Run the SAME ripgrep query in the agent's cwd. If rg isn't found or the
    // run errors (exit 2) or finds nothing (exit 1), pass through untouched.
    let mut cmd = tokenade_core::process_util::internal_command("rg");
    cmd.args(&argv);
    if let Some(cwd) = parsed.get("cwd").and_then(|v| v.as_str()) {
        cmd.current_dir(cwd);
    }
    tokenade_core::process_util::configure_no_window(&mut cmd);
    let Ok(out) = cmd.output() else { return 0 };
    if out.status.code() != Some(0) {
        return 0;
    }
    let raw = String::from_utf8_lossy(&out.stdout);
    let raw_tokens = tokenade_core::tokenizer::estimate(&raw);
    if raw_tokens < GREP_MIN_TOKENS {
        return 0;
    }
    let (compacted, _sav) = tokenade_core::searchout::compact_with_savings(&raw);
    let match_lines = raw.lines().count();
    // searchout caps at 50 files × 10 matches/file and ELIDES the rest. So the
    // elided matches are never lost: stash the full ripgrep output under a
    // content hash and point the agent at `expand-ref` to recover them. This
    // keeps the lever lossless-by-recovery (tokenade's invariant) rather than a
    // silent truncation.
    let cwd_for_stash = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let raw_hash = tokenade_core::dedup_ref::hash_content(raw.as_bytes());
    tokenade_core::dedup_ref::register(&cwd_for_stash, &raw_hash, &raw);
    let reason = format!(
        "[tokenade:grep-compact] {match_lines} match line(s), folded by file \
         ({raw_tokens}→{ct} tokens). tokenade ran the identical ripgrep query; \
         matches past 50 files / 10-per-file are elided HERE but fully \
         recoverable: `tokenade expand-ref {raw_hash}` (or re-run with \
         TOKENADE_NO_GREP=1).\n\n{compacted}",
        ct = tokenade_core::tokenizer::estimate(&compacted),
    );
    let reason_tokens = tokenade_core::tokenizer::estimate(&reason);
    // Net guard: native anchor (≈first GREP_HEAD_ANCHOR lines) + injected
    // compaction must beat the raw full result, else pass through.
    let anchor_tokens = tokenade_core::tokenizer::estimate(
        &raw.lines()
            .take(GREP_HEAD_ANCHOR as usize)
            .collect::<Vec<_>>()
            .join("\n"),
    );
    if reason_tokens + anchor_tokens >= raw_tokens {
        return 0;
    }
    let sig = grep_pending_sig(ti);
    if !stage_read_pending(session_id, &sig, 0, 0, &reason) {
        return 0;
    }
    // Cap the native Grep to a tiny anchor (merge, don't reconstruct).
    let mut updated = ti.clone();
    updated["head_limit"] = serde_json::json!(GREP_HEAD_ANCHOR);
    let decision = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "permissionDecisionReason":
                "tokenade: Grep capped to a head anchor; full compacted matches follow as additionalContext",
            "updatedInput": updated,
        }
    });
    println!("{decision}");
    let net = tokenade_core::Savings::new(raw_tokens, reason_tokens + anchor_tokens);
    let label = format!(
        "Grep:{}",
        ti.get("pattern").and_then(|v| v.as_str()).unwrap_or("")
    );
    gain_log::record("hook", "grep_compact", &label, net);
    tokenade_core::debug_log::log(
        "hookwrap-pretool",
        "grep_compaction_staged",
        serde_json::json!({
            "pattern": ti.get("pattern").and_then(|v| v.as_str()).unwrap_or(""),
            "raw_tokens": raw_tokens,
            "after_tokens": reason_tokens + anchor_tokens,
            "match_lines": match_lines,
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
    0
}
