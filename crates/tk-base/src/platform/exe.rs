//! **Which tokenade binary, and a short lock.**
//!
//! An executable launched from `target/debug` is not the installed one,
//! and confusing the two measures a version that is no longer the tree. The
//! lock is here because it protects a file, not a data structure.

/// Absolute path of the tokenade binary that a PERSISTED config should name.
///
/// Never the running binary when that binary is transient. A config written by
/// a build-dir or scratch-copy binary outlives it: `cargo clean`, a rebuild, or
/// deleting an experiment then breaks every hook, shim and MCP wrap that names
/// it, silently and for good.
///
/// Lived in `install_cli::shim` alone until 2026-08-08, which is why
/// `mcp_wrap` — a different crate — reimplemented the naive
/// `env::current_exe()` version and wired seven agents' MCP entries to
/// `…/target/release/tokenade`. One authority now,
/// in the crate both callers can reach.
/// IS the process running this code the installed tokenade?
///
/// A single declarer, because two callers want OPPOSITE directions
/// from it: `proxy_after_upgrade` delegates when the answer is no,
/// `restart_stale_proxy` abstains when the answer is no. Written twice
/// by hand, the `installed != ours` comparison was wrong in one direction.
///
/// Three systems, one rule per system:
/// * **Linux** — the kernel suffixes `/proc/self/exe` with ` (deleted)` as soon as the
///   file is replaced. That suffix alone proves the answer is no: the
///   bare path now designates the NEW binary, while this process's memory
///   carries the old code. Comparing paths after stripping it
///   would answer "yes" exactly during an update.
/// * **macOS / Windows** — the inode is not unlinked under a live process
///   (Windows even refuses to replace a running `.exe`), so the question
///   reduces to "is it the same FILE?", resolved by `canonicalize` so that
///   neither a symlink nor a `..` wrongly makes it answer no.
pub fn running_the_installed_binary() -> bool {
    let Ok(cur) = std::env::current_exe() else {
        return false;
    };
    let cur = cur.display().to_string();
    if cur.ends_with(" (deleted)") {
        return false;
    }
    same_executable(&cur, &stable_exe())
}

/// Do two paths designate the same executable file?
///
/// `canonicalize` on both sides: `~/.local/bin/tokenade` and the real path
/// behind a symlink are the same binary, and comparing them as
/// strings would say no. A path that does not resolve is kept as is, which
/// lets the textual comparison decide rather than inventing an equality.
pub fn same_executable(a: &str, b: &str) -> bool {
    let real = |p: &str| {
        std::fs::canonicalize(p)
            .map(|c| c.display().to_string())
            .unwrap_or_else(|_| p.to_string())
    };
    real(a) == real(b)
}

pub fn stable_exe() -> String {
    let cur = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "tokenade".into());
    stable_exe_from(&cur)
}

/// The rule, with the running path injected — so it is testable without
/// pretending to be a different binary.
pub fn stable_exe_from(cur: &str) -> String {
    if is_build_dir_path(cur) {
        if let Some(installed) = which_installed_tokenade() {
            return installed;
        }
    }
    // Never hand back an absolute path that isn't on disk: that installs a
    // dead command.
    if std::path::Path::new(cur).is_absolute() && !std::path::Path::new(cur).exists() {
        return which_installed_tokenade().unwrap_or_else(|| "tokenade".into());
    }
    // Generalized transient-exe guard (field bug 2026-07-13): an ad-hoc COPY of
    // the binary, run from a scratch dir that the `/target/`,`/dist/` check
    // does not recognise, would re-point everything at itself and then vanish.
    // If an installed tokenade exists and is a DIFFERENT file, name the
    // install. With no install present (a dev binary performing the very first
    // install) fall through to `cur`.
    if let Some(installed) = which_installed_tokenade() {
        let real = |p: &str| {
            std::fs::canonicalize(p)
                .map(|c| c.display().to_string())
                .unwrap_or_else(|_| p.to_string())
        };
        if real(cur) != real(&installed) {
            return installed;
        }
    }
    cur.to_string()
}

/// Is this path inside a build output directory?
pub fn is_build_dir_path(p: &str) -> bool {
    let n = p.replace('\\', "/");
    n.contains("/dist/") || n.contains("/target/")
}

/// Absolute path to an INSTALLED `tokenade` on PATH that is not itself a
/// build-dir artifact.
pub fn which_installed_tokenade() -> Option<String> {
    let names: &[&str] = if cfg!(windows) {
        &["tokenade.exe", "tokenade"]
    } else {
        &["tokenade"]
    };
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for name in names {
            let cand = dir.join(name);
            let cand_str = cand.display().to_string();
            if is_build_dir_path(&cand_str) {
                continue;
            }
            if let Ok(md) = std::fs::metadata(&cand) {
                if md.is_file() {
                    // Resolve symlinks so a wiped target behind the symlink is
                    // still caught as a build-dir path.
                    let real = std::fs::canonicalize(&cand).unwrap_or(cand);
                    if !is_build_dir_path(&real.display().to_string()) {
                        return Some(cand_str);
                    }
                }
            }
        }
    }
    None
}

/// How long ANY tokenade file lock may wait for another process before giving
/// up on it.
///
/// `fs2`'s `lock_exclusive()` blocks with **no ceiling**, and every append site
/// already discarded its result (`let _ = …`) — a failed lock has always meant
/// "carry on" — so the unbounded wait bought nothing but the wait. What it
/// costs is measured: in this repository's own suite, with 56 test binaries
/// running wrapped commands at once, one `tokenade wrap` had still not started
/// its work after **30 seconds**, four times across one session, where the same
/// test alone takes 0.25 s. The ledger is telemetry. A savings row is never
/// worth delaying the command the agent actually ran, and 250 ms is two orders
/// of magnitude above the 0.05 s median startup this contention normally costs.
pub const LEDGER_LOCK_WAIT: std::time::Duration = std::time::Duration::from_millis(250);

/// **The handle must have been opened with read access.** `LockFileEx`
/// requires `GENERIC_READ` or `GENERIC_WRITE` on it, and Rust's
/// `.append(true)` asks for `FILE_GENERIC_WRITE` MINUS `FILE_WRITE_DATA`,
/// which is neither: on Windows an append-only handle cannot be locked at all
/// and this function answers `false` for a lock nobody holds. `flock` asks
/// nothing of the open mode, so the omission is INVISIBLE on Unix and macOS.
///
/// MEASURED in the 1.1.9 release build, the first to poll for the lock instead
/// of blocking on it: 25 tests failed on the Windows leg and nowhere else,
/// twenty of them on `if !lock_briefly(&lock) { return }` over a temp
/// directory the test had created three lines earlier. The blocking
/// `lock_exclusive()` this replaced failed the same way; discarding its result
/// (`let _ = …`) is what kept it quiet. `every_lockable_append_is_readable`
/// holds the open sites to it.
///
/// Take `f`'s exclusive lock within [`LEDGER_LOCK_WAIT`]; say whether it was
/// taken.
///
/// Polls rather than blocks, because the answer "no" has to arrive. The append
/// sites ignore the result — one line under `O_APPEND` is what they already
/// wrote on a lock error — while every read-modify-write site (rotation,
/// rollup, prune) does nothing at all rather than rewrite the file underneath
/// whoever holds the lock, which is the race those locks exist to close.
pub fn lock_briefly(f: &std::fs::File) -> bool {
    use fs2::FileExt;
    let deadline = std::time::Instant::now() + LEDGER_LOCK_WAIT;
    loop {
        if f.try_lock_exclusive().is_ok() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}
