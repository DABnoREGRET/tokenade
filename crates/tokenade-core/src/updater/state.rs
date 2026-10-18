//! **What the update leaves on disk** — and the lock that keeps two copies
//! from stepping on each other.
//!
//! The cache carries the date of the last check: without it, every start
//! would pay a network round-trip. The opt-out sentinel carries the user's
//! refusal, which must survive an update.
//!
//! The LOCK is the only non-obvious part. The TTL fast path narrows the race
//! without closing it: two status-bar ticks (~3 Hz) can read the same stale
//! cache, both pass the gate, and each start a ~50 MB download. A
//! non-blocking exclusive `flock` admits only ONE worker; the losers give up
//! BEFORE any network access. The OS releases the lock when its holder dies,
//! so a stale lock never blocks later updates — no pid bookkeeping.

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpdateCheckCache {
    /// Unix-ms timestamp of the last check (success OR failure — TTL is
    /// the same to avoid hammering a flaky endpoint).
    #[serde(default)]
    pub last_check: u64,
    /// Highest manifest.version observed so far. Used to skip the swap
    /// path if the new manifest doesn't actually advance us.
    #[serde(default)]
    pub last_seen_version: String,
    /// Most recent failure reason, if any. Cleared on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// #1036 — version string of the binary the last time we ran the
    /// post-upgrade refresh (re-splice `~/.claude/CLAUDE.md` from the
    /// current `GLOBAL_CLAUDE_MD_BLOCK`, etc.). When this differs from
    /// `env!("CARGO_PKG_VERSION")`, the CLI runs the silent refresh
    /// and updates this field. Pre-fix the auto-updater swapped the
    /// binary but left the user's CLAUDE.md on the wording shipped by
    /// whichever version they first ran `tokenade install` against —
    /// so any post-install scaffold/block change (e.g. #1031's
    /// expand_ref-first reframing) never landed without a manual
    /// re-install.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_refreshed_version: Option<String>,
}

/// #1036 — true when the CLI should re-run the silent post-upgrade
/// refresh (drift-detection writes for `~/.claude/CLAUDE.md` etc.).
/// Returns true when the cached `last_refreshed_version` differs from
/// the supplied `current` — typically `env!("CARGO_PKG_VERSION")`.
/// Honours `TOKENADE_NO_AUTO_UPDATE=1` opt-out (same env var that
/// disables the auto-updater fetch itself; if the user opted out of
/// auto-update they almost certainly don't want post-update side
/// effects either). Treats first-ever invocation (cache field is
/// None) as "needs refresh" so fresh installs also pick up the
/// refresh on first run.
pub fn needs_post_upgrade_refresh(current: &str) -> bool {
    if is_opted_out() {
        return false;
    }
    let cache = load_cache();
    cache.last_refreshed_version.as_deref() != Some(current)
}

/// #1036 — record that the silent post-upgrade refresh completed for
/// the supplied version, so subsequent invocations skip until the
/// next binary upgrade lands. Best-effort: I/O failures are silently
/// swallowed because the next invocation will just retry the refresh
/// (the refresh itself is idempotent + no-op when nothing's drifted).
pub fn mark_post_upgrade_refresh_done(version: &str) {
    let mut cache = load_cache();
    cache.last_refreshed_version = Some(version.to_string());
    let _ = save_cache(&cache);
}

// ─────────────────────────────────────────────────────────────────────────
// Path helpers.
// ─────────────────────────────────────────────────────────────────────────

pub(crate) fn update_cache_path() -> PathBuf {
    license_cache_path_dir().join("update-check.json")
}

/// `~/.tokenade` (or `TOKENADE_CONFIG_DIR`). Exposed so CLI commands (e.g.
/// `tokenade upgrade --off`) can write sentinel files beside the update cache.
pub fn license_cache_path_dir() -> PathBuf {
    crate::platform::tokenade_state_dir()
}

/// Path to the auto-update opt-out sentinel file. Single source of
/// truth — `cmd_upgrade --off` writes it, `--on` removes it,
/// `is_opted_out()` reads it. Public so the CLI can stay in sync
/// without re-typing the filename literal (#991).
pub fn opt_out_sentinel() -> PathBuf {
    license_cache_path_dir().join("auto-update-off")
}

pub(crate) fn is_opted_out() -> bool {
    crate::env_compat::is_truthy_one("TOKENADE_NO_AUTO_UPDATE") || opt_out_sentinel().is_file()
}

