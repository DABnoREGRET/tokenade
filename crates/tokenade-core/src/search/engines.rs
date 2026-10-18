//! **Which engine to query, and how many at once.**
//!
//! An engine queried too often cuts us off, and two tokenade processes hitting
//! the same one at the same time get there twice as fast — hence a lock per
//! engine, a rotation, and a concurrency that depends on the BATCH rather than
//! on a constant. Each engine's health is persisted: without memory, every
//! process repeats the same bad experience.

use super::*;

/// A held cross-process reservation on ONE engine: no other tokenade process
/// (agent) fetches this engine while the guard lives. Releasing it (drop, or the
/// process dying — the OS drops the flock either way) frees the engine. This is
/// what turns fleet-wide engine use into a round-robin: an engine another agent
/// is hitting right now is SKIPPED, not queued behind it.
pub struct EngineLock {
    _file: std::fs::File,
    engine: Engine,
}

impl EngineLock {
    pub fn engine(&self) -> Engine {
        self.engine
    }
}

pub(super) fn engine_lock_path(e: Engine) -> std::path::PathBuf {
    health_path().with_file_name(format!("engine-{}.lock", e.name()))
}

/// Try to reserve `e` WITHOUT blocking. `None` when another process holds it —
/// the caller then round-robins to a different engine rather than waiting.
pub(super) fn try_lock_engine(e: Engine) -> Option<EngineLock> {
    use fs2::FileExt;
    let p = engine_lock_path(e);
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // `truncate(false)` stated, not defaulted: this file exists ONLY to carry an
    // flock. Nothing is ever written to it, and another process may be holding
    // the lock on it right now — clobbering its length would be a side effect
    // with no purpose.
    let f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&p)
        .ok()?;
    f.try_lock_exclusive().ok()?;
    Some(EngineLock {
        _file: f,
        engine: e,
    })
}

