//! **Write a file without leaving it half-written or open to everyone.**
//!
//! Atomic, and in 600. An interrupted write leaves a truncated cache
//! indistinguishable from an invalid cache; a world-readable write leaves a
//! secret on the disk of a shared machine.

use super::*;

/// Human size for user-facing lines. Deliberately skips KB — a number a person
/// is deciding on reads better in MB, and this is what `healthcheck` has always
/// printed, so the two surfaces must not disagree about the same directory.
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u == 0 {
        v /= 1_048_576.0;
        u = 1;
    }
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// Append one line to `path` in a single `write()`.
///
/// The reason this exists rather than `writeln!(f, "…")`: a format string is
/// streamed to the file as one `write()` per fragment, and tokenade's hooks run
/// as concurrent PROCESSES sharing these files. Their fragments interleave
/// mid-record, producing lines like
/// `{"ts":{"ts":17857689501785768950,"n":,"n":11}` — measured on a dogfood box
/// at 17 corrupt rows of 984 in `redact.jsonl`, and previously at 1221 of 31k in
/// `debug.log` before [`crate::debug_log`] was given the same treatment.
///
/// One `O_APPEND` write keeps each record contiguous. Note what this does NOT
/// promise: it is not a lock. Callers that also REWRITE the file (compaction,
/// rotation) need real exclusion — see `redact::record_redactions`.
///
/// Best-effort throughout: a ledger that cannot be written must never fail the
/// work that was being measured.
pub fn append_line(path: &std::path::Path, line: &str) {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut buf = String::with_capacity(line.len() + 1);
    buf.push_str(line);
    buf.push('\n');
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        // Readable so the handle can be LOCKED — see `lock_briefly`. Nothing
        // locks this one today; the rule is cheaper to keep than to audit.
        .read(true)
        .append(true)
        .open(path)
    {
        let _ = f.write_all(buf.as_bytes());
    }
}

/// Returns `(program, flag)` so the caller can build
/// `Command::new(program).arg(flag).arg(user_command)`:
///   * `("sh", "-c")` on Unix,
///   * `("cmd", "/C")` on Windows.
///
/// Tests on the CI Linux runner cover the `sh` path; the Windows path is
/// tested manually on a Windows VM (see `docs/CROSS-PLATFORM.md`).
pub fn shell_command() -> (&'static str, &'static str) {
    if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    }
}

pub fn claude_settings_has_tokenade_hooks() -> Option<bool> {
    let path = claude_config_dir()?.join("settings.json");
    let contents = std::fs::read_to_string(&path).ok()?;
    Some(claude_settings_has_tokenade_hooks_in(&contents))
}

/// Create-with-`0o600` write helper for atomic-write sites
/// across the workspace (bypass-counter, statusline cache, gain
/// rotate, gain prune). Plain `std::fs::write` respects the process
/// umask (typically 644); when combined with `fs::rename(&tmp,
/// &live)`, the live file gets DOWNGRADED from the explicit-list-
/// tightened 600 to 644 until the next `ensure_storage_perms_secure`
/// walk-pass retightens. Defense-in-depth fix matching
/// the cache and bypass-counter fixes: set the perm at
/// create-time, no window of looseness.
///
/// Caller still owns the atomic `rename` step — this helper only
/// handles the create-with-restricted-perms write of the tmp file.
/// Windows path falls back to plain `fs::write` (NTFS ACLs are a
/// separate concern, parent dir at 700 protects on Unix; same
/// posture as the stash-permission fix).
pub fn write_file_600(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(contents)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, contents)
    }
}

