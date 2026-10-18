//! PreToolUse handler for `Write` — the OUTPUT-side lever.
//!
//! Output tokens are ~5x the price of input and (measured on real traces)
//! 4-6x the volume, yet nothing steered them: an agent that re-emits a whole
//! 800-line file through `Write` to change 12 lines bills the other 788 lines
//! of output for nothing. This handler detects that shape — the target file
//! exists and the new content is mostly identical to it — and injects a
//! once-per-(session,file) additive nudge to use targeted `Edit` calls (or
//! one `tokenade exec` script for mechanical bulk rewrites) instead.
//!
//! It never blocks or rewrites the Write itself: additionalContext only, so
//! the worst case is a ~70-token steer. Disable with
//! `TOKENADE_NO_WRITE_NUDGE=1` (or the global TOKENADE_HOOK_DISABLED).

use super::{hook_disabled_by_env, log_emitted_additional_context};

/// Similarity threshold (percent of the EXISTING file's lines that reappear
/// verbatim in the new content) above which the Write is called a rewrite.
const REWRITE_IDENTICAL_PCT: usize = 80;
/// Don't bother below this many existing lines — regenerating a small file
/// costs little and Edit-per-hunk could plausibly be more output, not less.
const MIN_LINES: usize = 60;
/// Cap the comparison work (pathological megabyte writes).
const MAX_BYTES: u64 = 2 * 1024 * 1024;

