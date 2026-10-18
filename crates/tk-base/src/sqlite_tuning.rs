//! One place for the pragmas every tokenade sqlite database wants.
//!
//! All four stores (`dedup.db`, `fold_store.db`, `file_search.db`,
//! `index.db`) are rebuildable caches keyed by content hash or project path —
//! losing the last write costs a cache miss, never data. Two of them
//! (`dedup.db`, `fold_store.db`) are opened on the hot path of an agent turn,
//! inside a hook the user is waiting on, so open cost is latency the agent
//! pays every time.
//!
//! What we set and why:
//! - `journal_mode=WAL` — readers don't block the writer. Already everywhere.
//! - `synchronous=NORMAL` — one fsync per checkpoint instead of per commit.
//!   Deliberately NOT `OFF`: a competitor's C indexer uses `OFF`, but `OFF`
//!   risks a corrupt WAL on OS crash, and "delete `~/.tokenade/dedup.db` and
//!   lose the whole cache" is a worse failure than "lose the last row".
//! - `mmap_size` — read pages straight from the page cache instead of copying
//!   through a read() per page. `dedup.db` runs to hundreds of MB in a working
//!   install; this is the pragma that matters most for it.
//! - `temp_store=MEMORY` — sorts and temp b-trees stay off disk.
//! - `busy_timeout` — parallel hook processes contend; fail slow, not fast.
//!
//! From the 2026-07-29 competitor scan (codebase-memory-mcp),
//! minus the durability trade we declined.

use rusqlite::Connection;

/// Bytes of the database to memory-map. 256 MiB covers a large working
/// `dedup.db` whole; on 32-bit targets the address space cannot afford that.
#[cfg(target_pointer_width = "64")]
const MMAP_BYTES: i64 = 256 * 1024 * 1024;
#[cfg(not(target_pointer_width = "64"))]
const MMAP_BYTES: i64 = 32 * 1024 * 1024;

/// Apply the shared pragma set. Every pragma is best-effort: an older sqlite
/// that rejects one must not take the database down with it, so failures are
/// swallowed exactly as the per-store call sites did before this was factored
/// out.
pub fn tune(conn: &Connection) {
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    let _ = conn.pragma_update(None, "synchronous", "NORMAL");
    let _ = conn.pragma_update(None, "mmap_size", MMAP_BYTES);
    let _ = conn.pragma_update(None, "temp_store", "MEMORY");
    let _ = conn.busy_timeout(std::time::Duration::from_millis(500));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tuning_sticks_on_a_real_file_database() {
        // Must be a FILE database: sqlite ignores `mmap_size` for an
        // in-memory connection (nothing to map), so testing there would
        // assert on a value the pragma never sets.
        let path = std::env::temp_dir().join(format!(
            "tokenade-sqlite-tuning-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_file(&path);
        let conn = Connection::open(&path).expect("file db");
        tune(&conn);
        let mmap: i64 = conn
            .query_row("PRAGMA mmap_size", [], |r| r.get(0))
            .expect("mmap_size readable");
        assert_eq!(mmap, MMAP_BYTES);
        let temp_store: i64 = conn
            .query_row("PRAGMA temp_store", [], |r| r.get(0))
            .expect("temp_store readable");
        // 2 == MEMORY in sqlite's encoding.
        assert_eq!(temp_store, 2);
        let journal: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .expect("journal_mode readable");
        assert_eq!(journal.to_lowercase(), "wal");
        drop(conn);
        let _ = std::fs::remove_file(&path);
    }
}
