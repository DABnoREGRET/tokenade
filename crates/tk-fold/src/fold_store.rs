//! Persistent store for log-fold raw lines + slot values.
//!
//! When `logs::compact_generic_logs_indexed` folds `<TS> INFO conn
//! from <N> req=<UUID> [×100 lines · distinct N=7 · …]`, the
//! annotation tells the model the SHAPE of the run but not the
//! specific lines. The agent can then drill in via
//! `tokenade expand-fold <template_id>`
//! to retrieve the matching raw lines — e.g. "all lines where
//! N=10.0.0.5" or "lines containing 'auth-failed'".
//!
//! ## Why per-fold storage (and not just file-search)
//!
//! Semantic similarity is the wrong tool for "show me all lines from
//! IP X": every line in the fold is semantically near-identical, so
//! KNN doesn't discriminate. The slot extraction the templater
//! already did is structurally richer than embeddings for this query
//! shape — we just need to keep it around.
//!
//! ## Schema
//!
//! ```sql
//! CREATE TABLE folded_runs (
//!     content_hash TEXT NOT NULL,
//!     template_id  TEXT NOT NULL,
//!     template     TEXT NOT NULL,
//!     line_count   INTEGER NOT NULL,
//!     slot_kinds   TEXT NOT NULL,   -- JSON array of "TS"/"N"/"UUID"/"HEX"
//!     raw_lines    TEXT NOT NULL,   -- JSON array of strings
//!     slot_values  TEXT NOT NULL,   -- JSON array of arrays
//!     created_at   INTEGER NOT NULL,
//!     PRIMARY KEY (content_hash, template_id)
//! );
//! ```
//!
//! ## Eviction
//!
//! Folds inherit dedup_ref's 7-day TTL via `created_at`. The MCP
//! tool checks staleness on read and the `evict_stale` helper drops
//! expired records.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 7-day TTL — same as the §ref§ store.
pub const TTL_SECS: u64 = 7 * 86_400;

/// One folded run, ready for persistence or expansion.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FoldRecord {
    pub template_id: String,
    pub template: String,
    pub slot_kinds: Vec<String>,
    pub raw_lines: Vec<String>,
    /// Outer vec = one per raw_line. Inner vec = one slot value per kind
    /// (same length as `slot_kinds`). Stored alongside the raw line so
    /// the drill-down query can filter by slot value without re-walking.
    pub slot_values: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpandResult {
    pub content_hash: String,
    pub template_id: String,
    pub template: String,
    pub total_lines: usize,
    pub matched_lines: usize,
    pub lines: Vec<String>,
    /// Set when the request hit the cap and not all matching lines
    /// were returned.
    pub truncated: bool,
}

/// Max lines returned to the caller in one expand call when `max` is not
/// specified. Higher than this and we truncate — the user can refine their
/// filter or pass a higher explicit `max`.
pub const MAX_RETURNED_LINES: usize = 200;

/// Hard ceiling for an explicit caller-supplied `max`. Callers may request
/// up to this many lines when they need more than the default — e.g. pass
/// `max=500` to retrieve up to 500 lines from a single fold. We cap here
/// (not at `MAX_RETURNED_LINES`) so that a rogue or runaway caller cannot
/// exhaust memory by requesting an unbounded result set.
pub const MAX_EXPLICIT_LINES: usize = 2_000;

/// Compute the 12-char id for a template. Stable per template string —
/// two folds of the same shape collide intentionally so the store
/// can union their lines under one record.
pub fn template_id(template: &str) -> String {
    crate::dedup_ref::hash_content(template.as_bytes())
}

pub fn db_path() -> PathBuf {
    if let Some(p) = std::env::var_os("TOKENADE_FOLD_STORE_DB") {
        return PathBuf::from(p);
    }
    crate::platform::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".tokenade")
        .join("fold_store.db")
}

/// VACUUM the fold store. Returns `(before, after)` file sizes.
pub fn vacuum_db() -> (u64, u64) {
    crate::store_scrub::vacuum_store(&db_path(), open)
}

