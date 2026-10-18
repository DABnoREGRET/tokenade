//! **The snippet placed in each shell's rc.**
//!
//! POSIX, fish, PowerShell, nushell, csh, cmd: six languages for the same
//! function, and none shares another's syntax.
//!
//! Each must be idempotent and exactly reversible — an rc edited twice by a
//! different version leaves two blocks of which the second wins, and the user
//! will never know which one they are reading.

use super::*;

/// POSIX proxy function — compatible with bash, zsh, dash, ash, busybox sh,
/// ksh93, mksh, and any POSIX-conformant `/bin/sh`. The alias loop is
/// appended per-machine by [`shell_init_posix`] so only present binaries
/// are shadowed.
///
/// Deliberately avoids ALL bash/zsh extensions:
///   • no `local`        — not in POSIX spec (works on dash, but not guaranteed)
///   • no `printf '%q'`  — bash/zsh extension; prints `%q` literally on dash/sh
///   • no arrays         — bash/ksh extension
///
/// Quoting strategy: single-quote each argument, escaping embedded single
/// quotes as `'\''` (end-quote, escaped-quote, re-open-quote) using sed.
/// This is the only portable approach that handles all POSIX input correctly.
///
/// `TOKENADE_WRAPPING=1` is exported before calling `tokenade wrap` so that
/// the child shell (`sh -c` inside wrap) sees the flag and skips the alias,
/// preventing infinite recursion.
pub(crate) const SHELL_INIT_POSIX_FN: &str = r#"
# tokenade shell proxy (v2) — https://tokenade.net
# Compatible: bash, zsh, dash, ash, busybox sh, ksh, mksh, /bin/sh on any server.
# Active only when stdin is not a TTY (AI agent / piped session); bypassed at interactive
# terminals — including command substitutions $(...) where stdin stays the terminal.
# Pipeline safety: compaction must only happen on output going STRAIGHT to the
# agent. Mid-pipeline stages (`git log | head`) and file redirects
# (`git show x > f`) need raw bytes, so the proxy also passes through when:
#   - the command runs in a subshell (bash puts every pipeline stage in one;
#     zsh all but the last — the last writes to the agent, so wrapping it is
#     correct and wrap forwards stdin). Also covers $(...) and `cmd &`.
#   - stdout is a regular file (redirect) — checked via /dev/stdout, present
#     on Linux/macOS/BSD; if the probe path is missing the test is false and
#     behavior is unchanged.
# Residual gap: plain dash/ash expose no subshell counter, so a LEFT pipe
# stage there is still wrapped. Agents overwhelmingly run `bash -c`, which
# is fully covered.
# FUNCNEST guard: bash/zsh abort a legitimately-nested function call the moment
# their FUNCNEST limit is reached — and that abort fires at the CALL, before the
# body-level sentinel below can run. The sentinel already breaks any real
# recursion, so lift the limit (bash/zsh only) to avoid a spurious
# "maximum nested function level reached" abort. The inner `tokenade wrap` shell
# re-sources this fragment, re-applying the unset.
if [ -n "${BASH_VERSION:-}" ] || [ -n "${ZSH_VERSION:-}" ]; then unset FUNCNEST 2>/dev/null || true; fi
__tokenade_proxy() {
  # Structural recursion firebreak (zsh/bash FUNCNEST). The passthrough uses
  # `command "$@"` and "$@" is a parameter expansion (never re-parsed for
  # aliases), so the common path can't self-recurse. But an exotic env — a
  # `tokenade` shadowed by a shell function/alias, a `command_not_found_handler`
  # that retries, a global alias — could still re-enter this function in the
  # SAME shell and blow the stack (reported: `__tokenade_proxy:3: maximum
  # nested function level reached`). The sentinel is held for the WHOLE body
  # (through the command/wrap call, where any re-entry actually happens): a
  # re-entry sees it set, runs the raw command, and returns — no wrap, no loop.
  if [ "${__tk_active:-0}" = "1" ]; then
    command "$@"
    __tk_rc=$?
    # Clear the sentinel on the way out so the NEXT top-level command re-enables
    # compaction — otherwise a body interrupted mid-flight (Ctrl-C) would leave
    # __tk_active stuck at 1 and every later command would pass through raw.
    unset __tk_active
    return $__tk_rc
  fi
  __tk_active=1
  if [ -t 0 ] || [ "${TOKENADE_SHELL_PROXY:-1}" = "0" ] || [ "${TOKENADE_WRAPPING:-0}" = "1" ] \
    || [ "${BASH_SUBSHELL:-0}" != "0" ] || [ "${ZSH_SUBSHELL:-0}" != "0" ] || [ -f /dev/stdout ]; then
    command "$@"; __tk_rc=$?; __tk_active=0; return $__tk_rc
  fi
  _tq=''
  for _ta in "$@"; do
    _te=$(printf '%s' "$_ta" | sed "s/'/'\\\\''/g")
    _tq="${_tq}'${_te}' "
  done
  # `command tokenade` bypasses any shell function/alias that shadows the
  # tokenade name (another belt against re-entry); the child inherits
  # TOKENADE_WRAPPING and `tokenade wrap` also forces it into the inner shell.
  # Capture + return the inner exit code: the trailing `unset` used to make
  # the function ALWAYS return 0, masking e.g. `git diff --quiet` == 1.
  TOKENADE_WRAPPING=1 command tokenade wrap "${_tq% }"
  __tk_rc=$?
  __tk_active=0
  unset _tq _ta _te
  return $__tk_rc
}
# bash expands aliases only in interactive shells by default, but agents run
# `bash -c …` (non-interactive, loaded via $BASH_ENV) — force expansion there.
# zsh and dash expand aliases unconditionally.
if [ -n "${BASH_VERSION:-}" ]; then shopt -s expand_aliases; fi
"#;

