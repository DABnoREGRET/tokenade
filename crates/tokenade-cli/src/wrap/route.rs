//! **What was typed, and what will really run.**
//!
//! A command line can carry a shell, variable assignments, metacharacters,
//! a shell builtin, or a plain executable. The route decides, and getting it
//! wrong here measures the wrong program — `whoami /all` swallowed by a
//! passthrough, `net start` announced on macOS.

use super::*;

/// Fast-path routing decision for `cmd_wrap`: run the inner command via a
/// direct `Command` (no `sh -c` fork) when it is PROVABLY a plain
/// `word word word` invocation, else keep the historical `sh -c` behaviour.
/// Conservative by design — at the slightest doubt we return `Shell` (just
/// slower, never wrong). The `Direct` path feeds the IDENTICAL downstream
/// compaction / gain-logging / dedup pipeline, so its output is byte-
/// equivalent to `Shell` — only faster.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Route {
    Shell,
    Direct,
}

/// Shell metacharacters that force the `sh -c` path — ANY occurrence means
/// the command may lean on shell parsing (pipes, redirects, expansion,
/// quoting, subshells, globs, comments, history). Space/tab are the token
/// separators (tab is listed so an embedded tab still forces the shell).
pub(super) const WRAP_SHELL_METACHARS: &[char] = &[
    '|', '&', ';', '<', '>', '(', ')', '$', '`', '*', '?', '[', ']', '{', '}', '~', '!', '#', '\'',
    '"', '\\', '\n', '\t',
];

/// POSIX shell builtins / state-modifiers that have no standalone binary:
/// direct-exec would fail (`cd`, `export`, …) or only mutate a throwaway
/// child's state. Route these to `sh -c`.
pub(super) const WRAP_SHELL_BUILTINS: &[&str] = &[
    "cd", "export", "source", ".", "eval", "exec", "set", "unset", "alias", "unalias", "pushd",
    "popd", "umask", "ulimit", "trap", "wait", "jobs", "fg", "bg", "local", "declare", "typeset",
    "let", "shift", "return", "exit", "read", "readonly", "time",
];

/// True when `tok` is a leading env assignment matching `^[A-Za-z_][A-Za-z0-9_]*=`
/// (e.g. `FOO=bar cmd`) — the shell must interpret it, so route to `sh -c`.
pub(super) fn is_env_assignment(tok: &str) -> bool {
    let mut chars = tok.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    for c in chars {
        if c == '=' {
            return true;
        }
        if !(c.is_ascii_alphanumeric() || c == '_') {
            return false;
        }
    }
    false
}