fn open() -> Result<Connection, rusqlite::Error> {
    let path = db_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let conn = Connection::open(&path)?;
    crate::sqlite_tuning::tune(&conn);
    // Step 1: bare-minimum schema (no last_accessed) — present on
    // both fresh and pre-L62 DBs.
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS folded_runs (
            content_hash TEXT NOT NULL,
            template_id  TEXT NOT NULL,
            template     TEXT NOT NULL,
            line_count   INTEGER NOT NULL,
            slot_kinds   TEXT NOT NULL,
            raw_lines    TEXT NOT NULL,
            slot_values  TEXT NOT NULL,
            created_at   INTEGER NOT NULL,
            PRIMARY KEY (content_hash, template_id)
        );
        CREATE INDEX IF NOT EXISTS folded_runs_created ON folded_runs(created_at);
        "#,
    )?;
    // Step 2: add last_accessed for pre-L62 DBs (errors swallowed —
    // duplicate column is benign).
    let _ = conn.execute(
        "ALTER TABLE folded_runs ADD COLUMN last_accessed INTEGER NOT NULL DEFAULT 0",
        [],
    );
    // Step 3: index on the column we just guaranteed exists.
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS folded_runs_last_accessed ON folded_runs(last_accessed)",
        [],
    );
    Ok(conn)
}

