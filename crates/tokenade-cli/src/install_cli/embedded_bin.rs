//! The PATH shim: the literal path baked into it, and the guards that keep
//! a build directory or a temp upgrade path out of a user's shell profile.

use super::*;

/// Absolute path of the running tokenade binary, escaped for embedding in a
/// double-quoted JS / Python string literal (backslashes + quotes — Windows
/// paths are full of `\`). Falls back to the bare name (PATH-resolved at
/// runtime) if the path is unknown. The shims read this so they don't depend
/// on an OS-specific guess like `~/.local/bin/tokenade`.
/// Escape a string for embedding inside a double-quoted JS / Python literal.
pub(crate) fn escape_for_embedded_literal(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

pub(crate) fn tokenade_bin_literal() -> String {
    // Route through `stable_hook_exe` so shims (statusline, plugins) never
    // bake in a transient auto-updater temp path (`.tokenade.new.<pid>`) or
    // a build-dir artifact — same failure class the hooks hit.
    escape_for_embedded_literal(&stable_hook_exe())
}

/// True when `p`'s file name is an in-flight auto-updater temp binary —
/// `.<name>.new.<pid>` (see `updater::extract_and_swap_locked`, which
/// writes the freshly-downloaded binary there and probes it with
/// `<tmp> --version` BEFORE renaming it onto the live path). A hook,
/// statusline or shim command wired to this path dies the instant the
/// atomic swap renames the temp onto the live binary and unlinks the
/// temp name — the "sh: exec: …/.tokenade.new.9087: not found" incident.
pub(crate) fn is_temp_new_exe_path(p: &str) -> bool {
    let name = match Path::new(p).file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return false,
    };
    if !name.starts_with('.') {
        return false;
    }
    match name.rsplit_once(".new.") {
        // pid tail must be all-digits so we don't misfire on a user file
        // that merely happens to contain ".new.".
        Some((head, pid)) => {
            !head.is_empty() && !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit())
        }
        None => false,
    }
}

/// Given an updater temp-binary path (`…/.tokenade.new.9087`), return the
/// stable install destination it will be renamed onto (`…/tokenade`), i.e.
/// strip the leading `.` and the `.new.<pid>` suffix, keeping the same
/// parent dir. `.tokenade.exe.new.9087` → `…/tokenade.exe`. Returns `None`
/// when `p` isn't a temp path.
pub(crate) fn canonicalize_temp_new_exe(p: &str) -> Option<String> {
    if !is_temp_new_exe_path(p) {
        return None;
    }
    let path = Path::new(p);
    let name = path.file_name().and_then(|n| n.to_str())?;
    // `.tokenade.new.9087` → `tokenade.new.9087` → `tokenade`.
    let stem = name.strip_prefix('.')?;
    let final_name = stem.rsplit_once(".new.").map(|(a, _)| a)?;
    let dest = match path.parent() {
        Some(par) if !par.as_os_str().is_empty() => par.join(final_name),
        _ => PathBuf::from(final_name),
    };
    Some(dest.display().to_string())
}

/// Replace the `@@TOKENADE_BIN@@` placeholder in a shim asset with the absolute
/// binary path (cross-OS). The env var `TOKENADE_BIN` still wins at runtime; if
/// injection is ever skipped the shims fall back to the bare `tokenade` name.
pub(crate) fn inject_tokenade_bin(content: &str) -> String {
    content.replace("@@TOKENADE_BIN@@", &tokenade_bin_literal())
}

pub(super) fn which_installed_tokenade() -> Option<String> {
    tokenade_core::platform::which_installed_tokenade()
}

/// The tokenade path to bake into hook / statusline commands. A binary run
/// from a build dir (`dist/`, `target/`) must NOT wire the user's hooks to
/// its own transient path: when that artifact is rebuilt or wiped, every
/// hook AND the statusline die (live incident — the dry-run dist binary
/// self-healed the user's config to its own path). If we're a build-dir
/// binary, prefer an installed `tokenade` resolved on PATH; only fall back
/// to `current_exe` when none exists (a pure dev machine).
pub(crate) fn stable_hook_exe() -> String {
    let cur = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "tokenade".into());
    // Auto-updater temp binary (`.tokenade.new.<pid>`): the process is
    // running under the not-yet-renamed download during the pre-swap
    // `--version` probe. NEVER wire hooks to it — the swap unlinks it
    // moments later. Resolve to the stable install destination it will
    // be renamed onto (which still holds the live binary at probe time),
    // falling back to an installed `tokenade` on PATH.
    if is_temp_new_exe_path(&cur) {
        if let Some(dest) = canonicalize_temp_new_exe(&cur) {
            if Path::new(&dest).exists() {
                return dest;
            }
        }
        if let Some(installed) = which_installed_tokenade() {
            return installed;
        }
    }
    // The rest of the rule — build-dir, dead-path and transient-copy guards —
    // lives in `tokenade_core::platform::stable_exe_from`, because `mcp_wrap`
    // needs the same answer from another crate and had reimplemented the naive
    // `current_exe()` version instead. Only the
    // auto-updater's `.tokenade.new.<pid>` case stays here: it is specific to
    // this binary's own swap dance.
    tokenade_core::platform::stable_exe_from(&cur)
}
