//! **The database**, and what gets written into it unintentionally.
//!
//! The store is GLOBAL, not per project: an agent that switches repos
//! keeps its references, and that is the whole point.
//!
//! What goes in is tool content, hence possibly a secret. The
//! pass through `redact` is not a courtesy: the bytes returned by
//! `expand-ref` are those we stored, and an API key stored in clear
//! would come back out as-is months later.

use super::*;

/// Path to the global dedup DB. Honours `TOKENADE_DEDUP_DB` env
/// override (used by tests for isolation).
pub fn db_path() -> PathBuf {
    if let Some(p) = std::env::var_os("TOKENADE_DEDUP_DB") {
        return PathBuf::from(p);
    }
    crate::platform::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".tokenade")
        .join("dedup.db")
}

/// Open the DB and ensure schema. Safe to call repeatedly — each
/// call is a fresh connection (no shared pool; SQLite WAL handles
/// concurrent processes/threads). Failures (no home, write-locked
/// fs, etc.) bubble up as `Err` and callers degrade to no-op.
/// Count dedup_content rows whose `content` column contains the
/// substring. Used by `tokenade scrub-secret --dry-run` to preview
/// without modifying anything. Returns 0 on any DB error (best-effort).
pub fn count_content_matches(substring: &str) -> usize {
    let Ok(conn) = open_conn() else {
        return 0;
    };
    let pattern = format!("%{substring}%");
    conn.query_row::<i64, _, _>(
        "SELECT COUNT(*) FROM dedup_content WHERE content LIKE ?1",
        params![pattern],
        |r| r.get(0),
    )
    .unwrap_or(0) as usize
}

/// Reclaim disk space freed by deleted rows. Mirror of
/// [`semantic_db::vacuum_db`]. Returns (bytes_before, bytes_after).
pub fn vacuum_db() -> (u64, u64) {
    crate::store_scrub::vacuum_store(&db_path(), open_conn)
}

/// REPLACE every occurrence of `needle` with `replacement` in the
/// dedup_content table's content column. Returns count of rows
/// touched, or 0 on DB error.
pub fn replace_content_substring(needle: &str, replacement: &str) -> usize {
    let Ok(conn) = open_conn() else {
        return 0;
    };
    let pattern = format!("%{needle}%");
    conn.execute(
        "UPDATE dedup_content SET content = REPLACE(content, ?1, ?2) WHERE content LIKE ?3",
        params![needle, replacement, pattern],
    )
    .unwrap_or(0)
}

/// Iterate every row in `dedup_content` and re-apply the global
/// [`crate::redact::redact`] pass. Rows whose content changes get
/// UPDATEd with the redacted form (and `byte_count` re-synced).
/// Returns `(rows_touched, bytes_before, bytes_after)`.
///
/// One-shot historical cleanup that complements the write-path redaction fixes —
/// those three fixes redact on the WRITE path but leave any content
/// already on disk untouched. Idempotent: a second invocation
/// returns `rows_touched == 0` because the first pass left only
/// already-redacted strings behind.
///
/// Best-effort: DB-open failure returns `(0, 0, 0)`; per-row update
/// failures are swallowed (the caller can re-run if needed).
pub fn scrub_via_redact() -> (usize, u64, u64) {
    scrub_via_redact_inner(false)
}

/// Dry-run mode: count rows whose `content` would change if
/// `scrub_via_redact()` ran, but issue no UPDATEs. Returns
/// `(would_touch, db_size_before, db_size_before)` — `after == before`
/// because nothing is modified. Powers `tokenade scrub-cache --dry-run`.
pub fn scrub_via_redact_dry_run() -> (usize, u64, u64) {
    scrub_via_redact_inner(true)
}

pub(crate) fn scrub_via_redact_inner(dry_run: bool) -> (usize, u64, u64) {
    // Opening a SQLite database CREATES it. `scrub-cache --dry-run` prints
    // "no writes" and then made three empty databases on a machine that had
    // none — the one flag whose whole purpose is to touch nothing. A database
    // that does not exist has nothing to redact, so the honest answer is zero
    // without opening it.
    if dry_run && !db_path().exists() {
        return (0, 0, 0);
    }
    let Ok(conn) = open_conn() else {
        return (0, 0, 0);
    };
    let before = std::fs::metadata(db_path()).map_or(0, |m| m.len());
    // Materialise the (hash, content) list first so we don't hold a
    // SELECT statement open while issuing UPDATEs on the same conn.
    let rows: Vec<(String, String)> = {
        let Ok(mut stmt) = conn.prepare("SELECT hash, content FROM dedup_content") else {
            return (0, before, before);
        };
        let Ok(mapped) =
            stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        else {
            return (0, before, before);
        };
        let collected: Vec<(String, String)> = mapped.flatten().collect();
        collected
    };
    let mut touched = 0usize;
    for (hash, content) in rows {
        let redacted = crate::redact::redact(&content);
        if redacted != content {
            if dry_run {
                touched += 1;
                continue;
            }
            let n = conn
                .execute(
                    "UPDATE dedup_content SET content = ?1, byte_count = ?2 WHERE hash = ?3",
                    params![redacted, redacted.len() as i64, hash],
                )
                .unwrap_or(0);
            if n > 0 {
                touched += 1;
            }
        }
    }
    if !dry_run && touched > 0 {
        let _ = conn.execute("VACUUM", []);
    }
    let after = std::fs::metadata(db_path()).map_or(before, |m| m.len());
    (touched, before, after)
}

pub(crate) fn open_conn() -> Result<Connection, rusqlite::Error> {
    let path = db_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let conn = Connection::open(&path)?;
    crate::sqlite_tuning::tune(&conn);
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS dedup_content (
            hash       TEXT PRIMARY KEY,
            content    TEXT NOT NULL,
            byte_count INTEGER NOT NULL,
            last_used  INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS dedup_content_last_used
            ON dedup_content(last_used);

        CREATE TABLE IF NOT EXISTS dedup_path_pointers (
            path_key      TEXT PRIMARY KEY,
            content_hash  TEXT NOT NULL,
            last_updated  INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS dedup_path_pointers_updated
            ON dedup_path_pointers(last_updated);

        -- Read-range coverage (slice-paging dedup). One row per line range
        -- ALREADY delivered verbatim to the agent, scoped to the SESSION it
        -- was delivered in — so a covered range is provably still in the
        -- live context window (cross-session reads start from a fresh
        -- context and must NOT be dedup'd). `version` = "mtime|size", so any
        -- edit invalidates stale ranges automatically.
        CREATE TABLE IF NOT EXISTS dedup_read_ranges (
            session_id    TEXT NOT NULL,
            file_key      TEXT NOT NULL,
            version       TEXT NOT NULL,
            start_line    INTEGER NOT NULL,
            end_line      INTEGER NOT NULL,
            content_hash  TEXT NOT NULL,
            last_used     INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS dedup_read_ranges_lookup
            ON dedup_read_ranges(session_id, file_key, version);
        "#,
    )?;
    Ok(conn)
}
