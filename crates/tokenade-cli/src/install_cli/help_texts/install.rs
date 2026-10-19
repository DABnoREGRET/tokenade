//! **Help for the commands that SET or REMOVE something**:
//! `install`, `uninstall`, `unwrap-mcps`, `evict-stale`, `detect`,
//! `competitors`, `style`.
//!
//! Those are read by a human, once, before an action that touches their
//! machine. They may be long — and they must be wherever the action is hard
//! to undo.

use super::*;

/// #534 — dedicated help for `tokenade unwrap-mcps --help`. Without
/// this the generic fallback hides what the command does and the
/// `uninstall` / `unwrap-mcps` distinction (the latter is invokable
/// on its own when the user wants to test config changes without
/// running the full uninstall flow).
pub(crate) fn unwrap_mcps_help_text() -> &'static str {
    "tokenade unwrap-mcps — restore wrapped third-party MCP entries to original form.

USAGE:
  tokenade unwrap-mcps [--dry-run]

What this command does:
  * Walks every known agent config (Claude Code, Cursor, Codex,
    Kilo, Copilot, …) and rewrites every `tokenade mcp-wrap`-
    wrapped MCP entry back to its original `command` + `args`.
  * Idempotent — re-running after a clean unwrap is a no-op.
  * Does NOT remove the tokenade self-entry, hooks, or binary.

When to use this vs `tokenade uninstall`:
  * `unwrap-mcps` — you want to TEST what unwrapping looks like,
    or stop proxying third-party MCPs through tokenade while
    keeping the tokenade tooling itself installed. Leaves the
    tokenade self-entry, hooks, and CLAUDE.md block in place.
  * `uninstall` — runs this same unwrap pass FIRST, then also
    strips the tokenade self-entry from every agent config
    (#611), removes every `_tag: \"tokenade*\"` hook group from
    `~/.claude/settings.json` (#612), and strips the fenced
    block from `~/.claude/CLAUDE.md` (#552). Use this when
    removing for good; chain a package-manager uninstall after.

FLAGS:
  --help, -h          show this message and exit
  --dry-run           print which configs WOULD be restored, change
                      nothing. Honoured since #434 (was silently
                      ignored before).
"
}

/// #533 — dedicated help for `tokenade uninstall --help`. The bare
/// command's behaviour is surprising: it does NOT remove the binary
/// or hooks, only restores wrapped third-party MCP entries. Without
/// this text users discover the asymmetry the hard way.
pub(crate) fn uninstall_help_text() -> &'static str {
    "tokenade uninstall — full removal: undo the integration, wipe tokenade's
data, and remove or guide removal of the binary.

USAGE:
  tokenade uninstall [aliases] [--dry-run] [--headless] [--keep-data]

MODES:
  tokenade uninstall            full uninstall (steps 1-7 below)
  tokenade uninstall aliases    remove ONLY the shell proxy (step 5):
                                alias/eval lines in every rc file plus
                                ~/.tokenade/shellproxy.* fragments.
                                MCP wiring, hooks, CLAUDE.md and your
                                data stay. Aliases: alias, shell-proxy,
                                shellproxy.

What the full command does:
  1. Walks every known agent config (Claude Code, Cursor, Codex,
     Kilo, Copilot, …) and restores every `tokenade mcp-wrap`-
     wrapped MCP entry to its original `command` + `args`. Runs
     FIRST so a crashed uninstall still leaves working MCPs.
  2. Strips the fenced `<!-- tokenade:start --> … <!-- tokenade:end
     -->` block from `~/.claude/CLAUDE.md` (backup taken first).
  3. Removes the `tokenade` self-entry from `mcpServers` / `mcp` in
     every detected agent config.
  4. Strips every `_tag: \"tokenade*\"` hook group from
     `~/.claude/settings.json` (and the same for other agents).
  5. Removes the shell proxy / PATH shim from every rc file
     (.bashrc, .zshenv, .zshrc, config.fish, PowerShell, .cshrc, …)
     and deletes the ~/.tokenade/shellproxy.* fragments.
  6. DELETES tokenade's own data: `~/.tokenade/` (gain ledger,
     dedup.db, index.db, semantic.db, debug.log,
     install sentinels) AND the stash parent `~/.cache/tokenade/`
     (disclosure stash, model, fastembed cache). ONLY these two
     tokenade-owned trees are removed — nothing else in HOME or the
     cache dir is touched, and symlinks are never followed off-target.
     Suppress this with `--keep-data`.
  7. Handles the BINARY based on how it was installed:
       npm      → prints `npm uninstall -g @tokenade/cli` (npm owns it)
       cargo    → prints `cargo uninstall tokenade-cli`
       brew     → prints `brew uninstall tokenade`
       standalone (curl / ~/.local/bin / /usr/local/bin) → DELETES the
                  binary directly on Unix; on Windows a running .exe
                  can't self-delete, so it prints the `del` command.

  Idempotent — re-running after a clean uninstall is a no-op.

