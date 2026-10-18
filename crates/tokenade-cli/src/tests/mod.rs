//! Unit tests for the tokenade-cli binary. Moved verbatim out of main.rs
//! (refactor split) — `mod tests;` in main.rs includes this file.

use super::*;
// The per-module globs the main.rs -> tests.rs split left behind are gone:
// every one was unused, and a glob that imports nothing hides the fact that
// this file's tests reach for their subjects by full path.
use crate::mcp_proxy::{derive_mcp_target_label, transform_mcp_response_line};

// One file per subject, split out of an 8007-line `tests.rs`.
// A test lands next to the module it exercises; the helpers every
// file needs (env locks, tmp dirs, the CLI runner) stay here.
mod agent;
mod code_intel;
mod compactors;
mod dashboard_cli;
mod diagnostics_cli;
mod healthcheck;
mod hooks;
mod hookwrap;
mod install;
mod install_cli;
mod lifecycle;
mod mcp_proxy;
mod misc;
mod wrap;

/// Serializes tests that mutate process env (`HERMES_HOME`,
/// `XDG_CONFIG_HOME`) before calling an installer that resolves a path
/// from it. The lock stops a parallel test from observing a half-set
/// value. Poison-tolerant (a panicking holder must not wedge the suite).
/// ALIAS for [`tokenade_core::platform::ENV_LOCK`], after the per-site audit
/// that was asked for and then done. The two facts that made it safe:
///
/// * **No test holds it across a `run()` call** — the invariant `run()`'s own
///   doc comment already claimed, re-verified by walking every holder.
/// * **One lock order, `ENV_LOCK` outermost**, established at the seven sites
///   that take both. Two orders in one tree is an inversion waiting to happen,
///   and the tree had both.
static INSTALL_ENV_LOCK: &std::sync::Mutex<()> = &tokenade_core::platform::ENV_LOCK;

/// Serialises tests that rewrite process-global env (HOME) against tests whose
/// subject reads it while running in parallel. A plain mutex here serialises
/// the whole CLI suite (measured: 2.5 s → 71 s), so readers and the writer are
/// separated instead:
///
/// - every command via `run()`, and every test that folds or stashes, takes a
///   **read** lock — they run in parallel with each other;
/// - a test that must rewrite that env takes the **write** lock, the only
///   exclusive moment.
pub(crate) static ENV_RW_LOCK: std::sync::RwLock<()> = std::sync::RwLock::new(());

fn tmp(label: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "tokenade-cli-{}-{}-{}",
        label,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

#[cfg(unix)] // builds /proc-shaped fixtures (D5 cross-check)
fn proc(pid: &str, exe: &str, cmdline: &str) -> (String, String, String) {
    (pid.to_string(), exe.to_string(), cmdline.to_string())
}

/// Serialises every test that drives `TOKENADE_READ_PENDING_DIR`.
/// That env var is process-global, so without a shared lock parallel
/// tests stomp each other's dir mid-run (one test's `set_var` redirects
/// another's `hint_fresh_this_session`/`reset_session_hints` to the
/// wrong dir → spurious failures). Same pattern as `cwd_lock`.
fn read_pending_lock() -> &'static std::sync::Mutex<()> {
    // #297 — THE env lock, not a private one. What this guards is a `set_var`
    // of `TOKENADE_READ_PENDING_DIR`, and a lock only serialises the callers
    // that take the same one: a private mutex here let these tests mutate
    // process-wide state at the same moment as every other env-mutating test.
    // Verified before merging: none of the five call sites builds a
    // `ScratchHome` or takes `ENV_LOCK` itself, so nothing nests — `Mutex` is
    // not reentrant and that is the only hazard.
    &tokenade_core::platform::ENV_LOCK
}

// Process-wide lock: cargo runs tests in parallel and `set_current_dir`
// is process-global, so we must serialise every cwd-changing test.
fn cwd_lock() -> &'static std::sync::Mutex<()> {
    static M: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    M.get_or_init(|| std::sync::Mutex::new(()))
}