/// The engines a query could use RIGHT NOW: breaker CLOSED, budget left this
/// window, and at least `min_interval` since the last call to that engine.
/// Ordered least-recently-used first, priority breaking ties. Elapsed windows
/// are reset in place, so the caller persists whatever it touched.
///
/// One declarer, two readers: [`select_and_lock`] takes locks over this list,
/// and [`batch_concurrency`] counts it to decide how many queries of a batch
/// may be in flight. Counting it any other way would let the two disagree about
/// what "usable engine" means.
pub(super) fn eligible_engines(health: &mut Health, now: u64) -> Vec<(u64, usize, Engine)> {
    let window = window_secs();
    let now_ms = now_millis();
    let min_iv = min_interval_ms();
    let mut out: Vec<(u64, usize, Engine)> = ENGINES
        .iter()
        .enumerate()
        .filter_map(|(prio, &e)| {
            let h = health.entry(e.name().to_string()).or_default();
            if now.saturating_sub(h.window_start) >= window {
                h.window_start = now;
                h.window_count = 0;
            }
            let has_budget = h.window_count < engine_budget(e);
            let spaced = now_ms.saturating_sub(h.last_used_ms) >= min_iv;
            (h.open_until <= now && has_budget && spaced).then_some((h.last_used, prio, e))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    out
}

/// How long until an engine held back ONLY by the minimum gap becomes usable:
/// breaker closed, budget left (or its window elapsed), last call too recent.
/// `None` when no engine is waiting on the gap alone — an open breaker or a
/// spent budget is a real "no", and waiting a second would not change it.
pub(super) fn spacing_wait_ms(health: &Health, now: u64, now_ms: u64) -> Option<u64> {
    let window = window_secs();
    let min_iv = min_interval_ms();
    ENGINES
        .iter()
        .filter_map(|&e| {
            let h = health.get(e.name()).cloned().unwrap_or_default();
            let fresh_window = now.saturating_sub(h.window_start) >= window;
            let has_budget = fresh_window || h.window_count < engine_budget(e);
            let since = now_ms.saturating_sub(h.last_used_ms);
            (h.open_until <= now && has_budget && since < min_iv).then(|| min_iv - since)
        })
        .min()
}

/// How many queries of ONE batch may be in flight at once, given how many
/// engines are usable right now.
///
/// Each in-flight query reserves up to [`FANOUT`] engines for the WHOLE of its
/// fetch — seconds, not milliseconds — and [`select_and_lock`] never queues
/// behind a held engine: it round-robins, spins for about a second and a half,
/// then gives up. So a batch wider than `eligible / FANOUT` starves its own
/// tail, and the tail is told `all-engines: rate-limited`, which names a
/// fleet-wide wall when the truth is that the caller's own siblings hold every
/// engine. Measured on the real selector with a 3 s fetch: a batch of 4 loses
/// 1 query, of 5 loses 2, of 6 loses 3, while a batch of 3 loses none.
///
/// Never zero: a query with nothing eligible must still be ATTEMPTED and get
/// the real refusal, rather than be dropped by arithmetic.
pub fn concurrency_for(eligible: usize, queries: usize) -> usize {
    (eligible / FANOUT).max(1).min(queries.max(1))
}

/// [`concurrency_for`] against a GIVEN engine state. Injected, so a test can
/// pin the arithmetic against a known set of breakers without reaching for the
/// state directory — a path that comes from the environment, and the
/// environment is process-wide: a sibling test mutating any variable at the
/// wrong moment sends this read to the real health file and the assertion
/// measures the machine instead of the fixture.
pub fn batch_concurrency_from(health: &mut Health, now: u64, queries: usize) -> usize {
    concurrency_for(eligible_engines(health, now).len(), queries)
}

/// [`batch_concurrency_from`] against the engine health on disk right now.
pub fn batch_concurrency(queries: usize) -> usize {
    batch_concurrency_from(
        &mut load_health(),
        crate::platform::now_unix_secs(),
        queries,
    )
}

/// Choose AND cross-process-reserve the engines to fan out for this query — the
/// round-robin core. Returns up to [`FANOUT`] held engine locks, aiming for at
/// least [`MIN_FANOUT`]. An engine is eligible when its breaker is CLOSED, it
/// has REMAINING budget this window, and no other process holds it. Ordered
/// least-recently-used first (load spreads over time) with a per-process
/// rotation offset so two agents sharing the same view PREFER different engines
/// instead of contending on the top of the list.
///
/// Deadlock-free by construction: it only ever *tries* locks (never blocks while
/// holding one), so N processes racing the same engines can never wait on each
/// other. Under saturation it spins briefly to reach the floor, then proceeds
/// with what it has. Increments each chosen engine's window counter + last_used
/// in `health`; the caller persists that under the file lock before releasing.
pub fn select_and_lock(health: &mut Health, now: u64) -> Vec<EngineLock> {
    let mut candidates = eligible_engines(health, now);
    // The minimum gap is a CADENCE: it says "not yet", never "no". A query
    // that found nothing spaced was refused `all-engines: rate-limited` while
    // three healthy engines were a few hundred millis away from usable — the
    // third query of a batch whose second had just used them (2026-10-03).
    if candidates.len() < MIN_FANOUT {
        if let Some(wait) = spacing_wait_ms(health, now, now_millis()) {
            std::thread::sleep(std::time::Duration::from_millis(wait));
            candidates = eligible_engines(health, now);
        }
    }
    let now_ms = now_millis();
    if !candidates.is_empty() {
        let off = (std::process::id() as usize) % candidates.len();
        candidates.rotate_left(off);
    }

    let mut chosen: Vec<EngineLock> = Vec::new();
    // Phase 1 (non-blocking): take up to FANOUT engines nobody else is using.
    for &(_, _, e) in &candidates {
        if chosen.len() >= FANOUT {
            break;
        }
        if let Some(g) = try_lock_engine(e) {
            chosen.push(g);
        }
    }
    // Phase 2: if we're under the floor AND the shortfall is due to LOCKS (not
    // to too few eligible engines), spin briefly — engines free as other agents'
    // fetches finish. Try-lock only, so no deadlock is possible.
    if candidates.len() >= MIN_FANOUT {
        let mut spins = 0;
        while chosen.len() < MIN_FANOUT && spins < 20 {
            std::thread::sleep(std::time::Duration::from_millis(
                50 + (std::process::id() as u64 % 50),
            ));
            for &(_, _, e) in &candidates {
                if chosen.len() >= MIN_FANOUT {
                    break;
                }
                if chosen.iter().any(|g| g.engine == e) {
                    continue;
                }
                if let Some(g) = try_lock_engine(e) {
                    chosen.push(g);
                }
            }
            spins += 1;
        }
    }
    // Book the usage on the chosen engines (durable once the caller persists).
    for g in &chosen {
        let h = health.entry(g.engine.name().to_string()).or_default();
        h.window_count = h.window_count.saturating_add(1);
        h.last_used = now;
        h.last_used_ms = now_ms;
    }
    chosen
}

/// Fan out CONCURRENTLY over a FIXED engine set and fold the answers into an
/// Outcome, updating each engine's breaker in `health`. Shared by the pure
/// [`run`] (rotation-only) and [`run_search`] (budget + cross-process lock).
pub(super) fn fetch_and_update(
    query: &str,
    batch: &[Engine],
    health: &mut Health,
    now: u64,
) -> Outcome {
    if batch.is_empty() {
        return Outcome {
            results: Vec::new(),
            used: Vec::new(),
            refused: Vec::new(),
        };
    }
    // `Builder::spawn` (not `thread::spawn`), because thread creation can FAIL
    // under resource exhaustion (EAGAIN — the fork-bomb / thread-starvation
    // scenario this project has actually hit). `thread::spawn` PANICS there,
    // which would crash the whole search; on a failed spawn we fall back to
    // running that engine INLINE — serial but correct — so the fan-out degrades
    // instead of dying. `eng` is carried alongside so a join failure (a panicked
    // worker) can still be attributed and typed Unreachable.
    let handles: Vec<(Engine, Option<std::thread::JoinHandle<_>>)> = batch
        .iter()
        .copied()
        .map(|eng| {
            let q = query.to_string();
            let h = std::thread::Builder::new()
                .spawn(move || (eng, fetch_and_validate(eng, &q)))
                .ok();
            (eng, h)
        })
        .collect();
    let answers: Vec<(Engine, Result<Vec<serp::Result>, Refusal>)> = handles
        .into_iter()
        .map(|(eng, h)| match h {
            Some(handle) => handle.join().unwrap_or((eng, Err(Refusal::Unreachable))),
            None => (eng, fetch_and_validate(eng, query)),
        })
        .collect();

    let mut used = Vec::new();
    let mut refused = Vec::new();
    let mut per_engine: Vec<Vec<serp::Result>> = Vec::new();
    for (eng, answer) in answers {
        let h = health.entry(eng.name().to_string()).or_default();
        h.last_used = now;
        match answer {
            Ok(hits) => {
                h.fails = 0;
                h.open_until = 0;
                used.push(eng.name());
                per_engine.push(hits);
            }
            Err(why) => {
                h.fails += 1;
                if h.fails >= CB_MAX_FAILS {
                    h.open_until = now + cb_duration(why);
                }
                refused.push((eng.name(), why));
                crate::debug_log::log(
                    "search",
                    "engine_refused",
                    serde_json::json!({ "engine": eng.name(), "why": why.as_str() }),
                );
            }
        }
    }
    Outcome {
        results: merge_and_rank(&per_engine, query),
        used,
        refused,
    }
}

/// The healthiest [`FANOUT`] engines: closed breakers only, least-recently-used
/// first so load spreads, ties broken by [`ENGINES`] priority.
pub fn rotation_order(health: &Health, now: u64) -> Vec<Engine> {
    let mut open: Vec<(u64, usize, Engine)> = ENGINES
        .iter()
        .enumerate()
        .filter_map(|(prio, &e)| {
            let h = health.get(e.name()).copied().unwrap_or_default();
            (h.open_until <= now).then_some((h.last_used, prio, e))
        })
        .collect();
    open.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    open.into_iter().take(FANOUT).map(|(_, _, e)| e).collect()
}

/// Where the breaker state lives. SHARED by the hook and `tokenade search`
/// on purpose: a wall one channel hits is a wall the other should already know
/// about, and two files would mean the CLI cheerfully re-hammering an engine
/// the hook just found to be throttling us.
///
/// Honours `TOKENADE_WEBSEARCH_STATE_DIR` so tests never touch the real one.
pub fn health_path() -> std::path::PathBuf {
    let dir = match std::env::var_os("TOKENADE_WEBSEARCH_STATE_DIR") {
        Some(p) => std::path::PathBuf::from(p),
        None => crate::platform::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".tokenade")
            .join("sessions"),
    };
    dir.join("websearch-engines.json")
}

pub fn load_health() -> Health {
    std::fs::read_to_string(health_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Atomic write: temp file in the SAME directory, fsync, then rename over the
/// target (rename is atomic on one filesystem). Never a partial file, even if N
/// processes write at once — the loser's rename simply wins or loses whole.
pub(super) fn atomic_write_health(h: &Health) {
    let p = health_path();
    let Some(dir) = p.parent() else { return };
    let _ = std::fs::create_dir_all(dir);
    let Ok(s) = serde_json::to_string(h) else {
        return;
    };
    let tmp = dir.join(format!("websearch-engines.{}.tmp", std::process::id()));
    if let Ok(mut f) = std::fs::File::create(&tmp) {
        use std::io::Write;
        if f.write_all(s.as_bytes()).is_ok() {
            let _ = f.sync_all();
            if std::fs::rename(&tmp, &p).is_err() {
                let _ = std::fs::remove_file(&tmp);
            }
        } else {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

/// The breaker lock file. A separate advisory-lock target so the JSON itself is
/// only ever replaced by an atomic rename, never held open under the lock.
pub(super) fn health_lock_path() -> std::path::PathBuf {
    health_path().with_extension("json.lock")
}

/// Persist ONLY the engines this process touched, MERGING with whatever other
/// processes wrote since (D12). Under an exclusive advisory lock with a TINY
/// critical section: acquire, reload disk, overwrite only the engines we used,
/// atomic-write, release. An engine another process updated concurrently
/// survives because we never write the whole map — only our own keys.
pub fn persist_health_merge(local: &Health, touched: &[String]) {
    use fs2::FileExt;
    if let Some(dir) = health_path().parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Same as `try_lock_engine`: a pure flock carrier, never written to, and
    // possibly held by another process right now. `truncate(false)` says so.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(health_lock_path());
    // Best-effort: if the lock cannot be opened we still write, rather than
    // losing a breaker update — a rare race is better than a stuck-open engine.
    let held = lock.as_ref().ok().filter(|f| f.lock_exclusive().is_ok());
    let mut disk = load_health();
    for name in touched {
        if let Some(h) = local.get(name) {
            disk.insert(name.clone(), *h);
        }
    }
    atomic_write_health(&disk);
    if let Some(f) = held {
        let _ = f.unlock();
    }
}