#[cfg(unix)]
pub(super) fn path_is_executable(p: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
pub(super) fn path_is_executable(p: &std::path::Path) -> bool {
    p.is_file()
}

/// Resolve `name` the way the shell would when it decides `command not
/// found`: an explicit path (contains a separator) is resolvable iff the
/// file exists and is executable; a bare name is searched on `$PATH`.
/// Mirrors `tokenade_core::availability::which_uncached` but kept local so
/// `wrap_route` has no cross-crate coupling.
pub(crate) fn command_on_path(name: &str) -> bool {
    if name.contains('/') || (cfg!(windows) && name.contains('\\')) {
        return path_is_executable(std::path::Path::new(name));
    }
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    for dir in std::env::split_paths(&path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        if path_is_executable(&dir.join(name)) {
            return true;
        }
        #[cfg(windows)]
        {
            // Resolve `git` → `git.exe` the way cmd.exe does: try every
            // extension in %PATHEXT% (falling back to the documented default
            // set). Pure cmd.exe builtins (`dir`, `type`, `copy`, `echo`) have
            // NO backing file for any extension, so they stay unresolved here
            // and `wrap_route` sends them to the shell — never a broken
            // `Command::new("dir")`. Build `name.ext` by string join rather
            // than `set_extension`, which would clobber a real dot in the stem
            // (`python3.11` → `python3.exe`).
            let pathext =
                std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
            for ext in pathext.split(';') {
                let ext = ext.trim().trim_start_matches('.');
                if ext.is_empty() {
                    continue;
                }
                if path_is_executable(&dir.join(format!("{name}.{ext}"))) {
                    return true;
                }
            }
        }
    }
    false
}

/// Decide whether `cmd` can be direct-exec'd (fast-path) or must go through
/// `sh -c`. Reads the `TOKENADE_WRAP_NO_FASTPATH` kill-switch and the real
/// `$PATH`; the pure decision core is [`wrap_route_inner`].
pub(crate) fn wrap_route(cmd: &str, shell_override: Option<ShellKind>) -> Route {
    let no_fastpath = tokenade_core::env_compat::is_truthy_one("TOKENADE_WRAP_NO_FASTPATH");
    wrap_route_inner(cmd, shell_override, no_fastpath, command_on_path)
}

/// Pure, side-effect-free routing core (env + PATH injected) so the exhaustive
/// routing table can be tested deterministically. `Direct` requires: no
/// explicit `--shell`, kill-switch off, no shell metacharacter, first token
/// is neither an env assignment nor a shell builtin, and argv[0] resolves.
pub(crate) fn wrap_route_inner(
    cmd: &str,
    shell_override: Option<ShellKind>,
    no_fastpath: bool,
    on_path: impl Fn(&str) -> bool,
) -> Route {
    // `--shell` explicit → the caller knows what interpreter it wants; honor it.
    if shell_override.is_some() {
        return Route::Shell;
    }
    // Kill-switch: force the historical shell path.
    if no_fastpath {
        return Route::Shell;
    }
    // Any shell metacharacter → the shell may need to parse it.
    if cmd.chars().any(|c| WRAP_SHELL_METACHARS.contains(&c)) {
        return Route::Shell;
    }
    let mut toks = cmd.split_whitespace();
    let Some(first) = toks.next() else {
        return Route::Shell; // empty command: let the shell produce its message
    };
    // Leading env assignment (`FOO=bar cmd`).
    if is_env_assignment(first) {
        return Route::Shell;
    }
    // Shell builtin / state-modifier with no standalone binary.
    if WRAP_SHELL_BUILTINS.contains(&first) {
        return Route::Shell;
    }
    // argv[0] must resolve, else defer to `sh -c` for a matching error message.
    if !on_path(first) {
        return Route::Shell;
    }
    Route::Direct
}

/// What `wrap` needs from its argv once the flags are off the front.
pub(crate) struct WrapInvocation<'a> {
    /// The command as one string, re-joined: a careless caller may split it
    /// across argv slots, and `sh -c` re-tokenises it anyway.
    pub(crate) shell_cmd: String,
    /// The interpreter to run it under.
    pub(crate) shell_kind: ShellKind,
    /// `Some` only when `--shell` said so; the direct-exec route needs to know
    /// the difference between "chosen" and "detected".
    pub(crate) shell_override: Option<ShellKind>,
    /// Compact each stream separately and return it to its own fd.
    pub(crate) split_streams: bool,
    _rest: std::marker::PhantomData<&'a ()>,
}

/// Parse `[--shell <kind>] [--split-streams] <shell-cmd>`.
/// `Err(rc)` = stop and exit with that code (a usage error was printed).
pub(super) fn parse_invocation(rest: &[String]) -> Result<WrapInvocation<'_>, i32> {
    // `--shell=<kind>`). The Claude Code PowerShell-tool hook passes
    // `--shell powershell`; the Bash-tool hook passes nothing (Posix). A
    // manual caller with no flag is auto-detected from the environment, then
    // falls back to Posix — the historical default, so existing callers are
    // unaffected.
    let mut rest = rest;
    let mut shell_override: Option<ShellKind> = None;
    if let Some(first) = rest.first() {
        if first == "--shell" {
            if let Some(k) = rest.get(1).and_then(|s| ShellKind::parse(s)) {
                shell_override = Some(k);
                rest = &rest[2..];
            } else {
                eprintln!("tokenade wrap: --shell needs one of posix|powershell|cmd");
                return Err(2);
            }
        } else if let Some(val) = first.strip_prefix("--shell=") {
            match ShellKind::parse(val) {
                Some(k) => {
                    shell_override = Some(k);
                    rest = &rest[1..];
                }
                None => {
                    eprintln!("tokenade wrap: --shell needs one of posix|powershell|cmd");
                    return Err(2);
                }
            }
        }
    }
    // `--split-streams`: compact each stream on its own and send it back to the
    // fd it came from. The default MERGES stderr into stdout, which is right
    // for an agent (it wants one transcript) but wrong for a script: our own
    // rules tell hook-less agents to wrap EVERY command, and a wrapped
    // `cmd > data.json` then writes the error text into the data file, which
    // the bare command would never do. Opt-in, because the merge is also what
    // makes chronological interleaving readable — you cannot have both.
    let mut split_streams = false;
    if let Some(first) = rest.first() {
        if first == "--split-streams" {
            split_streams = true;
            rest = &rest[1..];
        }
    }
    if rest.is_empty() {
        eprintln!("usage: tokenade wrap [--shell <kind>] [--split-streams] <shell-cmd>");
        return Err(2);
    }
    let shell_kind = shell_override.unwrap_or_else(ShellKind::detect_from_env);
    Ok(WrapInvocation {
        shell_cmd: rest.join(" "),
        shell_kind,
        shell_override,
        split_streams,
        _rest: std::marker::PhantomData,
    })
}