FLAGS:
  --help, -h          show this message and exit
  --dry-run           list every config, data dir and binary that
                      WOULD be removed; change nothing.
  --keep-data         do the integration + binary steps but KEEP
                      ~/.tokenade and ~/.cache/tokenade (preserves your
                      savings ledger).
  --headless          non-interactive: run the integration + data wipe
                      with no banner and no next-step guidance, and
                      never touch the binary (used by the npm
                      `preuninstall` hook — npm removes the binary).

See also:
  * `tokenade unwrap-mcps` — same restore pass without the
    uninstall-flavoured banner / guidance.
  * `tokenade scrub-cache` — purge persisted secrets from caches
    before removal if the install ever logged credentials.
"
}

pub(crate) fn detect_help_text() -> &'static str {
    "tokenade detect — which agent is invoking tokenade, and through which channel.

USAGE:
  tokenade detect [--json]

Identifies the agent the ENVIRONMENT says is running (authoritative when
tokenade is called by an agent), and falls back to filesystem
fingerprints under the project root (.claude/, .cursor/, .codex/,
AGENTS.md, …) when the environment says nothing.

Prints:
  * the canonical agent name (`claude-code`, `cursor`, `codex`, … or
    `unknown`);
  * `coverage:` — the CHANNEL that agent type uses (hooks, plugin
    bridge, MCP-wrappable, …) and what it buys;
  * `mcp config hint:` — where that agent keeps its MCP config.

  --json   one object on stdout instead of two lines

THE COVERAGE LINE IS ABOUT THE AGENT TYPE, NOT YOUR MACHINE. It says
`hooks` for a hook-covered agent even when no hook is installed here —
it answers \"which channel would this agent use\", not \"is it wired up\".
To learn whether tokenade is actually INSTALLED for you, run
`tokenade health`: it checks the agent's config and names the fix when
something is missing."
}

/// #628 — dedicated help for `tokenade competitors` (alias of
/// `detect-competitors` / `scan`). Documents the four classification
/// badges so users can interpret `[ANTI-PATTERN]` / `[drop-in better]`
/// at a glance, and the `--json` flag.
pub(crate) fn competitors_help_text() -> &'static str {
    "tokenade incompatibilities — check for tools that conflict with Tokenade and propose fixes.

USAGE:
  tokenade incompatibilities [--json]
  tokenade incompat                # alias
  tokenade competitors             # alias
  tokenade detect-competitors      # alias
  tokenade scan                    # alias

Walks the filesystem + process tree looking for other token-tooling
fingerprints (config files, installed binaries, running daemons) that
can overlap or conflict with Tokenade. Each match is flagged with a
suggested fix:

  [redundant]      Tokenade already does this job — safe to remove.
  [overlapping]    Tokenade supersedes it; migrate your data first, then remove.
  [wasteful]       Known to inflate token usage — we recommend disabling it.
  [compatible]     Complementary, not a conflict; leave it installed.

The text path prints one block per match (name, one-liner, signals,
suggested fix) and points at `tokenade install` for an interactive
fix-up (runs by default; suppress with `--no-cleanup`).

Flags:
  --json   emit a structured JSON array of `{name, class, one_liner,
           rationale, signals[]}` records. Empty array `[]` when
           nothing is detected. Mirrors the `--json` flags on
           healthcheck/gain/dashboard/discover/measure.

Notes:
  * Read-only — no installs, no removals (those go through
    `tokenade install` with the default fix-up pass on).
  * Cheap: filesystem walks are bounded; no DB writes.
