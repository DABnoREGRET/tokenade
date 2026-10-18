//! **The `§ref:<hash>§` marker** — what replaces the bytes.
//!
//! The `root: &Path` parameter is kept on these functions for API
//! stability towards the dozen callers in `tokenade-cli`, but it is
//! IGNORED: storage is global.
//!
//! The marker's TAIL is the detail that makes the difference. A bare `§ref` says
//! "this is the same as before" and forces the agent to take it on trust;
//! a few trailing lines let it check that it really is the same
//! content without asking for anything again.

use super::*;

// ── Public API ──────────────────────────────────────────────────────
//
// The `root: &Path` parameter is kept on all functions for API stability
// across the dozen callers in tokenade-cli, but it is IGNORED — storage
// is global. Callers can keep passing `cwd` (the historical value) and
// the migration is transparent to them.

/// Look up a hash in the dedup store. Returns the cached content if
/// present AND younger than [`CACHE_TTL_SECS`]. Bumps `last_used` so
/// frequently-reused entries stay alive (LRU-style refresh).
pub fn lookup(_root: &Path, hash: &str) -> Option<String> {
    let conn = open_conn().ok()?;
    let now = crate::platform::now_unix_secs();
    let ttl_cutoff = now.saturating_sub(CACHE_TTL_SECS) as i64;
    let row: Option<(String, i64)> = conn
        .query_row(
            "SELECT content, last_used FROM dedup_content WHERE hash=?1",
            params![hash],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
        )
        .ok();
    let (content, last_used) = row?;
    if last_used < ttl_cutoff {
        // Stale — purge and miss.
        let _ = conn.execute("DELETE FROM dedup_content WHERE hash=?1", params![hash]);
        return None;
    }
    // LRU bump.
    let _ = conn.execute(
        "UPDATE dedup_content SET last_used=?1 WHERE hash=?2",
        params![now as i64, hash],
    );
    Some(content)
}

/// Write-barrier helper: for each still-resident hash, bump `last_used`
/// to *now* and return its `(hash, byte_count)`. Used by the PreCompact
/// hook to protect the refs that are live in the about-to-be-summarised
/// transcript from the LRU size-cap eviction (`evict_to_size_cap`) — the
/// agent will want to `expand_ref` them *after* the lossy summary, so we
/// must not let them be the eviction victims in the meantime.
///
/// One SELECT to discover residents, one UPDATE to refresh them; missing
/// hashes are silently skipped (they may live only in the disclose stash,
/// which `expand_ref` falls back to and which has its own LRU cap).
pub fn refresh_refs(hashes: &[String]) -> Vec<(String, u64)> {
    if hashes.is_empty() {
        return Vec::new();
    }
    let Ok(conn) = open_conn() else {
        return Vec::new();
    };
    let now = crate::platform::now_unix_secs() as i64;
    let placeholders = vec!["?"; hashes.len()].join(",");
    let params_vec: Vec<&dyn rusqlite::ToSql> =
        hashes.iter().map(|h| h as &dyn rusqlite::ToSql).collect();

    let mut found: Vec<(String, u64)> = Vec::new();
    if let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT hash, byte_count FROM dedup_content WHERE hash IN ({placeholders})"
    )) {
        if let Ok(rows) = stmt.query_map(params_vec.as_slice(), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        }) {
            for row in rows.flatten() {
                found.push((row.0, row.1.max(0) as u64));
            }
        }
    }

    if !found.is_empty() {
        let upd_ph = vec!["?"; found.len()].join(",");
        let mut up: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(found.len() + 1);
        up.push(&now);
        for (h, _) in &found {
            up.push(h as &dyn rusqlite::ToSql);
        }
        let _ = conn.execute(
            &format!("UPDATE dedup_content SET last_used=? WHERE hash IN ({upd_ph})"),
            up.as_slice(),
        );
    }
    found
}

