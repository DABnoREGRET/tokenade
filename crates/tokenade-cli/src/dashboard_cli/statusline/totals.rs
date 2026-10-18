//! **The number, and why it is CACHED.**
//!
//! The status line is rendered at every prompt: recomputing the totals from
//! `gain.jsonl` on that path would make the user wait for a number that has
//! not changed.
//!
//! The hot path reads a cache; the refresh runs DETACHED and debounced, so a
//! burst of renders launches only one worker per minute.

use super::*;

/// Fire-and-forget a detached recompute of the real-savings cache, debounced
/// so a burst of statusline renders spawns at most one worker per minute. The
/// child removes the lock when done; a crashed child self-clears after 60s.
pub(super) fn spawn_real_savings_refresh() {
    let Some(home) = tokenade_core::platform::home_dir() else {
        return;
    };
    let lock = home.join(".tokenade").join("real-savings-refresh.running");
    let busy = std::fs::metadata(&lock)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|m| m.elapsed().ok())
        .map(|e| e.as_secs() < 30)
        .unwrap_or(false);
    if busy {
        return;
    }
    if let Some(dir) = lock.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&lock, "");
    if let Ok(exe) = std::env::current_exe() {
        let mut refresh = std::process::Command::new(exe);
        refresh
            .arg("real-savings-refresh")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        // Background refresh fired from the status line: all stdio nulled, so
        // it needs no console and must not flash one.
        tokenade_core::process_util::configure_no_window(&mut refresh);
        let _ = refresh.spawn();
    }
}

/// Real per-model lifetime $ for the statusline — NEVER blocks. Fresh cache
/// is used as-is; a stale cache is shown immediately while a detached worker
/// refreshes it; only a wholly-absent cache computes inline (once, e.g. right
/// after install before the reporter or a dashboard has populated it).
pub(super) fn real_savings_usd_hotpath() -> f64 {
    // 60 s TTL (was 300): the lifetime $ looked frozen next to the live
    // session figure; the refresh worker is detached + debounced, so a
    // faster cadence costs nothing on the render path.
    match tokenade_core::gain::real_savings_read_cache(60) {
        Some((rs, true)) => rs.usd,
        Some((rs, false)) => {
            spawn_real_savings_refresh();
            rs.usd
        }
        None => tokenade_core::gain::real_savings_refresh().usd,
    }
}

/// Compute `(tokens_before, tokens_after)` from gain.jsonl with a
/// mtime-keyed cache (#484). Statusline ticks once per agent action
/// in Claude Code — for long-lived sessions the full 3+ MB re-parse
/// cost (~13ms) compounds. The cache lives at
/// `~/.tokenade/statusline_cache.json` and is invalidated when
/// gain.jsonl's mtime changes (covers both appends and rotation).
/// Best-effort: any cache I/O failure silently falls back to a fresh
/// recompute. Race-tolerant: two concurrent statusline ticks compute
/// the same totals; last-writer-wins on the cache file.
pub(crate) fn statusline_totals(path: &std::path::Path) -> (u64, u64) {
    // #1044 — the key covers BOTH files this figure is computed from. Keying
    // on the live ledger alone meant a rotation, which moves records INTO the
    // rollup and leaves the live file's mtime free to stay put for a moment,
    // could serve a cached total that no longer matched either file.
    let mtime_of = |p: &std::path::Path| {
        fs::metadata(p)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
    };
    let rollup_path = tokenade_core::gain::rollup_path_for(path);
    let file_mtime = mtime_of(path).map(|m| m.wrapping_add(mtime_of(&rollup_path).unwrap_or(0)));

    let cache_path = path.with_file_name("statusline_cache.json");
    // Cache hit: file mtime matches AND cache schema version matches.
    //
    // #743 — version gate. Caches without `version` (or with version<2)
    // pre-date the code-intel exclusion fix and contain inflated
    // totals. Treat them as miss → recompute → overwrite with v2.
    if let Some(mt) = file_mtime {
        if let Ok(body) = fs::read_to_string(&cache_path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
                let cached_mt = v["mtime_secs"].as_u64().unwrap_or(0);
                let cached_ver = v["version"].as_u64().unwrap_or(0);
                if cached_mt == mt && cached_ver >= 3 {
                    return (
                        v["before"].as_u64().unwrap_or(0),
                        v["after"].as_u64().unwrap_or(0),
                    );
                }
            }
        }
    }

    // Miss — recompute from scratch.
    //
    // #743 — apply the same aggregation rules as cmd_dashboard_text /
    // dashboard_json / dashboard_html / cmd_gain so all FIVE surfaces
    // agree on the headline savings figure. Pre-#743 the statusline
    // summed code-intel rows into total_before/total_after, which made
    // `[tokenade] saved …` drift from `tokenade dashboard`'s
    // "measured" hero. Code-intel rows are heuristic, not measured, so
    // they belong with `estimated_saved` (which the statusline
    // doesn't display) not with `saved`. Same #544 read-time heuristic
    // expansion as the dashboard.
    // #1044 — read the ledger UNION its durable rollup, exactly as the
    // dashboard does. Reading the live file alone made the statusline report
    // only what had not yet been rotated: on this install that was 357M of
    // 963M, so `[tokenade] saved …` showed barely a third of the figure the
    // dashboard showed for the same data, and it fell further after each
    // rotation. The aggregation rules themselves come from the shared scan,
    // so the two surfaces cannot drift apart again.
    let content = tokenade_core::gain::ledger_with_rollup(path);
    let scan = tokenade_core::gain::scan_ledger(&content, 0);
    let (total_before, total_after) = (scan.measured_before, scan.measured_after);

    // Best-effort cache write (atomic via tmp+rename so a concurrent
    // statusline reader never sees a half-written file).
    //
    // #743 — bump the cache schema with a `version` field. Old caches
    // (pre-#743, no version) included code-intel in the totals; we
    // invalidate them on first read by requiring `version == 2`.
    if let Some(mt) = file_mtime {
        let new_body = serde_json::json!({
            "version": 3,
            "mtime_secs": mt,
            "before": total_before,
            "after": total_after,
        });
        // #990 — atomic write at 0600 (Unix). Same TOCTOU class as
        // bypass-counter (#988) + gain.jsonl rotate/prune (#989): plain
        // `fs::write` would create at umask-default (644), and the
        // walk-pass (#985) only tightens on the next startup. 1 helper,
        // 4 callers.
        let _ =
            tokenade_core::platform::write_atomic_600(&cache_path, new_body.to_string().as_bytes());
    }

    (total_before, total_after)
}
