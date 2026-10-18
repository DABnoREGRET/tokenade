//! **After an edit, tell the agent what changed.**
//!
//! The `PostToolUse` hook of `Edit`/`Write` goes through here: read the
//! edited path, decide whether it is in the current project or in ANOTHER one
//! (in which case it is the other project's index that must be touched),
//! compose the hint that points to `tokenade diff`, and stay silent on every
//! failure path — missing stdin, wrong tool, no prior state cached, file too
//! small for the hint to be worth its noise.
//!
//! Moved out of `main.rs` on 2026-09-13: six functions, a single topic, and
//! the binary's root has no business carrying it. `main.rs` is the SWITCHYARD.

use crate::hooks;
use crate::index;
use tokenade_core::semantic;

/// #694 — drives the PostToolUse Edit/Write hint pointing at
/// `tokenade diff`. Returns true when a hint was emitted
/// (logged for telemetry), false otherwise. Silent on every failure
/// path: missing stdin, wrong tool name, no cached prior, file too
/// small to be worth the hint — agents shouldn't see noise.
pub(crate) fn emit_edit_diff_hint_from_stdin(
    root: &std::path::Path,
    stdin_json: Option<&str>,
) -> bool {
    let Some(json) = stdin_json else { return false };
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json) else {
        return false;
    };
    let tool = parsed
        .get("tool_name")
        .and_then(|s| s.as_str())
        .unwrap_or("");
    // Only the file-mutating tools — Edit, Write, MultiEdit. NotebookEdit
    // is a different beast (cell-based, not whole-file diff) so we skip.
    if !matches!(tool, "Edit" | "Write" | "MultiEdit") {
        return false;
    }
    let file_path = parsed
        .get("tool_input")
        .and_then(|i| i.get("file_path"))
        .and_then(|p| p.as_str())
        .unwrap_or("");
    if file_path.is_empty() {
        return false;
    }
    // Throttle (overhead audit: 616 emits / 266 KB in ONE session — the
    // top un-throttled context tax). The hint is a standing pointer to
    // `tokenade diff`; twice per (session,file) delivers the steer, the
    // 3rd+ repeat is pure spam. Editing a DIFFERENT file resets nothing —
    // each file gets its own budget.
    let skey: String = hooks::session_id_of(&parsed)
        .unwrap_or("nosess")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(40)
        .collect();
    let throttle = hooks::read_staging::read_pending_dir().join(format!(
        "edithint-{skey}-{}",
        tokenade_core::dedup_ref::hash_content(file_path.as_bytes())
    ));
    let sent: u32 = std::fs::read_to_string(&throttle)
        .ok()
        .and_then(|t| t.trim().parse().ok())
        .unwrap_or(0);
    if sent >= 2 {
        return false;
    }
    let Some(hint) = compose_edit_diff_hint(root, file_path) else {
        return false;
    };
    tokenade_core::platform::ensure_parent_dir(&throttle);
    let _ = std::fs::write(&throttle, (sent + 1).to_string());
    let decision = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "additionalContext": hint,
        }
    });
    crate::log_emitted_additional_context("hookwrap-postedit", "PostToolUse", &hint);
    println!("{decision}");
    true
}

/// #712 — extract `tool_input.file_path` from the PostToolUse stdin
/// JSON. Returns None when stdin is empty / unparseable / the field is
/// missing — every failure mode is silent so the hook never crashes.
/// Used by cmd_index_touch's outside-root early-exit (which mirrors
/// the same parse done inside `emit_edit_diff_hint_from_stdin`).
pub(crate) fn edited_file_path(stdin_json: Option<&str>) -> Option<String> {
    let json = stdin_json?;
    let parsed: serde_json::Value = serde_json::from_str(json).ok()?;
    let tool = parsed
        .get("tool_name")
        .and_then(|s| s.as_str())
        .unwrap_or("");
    if !matches!(tool, "Edit" | "Write" | "MultiEdit") {
        return None;
    }
    let fp = parsed
        .get("tool_input")
        .and_then(|i| i.get("file_path"))
        .and_then(|p| p.as_str())?;
    if fp.is_empty() {
        return None;
    }
    Some(fp.to_string())
}