/// Build the full POSIX fragment: proxy function + aliases for `cmds`
/// (the PATH-present, tokenade-known, non-denylisted command set).
pub(crate) fn shell_init_posix(cmds: &[String]) -> String {
    let mut out = String::from(SHELL_INIT_POSIX_FN);
    if cmds.is_empty() {
        out.push_str("# no proxy-able commands found on PATH — nothing aliased.\n");
        return out;
    }
    out.push_str(&format!("for __tc in {}; do\n", cmds.join(" ")));
    out.push_str("  # shellcheck disable=SC2139\n  alias \"$__tc\"=\"__tokenade_proxy $__tc\"\ndone\nunset __tc\n");
    out
}

/// Fish proxy function; alias loop appended by [`shell_init_fish`].
pub(crate) const SHELL_INIT_FISH_FN: &str = r#"
# tokenade shell proxy (v2) — https://tokenade.net
# Active only when stdin is not a TTY (AI agent session); bypassed at interactive
# terminals — including command substitutions (...) where stdin stays the terminal.
# This avoids interfering with shell prompts (Tide, Starship) that call git in substs.
# Only binaries present on PATH at generation time are aliased.
# Pipeline safety: passthrough inside command substitutions (prompt engines,
# scripts capturing output) and when stdout is a regular file (redirect) —
# those consumers need raw bytes, not compacted text. Residual gap: fish
# exposes no way to detect a LEFT pipe stage (`git log | head`), so that
# stage is still wrapped in non-interactive fish scripts; agents run
# `bash -c`, where the bash fragment covers it.
# Opt-out: set -Ux TOKENADE_SHELL_PROXY 0
function __tokenade_proxy
  # Re-entry firebreak (symmetric to the POSIX sentinel): if a shadowing
  # function/alias or command-not-found handler re-enters us in the SAME shell,
  # run the raw command and return — no wrap, no unbounded nesting.
  if set -q __tk_active
    command $argv
    return $status
  end
  set -g __tk_active 1
  if isatty stdin; or test "$TOKENADE_SHELL_PROXY" = "0"; or test "$TOKENADE_WRAPPING" = "1"
    command $argv
    set -l __tk_rc $status
    set -e __tk_active
    return $__tk_rc
  end
  # `command test`, not the builtin: MEASURED 2026-08-08, a redirect applied to
  # a FUNCTION CALL (`git status > out.txt`, and the alias IS a function) is not
  # visible to fish's builtin `test` — it answers NOTFILE where bash answers
  # ISFILE for the identical shape. The guard therefore never fired under fish
  # and redirected output was COMPACTED: the file's consumer got folded text
  # instead of the raw bytes it parses. An external `test` runs in a child that
  # inherits the redirected fd 1, so it sees the file. Verified all three ways:
  # redirect → file, no redirect → not a file, pipe → not a file.
  if status is-command-substitution; or command test -f /dev/stdout
    command $argv
    set -l __tk_rc $status
    set -e __tk_active
    return $__tk_rc
  end
  set -lx TOKENADE_WRAPPING 1
  # `command tokenade` bypasses any fish function shadowing the tokenade name.
  command tokenade wrap (string escape -- $argv | string join ' ')
  set -l __tk_rc $status
  set -e __tk_active
  return $__tk_rc
end
"#;

/// Build the full fish fragment for the PATH-present command subset.
pub(crate) fn shell_init_fish(cmds: &[String]) -> String {
    let mut out = String::from(SHELL_INIT_FISH_FN);
    if cmds.is_empty() {
        out.push_str("# no proxy-able commands found on PATH — nothing aliased.\n");
        return out;
    }
    out.push_str(&format!(
        "for __tc in {}\n  alias $__tc \"__tokenade_proxy $__tc\"\nend\n",
        cmds.join(" ")
    ));
    out
}

