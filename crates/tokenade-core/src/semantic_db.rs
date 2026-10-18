//! Consolidated SQLite store for per-project semantic indexes.
//!
//! Replaces the per-project `<root>/.tokenade/semantic.bin` file
//! with a single `~/.tokenade/semantic.db` keyed by project path.
//! Lets `scope: "global"` semantic_search do ONE DB query instead
//! of walking N filesystem entries, and removes the disk-duplication
//! the user flagged ("the semantic memory [...] should live in a
//! single shared database with a queryable parameter for
//! filtering").
//!
//! ## Phase 1 (this module)
//!
//! Store the SemanticIndex AS A BLOB (postcard-encoded, same format
//! as the legacy .bin file) inside a SQLite row. The runtime search
//! still loads the blob into the existing in-memory `SemanticIndex`
//! struct and queries it there. That keeps the search algorithm
//! unchanged while consolidating the on-disk layout.
//!
//! ## Phase 2 (future iteration)
//!
//! Switch to a relational schema:
//!   - `symbols(id, project_path, file, line, kind, name, signature, doc)`
//!   - `vec_symbols(rowid, embedding)` via sqlite-vec
//!   - `tokens(token, symbol_id)` for BM25 inverted index
//!
//! That lets server-side SQL do cross-project filtering AND
//! enables per-symbol embedding deduplication.
//!
//! ## Schema (phase 1)
//!
//! ```sql
//! CREATE TABLE semantic_indexes (
//!     project_path TEXT PRIMARY KEY,
//!     blob         BLOB NOT NULL,        -- postcard SemanticIndex
//!     last_indexed INTEGER NOT NULL,     -- unix seconds
//!     size_bytes   INTEGER NOT NULL,
//!     file_count   INTEGER NOT NULL,
//!     symbol_count INTEGER NOT NULL
//! );
//! CREATE INDEX idx_semantic_last ON semantic_indexes(last_indexed);
//! ```

use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

/// Open (or create) the global semantic database at
/// `~/.tokenade/semantic.db`. Returns the connection ready for
/// reads / writes. Schema is created lazily on first open.
pub fn open() -> Result<Connection, rusqlite::Error> {
    let path = db_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let conn = Connection::open(&path)?;
    // Step 1: bare-minimum schema that pre-L62 DBs ALSO have. We
    // explicitly omit `last_used` from CREATE TABLE so the same
    // execute_batch works on both fresh AND legacy DBs.
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS semantic_indexes (
            project_path TEXT PRIMARY KEY,
            blob         BLOB NOT NULL,
            last_indexed INTEGER NOT NULL,
            size_bytes   INTEGER NOT NULL,
            file_count   INTEGER NOT NULL DEFAULT 0,
            symbol_count INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_semantic_last
            ON semantic_indexes(last_indexed);
        "#,
    )?;
    // Step 2: L62 — add `last_used` column on legacy DBs (no-op on
    // fresh ones once it succeeds). Errors are swallowed: on a fresh
    // DB the second open would error "duplicate column", which is
    // benign.
    let _ = conn.execute(
        "ALTER TABLE semantic_indexes ADD COLUMN last_used INTEGER NOT NULL DEFAULT 0",
        [],
    );
    // Step 3: create the access-based index AFTER the column exists.
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_semantic_last_used ON semantic_indexes(last_used)",
        [],
    );
    Ok(conn)
}

/// L62 TTL — 7 days of "untouched by any search" → eviction.
pub const CACHE_TTL_SECS: u64 = 7 * 86_400;

