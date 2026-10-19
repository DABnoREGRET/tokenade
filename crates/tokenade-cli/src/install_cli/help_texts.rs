//! The `--help` text of every command that has one.
//!
//! Prose, not logic: they were interleaved with the installer's code, where a
//! 60-line string constant is 60 lines you scroll past to reach the next
//! function. `help_routing` picks which one to print.

mod explore;
mod install;
pub(crate) use explore::*;
pub(crate) use install::*;

use super::*;

/// #532 — dedicated help for `tokenade evict-stale --help`. Without
/// this the generic fallback prints the top-level command list, which
/// tells the user nothing about a destructive command that prunes
/// five SQLite stores + VACUUMs them + edits a registry file.
pub(crate) fn evict_stale_help_text() -> &'static str {
    "tokenade evict-stale — opportunistic cleanup of stale cache rows + reclaim disk.

USAGE:
  tokenade evict-stale [--models]

Sweeps every store tokenade owns, all bounded by a 7-day TTL on the
row's last-access / last-write timestamp:

  dedup_ref         drops content-addressable §ref:HASH§ rows not
                    used by any session in the last 7 days
  file_search       drops file-content chunks past the TTL
  fold_store        drops fold/templated-line records past the TTL
  semantic_db       drops cold per-project embedding indexes + any
                    zero-file orphan project rows (#446)
  index_db          drops cold per-project symbol indexes + any
                    zero-symbol orphan project rows (#447)
  disclose stash    drops stashed blobs past the TTL — these are the
                    bytes `tokenade expand-ref <hash>` hands back, so
                    a hash older than the TTL stops being expandable
                    and the command has to be re-run to get them
                    again. Also evicted oldest-first when the stash
                    dir passes its size cap (2 GiB, override with
                    TOKENADE_STASH_MAX_DIR_BYTES).
  sessions          drops per-session state files past the TTL — a
                    session that is still running keeps rewriting its
                    file, so an active one is not stale
  transcript cache  drops the parsed-transcript rows behind
                    `tokenade dashboard` for sessions or projects
                    that are gone; the dashboard re-reads what it
                    needs on its next run
  image reads       drops downscaled and full-res image copies past
                    the TTL, for the current project

Then prunes `~/.tokenade/projects-registry.txt` — append-only file
of every project tokenade has scaffolded; lines whose path no longer
exists on disk are removed (#448).

Also prunes `~/.tokenade/bypass-counter` when its 24h window has
expired (#859) — keeps the healthcheck `bypass_count` entry
honest and lets the next bypass start a fresh window instead of
incrementing a frozen one.

Finally, VACUUMs each SQLite DB so freed pages return to the OS
(#459 — first run on a busy install typically reclaims 10-20 MB).

Cheap when there's nothing to do. Safe to re-run any time; the next
agent session that touches a project will re-populate its row.

Notes:
  * The 7-day TTL is hard-coded (see dedup_ref CACHE_TTL_SECS); dedup_ref is
    also capped at 150 MiB (TOKENADE_DEDUP_MAX_BYTES).
  * --models  also delete the local model weights, which are on NO TTL
    and are by far the largest thing tokenade holds (19.9 GB of 20.6 GB
    on the machine this was measured on). Only local agents use them and
    they re-download on demand. Without this flag a default run only
    reports how much is held.
  * Also drops `gain.jsonl` rows with zero measured savings outside
    the code-intel path (#499) — that's measurement noise, not data.
  * Run after a `tokenade scrub-cache` if you want both the redact
    sweep and the disk reclaim in one go.
"
}

/// #508 — dedicated help for `tokenade dashboard` / `tokenade dashboard --help`.
/// Surfaces the two output modes (terminal vs HTML) and what each
/// section means, so the user doesn't have to dig in source / FAQ.
pub(crate) fn dashboard_help_text() -> &'static str {
    "tokenade dashboard — token-savings + agent-activity report.

USAGE:
  tokenade dashboard [--text | --no-open | --html [<path>] | --json | --watch [<secs>]]

Default (no flags): writes the interactive web dashboard to
~/.tokenade/dashboard.html and opens it in your browser. Nothing else is
printed — the page IS the dashboard. The terminal report is the fallback,
and appears only when the page cannot be put in front of you: a GUI-less
session (SSH, or no X11/Wayland display), or a browser launcher that
refused. `--text` asks for it on purpose.

The page is a static local file: a period selector that drives EVERY panel
(hero, KPIs, chart, breakdown tables), click-to-drill detail per period
(functions, layers, agents, projects, models), a with/without-tokenade
comparison chart, and an 'outside tokenade' tab. 100% local — no server, no
requests; your stats never leave the machine.

Terminal sections (--text, and the headless fallback):

  HERO              total tokens saved + percent + ops over all time.
  TOP COMPACTORS    per-op breakdown of measured savings (auto:json,
                    cargo-test, grep, …). Bar width = relative saved.
  BY LAYER          the same measured savings bucketed by logical layer
                    (shell, file-read, content-auto, web, wrapped-mcp)
                    so you see where the savings come from at a glance.
  WRAPPED MCPs      separate breakdown for third-party MCP servers
                    proxied via `tokenade install` (only shows when
                    such traffic exists).
  PROJECTS          per-project totals; rows sorted by saved bytes.
  AGENT TOKENS      what your agent consumed OUTSIDE Tokenade, read
                    from its local transcripts.
  CODE INTELLIGENCE heuristic estimates for symbol_find / semantic_search
                    / skeleton / structure_map / impact_of / call_hierarchy
                    (their savings can't be measured directly; the
                    multiplier is a per-tool heuristic, not a benchmark).
  INDEX & CACHE     dedup cache hit-rate + semantic index size.

Flags:
  --text            print the terminal report instead of opening the
                    page. --days N (default 30) windows the text report
                    (--text, --watch, and the fallback when no browser
                    opens): the page has its own period selector, so
                    --days is refused on the default run.
  --no-open         skip launching the browser; still writes the web
                    dashboard file and prints its path. Prints no report.
  --html [<path>]   write ONLY the web dashboard file (no terminal
                    report, no browser launch). With a path, write
                    there (mkdir -p the parent if needed); without one,
                    write to ~/.tokenade/dashboard.html. Useful for
                    scripts or for sharing the file.
  --json            emit a structured JSON object instead (CI / scripts).
                    Fields: operations, tokens_before/after/saved,
                    savings_pct, cache_hits/misses/hit_rate, by_layer[],
                    per_op[]. Mirrors --json on `healthcheck` and `gain`.
  --watch [<secs>]  live mode: re-render the text report every <secs>
                    seconds (default 2, clamped 1–3600), clearing the
                    screen each pass. Leave it open while an agent works
                    to watch savings tick up. Ctrl-C to exit. Takes
                    precedence over --html / --json (those are one-shot).

Notes:
  * Reads from ~/.tokenade/gain.jsonl (global) — every project's
    activity in one view.
  * Cheap: no compactor runs, no network, no DB writes (other than
    statusline-cache touches).
  * To prune historic noise (zero-saved rows, stale binary leaks),
    run `tokenade evict-stale` first.
"
}

/// #618 — dedicated help for `tokenade healthcheck`. Documents the
/// `--json` flag (#613) so CI consumers know how to script against
/// it, and the check categories so users know what's actually being
/// probed without having to read source.
pub(crate) fn healthcheck_help_text() -> &'static str {
    "tokenade healthcheck — verify the install is wired correctly.

USAGE:
  tokenade healthcheck [--json]
  tokenade health
  tokenade doctor

What it checks (one row per check):
  binary           tokenade binary path + version
  gain_ledger      ~/.tokenade/gain.jsonl writable
  agent_detect     which AI agent is configured for this cwd
  hook_installed   `_tag: tokenade*` hook in ~/.claude/settings.json
  mcp_entry        that NO stale `tokenade` entry is left in an agent's
                   `mcpServers` — tokenade is hook-first; an old entry
                   there points at a server it no longer runs
  wrapped_mcps     third-party MCP entries point to the current binary
  semantic_model   potion-code cached (it serves BOTH embed and rerank;
                   the HuggingFace e5-small + bge-reranker pair it
                   replaced is gone)
  stale_mcp_procs  running `tokenade mcp` processes against an unlinked binary
  claude-workspace-trust  folder trust prompt not accepted ⇒ Claude Code
                   runs no hooks here
  codex-hook-trust Codex hooks not approved (the LLM proxy covers them)
  hook-integrity   no tampered tokenade hook command in Claude settings
  project-filters  this repo's .tokenade/filters and whether they are trusted
  filter-syntax    a custom TOML filter that does not parse (fail)
  disk-footprint   space used by tokenade's caches
  …and a few platform rows (e.g. posix-shell).

Flags:
  --json   emit a structured JSON object (version, failures, ok, checks[])
           instead of the human report. Each check has a stable `id` so
           CI scripts can branch on specific failures (e.g. `.checks[]
           | select(.id == \"mcp_entry\")`).

Exit codes:
  0   all OK (some checks may surface info-only `--` rows)
  1   one or more `fail` rows
"
}

/// #831 — help for `tokenade audit-claude-md`. Explains the cascade
/// model and what the duplicate report means.
pub(crate) fn audit_claude_md_help_text() -> &'static str {
    "tokenade audit-claude-md — audit the always-resident CLAUDE.md cascade.

USAGE:
  tokenade audit-claude-md [DIR] [--json] [--apply | --compress | --stale]
  tokenade claudemd

Agent CLIs load a cascade of CLAUDE.md files into the system-prompt
prefix on every session: the global ~/.claude/CLAUDE.md plus a CLAUDE.md
(and .claude/CLAUDE.md) in every directory from the repo root down to
the working directory. That prefix is paid on every cache write and
re-sent on every cache miss, so duplicated boilerplate copy-pasted into
several files quietly inflates the resident context.

This reports, deterministically (no LLM):
  - per-file token cost and the total always-resident prefix size
  - lines duplicated ACROSS files, with the tokens you'd recover by
    hoisting each one into a single place

Arguments:
  DIR      starting directory for the cascade walk (default: cwd)

Flags:
  --json   emit a structured JSON object (total_tokens, duplicated_tokens,
           files[], duplicates[]) instead of the human report.
  --apply     REWRITES the files: removes cross-file duplicates (exact +
              semantic).
  --compress  REWRITES the files: whitespace compaction.
  --stale     list references to paths that no longer exist.

Note: this attacks the cacheable prefix, so the realized saving on a warm
session is smaller than the raw number — but the duplication is pure
waste on every cache write and miss, and trimming it never hurts. It is
complementary to tokenade's output-side compaction.
"
}

/// #618 — dedicated help for `tokenade gain`. Lists the three real
/// flags the command accepts (none were documented before — `--json`,
/// `--history`, `--by-source`) so users don't have to grep source to
/// find them.
pub(crate) fn gain_help_text() -> &'static str {
    "tokenade gain — token-savings summary from ~/.tokenade/gain.jsonl.

USAGE:
  tokenade gain [--json | --history] [--by-source] [--recent]

Default (no flags): renders a per-op (compactor) totals table:

    operations: 1913
    tokens: 24644722 → 393911
    saved:   24250811 (98.4%)
    by op:
      auto:json   142×  −89%
      cargo:test   53×  −76%
      ...

Flags:
  --json         emit a structured JSON object instead of the human
                 text. Top-level: version, operations, before, after,
                 saved, percent. Nested arrays: by_op[] and by_source[]
                 with per-row {before, after, count, saved, percent}.
                 Cannot be combined with --history (refused).
  --history      print one row per ledger entry (raw before/after/pct/label)
                 in addition to the summary. Useful for spot-checking.
  --recent       only the last 7 days of the ledger.
  --by-source    add a `by source:` section after `by op:` showing the
                 split between cli / hook / mcp / proxy sources.

Notes:
  * Reads from ~/.tokenade/gain.jsonl (the global ledger every project
    writes to). The dashboard uses the same file.
  * `tokenade evict-stale` prunes 7-day-stale rows + saved<10/pct<5%
    noise — useful before a hand-off report.
"
}

/// #627 — dedicated help for `tokenade discover`. Documents both
/// the `--days` window flag and the `--json` flag added in #619.
pub(crate) fn discover_help_text() -> &'static str {
    "tokenade discover — scan recent Claude sessions for install opportunities.

USAGE:
  tokenade discover [--days N] [--json]

Walks ~/.claude/projects/* (last N days, default 30) and tallies tool
traffic by name. Surfaces:
  * Top 15 tools by total bytes (markdown table)
  * Whether the Claude Code Bash hook is already wired
  * Up to 5 'opportunity' tools — heavy-traffic non-tokenade tools
    that installing the hook would compress (Bash/Read/Grep/Glob/
    WebFetch + any `mcp__*` server).

Flags:
  --days N   how many days of session history to scan (default: 30)
  --json     emit a structured JSON object instead of the markdown
             report. Fields: sessions_scanned, days, logs_dir,
             hook_installed, top_tools[], opportunities[].

Notes:
  * The byte counts ALREADY include tokenade's compaction if the
    hook is installed — that's why the text path picks a different
    suggestion block in that case.
  * Cheap: one pass through session JSONL, no DB writes.
"
}

/// #627 — dedicated help for `tokenade measure`. Documents the
/// `--json` flag (#620) and the 800-token target.
pub(crate) fn measure_help_text() -> &'static str {
    "tokenade measure — agent-loaded-file startup-token cost.

USAGE:
  tokenade measure [--json]

Sums the tokens of every file your AI agent auto-loads at session
start (CLAUDE.md, AGENTS.md, GEMINI.md, .cursorrules, .windsurfrules,
.clinerules, .github/copilot-instructions.md, plus any *.md in .claude/). The 800-token target is the published `nadimtuhin` baseline
for a frugal startup.

When over budget the text path estimates monthly USD cost at ~50
sessions/month on the default model + emits a context-aware fix
suggestion: prune ABOVE the scaffold marker if already installed,
or run `tokenade install --with-claude-md` if not.

Flags:
  --json   emit a structured JSON object instead of the human report.
           Fields: root, framework, files[], total_tokens,
           target_tokens, waste_tokens, waste_pct,
           monthly_usd_at_50_sessions, over_budget, already_scaffolded.
           `over_budget` + `already_scaffolded` are one-field branches
           for CI / pre-commit hooks.

Notes:
  * No writes, no network.
  * `framework` reflects the same detector tokenade install uses
    when scaffolding (rust, next, django, fastapi, …).
"
}

/// #627 — dedicated help for `tokenade semantic-status`. Documents
/// `--json` (#624) + the index source.
pub(crate) fn semantic_status_help_text() -> &'static str {
    "tokenade semantic-status — per-project state of ~/.tokenade/semantic.db.

USAGE:
  tokenade semantic-status [--json]

Lists every project whose embeddings have been indexed by
`tokenade index` / opportunistic semantic_search ingestion, with:
  * project path
  * file count (== doc count under the current 1-doc-per-file chunking)
  * size on disk
  * last-indexed timestamp

Flags:
  --json   structured JSON. Top-level: project_count, total_files,
           total_docs, total_size_bytes. Per-row: path (FULL — text
           mode truncates to 46 chars with `…`), files, docs,
           size_bytes, last_indexed_unix (raw seconds for CI
           threshold math), last_indexed_iso (pre-formatted).

Notes:
  * Read-only — does not trigger re-indexing.
  * `tokenade evict-stale` drops 7-day-stale rows + VACUUMs.
"
}

/// #627 — dedicated help for `tokenade detect`. Tiny but explains
/// the stdout/stderr split + the new --json shape.
pub(crate) fn auto_help_text() -> &'static str {
    "tokenade read — detect input format + dispatch to the right compactor.

USAGE:
  curl -s api/x | tokenade read -
  tokenade read blob.txt
  tokenade read src/main.rs        # source code: see SOURCE CODE below

FLAGS:
  --prompt \"q1, q2\"   return only the passages that answer, several
                      questions in ONE call (also --question, -q)
  --lines A-B         read only that region, 1-based and inclusive.
                      `A-` runs to the end, `-B` from the start, `A`
                      is one line. The answer opens with
                      `[tokenade: lines A-B of N]`, so a big file can
                      be read piece by piece without guessing where
                      the last piece stopped. A range past the end is
                      refused, not answered with the last line
  --cmd <command>     name the command that produced this text, so a
                      per-command compactor can be picked
  --frames [N]        video only: add deduplicated still frames

This is the catch-all entry point used by the PostToolUse Bash hook
and the MCP auto_compact tool. When the path is on argv, the file's
extension feeds the detector first (so a .rs file is treated as
source code even if the byte content looks SERP-ish, #570/#571).

SOURCE CODE:
  A file you are likely to edit (inside a git work tree, under ~20k
  tokens) comes back unchanged. Other source files over 3 KB come back
  with comments stripped, under a `mode=comments` banner.
  TOKENADE_NO_CODE_STRIP=1 disables the stripping.

Detection priority (first match wins):
  * source-code extension → source code (see SOURCE CODE above)
  * JSON / NDJSON / YAML / TOML / Markdown / CSV / TSV / XML
  * `git log --stat` → gitlog
  * `git diff` / `diff -u` → unified diff
  * curl -v / *.tlog → curl-verbose
  * Python/Rust/JS/Java/Go panic+trace → stacktrace
  * docker ps / kubectl get / ps aux → tabular
  * pip/npm/yarn/pnpm install → pkginstall
  * systemd unit status / journalctl → systemd / journal
  * accesslog / containerlogs / strace / tcpdump / dns
  * envvar dump (KEY=value) → env (with classify-redact)
  * SERP HTML / Prometheus metrics / GraphQL schema / OpenAPI spec
  * ansible-playbook output / Maven / Gradle / sbt → ansible/jvmbuild
  * Jupyter notebook (.ipynb) → notebook
  * everything else → generic logs / passthrough

Anti-inflation guard (L81): if the compacted output would be LARGER
than the raw input, returns the raw bytes with a `passthrough_inflated`
label so the agent never gets worse-than-input.

Every compactor scrubs known secrets at its public entry (sweep
#858–#892) — the value of an `Authorization: Bearer eyJ…` or
`AWS_SECRET_ACCESS_KEY=hunter2` is replaced with `<redacted>`
regardless of which compactor the auto-detector picks.

Notes:
  * `read` IS the content entry point — the per-format compactor
    commands were removed; the auto-detector picks the right one.
    To force a specific compactor when you KNOW the format, run the
    producing command through `tokenade wrap '<cmd>'`.
  * Pipe input or path-as-arg; stdin needs `-` (single dash)."
}

pub(crate) fn watch_help_text() -> &'static str {
    "tokenade watch — keep the symbol index fresh while you edit.

USAGE:
  tokenade watch

Reindexes incrementally on every save, in the current project. Ctrl+C to
stop. Rarely needed: `query`, `map`, `semantic`… build and refresh the
index on demand.
"
}

pub(crate) fn statusline_help_text() -> &'static str {
    "tokenade statusline — the savings ticker in your agent's status bar.

USAGE:
  tokenade statusline [--json]        render the ticker (what the agent runs)
  tokenade statusline off | on        opt out (restores your previous status
                                      line) / re-enable; aliases disable, enable
  tokenade statusline status          show whether the ticker is on

`install` wires it into Claude Code, Copilot CLI and Cursor CLI. A custom
status line you already had keeps running first; the ticker goes below it.
--json prints saved_tokens, tokens_before, tokens_after, percent, usd, model.
"
}