/// PowerShell (pwsh 7+ and Windows PowerShell 5.x) init fragment.
///
/// Guards:
///  - `[Console]::IsInputRedirected`  — true when stdin is not a TTY (AI agent session).
///    Using stdin (not stdout) mirrors the POSIX/fish fix: PS prompt engines call git
///    inside `$(...)` where stdout is redirected but stdin stays the terminal, so the
///    old `IsOutputRedirected` guard incorrectly activated the proxy in those prompts.
///  - `TOKENADE_SHELL_PROXY=0`        — permanent user opt-out.
///  - `TOKENADE_WRAPPING=1`           — recursion guard (set before calling `tokenade wrap`).
///
/// Source into your PS profile with:
///   `Invoke-Expression (& tokenade shell-init --shell powershell | Out-String)`
pub(crate) const SHELL_INIT_POWERSHELL_FN: &str = r#"
# tokenade shell proxy (v2) — https://tokenade.net
# Active only when stdin is redirected (AI agent / piped session); bypassed at interactive
# terminals — including command substitutions $(...) where stdin stays the terminal.
# Only binaries present on PATH at generation time are aliased.
# Opt-out: $env:TOKENADE_SHELL_PROXY = '0'
function global:__tokenade_proxy {
  param([string]$Cmd, [Parameter(ValueFromRemainingArguments=$true)][string[]]$Rest)
  # Pipeline safety: when this call is one stage of a longer pipeline
  # (`git log | Select-Object -First 5`), the downstream stage needs raw
  # output — compacting mid-pipeline corrupts it. PipelineLength counts the
  # stages of the invoking pipeline, so >1 means "not going straight to the
  # agent". (A bare `git log > f` redirect is not detectable from inside the
  # function; PowerShell agents are rare enough that we accept that gap.)
  # Resolve the REAL external binary once. `& $Cmd` alone re-runs full command
  # discovery, which finds the same-named wrapper FUNCTION first (Alias >
  # Function > Cmdlet > Application) and recurses. Pinning to Application/
  # ExternalScript bypasses the wrapper — the PowerShell twin of the POSIX
  # firebreak.
  $__tkReal = Get-Command -Name $Cmd -CommandType Application, ExternalScript -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($null -eq $__tkReal) { $__tkReal = $Cmd }
  if (-not [Console]::IsInputRedirected -or
      $MyInvocation.PipelineLength -gt 1 -or
      $env:TOKENADE_SHELL_PROXY -eq '0' -or
      $env:TOKENADE_WRAPPING -eq '1') {
    & $__tkReal @Rest; return $LASTEXITCODE
  }
  $esc = (@($Cmd) + $Rest) | ForEach-Object { "'" + ($_ -replace "'", "''") + "'" }
  $env:TOKENADE_WRAPPING = '1'
  try { & tokenade wrap ($esc -join ' ') }
  finally { Remove-Item Env:TOKENADE_WRAPPING -ErrorAction SilentlyContinue }
}
"#;

/// Build the full PowerShell fragment for the PATH-present command subset.
pub(crate) fn shell_init_powershell(cmds: &[String]) -> String {
    let mut out = String::from(SHELL_INIT_POWERSHELL_FN);
    if cmds.is_empty() {
        out.push_str("# no proxy-able commands found on PATH — nothing aliased.\n");
        return out;
    }
    let list = cmds
        .iter()
        .map(|c| format!("'{c}'"))
        .collect::<Vec<_>>()
        .join(",");
    out.push_str(&format!(
        "@({list}) | ForEach-Object {{\n  $__tc = $_\n  Invoke-Expression \"function global:$__tc {{ global:__tokenade_proxy '$__tc' @args }}\"\n}}\nRemove-Variable __tc -ErrorAction SilentlyContinue\n"
    ));
    out
}

