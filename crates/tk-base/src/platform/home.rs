//! **Which "home", and why the question is a trap.**
//!
//! `HOME` is process-global: mutating it to isolate a test shakes
//! the whole suite. Windows moved its directory once, and the migration
//! must be done EXACTLY once. The XDG roots, the cache, and the
//! ambient override are decided here so that nobody else has to
//! guess.

use super::*;

/// Pure home-resolution policy, split out so BOTH platforms' ordering is
/// unit-testable on any host (the real `home_dir` only exercises one `cfg`).
pub(super) fn pick_home(
    home: Option<std::ffi::OsString>,
    userprofile: Option<std::ffi::OsString>,
    windows: bool,
) -> Option<PathBuf> {
    let (first, second) = if windows {
        (userprofile, home) // Windows: USERPROFILE authoritative (matches the agent).
    } else {
        (home, userprofile) // Unix: HOME authoritative.
    };
    // A set-but-EMPTY var is not a home: `PathBuf::from("")` joins to a RELATIVE
    // path, so `home.join(".tokenade")` becomes `./.tokenade` and
    // `tokenade uninstall` recursively deletes a directory named
    // `tokenade`/`.tokenade` in the CWD — someone's project, not our data
    // (reproduced: real files destroyed). An empty `HOME` reaches us from
    // `env -i`, systemd units, sudo, CI, and `export HOME="$UNSET_VAR"` in an
    // rc. Absent > wrong: every caller already handles None by bailing out.
    //
    // Only emptiness is filtered here, deliberately: a Windows `$HOME` is
    // legitimately MSYS-shaped (`/c/Users/u`) and a drive path is legitimate on
    // a Unix host under test, so "is it absolute?" cannot be judged from this
    // pure function. That check belongs to the destructive callers, which run
    // on the real platform — see `remove_tree_no_follow`.
    [first, second]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .next()
}

/// One-shot migration paired with the Windows `home_dir` switch to
/// USERPROFILE-first. A Windows user whose Git-Bash `$HOME` differed from
/// `%USERPROFILE%` had their `~/.tokenade` (ledger, install sentinel, AND
/// LICENSE) stranded under the old `$HOME`. After the switch the new
/// USERPROFILE location is empty → savings history + activation would appear
/// lost and the install date would reset. So copy the small, non-rebuildable
/// state across. Windows-only, best-effort, idempotent.
#[cfg(windows)]
pub fn migrate_windows_home_once() {
    let (Some(h), Some(up)) = (std::env::var_os("HOME"), std::env::var_os("USERPROFILE")) else {
        return;
    };
    if h == up {
        return;
    }
    migrate_tokenade_dir(&PathBuf::from(h), &PathBuf::from(up));
}

/// No-op off Windows (there is no HOME/USERPROFILE split to repair).
#[cfg(not(windows))]
pub fn migrate_windows_home_once() {}

/// Host-agnostic core of [`migrate_windows_home_once`] — split out so the
/// copy + guard logic is unit-testable on any platform. Copies the small,
/// non-rebuildable `~/.tokenade` state from `old_home` to `new_home` ONLY when
/// the new location has no ledger yet (never clobbers existing data). The big
/// rebuildable caches (`*.db`, `debug.log*`) are deliberately NOT migrated.
/// Idempotent via a `.home-migrated` marker. Returns files copied.
pub fn migrate_tokenade_dir(old_home: &std::path::Path, new_home: &std::path::Path) -> usize {
    use std::fs;
    if old_home == new_home {
        return 0;
    }
    let old = old_home.join(".tokenade");
    let new = new_home.join(".tokenade");
    if old == new {
        return 0;
    }
    let marker = new.join(".home-migrated");
    // Already migrated, or the new location already has a real ledger →
    // don't run / don't clobber.
    if marker.exists() || new.join("gain.jsonl").exists() {
        return 0;
    }
    // Nothing worth migrating (no ledger / license / install date at old).
    let worth = ["gain.jsonl", "license.key", "installed-at"]
        .iter()
        .any(|f| old.join(f).is_file());
    if !worth {
        return 0;
    }
    let _ = fs::create_dir_all(&new);
    const SMALL: &[&str] = &[
        "license.key",
        "installed-at",
        "gain.jsonl",
        "gain.rollup.jsonl",
        "bypass_lost.jsonl",
        "coefficients.json",
        "mistakes.jsonl",
        "auto-update-off",
    ];
    let want = |n: &str| SMALL.contains(&n) || n.starts_with("gain.jsonl.");
    let mut copied = 0;
    if let Ok(rd) = fs::read_dir(&old) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.path().is_file()
                && want(&name)
                && !new.join(&name).exists()
                && fs::copy(e.path(), new.join(&name)).is_ok()
            {
                copied += 1;
            }
        }
    }
    let _ = fs::write(&marker, b"1");
    copied
}

