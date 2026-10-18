# Zero-context agent install contract

Tokenade is meant to be installed by **an AI coding agent that has never
heard of Tokenade before**. The typical sequence:

1. User points their Claude Code / Codex / Cursor / Copilot at
   `https://tokenade.net`.
2. The agent fetches the install instructions and runs them.
3. Tokenade is now installed and working — without the agent ever having
   needed prior context about what Tokenade is, where files live, or what
   the user should do next.

This document is the contract our install command must honour so step 3
actually works on a context-free machine.

## The five rules

### Rule 1 — Never write to user-owned global config; emit instructions instead

The install must NEVER silently write to:

- `~/.claude/CLAUDE.md` — the user's global Claude Code rules
- `~/.codex/config` (the non-MCP keys) — user preferences
- `~/.bashrc`, `~/.zshrc`, `~/.profile`, `~/.config/fish/config.fish`
- `~/.gitconfig`
- Any other dotfile that is the user's personal property

But sometimes we genuinely WANT one of these mutated — e.g. adding a
"prefer Tokenade tools" block to `~/.claude/CLAUDE.md` is high-value for
the user's future sessions. The right pattern is **agent-mediated**:

1. The install command's output includes a verbatim instruction block
   like *"To make future Claude Code sessions prefer Tokenade tools,
   add the following to `~/.claude/CLAUDE.md`. Ask the user first."*
2. The exact text to paste is delimited with clear fences so a
   context-free AI agent can copy it byte-for-byte.
3. The AI agent installing Tokenade reads that instruction and asks
   the user for permission before editing.
4. The USER consents (or not). Tokenade itself never made the write.

This keeps consent in the user's hands, makes installation reversible
(the user can see what's about to change), and lets an AI agent handle
the mechanics without prior Tokenade context.

What we ARE allowed to touch directly (because these files are MCP/hook
plumbing that tools are expected to register into).

Note (2026-08-04): the `mcpServers.tokenade` permission below is now DORMANT —
tokenade is hook-first and no longer registers a server of its own. `healthcheck`
checks for the ABSENCE of that entry. The permission is kept because the same
files are still written for the MCP **proxy** (wrapping third-party servers):

- `~/.claude.json` — but ONLY the `mcpServers.tokenade` key, and we
  preserve every other key including unrelated MCP servers
- `~/.claude/settings.json` — but ONLY the `hooks.PostToolUse[]` and
  `hooks.PreToolUse[]` arrays, tagged with `_tag: "tokenade"` so
  re-running install is idempotent and uninstall is surgical
- `~/.cursor/mcp.json`, `~/.codex/config.json`, etc. — same rule:
  add/update our entry, preserve everything else
- Tool-owned dirs we create: `~/.tokenade/`, `<cwd>/.tokenade/`,
  `<cwd>/.claude/{QUICK_START,ARCHITECTURE_MAP,COMMON_MISTAKES}.md`
  (only when explicitly requested via `--with-claude-md`, AND only
  inside the cwd — never in `~/.claude/`)

What we ARE allowed to touch (because these files are MCP/hook plumbing
that tools are expected to register into):

- `~/.claude.json` — but ONLY the `mcpServers.tokenade` key, and we
  preserve every other key including unrelated MCP servers
- `~/.claude/settings.json` — but ONLY the `hooks.PostToolUse[]` and
  `hooks.PreToolUse[]` arrays, tagged with `_tag: "tokenade"` so
  re-running install is idempotent and uninstall is surgical
- `~/.cursor/mcp.json`, `~/.codex/config.json`, etc. — same rule:
  add/update our entry, preserve everything else
- Tool-owned dirs we create: `~/.tokenade/`, `<cwd>/.tokenade/`,
  `<cwd>/.claude/{QUICK_START,ARCHITECTURE_MAP,COMMON_MISTAKES}.md`
  (only when explicitly requested via `--with-claude-md`)

When in doubt: don't write. Print a one-line suggestion the agent can
relay to the user instead.

### Rule 2 — Output must be parseable by a context-free agent

Every install / detect / uninstall command's stdout has three parts in
this order:

```
HUMAN HEADER:           one line, the "what" of the run
STRUCTURED ACTIONS:     one bullet per action taken or proposed
AGENT NEXT-STEPS:       a NEXT STEPS: block that tells the agent what to
                        relay to the user verbatim
```

The agent reading this stream should be able to faithfully tell the user
"X was done, restart your editor, and your savings will appear in
`tokenade dashboard`" without ever having to make up information.

### Rule 3 — Every destructive action requires explicit consent

- `--dry-run` (default for the cleanup phase if `--yes` is not passed)
  prints what would happen, changes nothing.
- `--yes` / `-y` opts the calling agent into running everything that
  was previewed. The agent is responsible for passing `--yes` only
  after the user agreed.
- `sudo`-requiring actions detect non-tty and emit the literal command
  the user should run themselves, instead of failing opaquely.
- Backups (`<path>.tokenade-backup-<ts>`) are taken before any in-place edit
  to a config file we share with the agent.

### Rule 4 — The CLI is its own documentation

A zero-context agent should be able to run `tokenade help-agent` and get
a structured guide that tells it:

- Every command the CLI exposes (with one-line summaries)
- Where data is persisted on disk
- How to enable / disable the hook
- How to read the debug log
- How to uninstall cleanly

We never assume the agent has read our README, our website, or any
external doc. If a behaviour matters, it must be discoverable through
the CLI itself.

### Rule 5 — Installs are idempotent and reversible

- Running `tokenade install` twice is safe (no duplicate MCP entries,
  no duplicate hook groups — we tag with `_tag: "tokenade"` and
  upsert).
- Running `tokenade uninstall` removes every trace we created and
  restores the user's configs to a state equivalent to never having
  installed.
- Migration data (memory imported from `claude-mem`, `agentmemory`,
  `context-mode`) STAYS — it's the user's, not ours. Uninstall doesn't
  touch the migrated memory.

## Compliance audit (re-checked against the binary — 2026-08-04)

| Rule | Compliant? | Notes |
|------|---|---|
| 1 — No user-owned global writes | ✅ since the manual `~/.claude/CLAUDE.md` edit was reverted | The install command itself never touches it; the human operator did. |
| 2 — Parseable output | 🔶 partial | We have action lines but not yet a structured NEXT STEPS: block. |
| 3 — Explicit consent | ✅ | `--dry-run` is the default for cleanup; `--yes` is required for destructive ops. |
| 4 — CLI as documentation | ❌ | No `tokenade help-agent`. Still true — the command does not exist. |
| 5 — Idempotent + reversible | ✅ | Idempotent, and `tokenade uninstall` ships: `--dry-run`, `--keep-data`, `--headless`, restores wrapped MCP entries, reports the disk each removal gives back, and returns a non-zero code on a partial failure. |

The remaining ❌ / 🔶 items are tracked in the project task list.

A scorecard is only worth its date. This one said "Uninstall command not yet
built" for the two months AFTER it shipped — a status table that undersells the
product is the same defect as one that oversells it, and it is the reason the
heading now names the day the rows were re-checked against the binary rather
than the day they were written.