/// Run the command and capture both streams under a memory cap.
/// `Err(rc)` = the child could not be spawned or waited on.
pub(super) fn run_child(
    shell_cmd: &str,
    shell_kind: ShellKind,
    shell_override: Option<ShellKind>,
) -> Result<(std::process::Output, bool), i32> {
    use tokenade_core::process_util::{
        drain_capped_keeping_tail, DEFAULT_STDERR_CAP, DEFAULT_STDOUT_CAP,
    };
    let spawned = match wrap_route(shell_cmd, shell_override) {
        Route::Direct => {
            // Metachar-free + quote-free ⇒ whitespace-split is a faithful argv.
            let argv: Vec<String> = shell_cmd.split_whitespace().map(String::from).collect();
            let mut c = Command::new(&argv[0]);
            c.args(&argv[1..])
                // Mirror spawn_wrapped's re-entry guard (harmless for a direct
                // child that never re-sources the proxy, kept for symmetry).
                .env("TOKENADE_WRAPPING", "1")
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            tokenade_core::process_util::configure_no_window(&mut c);
            tokenade_core::process_util::configure_own_group(&mut c);
            tokenade_core::process_util::configure_die_with_parent(&mut c);
            c.spawn()
        }
        Route::Shell => spawn_wrapped(shell_cmd, shell_kind),
    };
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            eprintln!("tokenade wrap: failed to run '{shell_cmd}': {e}");
            return Err(127);
        }
    };
    // Cancelling the wrapper must cancel the WORK. Without this, killing
    // `wrap` left the real command running with nobody reading its output.
    // One decision, both halves: the group only EXISTS when a terminal does not
    // own it, and both the relay and the reaper aim at that group.
    let reaper = if !tokenade_core::process_util::terminal_owns_the_group() {
        tokenade_core::process_util::forward_termination_to(child.id());
        // The relay only covers signals we can CATCH. A timeout sends SIGKILL,
        // and the group we just created is one the agent can no longer aim at,
        // so the reaper is what holds the guarantee for the kill we never see.
        tokenade_core::process_util::guard_group_against_our_death(child.id())
    } else {
        None
    };
    let so = child.stdout.take().expect("piped");
    let se = child.stderr.take().expect("piped");
    let so_thr = drain_capped_keeping_tail(so, DEFAULT_STDOUT_CAP);
    let se_thr = drain_capped_keeping_tail(se, DEFAULT_STDERR_CAP);
    let status = match child.wait() {
        Ok(s) => s,
        Err(e) => {
            tokenade_core::process_util::stop_forwarding_termination();
            // No stand-down on this path ON PURPOSE: we never learned that the
            // child died, so letting the reaper fire as we exit is the correct
            // direction of failure.
            eprintln!("tokenade wrap: failed waiting on '{shell_cmd}': {e}");
            return Err(127);
        }
    };
    // Reaped: the pid could be recycled, so stop aiming signals at it.
    tokenade_core::process_util::stop_forwarding_termination();
    if let Some(r) = reaper {
        r.stand_down();
    }
    let (stdout_buf, stdout_truncated) = so_thr.join().unwrap_or_else(|_| (Vec::new(), false));
    let (stderr_buf, stderr_truncated) = se_thr.join().unwrap_or_else(|_| (Vec::new(), false));
    let out = std::process::Output {
        status,
        stdout: stdout_buf,
        stderr: stderr_buf,
    };
    // Here, not at a serve site: this is the one function every wrap route
    // passes through holding BOTH the status and the bytes, and there are
    // three serve paths. A notice at one of them is a notice the other two do
    // not give — the same reason `disclose_signal_death` lives in
    // `exit_code_of`.
    disclose_success_with_failure_evidence(&out, shell_cmd);
    Ok((out, stdout_truncated || stderr_truncated))
}