/// Register (or refresh) a hash → content mapping. Cheap — one SQL
/// upsert per call. Idempotent: writing the same hash twice just
/// refreshes `last_used`.
pub fn register(_root: &Path, hash: &str, content: &str) {
    let Ok(conn) = open_conn() else { return };
    let now = crate::platform::now_unix_secs();
    let _ = conn.execute(
        "INSERT INTO dedup_content(hash, content, byte_count, last_used)
         VALUES(?1, ?2, ?3, ?4)
         ON CONFLICT(hash) DO UPDATE SET last_used=?4",
        params![hash, content, content.len() as i64, now as i64],
    );
}

/// Build the inline-reference marker. Format:
///
/// ```text
/// §ref:<hash>§ (NNN bytes, identical to a prior call — fetch via
/// tokenade expand-ref <hash> if needed)
/// ```
///
/// The "fetch via tokenade expand-ref" hint matters: without
/// it the model often re-invokes the original command to "see the
/// content again", defeating the savings.
pub fn ref_marker(hash: &str, byte_count: usize) -> String {
    // Announce the SIZE IN TOKENS, not just bytes: the agent is choosing how to
    // spend its context, and tokens is the unit it spends. Seeing "~14k tokens"
    // makes "ask a question" the obvious move without us having to forbid
    // anything — the number does the steering that a nudge cannot.
    let toks = byte_count / 4;
    format!(
        "§ref:{hash}§ (~{toks} tokens / {byte_count} bytes, identical to a prior call — \
        ask it what you need: `tokenade expand-ref {hash} --prompt \"q1, q2\"`)"
    )
}

/// How much of a repeated output's TAIL rides along with the pointer.
///
/// The same product answers "byte-identical to the previous run" twice, on two
/// routes, and only one of them is answerable. `exec-dedup` gives the age, the
/// exit code and the LAST FIVE LINES; `ref_marker` gave a hash and a size.
/// MEASURED 2026-09-05 over this machine's transcripts: of 208 dedup-folded
/// results delivered to an agent, the commands behind them are overwhelmingly
/// log polling — `tail -30 <log>`, `sleep 90; tail -16 <log>`,
/// `until [ -s <log> ]` — and 14 of those runs were followed by the agent
/// going AROUND tokenade (`tokenade raw`, `TOKENADE_HOOK_DISABLED=1`) to read
/// the bytes it had just been refused. For a poll, the tail IS the answer: it
/// says whether the job moved. A bare pointer makes the agent spend a turn to
/// learn "no".
pub(crate) const REF_TAIL_LINES: usize = 3;
pub(crate) const REF_TAIL_BYTES: usize = 240;

/// The last few lines of `content`, trimmed to [`REF_TAIL_BYTES`].
pub(crate) fn ref_tail(content: &str) -> String {
    let lines: Vec<&str> = content.trim_end().lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    let n = lines.len().min(REF_TAIL_LINES);
    let mut tail = lines[lines.len() - n..].join("\n");
    if tail.len() > REF_TAIL_BYTES {
        let mut cut = tail.len() - REF_TAIL_BYTES;
        while !tail.is_char_boundary(cut) {
            cut += 1;
        }
        tail = format!("…{}", &tail[cut..]);
    }
    tail
}

/// [`ref_marker`] carrying the tail of the content it replaces, when the tail
/// still leaves the pointer comfortably smaller than the bytes.
///
/// The margin is the SAME one [`worth_a_ref`] states — twice what is emitted —
/// applied to what is actually emitted here rather than to the bare marker.
/// A short output therefore keeps the bare pointer or, below the floor, is not
/// folded at all; a long one pays for its own tail many times over.
pub fn ref_marker_with_tail(hash: &str, byte_count: usize, content: &str) -> String {
    let bare = ref_marker(hash, byte_count);
    let tail = ref_tail(content);
    if tail.trim().is_empty() {
        return bare;
    }
    let n = tail.lines().count();
    let with = format!("{bare}\nLast {n} line(s), unchanged:\n{tail}");
    if byte_count >= 2 * with.len() {
        with
    } else {
        bare
    }
}
