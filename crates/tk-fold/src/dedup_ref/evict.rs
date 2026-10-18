//! **What we throw away, and what we recover from the old world.**
//!
//! A database that never loses anything ends up costing more than it
//! saves. Eviction has two criteria — AGE and SIZE — and they do not
//! say the same thing: an old entry is probably out of
//! context, an oversized database is a disk problem.
//!
//! The migration imports the old per-project store then deletes it: leaving
//! both would let the old one answer a caller who believes it reads the new one.

use super::*;

/// Delete entries older than [`CACHE_TTL_SECS`]. Returns count
/// removed. Safe to call from any tool; idempotent.
///
/// The `_root` parameter is a legacy vestige from the pre-L53 file-based
/// store (`<root>/.tokenade/dedup/`). Dedup data is now in the global
/// SQLite DB and the path is ignored. Pass any value; callers that pass
/// a real project root do so harmlessly.
pub fn evict_stale(_root: &Path) -> Result<usize, rusqlite::Error> {
    let Ok(conn) = open_conn() else { return Ok(0) };
    let cutoff = crate::platform::now_unix_secs().saturating_sub(CACHE_TTL_SECS) as i64;
    let removed_content = conn.execute(
        "DELETE FROM dedup_content WHERE last_used < ?1",
        params![cutoff],
    )?;
    // Path pointers also age out — keeps the table tidy.
    let _ = conn.execute(
        "DELETE FROM dedup_path_pointers WHERE last_updated < ?1",
        params![cutoff],
    );
    // Read-range coverage rows age out on the same TTL.
    let _ = conn.execute(
        "DELETE FROM dedup_read_ranges WHERE last_used < ?1",
        params![cutoff],
    );
    Ok(removed_content)
}

/// LRU size-cap eviction for `dedup_content` (OPP-1). Sums the stored
/// `byte_count`; if it exceeds [`dedup_max_bytes`], deletes rows
/// oldest-`last_used`-first until the store is back under the cap, then
/// VACUUMs ONCE to return the freed pages to the OS (SQLite keeps them
/// on the freelist otherwise, so the file would stay large). Returns
/// `(rows_removed, bytes_freed)`. Best-effort: `(0, 0)` on DB error.
///
/// Complements [`evict_stale`] (age-only): a burst of unique large
/// outputs inside the 7-day TTL window can blow the budget long before
/// any entry is old enough to age out. The path-pointer table is tiny
/// (≈100 KB in production) so it is left to the TTL pass — a pointer
/// orphaned by a content eviction just misses harmlessly in [`lookup`].
pub fn evict_to_size_cap() -> (usize, u64) {
    let Ok(conn) = open_conn() else { return (0, 0) };
    evict_to_size_cap_conn(&conn, dedup_max_bytes())
}

/// Testable inner of [`evict_to_size_cap`] — takes the connection and
/// cap explicitly so tests can drive a tiny budget against synthetic
/// rows. Snapshots the eviction set before issuing any DELETE so the
/// iteration never races its own mutations.
pub(crate) fn evict_to_size_cap_conn(conn: &Connection, cap: u64) -> (usize, u64) {
    let total: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(byte_count), 0) FROM dedup_content",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if (total as u64) <= cap {
        return (0, 0);
    }
    // Walk oldest-`last_used`-first, accumulating bytes until we've
    // marked enough rows to bring the store back under the cap. Mirrors
    // the oldest-mtime-first file sweep in `disclose::evict_to_size_cap`.
    let mut over = (total as u64).saturating_sub(cap);
    let victims: Vec<(String, i64)> = {
        let Ok(mut stmt) = conn
            .prepare("SELECT hash, byte_count FROM dedup_content ORDER BY last_used ASC, hash ASC")
        else {
            return (0, 0);
        };
        let Ok(mapped) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        else {
            return (0, 0);
        };
        let mut v = Vec::new();
        for row in mapped.flatten() {
            if over == 0 {
                break;
            }
            let len = row.1.max(0) as u64;
            over = over.saturating_sub(len);
            v.push(row);
        }
        v
    };
    let mut removed = 0usize;
    let mut bytes_freed = 0u64;
    for (hash, len) in &victims {
        if conn
            .execute("DELETE FROM dedup_content WHERE hash=?1", params![hash])
            .unwrap_or(0)
            > 0
        {
            removed += 1;
            bytes_freed += (*len).max(0) as u64;
        }
    }
    if removed > 0 {
        // Cascade. Evicting a blob leaves every pointer and range that named it
        // referring to nothing, and nothing else ever removed those rows:
        // measured on a dogfood install at 2797 of 4567 pointers (61 %) and 823
        // of 1090 ranges dangling. The lookups are written to fail closed on a
        // missing blob, so this is not a correctness fix — it is the reason the
        // two index tables grew without bound while the table they index is
        // capped.
        let _ = conn.execute(
            "DELETE FROM dedup_path_pointers WHERE content_hash NOT IN \
             (SELECT hash FROM dedup_content)",
            [],
        );
        let _ = conn.execute(
            "DELETE FROM dedup_read_ranges WHERE content_hash NOT IN \
             (SELECT hash FROM dedup_content)",
            [],
        );
        // VACUUM cannot run inside a transaction; we hold none here.
        let _ = conn.execute("VACUUM", []);
    }
    (removed, bytes_freed)
}

