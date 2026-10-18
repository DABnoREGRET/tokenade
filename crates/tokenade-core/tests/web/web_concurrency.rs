//! Concurrency guarantees (D11, D12) — exercised by dedicated tests, not a
//! battery. Tokenade runs as MANY processes at once (several agents, each its
//! own process, all against the same on-disk breaker file), and within one
//! process the search fan-out is concurrent. Both must be correct by design.
//!
//! Cross-process is approximated here by threads that each `open()` the lock
//! file separately: on POSIX `flock` is keyed to the open file description, so
//! two independent opens contend for real even inside one process — the same
//! mutual exclusion two processes would see. The substantive guarantees under
//! test are Risk #5 (atomic write + advisory lock + MERGE, no lost update, no
//! corruption) and Risk #6 (a per-engine permit that never lets one engine be
//! hit twice at once).

use tokenade_core::search::{
    load_health, persist_health_merge, select_and_lock, EngineHealth, Health, ENGINES, FANOUT,
    MIN_FANOUT,
};

/// Point the breaker state at a private temp dir for the whole test binary, so
/// racing writers never touch a developer's real `~/.tokenade`.
fn isolate_state_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("tokenade-cc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("TOKENADE_WEBSEARCH_STATE_DIR", &dir);
    dir
}

/// D12, in one sequential test (the state dir is selected by a process-global
/// env var, so the two phases must not run as separate parallel `#[test]`s that
/// would race each other's dir):
///   phase 1 — N writers each update a DISTINCT engine; every update must
///             survive the merge (no lost update, no corruption);
///   phase 2 — many writers hammer the SAME engine; the record stays consistent.
#[test]
fn cross_process_breaker_writes_are_atomic_locked_and_merged() {
    let _dir = isolate_state_dir();
    // Start clean.
    persist_health_merge(&Health::new(), &[]);

    const N: usize = 16;
    let mut handles = Vec::new();
    for i in 0..N {
        handles.push(std::thread::spawn(move || {
            let name = format!("eng{i}");
            let mut local = Health::new();
            local.insert(
                name.clone(),
                EngineHealth {
                    fails: i as u32,
                    open_until: 1_000 + i as u64,
                    last_used: i as u64,
                    ..Default::default()
                },
            );
            // Hammer it a few times to widen the race window.
            for _ in 0..5 {
                persist_health_merge(&local, std::slice::from_ref(&name));
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    let disk = load_health();
    for i in 0..N {
        let name = format!("eng{i}");
        let h = disk
            .get(&name)
            .unwrap_or_else(|| panic!("{name} was lost — a concurrent write stomped it"));
        assert_eq!(
            h.fails, i as u32,
            "{name} carries the wrong value — a merge dropped it"
        );
    }
    assert_eq!(disk.len(), N, "every engine's update must survive the race");

    // Phase 2: many writers hammer ONE engine. The last write under the lock
    // wins whole — never a half-written record — and the other N engines from
    // phase 1 must be untouched (merge, not overwrite).
    let mut handles = Vec::new();
    for round in 0..8u32 {
        handles.push(std::thread::spawn(move || {
            let mut local = Health::new();
            local.insert(
                "bing".to_string(),
                EngineHealth {
                    fails: round,
                    open_until: 0,
                    last_used: round as u64,
                    ..Default::default()
                },
            );
            persist_health_merge(&local, &["bing".to_string()]);
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    let disk = load_health();
    assert!(
        disk.contains_key("bing"),
        "the engine must survive concurrent same-key writes"
    );
    assert!(
        disk["bing"].fails < 8,
        "a present, well-formed record proves the file parsed"
    );
    assert_eq!(
        disk.len(),
        N + 1,
        "hammering one engine must not drop the other N — merge, not overwrite"
    );
}

/// Engine budget + cross-process lock + round-robin (the fleet rate-limiter).
#[test]
fn engine_budget_lock_and_round_robin() {
    let _dir = isolate_state_dir();
    // Long window so nothing resets mid-test; small budget so it's easy to hit.
    std::env::set_var("TOKENADE_ENGINE_WINDOW_SECS", "100000");
    std::env::set_var("TOKENADE_ENGINE_BUDGET", "1");
    let now = 1_000_000u64;

    // Fresh state: selection reserves up to FANOUT engines, at least the floor.
    let mut health = Health::new();
    let locks = select_and_lock(&mut health, now);
    assert!(
        locks.len() >= MIN_FANOUT,
        "must reach the floor on a fresh fleet: {}",
        locks.len()
    );
    assert!(
        locks.len() <= FANOUT,
        "must not exceed FANOUT: {}",
        locks.len()
    );
    // Each chosen engine had its window counter booked to the budget (1).
    for g in &locks {
        assert_eq!(health[g.engine().name()].window_count, 1);
    }

    // While those locks are HELD, a second selection cannot re-reserve them —
    // the cross-process lock (same process, separate FDs) blocks it. With
    // budget=1 the first selection also spent every engine it took, so a second
    // selection over the SAME snapshot can only take engines the first didn't.
    let held: std::collections::HashSet<&str> = locks.iter().map(|g| g.engine().name()).collect();
    let mut health2 = health.clone();
    let locks2 = select_and_lock(&mut health2, now);
    for g in &locks2 {
        assert!(
            !held.contains(g.engine().name()),
            "a held engine must not be re-locked: {}",
            g.engine().name()
        );
    }
    drop(locks);
    drop(locks2);

    // Budget exhaustion: mark EVERY engine at budget in the current window →
    // nothing eligible → empty selection (caller then falls back to native).
    let mut spent = Health::new();
    for e in ENGINES {
        spent.insert(
            e.name().to_string(),
            EngineHealth {
                window_start: now,
                window_count: 99,
                ..Default::default()
            },
        );
    }
    assert!(
        select_and_lock(&mut spent, now).is_empty(),
        "all engines out of budget must yield no selection (→ native fallback)"
    );

    // Min-interval (1s default): budget + breaker fine, but every engine was
    // called <1s ago → the selector WAITS out the rest of the gap and then
    // takes them (a cadence is "not yet", never "no" — a batch's third query
    // was refused `all-engines: rate-limited` 400 ms before three healthy
    // engines became usable, 2026-10-03); the same set >1s ago → no wait.
    let now_ms = tokenade_core::search::now_millis();
    let mut recent = Health::new();
    for e in ENGINES {
        recent.insert(
            e.name().to_string(),
            EngineHealth {
                window_start: now,
                last_used_ms: now_ms.saturating_sub(400),
                ..Default::default()
            },
        );
    }
    let t0 = std::time::Instant::now();
    let waited = select_and_lock(&mut recent, now);
    assert!(
        waited.len() >= MIN_FANOUT,
        "engines called <1s ago are a wait, not a refusal: {}",
        waited.len()
    );
    assert!(
        t0.elapsed() >= std::time::Duration::from_millis(400),
        "the gap must still be honoured: took {:?}",
        t0.elapsed()
    );
    drop(waited);
    let mut aged = Health::new();
    for e in ENGINES {
        aged.insert(
            e.name().to_string(),
            EngineHealth {
                window_start: now,
                last_used_ms: now_ms.saturating_sub(2_000),
                ..Default::default()
            },
        );
    }
    assert!(
        !select_and_lock(&mut aged, now).is_empty(),
        "engines last called >1s ago must be eligible again"
    );

    std::env::remove_var("TOKENADE_ENGINE_WINDOW_SECS");
    std::env::remove_var("TOKENADE_ENGINE_BUDGET");
}
