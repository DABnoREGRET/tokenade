//! **The ledger as a FILE: locks, rotation, rollups, migrations.**
//!
//! An `append` is atomic below `PIPE_BUF`, and not beyond. A file that grows
//! forever becomes unreadable. An earlier version wrote elsewhere. Everything
//! that answers "where are the bytes, and how not to lose them" is here — and
//! nothing here knows what a row MEANS.

use super::*;

/// The file whose lock stands for `path` — a SIDECAR, never the ledger itself.
///
/// Windows file locks are MANDATORY. `LockFileEx` on `gain.jsonl` blocks every
/// OTHER handle to it, so the read-filter-write-rename these locks exist to
/// protect could not read its own file: os error 33, "another process has
/// locked a portion of the file". MEASURED on the 1.1.9 release build's
/// Windows leg, on all seven rotation and rollup paths at once, the moment
/// locking started working there at all. `flock` is advisory, so locking the
/// data file is harmless on Unix and macOS and the whole class was invisible.
///
/// Every participant must agree on ONE lock object or they stop excluding each
/// other, so `record`, the rotation, the rollup and the prune all lock this.
/// The suffix keeps it out of every `*.jsonl` reader in the tree.
pub(super) fn lock_path_for(path: &std::path::Path) -> std::path::PathBuf {
    let mut p = path.as_os_str().to_os_string();
    p.push(".lock");
    std::path::PathBuf::from(p)
}

/// Rewrite the ledger keeping only records whose `ts` is within `window_secs`
/// of now. Records past the window are discarded. The rotated file is
/// written atomically (write tmp + rename).
pub fn rotate_if_oversized(path: &std::path::Path, window_secs: u64) -> std::io::Result<()> {
    // #1035 — cross-process flock for the entire read-filter-write-rename
    // cycle. Pre-fix, `record()` released its flock between the write and
    // the rotation call (lines 197-212), and `rotate_if_oversized` held
    // no lock during its read-filter-write-rename. A concurrent process
    // writing between rotation's read and rename had its record silently
    // dropped — the same class of bug as #500 (write-time data loss
    // from truncated trailing lines) but multi-process. After #1033 wired
    // the M9 reporter, each lost rotation-race record = one lost usage
    // event = silent under-billing on the paid product.
    //
    // The lock is advisory (fs2::FileExt::lock_exclusive → flock LOCK_EX
    // on Unix, LockFileEx on Windows); every caller that mutates
    // gain.jsonl goes through `record()` or this function, so all writers
    // respect it. The handle is dropped at function exit; the OS releases
    // the flock with it. Process-local in-process serialisation stays
    // the caller's responsibility (record() holds write_lock() across
    // its call to this function; tests run single-threaded).
    let lock_file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(lock_path_for(path))?;
    if !crate::platform::lock_briefly(&lock_file) {
        // Someone else holds it. This whole function is a read-filter-write-
        // rename, so proceeding unlocked is exactly the race #1035 closed: a
        // record written between our read and our rename disappears. The
        // rotation is opportunistic — the next write checks again — so the
        // safe answer to "cannot lock" is to not rotate.
        return Ok(());
    }
    let now = crate::platform::now_unix_secs();
    let cutoff = now.saturating_sub(window_secs);
    let content = fs::read_to_string(path)?;
    let mut kept = String::with_capacity(content.len());
    let mut dropped_lines: Vec<String> = Vec::new();
    for line in content.lines() {
        // #500 — keep malformed lines verbatim. The previous behaviour
        // treated parse failures as `ts = 0`, which always fell below
        // `cutoff` and got SILENTLY DROPPED. Real-world cause: a
        // process killed mid-write leaves a truncated trailing line;
        // or an older record predates a schema field. Either way we
        // have no grounds to delete content we can't parse.
        let parsed = serde_json::from_str::<serde_json::Value>(line).ok();
        let ts_opt = parsed.as_ref().and_then(|v| v["ts"].as_u64());
        let in_window = match ts_opt {
            Some(ts) => ts >= cutoff,
            None => true, // unparseable or missing ts → preserve
        };
        if in_window {
            kept.push_str(line);
            kept.push('\n');
        } else {
            dropped_lines.push(line.to_string());
        }
    }
    if dropped_lines.is_empty() {
        // Nothing aged out — the threshold was tripped but everything's
        // still in window. To avoid being called every record once we hit
        // the threshold, drop the oldest 25 % as a fallback.
        let lines: Vec<&str> = content.lines().collect();
        // Guard against degenerate cases: empty file or ≤4 lines (where
        // dropping 25 % would drop everything or rotate nothing useful).
        if lines.len() <= 4 {
            return Ok(()); // nothing to do; let the file grow a bit more
        }
        let keep_from = lines.len() / 4;
        dropped_lines = lines[..keep_from].iter().map(|s| s.to_string()).collect();
        kept = lines[keep_from..].join("\n");
        kept.push('\n');
    }
    // #1042 — BEFORE the aged-out rows are discarded, fold their savings
    // into the durable per-day rollup. The dashboard reads ledger + rollup,
    // so lifetime totals survive rotation. Without this every rotation (and
    // every historical `.bak` reset) silently zeroed the "all-time" figure —
    // the recurring "my saved tokens reset" the user reported. Best-effort:
    // Folded FIRST, dropped only once folded: a rollup that could not be
    // locked or written used to be shrugged off ("rebuilt on the next
    // write") while the rotation went on and deleted the very rows it had
    // failed to fold — savings gone from every lifetime total. Not rotating
    // is safe: the next write checks again.
    if !fold_lines_into_rollup(&dropped_lines, &rollup_path_for(path)) {
        return Ok(());
    }
    crate::platform::write_atomic_600(path, kept.as_bytes())
}

