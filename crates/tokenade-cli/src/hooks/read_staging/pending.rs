//! **What we set aside between the two halves of a hook.**
//!
//! `PreToolUse` decides, `PostToolUse` reports, and nothing travels between
//! the two: they are two processes. State therefore goes through files, and
//! files outlive those who write them.
//!
//! Hence the TTLs and the sweep: state left behind by a dead session would
//! make today's hook answer with yesterday's decision — and nothing would say
//! so.

use super::*;

pub(crate) const READ_HEAD_LINES: u64 = 20;

/// How long a staged entry stays valid. The PostToolUse hook fires
/// within the same tool call (seconds), so anything older is an orphan
/// (postread hook missing, tool call aborted) and must not leak into
/// an unrelated later Read of the same file.
pub(crate) const READ_PENDING_TTL_SECS: u64 = 600;

/// Recency window for the read-dedup. A re-Read of a file last read more than
/// this long ago is served in FULL rather than a `§ref` diff/"unchanged":
/// after that long the prior content has very likely scrolled out of the
/// model's context window, so a `§ref` would just force an `expand_ref`
/// round-trip (the failure mode THOL's traces surfaced). Time is a proxy for
/// "still in context" — turns would be better, but a hook has no turn counter.
/// Override with `TOKENADE_READ_RECENCY_SECS` (0 disables the gate = old
/// always-dedup behaviour).
pub(crate) fn read_context_recency_secs() -> u64 {
    std::env::var("TOKENADE_READ_RECENCY_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(900) // 15 min — a session's active-context window
}

/// Written by `tokenade install` (global) once the PostToolUse Read
/// hook is registered; checked by the PreToolUse side before switching
/// to the allow+updatedInput flow. Without the marker, staged context
/// would never be delivered, so we stay on the legacy deny.
pub(crate) fn postread_marker_path() -> std::path::PathBuf {
    tokenade_core::platform::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".tokenade")
        .join("postread-hook-ok")
}

/// Counter files (`writegen-`, `loop-`, `batch-streak-`) are session state, not
/// staged payloads, so the 10-minute [`READ_PENDING_TTL_SECS`] must not touch
/// them: it would reset the loop guard in any session with a long think gap.
/// They used to be exempted from GC *entirely*, which is a different thing from
/// "kept longer" — measured 2026-08-03 on a real install, `read-pending` held
/// **12 228 files**, of which 9 221 were `loop-*` and 5 692 were between one and
/// seven days old. Immortality is not a retention policy.
///
/// The cost is inodes and block overhead, NOT data: each counter is a handful of
/// bytes, so `du` reported 49 MB (12 k files x one 4 KiB block) while the sum of
/// the contents rounds to zero. Expect the sweep's `bytes freed` to read ~0 and
/// its `files removed` to read in the thousands — that is the shape of this
/// problem, not a sweep that did nothing.
///
/// A day is far beyond any think gap and far below forever: a session idle for
/// 24 h is over, and resetting its loop guard costs nothing.
pub(crate) const COUNTER_TTL_SECS: u64 = 24 * 3600;

/// Is this a session-state counter rather than a staged payload?
pub(crate) fn is_counter_file(name: &str) -> bool {
    name.starts_with("writegen-") || name.starts_with("loop-") || name.starts_with("batch-streak-")
}

/// Delete what is past its horizon: staged payloads after
/// [`READ_PENDING_TTL_SECS`], counters after [`COUNTER_TTL_SECS`]. Returns
/// (files removed, bytes freed).
///
/// Path-injected so it is testable without touching `HOME`, and driven from the
/// daily maintenance rather than from a hot path — the opportunistic GCs inside
/// the staging functions only run when a Read or Grep happens to be staged,
/// which is why the directory grew to 49 MB despite having a 10-minute TTL.
pub(crate) fn sweep_read_pending_in(dir: &std::path::Path) -> (usize, u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    let (mut n, mut bytes) = (0usize, 0u64);
    for e in entries.flatten() {
        // lstat, not stat: this deletes. `remove_file` unlinks rather than
        // following, but the age decision must be the LINK's, not a target's
        // outside our directory. Same discipline as
        // `platform::sweep_own_backups_in`.
        let Ok(meta) = std::fs::symlink_metadata(e.path()) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let Some(age) = meta.modified().ok().and_then(|t| t.elapsed().ok()) else {
            continue;
        };
        let name = e.file_name();
        let name = name.to_string_lossy();
        let horizon = if is_counter_file(&name) {
            COUNTER_TTL_SECS
        } else {
            READ_PENDING_TTL_SECS
        };
        if age.as_secs() > horizon && std::fs::remove_file(e.path()).is_ok() {
            n += 1;
            bytes += meta.len();
        }
    }
    (n, bytes)
}