/// Atomic 0600 write: create a temp file at 0600, fill it, then rename it
/// over `path`.
///
/// The temp name carries the pid and a counter, and that is the whole point.
/// Six callers had each spelled this dance out for themselves and every one
/// chose a name that every process shares — `<path>.tmp`,
/// `gain.jsonl.rotating`, `gain.jsonl.pruning`, `<name>.merging`. The ledger's
/// own write lock is **process-local**, and its comment says so: cross-process
/// safety rests on the OS making small APPENDS atomic, which says nothing
/// about the rewrite that rotation and pruning perform. Two `tokenade` runs
/// crossing the rotation threshold in the same moment — which is exactly when
/// rotation happens, the busiest moment — write into one temp file and rename
/// a half-written one over the accounting the product reports.
///
/// Mode is carried over from the destination when it exists, so a file the
/// user tightened further is not widened back to 0600 by a save.
pub fn write_atomic_600(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let stem = path.file_name().and_then(|s| s.to_str()).unwrap_or("state");
    let tmp = path.with_file_name(format!(".{stem}.{}.{n}.tmp", std::process::id()));
    write_file_600(&tmp, contents)?;
    #[cfg(unix)]
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Test-only seam: same as [`claude_settings_has_tokenade_hooks`] but on a
/// raw string so fixtures don't need a real `$HOME`.
pub fn claude_settings_has_tokenade_hooks_in(contents: &str) -> bool {
    const VARIANTS: &[&str] = &[
        " hookwrap\"",
        " hookwrap ",
        " hookwrap'",
        " hookwrap-pretool",
        " hookwrap-precompact",
        " hookwrap-sessionstart",
        " hookwrap-userprompt",
        " index-touch",
    ];
    contents.contains("\"_tag\": \"tokenade")
        || contents.contains("\"_tag\":\"tokenade")
        || VARIANTS.iter().any(|v| contents.contains(v))
}

/// Returns `Some(true)` when `~/.claude/settings.json` contains the
/// PreToolUse `hookwrap-pretool` hook. Legacy name: this hook used to be
/// gated behind `--with-lsp-enforcement-hook`, but since the 2026-06 pivot
/// it is installed by DEFAULT (matcher `Bash|Read|WebFetch|Task|Agent` —
/// Grep/Glob "enforcement" was dropped), and that flag is now a deprecated
/// no-op. So this returns true for essentially every default install. Kept
/// only as a `discover` --json signal; the dead "install LSP enforcement"
/// suggestion it used to gate has been removed. Dual-marker robustness as
/// [`claude_settings_has_tokenade_hooks`].
pub fn claude_settings_has_lsp_enforcement_hook() -> Option<bool> {
    let path = claude_config_dir()?.join("settings.json");
    let contents = std::fs::read_to_string(&path).ok()?;
    Some(claude_settings_has_lsp_enforcement_hook_in(&contents))
}

/// Test-only seam for [`claude_settings_has_lsp_enforcement_hook`].
pub fn claude_settings_has_lsp_enforcement_hook_in(contents: &str) -> bool {
    // Detects the default PreToolUse hook by its tag / command. (Since the
    // 2026-06 pivot `hookwrap-pretool` is installed by default, not opt-in.)
    contents.contains("\"_tag\": \"tokenade-pretool")
        || contents.contains("\"_tag\":\"tokenade-pretool")
        || contents.contains(" hookwrap-pretool")
}

/// Returns `Some(true)` when `~/.claude/CLAUDE.md` contains the
/// tokenade scaffold block (the one `tokenade install --with-claude-md`
/// writes — fenced by `<!-- tokenade:start --> … <!-- tokenade:end -->`).
/// Used by `tokenade discover` to gate the "install --with-
/// claude-md" next-step suggestion.
pub fn global_claude_md_has_tokenade_block() -> Option<bool> {
    let path = claude_config_dir()?.join("CLAUDE.md");
    let contents = std::fs::read_to_string(&path).ok()?;
    Some(global_claude_md_has_tokenade_block_in(&contents))
}

/// Test-only seam for [`global_claude_md_has_tokenade_block`].
pub fn global_claude_md_has_tokenade_block_in(contents: &str) -> bool {
    // Matches the fence in cli/main.rs::GLOBAL_CLAUDE_MD_FENCE_START.
    // Kept as a literal here to avoid a circular dep into the cli crate.
    contents.contains("<!-- tokenade:start -->")
}