/// Path of the durable per-day rollup that mirrors a ledger file:
/// `gain.jsonl` → `gain.rollup.jsonl`, `bypass_lost.jsonl` →
/// `bypass_lost.rollup.jsonl`. The rollup is NEVER rotated — it holds one
/// aggregated row per (UTC-day, op) and is why lifetime totals survive
/// ledger rotation. Rows carry `"rollup": true` and a `"rollup_calls"`
/// cardinality so read-side aggregators can honour the collapsed count.
pub fn rollup_path_for(ledger: &std::path::Path) -> PathBuf {
    let name = ledger
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("gain.jsonl");
    let rolled = name
        .strip_suffix(".jsonl")
        .map(|s| format!("{s}.rollup.jsonl"))
        .unwrap_or_else(|| format!("{name}.rollup.jsonl"));
    ledger.with_file_name(rolled)
}

/// Read a ledger together with its durable rollup, returned as one JSONL
/// string (current rows first, then rolled-up day buckets). #1042 — every
/// dashboard surface (text / json / web) must aggregate over BOTH so the
/// lifetime figure survives ledger rotation; reading only `gain.jsonl`
/// collapsed Paul's 180M lifetime to the ~350k still inside the 30-day
/// window. Rollup rows carry `rollup_calls` so read-side aggregators keep
/// the collapsed cardinality (and `ts` = UTC-day-start, so time filters
/// still bucket them). Caller should run `migrate_legacy_backups_into_rollup`
/// once before this to fold any pre-#1042 `.bak` files into the rollup.
pub fn ledger_with_rollup(ledger: &std::path::Path) -> String {
    let current = mark_superseded_lines(&std::fs::read_to_string(ledger).unwrap_or_default());
    let rolled = std::fs::read_to_string(rollup_path_for(ledger)).unwrap_or_default();
    if rolled.is_empty() {
        return current;
    }
    if current.is_empty() {
        return rolled;
    }
    format!("{}\n{}", current.trim_end_matches('\n'), rolled)
}

/// The stages that REPLACE, within the same call, the already-compacted
/// output: the stash, and the two dedups. Each writes its compacted→delivered
/// row AFTER the compactor's raw→compacted row (`wrap.rs`: the two intervals
/// tile, so `saved` adds up exactly). But `after` does not add up: the
/// compactor's output was never delivered.
pub const SUPERSEDING_OPS: [&str; 3] = ["disclose:stash", "exec_dedup", "cmd_dedup"];