/// Save (or union) a fold record. If a record with the same
/// `(content_hash, template_id)` exists, the new lines are appended
/// to the existing ones — this lets two non-consecutive runs of the
/// same template merge under one drill-down key.
pub fn save_fold(content_hash: &str, rec: &FoldRecord) -> Result<(), rusqlite::Error> {
    // Redact at the SINK, not at the callers. `expand-fold` already redacts on
    // the way out, so the model never saw a secret either way — but the folded
    // lines were landing on disk raw, so `~/.tokenade/fold_store.db` held
    // cleartext credentials that appeared in a repeated log line, readable by
    // anything with filesystem access. `dedup.db` has redacted-before-store
    // since #477; this store was the remaining hole. Doing it here means no
    // future caller can reintroduce it.
    let rec = &FoldRecord {
        template_id: rec.template_id.clone(),
        template: crate::redact::redact(&rec.template),
        slot_kinds: rec.slot_kinds.clone(),
        raw_lines: rec
            .raw_lines
            .iter()
            .map(|l| crate::redact::redact(l))
            .collect(),
        slot_values: rec
            .slot_values
            .iter()
            .map(|vals| vals.iter().map(|v| crate::redact::redact(v)).collect())
            .collect(),
    };
    let conn = open()?;
    let now = crate::platform::now_unix_secs();
    let existing: Option<(String, String)> = conn
        .query_row(
            "SELECT raw_lines, slot_values FROM folded_runs WHERE content_hash=?1 AND template_id=?2",
            params![content_hash, rec.template_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .ok();
    let (raw_lines_json, slot_values_json, total) = match existing {
        Some((rl, sv)) => {
            let mut prior_lines: Vec<String> = serde_json::from_str(&rl).unwrap_or_default();
            let mut prior_slots: Vec<Vec<String>> = serde_json::from_str(&sv).unwrap_or_default();
            prior_lines.extend(rec.raw_lines.iter().cloned());
            prior_slots.extend(rec.slot_values.iter().cloned());
            let total = prior_lines.len();
            (
                serde_json::to_string(&prior_lines).unwrap_or_default(),
                serde_json::to_string(&prior_slots).unwrap_or_default(),
                total,
            )
        }
        None => (
            serde_json::to_string(&rec.raw_lines).unwrap_or_default(),
            serde_json::to_string(&rec.slot_values).unwrap_or_default(),
            rec.raw_lines.len(),
        ),
    };
    let slot_kinds_json = serde_json::to_string(&rec.slot_kinds).unwrap_or_default();
    conn.execute(
        "INSERT INTO folded_runs(content_hash, template_id, template, line_count, slot_kinds, raw_lines, slot_values, created_at, last_accessed)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
         ON CONFLICT(content_hash, template_id) DO UPDATE SET
             line_count=?4, slot_kinds=?5, raw_lines=?6, slot_values=?7, last_accessed=?8",
        params![
            content_hash,
            rec.template_id,
            rec.template,
            total as i64,
            slot_kinds_json,
            raw_lines_json,
            slot_values_json,
            now as i64,
        ],
    )?;
    Ok(())
}

/// Drill into a fold. `slot_filter` keys are kind labels (`TS`, `N`,
/// `UUID`, `HEX`); values are matched as **substrings** of the slot
/// value at that line. An empty filter returns all lines. Caller can
/// further restrict via `max` (defaults to [`MAX_RETURNED_LINES`]).
pub fn expand_fold(
    content_hash: &str,
    template_id: &str,
    slot_filter: &std::collections::BTreeMap<String, String>,
    max: Option<usize>,
) -> Result<Option<ExpandResult>, rusqlite::Error> {
    let conn = open()?;
    let row: Option<(String, i64, String, String, String, i64)> = conn
        .query_row(
            "SELECT template, line_count, slot_kinds, raw_lines, slot_values, created_at
             FROM folded_runs WHERE content_hash=?1 AND template_id=?2",
            params![content_hash, template_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)?,
                ))
            },
        )
        .ok();
    let Some((template, line_count, slot_kinds_json, raw_lines_json, slot_values_json, created_at)) =
        row
    else {
        return Ok(None);
    };
    // Hard TTL guard on created_at — a fold is always evicted once it
    // is older than TTL_SECS, regardless of how recently it was read.
    // We bump last_accessed below so that evict_stale() does not
    // garbage-collect an actively-queried fold before this hard bound.
    let age = crate::platform::now_unix_secs().saturating_sub(created_at as u64);
    if age > TTL_SECS {
        let _ = conn.execute(
            "DELETE FROM folded_runs WHERE content_hash=?1 AND template_id=?2",
            params![content_hash, template_id],
        );
        return Ok(None);
    }
    let _ = conn.execute(
        "UPDATE folded_runs SET last_accessed=?1 WHERE content_hash=?2 AND template_id=?3",
        params![
            crate::platform::now_unix_secs() as i64,
            content_hash,
            template_id
        ],
    );
    let slot_kinds: Vec<String> = serde_json::from_str(&slot_kinds_json).unwrap_or_default();
    let raw_lines: Vec<String> = serde_json::from_str(&raw_lines_json).unwrap_or_default();
    let slot_values: Vec<Vec<String>> = serde_json::from_str(&slot_values_json).unwrap_or_default();
    let cap = max.unwrap_or(MAX_RETURNED_LINES).min(MAX_EXPLICIT_LINES);
    let mut matched: Vec<String> = Vec::new();
    let mut total_matched = 0usize;
    for (i, line) in raw_lines.iter().enumerate() {
        if slot_filter.is_empty() {
            total_matched += 1;
            if matched.len() < cap {
                matched.push(line.clone());
            }
            continue;
        }
        let row_vals = slot_values.get(i);
        let pass = slot_filter.iter().all(|(needle_kind, needle)| {
            // For each filter, find a slot whose kind matches AND value
            // contains the needle.
            let Some(vals) = row_vals else { return false };
            slot_kinds.iter().enumerate().any(|(idx, k)| {
                k == needle_kind && vals.get(idx).is_some_and(|v| v.contains(needle))
            })
        });
        if pass {
            total_matched += 1;
            if matched.len() < cap {
                matched.push(line.clone());
            }
        }
    }
    Ok(Some(ExpandResult {
        content_hash: content_hash.to_string(),
        template_id: template_id.to_string(),
        template,
        total_lines: line_count as usize,
        matched_lines: total_matched,
        lines: matched,
        truncated: total_matched > cap,
    }))
}

