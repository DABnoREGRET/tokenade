//! **Where things are, on the three systems.**
//!
//! A path is not a string: asking a string whether a path goes through
//! a directory gives a WRONG answer on Windows, and six sites did it
//! — only one turned red, the other five silently answered no. Hence
//! `path_has_segment`, and the rule: ask the file system, not
//! the text.

use super::*;

/// Two-letter system UI language, multi-OS.
/// Unix: LC_ALL > LC_MESSAGES > LANG > LANGUAGE (POSIX precedence).
/// Windows: those envs are usually absent — shell out ONCE to
/// PowerShell's Get-Culture and cache the result in ~/.tokenade/locale
/// (the statusline ticks continuously; a per-tick process spawn would
/// be wasteful and visible).
pub fn system_lang() -> String {
    for var in ["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"] {
        if let Ok(v) = std::env::var(var) {
            let v = v.trim().to_lowercase();
            if v.len() >= 2 && v != "c" && !v.starts_with("c.") && !v.starts_with("posix") {
                return v[..2].to_string();
            }
        }
    }
    if cfg!(windows) {
        let cache = home_dir().map(|h| h.join(".tokenade").join("locale"));
        if let Some(c) = &cache {
            // 7-day TTL: a user changing the system language shouldn't be
            // stuck on the old locale forever (audit C).
            let fresh = std::fs::metadata(c)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|m| std::time::SystemTime::now().duration_since(m).ok())
                .map(|age| age.as_secs() < 7 * 86_400)
                .unwrap_or(false);
            if fresh {
                if let Ok(v) = std::fs::read_to_string(c) {
                    let v = v.trim().to_lowercase();
                    if v.len() == 2 {
                        return v;
                    }
                }
            }
        }
        let mut ps = crate::process_util::internal_command("powershell");
        ps.args([
            "-NoProfile",
            "-Command",
            "(Get-Culture).TwoLetterISOLanguageName",
        ]);
        // `output()` captures both streams, so this needs no console — and it
        // is asked for the UI language, which every ticker render can reach.
        crate::process_util::configure_no_window(&mut ps);
        if let Ok(out) = ps.output() {
            let v = String::from_utf8_lossy(&out.stdout).trim().to_lowercase();
            if v.len() == 2 {
                if let Some(c) = &cache {
                    ensure_parent_dir(c);
                    let _ = std::fs::write(c, &v);
                }
                return v;
            }
        }
    }
    "en".to_string()
}

/// User home directory.
///
/// Unix: `$HOME` (then `$USERPROFILE` as a defensive fallback).
///
/// Windows: `%USERPROFILE%` WINS, `$HOME` is only a fallback. This ordering
/// is deliberate and load-bearing: the native Windows home is `%USERPROFILE%`,
/// and that's the dir Claude Code (and every other agent) uses for `~/.claude`
/// etc. A Git-Bash / MSYS2 shell ALSO exports `$HOME`, but it can point
/// ELSEWHERE (a unix-style MSYS home, or a custom override). If tokenade
/// preferred `$HOME`, its install (run from one shell) and its hooks (invoked
/// by the agent in a different env) could resolve DIFFERENT `~/.tokenade` /
/// `~/.claude` paths — so the hooks land in a `settings.json` the agent never
/// reads, nothing compacts, and the dashboard + gain ledger look empty.
/// Diagnosed from a Windows beta-tester report (2026-06-23): install + report
/// near-empty, dashboard partly empty, zero tokenade activity in the session.
///
/// Returns `None` when neither var is present — callers treat that as "unknown
/// home" rather than panic.
/// **Create the PARENT directory of a file about to be written.**
///
/// Eighteen places in this repo wrote
/// `let _ = std::fs::create_dir_all(p.parent().unwrap());` — eighteen copies
/// of the same line, and eighteen panic paths for a case nobody
/// ever met (`parent()` only returns `None` on the root or on an
/// empty path). The `let _ =` already said that a creation failure is tolerated;
/// the `unwrap` right next to it said the opposite in the same breath.
///
/// Returns `false` when there is no parent or the creation failed — a
/// caller that does not care ignores the value, as the eighteen did.
pub fn ensure_parent_dir(path: &std::path::Path) -> bool {
    match path.parent() {
        Some(dir) => std::fs::create_dir_all(dir).is_ok(),
        None => false,
    }
}

