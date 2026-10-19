//! One-time migration from the 1.x (licensed) line to the open-source 2.x.
//!
//! 1.x kept licence, quota and usage-reporter state under `~/.tokenade`. 2.x
//! reads none of it, so the first 2.x run removes it once (sentinel
//! `oss-migrated`). Hooks and MCP entries are refreshed by the existing
//! version-gated post-upgrade refresh (`hook_health::maybe_run_post_upgrade_refresh`);
//! their format did not change.
//!
//! The one-line notice goes to STDERR only, and only when stderr is a
//! terminal: stdout belongs to hook JSON / MCP JSON-RPC / `eval`'d shell-init,
//! and a hook's stderr is never seen by a person. A machine whose first 2.x
//! runs are all hooks keeps the notice pending until a human runs a command.
//! Everything here is best-effort and silent on failure.

use std::io::IsTerminal;
use std::path::Path;

/// 1.x licence / quota / reporter state, relative to `~/.tokenade`.
pub(crate) const LEGACY_STATE: &[&str] = &[
    "license.key",
    "quota.json",
    "machine-id",
    "report-health.json",
    "referral-code",
    "reporter.lock",
    "reporter-offset",
    "reporter-offset-estimated",
    "net-probe-ts",
    "gain.jsonl.pending-upload.jsonl",
    "gain_estimated.jsonl.pending-upload.jsonl",
    "failopen-log-ts",
    "statusline-promos-off",
    "report/last-upload-ts",
];

/// Left behind by 2.0.0: `reporter.lock` re-created by a 1.x process still
/// running during the swap, and the support bundle of the removed
/// `tokenade report` (a copy of agent transcripts nothing in 2.x reads or
/// sends). Swept once more, separately, so machines already past `DONE` get it.
pub(crate) const LEGACY_STATE_LATE: &[&str] =
    &["reporter.lock", "last-report.zip", "quota.json.bak"];

const DONE: &str = "oss-migrated";
const DONE_LATE: &str = "oss-migrated-2";
const NOTICE_PENDING: &str = "oss-notice-pending";

pub(crate) const NOTICE: &str =
    "Tokenade is now open source (MIT OR Apache-2.0) — no account, no telemetry.";

/// Run at most once per machine; cheap (one `stat`) afterwards.
pub(crate) fn run() {
    let dir = tokenade_core::platform::tokenade_state_dir();
    migrate_at(&dir);
    sweep_late_at(&dir);
    if std::io::stderr().is_terminal() {
        if let Some(msg) = take_notice_at(&dir) {
            eprintln!("[tokenade] {msg}");
        }
    }
}

/// Remove the 1.x state once. Returns how many legacy files were removed.
/// A fresh install (no `~/.tokenade` yet) is left untouched: nothing to
/// migrate, and creating the dir here would be a side effect of `--version`.
pub(crate) fn migrate_at(dir: &Path) -> usize {
    if !dir.is_dir() || dir.join(DONE).exists() {
        return 0;
    }
    let mut removed = 0;
    for rel in LEGACY_STATE {
        if std::fs::remove_file(dir.join(rel)).is_ok() {
            removed += 1;
        }
    }
    let _ = std::fs::remove_dir(dir.join("report")); // only if now empty
    if removed > 0 {
        let _ = std::fs::write(dir.join(NOTICE_PENDING), b"1");
    }
    let _ = std::fs::write(dir.join(DONE), env!("CARGO_PKG_VERSION"));
    tokenade_core::debug_log::log(
        "oss-migration",
        "legacy_state_removed",
        serde_json::json!({ "files": removed }),
    );
    removed
}

/// Second, silent pass (sentinel `DONE_LATE`). Returns how many files went.
pub(crate) fn sweep_late_at(dir: &Path) -> usize {
    if !dir.is_dir() || dir.join(DONE_LATE).exists() {
        return 0;
    }
    let removed = LEGACY_STATE_LATE
        .iter()
        .filter(|rel| std::fs::remove_file(dir.join(rel)).is_ok())
        .count();
    let _ = std::fs::write(dir.join(DONE_LATE), env!("CARGO_PKG_VERSION"));
    removed
}

/// The notice, once: `Some` only if a 1.x install was migrated and the
/// notice has not been shown yet. Consumes the pending marker.
pub(crate) fn take_notice_at(dir: &Path) -> Option<&'static str> {
    std::fs::remove_file(dir.join(NOTICE_PENDING)).ok()?;
    Some(NOTICE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let d =
            std::env::temp_dir().join(format!("tk-oss-mig-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_1x_install_is_cleaned_once_and_notified_once() {
        let d = tmp("1x");
        std::fs::create_dir_all(d.join("report")).unwrap();
        for f in LEGACY_STATE {
            std::fs::write(d.join(f), b"x").unwrap();
        }
        std::fs::write(d.join("gain.jsonl"), b"{}\n").unwrap();
        assert_eq!(migrate_at(&d), LEGACY_STATE.len());
        for f in LEGACY_STATE {
            assert!(!d.join(f).exists(), "{f} survived");
        }
        assert!(
            d.join("gain.jsonl").exists(),
            "the savings ledger is user data"
        );
        assert_eq!(take_notice_at(&d), Some(NOTICE));
        assert_eq!(take_notice_at(&d), None, "the notice is shown once");
        // Second run: sentinel present, nothing touched.
        std::fs::write(d.join("license.key"), b"x").unwrap();
        assert_eq!(migrate_at(&d), 0);
        assert!(d.join("license.key").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_late_sweep_runs_once_even_after_the_first_migration() {
        let d = tmp("late");
        std::fs::write(d.join(DONE), b"2.0.0").unwrap();
        for f in LEGACY_STATE_LATE {
            std::fs::write(d.join(f), b"x").unwrap();
        }
        assert_eq!(sweep_late_at(&d), LEGACY_STATE_LATE.len());
        assert_eq!(take_notice_at(&d), None, "no second notice");
        std::fs::write(d.join("last-report.zip"), b"x").unwrap();
        assert_eq!(sweep_late_at(&d), 0);
        assert!(d.join("last-report.zip").exists());
        assert!(!d.join("absent").exists() && sweep_late_at(&d.join("absent")) == 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_fresh_install_gets_no_notice_and_no_dir() {
        let d = tmp("fresh");
        assert_eq!(migrate_at(&d), 0);
        assert_eq!(take_notice_at(&d), None);
        let missing = d.join("absent");
        assert_eq!(migrate_at(&missing), 0);
        assert!(!missing.exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
