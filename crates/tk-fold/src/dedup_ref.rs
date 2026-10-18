//! Inline-reference deduplication of recurring tool output.
//!
//! Inspired by SQZ's `cache_manager.rs` shorthand. When the same
//! command produces identical (post-compaction) output a second
//! time, we substitute the bytes with a small `§ref:<hash>§ (content
//! N bytes, …)` marker. The agent can fetch the full bytes back via
//! the MCP `expand_ref` tool when it needs them; otherwise the
//! marker alone is enough context (the prior turn already showed
//! the content in full).
//!
//! Why this saves so much: agentic workflows re-run the same probe
//! (`cat package.json`, `git status`, `ls src/`) MANY times across a
//! session. Each call today costs the full byte count. With dedup,
//! the second call onwards costs ~30 tokens of ref marker.
//!
//! ## Scope: GLOBAL (since L53, 2026-05-29)
//!
//! Storage is a single SQLite at `~/.tokenade/dedup.db`. Content is
//! hash-keyed — the same `git status` output produced in project A
//! and project B collides correctly (and that's a feature: cross-
//! project dedup). Path-pointers (used by L13 diff-Read) embed the
//! ABSOLUTE file path in their key, so they remain project-unique.
//!
//! Override the DB location via `TOKENADE_DEDUP_DB` — tests use this
//! for isolation.
//!
//! The legacy file-based store at `<root>/.tokenade/dedup/` is migrated
//! into the global DB by `project_scaffold::migrate_local_dedup_store`
//! on first scaffold of a project that still has one.
//!
//! Hash: SHA-256 of the compact output, truncated to 12 hex chars
//! (48 bits — far more than enough for collision avoidance within a
//! single session).

mod evict;
mod marker;
mod pointers;
mod ranges;
mod sha256;
mod store;
pub use evict::*;
pub use marker::*;
pub use pointers::*;
pub use ranges::*;
pub use sha256::*;
pub use store::*;

use rusqlite::{params, Connection};
use std::fmt::Write;
use std::path::{Path, PathBuf};

/// Bytes-on-disk threshold. Outputs smaller than this skip the
/// dedup path entirely.
///
/// Calibration: the bare `§ref:HASH§ (Nbytes)` marker is ~30 chars,
/// but the full Read/Bash deny+reason wrapper (banner line +
/// "file `{path}` is identical… expand_ref hash=… to re-fetch")
/// runs ~144 tokens (~575 bytes). Production debug.log: 16+
/// `read_content_dedup_skipped_inflation` events on files in the
/// 200–600 byte range where the marker would dwarf the content.
/// At 1500 bytes (~375 tokens), the content is comfortably 2× the
/// wrapper, so dedup nets a real win even after the wrapper cost.
pub const MIN_BYTES_TO_DEDUPE: usize = 1500;

/// The `§ref:` marker's own cost in bytes — an upper bound, measured.
///
/// 2026-09-04, the same content read twice through `tokenade read`:
/// `§ref:f1bc211a6ba4§ (~529 tokens / 2118 bytes, identical to a prior call —
/// ask it what you need: ...)` is **154 bytes**, and only the two numbers vary
/// with the content. `MIN_BYTES_TO_DEDUPE`'s reasoning above quotes ~575 bytes
/// for that marker: that is what it cost when the constant was chosen, and it
/// is a quarter of that now. A floor derived from the old figure refuses folds
/// that are plainly wins, so this is stated separately, and a test pins it to
/// the marker the product actually emits.
pub const REF_MARKER_BYTES: usize = 170;

/// Is this much DELIVERED content worth replacing with a `§ref:` marker?
///
/// The pointer has to be smaller than the bytes it replaces, and comfortably:
/// recovering it costs the agent a whole turn, so a fold that saves a handful
/// of bytes is a loss. Twice the marker is the margin the delivery path has
/// always asserted (`second.len() * 2 < first.len()`); this is that same
/// judgement, stated where the decision is made instead of only where it is
/// checked.
///
/// The `wrap`/`read` path — the one `tokenade read` and every plugin bridge go
/// through — asked nothing at all. Measured 2026-09-04: a 188-byte file came
/// back as the marker, the content minus 34 bytes plus a turn to get it back,
/// and an EMPTY file came back as those same bytes for **zero**, telling the
/// agent to "ask it what you need" about nothing and calling a file it had
/// never seen "identical to a prior call".
pub fn worth_a_ref(byte_count: usize) -> bool {
    byte_count >= 2 * REF_MARKER_BYTES
}

/// Cache TTL — entries older than this are eligible for eviction by
/// [`evict_stale`]. The hook itself just refuses to USE stale entries
/// (returns None from [`lookup`]) so an evict pass isn't required for
/// correctness, only for disk hygiene.
pub const CACHE_TTL_SECS: u64 = 7 * 86_400; // 7 days

/// Total-byte budget for the `dedup_content` store. We persist the FULL
/// compact bytes of every cached output (so `expand_ref` can serve them
/// back), but measured reuse of output-dedup entries is ~1.5% — the
/// store is overwhelmingly write-once. Left unbounded it grows without
/// limit (production audit: 356 MB across ~8k rows). A byte budget plus
/// LRU eviction (oldest `last_used` first) keeps it bounded at near-zero
/// hit cost. Env-overridable via `TOKENADE_DEDUP_MAX_BYTES`; mirrors
/// [`crate::disclose::stash_max_dir_bytes`].
const DEDUP_MAX_BYTES: u64 = 150 * 1024 * 1024; // 150 MiB

/// Resolve the active dedup-store byte cap. Reads the env var on each
/// call (no caching) so a long-lived MCP server can be re-tuned without
/// a restart. Empty / unparseable / zero values fall back to the
/// compile-time default — same forgiving semantics as
/// [`crate::disclose::stash_max_dir_bytes`].
pub fn dedup_max_bytes() -> u64 {
    match std::env::var("TOKENADE_DEDUP_MAX_BYTES") {
        Ok(s) => s
            .trim()
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .unwrap_or(DEDUP_MAX_BYTES),
        Err(_) => DEDUP_MAX_BYTES,
    }
}

/// One stored dedup record on disk.
#[derive(Debug, Clone)]
pub struct CachedRef {
    /// 12-char hex prefix of the content's SHA-256.
    pub hash: String,
    /// Full compact content the hash maps to.
    pub content: String,
    /// Unix-second when this entry was last written or read.
    pub last_used: u64,
}

#[cfg(test)]
include!("dedup_ref_tests.rs");
