#![allow(unused_imports)]
//! Shell proxy (aliases) — no longer INSTALLED by anything (since
//! 2026-07: the alias proxy and the PATH shim are both out of the product).
//! `tokenade install` never writes an rc alias line, and an upgrade rips out
//! whatever an older version left (see `uninstall::uninstall_shell_proxy`).
//!
//! What survives here is the ON-DEMAND fragment generator behind
//! `tokenade shell-init` ([`cmd_shell_init`]) and its alias target
//! `tokenade shellwrap` ([`cmd_shellwrap`]): a user who WANTS the aliases can
//! still source the fragment themselves. Nothing writes them to disk for them.
//! [`PROXY_DENYLIST`] / [`proxyable_on_path`] / the per-shell fragments stay
//! alive as that generator's inputs.
//!
//! Wired back into the `install` namespace by mod.rs via
//! `pub(crate) use <name>::*;`.
use super::*;
use crate::hooks::*;
use crate::wrap::*;
use crate::*;
use std::fs;
use std::path::{Path, PathBuf};

mod denylist;
mod snippets;
pub(crate) use denylist::*;
pub(crate) use snippets::*;

pub(crate) fn shell_init_help_text() -> &'static str {
    "tokenade shell-init — print the shell proxy fragment for sourcing.

USAGE:
  tokenade shell-init [--shell SHELL] [--all]

The fragment aliases noisy commands to route through `tokenade wrap`
when stdin is not a TTY (i.e. for AI agents, not humans). The command
list is DERIVED from tokenade's compactor registry at build time (every
command tokenade knows how to compact), minus a safety denylist
(text-stream tools, exact-bytes tools, shells, prefix wrappers), then
filtered to what is ACTUALLY PRESENT on PATH at generation time —
absent tools are never shadowed, so `command -v kubectl`-style probes
(scripts, prompt engines) keep telling the truth.

SHELL — auto-detected from $SHELL / PSModulePath when omitted:
  sh|bash|zsh|dash|ksh   POSIX fragment    eval \"$(tokenade shell-init)\"
  fish                   fish fragment     tokenade shell-init --shell fish | source
  powershell|pwsh        PS fragment       Invoke-Expression (& tokenade shell-init --shell powershell | Out-String)
  nushell|nu             nushell fragment  tokenade shell-init --shell nushell o> ~/.tokenade/shellproxy.nu; source it from config.nu
  csh|tcsh               csh fragment      aliases route via `tokenade shellwrap` (csh has no functions)
  cmd                    doskey macros     interactive cmd.exe only; see fragment header

FLAGS:
  --shell, -s SHELL   target shell (see above)
  --all               skip the PATH-presence filter and alias every
                      registry-derived command (useful for inspection/tests)
  --help, -h          show this message and exit

Opt-outs at runtime: TOKENADE_SHELL_PROXY=0 disables the proxy;
`tokenade uninstall aliases` removes it from every rc file."
}

/// `tokenade shell-init [--shell sh|bash|zsh|fish|powershell|nushell|csh|tcsh|cmd] [--all]`
///
/// Prints the shell proxy fragment for the requested shell (or auto-detects
/// from `$SHELL` / `$PSVersionTable`). Aliases are generated for the
/// intersection of the registry-derived command set (see
/// [`proxyable_on_path`]) and the binaries on PATH, so absent tools are
/// never shadowed; `--all` skips the presence filter.
/// Refuse an argument this command does not know, like every other
/// argument-less verb does.
///
/// `shell-init` accepted ANYTHING and printed the default fragment: measured
/// 2026-09-03, `--json`, `--nope`, `--posix`, `-x` and a bare `garbage` all
/// returned 0 and the same 4 916 bytes, while eight sibling verbs — `detect`,
/// `read-mode`, `measure`, `receipt`, `semantic-status`, `incompatibilities`,
/// `mcp-stats`, `hooks` — refuse an unknown flag with exit 2. `semantic`'s
/// own help states the norm: "An unknown flag is refused rather than folded
/// in."
///
/// Silence is not free here, because the argument that was ignored decides
/// WHICH SHELL the fragment is written for. `tokenade shell-init fish | source`
/// — the flag forgotten, the shell named as a positional — printed the POSIX
/// fragment, 4 916 bytes of `function`/`alias` syntax fish cannot parse, and
/// said nothing. `-all` (one dash) printed the 4 916-byte PATH-filtered list
/// where `--all` prints 29 461 bytes, so the caller believed it had the full
/// list.
///
/// The verdict itself is [`crate::install_cli::reject_unknown_flag`], so the
/// wording and the did-you-mean hint stay in one place.
fn validate_shell_init_args(rest: &[String]) -> Result<(), i32> {
    const ALLOWED: &[&str] = &["--shell", "-s", "--all"];
    let mut it = rest.iter().peekable();
    while let Some(a) = it.next() {
        if a == "--shell" || a == "-s" {
            // A flag that eats the NEXT flag as its value is the same silence
            // one level down: `--shell --all` would name the shell "--all".
            match it.peek() {
                Some(v) if !v.starts_with('-') => {
                    it.next();
                }
                _ => {
                    eprintln!(
                        "tokenade shell-init: flag '{a}' expects a value (e.g. `{a} fish`). \
                         Try `tokenade shell-init --help`."
                    );
                    return Err(2);
                }
            }
            continue;
        }
        if a.starts_with('-') && a.len() > 1 {
            crate::install_cli::reject_unknown_flag(a, "shell-init", ALLOWED)?;
            continue;
        }
        eprintln!(
            "tokenade shell-init: unexpected argument '{a}' — name the shell with \
             `--shell {a}` (this command takes no positional arguments). \
             Try `tokenade shell-init --help`."
        );
        return Err(2);
    }
    Ok(())
}

