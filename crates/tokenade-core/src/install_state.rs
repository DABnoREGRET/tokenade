//! Install sentinel + hook self-heal throttle stamps under `~/.tokenade`.

use std::path::{Path, PathBuf};

/// Path of the install sentinel written at `tokenade install` time.
pub fn install_sentinel_path(home: &Path) -> PathBuf {
    home.join(".tokenade").join("installed-at")
}

/// Write the install sentinel (unix secs as text) if absent. Idempotent:
/// the FIRST install date is the one that matters, so we never overwrite.
/// Best-effort — never errors out the caller.
pub fn ensure_install_sentinel(home: &Path) {
    let p = install_sentinel_path(home);
    if p.exists() {
        return;
    }
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&p, crate::platform::now_unix_secs().to_string());
}

/// Throttle stamp for the hook self-heal pass (`ensure_hooks`).
pub fn hooks_checked_path(home: &Path) -> PathBuf {
    home.join(".tokenade").join("hooks-checked-at")
}

/// True if tokenade was ever installed on this machine (the install
/// sentinel exists). The self-heal pass keys off this so it re-asserts
/// hooks for a machine that opted in, but NEVER resurrects hooks on one
/// that never installed — or that uninstalled, since `clear_install_state`
/// removes the sentinel.
pub fn installed_ever(home: &Path) -> bool {
    install_sentinel_path(home).exists()
}

/// True if the self-heal check is due: throttle stamp missing or older
/// than `ttl_secs`. One stat — cheap enough for the hot per-command path
/// (`tokenade wrap`), which only does the real work once per TTL window.
pub fn hooks_check_due(home: &Path, ttl_secs: u64) -> bool {
    let m = crate::platform::mtime_epoch(&hooks_checked_path(home));
    m == 0 || crate::platform::now_unix_secs().saturating_sub(m) >= ttl_secs
}

/// Stamp the self-heal throttle to now (best-effort).
pub fn mark_hooks_checked(home: &Path) {
    let p = hooks_checked_path(home);
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&p, crate::platform::now_unix_secs().to_string());
}

/// Clear the install sentinel + self-heal stamp so a later session does
/// NOT resurrect the hooks. Called by `tokenade uninstall`.
pub fn clear_install_state(home: &Path) {
    let _ = std::fs::remove_file(install_sentinel_path(home));
    let _ = std::fs::remove_file(hooks_checked_path(home));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_state_and_throttle_helpers() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let home =
            std::env::temp_dir().join(format!("tk-install-state-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        // Fresh machine: never installed, check is due (no stamp).
        assert!(!installed_ever(&home));
        assert!(hooks_check_due(&home, 3600));
        ensure_install_sentinel(&home);
        assert!(installed_ever(&home));
        // Stamp → not due within TTL, due once TTL=0.
        mark_hooks_checked(&home);
        assert!(hooks_checked_path(&home).exists());
        assert!(!hooks_check_due(&home, 3600));
        assert!(hooks_check_due(&home, 0));
        // Uninstall clears both → back to "never installed".
        clear_install_state(&home);
        assert!(!installed_ever(&home));
        assert!(!hooks_checked_path(&home).exists());
        assert!(hooks_check_due(&home, 3600));
        let _ = std::fs::remove_dir_all(&home);
    }
}
