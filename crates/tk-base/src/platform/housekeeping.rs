//! **Housekeeping: our own traces, and their permissions.**
//!
//! The backups we write delete themselves, the stores that have
//! swollen get compacted, and everything carrying secrets closes back to 600.
//! The criterion is constant: this cleanup touches ONLY what tokenade wrote —
//! a third party's backup that looks like ours is not ours.

use super::*;

/// How long tokenade keeps its OWN transient backups before sweeping them.
///
/// Generous on purpose: these are safety copies taken around a migration, a
/// ledger purge or a log rotation, and the window where one is still wanted is
/// days, not weeks. Measured 2026-08-03 on a real install: ~38 MB of them,
/// aged 13 to 63 days, with nothing anywhere that removes them.
pub const OWN_BACKUP_TTL_SECS: u64 = 30 * 86_400;

/// Prefixes/shapes that are unambiguously tokenade's own transient backups.
///
/// Deliberately NOT included: `last-report.zip`. That one is a support bundle
/// the user asked for and may be about to send to us — regenerable, but theirs,
/// and deleting something a person created on purpose is a different act from
/// collecting our own litter.
pub(super) fn is_own_transient_backup(name: &str) -> bool {
    name.starts_with("debug.log.bak-")
        || name.starts_with("gain.jsonl.bak-")
        || name.starts_with("gain.jsonl.prepurge-")
        || name.starts_with("gain.jsonl.pre")
        || name.starts_with("backup-recalc-")
}

/// Remove tokenade's own transient backups older than [`OWN_BACKUP_TTL_SECS`]
/// from `dir`. Returns (entries removed, bytes freed). Path-injected so it is
/// testable without touching `HOME`.
///
/// Only the shapes in [`is_own_transient_backup`] are eligible: never a live
/// file, never anything the user made. Best-effort — a failure to remove is
/// skipped, not reported, because this runs inside a hook.
pub fn sweep_own_backups_in(dir: &std::path::Path) -> (usize, u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    let (mut n, mut freed) = (0usize, 0u64);
    for e in entries.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy().to_string();
        if !is_own_transient_backup(&name) {
            continue;
        }
        // `symlink_metadata` (lstat), never `metadata` (stat): this function
        // DELETES, and a name matching our backup shape could be a symlink the
        // user planted — or one left by something else entirely. Following it
        // would take `remove_dir_all` outside `~/.tokenade`. We remove the link
        // and stop there; the target is not ours.
        let Ok(meta) = std::fs::symlink_metadata(e.path()) else {
            continue;
        };
        let Some(age) = meta.modified().ok().and_then(|t| t.elapsed().ok()) else {
            continue;
        };
        if age.as_secs() <= OWN_BACKUP_TTL_SECS {
            continue;
        }
        let path = e.path();
        if meta.file_type().is_symlink() {
            if std::fs::remove_file(&path).is_ok() {
                n += 1;
            }
        } else if meta.is_dir() {
            let bytes = dir_bytes(&path);
            if std::fs::remove_dir_all(&path).is_ok() {
                n += 1;
                freed += bytes;
            }
        } else if std::fs::remove_file(&path).is_ok() {
            n += 1;
            freed += meta.len();
        }
    }
    (n, freed)
}

/// Recursive byte total, best-effort (0 on any error). Uses `symlink_metadata`
/// so a symlinked subtree is counted as the link it is, not as the bytes it
/// points at — the same reason the sweep above never follows one.
///
/// Public because two user-facing surfaces must agree about the same directory:
/// `healthcheck`'s "tokenade is holding X on disk" and what `uninstall` says it
/// is about to delete. `healthcheck` used to carry its own copy, which followed
/// symlinks and so could report bytes it did not own.
pub fn dir_bytes(dir: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match std::fs::symlink_metadata(e.path()) {
            Ok(m) if m.file_type().is_symlink() => 0,
            Ok(m) if m.is_dir() => dir_bytes(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

/// Write `bytes` to `path` with 0600 from the FIRST byte, creating or
/// truncating like [`std::fs::write`].
///
/// `std::fs::write` creates at 0666 masked by the process umask, which is 0644
/// on a normal install — so every stash entry, which holds the full raw text of
/// a tool result, was born world-readable and only became 0600 if a LATER CLI
/// start happened to run the repair walk in [`tighten_stash`]. Measured on the
/// development machine 2026-08-27: **817 of 828 stash files at 0644**, the 11
/// tight ones being those that existed on the single day the walk ran.
///
/// The directory at 0700 is still the protection that holds, and it held here.
/// This is the second lock: a mode that survives the directory being loosened
/// by a restore, an rsync, or a tar unpacked with its own permissions. Every
/// other file tokenade owns is already held to 0600 by
/// [`ensure_storage_perms_secure`]; the stash was the one exception, and it is
/// the one holding the most.
pub fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
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
        // `mode` applies at CREATION only: an existing file keeps whatever it
        // had, so say it again for the entry a previous version wrote loose.
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = f.metadata()?.permissions();
            if perms.mode() & 0o177 != 0 {
                perms.set_mode(0o600);
                let _ = f.set_permissions(perms);
            }
        }
        f.write_all(bytes)
    }
    #[cfg(not(unix))]
    {
        // Windows: NTFS ACLs are a separate concern, as everywhere else here.
        std::fs::write(path, bytes)
    }
}

