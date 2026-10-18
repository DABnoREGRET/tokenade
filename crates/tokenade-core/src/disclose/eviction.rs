//! Keeping the stash bounded: the size cap, the age sweep, and the
//! redaction re-scrub.
//!
//! The cap sacrifices replaceable entries before the last copy of anything —
//! mtime alone would delete a sole copy ahead of a blob ten times its size.

use super::*;

/// Resolve the active total-stash-dir cap. Reads the env var on each
/// call (no caching) so operators can flip the knob without restarting
/// a long-lived MCP server. Empty / unparseable values fall back to
/// the compile-time default — same forgiving semantics as
/// `threshold_tokens` (#1010).
pub fn stash_max_dir_bytes() -> u64 {
    match std::env::var("TOKENADE_STASH_MAX_DIR_BYTES") {
        Ok(s) => s
            .trim()
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .unwrap_or(STASH_MAX_DIR_BYTES),
        Err(_) => STASH_MAX_DIR_BYTES,
    }
}

pub fn evict_to_size_cap() -> std::io::Result<(usize, u64)> {
    evict_to_size_cap_inner(&stash_dir(), stash_max_dir_bytes())
}

/// Drop stashed files whose mtime is older than [`CACHE_TTL_SECS`].
/// Iterate every text file in the stash dir, re-apply
/// [`crate::redact::redact`], and rewrite the file in place when the
/// content changes. Skips binaries (heuristic: `detect_binary` on the
/// loaded bytes) so PNG / PDF / archive stashes are not touched.
/// Returns `(files_touched, bytes_freed)`.
///
/// Complements #475 (which redacts on WRITE) by cleaning up any
/// historical content written before #475 shipped. Pair with
/// [`crate::dedup_ref::scrub_via_redact`] for full storage cleanup.
pub fn scrub_via_redact() -> std::io::Result<(usize, u64)> {
    scrub_via_redact_inner(false)
}

/// #512 — dry-run mode: count files whose content would change if
/// `scrub_via_redact()` ran, but write nothing. `bytes_freed` is the
/// total bytes we'd reclaim. Powers `tokenade scrub-cache --dry-run`.
pub fn scrub_via_redact_dry_run() -> std::io::Result<(usize, u64)> {
    scrub_via_redact_inner(true)
}

pub(super) fn scrub_via_redact_inner(dry_run: bool) -> std::io::Result<(usize, u64)> {
    let dir = stash_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok((0, 0));
    };
    let mut touched = 0usize;
    let mut bytes_freed: u64 = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = path.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let original_len = meta.len();
        // Read as bytes first so we can skip binary stashes without
        // tripping a UTF-8 panic.
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if detect_binary(&bytes).is_some() {
            continue;
        }
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        let redacted = crate::redact::redact(&text);
        if redacted != text {
            // Dry-run accounts unconditionally (no I/O); apply mode
            // only counts after a successful write so partial-failure
            // reports stay honest.
            let do_count = dry_run || std::fs::write(&path, &redacted).is_ok();
            if do_count {
                touched += 1;
                bytes_freed += original_len.saturating_sub(redacted.len() as u64);
            }
        }
    }
    Ok((touched, bytes_freed))
}

/// Returns the number removed. Called opportunistically from
/// [`maybe_stash`], `tokenade install`, and `tokenade evict-stale`.
/// Most recent mtime among a directory's immediate entries. `None` when the
/// directory is empty or unreadable.
pub(super) fn newest_entry_mtime(dir: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| e.metadata().ok()?.modified().ok())
        .max()
}

/// Drop keep-markers whose stash entry is gone.
///
/// The size cap can still take a sole copy once nothing replaceable is left,
/// and a marker left behind would then protect a file that no longer exists —
/// tiny, but unbounded over months, which is the shape of every disk leak this
/// module has already had to fix once.
pub(super) fn sweep_orphan_markers() {
    let keep = keep_marker_dir();
    let Ok(entries) = std::fs::read_dir(&keep) else {
        return;
    };
    let stash = stash_dir();
    let live: std::collections::HashSet<String> = std::fs::read_dir(&stash)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let n = e.file_name();
            let n = n.to_str()?;
            Some(n.split_once('.').map(|(a, _)| a).unwrap_or(n).to_string())
        })
        .collect();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !live.contains(name) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

pub fn evict_stale() -> std::io::Result<usize> {
    sweep_orphan_markers();
    let dir = stash_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(0);
    };
    let now = std::time::SystemTime::now();
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = path.metadata() else { continue };
        // Directories too. The media frame cache stages `frames-<hash>/*.jpg`
        // as a DIRECTORY under the stash, and the old `is_file()` skip meant it
        // was never reclaimed — an unbounded pile of JPEGs, the same leak class
        // as the document `.txt` extracts.
        //
        // A directory's age is that of its NEWEST entry, not its own mtime: a
        // cache being actively reused must stay, and directory mtimes do not
        // reliably track reads on every filesystem.
        let mtime = if meta.is_dir() {
            match newest_entry_mtime(&path) {
                Some(t) => t,
                // Empty directory → nothing to lose, treat as expired.
                None => std::time::UNIX_EPOCH,
            }
        } else {
            match meta.modified() {
                Ok(t) => t,
                Err(_) => continue,
            }
        };
        // A sole-copy entry outlives the TTL: nothing else holds these bytes.
        if let Some(stem) = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.split_once('.').map(|(a, _)| a).unwrap_or(n).to_string())
        {
            if is_sole_copy(&stem) {
                continue;
            }
        }
        let Ok(age) = now.duration_since(mtime) else {
            continue;
        };
        if age.as_secs() <= CACHE_TTL_SECS {
            continue;
        }
        let gone = if meta.is_dir() {
            std::fs::remove_dir_all(&path).is_ok()
        } else {
            std::fs::remove_file(&path).is_ok()
        };
        if gone {
            removed += 1;
        }
    }
    Ok(removed)
}