/// User cache directory.
///
/// Resolution order:
///   1. `$XDG_CACHE_HOME` (any platform — explicit override wins).
///   2. `$LOCALAPPDATA` (Windows native default).
///   3. `$HOME/.cache` (Unix XDG default).
///   4. `$USERPROFILE/AppData/Local` (Windows fallback if LOCALAPPDATA is
///      missing — unlikely but defensive).
///   5. `None` if every option fails.
/// Is `home` the home directory of the CURRENT process?
///
/// The gate for every ambient path override. `XDG_CONFIG_HOME`,
/// `XDG_DATA_HOME`, `GOOSE_PATH_ROOT` and friends describe where the CURRENT
/// user's data lives — they say nothing about a home a caller named
/// explicitly. When a test, a `--home` install or a fixture passes its own
/// home, that argument must win, or the caller cannot isolate the call at all.
pub fn is_process_home(home: &std::path::Path) -> bool {
    home_dir().is_some_and(|h| h == home)
}

/// `$XDG_CONFIG_HOME` (else `<home>/.config`), honouring the variable only
/// when `home` is the process home. See [`is_process_home`].
pub fn xdg_config_root(home: &std::path::Path) -> PathBuf {
    ambient_root(home, "XDG_CONFIG_HOME", ".config")
}

/// `$XDG_DATA_HOME` (else `<home>/.local/share`), same gate.
pub fn xdg_data_root(home: &std::path::Path) -> PathBuf {
    ambient_root(home, "XDG_DATA_HOME", ".local/share")
}

/// An agent-specific root override (`GOOSE_PATH_ROOT`, `CRUSH_GLOBAL_DATA`,
/// `ZS_DATA_DIR`, `ANTIGRAVITY_DATA_DIR`…), under the same gate. `None` when
/// the variable is unset, blank, or when `home` is not the process home.
pub fn ambient_override(home: &std::path::Path, var: &str) -> Option<PathBuf> {
    if !is_process_home(home) {
        return None;
    }
    let v = std::env::var_os(var)?;
    let p = PathBuf::from(v);
    if p.as_os_str().is_empty() {
        return None;
    }
    Some(p)
}

pub(super) fn ambient_root(home: &std::path::Path, var: &str, fallback: &str) -> PathBuf {
    ambient_override(home, var).unwrap_or_else(|| home.join(fallback))
}

pub fn cache_dir() -> Option<PathBuf> {
    // Set-but-EMPTY is treated as unset at every step: `PathBuf::from("")` is
    // relative, so an empty `XDG_CACHE_HOME` yields `tokenade` and an empty
    // `HOME` yields `.cache` — both relative to the CWD, which `tokenade
    // uninstall` would then recursively delete (reproduced: real user files
    // destroyed). Same policy as `pick_home`; see its note.
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(p) = var("XDG_CACHE_HOME") {
        return Some(p);
    }
    if let Some(p) = var("LOCALAPPDATA") {
        return Some(p);
    }
    if let Some(p) = var("HOME") {
        return Some(p.join(".cache"));
    }
    if let Some(p) = var("USERPROFILE") {
        return Some(p.join("AppData").join("Local"));
    }
    None
}