/// Rewrites a project-root-relative path so it opens from `cwd`.
///
/// Every store tokenade keeps is keyed on the PROJECT and holds root-relative
/// paths, while every command runs in a DIRECTORY and
/// hands its output to something that will open those paths from there.
/// This is the one place that bridge is spelled — the code index and
/// the semantic index both use it, and a second copy would be two rules that
/// can drift apart while both look right.
pub struct PathRebase {
    /// `cwd` relative to the root, with a trailing slash.
    prefix: String,
    /// One `../` per component of `prefix`.
    up: String,
}

impl PathRebase {
    /// `None` when `cwd` IS the root — the common case, and nothing to do —
    /// or when it lies outside the project, where no relative base exists.
    pub fn new(root: &std::path::Path, cwd: &std::path::Path) -> Option<Self> {
        let rel = cwd.strip_prefix(root).ok()?;
        let prefix = rel.to_string_lossy().replace('\\', "/");
        let depth = prefix.split('/').filter(|s| !s.is_empty()).count();
        if depth == 0 {
            return None;
        }
        Some(Self {
            prefix: format!("{prefix}/"),
            up: "../".repeat(depth),
        })
    }

    /// A file UNDER `cwd` loses the prefix; anything else climbs back out.
    pub fn apply(&self, file: &str) -> String {
        match file.strip_prefix(&self.prefix) {
            Some(inside) => inside.to_string(),
            None => format!("{}{file}", self.up),
        }
    }
}

/// The project root for `start`: the nearest ancestor carrying a VCS marker,
/// or `start` itself when there is none.
///
/// Why this exists: the index is keyed by the directory a command ran in, and
/// that directory used to be the raw cwd. An agent that `cd`s into a
/// subdirectory — which is most of what an agent does — got its OWN index of
/// that subtree, so `query` answered `no symbol matching` about a symbol
/// declared one crate over. Measured on this repository: `query
/// fresh_index_with_calls` from `crates/tokenade-core/src` said the symbol did
/// not exist, and the store carried a second 12 MB index of that subtree
/// beside the 22 MB one of the repository. A confident negative from a
/// narrower scope is the failure this whole document keeps naming.
///
/// STOPS at `$HOME` and at the filesystem root, and never returns either. A
/// home directory under version control — dotfile repositories are common —
/// would otherwise make every project in it index the whole home directory.
pub fn project_root_of(start: &std::path::Path) -> PathBuf {
    const VCS_MARKERS: &[&str] = &[".git", ".hg", ".svn", ".jj"];
    // Bounded: a symlink loop cannot make `parent()` cycle, but a
    // pathologically deep path should not walk forever either.
    const MAX_DEPTH: usize = 64;
    let home = home_dir();
    let mut cur = Some(start);
    for _ in 0..MAX_DEPTH {
        let Some(dir) = cur else { break };
        if dir.parent().is_none() {
            break; // filesystem root
        }
        if home.as_deref() == Some(dir) {
            break;
        }
        // A worktree's `.git` is a FILE, not a directory — `exists` covers both.
        if VCS_MARKERS.iter().any(|m| dir.join(m).exists()) {
            return dir.to_path_buf();
        }
        cur = dir.parent();
    }
    start.to_path_buf()
}

/// Bumped when the stash permission rule changes, so one repair walk runs on
/// installs that had already completed the previous one. `2` = entries are
/// created 0600 by [`write_private`] rather than repaired afterwards.
#[cfg(unix)]
pub(super) const STASH_PERMS_RULE: &[u8] = b"2";

