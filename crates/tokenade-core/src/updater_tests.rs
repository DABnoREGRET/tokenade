// The tests of `crate::updater`, moved out of the production file.
//
// Mounted by `include!`: the tokens are spliced INTO `updater`, so every
// `super::` keeps its meaning. Split by `srcsweep::production_lines`.

#[cfg(test)]
mod tests {
    use super::*;

    /// Several tests in this module mutate `$HOME` to redirect
    /// `load_cache`/`save_cache` at a per-test tempdir. Cargo runs the
    /// module's tests in parallel by default, so without serialisation a
    /// sibling test can set `$HOME` to its own tempdir mid-flight and
    /// `load_cache` returns stale (or absent) state, failing the
    /// round-trip assertions. Every HOME-touching test takes this lock
    /// for its whole body.
    /// One lock, because the ENVIRONMENT is one. This used to be a private
    /// `Mutex` of its own, which serialises the tests that take IT and
    /// nothing else: measured across `tokenade-core`, twelve lock groups
    /// existed and four shared a variable with the crate-wide guard —
    /// **`HOME` alone was set under three mutually unaware locks**. That is
    /// the shape of the flakes once observed: one run in
    /// eight, never the same test name. A lock is only sound when no OTHER
    /// lock touches its variables.
    fn home_env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    /// Single-flight: two concurrent acquisitions of the SAME lock path —
    /// exactly one wins (`Acquired`), the other is told to bail
    /// (`Contended`). Proves the TOCTOU-spawned children collapse to one
    /// active updater.
    #[test]
    fn update_lock_is_single_flight() {
        let path =
            std::env::temp_dir().join(format!("tokenade-updlock-sf-{}.lock", std::process::id()));
        let _ = fs::remove_file(&path);

        let a = try_acquire_lock_at(&path);
        assert!(matches!(a, LockOutcome::Acquired(_)), "first must acquire");
        // Second attempt while the first guard is still alive → contended.
        assert!(
            matches!(try_acquire_lock_at(&path), LockOutcome::Contended),
            "second concurrent attempt must be Contended, not a parallel updater"
        );
        // A third, still contended — the guarantee is holder-count == 1.
        assert!(matches!(try_acquire_lock_at(&path), LockOutcome::Contended));

        drop(a); // holder exits → OS releases the flock
                 // Now a fresh updater can proceed (no stale-lock wedge).
        assert!(
            matches!(try_acquire_lock_at(&path), LockOutcome::Acquired(_)),
            "released lock must be re-acquirable"
        );
        let _ = fs::remove_file(&path);
    }