/// What a row really put in front of the model: `delivered` when the next
/// stage replaced it, otherwise `after`. Measured 2026-10-02 on the raw
/// ledger: 218 013 "delivered" tokens that never were, 168 378 of them for
/// `auto:json` (a polling loop served by `exec_dedup`), which lifted it to
/// 3rd in the ranking. `before`/`after`/`saved` stay intact: they carry the
/// invariant `saved ≤ before−after`.
pub fn delivered_of(v: &serde_json::Value) -> u64 {
    v.get("delivered")
        .and_then(|x| x.as_u64())
        .unwrap_or_else(|| v["after"].as_u64().unwrap_or(0))
}

/// Sets `delivered: 0` on every compactor row that a [`SUPERSEDING_OPS`] row
/// of the SAME call replaced — same `sess`, same `label`, timestamps at most
/// 2 s apart (two writes from one process can straddle a second). Returns the
/// number of rows marked.
pub fn mark_superseded(rows: &mut [serde_json::Value]) -> usize {
    let key = |v: &serde_json::Value| {
        (
            v["sess"].as_str().map(str::to_string),
            v["label"].as_str().map(str::to_string),
        )
    };
    let mut downstream: std::collections::HashMap<_, Vec<u64>> = Default::default();
    for v in rows.iter() {
        let op = v["op"].as_str().unwrap_or("");
        if SUPERSEDING_OPS.contains(&op) && v["label"].is_string() {
            downstream
                .entry(key(v))
                .or_default()
                .push(v["ts"].as_u64().unwrap_or(0));
        }
    }
    if downstream.is_empty() {
        return 0;
    }
    let mut marked = 0;
    for v in rows.iter_mut() {
        let op = v["op"].as_str().unwrap_or("");
        if SUPERSEDING_OPS.contains(&op) || v["rollup"].as_bool() == Some(true) {
            continue;
        }
        let ts = v["ts"].as_u64().unwrap_or(0);
        let hit = downstream
            .get(&key(v))
            .is_some_and(|tss| tss.iter().any(|t| t.abs_diff(ts) <= 2));
        if hit && v["after"].as_u64().unwrap_or(0) > 0 {
            v["delivered"] = serde_json::Value::from(0u64);
            marked += 1;
        }
    }
    marked
}

