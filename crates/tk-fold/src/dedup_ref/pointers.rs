//! **Pointers by PATH** — to serve a diff rather than a file.
//!
//! Next to the index by identity, we keep "the last known fingerprint
//! for THIS file + offset + limit". When the current bytes match
//! no fingerprint, the `Read` interceptor thus finds the
//! previous version and serves a unified diff instead of the whole file.

use super::*;

// ── Path-keyed pointers (L13 diff-Read support) ─────────────────────
//
// Beyond the identity (content-hash) index used by the bash + Read
// dedupers, we ALSO track "the most recent content hash we cached
// for THIS file+offset+limit key". The Read interceptor uses this
// to find a prior version when the current bytes don't match any
// identity hash, so it can serve a unified diff instead of the
// full new file.

/// Hash a `(file_path, offset, limit)` triple to a stable key for
/// the path-pointer table. We hash so any special characters in the
/// path don't cause issues — the absolute path is in the hash so
/// cross-project safety is automatic.
pub fn path_key(file_path: &str, offset: u64, limit: u64) -> String {
    hash_content(format!("path:{file_path}|{offset}|{limit}").as_bytes())
}

/// Record "for this path key, the latest content we saw hashes to
/// `content_hash`". Idempotent — overwrites the prior pointer.
pub fn register_path_pointer(_root: &Path, path_key: &str, content_hash: &str) {
    let Ok(conn) = open_conn() else { return };
    let now = crate::platform::now_unix_secs();
    let _ = conn.execute(
        "INSERT INTO dedup_path_pointers(path_key, content_hash, last_updated)
         VALUES(?1, ?2, ?3)
         ON CONFLICT(path_key) DO UPDATE SET content_hash=?2, last_updated=?3",
        params![path_key, content_hash, now as i64],
    );
}

/// Look up the most-recent content for a given path key, if any.
/// Returns `Some((prior_content_hash, prior_content))` when the
/// pointer exists AND the content blob is still in the cache;
/// `None` otherwise. Honours [`CACHE_TTL_SECS`] via [`lookup`].
pub fn lookup_by_path(root: &Path, path_key: &str) -> Option<(String, String)> {
    let conn = open_conn().ok()?;
    let prior_hash: Option<String> = conn
        .query_row(
            "SELECT content_hash FROM dedup_path_pointers WHERE path_key=?1",
            params![path_key],
            |r| r.get(0),
        )
        .ok();
    let prior_hash = prior_hash?;
    let content = lookup(root, &prior_hash)?;
    Some((prior_hash, content))
}

/// Like [`lookup_by_path`] but also returns the pointer's
/// `last_updated` unix timestamp. Used by the exec-dedup pass in
/// `tokenade wrap` to report how long a command's output has been
/// unchanged ("same output as N s ago").
pub fn lookup_by_path_with_age(root: &Path, path_key: &str) -> Option<(String, String, u64)> {
    let conn = open_conn().ok()?;
    let row: Option<(String, i64)> = conn
        .query_row(
            "SELECT content_hash, last_updated FROM dedup_path_pointers WHERE path_key=?1",
            params![path_key],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
        )
        .ok();
    let (prior_hash, last_updated) = row?;
    let content = lookup(root, &prior_hash)?;
    // AGE in seconds — the raw `last_updated` EPOCH timestamp used to be
    // returned here while the caller compared it against a small recency
    // WINDOW (`age > 300`?): every epoch value passed that test, so every
    // read-diff candidate was rejected as "stale context" forever. This
    // one-line unit bug silenced the entire diff-delivery feature.
    let age = crate::platform::now_unix_secs().saturating_sub(last_updated.max(0) as u64);
    Some((prior_hash, content, age))
}

/// Stable pointer key for a shell command's output — the exec-dedup
/// equivalent of [`path_key`]. Keyed on (cwd, command line) so the
/// same command run from two different projects never cross-hits.
pub fn command_key(cwd: &str, command: &str) -> String {
    hash_content(format!("cmd:{cwd}|{command}").as_bytes())
}