"
}

pub(crate) fn style_help_text() -> &'static str {
    "tokenade style — the short note added ahead of your prompt so the agent
answers without greetings, recaps or narration.

USAGE:
  tokenade style                         # show the current setting
  tokenade style <auto|chat|coding|off>

  auto     default. Picks chat or coding per prompt from what you typed
           (English and French keywords); no clear signal, no note.
  chat     concise answers: no greetings, no recap, no narration, no
           closing summary.
  coding   one ASCII status line per tool call (verb · target · outcome),
           identical findings grouped with (×N).
  off      never touch the agent's voice (e.g. for benchmarking).

Notes:
  * Global: stored in ~/.tokenade/style.json. A project's
    .tokenade/style.json left by an older version is moved there.
  * Claude Code gets the note from its UserPromptSubmit hook on the first
    turn, when the style changes and after a compaction; agents without a
    prompt hook get it from their rules file or the LLM proxy, never both.
  * Takes effect on the next prompt; nothing needs restarting.
"
}

pub(crate) fn install_help_text() -> &'static str {
    "tokenade install — register tokenade with detected AI coding agents.

USAGE:
  tokenade install [flags]

Defaults (zero-config optimal install):
  * Detects installed agents (claude-code, cursor, codex, kilo-code,
    copilot, openclaw, …) and wires tokenade into each one's hook or
    plugin mechanism.
  * Installs PostToolUse + PreToolUse hooks for Claude Code (compaction,
    LSP redirects, search-tool nudges).
  * Writes a per-project CLAUDE.md scaffold teaching the tokenade CLI.
  * Global install (writes to ~/.<agent>/config) unless --per-project.
  * Enables ~/.tokenade/debug.log for live tailing.

FLAGS:
  --help, -h               show this message and exit
  --yes, -y                assume yes for destructive cleanup prompts
  --dry-run                print what would be done, change nothing
  --only <agent>           restrict install to a single agent
                           (claude-code, cursor, codex, kilo-code, copilot, droid, …)
  --per-project            install into ./.<agent>/ instead of global
  --profile <name>         use a domain-tailored CLAUDE.md template
  --no-cleanup             skip the incompatible-tools fix-up pass
  --no-debug-log           don't enable ~/.tokenade/debug.log
  --debug-log <path>       custom debug.log location
  --autocompact-pct <N>    ask the agent to auto-compact at N% of its context
                           window (10-95, default 70). Re-run with a different
                           N to change it; upgrades keep the N you chose. A
                           value you edit by hand is never overwritten — this
                           flag is how you hand the threshold back to tokenade
  --no-autocompact         remove the auto-compact threshold tokenade set
                           (a hand-set value is left alone)
  --no-hook                skip the Claude Code PostToolUse + Edit/Write
                           auto-index hooks (MCP only — power-user opt-out)
  --no-wrap-mcps           skip the auto-wrap of third-party MCP servers
                           behind `tokenade mcp-wrap` (opt-out of the
                           per-MCP compaction/stash pipeline)
  --with-hook              (legacy) opt-in for the PostToolUse hook
                           — now the default; flag retained for back-compat
  --with-claude-md         add a framework-aware CLAUDE.md (Next.js /
                           FastAPI / Rust / …) — opt-in; the minimal
                           tokenade-rules block is scaffolded either way
  --with-lsp-enforcement-hook  DEPRECATED no-op (still accepted so old
                           scripts don't error). The PreToolUse hook it
                           used to gate is now installed by default.
  --with-brevity-skill     terse-response style hint in CLAUDE.md — opt-in
                           (can degrade reasoning on complex tasks)
  --defaults               forget the choices remembered from earlier
                           installs. Otherwise --no-hook, --only, --per-project,
                           --no-debug-log, --debug-log, --no-autocompact,
                           --no-wrap-mcps, --profile, --with-* are kept by
                           every later install, upgrade and self-repair

EXAMPLES:
  tokenade install                       # everything, all detected agents
  tokenade install --only claude-code    # just Claude Code
  tokenade install --dry-run             # preview changes
  tokenade install --per-project         # ./.claude/ instead of ~/.claude/

See `tokenade help` for the top-level command list."
}