/// [`mark_superseded`] over a JSONL text: only the marked rows are rewritten,
/// the others (unreadable ones included) pass through as is.
pub(super) fn mark_superseded_lines(text: &str) -> String {
    let mut rows: Vec<(usize, serde_json::Value)> = text
        .lines()
        .enumerate()
        .filter_map(|(i, l)| serde_json::from_str(l).ok().map(|v| (i, v)))
        .collect();
    let mut values: Vec<serde_json::Value> = rows.iter().map(|(_, v)| v.clone()).collect();
    if mark_superseded(&mut values) == 0 {
        return text.to_string();
    }
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    for ((i, old), new) in rows.iter_mut().zip(values) {
        if *old != new {
            lines[*i] = new.to_string();
        }
    }
    let mut out = lines.join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Aggregate `lines` (raw ledger JSONL rows about to be discarded) into the
/// durable rollup at `rollup_path`, summing `before`/`after`/calls per
/// (UTC-day-start, op) bucket and merging with rows already present.
/// Best-effort: unparseable lines are skipped; IO errors are swallowed
/// (telemetry must never break the caller). Takes a cross-process flock on
/// the rollup file for the read-merge-write-rename cycle. Does NOT take the
/// process `write_lock()`: its only callers already hold it (`record()` →
/// `rotate_if_oversized` → here) or run single-threaded at dashboard start
/// (the migration), and the std `Mutex` is non-reentrant, so re-acquiring
/// it would deadlock the very rotation path it's meant to protect.
/// What a rollup bucket puts where a row carries an identity: it aggregates
/// several, so it has none.
pub const ROLLED_UP: &str = "(rolled-up)";

/// The COMPACTOR NAME of a ledger row, or `None` if it names none.
///
/// Three shapes, one rule. `cmd` names the PROGRAM whose output was folded: it
/// only exists on a row born from a COMMAND, and it is the name, not the label
/// repeating the command line. `compactor` is what a rollup bucket keeps of
/// what it folded. `label` carries the compactor of a row born from CONTENT —
/// a `read`, an MCP response, a page — except when it is the sentinel, which
/// is nobody's name.
///
/// Public because the bench asked the same question on its side: two copies of
/// one rule drift as soon as a shape is added.
pub fn compactor_of(v: &serde_json::Value) -> Option<&str> {
    for key in ["cmd", "compactor", "label"] {
        if let Some(name) = v[key].as_str().filter(|n| !n.is_empty() && *n != ROLLED_UP) {
            return Some(name);
        }
    }
    None
}

/// `true` once `lines` are durably in the rollup (or there was nothing to
/// fold) — the caller may then discard them, and only then.
pub(super) fn fold_lines_into_rollup(lines: &[String], rollup_path: &std::path::Path) -> bool {
    if lines.is_empty() {
        return true;
    }
    use std::collections::BTreeMap;
    #[derive(Default, Clone, Copy)]
    struct Bucket {
        before: u64,
        after: u64,
        delivered: u64,
        calls: u64,
        realistic: u64,
    }
    // Bucket key carries the ACCOUNTING dimensions, not just (day, op):
    // rolled-up rows used to lose `class`/`io`/`estimated`, so the first
    // rotation of gain_estimated.jsonl silently dropped the whole rolled
    // history out of the weighted buckets (output x5 / compounding /
    // overhead all read `class`) — the user's lifetime figure would fall
    // off a cliff. Old class-less rollup rows keep folding into the
    // empty-dimension bucket, so re-merges stay additive.
    // …and the COMPACTOR NAME with them. Without it, a rotation makes
    // everything it folds anonymous: `label` becomes `(rolled-up)` and `cmd`
    // disappears, so nothing says any more WHICH compactor of a family
    // delivered those bytes. MEASURED 2026-09-07: 44 411 043 of the 52 690 500
    // bytes still delivered — 84% — were in rollup buckets and belonged to no
    // nameable compactor, which blinds the value ranking exactly where there
    // is the most to gain. The name enters the key (measured: 370 buckets
    // become 887, ×2.4) and comes back out under `compactor` — a NEW field, so
    // that no reader of `label` changes behaviour.
    type RollKey = (u64, String, String, String, bool, String, bool);
    fn fold_one(buckets: &mut BTreeMap<RollKey, Bucket>, v: &serde_json::Value) {
        let ts = v["ts"].as_u64().unwrap_or(0);
        let day_ts = (ts / 86_400) * 86_400;
        let op = v["op"]
            .as_str()
            .or_else(|| v["filter"].as_str())
            .unwrap_or("?")
            .to_string();
        let class = v["class"].as_str().unwrap_or("").to_string();
        let io = v["io"].as_str().unwrap_or("").to_string();
        let est = v["estimated"].as_bool() == Some(true) || v["est"].as_bool() == Some(true);
        let before = v["before"].as_u64().unwrap_or(0);
        let after = v["after"].as_u64().unwrap_or(0);
        // pre-aggregated rollup rows carry their own collapsed cardinality.
        let calls = v["rollup_calls"].as_u64().unwrap_or(1);
        // Discount per-event BEFORE summing — the harness cap applies per
        // event, so it can't be reconstructed from a bucket's summed `before`.
        let realistic = realistic_of_row(v);
        // Same rule as everywhere else: see `compactor_of`.
        let compactor = compactor_of(v).unwrap_or("").to_string();
        // `billed` too: a rolled image row must not be converted again.
        let billed = v["billed"] == true;
        let e = buckets
            .entry((day_ts, op, class, io, est, compactor, billed))
            .or_default();
        e.before = e.before.saturating_add(before);
        e.after = e.after.saturating_add(after);
        e.delivered = e.delivered.saturating_add(delivered_of(v));
        e.calls = e.calls.saturating_add(calls);
        e.realistic = e.realistic.saturating_add(realistic);
    }

    if let Some(dir) = rollup_path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let lock_file = match fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(lock_path_for(rollup_path))
    {
        Ok(f) => f,
        Err(_) => return false,
    };
    if !crate::platform::lock_briefly(&lock_file) {
        // Same read-modify-write shape as the rotation, and the same answer:
        // not now. The caller keeps its lines for the next attempt.
        return false;
    }
    let mut buckets: BTreeMap<RollKey, Bucket> = BTreeMap::new();
    // seed from the existing rollup so merges are additive across rotations.
    if let Ok(existing) = fs::read_to_string(rollup_path) {
        for line in existing.lines() {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                fold_one(&mut buckets, &v);
            }
        }
    }
    // Marked BEFORE the fold: a bucket no longer has a call to pair with.
    let mut raw: Vec<serde_json::Value> = lines
        .iter()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    mark_superseded(&mut raw);
    for v in &raw {
        fold_one(&mut buckets, v);
    }
    let mut out = String::new();
    for ((day_ts, op, class, io, est, compactor, billed), b) in &buckets {
        let mut row = serde_json::json!({
            "ts": day_ts,
            "source": "rollup",
            "op": op,
            "label": ROLLED_UP,
            "project": ROLLED_UP,
            "agent": ROLLED_UP,
            "before": b.before,
            "after": b.after,
            "saved": b.before.saturating_sub(b.after),
            "realistic": b.realistic,
            "rollup": true,
            "rollup_calls": b.calls,
        });
        if b.delivered != b.after {
            row["delivered"] = serde_json::Value::from(b.delivered);
        }
        if !compactor.is_empty() {
            row["compactor"] = serde_json::Value::from(compactor.as_str());
        }
        if !class.is_empty() {
            row["class"] = serde_json::Value::from(class.as_str());
        }
        if !io.is_empty() {
            row["io"] = serde_json::Value::from(io.as_str());
        }
        if *est {
            row["estimated"] = serde_json::Value::Bool(true);
        }
        if *billed {
            row["billed"] = serde_json::Value::Bool(true);
        }
        out.push_str(&row.to_string());
        out.push('\n');
    }
    crate::platform::write_atomic_600(rollup_path, out.as_bytes()).is_ok()
}