pub(crate) use tokenade_core::session::read_pending_dir;

/// Where compacted views from a SPECIFIC Read compactor (json/csv/yaml/…) are
/// written so the agent can read them back as an ordinary file. See
/// `stage_read_temp_file` for the why.
pub(crate) fn read_temp_dir() -> std::path::PathBuf {
    if let Some(p) = tokenade_core::env_compat::path_from_env("TOKENADE_READ_TEMP_DIR") {
        return p;
    }
    tokenade_core::platform::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".tokenade")
        .join("read-temp")
}

/// True iff `file_path` is one of OUR own compacted-view temp files. Such a
/// Read must pass through untouched — re-compacting it would loop, and it is
/// already the minimal view we want the agent to hold.
pub(crate) fn is_read_temp_file(file_path: &str) -> bool {
    let dir = read_temp_dir();
    std::path::Path::new(file_path).starts_with(&dir) || is_a_stashed_copy(file_path)
}

/// **The recovery copy is served as it was stored.** A stash file is the
/// COMPLETE output a fold set aside; an agent that opens it with `Read` is
/// fetching exactly the bytes the fold hid. Read in a real session
/// (2026-10-02, an agent driving a CLI): the agent read `~/.cache/tokenade/stash/<h>.json`
/// to get past cut JSON fields, and the Read hook folded the stash ITSELF
/// (1 114 → 489 tokens, `[tokenade:read-auto-compact kind=json]`) behind a
/// new hash — the way back led to a second fold.
pub(crate) fn is_a_stashed_copy(file_path: &str) -> bool {
    std::path::Path::new(file_path).starts_with(tokenade_core::disclose::stash_dir())
}

/// Write a specific-compactor's FULL compacted view to a throwaway file and
/// return its path, so the Read can be REDIRECTED there (requirement from
/// 2026-06-19: "serve the … compacted tokens directly").
///
/// Why this and not the head-slice+additionalContext flow: the head slice
/// leaves a ~100-token tool_result — too small to anchor a prompt-cache
/// breakpoint — so the read phase is never durably cached and Claude Code
/// re-creates the whole Read block at the first Edit (measured: one
/// cache_read→base invalidation per run, cc ~16k, wiping the compaction gain).
/// Redirecting `file_path` to a temp file holding the compacted view means the
/// agent gets the WHOLE compacted body as ONE ordinary tool_result — exactly
/// like the control arm's full reads, which grow the cache monotonically — so
/// we keep the token saving AND stop fragmenting the cache. A view too large
/// for the temp file is handled by Claude Code's own native Read truncation +
/// offset/grep steer (cache-stable), the same path the control arm hits.
///
/// `.txt` extension + the dedicated dir keep the temp file out of every
/// format-specific compactor (and `is_read_temp_file` short-circuits the Read
/// handler), so reading it back never re-enters compaction.
pub(crate) fn stage_read_temp_file(
    session_id: &str,
    file_path: &str,
    offset: u64,
    limit: u64,
    content: &str,
) -> Option<std::path::PathBuf> {
    let dir = read_temp_dir();
    std::fs::create_dir_all(&dir).ok()?;
    // Opportunistic GC — same TTL as read-pending; the dir only holds
    // in-flight views the agent is about to read.
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let stale = e
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.elapsed().ok())
                .map(|d| d.as_secs() > READ_PENDING_TTL_SECS)
                .unwrap_or(false);
            // Counter files (write-generation, loop-guard, batch streaks)
            // are session state, not staged payloads — GC'ing them reset
            // the loop guard in any session with a >10-min think gap.
            let name = e.file_name();
            let name = name.to_string_lossy();
            // Counters get the longer horizon rather than immunity — see
            // `COUNTER_TTL_SECS` for what immunity cost.
            let is_counter = is_counter_file(&name);
            let very_stale = e
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.elapsed().ok())
                .map(|d| d.as_secs() > COUNTER_TTL_SECS)
                .unwrap_or(false);
            if (stale && !is_counter) || (is_counter && very_stale) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let key = read_pending_key(session_id, file_path, offset, limit);
    // Write-then-rename so a concurrent reader never sees a half-written view.
    let tmp = dir.join(format!("{key}.txt.tmp-{}", std::process::id()));
    let dst = dir.join(format!("{key}.txt"));
    if std::fs::write(&tmp, content).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    std::fs::rename(&tmp, &dst).ok()?;
    Some(dst)
}