pub fn home_dir() -> Option<PathBuf> {
    pick_home(
        std::env::var_os("HOME"),
        std::env::var_os("USERPROFILE"),
        cfg!(windows),
    )
}

/// Where Claude Code keeps its configuration — `CLAUDE_CONFIG_DIR` when the
/// user has moved it, `$HOME/.claude` otherwise.
///
/// Read out of Claude Code's own binary rather than assumed (2.1.251):
///
/// ```text
/// function s(){return process.env.CLAUDE_CONFIG_DIR}
/// var He = si(() => (s() ?? i(g(), ".claude")).normalize("NFC"), s);
/// ```
///
/// `He()` is the config directory and everything hangs off it — settings,
/// `projects/` and the transcripts inside it, the plugin dirs. Tokenade
/// honoured the variable in none of its hundred-odd `.claude` joins, and the
/// consequence was not a degradation: measured 2026-08-31 on a clean scratch
/// home with the config relocated, `tokenade install --only claude-code`
/// answered "no AI coding agent detected on this machine" and wrote nothing,
/// because `presence_markers` looks for `~/.claude` and the user does not have
/// one. Same machine, same binary, config in the default place: install
/// succeeds and the settings file gains 22 tokenade references.
pub fn claude_config_dir_for(home: &std::path::Path) -> PathBuf {
    match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        // A set-but-EMPTY value is not a directory, for the same reason an
        // empty `HOME` is not a home: it joins to a relative path and every
        // read and write lands under the current directory. See `pick_home`.
        _ => home.join(".claude"),
    }
}

/// The same rule for the machine's real home.
pub fn claude_config_dir() -> Option<PathBuf> {
    Some(claude_config_dir_for(&home_dir()?))
}

/// `.claude.json` moves with the directory, and it is NOT inside it.
///
/// From the same binary: ``let t=`.claude${_q()}.json`; return
/// c(process.env.CLAUDE_CONFIG_DIR || z(), t)`` — the file is named
/// `.claude.json` and placed in `CLAUDE_CONFIG_DIR` when that is set, and in
/// `$HOME` when it is not. So it is a sibling of the config dir in the default
/// layout and a child of it in the relocated one, which is not a rule anyone
/// would guess from the default.
pub fn claude_dot_json_for(home: &std::path::Path) -> PathBuf {
    match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(v) if !v.is_empty() => PathBuf::from(v).join(".claude.json"),
        _ => home.join(".claude.json"),
    }
}

/// Which files a bare `tokenade` can reach through this PATH, in PATH order.
///
/// Two tokenade binaries on one PATH is not hypothetical: measured 2026-09-01
/// on the development machine, `~/.local/bin/tokenade` (1.1.7) and
/// `~/.cargo/bin/tokenade` (0.5.1) were both reachable, forty megabytes apart
/// and fifteen months apart in behaviour. The one that runs is decided by PATH
/// order alone, which a shell rc, an editor, a systemd unit or a Makefile can
/// each reorder without telling anybody, and nothing in the product said the
/// second copy existed. `healthcheck` reported the running binary's own path
/// and version and stopped there — it could only ever describe the copy that
/// had already won.
///
/// Pure in its input so both platforms' rules are testable on either host: the
/// PATH string and the candidate file names come from the caller. Entries are
/// de-duplicated by IDENTITY, not by spelling — a symlink farm
/// (`~/.local/bin/tokenade` -> `/opt/tokenade/bin/tokenade`) is one install
/// reached twice, not two installs, and reporting it as a conflict would send
/// the user deleting half of their own working setup.
pub fn tokenade_on_path_in(path_var: &std::ffi::OsStr, names: &[&str]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    for dir in std::env::split_paths(path_var) {
        // An empty PATH entry means "the current directory" to a shell. We do
        // not follow it: healthcheck must not report a `./tokenade` that only
        // exists because of where it was run from.
        if dir.as_os_str().is_empty() {
            continue;
        }
        for name in names {
            let cand = dir.join(name);
            if !is_executable_file(&cand) {
                continue;
            }
            let identity = cand.canonicalize().unwrap_or_else(|_| cand.clone());
            if seen.contains(&identity) {
                continue;
            }
            seen.push(identity);
            out.push(cand);
        }
    }
    out
}