/// One-shot, idempotent recovery of pre-existing ledger backups into the
/// durable rollup. Historic resets/rotations left `gain.jsonl.bak-<ts>`,
/// `gain.jsonl.pre*`, `gain.jsonl.<n>` snapshots on disk whose savings the
/// dashboard never counted (it reads only the live ledger). This folds each
/// such backup into the rollup — deduplicating byte-identical rows across
/// overlapping snapshots so they aren't double-counted — then renames it
/// `<name>.counted` so a second call is a no-op. Returns rows ingested.
///
/// Conservative by design: dedup-by-line-hash can under-count two genuinely
/// distinct events that happen to be byte-identical, but never over-counts.
/// For a "trust the numbers" surface, under-counting beats inflation.
pub fn migrate_legacy_backups_into_rollup() -> usize {
    // Migration audit: the estimated ledger's stray backups (pre-#1042
    // rotation schemes) were never folded — a migrating user's historical
    // output/batching/compounding vanished from lifetime while the measured
    // twin was recovered. Same recipe, third file.
    migrate_backups_for(&jsonl_path())
        + migrate_backups_for(&lost_ledger_path())
        + migrate_backups_for(&estimated_ledger_path())
}

pub(super) fn migrate_backups_for(ledger: &std::path::Path) -> usize {
    let Some(dir) = ledger.parent() else {
        return 0;
    };
    let Some(base) = ledger.file_name().and_then(|s| s.to_str()) else {
        return 0;
    };
    let base = base.to_string();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };
    let mut backups: Vec<PathBuf> = Vec::new();
    for ent in entries.flatten() {
        let fname = ent.file_name();
        let Some(name) = fname.to_str() else { continue };
        if name == base {
            continue; // the live ledger
        }
        if name.ends_with(".counted") {
            continue; // already ingested on a prior run
        }
        if name.contains(".rollup.") {
            continue; // our own rollup / its merge temp
        }
        // Backups of THIS ledger only: "<base>.bak…", "<base>.pre…",
        // or a numeric-suffixed rotation like "<base>.1".
        let numeric_suffix = name
            .strip_prefix(&format!("{base}."))
            .map(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
            .unwrap_or(false);
        let is_backup = name.starts_with(&format!("{base}.bak"))
            || name.starts_with(&format!("{base}.pre"))
            || numeric_suffix;
        if is_backup {
            backups.push(ent.path());
        }
    }
    if backups.is_empty() {
        return 0;
    }
    backups.sort();
    use std::collections::HashSet;
    let hash_line = |s: &str| -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        s.trim().hash(&mut h);
        h.finish()
    };
    // Seed the dedup set with every line the LIVE ledger still holds.
    //
    // A backup is a SNAPSHOT, not an archive: `gain.jsonl.prepurge-…` and
    // `gain.jsonl.bak-…` were copied while those rows were in the ledger, and
    // rotation had not yet moved them anywhere. Folding them into the rollup
    // while they sit in the live file means the dashboard — which reads
    // `ledger + rollup` — counts them TWICE. Deduping only across backups (what
    // this did before) never looked at the file the rollup is added to.
    //
    // Measured on a dogfood install before this line existed: 29 216 rows
    // present in both a `.counted` backup and the live ledger, **199 194 952
    // tokens double-counted — +93 % on the lifetime figure**. Exactly the class
    // of the +203.7 % usage double-count of 2026-07-31, one layer down.
    //
    // Residual, stated because it is not fixed here: a backup can also hold
    // rows that a PRIOR rotation already folded and then removed from the
    // ledger. Those are invisible to this check — the rollup is aggregated, so
    // nothing records which lines it absorbed. `.counted` keeps that from
    // repeating per backup; a per-line ledger of what was folded would be the
    // complete fix and costs a growing sidecar.
    let mut seen: HashSet<u64> = HashSet::new();
    if let Ok(live) = fs::read_to_string(ledger) {
        for line in live.lines() {
            let line = line.trim();
            if !line.is_empty() {
                seen.insert(hash_line(line));
            }
        }
    }
    let mut unique: Vec<String> = Vec::new();
    for bak in &backups {
        if let Ok(body) = fs::read_to_string(bak) {
            for line in body.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if seen.insert(hash_line(line)) {
                    unique.push(line.to_string());
                }
            }
        }
    }
    let ingested = unique.len();
    // Unfolded backups stay unmarked, so the next run retries them.
    if ingested > 0 && !fold_lines_into_rollup(&unique, &rollup_path_for(ledger)) {
        return 0;
    }
    // Mark every processed backup as counted so re-runs are no-ops. The
    // data is preserved (renamed, not deleted) so a user can audit it.
    for bak in &backups {
        let counted = bak.with_file_name(format!(
            "{}.counted",
            bak.file_name().and_then(|s| s.to_str()).unwrap_or("backup")
        ));
        let _ = fs::rename(bak, counted);
    }
    ingested
}