/// Nushell proxy function; alias lines appended by [`shell_init_nushell`].
///
/// Nushell has no `eval`, so this fragment is meant to be written to a file
/// and `source`d from `~/.config/nushell/config.nu` — `tokenade shell-init
/// --shell nushell` prints it; nothing installs it for you.
///
/// Opt-out: `$env.TOKENADE_SHELL_PROXY = "0"` in your `env.nu`.
pub(crate) const SHELL_INIT_NUSHELL_FN: &str = r#"
# tokenade shell proxy (v2) — https://tokenade.net
# Active only when stdin is not a terminal — the same guard as the
# POSIX/fish/PowerShell fragments. (v1 keyed on $nu.is-interactive, which
# diverged: pipes inside an interactive session were wrapped on every other
# shell but never here, and scripts run with a terminal stdin were wrapped
# here but nowhere else.)
# Only binaries present on PATH at generation time are aliased.
# Known gap: nushell exposes neither pipeline position nor an
# is-command-substitution probe, so a mid-pipeline stage (`git log | lines`)
# or an `o> file` redirect in a NON-interactive nu script is still wrapped
# and receives compacted text. Interactive sessions are safe (stdin guard),
# and agents run `bash -c`, which the bash fragment covers fully.
# Opt-out: set TOKENADE_SHELL_PROXY=0 in env.nu
def --wrapped __tokenade_proxy [cmd: string, ...rest: string] {
  let proxy   = ($env | get -i TOKENADE_SHELL_PROXY | default "1")
  let wrap    = ($env | get -i TOKENADE_WRAPPING    | default "0")
  if (is-terminal --stdin) or $proxy == "0" or $wrap == "1" {
    run-external $cmd ...$rest
    return
  }
  let escaped = ([$cmd] ++ $rest
    | each {|it| "'" + ($it | str replace --all "'" "''") + "'"}
    | str join " ")
  with-env {TOKENADE_WRAPPING: "1"} { ^tokenade wrap $escaped }
}
"#;

/// Build the full nushell fragment for the PATH-present command subset.
pub(crate) fn shell_init_nushell(cmds: &[String]) -> String {
    let mut out = String::from(SHELL_INIT_NUSHELL_FN);
    let cmds: Vec<&String> = cmds
        .iter()
        .filter(|c| !NU_BUILTIN_COLLISIONS.contains(&c.as_str()))
        .collect();
    if cmds.is_empty() {
        out.push_str("# no proxy-able commands found on PATH — nothing aliased.\n");
        return out;
    }
    let width = cmds.iter().map(|c| c.len()).max().unwrap_or(0);
    for c in cmds {
        out.push_str(&format!("alias {c:<width$} = __tokenade_proxy {c}\n"));
    }
    out
}

/// csh/tcsh proxy header; alias lines appended by [`shell_init_csh`].
///
/// csh cannot define functions, so the TTY / opt-out / recursion guards
/// cannot live in shell code — each alias routes through
/// `tokenade shellwrap`, which performs the same checks in-process and
/// exec-passthroughs at an interactive terminal.
pub(crate) const SHELL_INIT_CSH_FN: &str = r#"
# tokenade shell proxy (v2) — https://tokenade.net
# csh/tcsh cannot define shell functions, so each alias routes through
# `tokenade shellwrap`, which applies the guards the other shells inline:
# at an interactive terminal (stdin is a TTY), with TOKENADE_SHELL_PROXY=0,
# or under TOKENADE_WRAPPING=1 it runs the real binary untouched.
# Only binaries present on PATH at generation time are aliased.
"#;

/// Build the full csh/tcsh fragment for the PATH-present command subset.
pub(crate) fn shell_init_csh(cmds: &[String]) -> String {
    let mut out = String::from(SHELL_INIT_CSH_FN);
    if cmds.is_empty() {
        out.push_str("# no proxy-able commands found on PATH — nothing aliased.\n");
        return out;
    }
    for c in cmds {
        out.push_str(&format!("alias {c} 'tokenade shellwrap {c} \\!*'\n"));
    }
    out
}

/// Build the cmd.exe fragment (doskey macros) for the PATH-present subset.
///
/// Printed by `shell-init --shell cmd` for users who want it, but NOT
/// auto-installed: doskey macros only expand at the interactive prompt
/// (never inside `cmd /c`, which is how agents run commands), and at an
/// interactive prompt `tokenade shellwrap` passes through anyway. Agents
/// on Windows get real coverage via the PowerShell profile instead.
pub(crate) fn shell_init_cmd(cmds: &[String]) -> String {
    let mut out = String::from(
        "@echo off\r\n\
         rem tokenade shell proxy (v2) - https://tokenade.net\r\n\
         rem doskey macros expand at the INTERACTIVE prompt only; `cmd /c ...`\r\n\
         rem (how AI agents run commands) never expands them, so this fragment\r\n\
         rem exists for completeness - use the PowerShell profile for agent\r\n\
         rem coverage on Windows. `tokenade shellwrap` re-checks stdin/opt-out\r\n\
         rem itself, so these macros are always safe.\r\n\
         rem Load at startup via the AutoRun registry value:\r\n\
         rem   reg add \"HKCU\\Software\\Microsoft\\Command Processor\" /v AutoRun /t REG_SZ /d \"%USERPROFILE%\\.tokenade\\shellproxy.cmd\" /f\r\n",
    );
    if cmds.is_empty() {
        out.push_str("rem no proxy-able commands found on PATH - nothing aliased.\r\n");
        return out;
    }
    for c in cmds {
        out.push_str(&format!("doskey {c}=tokenade shellwrap {c} $*\r\n"));
    }
    out
}
