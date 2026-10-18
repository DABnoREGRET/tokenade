//! Writing a file without leaving a torn one behind.
//!
//! `fs::write` truncates first, so a reader arriving mid-write sees an empty
//! file — measured as 69 % empty reads on state files. Every write here goes
//! through a temp file and a rename.

use super::*;

/// Write `content` to `path`, but ONLY if the existing file's bytes
/// differ. Re-running `tokenade install` when nothing has changed
/// should be a true no-op — same mtime, same inode, no permission
/// re-prompts from the agent that's watching the config file.
///
/// Returns `Ok(true)` when the write happened, `Ok(false)` when it
/// was skipped because the file already had identical bytes. Inspired
/// by codegraph's installer atomicity model (`installer/targets/shared.ts`).
pub(super) fn idempotent_write(path: &Path, content: &str) -> std::io::Result<bool> {
    if let Ok(existing) = fs::read_to_string(path) {
        if existing == content {
            return Ok(false);
        }
    }
    fs::write(path, content)?;
    Ok(true)
}

/// #1002 — atomic write helper for user-owned JSON config files
/// (Claude Code's `settings.json`, MCP configs surfaced via this
/// path, etc.). Pre-fix the 7 install-time sites in this file used
/// plain `fs::write(path, body)`, which on kill / OOM mid-write
/// left the file truncated → wiped ALL of the user's settings
/// (not just tokenade-managed). Mirrors the same shape as
/// `mcp_wrap::write_locked` (#1001): tmp + fsync + atomic rename,
/// so the file is either prior valid JSON or new valid JSON,
/// never half-written. No flock here — the install path is
/// single-process by design (the user runs `tokenade install`
/// once at a time); the lock pattern in mcp_wrap exists to guard
/// against `tokenade-mcp` rescanning in parallel.
/// Text twin of `write_json_atomic` for rc files (.bashrc, CLAUDE.md…):
/// same symlink-preserving tmp+rename, same crash-safety. Audit: 5 rc
/// sites used plain fs::write — a Ctrl-C mid-write truncated .bashrc.
pub(crate) fn write_text_atomic(path: &std::path::Path, body: &str) -> std::io::Result<()> {
    write_json_atomic(path, body)
}

/// Refuse to write through a symlink that arrived with a `git clone`.
///
/// [`write_json_atomic`] deliberately follows symlinks so a dotfiles manager
/// (chezmoi, stow) keeps owning `~/.claude/settings.json`. That is right when
/// the link is the user's own choice, and wrong when it came from a checked-out
/// repository: a hostile repo can ship `.claude/settings.json` as a symlink to
/// `~/.ssh/authorized_keys`, and a project-scope install would write JSON
/// straight through it. Callers resolving a repo-scoped config path check this
/// first; user-scope paths keep following links.
/// First symlinked component of `path` at or below `root`, if any. Components
/// above `root` are the user's own filesystem and are not our business.
pub(crate) fn first_symlink_below(
    root: &std::path::Path,
    path: &std::path::Path,
) -> Option<std::path::PathBuf> {
    let rest = path.strip_prefix(root).ok()?;
    let mut cur = root.to_path_buf();
    for part in rest.iter() {
        cur.push(part);
        // `symlink_metadata` does NOT follow the final component — that is the
        // whole point; `metadata` would happily report the target's type.
        match std::fs::symlink_metadata(&cur) {
            Ok(md) if md.file_type().is_symlink() => return Some(cur),
            // Not existing yet is the normal case for a fresh install.
            _ => continue,
        }
    }
    None
}

/// The pristine bytes, when the text about to be written means the same thing.
pub(crate) fn write_json_atomic(path: &std::path::Path, body: &str) -> std::io::Result<()> {
    // Symlink-preserving (audit: rename onto a symlink REPLACES the link,
    // silently orphaning dotfiles-managed configs — chezmoi/stow users).
    let path = &std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    // If this write says exactly what the file said BEFORE this product first
    // touched it, put the user's own bytes back instead of our rendering of
    // them.
    //
    // Uninstall removes what install added and then re-serialises, so a config
    // that came in as `{"mcpServers":{}}` on one line went out as three
    // indented lines with a trailing newline. Nothing of the user's had
    // changed and their `git diff` said otherwise — measured 2026-08-19 on
    // Claude Code, Gemini CLI and Qwen, the three agents whose config this
    // helper owns.
    //
    // Only when the two parse to the SAME JSON. A formatting difference is the
    // one thing this may hide; a content difference must always be written.
    tokenade_core::paths::backup_once(path);
    let body = &match tokenade_core::paths::pristine_if_equivalent(path, body) {
        Some(original) => original,
        None => body.to_string(),
    };
    // Unique tmp name (pid): two concurrent SessionStart heals sharing a
    // FIXED tmp name could interleave writes and publish corrupt JSON via
    // rename (audit). fsync before rename — the doc used to claim it
    // without doing it.
    let tmp = path.with_extension(format!("tokenade.tmp.{}", std::process::id()));
    {
        use std::io::Write as _;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(body.as_bytes())?;
        f.sync_all()?;
    }
    // `rename` gives the destination the TMP file's permissions — whatever the
    // umask allows, typically 0644. This helper rewrites `~/.codex/config.toml`
    // and `~/.claude/settings.json`, files that can hold provider credentials
    // and that a user may deliberately have tightened. MEASURED 2026-08-07: a
    // config.toml at 0600 came back 0644 after `tokenade install` — an
    // owner-only file made world-readable as a side effect of a save.
    // `paths::write_atomic` in tokenade-core already carried this rule and the
    // reasoning verbatim; this writer and `mcp_wrap::write_locked` did not.
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