/// Drop projects whose `last_used` is older than [`CACHE_TTL_SECS`].
///
/// Two eviction conditions:
///   * Standard: `last_used > 0 AND last_used < cutoff`. Pre-L62 rows
///     (`last_used=0`) are kept by this clause until they get touched
///     once — then they switch to the access-based TTL.
///   * Zero-content (#446): `file_count = 0 AND last_indexed < cutoff`.
///     Orphan rows created by tests or aborted initialisations that
///     never indexed real content. Production audit found 83+ such
///     rows from `/tmp/claude-1000/tokenade-mcp-test-*` tempdirs;
///     they have no content to lose, and any real project with this
///     shape gets re-indexed on the next save.
pub fn evict_stale(conn: &Connection) -> Result<usize, rusqlite::Error> {
    let cutoff = crate::platform::now_unix_secs().saturating_sub(CACHE_TTL_SECS) as i64;
    let standard = conn.execute(
        "DELETE FROM semantic_indexes WHERE last_used > 0 AND last_used < ?1",
        params![cutoff],
    )?;
    // Zero-content rows: shorter TTL (1 day) because they hold
    // nothing of value — real projects always index at least one
    // file. A fresh save will recreate the row trivially.
    const ORPHAN_TTL_SECS: u64 = 86_400; // 1 day
    let orphan_cutoff = crate::platform::now_unix_secs().saturating_sub(ORPHAN_TTL_SECS) as i64;
    let orphans = conn.execute(
        "DELETE FROM semantic_indexes WHERE file_count = 0 AND last_indexed < ?1",
        params![orphan_cutoff],
    )?;
    Ok(standard + orphans)
}

/// Run `VACUUM` to reclaim disk space freed by deleted rows. SQLite
/// keeps deleted pages in a freelist by default — VACUUM rewrites
/// the entire DB without the freelist gaps. Best-effort; returns
/// (bytes_before, bytes_after) so the caller can report the win.
/// Returns (0, 0) on error.
pub fn vacuum_db() -> (u64, u64) {
    crate::store_scrub::vacuum_store(&db_path(), open)
}

/// Path to the global semantic db. Honours `TOKENADE_SEMANTIC_DB`.
///
/// This used to say "no env-var override — the design is that there's exactly
/// ONE DB per machine". That is true of `dedup.db`, `index.db`,
/// `fold_store.db` and `file_search.db` as well, and all four take an override,
/// so the rationale never distinguished this store; it was simply the one that
/// got missed. The cost was measurable: with no way to redirect it, the tests
/// that exercise this module wrote into the developer's REAL
/// `~/.tokenade/semantic.db` — 81 rows keyed on `/tmp/tokenade-sem-<pid>-…`
/// found there. A suite that mutates real user state is one bad `DELETE` away
/// from destroying it, and this crate has already had to fix a test that
/// re-chmod'd 3396 real files.
pub fn db_path() -> PathBuf {
    if let Some(p) = std::env::var_os("TOKENADE_SEMANTIC_DB") {
        return PathBuf::from(p);
    }
    #[cfg(test)]
    return crate::index_db::per_test_store("semantic.db");
    #[cfg(not(test))]
    crate::platform::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".tokenade")
        .join("semantic.db")
}

/// Store the project's semantic index blob, replacing any prior
/// entry. `blob` is the postcard-encoded SemanticIndex; we don't
/// peek into it here so this module stays independent of the
/// SemanticIndex struct definition.
///
/// `file_count` / `symbol_count` are stored for telemetry — they
/// power `tokenade semantic-status` summaries without needing to
/// deserialize the blob.
pub fn save_blob(
    conn: &Connection,
    project_path: &Path,
    blob: &[u8],
    file_count: usize,
    symbol_count: usize,
) -> Result<(), rusqlite::Error> {
    let key = project_path.display().to_string();
    // #734, applied where it was missing: a test harness that redirects the
    // cwd but not HOME writes into the REAL store, and this table keys rows by
    // project path. Measured 2026-09-03: 38 of 104 rows here were throwaway
    // tempdirs, and `semantic-status` reported 52 projects where 14 exist.
    // Silently skipping is right — the index is a cache, so not writing it
    // costs a re-index and nothing else.
    if crate::stores::write_would_leak_into_real_store(&key, conn.path()) {
        return Ok(());
    }
    let now = crate::platform::now_unix_secs();
    conn.execute(
        "INSERT INTO semantic_indexes(project_path, blob, last_indexed, size_bytes, file_count, symbol_count)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(project_path) DO UPDATE SET
             blob=?2, last_indexed=?3, size_bytes=?4, file_count=?5, symbol_count=?6",
        params![key, blob, now as i64, blob.len() as i64, file_count as i64, symbol_count as i64],
    )?;
    Ok(())
}