/// Tighten the stash, with both paths injected so the policy is testable
/// without mutating process-wide `HOME` — the same reason [`pick_home`] exists.
/// (The first version of these tests DID set `HOME`, and a concurrent test in
/// another thread resolving its home mid-window turned a 7-second suite into a
/// stalled one. Env mutation in a parallel test binary is a shared-state bug,
/// not a testing style.)
///
/// The DIRECTORY mode is the protection that actually holds: at 0700 no other
/// user can traverse in, whatever the individual files say. So it is tightened
/// unconditionally — one syscall.
///
/// The per-file walk is O(stash size) and the stash only grows. Measured
/// 2026-08-03 on a real install: 3 396 files = ~12 ms, paid on EVERY CLI start,
/// i.e. twice per tool call through the hooks. It was also pure re-work — the
/// writer already creates 0600 and nothing else writes there. So it now runs
/// only when the state gives a reason:
///   * the directory was found loose (the loose-stash scenario, and the only way a
///     third party could have reached the files at all), or
///   * this install has never completed a walk, which repairs files created
///     before the hardening pass existed.
///
/// `sentinel` must live OUTSIDE the stash so it can never be mistaken for a
/// stash entry by the reader or by LRU eviction.
#[cfg(unix)]
pub(super) fn tighten_stash(stash: &std::path::Path, sentinel: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let was_tight = std::fs::metadata(stash)
        .map(|m| m.permissions().mode() & 0o777 == 0o700)
        .unwrap_or(false);
    if let Ok(meta) = std::fs::metadata(stash) {
        let mut perms = meta.permissions();
        perms.set_mode(0o700);
        let _ = std::fs::set_permissions(stash, perms);
    }
    // The sentinel carries a VERSION, not just its existence. An install that
    // completed the walk under an older rule must run it once more when the
    // rule changes — and it did change: entries were being CREATED at 0644, so
    // "this install has already walked" meant "this install stopped repairing
    // the thing it never fixed at the source".
    if was_tight && std::fs::read(sentinel).is_ok_and(|b| b == STASH_PERMS_RULE) {
        return;
    }
    let tighten_file = |p: &std::path::Path| {
        // lstat: `set_permissions` FOLLOWS symlinks, so a link planted in the
        // stash would let us chmod a file outside it. Regular files only.
        if let Ok(meta) = std::fs::symlink_metadata(p) {
            if meta.file_type().is_file() {
                let mut perms = meta.permissions();
                perms.set_mode(0o600);
                let _ = std::fs::set_permissions(p, perms);
            }
        }
    };
    if let Ok(entries) = std::fs::read_dir(stash) {
        for entry in entries.flatten() {
            tighten_file(&entry.path());
        }
    }
    let _ = std::fs::write(sentinel, STASH_PERMS_RULE);
    tighten_file(sentinel);
}

/// Every text ledger this product writes under `~/.tokenade`.
///
/// Matched on the NAME containing `.jsonl` or `.log`, not on the extension.
/// A rotation renames `gain.rollup.jsonl` to `gain.rollup.jsonl.bak-<epoch>`,
/// whose extension is `bak-1784599592` — so an extension test drops the file
/// that holds the rows that were just moved out of the live one, and
/// `debug.log.1` likewise.
///
/// One list, because there were two: `scrub-secret` enumerated the ledgers to
/// rewrite and `ensure_storage_perms_secure` named the files to hold at 0600,
/// and the second was written out by hand. Measured on the development machine
/// 2026-08-27: `bypass_lost.jsonl`, 20 KB of command lines, at 0644 — the same
/// rows `gain.jsonl` carries, which the hand-written list did name.
pub fn state_text_ledgers(tdir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out: Vec<std::path::PathBuf> = std::fs::read_dir(tdir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains(".jsonl") || n.contains(".log"))
        })
        .collect();
    out.sort();
    out
}

/// Every file under `~/.tokenade` that must not be world-readable: the text
/// ledgers above, the SQLite stores with their `-wal`/`-shm` siblings, the
/// project registry, and the support bundle.
///
/// `last-report.zip` belongs here more than anything else in the directory: it
/// is a copy of the user's agent TRANSCRIPTS. Measured on the development
/// machine 2026-08-27, one bundle held 13 MB — full conversation JSONL from
/// three separate projects, `debug.log`, and a memory file — at 0644, because
/// the hand-written list did not name it and `std::fs::write` creates 0666
/// masked by the umask.
pub fn state_files_to_harden(tdir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = state_text_ledgers(tdir);
    let mut rest: Vec<std::path::PathBuf> = std::fs::read_dir(tdir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                n.contains(".db") || n == "projects-registry.txt" || n == "last-report.zip"
            })
        })
        .collect();
    rest.sort();
    out.append(&mut rest);
    out
}