/// Look up the most-recent `content_hash` for a given `template_id`.
/// Used by `expand_fold` when the caller doesn't pass an explicit
/// `content_hash` — typical for hints generated mid-session where the
/// model just quotes the template_id from the fold annotation.
pub fn latest_content_hash(template_id: &str) -> Option<String> {
    let conn = open().ok()?;
    conn.query_row(
        "SELECT content_hash FROM folded_runs WHERE template_id=?1 ORDER BY created_at DESC LIMIT 1",
        params![template_id],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

/// Drop folds past [`TTL_SECS`] on EITHER horizon.
///
/// `created_at` is the hard one, and it was missing here while
/// [`expand_fold`] has enforced it all along: "a fold is always evicted once
/// it is older than TTL_SECS, regardless of how recently it was read". Only
/// the reader carried that rule, so the store kept — for an unbounded time —
/// rows the one reader is guaranteed to refuse. `save_fold` bumps
/// `last_accessed` on conflict and leaves `created_at` at the FIRST save, so
/// a fold re-saved inside each window is immortal to this pass while being
/// dead to every read of it. Measured on a dogfood install: one row created
/// 2026-08-03 still resident on 2026-09-01 — 28.8 days on a 7-day store,
/// 66.7 MB, walked past by every daily sweep since 2026-08-09.
///
/// The horizons AGREE by construction rather than by inspection: `expand_fold`
/// asks `age > TTL_SECS` and this asks `created_at < now - TTL_SECS`, which is
/// the same boundary. `a_fold_older_than_the_ttl_is_dropped_however_fresh_its_access`
/// pins that.
///
/// The old `last_accessed = 0` safety net for pre-L62 rows is GONE, not
/// forgotten: `created_at < ?1` subsumes it exactly, and a clause that cannot
/// change an outcome is noise.
pub fn evict_stale() -> Result<usize, rusqlite::Error> {
    let conn = open()?;
    let cutoff = crate::platform::now_unix_secs().saturating_sub(TTL_SECS) as i64;
    conn.execute(
        "DELETE FROM folded_runs
         WHERE created_at < ?1
            OR (last_accessed > 0 AND last_accessed < ?1)",
        params![cutoff],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn io_lock() -> &'static Mutex<()> {
        static LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn isolated_db(label: &str) -> std::sync::MutexGuard<'static, ()> {
        let guard = io_lock().lock().unwrap_or_else(|p| p.into_inner());
        let p = std::env::temp_dir().join(format!(
            "tokenade-fold-store-{label}-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&p);
        // SAFETY: env var is process-wide; io_lock serialises tests.
        unsafe {
            std::env::set_var("TOKENADE_FOLD_STORE_DB", &p);
        }
        guard
    }

    /// A credential inside a repeated log line must not reach the database
    /// file. `expand_fold` redacts on the way out, so this is invisible from
    /// the agent's side — the leak was purely the bytes at rest.
    #[test]
    fn a_secret_in_a_folded_line_never_lands_on_disk() {
        let _g = isolated_db("redaction");
        let key = "AKIAIOSFODNN7EXAMPLE";
        let rec = FoldRecord {
            template_id: template_id("<TS> auth key=<S> ok"),
            template: "<TS> auth key=<S> ok".to_string(),
            slot_kinds: vec!["TS".into(), "S".into()],
            raw_lines: vec![
                format!("2026-07-29T10:00:00Z auth key={key} ok"),
                format!("2026-07-29T10:00:01Z auth key={key} ok"),
            ],
            slot_values: vec![
                vec!["2026-07-29T10:00:00Z".into(), key.into()],
                vec!["2026-07-29T10:00:01Z".into(), key.into()],
            ],
        };
        save_fold("hash-redaction", &rec).expect("save");
        let bytes = std::fs::read(db_path()).expect("db readable");
        let body = String::from_utf8_lossy(&bytes);
        assert!(
            !body.contains(key),
            "credential persisted verbatim to {}",
            db_path().display()
        );
    }

    fn sample_record() -> FoldRecord {
        FoldRecord {
            template_id: template_id("<TS> INFO conn from <N> ok"),
            template: "<TS> INFO conn from <N> ok".to_string(),
            slot_kinds: vec!["TS".into(), "N".into()],
            raw_lines: vec![
                "2026-05-29T13:42:18Z INFO conn from 10.0.0.5 ok".to_string(),
                "2026-05-29T13:42:19Z INFO conn from 10.0.0.5 ok".to_string(),
                "2026-05-29T13:42:20Z INFO conn from 10.0.0.7 ok".to_string(),
            ],
            slot_values: vec![
                vec!["2026-05-29T13:42:18Z".into(), "10.0.0.5".into()],
                vec!["2026-05-29T13:42:19Z".into(), "10.0.0.5".into()],
                vec!["2026-05-29T13:42:20Z".into(), "10.0.0.7".into()],
            ],
        }
    }

    #[test]
    fn save_then_expand_no_filter_returns_all_lines() {
        let _g = isolated_db("all");
        let rec = sample_record();
        save_fold("hash-1", &rec).unwrap();
        let r = expand_fold("hash-1", &rec.template_id, &Default::default(), None)
            .unwrap()
            .unwrap();
        assert_eq!(r.total_lines, 3);
        assert_eq!(r.matched_lines, 3);
        assert_eq!(r.lines.len(), 3);
        assert!(!r.truncated);
    }

    #[test]
    fn filter_by_n_value_returns_only_matching() {
        let _g = isolated_db("filter");
        let rec = sample_record();
        save_fold("hash-2", &rec).unwrap();
        let mut filter = std::collections::BTreeMap::new();
        filter.insert("N".into(), "10.0.0.5".into());
        let r = expand_fold("hash-2", &rec.template_id, &filter, None)
            .unwrap()
            .unwrap();
        assert_eq!(r.matched_lines, 2);
        assert!(r.lines.iter().all(|l| l.contains("10.0.0.5")));
    }

    #[test]
    fn second_save_unions_lines_under_same_template_id() {
        let _g = isolated_db("union");
        let mut rec = sample_record();
        save_fold("hash-3", &rec).unwrap();
        rec.raw_lines = vec!["2026-05-29T13:42:21Z INFO conn from 10.0.0.9 ok".to_string()];
        rec.slot_values = vec![vec!["2026-05-29T13:42:21Z".into(), "10.0.0.9".into()]];
        save_fold("hash-3", &rec).unwrap();
        let r = expand_fold("hash-3", &rec.template_id, &Default::default(), None)
            .unwrap()
            .unwrap();
        assert_eq!(r.total_lines, 4);
    }

    #[test]
    fn unknown_template_id_returns_none() {
        let _g = isolated_db("unknown");
        let r = expand_fold("nope", "nope", &Default::default(), None).unwrap();
        assert!(r.is_none());
    }

    /// The GC and the reader must agree about when a fold is dead.
    ///
    /// `expand_fold` has always enforced `created_at` as the HARD horizon;
    /// `evict_stale` looked only at `last_accessed`, and `save_fold` bumps
    /// that on conflict while leaving `created_at` at the first save. A fold
    /// re-saved inside each window was therefore immortal to the sweep and
    /// dead to every read — 66.7 MB of it on the install that found this.
    ///
    /// Two rows, because the reader DELETES what it refuses: `keep-fresh`
    /// pins that the pass is a horizon and not a truncation.
    #[test]
    fn a_fold_older_than_the_ttl_is_dropped_however_fresh_its_access() {
        let _g = isolated_db("hard-ttl");
        let rec = sample_record();
        save_fold("drop-old", &rec).unwrap();
        save_fold("keep-fresh", &rec).unwrap();
        let conn = open().unwrap();
        let ancient = (crate::platform::now_unix_secs() - (TTL_SECS + 60)) as i64;
        // Well inside the TTL, but strictly in the PAST. A row stamped at this
        // exact second would also survive a pass whose cutoff is `now`, so
        // "kept" would say nothing about the window actually being TTL wide.
        conn.execute(
            "UPDATE folded_runs SET created_at=?1, last_accessed=?1 WHERE content_hash='keep-fresh'",
            params![(crate::platform::now_unix_secs() - 60) as i64],
        )
        .unwrap();
        // Exactly the shape `save_fold` leaves behind on a re-save: the first
        // save's `created_at`, this instant's `last_accessed`.
        conn.execute(
            "UPDATE folded_runs SET created_at=?1, last_accessed=?2 WHERE content_hash='drop-old'",
            params![ancient, crate::platform::now_unix_secs() as i64],
        )
        .unwrap();

        // The OTHER horizon, still load-bearing: fresh birth, stale access.
        save_fold("drop-unread", &rec).unwrap();
        conn.execute(
            "UPDATE folded_runs SET last_accessed=?1 WHERE content_hash='drop-unread'",
            params![ancient],
        )
        .unwrap();

        let removed = evict_stale().unwrap();
        assert_eq!(removed, 2, "one row per horizon, and nothing else");
        let gone: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM folded_runs WHERE content_hash='drop-old'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(gone, 0, "row past its hard TTL survived evict_stale");
        let unread: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM folded_runs WHERE content_hash='drop-unread'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(unread, 0, "row past its access TTL survived evict_stale");
        let kept: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM folded_runs WHERE content_hash='keep-fresh'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kept, 1, "evict_stale took a row that is inside the TTL");

        // And the reader draws the line in the same place, on the same shape.
        save_fold("agree", &rec).unwrap();
        conn.execute(
            "UPDATE folded_runs SET created_at=?1, last_accessed=?2 WHERE content_hash='agree'",
            params![ancient, crate::platform::now_unix_secs() as i64],
        )
        .unwrap();
        assert!(
            expand_fold("agree", &rec.template_id, &Default::default(), None)
                .unwrap()
                .is_none(),
            "reader and GC disagree about the same row"
        );
    }

    #[test]
    fn expired_fold_is_dropped_on_lookup() {
        let _g = isolated_db("expired");
        let rec = sample_record();
        save_fold("hash-old", &rec).unwrap();
        // Back-date the row past the TTL.
        let conn = open().unwrap();
        let ancient = (crate::platform::now_unix_secs() - (TTL_SECS + 60)) as i64;
        conn.execute(
            "UPDATE folded_runs SET created_at=?1 WHERE content_hash=?2",
            params![ancient, "hash-old"],
        )
        .unwrap();
        let r = expand_fold("hash-old", &rec.template_id, &Default::default(), None).unwrap();
        assert!(r.is_none());
        // Row should be purged.
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM folded_runs WHERE content_hash='hash-old'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn max_cap_truncates_with_marker() {
        let _g = isolated_db("cap");
        let mut rec = sample_record();
        for i in 0..MAX_RETURNED_LINES + 50 {
            rec.raw_lines.push(format!("line {i}"));
            rec.slot_values
                .push(vec![format!("ts-{i}"), format!("ip-{i}")]);
        }
        save_fold("hash-cap", &rec).unwrap();
        let r = expand_fold("hash-cap", &rec.template_id, &Default::default(), None)
            .unwrap()
            .unwrap();
        assert!(r.truncated);
        assert_eq!(r.lines.len(), MAX_RETURNED_LINES);
        assert!(r.matched_lines > MAX_RETURNED_LINES);
    }

    #[test]
    fn explicit_max_above_default_returns_more_lines() {
        // Verifies that MAX_EXPLICIT_LINES > MAX_RETURNED_LINES is actually
        // honoured: a caller passing max >> MAX_RETURNED_LINES must get more
        // than MAX_RETURNED_LINES lines back, not silently capped at 200.
        //
        // Note: sample_record() pre-populates 3 lines, so after adding
        // `inserted` more the record has inserted+3 lines total. We pass a
        // cap of MAX_RETURNED_LINES*5 (=1000) which is well above that total,
        // so no truncation should occur.
        let _g = isolated_db("explicit-cap");
        let mut rec = sample_record();
        let inserted = MAX_RETURNED_LINES + 100;
        for i in 0..inserted {
            rec.raw_lines.push(format!("line {i}"));
            rec.slot_values
                .push(vec![format!("ts-{i}"), format!("ip-{i}")]);
        }
        save_fold("hash-explicit", &rec).unwrap();
        let r = expand_fold(
            "hash-explicit",
            &rec.template_id,
            &Default::default(),
            Some(MAX_RETURNED_LINES * 5),
        )
        .unwrap()
        .unwrap();
        // Cap (1000) >> actual total (inserted+3) → no truncation.
        assert!(
            !r.truncated,
            "should not truncate: cap 1000 >> actual line count"
        );
        // The explicit cap must have allowed more lines than the default 200.
        assert!(
            r.lines.len() > MAX_RETURNED_LINES,
            "explicit max must allow more than the default {} cap, got {}",
            MAX_RETURNED_LINES,
            r.lines.len(),
        );
    }
}
