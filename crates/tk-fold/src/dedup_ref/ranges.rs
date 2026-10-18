//! **Coverage of read ranges** — the dedup that survives pagination.
//!
//! An agent paging through a big file (`off=0 lim=500`, then `off=100
//! lim=120`, `off=150 lim=20`) escapes identity dedup entirely:
//! each (offset, limit) has a different fingerprint. But the following
//! slices are often CONTAINED in a slice already delivered, hence already
//! in the agent's context.
//!
//! The required invariant is "is the content STILL in context?", and it
//! rests on three conditions: coverage is per SESSION, it is INVALIDATED
//! when the transcript is compacted, and the bytes remain RECOVERABLE through
//! `expand-ref`. Only ranges delivered VERBATIM are recorded, never
//! a compacted view — a `§ref` always designates bytes actually seen.

use super::*;

// ── Read-range coverage (slice-paging dedup) ────────────────────────
//
// Agents that page a large file (`Read off=0 lim=500`, then `off=100
// lim=120`, `off=150 lim=20`, …) miss identity dedup entirely — every
// (offset,limit) hashes differently. But the later slices are often
// FULLY CONTAINED in a slice already shipped, so their bytes are
// already in the agent's context. We track delivered line ranges and,
// when a new read is fully covered, serve a §ref instead of re-shipping.
//
// SAFETY — "is the content still in context?" (required invariant):
//   * SESSION-SCOPED. Coverage only counts ranges delivered in the
//     SAME `session_id`. A new session has a fresh context window, so a
//     range from a prior session is NOT in context and must re-ship.
//   * PRECOMPACT-INVALIDATED. When the host compacts/summarises the
//     transcript, earlier turns may be gone — `clear_session_ranges`
//     (called from the PreCompact hook) drops the session's ranges.
//   * RECOVERABLE. The delivered range's bytes are stored in
//     `dedup_content`, so `tokenade expand-ref <hash>` reconstitutes
//     them on ANY agent / OS even if the agent did lose them.
// Only ranges delivered VERBATIM are registered (never a compacted /
// head-sliced view), so a §ref always points at bytes the agent really
// saw.

/// Stable per-file key for the range table (absolute path, hashed so
/// odd characters are safe; cross-project isolation is automatic).
pub fn range_file_key(file_path: &str) -> String {
    hash_content(format!("range:{file_path}").as_bytes())
}

/// Register a line range `[start_line, end_line)` delivered VERBATIM to
/// the agent in `session_id`, for file version `version` ("mtime|size").
/// `content_hash` must be the hash under which the delivered bytes were
/// stored via [`store`], so a later §ref is recoverable. No-op on a
/// degenerate range.
pub fn register_read_range(
    session_id: &str,
    file_key: &str,
    version: &str,
    start_line: u64,
    end_line: u64,
    content_hash: &str,
) {
    if end_line <= start_line || session_id.is_empty() {
        return;
    }
    let Ok(conn) = open_conn() else { return };
    let _ = conn.execute(
        "INSERT INTO dedup_read_ranges
            (session_id, file_key, version, start_line, end_line, content_hash, last_used)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            session_id,
            file_key,
            version,
            start_line as i64,
            end_line as i64,
            content_hash,
            crate::platform::now_unix_secs() as i64
        ],
    );
}

/// Freshness cutoff timestamp: `now - effective_age`, where `effective_age`
/// is `max_age_secs` clamped to `CACHE_TTL_SECS` (and `0` means "TTL only").
/// Callers pass the host's context-recency window so a range that's still in
/// the DB but probably scrolled out of the live context isn't trusted.
pub(crate) fn freshness_cutoff(max_age_secs: u64) -> i64 {
    let age = if max_age_secs == 0 {
        CACHE_TTL_SECS
    } else {
        max_age_secs.min(CACHE_TTL_SECS)
    };
    crate::platform::now_unix_secs().saturating_sub(age) as i64
}