/// A scratch working directory AND a scratch HOME.
///
/// It redirected only the cwd, so any command that reads the user's store read
/// the REAL one. Measured 2026-09-03: `cli_dashboard_runs_on_empty_dir` — three
/// lines that assert `dashboard` exits 0 in an empty directory — took **181
/// seconds of a 194-second suite**, because it scanned this machine's actual
/// 9.3 MB gain ledger through a debug build. It is the single slowest test in
/// the workspace by two orders of magnitude, and it was never testing the
/// ledger at all. A test named "on empty dir" that reads the developer's data
/// is also wrong on its own terms, whatever it costs.
///
/// **Two locks on ONE variable lock nothing.** `cwd_lock` serialises the
/// callers of this helper, and that is all it does: `HOME` is also written by
/// the dozen tests that take `INSTALL_ENV_LOCK` (that is,
/// `platform::ENV_LOCK`), which does not know `cwd_lock`. The two families
/// stepped on each other. MEASURED on 2026-09-12:
/// `with_tmp_cwd_gives_a_scratch_home_and_restores_it` accused this helper of
/// not restoring `HOME` — "left: /home/<user>, right: /tmp/tok-gem-2884808"
/// — when it had restored it perfectly: the "real" `HOME` the test had read
/// BEFORE was already a neighbouring test's scratch.
///
/// So this helper ALSO takes `ENV_LOCK`, and takes it FIRST (`cwd_lock` has no
/// other site, no lock-order inversion is possible). A caller that built a
/// `ScratchHome` or a `StoreIsolation` INSIDE the closure would deadlock
/// itself: none does, and this is the right place to learn it.
fn with_tmp_cwd<F: FnOnce() -> R, R>(f: F) -> R {
    with_tmp_cwd_probe(f).2
}

/// Like [`with_tmp_cwd`], but also returns `HOME` **before** and **after**
/// the restore, both read under the lock.
///
/// It is the only way to write the invariant "`HOME` is put back" without
/// measuring it from outside the lock — which is exactly where the false
/// accusation above came from.
fn with_tmp_cwd_probe<F: FnOnce() -> R, R>(
    f: F,
) -> (Option<std::ffi::OsString>, Option<std::ffi::OsString>, R) {
    let _env = tokenade_core::platform::ENV_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _guard = cwd_lock().lock().unwrap_or_else(|p| p.into_inner());
    let prev = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let prev_home = std::env::var_os("HOME");
    let d = tmp("cwd");
    std::env::set_current_dir(&d).unwrap();
    std::env::set_var("HOME", &d);
    let r = f();
    match &prev_home {
        Some(h) => std::env::set_var("HOME", h),
        None => std::env::remove_var("HOME"),
    }
    let restored = std::env::var_os("HOME");
    // Best-effort cwd restore — if prev is gone we just keep the tmp,
    // which is harmless because the next test will set its own.
    let _ = std::env::set_current_dir(prev);
    let _ = fs::remove_dir_all(&d);
    (prev_home, restored, r)
}

fn args(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

/// Hermetic wrapper around main's dispatcher (shadows `crate::run` for the
/// tests below): holds [`env_guard`] for the length of the call, so a command
/// never runs while another test has process env rewritten. Verified
/// deadlock-free: no test holds `INSTALL_ENV_LOCK` across a `run()` call.
fn run(a: &[String]) -> i32 {
    let _g = env_guard();
    crate::run(a)
}

/// The READ side of [`ENV_RW_LOCK`]: parallel with every other reader,
/// exclusive only against a test that rewrites process env. Every test whose
/// subject reads that env (stash location, gain ledger) must hold this — not
/// only the ones going through [`run`].
pub(crate) fn env_guard() -> std::sync::RwLockReadGuard<'static, ()> {
    ENV_RW_LOCK.read().unwrap_or_else(|p| p.into_inner())
}

/// Reject any exe token that would reopen either prod bug, in any shell
/// Claude Code might spawn the hook/command with. Used by the cases below.
fn assert_win_exe_token_cross_shell_safe(tok: &str, ctx: &str) {
    // cmd.exe: a path token must not be a *bare* (unquoted) path — quoting
    // is what stops cmd from reading `/AppData` etc. as switches.
    assert!(
        tok.starts_with('"') && tok.ends_with('"'),
        "{ctx}: exe token must be double-quoted (cmd.exe switch-safe): {tok}"
    );
    // Git Bash: backslashes only survive INSIDE the quotes — and there
    // must be NO unquoted backslash anywhere.
    let inner = &tok[1..tok.len() - 1];
    assert!(
        !inner.contains('"'),
        "{ctx}: inner path must not contain a nested quote: {tok}"
    );
    // cmd.exe is happiest with native separators; we emit backslashes.
    assert!(
        !inner.contains('/'),
        "{ctx}: forward slash in path reopens the cmd.exe-interactive bug: {tok}"
    );
}