/// The file names a bare `tokenade` can resolve to on this platform.
///
/// Windows resolves a bare name through PATHEXT, and npm's global installs
/// ship a `.cmd` shim beside the `.exe` — so the name that wins there is
/// frequently not the one with the executable bit on Unix.
pub fn tokenade_exe_names() -> &'static [&'static str] {
    tokenade_exe_names_for(cfg!(windows))
}

/// The rule, parameterised, so BOTH platforms' name lists are checkable from
/// either host — the same reason [`pick_home`] takes its platform as an
/// argument. A `cfg!` in the body would leave the other half of the answer
/// untested everywhere it matters.
pub fn tokenade_exe_names_for(windows: bool) -> &'static [&'static str] {
    if windows {
        &["tokenade.exe", "tokenade.cmd", "tokenade.bat", "tokenade"]
    } else {
        &["tokenade"]
    }
}

/// Everything the shell would consider, using this process's real PATH.
pub fn tokenade_on_path() -> Vec<PathBuf> {
    match std::env::var_os("PATH") {
        Some(p) => tokenade_on_path_in(&p, tokenade_exe_names()),
        None => Vec::new(),
    }
}

/// Same file, whatever the spelling — symlinks resolved, so a shim farm reads
/// as one install.
/// Does the path go through a directory named `segment`?
///
/// The question seems answered by `p.to_string_lossy().contains("/cmd/")`, and
/// it is wrong on Windows, where the separator is `\`. The defect does not
/// necessarily show: of the six sites this repo carried, only one turned
/// a test RED (`source_guard`, which stopped the 1.1.11 release after three
/// platforms were built) — the others simply answered NO
/// silently, and the bench measured a release binary while declaring it debug.
///
/// Compares only whole SEGMENTS, so `deps` does not match `mydeps`,
/// where the substring search needed slashes around it
/// for exactly that reason.
pub fn path_has_segment(p: &std::path::Path, segment: &str) -> bool {
    p.components()
        .any(|c| c.as_os_str().to_string_lossy() == segment)
}

pub fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    let ca = a.canonicalize().unwrap_or_else(|_| a.to_path_buf());
    let cb = b.canonicalize().unwrap_or_else(|_| b.to_path_buf());
    ca == cb
}

#[cfg(unix)]
pub(super) fn is_executable_file(p: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
pub(super) fn is_executable_file(p: &std::path::Path) -> bool {
    // Windows has no executable bit; the extension list in
    // `tokenade_exe_names` is what decides, and the caller has already applied
    // it. Anything that exists as a file under one of those names is a
    // candidate.
    std::fs::metadata(p).map(|m| m.is_file()).unwrap_or(false)
}

/// Tokenade's own state directory (`~/.tokenade`): update cache, opt-out
/// sentinel, migration stamps. `TOKENADE_CONFIG_DIR` overrides it (benches,
/// tests); a boolean-typo or empty value is ignored so the
/// state never lands in a RELATIVE dir resolved against the caller's CWD.
/// Windows-safe: `home_dir()` resolves `%USERPROFILE%` when `$HOME` is unset.
pub fn tokenade_state_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("TOKENADE_CONFIG_DIR") {
        if crate::env_compat::is_usable_value(&dir) {
            return PathBuf::from(dir);
        }
    }
    if let Some(home) = home_dir() {
        return home.join(".tokenade");
    }
    PathBuf::from(".tokenade")
}