/// If `[start_line, end_line)` is FULLY covered by the union of ranges
/// delivered for `(session_id, file_key, version)` AND fresher than
/// `max_age_secs` (the context-recency window — belt-and-suspenders on top of
/// session scope: a same-session range older than the window has likely
/// scrolled out of context), return the `content_hash` of one covering range
/// (for the §ref marker + expand-ref). Conservative: any gap → `None`.
pub fn range_is_covered(
    session_id: &str,
    file_key: &str,
    version: &str,
    start_line: u64,
    end_line: u64,
    max_age_secs: u64,
) -> Option<String> {
    if end_line <= start_line || session_id.is_empty() {
        return None;
    }
    let conn = open_conn().ok()?;
    let cutoff = freshness_cutoff(max_age_secs);
    let mut stmt = conn
        .prepare(
            // JOIN, not a plain select: a range whose blob has been EVICTED
            // can no longer prove anything. The caller substitutes the slice
            // with `expand-ref <cover_hash>`, so returning a hash whose row is
            // gone hands the agent a recovery command that resolves to nothing
            // — it loses the lines and has no way back. Measured on a dogfood
            // install before this join: 823 of 1090 range rows pointed at
            // content the size-cap eviction had already dropped.
            //
            // Dropping such a range narrows the coverage window (a gap opens
            // and the sweep below stops there), which is the honest answer:
            // those bytes are no longer recoverable, so they cannot be claimed
            // as already-delivered.
            "SELECT r.start_line, r.end_line, r.content_hash FROM dedup_read_ranges r
             JOIN dedup_content c ON c.hash = r.content_hash
             WHERE r.session_id=?1 AND r.file_key=?2 AND r.version=?3 AND r.last_used>=?4
             ORDER BY r.start_line ASC",
        )
        .ok()?;
    let rows = stmt
        .query_map(params![session_id, file_key, version, cutoff], |r| {
            Ok((
                r.get::<_, i64>(0)? as u64,
                r.get::<_, i64>(1)? as u64,
                r.get::<_, String>(2)?,
            ))
        })
        .ok()?;
    // Sweep merged intervals; track how far contiguous coverage reaches
    // from `start_line`, and remember a hash that participates in it.
    let mut reach = start_line;
    let mut cover_hash: Option<String> = None;
    for row in rows.flatten() {
        let (s, e, h) = row;
        if s > reach {
            break; // gap before we reached the target end
        }
        if e > reach {
            reach = e;
            cover_hash = Some(h);
        }
        if reach >= end_line {
            return cover_hash;
        }
    }
    if reach >= end_line {
        cover_hash
    } else {
        None
    }
}

/// Most-recent content delivered for `(session_id, file_key)`, regardless of
/// version or slice coordinates — the baseline for a slice-tolerant diff when
/// the agent re-reads a CHANGED file at different `offset/limit`. Session-
/// scoped (the baseline is provably still in context); version-AGNOSTIC (the
/// whole point is to diff pre-edit bytes against the post-edit slice). Returns
/// `(content_hash, content)` when the row's blob is still cached, else `None`.
pub fn lookup_session_file_latest(
    session_id: &str,
    file_key: &str,
    max_age_secs: u64,
) -> Option<(String, String)> {
    if session_id.is_empty() {
        return None;
    }
    let conn = open_conn().ok()?;
    let cutoff = freshness_cutoff(max_age_secs);
    let hash: String = conn
        .query_row(
            // rowid DESC breaks last_used ties (two reads in the same second)
            // in favour of the most-recently INSERTED row.
            "SELECT content_hash FROM dedup_read_ranges
             WHERE session_id=?1 AND file_key=?2 AND last_used>=?3
             ORDER BY last_used DESC, rowid DESC LIMIT 1",
            params![session_id, file_key, cutoff],
            |r| r.get(0),
        )
        .ok()?;
    let content = lookup(Path::new("."), &hash)?;
    Some((hash, content))
}

/// Drop every range for `session_id`. Called from the PreCompact hook:
/// once the host summarises the transcript we can no longer assume the
/// earlier turns (hence the delivered bytes) survive in context.
pub fn clear_session_ranges(session_id: &str) {
    if session_id.is_empty() {
        return;
    }
    let Ok(conn) = open_conn() else { return };
    let _ = conn.execute(
        "DELETE FROM dedup_read_ranges WHERE session_id=?1",
        params![session_id],
    );
}
