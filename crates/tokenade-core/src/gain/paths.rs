//! Where every ledger lives.
//!
//! Four ledgers (measured, estimated, lost, per-session overhead) under one
//! root, resolved through one function each so a test root redirects all of
//! them at once.

use super::*;

/// Override the project root used to locate `.tokenade/gain.jsonl`. Must be called
/// before any `record` call to take effect (it's a `OnceLock`).
pub fn set_root(p: PathBuf) {
    let _ = ROOT_OVERRIDE.set(p);
}

/// Where this process is currently writing records. Used by tests
/// that asserted line-counts on the ledger after `set_root` redirect.
pub fn current_ledger_path() -> PathBuf {
    jsonl_path()
}

/// Returns the **global** ledger path at `~/.tokenade/gain.jsonl`.
///
/// The ledger is global — not per-project — so `tokenade dashboard` run
/// from any directory shows the union of every project's savings. Each
/// record carries a `project` field set to the cwd at write time, so
/// per-project breakdown is still trivial (filter on `project`).
///
/// Test override via [`set_root`] still works (lands at `<root>/
/// .tokenade/gain.jsonl`) so unit tests stay hermetic.
pub(super) fn jsonl_path() -> PathBuf {
    jsonl_path_for("gain.jsonl")
}

pub(super) fn jsonl_path_for(file_name: &str) -> PathBuf {
    if let Some(p) = ROOT_OVERRIDE.get() {
        return p.join(".tokenade").join(file_name);
    }
    crate::platform::home_dir()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join(".tokenade")
        .join(file_name)
}

/// Ledger of savings LOST to hook bypasses (`TOKENADE_HOOK_DISABLED=1 …`,
/// `tokenade raw …`). Deliberately a SEPARATE file from `gain.jsonl`:
/// a lost-savings row has `before > after` like a real gain row, so if
/// it lived in the same ledger every aggregation surface (dashboard
/// text/json/web, `tokenade gain`, statusline, the monthly license
/// quota) would count tokens that were never actually saved. Keeping
/// the file separate makes non-inflation true by construction — for
/// every current reader AND every older binary version.
pub fn lost_ledger_path() -> PathBuf {
    jsonl_path_for("bypass_lost.jsonl")
}

/// Ledger of APPROXIMATE savings + overhead (see [`record_estimated`]).
/// SEPARATE from `gain.jsonl` for the same non-inflation-by-construction
/// reason as [`lost_ledger_path`]: an exact-ledger reader must never sum
/// an estimate as if it were measured. The dashboard reads this file
/// explicitly and reports it under its own "~estimated" line.
/// Public path to the MEASURED ledger (for the `audit-gains` reconciler).
pub fn measured_ledger_path() -> PathBuf {
    jsonl_path_for("gain.jsonl")
}

pub fn estimated_ledger_path() -> PathBuf {
    jsonl_path_for("gain_estimated.jsonl")
}