pub fn ensure_storage_perms_secure() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // lstat everywhere below, never stat. `set_permissions` FOLLOWS a
        // symlink (verified 2026-08-03: a link chmod'd its target from 644 to
        // 600), so a link planted under `~/.tokenade` with one of the names we
        // harden would let this function change the mode of a file outside it.
        // Regular files and real directories only; a symlink is skipped.
        let chmod_dir = |p: &std::path::Path| {
            if let Ok(meta) = std::fs::symlink_metadata(p) {
                if meta.file_type().is_dir() {
                    let mut perms = meta.permissions();
                    perms.set_mode(0o700);
                    let _ = std::fs::set_permissions(p, perms);
                }
            }
        };
        let chmod_file = |p: &std::path::Path| {
            if let Ok(meta) = std::fs::symlink_metadata(p) {
                if meta.file_type().is_file() {
                    let mut perms = meta.permissions();
                    perms.set_mode(0o600);
                    let _ = std::fs::set_permissions(p, perms);
                }
            }
        };
        if let Some(home) = home_dir() {
            let tk = home.join(".tokenade");
            chmod_dir(&tk);
            // Enumerated from the directory, never named one by one: the
            // hand-written list held fifteen names and the directory had grown
            // past it. See [`state_files_to_harden`] for what it was missing.
            for f in state_files_to_harden(&tk) {
                chmod_file(&f);
            }
        }
        if let Some(cache) = cache_dir() {
            let tk_cache = cache.join("tokenade");
            tighten_stash(
                &tk_cache.join("stash"),
                &tk_cache.join(".stash-perms-checked"),
            );
        }
    }
    // Windows path: no-op. NTFS ACLs are a separate concern.
}

/// The shell command tuple for running an arbitrary command string.
///
/// Is this SQLite file carrying enough dead space to be worth rewriting?
///
/// SQLite never returns freed pages to the filesystem on its own — it keeps
/// them on a freelist and reuses them. That is the right default for a hot
/// store, and wrong for tokenade's, which churn: measured on a dogfood install,
/// `index.db` was **36 % freelist (19.4 MB of 53.6)**, `fold_store.db` 49 %,
/// `file_search.db` 97 %. `dedup.db` was the only one at 0 %, because its
/// eviction path already ends in `VACUUM`.
///
/// Both conditions must hold, and each rules out a different waste:
///   * a FRACTION, so a store that is simply large is left alone;
///   * an ABSOLUTE floor, so a mostly-empty 200 KB file is not rewritten for
///     190 KB.
///
/// A VACUUM rewrites the whole file and takes an exclusive lock, so this is
/// only ever asked once a day, off the per-command path. Measured: 88 ms to
/// reclaim 18.5 MB from a 51 MB `index.db`.
pub fn sqlite_waste_is_worth_vacuuming(page_count: u64, freelist: u64, page_size: u64) -> bool {
    freelist * page_size >= 4 * 1024 * 1024 && page_count > 0 && freelist * 4 >= page_count
}

/// VACUUM every store in `dir` whose freelist has grown past
/// [`sqlite_waste_is_worth_vacuuming`].
///
/// Returns `(stores_vacuumed, bytes_reclaimed)`. Entirely best-effort: a store
/// that is locked by another process, unreadable, or not a database is skipped
/// — reclaiming disk must never be able to fail a session.
pub fn vacuum_bloated_stores(dir: &std::path::Path) -> (usize, u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    let mut stores = 0usize;
    let mut freed = 0u64;
    for ent in entries.flatten() {
        let path = ent.path();
        if path.extension().and_then(|e| e.to_str()) != Some("db") {
            continue;
        }
        let Ok(before) = std::fs::metadata(&path).map(|m| m.len()) else {
            continue;
        };
        let Ok(conn) = rusqlite::Connection::open(&path) else {
            continue;
        };
        let page = |name: &str| -> u64 {
            conn.query_row(&format!("PRAGMA {name}"), [], |r| r.get::<_, i64>(0))
                .map(|v| v.max(0) as u64)
                .unwrap_or(0)
        };
        if !sqlite_waste_is_worth_vacuuming(
            page("page_count"),
            page("freelist_count"),
            page("page_size"),
        ) {
            continue;
        }
        if conn.execute_batch("VACUUM").is_err() {
            continue;
        }
        drop(conn);
        let after = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(before);
        stores += 1;
        freed += before.saturating_sub(after);
    }
    (stores, freed)
}
