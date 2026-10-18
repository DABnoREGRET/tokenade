# Cross-platform contract

Tokenade ships as a single binary that runs on Linux, macOS, and
Windows — and gets invoked from a variety of AI coding agents, each
with their own per-OS conventions. This document is the contract we
maintain so a hand-written or AI-led change to Tokenade doesn't
silently break a non-Linux user.

## Path conventions per platform

All path resolution **must** go through `tokenade_core::platform`:

| Helper | Linux/macOS | Windows |
|---|---|---|
| `home_dir()` | `$HOME` | `%USERPROFILE%` (then `$HOME` if MSYS2) |
| `cache_dir()` | `$XDG_CACHE_HOME` ∥ `$HOME/.cache` | `%LOCALAPPDATA%` ∥ `%USERPROFILE%\AppData\Local` |
| `config_dir()` | `$XDG_CONFIG_HOME` ∥ `$HOME/.config` | `%APPDATA%` ∥ `%USERPROFILE%\AppData\Roaming` |
| `shell_command()` | `("sh", "-c")` | `("cmd", "/C")` |

NEVER:

- Hard-code `~/` in a string and `Path::new()` it — use `home_dir().join(...)` instead
- Use `/` as an explicit separator in a path **literal** (use `.join()`) — Rust's `Path` accepts `/` on Windows but the convention is a one-arg-per-segment `.join()` chain for readability and lint cleanliness
- Use `env::var("HOME")` directly — call `platform::home_dir()` so `%USERPROFILE%` is honoured on Windows
- Hard-code `/tmp/` paths — use `std::env::temp_dir()`
- Assume `sh` exists — use `platform::shell_command()` or branch on `cfg!(windows)`

## Agent config paths per OS

Each AI coding agent stores its MCP config in a different place per OS.
Tokenade's `tokenade install` writes to the right one based on the
host OS. Known conventions (verified or best-known; PRs welcome):

| Agent       | Linux                                  | macOS                                   | Windows                                          |
|-------------|----------------------------------------|------------------------------------------|--------------------------------------------------|
| Claude Code | `~/.claude.json` (+`~/.claude/settings.json`) | same                                    | `%USERPROFILE%\.claude.json` (Anthropic uses POSIX-style on Win) |
| Cursor      | `~/.cursor/mcp.json`                   | `~/.cursor/mcp.json`                    | `%APPDATA%\Cursor\User\globalStorage\mcp.json` ⚠ |
| Codex       | `~/.codex/config.json`                 | `~/.codex/config.json`                  | `%USERPROFILE%\.codex\config.json` (POSIX-style) |
| OpenCode    | `~/.config/opencode/opencode.json`     | same (XDG)                              | `%APPDATA%\opencode\opencode.json` ⚠            |
| Copilot CLI | `~/.config/github-copilot/mcp.json`    | same (XDG)                              | `%APPDATA%\GitHub Copilot CLI\mcp.json` ⚠       |
| Kilo Code   | `~/.config/kilo-code/mcp.json`         | same                                    | `%APPDATA%\Kilo Code\mcp.json` ⚠                |
| Hermes      | `~/.hermes/config.yaml`                | same                                    | `%USERPROFILE%\.hermes\config.yaml`             |

⚠ marks paths we **suspect** based on each tool's vendor conventions but
have not verified on a real Windows install. When detection is on a
platform we haven't physically tested, prefer the conservative path
(`~/` in `home_dir()` form) and **probe-with-fallback**: try each candidate
in order, use whichever exists; if none, create at the most-likely path
and log a warning the user can correct.

## Shell semantics for hooks

`build_hook_command` (in `tokenade-cli/src/main.rs`) emits the literal
shell snippet that Claude Code stores in `settings.json` and runs on
each hook event. The snippet must be valid in the host shell:

| host | wrapper used | reason |
|---|---|---|
| Linux/macOS | `sh -c 'TOKENADE_DEBUG_LOG=<p> exec <exe> <sub>'` | POSIX-portable, `exec` flattens the process tree |
| Windows    | `cmd /C "set TOKENADE_DEBUG_LOG=<p>&& <exe> <sub>"` | cmd.exe idiom; usable from PowerShell-launched parents too |

If Claude Code on Windows ever delegates to PowerShell instead of
cmd.exe, we'll also need a PowerShell variant. The chosen wrapper is
detected at INSTALL time via `cfg!(windows)`, so a binary built on
Linux that's later run on Windows would emit the Linux wrapper. The
recommended distribution path is one binary per OS; the CI matrix
already builds for all three.

## Per-agent message surfacing

Different AI agents show Tokenade's output via different channels.
This affects how we should signal long-running operations (indexing,
model downloads) and savings annotations.

| Agent       | MCP `_meta` shown to user? | Hook `additionalContext` shown to user? | Stderr of MCP server shown to user? |
|-------------|---------------------------:|----------------------------------------:|-------------------------------------:|
| Claude Code | no (agent-only)            | no, but agent sees it as system reminder | no (debug log only) |
| Cursor      | no (agent-only)            | n/a (no hook framework)                  | no |
| Codex       | no (agent-only)            | n/a                                      | sometimes visible in terminal |
| OpenCode    | unknown                    | n/a                                      | unknown |
| Copilot CLI | unknown                    | n/a                                      | unknown |

**Conclusion**: Tokenade should never assume the user sees any of its
direct output. The signal the user always sees is the **tool-call
duration in the agent's spinner / status bar** (codegraph relies on
this exclusively). Status `eprintln!` calls in Tokenade must go through
`tokenade_core::progress!` which TTY-gates: visible when the user runs
`tokenade ask` themselves in a shell, silent when an agent invokes the
binary over a pipe. The agent never wastes context tokens on our chatter.

## Compliance audit (as of last edit)

| Surface | Status |
|---|---|
| `platform::home_dir / cache_dir / config_dir / shell_command` | ✅ cross-platform |
| `progress!` macro TTY-gates all status messages | ✅ |
| `build_hook_command` cross-platform | ✅ (`cfg!(windows)` branch) |
| Agent MCP config paths cross-OS | 🔶 Linux-correct; Windows paths unverified |
| Tests run on Windows CI | ❌ not yet — current CI is Linux-only |
| Probe-with-fallback for unknown agent paths | ❌ not yet implemented |

Items marked ❌ / 🔶 are tracked in the project task list. Don't add
new code that violates the cross-platform contract without raising the
corresponding fix.