/// #499 — Compact the global gain.jsonl by dropping noise rows that
/// carry zero analytical signal: any row whose `saved == 0` and whose
/// `op` is NOT a code-intel op (the dashboard's code-intel heuristic
/// legitimately stores `before == after` rows then expands the baseline
/// at read time, so those must be preserved). Returns the number of
/// rows dropped.
///
/// Why this exists when the per-call-site guards already drop zero-
/// saved rows: long-running MCP processes spawned before a binary
/// upgrade keep emitting noise from the old code path until the user
/// restarts their agent. Production audit 2026-05-31 found ~1200
/// historic op=passthrough rows from a stale MCP that pre-dated the
/// #424 cmd_wrap guard, dragging dashboard averages down. This is the
/// catch-all that lets `tokenade evict-stale` heal the ledger.
///
/// File handling mirrors [`rotate_if_oversized`]: read, filter, write
/// atomically via tmp + rename, while holding the cross-process
/// write lock. NotFound is treated as "nothing to do".
pub fn prune_noise() -> std::io::Result<usize> {
    let path = jsonl_path();
    // #1035 — in-process mutex moved here (was at line 340 below). The
    // pre-fix order held the in-process lock only for the final write,
    // leaving the read+filter exposed to interleaved record() calls from
    // other threads in the same process (MCP server's serve loop +
    // `evict-stale` CLI subcommand running concurrently is the realistic
    // case). Moving the take to the top covers the entire read-filter-
    // write-rename cycle.
    let _guard = write_lock().lock().unwrap_or_else(|p| p.into_inner());
    // #1035 — cross-process flock for the rest of the cycle. Same
    // rationale as rotate_if_oversized: any other process writing
    // between our read and rename would have its record silently
    // dropped. The lock is the SIDECAR every other participant takes, so a
    // writer and this cycle still exclude each other.
    // No ledger, nothing to prune — and no reason to leave a `.lock` beside a
    // file that does not exist. The open below CREATES, so it can no longer
    // answer NotFound the way locking the ledger itself did.
    if !path.exists() {
        return Ok(0);
    }
    let lock_file = match fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(lock_path_for(&path))
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    if !crate::platform::lock_briefly(&lock_file) {
        // A read-filter-rewrite cycle: proceeding unlocked is the race that
        // loses whatever another process appends between the read and the
        // rename. Pruning is opportunistic, so not pruning is the safe answer.
        return Ok(0);
    }
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let mut kept = String::with_capacity(content.len());
    let mut dropped = 0usize;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Malformed JSON: keep verbatim — we don't have grounds to
        // decide it's noise. The dashboard parser already tolerates
        // skipped lines.
        let v: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => {
                kept.push_str(trimmed);
                kept.push('\n');
                continue;
            }
        };
        let before = v["before"].as_u64().unwrap_or(0);
        let after = v["after"].as_u64().unwrap_or(0);
        // Same op resolution as every reader (`decode_row`): a legacy row
        // carries the compactor under `filter`, and reading `op` alone made it
        // look unlabelled — which put code-intel rows, exempt by design, back
        // under the noise threshold that DELETES them.
        let op = v["op"]
            .as_str()
            .or_else(|| v["filter"].as_str())
            .unwrap_or("");
        // #542: bring the prune threshold in line with the writer's
        // gate (saved >= 10 tokens AND saved >= 5 %). Without this,
        // entries that the centralized log_gain helper (#541) would
        // now reject still hang around in historic data and drag
        // dashboard averages. Code-intel ops are exempt — their
        // savings can't be measured directly so a small `saved`
        // there is signal, not noise (see code_intel_heuristic).
        if !worth_recording(before, after, op) {
            dropped += 1;
            continue;
        }
        kept.push_str(trimmed);
        kept.push('\n');
    }
    if dropped == 0 {
        return Ok(0);
    }
    // #1035 — write_lock acquisition removed: now held at top of fn.
    crate::platform::write_atomic_600(&path, kept.as_bytes())?;
    Ok(dropped)
}