/// Load the project's semantic-index blob, or None if no entry
/// exists for this project. Caller deserializes via postcard.
///
/// Side effect: bumps `last_used=now()` so the L62 access-based
/// TTL eviction won't drop an actively-queried project. Cheap —
/// one UPDATE per hit on the hot search path.
pub fn load_blob(
    conn: &Connection,
    project_path: &Path,
) -> Result<Option<Vec<u8>>, rusqlite::Error> {
    let key = project_path.display().to_string();
    let mut stmt = conn.prepare("SELECT blob FROM semantic_indexes WHERE project_path=?1")?;
    let mut rows = stmt.query(params![&key])?;
    if let Some(row) = rows.next()? {
        let blob = row.get::<_, Vec<u8>>(0)?;
        // Drop the rows iterator before issuing the UPDATE so we don't
        // hold a read lock across a write.
        drop(rows);
        drop(stmt);
        let _ = conn.execute(
            "UPDATE semantic_indexes SET last_used=?1 WHERE project_path=?2",
            params![crate::platform::now_unix_secs() as i64, key],
        );
        Ok(Some(blob))
    } else {
        Ok(None)
    }
}

/// One row of [`list_projects`]: `(project_path, last_indexed_secs,
/// size_bytes, file_count, symbol_count)`. Named so the clippy
/// `type_complexity` lint stops flagging the inline tuple.
pub use crate::store_scrub::ProjectSummary;

/// List every project currently in the global semantic DB along
/// with its summary stats. Sorted most-recently-indexed first.
/// Powers the `semantic_status` MCP tool + `tokenade semantic-status`
/// CLI.
pub fn list_projects(conn: &Connection) -> Result<Vec<ProjectSummary>, rusqlite::Error> {
    crate::store_scrub::list_projects(conn, "semantic_indexes", "last_indexed")
}

/// Remove the entry for a project (e.g. when the project dir was
/// deleted). Returns the number of rows affected (0 or 1).
pub fn forget(conn: &Connection, project_path: &Path) -> Result<usize, rusqlite::Error> {
    crate::store_scrub::forget_project(conn, "semantic_indexes", project_path)
}

/// The scan every purge below runs. One string, because the column names are
/// the same question asked of the same table three times.
const SELECT_BLOBS: &str = "SELECT project_path, blob FROM semantic_indexes";

/// Drop every project index whose stored blob would be changed by the redact
/// patterns. Returns the number removed (or, in a dry run, matched).
///
/// The needle-free sibling of [`purge_containing`], for `scrub-cache`: it does
/// not know WHICH secret leaked, only that the patterns recognise one. Same
/// remedy for the same reason — the blob is postcard-encoded and the index is
/// derived data.
///
/// Blobs written since the comment redaction landed cannot match, so on a
/// current install this finds nothing; it exists for the ones written before.
pub fn purge_redactable(dry_run: bool) -> usize {
    // See `dedup_ref::scrub_via_redact_inner`: opening creates the file, and
    // a dry run must not.
    if dry_run && !db_path().exists() {
        return 0;
    }
    let Ok(conn) = open() else {
        return 0;
    };
    let victims =
        crate::store_scrub::victim_keys(&conn, SELECT_BLOBS, crate::store_scrub::is_redactable)
            .unwrap_or_default();
    if dry_run {
        return victims.len();
    }
    let mut removed = 0usize;
    for path in &victims {
        removed += conn
            .execute(
                "DELETE FROM semantic_indexes WHERE project_path=?1",
                params![path],
            )
            .unwrap_or(0);
    }
    if removed > 0 {
        drop(conn);
        let _ = vacuum_db();
    }
    removed
}