    /// A pre-existing lock FILE (left by a crashed updater) does NOT block a
    /// new update: an advisory flock is released by the OS on process death,
    /// so re-opening the same path and try-locking succeeds. Mirrors "crash →
    /// lock released by the OS".
    #[test]
    fn stale_lock_file_does_not_wedge_updates() {
        let path = std::env::temp_dir().join(format!(
            "tokenade-updlock-stale-{}.lock",
            std::process::id()
        ));
        // Simulate a corpse lock file with no live holder.
        fs::write(&path, b"").unwrap();
        assert!(
            matches!(try_acquire_lock_at(&path), LockOutcome::Acquired(_)),
            "a lock file with no live holder must be acquirable"
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn semver_gt_basic() {
        assert!(semver_gt("0.3.0", "0.2.9"));
        assert!(semver_gt("1.0.0", "0.99.99"));
        assert!(!semver_gt("0.2.0", "0.2.0"));
        assert!(!semver_gt("0.2.0", "0.2.1"));
        // Higher numeric triple wins even when one side has a prerelease.
        assert!(semver_gt("0.3.0-beta.1", "0.2.0"));
        // #956 — `v` prefix tolerated so a manifest like {"version":
        // "v1.0.0"} doesn't silently fail the major-version upgrade.
        // Without the strip, parse("v1.0.0") returned (0,0,0).
        assert!(semver_gt("v1.0.0", "0.99.99"));
        assert!(semver_gt("v0.3.0", "v0.2.0"));
        assert!(!semver_gt("v0.2.0", "0.2.0"));
        // Capital V also handled (some Windows tools emit it).
        assert!(semver_gt("V1.2.3", "1.2.2"));
    }

    /// #1026 — pre-fix the parser stripped `-X` and `+X` entirely, so
    /// `semver_gt("0.5.0", "0.5.0-rc1")` returned false and a client
    /// running a prerelease build (cargo install --branch, custom
    /// git-tag build) was permanently pinned with no auto-upgrade
    /// path to GA at the same triple. Per semver §11:
    ///   X.Y.Z-anything  <  X.Y.Z
    /// Lock in the prerelease-vs-release ordering and that
    /// build-metadata still doesn't affect precedence.
    #[test]
    fn semver_gt_prerelease_ordering() {
        // The headline regression: rc → GA at same triple MUST upgrade.
        assert!(
            semver_gt("0.5.0", "0.5.0-rc1"),
            "release must be greater than prerelease at same triple"
        );
        assert!(semver_gt("0.5.0", "0.5.0-rc.99"));
        assert!(semver_gt("0.5.0", "0.5.0-beta"));
        assert!(semver_gt("0.5.0", "0.5.0-alpha.1"));

        // Inverse: GA must NOT "downgrade" to a prerelease.
        assert!(!semver_gt("0.5.0-rc1", "0.5.0"));

        // Two releases at the same triple are equal, not greater.
        assert!(!semver_gt("0.5.0", "0.5.0"));

        // Two prereleases at the same triple — best-effort string
        // compare. rc2 > rc1, beta < rc lexicographically.
        assert!(semver_gt("0.5.0-rc2", "0.5.0-rc1"));
        assert!(!semver_gt("0.5.0-rc1", "0.5.0-rc2"));

        // Higher numeric triple dominates regardless of prerelease.
        assert!(semver_gt("0.6.0-alpha", "0.5.0"));
        assert!(semver_gt("0.6.0-alpha", "0.5.0-rc99"));
        assert!(!semver_gt("0.5.0-rc99", "0.6.0"));

        // Build metadata is stripped first, MUST NOT affect precedence.
        assert!(!semver_gt("0.5.0+build.1", "0.5.0+build.999"));
        assert!(!semver_gt("0.5.0+build.1", "0.5.0"));
        // Prerelease with build metadata: split on '+' first, then '-'.
        assert!(semver_gt("0.5.0", "0.5.0-rc1+build.1"));

        // `v` prefix still respected on both sides.
        assert!(semver_gt("v0.5.0", "v0.5.0-rc1"));
    }

    #[test]
    fn cache_roundtrip() {
        let _g = home_env_lock();
        let tmp =
            std::env::temp_dir().join(format!("tokenade-updater-cache-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let saved_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &tmp);
        let c = UpdateCheckCache {
            last_check: 12345,
            last_seen_version: "0.3.0".into(),
            last_error: None,
            last_refreshed_version: None,
        };
        save_cache(&c).unwrap();
        let r = load_cache();
        assert_eq!(r.last_check, 12345);
        assert_eq!(r.last_seen_version, "0.3.0");
        assert_eq!(r.last_refreshed_version, None);
        let _ = fs::remove_dir_all(&tmp);
        match saved_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    /// #1036 — `needs_post_upgrade_refresh` decides whether the CLI
    /// should re-run the silent post-upgrade refresh on this
    /// invocation. Verifies:
    ///   * fresh install (cache field None) → fires (so first-ever
    ///     invocation after install picks up the refresh path),
    ///   * cached version matches current binary → no-op,
    ///   * cached version != current → fires (the headline case:
    ///     auto-update swapped the binary, cache still has the prior
    ///     version, refresh must run),
    ///   * TOKENADE_NO_AUTO_UPDATE=1 → never fires regardless of
    ///     mismatch (user opted out of the whole update mechanism).
    ///
    /// `mark_post_upgrade_refresh_done` round-trips through the cache.
    #[test]
    fn post_upgrade_refresh_gate_branches() {
        let _g = home_env_lock();
        let tmp = std::env::temp_dir().join(format!(
            "tokenade-updater-refresh-gate-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let saved_home = std::env::var("HOME").ok();
        let saved_opt = std::env::var("TOKENADE_NO_AUTO_UPDATE").ok();
        std::env::set_var("HOME", &tmp);
        std::env::remove_var("TOKENADE_NO_AUTO_UPDATE");

        // Fresh state: cache file absent → load_cache returns Default
        // (last_refreshed_version = None) → needs_refresh = true.
        assert!(
            needs_post_upgrade_refresh("0.3.0"),
            "fresh install must trigger refresh"
        );

        // Mark done at 0.3.0 → subsequent check for the same version
        // is a no-op.
        mark_post_upgrade_refresh_done("0.3.0");
        assert!(
            !needs_post_upgrade_refresh("0.3.0"),
            "matching version must skip"
        );

        // Headline case: binary auto-updated, cache still at 0.3.0,
        // current binary is now 0.4.0 → fires.
        assert!(
            needs_post_upgrade_refresh("0.4.0"),
            "version mismatch must trigger refresh"
        );

        // Opt-out wins regardless of mismatch.
        std::env::set_var("TOKENADE_NO_AUTO_UPDATE", "1");
        assert!(
            !needs_post_upgrade_refresh("0.5.0"),
            "TOKENADE_NO_AUTO_UPDATE must suppress refresh"
        );
        std::env::remove_var("TOKENADE_NO_AUTO_UPDATE");

        // Round-trip after a mark — confirms the field actually
        // persisted to disk, not just to an in-memory copy.
        mark_post_upgrade_refresh_done("0.5.0");
        let cache = load_cache();
        assert_eq!(cache.last_refreshed_version.as_deref(), Some("0.5.0"));

        let _ = fs::remove_dir_all(&tmp);
        match saved_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        if let Some(v) = saved_opt {
            std::env::set_var("TOKENADE_NO_AUTO_UPDATE", v);
        }
    }

    #[test]
    fn read_single_tar_entry_extracts_payload() {
        // POSIX tar with a single 5-byte "hello" file.
        let mut tar = vec![0u8; 1024];
        // name
        tar[0..5].copy_from_slice(b"hello");
        // size 5 as 11-char octal + null
        tar[124..132].copy_from_slice(b"00000005");
        tar[132] = b' ';
        // typeflag '0' = regular file
        tar[156] = b'0';
        // checksum placeholder (8 spaces, then computed)
        for slot in &mut tar[148..156] {
            *slot = b' ';
        }
        let cksum: u32 = tar.iter().take(512).map(|&b| b as u32).sum();
        let cksum_str = format!("{:06o}\0 ", cksum);
        tar[148..156].copy_from_slice(cksum_str.as_bytes());
        // data
        tar[512..517].copy_from_slice(b"hello");
        let out = read_single_tar_entry(&tar).unwrap();
        assert_eq!(out, b"hello");
    }

    /// After the rename, the LIVE path is launched (`--version
    /// --no-auto-update`), not the probe's temp file. `check_and_swap` and
    /// `check_and_swap_now` both go through `extract_and_swap`, so an explicit
    /// upgrade AND the daily check heal the lectures with the NEW code.
    #[test]
    fn post_swap_live_binary_is_execd_to_heal_lectures() {
        let src = include_str!("updater/swap.rs");
        let after_rename = src
            .split("fs::rename(&tmp_path, exe_path)")
            .nth(1)
            .expect("atomic rename");
        assert!(
            after_rename.contains("Command::new(exe_path)"),
            "post-swap must exec the live path, not the tmp probe"
        );
        assert!(
            after_rename.contains("--no-auto-update"),
            "post-swap child must not re-enter the 24h updater"
        );
        assert!(
            after_rename.contains("--version"),
            "post-swap child must hit main()'s lecture heal"
        );
        let probe = src
            .split("PRE-swap health probe")
            .nth(1)
            .expect("probe comment")
            .split("fs::rename")
            .next()
            .expect("probe ends at rename");
        assert!(
            probe.contains("Command::new(&tmp_path)"),
            "pre-swap probe stays on the temp path"
        );
        assert!(
            !probe.contains("--no-auto-update"),
            "pre-swap probe must not be the heal launch"
        );
    }

    #[cfg(unix)]
    #[test]
    fn extract_and_swap_runs_live_binary_after_rename() {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("tokenade-swap-heal-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let live = dir.join("tokenade");
        fs::write(&live, b"old\n").unwrap();
        fs::set_permissions(&live, fs::Permissions::from_mode(0o755)).unwrap();

        let script =
            b"#!/bin/sh\nprintf '%s\\n' \"$0 $*\" >> \"$(dirname \"$0\")/spawned.log\"\nexit 0\n";
        let size = script.len();
        let data_blocks = size.div_ceil(512);
        let mut tar = vec![0u8; 512 + data_blocks * 512 + 1024];
        tar[..8].copy_from_slice(b"tokenade");
        let size_str = format!("{size:011o}");
        tar[124..135].copy_from_slice(size_str.as_bytes());
        tar[135] = 0;
        tar[156] = b'0';
        for slot in &mut tar[148..156] {
            *slot = b' ';
        }
        let cksum: u32 = tar.iter().take(512).map(|&b| b as u32).sum();
        let cksum_str = format!("{cksum:06o}\0 ");
        tar[148..156].copy_from_slice(cksum_str.as_bytes());
        tar[512..512 + size].copy_from_slice(script);
        let mut enc = GzEncoder::new(Vec::new(), Compression::fast());
        enc.write_all(&tar).unwrap();
        let tarball = enc.finish().unwrap();

        extract_and_swap(&tarball, &live).expect("swap");

        let log = fs::read_to_string(dir.join("spawned.log")).expect("spawned.log");
        let lines: Vec<&str> = log.lines().filter(|l| !l.is_empty()).collect();
        assert!(
            lines
                .iter()
                .any(|l| l.contains(".tokenade.new.") && l.ends_with(" --version")),
            "pre-swap probe on tmp:\n{log}"
        );
        assert!(
            lines.iter().any(|l| {
                l.contains("/tokenade --version --no-auto-update") && !l.contains(".tokenade.new.")
            }),
            "post-swap live heal launch:\n{log}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn manifest_roundtrip_keeps_key_order_stable() {
        // The TS emitter writes targets sorted; serde with BTreeMap also
        // serialises sorted. Verify we round-trip without surprises.
        let mut targets = std::collections::BTreeMap::new();
        targets.insert(
            "x86_64-unknown-linux-musl".to_string(),
            ManifestTarget {
                url: "https://x".into(),
                sha256: "abc".into(),
                size: 1,
            },
        );
        let m = ManifestV1 {
            v: 1,
            version: "0.3.0".into(),
            released_at: 0,
            min_supported_version: None,
            targets,
        };
        let s = serde_json::to_string(&m).unwrap();
        assert!(s.contains("\"v\":1"));
        let back: ManifestV1 = serde_json::from_str(&s).unwrap();
        assert_eq!(back.version, "0.3.0");
    }

    #[test]
    fn update_endpoint_falls_back_on_boolean_typo() {
        // #851 — `TOKENADE_UPDATE_ENDPOINT=1` would silently aim the
        // update check at `1/manifest.json`; the env_compat guard must
        // ignore the typo and the default endpoint must win.
        let prev = std::env::var("TOKENADE_UPDATE_ENDPOINT").ok();
        for v in ["1", "0", "true", "false", "on", "off"] {
            std::env::set_var("TOKENADE_UPDATE_ENDPOINT", v);
            assert_eq!(
                crate::env_compat::update_endpoint(),
                crate::env_compat::DEFAULT_UPDATE_ENDPOINT.trim_end_matches('/'),
                "typo {v:?} should fall back to default"
            );
        }
        // Empty / whitespace-only must also fall back — otherwise
        // an explicit `export TOKENADE_UPDATE_ENDPOINT=""` in CI
        // produces a 404 at root `/manifest.json`.
        for v in ["", " ", "\t", "\n"] {
            std::env::set_var("TOKENADE_UPDATE_ENDPOINT", v);
            assert_eq!(
                crate::env_compat::update_endpoint(),
                crate::env_compat::DEFAULT_UPDATE_ENDPOINT.trim_end_matches('/'),
                "empty/whitespace {v:?} should fall back to default"
            );
        }
        // Real values still override.
        std::env::set_var("TOKENADE_UPDATE_ENDPOINT", "https://staging.example/");
        assert_eq!(
            crate::env_compat::update_endpoint(),
            "https://staging.example"
        );
        std::env::remove_var("TOKENADE_UPDATE_ENDPOINT");
        if let Some(p) = prev {
            std::env::set_var("TOKENADE_UPDATE_ENDPOINT", p);
        }
    }

    #[test]
    fn no_target_msg_lists_available_targets() {
        let mut targets = std::collections::BTreeMap::new();
        targets.insert(
            "x86_64-unknown-linux-gnu".to_string(),
            ManifestTarget {
                url: "x".into(),
                sha256: "y".into(),
                size: 1,
            },
        );
        targets.insert(
            "aarch64-apple-darwin".to_string(),
            ManifestTarget {
                url: "x".into(),
                sha256: "y".into(),
                size: 1,
            },
        );
        let m = ManifestV1 {
            v: 1,
            version: "0.3.0".into(),
            released_at: 0,
            min_supported_version: None,
            targets,
        };
        let msg = no_target_msg(&m, "weird-target");
        assert!(msg.contains("weird-target"), "got: {msg}");
        // Sorted: aarch64-apple-darwin before x86_64-unknown-linux-gnu.
        assert!(
            msg.contains("aarch64-apple-darwin, x86_64-unknown-linux-gnu"),
            "got: {msg}"
        );
        assert!(
            !msg.contains("0 targets"),
            "non-empty manifest should NOT mention 0 targets: {msg}"
        );
    }

    #[test]
    fn no_target_msg_flags_empty_manifest() {
        let m = ManifestV1 {
            v: 1,
            version: "0.3.0".into(),
            released_at: 0,
            min_supported_version: None,
            targets: std::collections::BTreeMap::new(),
        };
        let msg = no_target_msg(&m, "x86_64-unknown-linux-gnu");
        assert!(msg.contains("0 targets"), "got: {msg}");
        assert!(msg.contains("mis-publish"), "got: {msg}");
    }

    /// #994 lock-in: `min_supported_blocks` returns `Some(err)` exactly
    /// when the manifest publishes a min_supported_version that is
    /// strictly newer than `current`. Absent / equal / older boundary
    /// → no block. Pre-fix the field was deserialised but NEVER
    /// inspected, so servers had no way to gate a breaking-change
    /// auto-upgrade.
    #[test]
    fn min_supported_blocks_only_when_current_is_below_boundary() {
        let mk = |min: Option<&str>| ManifestV1 {
            v: 1,
            version: "0.5.0".into(),
            released_at: 0,
            min_supported_version: min.map(String::from),
            targets: std::collections::BTreeMap::new(),
        };
        // Absent → no block.
        assert!(min_supported_blocks(&mk(None), "0.1.0").is_none());
        // Current below the boundary → block, message names both versions.
        let m = mk(Some("0.4.0"));
        let msg = min_supported_blocks(&m, "0.2.0").expect("boundary should block");
        assert!(msg.contains("0.5.0") && msg.contains("0.4.0") && msg.contains("0.2.0"));
        assert!(msg.contains("release notes"));
        // Current at the boundary → NOT blocked (>= is fine).
        assert!(min_supported_blocks(&mk(Some("0.4.0")), "0.4.0").is_none());
        // Current above the boundary → NOT blocked.
        assert!(min_supported_blocks(&mk(Some("0.4.0")), "0.4.5").is_none());
        // `v` prefix tolerated on both sides (mirrors semver_gt).
        assert!(min_supported_blocks(&mk(Some("v0.4.0")), "v0.2.0").is_some());
        assert!(min_supported_blocks(&mk(Some("v0.4.0")), "v0.4.0").is_none());
    }

    /// #1024 — `spawn_background_check` used
    /// `now.saturating_sub(last_check) < ttl_ms` as the freshness
    /// check, which silently treats clock-rewind (now < last_check)
    /// as "fresh" forever until the wall-clock catches back up.
    /// Asserts:
    /// (a) empty cache (last_check==0) is NOT fresh (initial bootstrap).
    /// (b) within TTL window → fresh.
    /// (c) past TTL window → NOT fresh.
    /// (d) clock-rewind (now < last_check) → NOT fresh (regression-
    ///     catching: pre-fix this returned true).
    #[test]
    fn ttl_cache_fresh_handles_clock_rewind_and_bootstrap() {
        let ttl: u64 = 24 * 60 * 60 * 1000; // 24h in ms
                                            // Base "now" comfortably larger than ttl so the math doesn't
                                            // underflow the test fixture itself.
        let now: u64 = 10 * ttl;

        // (a) Bootstrap — last_check==0 means we've never checked.
        assert!(!ttl_cache_fresh(now, 0, ttl), "empty cache must spawn");

        // (b) Within TTL.
        assert!(
            ttl_cache_fresh(now, now - 1000, ttl),
            "1 sec ago should be fresh"
        );
        assert!(
            ttl_cache_fresh(now, now - (ttl - 1), ttl),
            "just inside TTL must still be fresh"
        );

        // (c) At/past TTL boundary.
        assert!(
            !ttl_cache_fresh(now, now - ttl, ttl),
            "exactly at TTL boundary must NOT be fresh (= triggers spawn)"
        );
        assert!(
            !ttl_cache_fresh(now, now - (ttl + 1), ttl),
            "past TTL must NOT be fresh"
        );

        // (d) Clock-rewind: bogus future last_check. Pre-fix this
        // returned true → permanent skip. Now must return false.
        assert!(
            !ttl_cache_fresh(now, now + 1000, ttl),
            "clock-rewind (now < last_check) must invalidate cache, not skip forever"
        );
        assert!(
            !ttl_cache_fresh(now, now + ttl, ttl),
            "huge rewind also invalidates"
        );
        // Even tiny rewind invalidates — better to spawn one extra
        // check than to silently disable updates.
        assert!(
            !ttl_cache_fresh(now, now + 1, ttl),
            "1ms rewind must still invalidate"
        );
    }

    /// #1023 — `check_only` (powering `tokenade upgrade --check`)
    /// previously skipped the `manifest.v != 1` check that both swap
    /// paths enforce, so a future v=2 manifest would show "update
    /// available" in --check and then error during the real upgrade.
    /// Routing through `validate_manifest_basics` makes the contract
    /// explicit AND testable without HTTP mocking. Lock in that v=1
    /// passes and any other version is rejected with the same wording
    /// regardless of caller.
    #[test]
    fn validate_manifest_basics_accepts_v1_rejects_other() {
        let mk = |v: u32| ManifestV1 {
            v,
            version: "0.5.0".into(),
            released_at: 0,
            min_supported_version: None,
            targets: std::collections::BTreeMap::new(),
        };
        assert!(validate_manifest_basics(&mk(1)).is_ok());
        for bad in [0, 2, 3, 99] {
            let err = validate_manifest_basics(&mk(bad))
                .expect_err("non-v1 manifest must be rejected so --check and --swap agree");
            assert!(
                err.contains(&format!("v={bad}")),
                "error must name the offending version, got {err}"
            );
            assert!(
                err.contains("v=1"),
                "error must name the expected version, got {err}"
            );
        }
    }

    /// #998 lock-in: `save_cache` writes atomically (tmp + rename), so
    /// a partial-write scenario can never leave the file with corrupt
    /// JSON that would force `load_cache` to fall back to Default and
    /// re-network on the next invocation. Asserts the live cache file
    /// is whole JSON post-save (no `.json.tmp` masquerading as the
    /// real cache) and the round-trip preserves all fields.
    #[test]
    fn save_cache_is_atomic_and_roundtrip_stable() {
        let _g = home_env_lock();
        let tmp =
            std::env::temp_dir().join(format!("tokenade-updater-atomic-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let saved_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &tmp);

        let c = UpdateCheckCache {
            last_check: 9_999_999,
            last_seen_version: "0.4.2".into(),
            last_error: Some("network: temporary".into()),
            last_refreshed_version: None,
        };
        save_cache(&c).unwrap();

        // The .json.tmp must not survive past the rename — if it does,
        // a future caller could mistake it for the canonical cache.
        let canonical = update_cache_path();
        let tmp_path = canonical.with_extension("json.tmp");
        assert!(!tmp_path.exists(), "tmp file leaked past rename");
        assert!(canonical.is_file(), "canonical cache file missing");

        let back = load_cache();
        assert_eq!(back.last_check, 9_999_999);
        assert_eq!(back.last_seen_version, "0.4.2");
        assert_eq!(back.last_error.as_deref(), Some("network: temporary"));

        let _ = fs::remove_dir_all(&tmp);
        match saved_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    /// #991 lock-in: the public `opt_out_sentinel()` helper must point
    /// at the same file `is_opted_out()` checks. A rename of the
    /// filename literal that only updates the reader would
    /// silently break the `tokenade upgrade --off/--on` CLI; this test
    /// catches that drift at build time.
    #[test]
    fn opt_out_sentinel_file_name_is_stable() {
        let p = opt_out_sentinel();
        assert_eq!(
            p.file_name().and_then(|s| s.to_str()),
            Some("auto-update-off"),
            "the sentinel basename is a stable contract — \
             `cmd_upgrade --off/--on` writes/removes it by this exact name"
        );
    }
}