/// Build a page whose distilled size is controllable and whose anchor
/// term (`zeta_widget`) appears on a sparse subset of lines.
fn gate_page(lines: usize) -> String {
    let mut p = String::from("# doc\n");
    for i in 0..lines {
        if i % 20 == 0 {
            p.push_str(&format!(
                "Row {i}: zeta_widget calibration defaults to 42ms here.\n"
            ));
        } else {
            p.push_str(&format!(
                "Ordinary filler prose sentence number {i} about assorted things.\n"
            ));
        }
    }
    p
}

#[cfg(test)]
mod one_lock_for_the_environment {
    /// #297 — every private `Mutex<()>` that serialises ENVIRONMENT mutation is
    /// a lock nobody else takes, which is the same as no lock at all. Two were
    /// merged into `platform::ENV_LOCK` and one now IS it.
    ///
    /// The survivors are listed with the reason they are not the same thing, so
    /// adding a sixth is a decision instead of an accident:
    ///
    /// * `ENV_RW_LOCK` is an `RwLock`. Reader parallelism is the point —
    ///   every test whose subject reads process env holds the READ side, and
    ///   collapsing it into a plain mutex would serialise all of them.
    /// * `INSTALL_ENV_LOCK` is held across `run()`, which takes the
    ///   `ENV_RW_LOCK` read lock. Merging it needs a per-site audit of that interaction, not
    ///   a sweep.
    /// * `cwd_lock` guards `set_current_dir`, a DIFFERENT process-global.
    ///   Merging distinct resources serialises tests that never conflict.
    #[test]
    fn no_new_private_environment_mutex_appears() {
        // The WHOLE test tree, not `mod.rs` alone. The fourteen other files of
        // `src/tests/` declare tests that mutate `env` like this one; a sixth
        // private lock would have been born there unseen, and that is exactly
        // what this test claims to prevent.
        let dir = tokenade_core::source_guard::crates_dir()
            .join("tokenade-cli")
            .join("src")
            .join("tests");
        let mut sources: Vec<(String, String)> = Vec::new();
        let mut stack = vec![dir];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    let body = std::fs::read_to_string(&p).expect("source lisible");
                    sources.push((p.display().to_string(), body));
                }
            }
        }
        sources.sort();
        assert!(
            sources.len() >= 14,
            "seulement {} fichier(s) de test parcouru(s)",
            sources.len()
        );
        let known = ["ENV_RW_LOCK", "INSTALL_ENV_LOCK", "cwd_lock"];
        // A RETURN TYPE is not a declaration, and a NEIGHBOURING COMMENT is not
        // a name. Two earlier versions of this guard got that wrong: the first
        // read `fn read_pending_lock() -> &'static Mutex<()>` as a declaration,
        // and the second skipped a real find because the doc comment three
        // lines above happened to say "same pattern as `cwd_lock`". Both were
        // caught by mutation on a clean tree — a detector that accuses, or
        // excuses, without reading the identifier is not evidence.
        //
        // So: the static's OWN name, or the name of the function that encloses
        // it. Nothing else counts.
        let mut found = Vec::new();
        for (path, src) in &sources {
            let lines: Vec<&str> = src.lines().map(str::trim).collect();
            // Le nom de la fonction englobante ne traverse PAS un fichier.
            let mut enclosing = String::new();
            for (i, t) in lines.iter().enumerate() {
                if let Some(rest) = t.strip_prefix("fn ") {
                    enclosing = rest.split('(').next().unwrap_or("").to_string();
                }
                if t.starts_with("//") || t.starts_with("fn ") {
                    continue;
                }
                if !t.contains("Mutex<()>") && !t.contains("RwLock<()>") {
                    continue;
                }
                let Some(rest) = t.strip_prefix("static ") else {
                    continue;
                };
                let name = rest.split(':').next().unwrap_or("").trim();
                if known.contains(&name) || known.contains(&enclosing.as_str()) {
                    continue;
                }
                found.push(format!("{path}:{}: {t}", i + 1));
            }
        }
        assert!(
            found.is_empty(),
            "a private lock over process-global state, not on the reasoned list:\n{}",
            found.join("\n")
        );
    }
}