/// [`purge_rows_containing`] against the ambient database, for callers that do
/// not carry rusqlite. Returns `(matched, removed)`; a dry run leaves the rows
/// in place and reports what it would take.
pub fn purge_containing(needle: &str, dry_run: bool) -> (usize, usize) {
    let Ok(conn) = open() else {
        return (0, 0);
    };
    if dry_run {
        let n = crate::store_scrub::victim_keys(&conn, SELECT_BLOBS, |blob| {
            crate::store_scrub::contains_bytes(blob, needle)
        })
        .map_or(0, |v| v.len());
        return (n, 0);
    }
    let removed = purge_rows_containing(&conn, needle).unwrap_or(0);
    if removed > 0 {
        // VACUUM, or the bytes stay readable in the file's free pages and the
        // tool reports "scrubbed" about a string `grep` still finds. Same
        // reason `scrub-cache` vacuums dedup.db after redacting it.
        drop(conn);
        let _ = vacuum_db();
    }
    (removed, removed)
}

/// Drop every project index whose stored blob contains `needle`.
///
/// A semantic index is seeded with each file's declared symbols AND its first
/// comments, which is what makes semantic search work — and it means a
/// credential written in a comment is copied into `~/.tokenade/semantic.db`,
/// a global store outside the repository. Measured 2026-08-27: a marker in a
/// doc comment appears there and in none of the other caches.
///
/// DELETE, never patch. The blob is postcard-encoded, so replacing bytes
/// inside it would corrupt the length prefixes that frame every field; and an
/// index is derived data that rebuilds from the source on the next query, so
/// dropping it costs one rebuild and loses nothing.
///
/// Returns the number of project indexes removed.
pub fn purge_rows_containing(conn: &Connection, needle: &str) -> Result<usize, rusqlite::Error> {
    if needle.is_empty() {
        return Ok(0);
    }
    let victims = crate::store_scrub::victim_keys(conn, SELECT_BLOBS, |blob| {
        crate::store_scrub::contains_bytes(blob, needle)
    })?;
    let mut removed = 0usize;
    for path in &victims {
        removed += conn.execute(
            "DELETE FROM semantic_indexes WHERE project_path=?1",
            params![path],
        )?;
    }
    Ok(removed)
}