/// #712 — true when `file_path` resolves to a location under `root`.
/// Uses canonicalize so symlinks pointing in/out are honoured. Falls
/// back to a textual prefix match when canonicalization fails (e.g.
/// the file was just deleted between the Edit and this hook — rare).
pub(crate) fn path_within_root(root: &std::path::Path, file_path: &str) -> bool {
    let p = std::path::PathBuf::from(file_path);
    match (std::fs::canonicalize(root), std::fs::canonicalize(&p)) {
        (Ok(rc), Ok(pc)) => pc.starts_with(&rc),
        _ => {
            // Best-effort textual fallback. Path::starts_with checks
            // component-wise so trailing slashes / `.` don't trick it.
            p.starts_with(root)
        }
    }
}

/// When a PostToolUse Edit/Write fires for a file OUTSIDE the current project
/// root, look up `~/.tokenade/projects-registry.txt` to find the registered
/// project that owns the file and refresh its index instead. Returns true when
/// an index was successfully touched.
pub(crate) fn touch_cross_project_index(file_path: &str, started: std::time::Instant) -> bool {
    let home = match tokenade_core::platform::home_dir() {
        Some(h) => h,
        None => return false,
    };
    let registry_path = home.join(".tokenade").join("projects-registry.txt");
    let contents = match std::fs::read_to_string(&registry_path) {
        Ok(c) => c,
        Err(_) => return false,
    };

    // Find the longest registered root that owns file_path.
    let file_pb = std::path::PathBuf::from(file_path);
    let best_root: Option<std::path::PathBuf> = contents
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter_map(|line| {
            let root = std::path::PathBuf::from(line);
            if path_within_root(&root, file_path) {
                Some(root)
            } else {
                None
            }
        })
        .max_by_key(|p| p.as_os_str().len());

    let cross_root = match best_root {
        Some(r) => r,
        None => return false,
    };

    // Only proceed if the root has a .tokenade/ dir (it's an active project).
    if !cross_root.join(".tokenade").is_dir() {
        return false;
    }

    let _ = file_pb; // used above in path_within_root
    let idx = match index::Index::ensure_fresh(&cross_root) {
        Ok(i) => i,
        Err(e) => {
            tokenade_core::debug_log::log(
                "index-touch",
                "cross_project_index_err",
                serde_json::json!({
                    "cross_root": cross_root.display().to_string(),
                    "file_path": file_path,
                    "error": e.to_string(),
                }),
            );
            return false;
        }
    };
    let sem = match semantic::SemanticIndex::load(&cross_root) {
        Some(prev) if !prev.mtimes.is_empty() => {
            semantic::SemanticIndex::rebuild_incremental(&prev, &idx, &cross_root)
        }
        _ => semantic::SemanticIndex::build_from_index(&idx, &cross_root),
    };
    let _ = sem.save(&cross_root);

    tokenade_core::debug_log::log(
        "index-touch",
        "cross_project_indexed",
        serde_json::json!({
            "cross_root": cross_root.display().to_string(),
            "file_path": file_path,
            "files_indexed": idx.files_indexed,
            "symbols": idx.symbols.len(),
            "semantic_docs": sem.docs.len(),
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
    true
}

/// Build the file_diff hint string when the conditions are met:
///   - file_path exists and is a regular file ≥ MIN_BYTES_TO_DEDUPE,
///   - tokenade has a path-pointer for it (= agent Read it before).
///
/// Returns None otherwise. Pure-ish (only fs::metadata + dedup_ref
/// lookup); side-effect-free, easy to unit-test.
pub(crate) fn compose_edit_diff_hint(root: &std::path::Path, file_path: &str) -> Option<String> {
    let path = std::path::PathBuf::from(file_path);
    let meta = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(_) => {
            // #715 — file missing right after the Edit completed (rare:
            // agent deleted via Bash in same turn, or wrong cwd). Log so
            // a future bug isn't invisible.
            tokenade_core::debug_log::log(
                "edit-hint",
                "skipped_file_missing",
                serde_json::json!({"file_path": file_path}),
            );
            return None;
        }
    };
    if !meta.is_file() {
        tokenade_core::debug_log::log(
            "edit-hint",
            "skipped_not_regular_file",
            serde_json::json!({"file_path": file_path}),
        );
        return None;
    }
    let size = meta.len();
    if size < tokenade_core::dedup_ref::MIN_BYTES_TO_DEDUPE as u64 {
        // Tiny file — `Read` cost is already small. The hint's overhead
        // (~50 tokens) wouldn't pay back here.
        tokenade_core::debug_log::log(
            "edit-hint",
            "skipped_too_small",
            serde_json::json!({"file_path": file_path, "size": size}),
        );
        return None;
    }
    // path_key(file, 0, 0) = whole-file slice. That's what an
    // Edit-then-verify Read targets by default.
    let path_key = tokenade_core::dedup_ref::path_key(file_path, 0, 0);
    let (_prior_hash, _prior_content) =
        match tokenade_core::dedup_ref::lookup_by_path(root, &path_key) {
            Some(v) => v,
            None => {
                // #715/#717 — most common silent-return cause: dedup
                // snapshot missing. Means either (a) the agent never
                // Read this file in this project, (b) they Read with
                // non-default offset/limit so the path_key differs from
                // (0,0), or (c) the snapshot was evicted (7-day TTL).
                // #717 — include the actual path_key hash so it can be
                // grep'd directly against `read_dedup_miss_registered`
                // events. A future fix will likely either probe
                // multiple candidate keys or canonicalise registration.
                tokenade_core::debug_log::log(
                    "edit-hint",
                    "skipped_no_snapshot",
                    serde_json::json!({
                        "file_path": file_path,
                        "size": size,
                        "looked_up_path_key": path_key,
                        "looked_up_offset": 0,
                        "looked_up_limit": 0,
                    }),
                );
                return None;
            }
        };
    // Estimate the Read cost the agent would otherwise pay so the
    // hint is concrete. Cheap: estimate against the on-disk file we
    // just stat'd.
    let token_estimate = (size / 4).max(1); // ~4 bytes/token rough
                                            // Keep this LEAN: it is injected as additionalContext on EVERY qualifying
                                            // edit (field data: 616 emits / 266 KB in one session). The actionable
                                            // payload is "edited X; verify with a compacted diff instead of re-Reading"
                                            // — the long explanation of what the diff returns was repeated every time
                                            // for no added signal. Trimmed to ~half the bytes, same nudge.
                                            // MEASURED 2026-08-04 over the 737 hints this install actually emitted:
                                            // 354 named a file UNTRACKED in its repo and 22 a file in no repo at all,
                                            // so `git diff` answered with nothing or with "not a git repository" on
                                            // **51 %** of them. The agent then re-Reads anyway — the exact cost the
                                            // hint exists to avoid. (The generation before this one named `tokenade
                                            // diff`, a command that has since been removed; the CLI answers it with
                                            // "'diff' is a FORMAT, not a command". Two texts, both partly wrong.)
                                            //
                                            // What is always true is the thing we already checked to get here: a
                                            // whole-file snapshot EXISTS for this path. That is the same precondition
                                            // `read_diffs::try_exact_slice_diff` needs, so a plain Read of this file
                                            // comes back as a unified diff against the pre-edit copy — no git, no
                                            // tracked/untracked distinction, nothing to install. Point at that.
                                            //
                                            // Stated as a preference, not a promise: the read path keeps the diff only
                                            // when it saves at least 40 % over the full text, and hands back the whole
                                            // file when an edit was pervasive enough that the diff would not be
                                            // smaller. Both outcomes are correct; only the first is cheaper.
    Some(edit_hint_text(file_path, token_estimate))
}

/// The hint's wording, split out so it can be tested without a dedup database:
/// the defect this pins was in the TEXT, not in the lookup that precedes it.
pub(crate) fn edit_hint_text(file_path: &str, token_estimate: u64) -> String {
    format!(
        "[tokenade:edit-hint] edited {file_path} (~{token_estimate} tok to re-Read in full). \
         tokenade still holds the pre-edit copy, so Read it again to see your own change: \
         you get a diff against that copy rather than the whole file."
    )
}