/// ── Migration from the legacy per-project file store ───────────────
///
/// Pre-L53 storage was `<root>/.tokenade/dedup/<hash>.txt` plus
/// `<root>/.tokenade/dedup/_paths/<key>.ptr`. This helper imports
/// every still-fresh entry into the global DB and removes the
/// per-project directory. Called by `project_scaffold::ensure` on
/// the project's first MCP touch.
///
/// Returns `Some(action_summary)` if anything was moved, `None`
/// otherwise. Errors are swallowed — migration is best-effort.
pub fn migrate_local_store(root: &Path) -> Option<String> {
    let legacy_dir = root.join(".tokenade").join("dedup");
    if !legacy_dir.is_dir() {
        return None;
    }
    let Ok(conn) = open_conn() else {
        return None;
    };
    let now = crate::platform::now_unix_secs();
    let ttl_cutoff = now.saturating_sub(CACHE_TTL_SECS);
    let mut imported_content = 0usize;
    let mut imported_pointers = 0usize;

    // Content blobs at .tokenade/dedup/<hash>.txt
    if let Ok(entries) = std::fs::read_dir(&legacy_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
            if ext != "txt" {
                continue;
            }
            let Ok(meta) = path.metadata() else { continue };
            let Ok(mtime) = meta.modified() else { continue };
            let last_used = mtime
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(now);
            if last_used < ttl_cutoff {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            // The store is CONTENT-ADDRESSED: the filename is supposed to BE
            // the hash of the bytes inside. Importing on the filename's word
            // alone lets a cloned repository ship `.tokenade/dedup/<hash>.txt`
            // holding anything it likes, and `expand-ref <hash>` would later
            // hand that back as genuine recovered output — the promise that
            // makes lossy folding safe, answered with bytes no command ever
            // produced. A real legacy store always satisfies this check,
            // because an older tokenade wrote it content-addressed.
            if hash_content(content.as_bytes()) != stem {
                crate::debug_log::log(
                    "dedup",
                    "legacy_import_rejected_hash_mismatch",
                    serde_json::json!({"path": path.display().to_string()}),
                );
                continue;
            }
            if conn
                .execute(
                    "INSERT INTO dedup_content(hash, content, byte_count, last_used)
                     VALUES(?1, ?2, ?3, ?4)
                     ON CONFLICT(hash) DO UPDATE SET
                         last_used=MAX(last_used, ?4)",
                    params![stem, content, content.len() as i64, last_used as i64],
                )
                .is_ok()
            {
                imported_content += 1;
            }
        }
    }

    // Path pointers at .tokenade/dedup/_paths/<key>.ptr
    let paths_dir = legacy_dir.join("_paths");
    if let Ok(entries) = std::fs::read_dir(&paths_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
            if ext != "ptr" {
                continue;
            }
            let Ok(meta) = path.metadata() else { continue };
            let Ok(mtime) = meta.modified() else { continue };
            let last_updated = mtime
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(now);
            if last_updated < ttl_cutoff {
                continue;
            }
            let Ok(content_hash) = std::fs::read_to_string(&path) else {
                continue;
            };
            let content_hash = content_hash.trim();
            if content_hash.is_empty() {
                continue;
            }
            if conn
                .execute(
                    "INSERT INTO dedup_path_pointers(path_key, content_hash, last_updated)
                     VALUES(?1, ?2, ?3)
                     ON CONFLICT(path_key) DO UPDATE SET
                         content_hash=?2,
                         last_updated=MAX(last_updated, ?3)",
                    params![stem, content_hash, last_updated as i64],
                )
                .is_ok()
            {
                imported_pointers += 1;
            }
        }
    }

    // Tear the legacy dir down on success — errors swallowed.
    let _ = std::fs::remove_dir_all(&legacy_dir);

    if imported_content == 0 && imported_pointers == 0 {
        Some(".tokenade/dedup (deleted legacy store — all entries stale)".into())
    } else {
        Some(format!(
            ".tokenade/dedup (migrated {imported_content} blob(s) + \
             {imported_pointers} path pointer(s) to global ~/.tokenade/dedup.db)"
        ))
    }
}