/// Drop entries for projects whose root dir no longer exists.
/// Returns the count removed. Cheap garbage collection — call
/// occasionally (e.g. on `tokenade install`).
pub fn prune_missing(conn: &Connection) -> Result<usize, rusqlite::Error> {
    let projects = list_projects(conn)?;
    let mut removed = 0;
    for (path, _, _, _, _) in projects {
        if !path.exists() {
            removed += forget(conn, &path)?;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A credential in a source COMMENT reaches `semantic.db`, and the
    /// emergency scrubber must find it.
    ///
    /// The semantic index is seeded with each file's symbols AND its first
    /// comments — that is what makes semantic search work, and it copies the
    /// comment into a global store outside the repository. Measured
    /// 2026-08-27: a marker planted in a doc comment appears in
    /// `semantic.db` and in none of the other caches, while `scrub-secret`
    /// answered "no occurrences found".
    /// `scrub-cache` drops the indexes the patterns recognise, and only those.
    ///
    /// It does not know WHICH secret leaked — only that the redact rules see
    /// one — so it cannot use the needle path. Same remedy for the same
    /// reason: the blob is postcard-encoded and the index is derived data.
    /// Measured on this machine when it landed: 5 of 519 project indexes,
    /// matched by the `providers`, `pii` and `entropy` rules.
    #[test]
    fn purge_redactable_drops_only_what_the_rules_recognise() {
        let db = std::env::temp_dir().join(format!(
            "tokenade-semdb-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&db);
        let _env = crate::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        // SAFETY: env vars are process-wide; ENV_LOCK is held for the guard.
        unsafe {
            std::env::set_var("TOKENADE_SEMANTIC_DB", &db);
        }
        struct Restore;
        impl Drop for Restore {
            fn drop(&mut self) {
                // SAFETY: still inside the ENV_LOCK guard's lifetime.
                unsafe {
                    std::env::remove_var("TOKENADE_SEMANTIC_DB");
                }
            }
        }
        let _restore = Restore;
        {
            let conn = open().unwrap();
            save_blob(
                &conn,
                std::path::Path::new("/tmp/leaky"),
                b"aws_secret_access_key = AKIAIOSFODNN7EXAMPLE",
                1,
                1,
            )
            .unwrap();
            save_blob(
                &conn,
                std::path::Path::new("/tmp/ordinary"),
                b"fn parse_config() -> Config",
                1,
                1,
            )
            .unwrap();
        }
        assert_eq!(purge_redactable(true), 1, "dry run must count one");
        {
            let conn = open().unwrap();
            assert!(
                load_blob(&conn, std::path::Path::new("/tmp/leaky"))
                    .unwrap()
                    .is_some(),
                "a dry run must not delete"
            );
        }
        assert_eq!(purge_redactable(false), 1);
        let conn = open().unwrap();
        assert!(
            load_blob(&conn, std::path::Path::new("/tmp/leaky"))
                .unwrap()
                .is_none(),
            "the matching index survived"
        );
        assert!(
            load_blob(&conn, std::path::Path::new("/tmp/ordinary"))
                .unwrap()
                .is_some(),
            "an ordinary index was dropped"
        );
        drop(conn);
        let _ = std::fs::remove_file(&db);
    }

    #[test]
    fn a_needle_inside_a_stored_blob_is_purged() {
        let conn = tmp_db();
        let needle = "AKIA-TEST-NEEDLE-0123";
        save_blob(
            &conn,
            std::path::Path::new("/tmp/with-secret"),
            format!("noise {needle} noise").as_bytes(),
            1,
            1,
        )
        .unwrap();
        save_blob(
            &conn,
            std::path::Path::new("/tmp/clean"),
            b"nothing to see",
            1,
            1,
        )
        .unwrap();
        assert_eq!(
            purge_rows_containing(&conn, needle).unwrap(),
            1,
            "the row holding the needle must be dropped"
        );
        assert!(
            load_blob(&conn, std::path::Path::new("/tmp/with-secret"))
                .unwrap()
                .is_none(),
            "the affected index survived"
        );
        assert!(
            load_blob(&conn, std::path::Path::new("/tmp/clean"))
                .unwrap()
                .is_some(),
            "an unaffected project must not be dropped"
        );
        // An empty needle must not empty the store.
        assert_eq!(purge_rows_containing(&conn, "").unwrap(), 0);
    }

    fn tmp_db() -> Connection {
        // Use a fresh in-memory DB per test to avoid touching the
        // real ~/.tokenade/semantic.db AND to keep tests isolated.
        // Schema mirrors the production migration (open() adds
        // `last_used` via ALTER TABLE).
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE semantic_indexes (
                project_path TEXT PRIMARY KEY,
                blob         BLOB NOT NULL,
                last_indexed INTEGER NOT NULL,
                size_bytes   INTEGER NOT NULL,
                file_count   INTEGER NOT NULL DEFAULT 0,
                symbol_count INTEGER NOT NULL DEFAULT 0,
                last_used    INTEGER NOT NULL DEFAULT 0
            );
            "#,
        )
        .unwrap();
        conn
    }

    /// Same schema, but on DISK — the tempdir guard asks what a write
    /// targets, and an in-memory database is nobody's store.
    fn file_db(dir: &std::path::Path) -> Connection {
        let conn = Connection::open(dir.join("store.db")).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE semantic_indexes (
                project_path TEXT PRIMARY KEY,
                blob         BLOB NOT NULL,
                last_indexed INTEGER NOT NULL,
                size_bytes   INTEGER NOT NULL,
                file_count   INTEGER NOT NULL DEFAULT 0,
                symbol_count INTEGER NOT NULL DEFAULT 0,
                last_used    INTEGER NOT NULL DEFAULT 0
            );
            "#,
        )
        .unwrap();
        conn
    }

    #[test]
    fn save_and_load_round_trip() {
        let conn = tmp_db();
        let p = PathBuf::from("/home/me/proj-a");
        let blob = b"\x01\x02\x03 postcard-bytes \xff".to_vec();
        save_blob(&conn, &p, &blob, 42, 123).unwrap();
        let loaded = load_blob(&conn, &p).unwrap().unwrap();
        assert_eq!(loaded, blob);
    }

    #[test]
    fn second_save_replaces_first() {
        let conn = tmp_db();
        let p = PathBuf::from("/home/me/proj-a");
        save_blob(&conn, &p, b"v1", 1, 1).unwrap();
        save_blob(&conn, &p, b"v2-larger", 5, 50).unwrap();
        let loaded = load_blob(&conn, &p).unwrap().unwrap();
        assert_eq!(loaded, b"v2-larger");
        let projects = list_projects(&conn).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].3, 5); // file_count from latest save
    }

    #[test]
    fn list_projects_sorted_recent_first() {
        let conn = tmp_db();
        let pa = PathBuf::from("/home/me/old");
        let pb = PathBuf::from("/home/me/new");
        save_blob(&conn, &pa, b"old", 1, 1).unwrap();
        save_blob(&conn, &pb, b"new", 1, 1).unwrap();
        // Hand-craft last_indexed instead of relying on a sleep between the
        // two saves: save_blob stamps last_indexed=now() in *whole seconds*,
        // and under VirtualBox the guest clock can drift backwards on resume,
        // so the older save occasionally outranked the newer one (flaky only
        // on the Windows/macOS release VMs). Setting explicit values makes
        // the ordering depend solely on the ORDER BY, not on wall-clock.
        let set = |p: &PathBuf, t: i64| {
            conn.execute(
                "UPDATE semantic_indexes SET last_indexed = ?1 WHERE project_path = ?2",
                params![t, p.display().to_string()],
            )
            .unwrap();
        };
        set(&pa, 100);
        set(&pb, 200);
        let projects = list_projects(&conn).unwrap();
        assert_eq!(projects.len(), 2);
        assert_eq!(projects[0].0, pb);
        assert_eq!(projects[1].0, pa);
    }

    #[test]
    fn forget_removes_single_entry() {
        let conn = tmp_db();
        let p = PathBuf::from("/home/me/proj");
        save_blob(&conn, &p, b"x", 1, 1).unwrap();
        let removed = forget(&conn, &p).unwrap();
        assert_eq!(removed, 1);
        assert!(load_blob(&conn, &p).unwrap().is_none());
    }

    #[test]
    fn evict_stale_drops_zero_file_orphans_past_ttl() {
        // Regression for #446: tests / aborted init save 0-file rows
        // that survived the standard `last_used > 0` TTL clause. The
        // new orphan clause must drop them when `last_indexed` is
        // older than the TTL window.
        let conn = tmp_db();
        let orphan = PathBuf::from("/tmp/claude-1000/tokenade-mcp-test-X");
        // Save with file_count=0 (the test-fixture shape).
        save_blob(&conn, &orphan, b"", 0, 0).unwrap();
        // Manually antedate `last_indexed` past the orphan TTL
        // (1 day) so the orphan clause triggers.
        let stale = crate::platform::now_unix_secs().saturating_sub(2 * 86_400) as i64;
        conn.execute(
            "UPDATE semantic_indexes SET last_indexed = ?1 WHERE project_path = ?2",
            params![stale, orphan.display().to_string()],
        )
        .unwrap();
        // Add one fresh non-orphan entry — must survive.
        let fresh = PathBuf::from("/home/me/real-project");
        save_blob(&conn, &fresh, b"v1", 10, 100).unwrap();

        let dropped = evict_stale(&conn).unwrap();
        assert_eq!(dropped, 1, "orphan must be evicted");
        assert!(load_blob(&conn, &orphan).unwrap().is_none());
        assert!(load_blob(&conn, &fresh).unwrap().is_some());
    }

    #[test]
    fn evict_stale_keeps_fresh_zero_file_entries() {
        // A zero-file entry from RIGHT NOW (e.g. fresh project that
        // hasn't been indexed yet) must NOT be evicted — only stale
        // ones (last_indexed past TTL) are dropped.
        let conn = tmp_db();
        let recent = PathBuf::from("/home/me/just-created");
        save_blob(&conn, &recent, b"", 0, 0).unwrap();
        let dropped = evict_stale(&conn).unwrap();
        assert_eq!(dropped, 0);
        assert!(load_blob(&conn, &recent).unwrap().is_some());
    }

    #[test]
    fn a_test_tempdir_never_reaches_the_project_index() {
        // #734 guarded the gain ledger and nothing else, so a harness that
        // redirects the cwd but not HOME wrote into the REAL store: measured
        // 2026-09-03, 38 of 104 rows here and 85 of 180 in `index.db` were
        // throwaway tempdirs, the newest from that morning, and
        // `semantic-status` reported 52 projects where 14 existed.
        // The guard asks whether the write would land in the USER's store:
        // a tempdir cwd with a REAL home. An isolated test redirects both and
        // must still be able to save, or the guard breaks the suite instead of
        // the leak — which is exactly what happened on the first attempt.
        // HOME is process-wide and this test moves it, so it must take the
        // crate-wide guard: without it a sibling `index_db` test saving under
        // a tempdir path ran while HOME said "/home/somebody-real" and had its
        // write refused. Same lesson as every other env-touching test here —
        // a lock only protects the parties that take it.
        let _g = crate::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // BOTH vars, and not as belt-and-braces: `platform::home_dir` reads
        // USERPROFILE first on Windows, so a test that moves only HOME moves
        // nothing there — the guard then asks about the REAL profile and
        // refuses the isolated write. Invisible on Unix, where HOME wins.
        let prev_home = std::env::var_os("HOME");
        let prev_profile = std::env::var_os("USERPROFILE");
        std::env::set_var("HOME", "/home/somebody-real");
        std::env::set_var("USERPROFILE", "/home/somebody-real");
        // A FILE-backed connection: the guard asks what the write targets, and
        // an in-memory database is nobody's store.
        let dir = std::env::temp_dir().join(format!(
            "tokenade-guard-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let conn = file_db(&dir);
        for p in [
            "/tmp/claude-1000/tokenade-cli-cwd-12345-67890",
            "/tmp/tokenade-core-gain-noop-1-2",
            "/var/folders/x/tokenade-mcp-fixture-1",
            "/tmp/cargo-test-1234",
        ] {
            save_blob(&conn, std::path::Path::new(p), b"blob", 1, 1).unwrap();
        }
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM semantic_indexes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "a test tempdir was written into the project index");

        // A real project path still saves — the guard must not swallow work.
        // This one contains "tokenade" without being a tempdir, which is the
        // shape #734's own test pins on the predicate.
        save_blob(
            &conn,
            std::path::Path::new("/home/me/projects/tokenade"),
            b"blob",
            3,
            7,
        )
        .unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM semantic_indexes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "a real project was dropped by the tempdir guard");

        // Now the isolated case: HOME is a tempdir too, so the store is the
        // test's own and the write must go through.
        std::env::set_var("HOME", "/tmp/claude-1000/tokenade-cli-cwd-999-1");
        std::env::set_var("USERPROFILE", "/tmp/claude-1000/tokenade-cli-cwd-999-1");
        save_blob(
            &conn,
            std::path::Path::new("/tmp/claude-1000/tokenade-cli-cwd-999-1"),
            b"blob",
            1,
            1,
        )
        .unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM semantic_indexes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            n, 2,
            "an isolated test (cwd AND home both temp) was refused, so the \
             guard breaks the suite rather than the leak"
        );
        match prev_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        match prev_profile {
            Some(h) => std::env::set_var("USERPROFILE", h),
            None => std::env::remove_var("USERPROFILE"),
        }
        drop(conn);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_missing_drops_nonexistent_paths() {
        let conn = tmp_db();
        // A bogus path that DEFINITELY doesn't exist (random nano-id).
        let bogus = PathBuf::from(format!(
            "/tmp/tokenade-semantic-db-nonexistent-{}",
            std::process::id()
        ));
        save_blob(&conn, &bogus, b"x", 1, 1).unwrap();
        // Real path (test bin's dir exists).
        let real = std::env::temp_dir();
        save_blob(&conn, &real, b"y", 1, 1).unwrap();
        let pruned = prune_missing(&conn).unwrap();
        assert_eq!(pruned, 1);
        assert!(load_blob(&conn, &bogus).unwrap().is_none());
        assert!(load_blob(&conn, &real).unwrap().is_some());
    }
}