// ─────────────────────────────────────────────────────────────────────────
// Single-flight lock.
//
// The TTL fast-path + pre-stamp in `spawn_background_check` narrow but
// cannot CLOSE the spawn race: two statusline ticks (~3 Hz) can read the
// same stale cache, both pass the gate, both spawn a `--auto-background`
// child — each firing a ~50 MB download in parallel. An exclusive,
// non-blocking advisory flock on `~/.tokenade/update.lock` (same fs2
// mechanism the reporter uses on `reporter.lock`) admits exactly ONE
// worker; the losers bail instantly, BEFORE any network I/O. The OS
// releases the flock when the holder dies (drop, early return, crash), so
// a stale lock never wedges future updates — no pid/staleness bookkeeping
// needed.
// ─────────────────────────────────────────────────────────────────────────

/// Path to the updater's single-flight lock, alongside the licence cache
/// (same dir & perms as `reporter.lock` / `update-check.json`).
pub(crate) fn update_lock_path() -> PathBuf {
    license_cache_path_dir().join("update.lock")
}

/// Outcome of a non-blocking attempt to acquire the updater lock.
pub(crate) enum LockOutcome {
    /// We hold the lock. Keep the `File` alive to retain it — dropping it
    /// (end of scope / return / panic) releases the flock.
    Acquired(std::fs::File),
    /// Another updater already holds the lock — caller should bail.
    Contended,
    /// The lock file couldn't be opened (read-only FS, perms, …). Fail-open:
    /// callers proceed unlocked = the pre-lock behaviour, never blocked.
    Unavailable,
}

/// Non-blocking `flock(LOCK_EX)` on `path`. Split out from
/// [`try_acquire_update_lock`] so tests can point it at a temp file.
pub(crate) fn try_acquire_lock_at(path: &Path) -> LockOutcome {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    // `.read(true)` + `.write(true)`: on Windows LockFileEx needs both
    // access rights or the lock is a silent no-op (same note as the
    // reporter lock). `create(true)` so the first ever run makes the file.
    // `truncate(false)` stated: this is a lock carrier, never written, and a
    // concurrent updater may hold it right now.
    let file = match std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
    {
        Ok(f) => f,
        Err(_) => return LockOutcome::Unavailable,
    };
    use fs2::FileExt;
    match file.try_lock_exclusive() {
        Ok(()) => LockOutcome::Acquired(file),
        // fs2 maps lock contention to a platform-specific error; compare its
        // kind against the canonical contended error to distinguish "someone
        // else holds it" (bail) from a genuine I/O failure (fail-open).
        Err(e) if e.kind() == fs2::lock_contended_error().kind() => LockOutcome::Contended,
        Err(_) => LockOutcome::Unavailable,
    }
}

/// Try to grab the process-wide updater lock without blocking.
pub(crate) fn try_acquire_update_lock() -> LockOutcome {
    try_acquire_lock_at(&update_lock_path())
}

// ─────────────────────────────────────────────────────────────────────────
// Cache I/O.
// ─────────────────────────────────────────────────────────────────────────

pub fn load_cache() -> UpdateCheckCache {
    let path = update_cache_path();
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_cache(cache: &UpdateCheckCache) -> std::io::Result<()> {
    let path = update_cache_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(cache).map_err(std::io::Error::other)?;
    // #998 — atomic write: tmp + rename. Pre-fix a Ctrl-C / OOM / system
    // crash mid-`fs::write` left the cache file truncated; `load_cache`
    // silently fell back to Default via the `.ok()` swallow, which forced
    // a fresh network manifest fetch on the next invocation (ignoring the
    // 24h TTL). Rare but real — the rename is atomic across all the
    // filesystems we target, so the file is either the prior valid JSON
    // or the new valid JSON, never half-written. Same pattern as the
    // gain.jsonl rotate path (#989). No 0600 needed: this cache holds
    // last_check / last_seen_version / last_error — not secrets.
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, &bytes)?;
    fs::rename(&tmp, &path)
}

pub(crate) use crate::platform::now_unix_millis as now_ms;

/// True when the cached `last_check` timestamp is still within the TTL
/// window and the spawn can be skipped.
///
/// #1024 — pre-fix the call site was
/// `cache.last_check > 0 && now_ms().saturating_sub(cache.last_check)
/// < ttl_ms`. The saturating-sub silently collapses to 0 when
/// `now < last_check`, which then trivially satisfies `< ttl_ms` → we
/// SKIP the spawn forever until wall-clock catches back up. Trigger
/// scenarios: laptop boots with wrong-future clock (dead-battery RTC
/// reset) then NTP-syncs backward, VM resume with time-jump, manual
/// clock-set backward. The user loses auto-updates silently for as
/// long as the rewind delta.
///
/// Fix: treat `now < last_check` as cache-invalid (NOT fresh) so the
/// next call spawns the check and overwrites the bogus future
/// timestamp. Also preserves the `last_check == 0` initial bootstrap
/// (cache empty → never fresh → spawn).
pub(crate) fn ttl_cache_fresh(now: u64, last_check: u64, ttl_ms: u64) -> bool {
    if last_check == 0 || now < last_check {
        return false;
    }
    now - last_check < ttl_ms
}