pub(crate) fn cmd_shell_init(rest: &[String]) -> i32 {
    if crate::help_routing::asks_for_help(rest) {
        println!("{}", shell_init_help_text());
        return 0;
    }
    if let Err(rc) = validate_shell_init_args(rest) {
        return rc;
    }
    let shell_arg = rest
        .iter()
        .position(|a| a == "--shell" || a == "-s")
        .and_then(|i| rest.get(i + 1))
        .map(|s| s.as_str())
        .unwrap_or("");
    let shell_env = std::env::var("SHELL").unwrap_or_default();
    // PowerShell sets PSVersionTable; also check PSMODULEPATH as a fallback.
    // Windows only: Linux CI runners and pwsh installs export PSModulePath to
    // every process, which says nothing about the shell that will source this.
    let is_ps_env = cfg!(windows)
        && (std::env::var("PSVersionTable").is_ok()
            || std::env::var("PSMODULEPATH").is_ok()
            || std::env::var("PSModulePath").is_ok());
    let shell_hint = if !shell_arg.is_empty() {
        shell_arg
    } else {
        &shell_env
    };
    let cmds: Vec<String> = if rest.iter().any(|a| a == "--all") {
        proxyable_all()
    } else {
        proxyable_on_path(&std::env::var_os("PATH").unwrap_or_default())
    };
    if shell_hint.contains("fish") {
        print!("{}", shell_init_fish(&cmds));
    } else if shell_hint.contains("pwsh")
        || shell_hint.contains("powershell")
        || shell_hint.contains("ps1")
        || shell_hint == "ps"
        || is_ps_env
    {
        print!("{}", shell_init_powershell(&cmds));
    } else if shell_hint.contains("csh") {
        // csh and tcsh — checked before the generic fallback; note zsh
        // does NOT match ("zsh" has no "csh" substring).
        print!("{}", shell_init_csh(&cmds));
    } else if shell_hint == "cmd" || shell_hint.contains("cmd.exe") {
        print!("{}", shell_init_cmd(&cmds));
    } else if shell_hint.contains("nu") || shell_hint.contains("nushell") {
        print!("{}", shell_init_nushell(&cmds));
    } else {
        // Covers bash, zsh, sh, dash, ash, busybox sh, ksh, mksh — all POSIX.
        print!("{}", shell_init_posix(&cmds));
    }
    0
}

/// `tokenade shellwrap <cmd> [args…]` — alias target for shells whose alias
/// mechanism cannot carry logic (csh/tcsh aliases, cmd.exe doskey macros).
///
/// Applies in-process the guards the other shell fragments inline as shell
/// code: when stdin is a TTY (interactive human), when
/// `TOKENADE_SHELL_PROXY=0` (opt-out), or when `TOKENADE_WRAPPING=1`
/// (recursion guard) the real command runs untouched with its exit code
/// passed through (127 when the binary is missing, like a shell). Otherwise
/// the argv is single-quote-escaped exactly like the POSIX fragment does
/// and routed through `tokenade wrap` for compaction.
pub(crate) fn cmd_shellwrap(rest: &[String]) -> i32 {
    use std::io::IsTerminal;
    if crate::help_routing::asks_for_help_leading(rest) {
        println!(
            "usage: tokenade shellwrap <cmd> [args…]\n\nTTY-aware alias target for csh/tcsh and cmd.exe (their aliases can't\ncarry logic). Interactive stdin / TOKENADE_SHELL_PROXY=0 /\nTOKENADE_WRAPPING=1 → exec the real command untouched; otherwise route\nthrough `tokenade wrap` for compaction. Exit code mirrors the inner\ncommand.\n\nFlags:\n  --help, -h    show this message and exit"
        );
        return 0;
    }
    let Some(prog) = rest.first() else {
        eprintln!("usage: tokenade shellwrap <cmd> [args…]");
        return 2;
    };
    // Pipeline safety (csh path): when stdout is redirected to a regular
    // file (`git show x > f`), the caller wants raw bytes — compacting would
    // corrupt the file. Mirrors the `[ -f /dev/stdout ]` check the POSIX
    // fragment inlines. (Mid-pipeline detection is not possible from here:
    // stdout being a pipe is indistinguishable from the agent's own capture
    // pipe, so left-of-pipe stages in csh remain a documented gap.)
    #[cfg(unix)]
    let stdout_is_file = std::fs::metadata("/dev/stdout").is_ok_and(|m| m.is_file());
    #[cfg(not(unix))]
    let stdout_is_file = false;
    let passthrough = std::io::stdin().is_terminal()
        || stdout_is_file
        || std::env::var("TOKENADE_SHELL_PROXY").as_deref() == Ok("0")
        || std::env::var("TOKENADE_WRAPPING").as_deref() == Ok("1");
    if passthrough {
        return match Command::new(prog).args(&rest[1..]).status() {
            Ok(s) => crate::wrap::exit_code_of(s),
            Err(e) => {
                eprintln!("tokenade shellwrap: {prog}: {e}");
                127
            }
        };
    }
    // Same single-quote escaping the POSIX fragment uses, so the `sh -c`
    // inside `wrap` re-tokenises the argv exactly as received.
    let quoted: Vec<String> = rest
        .iter()
        .map(|a| format!("'{}'", a.replace('\'', "'\\''")))
        .collect();
    std::env::set_var("TOKENADE_WRAPPING", "1");
    cmd_wrap(&[quoted.join(" ")])
}