pub(crate) fn handle_write_pretool(parsed: &serde_json::Value, started: std::time::Instant) -> i32 {
    if hook_disabled_by_env() || std::env::var("TOKENADE_NO_WRITE_NUDGE").is_ok() {
        return 0;
    }
    let file_path = parsed
        .pointer("/tool_input/file_path")
        .and_then(|p| p.as_str())
        .unwrap_or("");
    let new_content = parsed
        .pointer("/tool_input/content")
        .and_then(|c| c.as_str())
        .unwrap_or("");
    if file_path.is_empty() || new_content.is_empty() {
        return 0;
    }
    let Ok(meta) = std::fs::metadata(file_path) else {
        return 0; // new file — a full Write is the right tool
    };
    if meta.len() > MAX_BYTES {
        return 0;
    }
    let Ok(existing) = std::fs::read_to_string(file_path) else {
        return 0; // binary / unreadable — stay out of the way
    };
    let old_lines: Vec<&str> = existing.lines().collect();
    if old_lines.len() < MIN_LINES {
        return 0;
    }

    // Multiset membership: what fraction of the existing lines survive
    // verbatim in the new content? (Order-insensitive on purpose — moved
    // blocks still count as "not worth re-emitting".) Blank lines excluded
    // so they don't inflate similarity.
    let mut new_count: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for l in new_content.lines() {
        if !l.trim().is_empty() {
            *new_count.entry(l).or_insert(0) += 1;
        }
    }
    let mut kept = 0usize;
    let mut total = 0usize;
    for l in &old_lines {
        if l.trim().is_empty() {
            continue;
        }
        total += 1;
        if let Some(n) = new_count.get_mut(l) {
            if *n > 0 {
                *n -= 1;
                kept += 1;
            }
        }
    }
    if total == 0 {
        return 0;
    }
    let identical_pct = kept * 100 / total;
    let changed = total.saturating_sub(kept);
    tokenade_core::debug_log::log(
        "hookwrap-pretool",
        "write_rewrite_scan",
        serde_json::json!({
            "file": file_path,
            "old_lines": total,
            "identical_pct": identical_pct,
            "changed_lines": changed,
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
    if identical_pct < REWRITE_IDENTICAL_PCT {
        return 0;
    }

    // ECONOMIC gate (bench catch: pipeline-py +22%, +4 turns/run): heeding
    // the nudge costs one Edit round-trip PER HUNK (~800 input tokens each
    // re-billed context), while the win is the identical chars NOT re-
    // emitted as output. Nudge only when the trade is clearly positive:
    // few contiguous changed regions AND a big identical volume. Scattered
    // small edits across a mid-size file are CHEAPER as one Write.
    let mut hunks = 0usize;
    let mut in_hunk = false;
    let mut old_set: std::collections::HashSet<&str> = Default::default();
    for l in &old_lines {
        if !l.trim().is_empty() {
            old_set.insert(*l);
        }
    }
    for l in new_content.lines() {
        if l.trim().is_empty() {
            continue;
        }
        if !old_set.contains(l) {
            if !in_hunk {
                hunks += 1;
                in_hunk = true;
            }
        } else {
            in_hunk = false;
        }
    }
    let identical_tokens_est: usize = old_lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len())
        .sum::<usize>()
        * identical_pct
        / 100
        / 4;
    // saved output ≈ identical_tokens_est (5x-priced) vs edit cost ≈
    // hunks × 800 input tokens (1x) + turn latency. Require 5*saved > 2×cost
    // AND a hunk cap (each hunk is a failure-prone round-trip).
    if hunks > 5 || identical_tokens_est * 5 < hunks * 800 * 2 {
        tokenade_core::debug_log::log(
            "hookwrap-pretool",
            "write_rewrite_skip_economics",
            serde_json::json!({"file": file_path, "hunks": hunks, "identical_tokens_est": identical_tokens_est}),
        );
        return 0;
    }

    // Once per (session, file): the point is a course correction, not a nag.
    let session = crate::hooks::session_id_of(parsed).unwrap_or("nosess");
    let skey: String = session
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(40)
        .collect();
    let marker = super::read_staging::read_pending_dir().join(format!(
        "writenudge-{skey}-{}",
        tokenade_core::dedup_ref::hash_content(file_path.as_bytes())
    ));
    if marker.exists() {
        return 0;
    }
    tokenade_core::platform::ensure_parent_dir(&marker);
    // Store the re-emission estimate (chars of identical lines): if the agent
    // heeds the nudge (next touch on this file is an Edit, seen by the
    // pre-edit-snapshot hook), that many output-chars were NOT re-generated —
    // credited there as an estimated output-class row. `0` = already credited.
    let identical_chars: usize = old_lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len())
        .sum::<usize>()
        * identical_pct
        / 100;
    let _ = std::fs::write(&marker, identical_chars.to_string());

    let nudge = format!(
        "[tokenade:output] This Write re-emits ~{identical_pct}% of `{file_path}` \
         unchanged ({changed} of {total} lines actually differ). Output tokens \
         are the most expensive kind — for existing files prefer targeted \
         `Edit` calls (one per hunk), or ONE `tokenade exec` script when the \
         change is mechanical across many sites. Let this Write proceed if \
         the file genuinely needs restructuring."
    );
    log_emitted_additional_context("hookwrap-pretool-writenudge", "PreToolUse", &nudge);
    tokenade_core::gain::record_nudge(
        "write-rewrite",
        "fire",
        tokenade_core::gain::NudgeMetric::Tokens(identical_chars / 4),
    );
    println!(
        "{}",
        serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "additionalContext": nudge,
            }
        })
    );
    tokenade_core::debug_log::log(
        "hookwrap-pretool",
        "write_rewrite_nudge",
        serde_json::json!({"file": file_path, "identical_pct": identical_pct}),
    );
    0
}

#[cfg(test)]
mod tests {
    #[test]
    fn similarity_math_counts_multiset() {
        // 10 identical lines minus 2 changed → 80%
        let old: Vec<String> = (0..10).map(|i| format!("line {i}")).collect();
        let mut new = old.clone();
        new[3] = "CHANGED".into();
        new[7] = "ALSO CHANGED".into();
        let newc = new.join("\n");
        let mut new_count: std::collections::HashMap<&str, usize> = Default::default();
        for l in newc.lines() {
            *new_count.entry(l).or_insert(0) += 1;
        }
        let kept = old
            .iter()
            .filter(|l| {
                new_count.get_mut(l.as_str()).map(|n| {
                    if *n > 0 {
                        *n -= 1;
                        true
                    } else {
                        false
                    }
                }) == Some(true)
            })
            .count();
        assert_eq!(kept * 100 / old.len(), 80);
    }
}
