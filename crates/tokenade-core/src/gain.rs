//! Single, shared ledger of every token-saving operation.
//!
//! Before this module each `tokenade` subsystem (CLI, MCP server, library helpers)
//! invented its own log writer or — more often — recorded nothing at all,
//! which meant `tokenade gain` undercounted real savings. Now every public
//! compactor in the codebase funnels through [`record`] and the resulting
//! JSONL stream is the canonical source of truth for `tokenade gain`.
//!
//! Format: one JSON object per line in `<root>/.tokenade/gain.jsonl`. A best-effort
//! file write — never panics, never blocks the caller (failures are silent).

mod paths;
mod session;
use crate::Savings;
pub use paths::*;
pub use session::*;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// Process-local write lock. Prevents interleaved writes from threads in
/// the same process (`tokenade mcp` serving multiple requests, the watcher
/// triggering an index rebuild that logs cache hits, etc.). Cross-process
/// writes still rely on the OS guaranteeing atomic appends for writes
/// smaller than `PIPE_BUF` (4 KiB on Linux/macOS). That claim used to be
/// justified here with "every record we emit is ≤200 bytes", which was never
/// measured: on a real ledger 2026-09-03, 791 of 875 rows were over 200 bytes
/// and SEVEN were over `PIPE_BUF`, the largest 8 696 — a `sha256sum` whose
/// file list made the `label` field alone 8 359 bytes. The label is bounded at
/// the write site now, which is what makes this paragraph true.
fn write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

// #989 wrote a private `write_atomic_600` helper here; #990 promoted
// it to `crate::platform::write_file_600` so the same logic serves
// CLI sites (bypass-counter, statusline cache) too. Local callers
// just go through the public helper now.

/// Where the JSONL lives. Set with [`set_root`] before the first record call
/// to override the default. That default is `~/.tokenade/gain.jsonl` — the
/// ledger is GLOBAL, not per-project (see `gain::paths::jsonl_path`, which
/// owns the resolution); each row carries a `project` field instead. This
/// comment said `<cwd>/…` and sent a probe looking in the wrong directory on
/// 2026-08-08.
static ROOT_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

/// Max bytes before we rotate the ledger. 10 MiB is enough for ~100k events
/// at ~100 bytes each — months of normal usage. Anything beyond is unbounded
/// growth.
pub const ROTATION_THRESHOLD_BYTES: u64 = 10 * 1024 * 1024;

/// On rotation we keep the last 30 days of records and discard older ones.
pub const ROTATION_WINDOW_SECS: u64 = 30 * 86_400;

// The session key is set ONCE by the hook entry points, right after reading
// their payload; every row this process records then carries a `sess` field.
// Its documentation lives with it, in `session` — this paragraph followed it
// there on 2026-09-13: left here as `///`, it documented the `use` below.
//
// Imported, NOT re-exported: `gain::session_key` must no longer answer,
// otherwise the dependency stays real and only its spelling changed.
use crate::session::{session_key, session_overhead_counter_path};

/// TTL for per-session state files. Sessions are ephemeral, so a file
/// untouched this long belongs to a finished session and is safe to drop —
/// the current session's files carry a fresh mtime and are never swept.
/// Matches the stash TTL ([`crate::disclose::CACHE_TTL_SECS`]).
pub const SESSIONS_TTL_SECS: u64 = 7 * 86_400;

// The ledger is a FILE, a row written into it, and four ways to use it.
// `pub use` puts it all back at the `gain::` path the repo spells everywhere.
mod asked;
mod bank;
pub use asked::{answer_over_blob_asked, prompt_scoped_extract_asked};
pub use bank::bank_fold;
mod as_agent;
mod audit;
mod files;
mod fire_context;
pub use as_agent::{as_agent, set_process_agent};
mod record;
mod rereads;
pub use rereads::{
    fold_rereads, injected_rereads, rewrite_weight, Timeline, EXACT_LABEL, PROXY_LABEL,
    SUBAGENT_BRIEF_REREADS, SUBAGENT_EVENT_REREADS, SUBAGENT_RULES_REREADS,
};
mod scan;
mod weigh;
pub use audit::*;
pub use files::*;
mod nudge_kinds;
pub use nudge_kinds::*;
pub use record::*;
pub use scan::*;
pub use weigh::*;

#[cfg(test)]
include!("gain_tests.rs");