/// L57 migrator. Append a legacy per-project
/// `<root>/.tokenade/gain.jsonl` to the global
/// `~/.tokenade/gain.jsonl`, then delete the per-project file. Each
/// record carries a `project` field so the global ledger remains
/// per-project-breakdownable after the merge.
///
/// Best-effort: read/write failures leave the per-project file in
/// place (no data loss, just no migration).
pub fn migrate_local_ledger(root: &std::path::Path) -> Option<String> {
    let local = root.join(".tokenade").join("gain.jsonl");
    if !local.is_file() {
        return None;
    }
    let Ok(content) = fs::read_to_string(&local) else {
        return None;
    };
    let line_count = content.lines().filter(|l| !l.trim().is_empty()).count();

    // Always land at the user-home location, bypassing the set_root
    // override that tests use. The migrator is invoked at project
    // scaffold time, before any test override is in play.
    let global = crate::platform::home_dir()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join(".tokenade")
        .join("gain.jsonl");
    if let Some(dir) = global.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _guard = write_lock().lock().unwrap_or_else(|p| p.into_inner());
    let Ok(f) = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(&global)
    else {
        return None;
    };
    use fs2::FileExt;
    let _ = crate::platform::lock_briefly(&f);
    let mut f = f;
    let _ = f.write_all(content.as_bytes());
    if !content.ends_with('\n') {
        let _ = f.write_all(b"\n");
    }
    let _ = FileExt::unlock(&f);
    drop(_guard);
    let _ = fs::remove_file(&local);
    Some(format!(
        ".tokenade/gain.jsonl (merged {line_count} record(s) into ~/.tokenade/gain.jsonl)"
    ))
}
