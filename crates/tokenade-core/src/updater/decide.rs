//! **Should we update, and to what?**
//!
//! Two constraints of the module take precedence here: NEVER block the user —
//! they launched `tokenade auto`, not an update, so the check runs in a
//! detached thread — and NEVER write to stdout, which the MCP server uses to
//! speak JSON-RPC.
//!
//! Version comparison handles the release/pre-release boundary: per the spec,
//! `X.Y.Z-something` < `X.Y.Z`, so a client on a pre-release MUST be able to
//! move up to the GA of the same triplet. And the HTTP timeout is a PER-READ
//! limit, not a total: 130 MB in 120 s would require a sustained 1.08 MB/s,
//! and a slower user could never update.

use super::*;

// ─────────────────────────────────────────────────────────────────────────
// Update gate — called by every CLI/MCP startup. Cheap fast-path: env
// opt-out check + cache TTL read + bail. Network only on TTL miss.
// ─────────────────────────────────────────────────────────────────────────

/// Spawn the updater in a detached thread. Safe to call from any startup
/// path; returns immediately. The thread will silently no-op when:
/// - `TOKENADE_NO_AUTO_UPDATE=1` is set (CI / corporate firewall lockdown)
/// - the cache file says we checked within the last 24h
/// - the binary path doesn't look swap-able (e.g. running from a tarball,
///   read-only filesystem, system-package install)
/// - the manifest endpoint is unreachable or signature fails verification
pub fn spawn_background_check() {
    if is_opted_out() {
        return;
    }
    // Fast-path: skip the spawn entirely if cache says we're fresh. The
    // OS thread creation cost matters when this runs on every CLI invocation.
    let cache = load_cache();
    let ttl_ms = UPDATE_TTL.as_millis() as u64;
    if ttl_cache_fresh(now_ms(), cache.last_check, ttl_ms) {
        return;
    }
    // A detached THREAD dies with the process — and hook invocations live
    // milliseconds, so the check virtually never completed in the field
    // (this machine: hundreds of invocations/day, last completed check
    // 4 days stale, last swap 0.6.2). A detached PROCESS survives the
    // parent: spawn ourselves with the hidden --auto-background flag.
    // The TTL fast-path above keeps this to at most one spawn per 24 h.
    // Pre-spawn short-circuit (optimisation, not the correctness guarantee):
    // if an updater is already running its lock is held, so don't even spawn
    // a child that would just bail in `run_auto_background`. Probe with a
    // non-blocking try-lock and release immediately (the temporary — and any
    // `File` it holds — drops at the end of this statement); the child does
    // the authoritative acquisition. Only `Contended` skips; `Acquired`
    // (nobody running) and `Unavailable` (FS error) fall through to the
    // normal spawn. A crashed holder's lock is OS-released, so this can never
    // durably wedge future spawns.
    if let LockOutcome::Contended = try_acquire_update_lock() {
        return;
    }
    // Stamp the TTL marker BEFORE spawning: the gate above is mtime-based
    // and the child only wrote it at the END of its network work — during
    // that in-flight window every statusline tick (~3 Hz) spawned another
    // full check (audit B, critical). Worst case now: two ticks racing
    // this exact line — bounded, harmless.
    {
        let mut c = load_cache();
        c.last_check = now_ms();
        let _ = save_cache(&c);
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    // Fully detach (new session via setsid on unix / detached creation flags
    // on windows): the child does a ~50 MB download that far outlives this
    // parent — usually a hook invocation exiting in milliseconds — so it must
    // survive SIGHUP and a killpg of the parent's process group.
    let _ = crate::process_util::spawn_fully_detached(&exe, &["upgrade", "--auto-background"]);
}

/// Body of the detached `upgrade --auto-background` child: the full check
/// (+ swap when a newer signed release exists), with every outcome written
/// to the cache so the next TTL gate sees it.
pub fn run_auto_background() {
    // Single-flight guard (see the lock section above). Acquire the
    // exclusive lock as the FIRST thing — before any network/download — so
    // that of the 2-3 children a spawn TOCTOU may launch, exactly one does
    // the work and the rest return here for free. Held for the whole fn:
    // dropped (→ flock released) on normal return, the `?`-style early
    // returns inside `check_and_swap`, or a crash. Fail-open: `Unavailable`
    // (read-only FS, …) proceeds unlocked = the pre-lock behaviour.
    let _lock = match try_acquire_update_lock() {
        LockOutcome::Contended => return, // another updater already in flight
        LockOutcome::Acquired(f) => Some(f),
        LockOutcome::Unavailable => None,
    };
    if let Err(e) = check_and_swap() {
        let mut c = load_cache();
        c.last_check = now_ms();
        c.last_error = Some(e.to_string());
        let _ = save_cache(&c);
        debug_log::log("updater", "check_failed", serde_json::json!({ "err": e }));
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Update flow — runs in the background thread.
// ─────────────────────────────────────────────────────────────────────────

/// Synchronous version of the background check, used by `tokenade upgrade`.
/// Returns `Ok(Some(version))` when a swap happened, `Ok(None)` if already
/// up to date, `Err(msg)` on any failure.
pub fn check_and_swap_now() -> Result<Option<String>, String> {
    let endpoint = crate::env_compat::update_endpoint();
    let manifest_bytes =
        crate::net::get_body_capped(&format!("{endpoint}/manifest.json"), MANIFEST_MAX_BYTES)?;
    let sig_bytes = crate::net::get_body_capped(&format!("{endpoint}/manifest.json.sig"), 256)?;
    crate::signing::verify_detached(&manifest_bytes, &sig_bytes)?;
    let manifest: ManifestV1 =
        serde_json::from_slice(&manifest_bytes).map_err(|e| format!("manifest parse: {e}"))?;
    validate_manifest_basics(&manifest)?;
    let current = env!("CARGO_PKG_VERSION");
    if !semver_gt(&manifest.version, current) {
        let mut c = load_cache();
        c.last_check = now_ms();
        c.last_seen_version = manifest.version;
        c.last_error = None;
        let _ = save_cache(&c);
        return Ok(None);
    }
    // #994 — honour the manifest's minSupportedVersion. Refuse the
    // auto-upgrade when the current client predates the boundary.
    if let Some(msg) = min_supported_blocks(&manifest, current) {
        return Err(msg);
    }
    let target = current_target_triple();
    let entry = manifest
        .targets
        .get(target)
        .ok_or_else(|| no_target_msg(&manifest, target))?;
    let tarball =
        crate::net::get_body_capped(&entry.url, entry.size.min(BINARY_MAX_BYTES) as usize + 1024)?;
    if tarball.len() as u64 != entry.size {
        return Err(format!(
            "size mismatch: {} vs {}",
            tarball.len(),
            entry.size
        ));
    }
    let mut h = Sha256::new();
    h.update(&tarball);
    let got = hex::encode(h.finalize());
    if got.to_lowercase() != entry.sha256.to_lowercase() {
        return Err(format!("sha256 mismatch: got {got}, want {}", entry.sha256));
    }
    let exe_path = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    extract_and_swap(&tarball, &exe_path)?;
    let mut c = load_cache();
    c.last_check = now_ms();
    c.last_seen_version = manifest.version.clone();
    c.last_error = None;
    let _ = save_cache(&c);
    Ok(Some(manifest.version))
}

/// Light variant — fetch+verify the manifest, return what would be
/// swapped, but DON'T touch the live binary. Used by
/// `tokenade upgrade --check` for dry-run inspection.
pub struct AvailableUpdate {
    pub version: String,
    pub target: String,
    pub url: String,
    pub size: u64,
    /// Unix-seconds release timestamp from the manifest. #996 — was
    /// deserialised into `ManifestV1.released_at` for several
    /// releases but never surfaced anywhere; the CLI now formats it
    /// as "released N days ago" so users can spot fresh-vs-aged
    /// releases at a glance.
    pub released_at: u64,
}

pub fn check_only() -> Result<Option<AvailableUpdate>, String> {
    let endpoint = crate::env_compat::update_endpoint();
    let manifest_bytes =
        crate::net::get_body_capped(&format!("{endpoint}/manifest.json"), MANIFEST_MAX_BYTES)?;
    let sig_bytes = crate::net::get_body_capped(&format!("{endpoint}/manifest.json.sig"), 256)?;
    crate::signing::verify_detached(&manifest_bytes, &sig_bytes)?;
    let manifest: ManifestV1 =
        serde_json::from_slice(&manifest_bytes).map_err(|e| format!("manifest parse: {e}"))?;
    // #1023 — `--check` previously skipped the manifest.v check that
    // both swap paths enforce, so a v=2 future manifest produced a
    // misleading "update available" then `tokenade upgrade` errored.
    validate_manifest_basics(&manifest)?;
    let current = env!("CARGO_PKG_VERSION");
    if !semver_gt(&manifest.version, current) {
        return Ok(None);
    }
    // #994 — surface the min-supported-version boundary in --check too,
    // so users see the actionable error WITHOUT triggering a real swap.
    if let Some(msg) = min_supported_blocks(&manifest, current) {
        return Err(msg);
    }
    let target = current_target_triple();
    let entry = manifest
        .targets
        .get(target)
        .ok_or_else(|| no_target_msg(&manifest, target))?;
    Ok(Some(AvailableUpdate {
        version: manifest.version,
        target: target.to_string(),
        url: entry.url.clone(),
        size: entry.size,
        released_at: manifest.released_at,
    }))
}

pub(crate) fn check_and_swap() -> Result<(), String> {
    let endpoint = crate::env_compat::update_endpoint();
    let manifest_url = format!("{endpoint}/manifest.json");
    let sig_url = format!("{endpoint}/manifest.json.sig");

    let manifest_bytes = crate::net::get_body_capped(&manifest_url, MANIFEST_MAX_BYTES)?;
    let sig_bytes = crate::net::get_body_capped(&sig_url, 256)?;

    crate::signing::verify_detached(&manifest_bytes, &sig_bytes)?;

    let manifest: ManifestV1 =
        serde_json::from_slice(&manifest_bytes).map_err(|e| format!("manifest parse: {e}"))?;
    validate_manifest_basics(&manifest)?;

    let current = env!("CARGO_PKG_VERSION");
    if !semver_gt(&manifest.version, current) {
        // Up-to-date. Persist success.
        let mut c = load_cache();
        c.last_check = now_ms();
        c.last_seen_version = manifest.version.clone();
        c.last_error = None;
        let _ = save_cache(&c);
        debug_log::log(
            "updater",
            "up_to_date",
            serde_json::json!({ "current": current, "latest": manifest.version }),
        );
        return Ok(());
    }
    // #994 — background path also honours minSupportedVersion. Log the
    // boundary breach + last_error so `tokenade healthcheck` surfaces it,
    // then return without swapping. The CLI `upgrade` arms (above) get
    // the actionable message; the background path is silent-by-design
    // (NEVER println), so we log instead.
    if let Some(msg) = min_supported_blocks(&manifest, current) {
        let mut c = load_cache();
        c.last_check = now_ms();
        c.last_seen_version = manifest.version.clone();
        c.last_error = Some(msg.clone());
        let _ = save_cache(&c);
        debug_log::log(
            "updater",
            "min_supported_blocked",
            serde_json::json!({
                "current": current,
                "latest": manifest.version,
                "min_supported": manifest.min_supported_version,
            }),
        );
        return Err(msg);
    }

    let target = current_target_triple();
    // #995 — use the canonical `no_target_msg` helper. Pre-fix, this
    // third (background) check site inlined the stale
    // `"no binary published for target X"` format that the doc comment
    // on `no_target_msg` says is shared by `check_and_swap_now` and
    // `check_only`. The contract was already wrong — the background
    // path emitted the unhelpful error to debug.log + update-check
    // cache, while the two CLI sites listed the available targets
    // (or flagged the empty-manifest mis-publish). Now all 3 sites
    // agree.
    let entry = manifest
        .targets
        .get(target)
        .ok_or_else(|| no_target_msg(&manifest, target))?;

    debug_log::log(
        "updater",
        "update_available",
        serde_json::json!({
            "current": current,
            "latest": manifest.version,
            "target": target,
            "size": entry.size,
        }),
    );

    let tarball =
        crate::net::get_body_capped(&entry.url, entry.size.min(BINARY_MAX_BYTES) as usize + 1024)?;
    if tarball.len() as u64 != entry.size {
        return Err(format!(
            "downloaded size {} ≠ manifest size {}",
            tarball.len(),
            entry.size
        ));
    }
    let mut h = Sha256::new();
    h.update(&tarball);
    let got = hex::encode(h.finalize());
    if got.to_lowercase() != entry.sha256.to_lowercase() {
        return Err(format!(
            "sha256 mismatch: got {got}, manifest says {}",
            entry.sha256
        ));
    }

    let exe_path = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    extract_and_swap(&tarball, &exe_path)?;

    let mut c = load_cache();
    c.last_check = now_ms();
    c.last_seen_version = manifest.version.clone();
    c.last_error = None;
    let _ = save_cache(&c);
    debug_log::log(
        "updater",
        "swapped",
        serde_json::json!({ "to": manifest.version, "target": target }),
    );
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────
// Signature verification — canonical bytes from the release pipeline
// (JSON.stringify with a fixed key order). The emitter writes the
// canonical bytes verbatim, so we just verify the raw fetched bytes.
// ─────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────
// Semver — manifest.version > current. Compares the numeric triple
// (major.minor.patch) AND handles the release-vs-prerelease boundary:
// per semver, `X.Y.Z-anything` < `X.Y.Z`, so a client on a prerelease
// MUST be allowed to upgrade to the GA at the same triple.
// ─────────────────────────────────────────────────────────────────────────

pub(crate) fn semver_gt(a: &str, b: &str) -> bool {
    /// Returns ((major, minor, patch), Option<prerelease_string>).
    /// Build metadata (`+...`) is stripped first since per semver it
    /// never affects precedence.
    fn parse(s: &str) -> ((u64, u64, u64), Option<String>) {
        // #956 — defensively strip an optional `v` prefix. A manifest
        // publishing `"v1.0.0"` (a common convention — git tags ship
        // that way) would otherwise parse as (0,0,0) because `"v1"`
        // fails u64::from_str, silently disabling the upgrade for the
        // entire major version.
        let s = s
            .strip_prefix('v')
            .or_else(|| s.strip_prefix('V'))
            .unwrap_or(s);
        // Strip build metadata first — never affects ordering.
        let no_build = s.split('+').next().unwrap_or(s);
        // Split numeric core from optional prerelease label.
        let (numeric, prerelease) = match no_build.split_once('-') {
            Some((n, p)) if !p.is_empty() => (n, Some(p.to_string())),
            _ => (no_build, None),
        };
        let mut it = numeric.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
        let triple = (
            it.next().unwrap_or(0),
            it.next().unwrap_or(0),
            it.next().unwrap_or(0),
        );
        (triple, prerelease)
    }
    let (a_triple, a_pre) = parse(a);
    let (b_triple, b_pre) = parse(b);
    if a_triple != b_triple {
        return a_triple > b_triple;
    }
    // Same numeric triple. Per semver §11: prereleases sort BEFORE
    // the corresponding release. #1026 — pre-fix this returned false
    // even for `semver_gt("0.5.0", "0.5.0-rc1")`, leaving clients
    // running a `-rc1` build (custom builds, git-tag installs)
    // permanently pinned with no auto-upgrade path to GA.
    match (&a_pre, &b_pre) {
        // a is release, b is prerelease → a > b (rc → GA upgrades).
        (None, Some(_)) => true,
        // a is prerelease, b is release → a < b (no GA → rc downgrade).
        (Some(_), None) => false,
        // Both release at same triple → equal, not greater.
        (None, None) => false,
        // Both prerelease at same triple — best-effort string compare
        // so `rc2 > rc1` works for the common case. Full semver
        // identifier-by-identifier ordering would be more correct but
        // adds complexity for a path we don't ship into anyway; the
        // string fallback is still better than the pre-fix "always
        // equal" behaviour.
        (Some(ap), Some(bp)) => ap > bp,
    }
}

/// #994 — gate the auto-upgrade when the manifest's
/// `minSupportedVersion` is newer than the currently-installed
/// client. The field is documented as "older versions should NOT
/// auto-update across this boundary (forces user to read the release
/// notes)" — but pre-fix no check site actually inspected it, so
/// servers could publish breaking-change boundaries that clients
/// would silently jump over.
///
/// Returns `Some(actionable-error)` when the upgrade should be
/// refused, `None` when the boundary is met (or absent). The error
/// message points the user at manual upgrade + release notes.
pub(crate) fn min_supported_blocks(manifest: &ManifestV1, current: &str) -> Option<String> {
    let min = manifest.min_supported_version.as_deref()?;
    if !semver_gt(min, current) {
        return None;
    }
    Some(format!(
        "this release ({}) requires tokenade >= {} as the upgrade base, but you're on {} — \
         please read the release notes at https://github.com/pi-infected/tokenade/blob/main/PATCHNOTES.md \
         and upgrade manually (`npm i -g @tokenade/cli@latest`, or download from \
         https://github.com/pi-infected/tokenade/releases)",
        manifest.version, min, current
    ))
}

// ─────────────────────────────────────────────────────────────────────────
// HTTP — 10-second connect deadline, 120-second PER-READ deadline, and a
// generous absolute ceiling so nothing hangs forever.
//
// This comment claimed "per-read" for months while the code passed the number
// to `ClientBuilder::timeout`, which is a TOTAL deadline: connect through last
// body byte. 130 MB inside 120 s demands 1.08 MB/s sustained, so a user slower
// than that could never upgrade — reported from the field 2026-06-10 on macOS,
// `✗ Upgrade failed: http body: timed out reading response`.
//
// It had already gone wrong the same way once: the note below records the total
// deadline being raised from 10 s after it truncated downloads into a "size
// mismatch". Raising the number moved the wall; it did not stop it being one.
// `net::streaming_client` uses `read_timeout` now, which fires only when no
// byte arrives at all. It also says the mention of `ureq` is stale — this path
// has been reqwest for a while.
