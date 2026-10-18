#![allow(unused_imports)]
//! Mechanical module split of install.rs (no behaviour change). Wired
//! back into the `install` namespace by mod.rs via `pub(crate) use <name>::*;`.
use super::*;
use crate::hooks::*;
use crate::wrap::*;
use crate::*;
use std::fs;
use std::path::{Path, PathBuf};

/// Used by the universal `--help` interceptor as a fallback when a
/// command has no dedicated help text yet. Returns the matching
/// "  cmd <args>  description" line from the top-level help block
/// so the user gets a focused mini-help instead of the full 60-line
/// command catalogue. Returns None if cmd isn't documented here
/// (caller falls back to print_help()).
pub(crate) fn oneline_help_for(cmd: &str) -> Option<String> {
    // Scan the same string literal print_help() prints — single source
    // of truth, no drift possible. Note: Rust's `\n\` line continuation
    // in the help body literal eats leading whitespace, so command
    // lines start at column 0 (not indented). Match by "first word
    // equals cmd AND line contains a 2+ space separator before the
    // description" — distinguishes command rows from section headers
    // like "USAGE:" / "COMMANDS:" / "YOUR OWN COMPACTORS:".
    let body = top_level_help_body();
    if let Some(line) = scan_help_body(&body, cmd) {
        return Some(line);
    }
    // Alias retry: the help body only lists primary command names
    // (`serp`/`notebook`/`stack`/...) but the dispatcher accepts
    // many aliases (`google`/`nb`/`stacktrace`/...) that fall
    // through here unmatched. Map aliases to their primary and
    // retry so `tokenade google --help` finds the serp line.
    if let Some(primary) = alias_to_primary(cmd) {
        if let Some(line) = scan_help_body(&body, primary) {
            return Some(line);
        }
    }
    None
}

pub(crate) fn scan_help_body(body: &str, cmd: &str) -> Option<String> {
    for line in body.lines() {
        let first = line.split_whitespace().next().unwrap_or("");
        if first != cmd {
            continue;
        }
        // Section headers ("USAGE:", "COMMANDS:", "YOUR OWN COMPACTORS:") and
        // bare-flag continuation lines (rare — e.g. install's "flags:"
        // sub-bullet) are NOT command rows. A command row always has
        // at least one more whitespace-delimited token after the cmd
        // name — its argument syntax or description. Originally the
        // filter required 2+ consecutive spaces (for alignment), but
        // longer cmd names like `terraform <file|->` push padding
        // down to a single space and were missed.
        let rest = line[first.len()..].trim_start();
        if rest.is_empty() {
            continue;
        }
        return Some(line.to_string());
    }
    None
}

/// Map a known alias to its primary command name. Mirrors the
/// dispatcher's `match cmd { ... }` arms at run()::~272-320. Used
/// by oneline_help_for so `tokenade <alias> --help` shows the
/// primary's mini-help instead of falling back to the full
/// command catalogue. Returns None for primaries (they match
/// directly via scan_help_body) and for commands without aliases.
pub(crate) fn alias_to_primary(cmd: &str) -> Option<&'static str> {
    match cmd {
        // Code-intel.
        "find" => Some("query"),
        "ask" | "sem" => Some("semantic"),
        // Compactors with aliased entry points.
        // `serp` and the engine names are hidden compat aliases for the one
        // search command; they are no longer listed.
        "serp" | "google" | "bing" | "ddg" => Some("search"),
        // (per-format compactor command aliases removed 2026-07 — those
        // commands no longer exist; `read` reaches the core compactors.)
        // `exec-compact` is the name our own scaffolded rules and subagent
        // guidance tell agents to use for a noisy command; it must resolve.
        "exec-compact" => Some("wrap"),
        // Bypass.
        "bypass" | "noproxy" => Some("raw"),
        // `mcp-proxy` is the OLD name, kept because it is written into every
        // shipped `CLAUDE.md`. `mcp-wrap` is the command.
        "mcp-proxy" | "mcp_proxy" | "mcp_wrap" => Some("mcp-wrap"),
        "llm_proxy" => Some("llm-proxy"),
        // Inspection.
        "stats" | "dash" => Some("dashboard"),
        "health" | "doctor" => Some("healthcheck"),
        // Read-mode ladder status — dispatcher arm accepts both forms.
        "readmode" => Some("read-mode"),
        // "review" → review-context: removed from help 2026-07-08 (feature not
        // studied, 3 calls/1 user fleet-wide). The command is still dispatched
        // (hidden); re-benchmark before any re-promotion.
        "mcpstats" => Some("mcp-stats"),
        "mistakes" => Some("mistake"),
        "summarize-turn" | "turnsummary" => Some("turn-summary"),
        "add-filter" => Some("add-compactor"),
        "trust_filters" => Some("trust-filters"),
        "savings-receipt" => Some("receipt"),
        "cachenote" => Some("cache-note"),
        // CLAUDE.md cascade auditor aliases.
        "audit-claudemd" | "claude-md-audit" | "claudemd" => Some("audit-claude-md"),
        // Gemini CLI hook entry-point alias.
        "geminihook" => Some("gemini-hook"),
        // Maintenance.
        "selfupdate" | "self-update" => Some("upgrade"),
        // Think-in-code runner — dispatcher line 379.
        "exec" | "run-script" => Some("execute"),
        // Underscore variants — KNOWN_COMMANDS accepts both forms
        // but the help body only documents the hyphen form.
        "shell_init" => Some("shell-init"),
        "evict_stale" => Some("evict-stale"),
        "scrub_cache" => Some("scrub-cache"),
        "scrub_secret" => Some("scrub-secret"),
        "semantic_status" => Some("semantic-status"),
        "unwrap_mcps" => Some("unwrap-mcps"),
        // `xref`/`xfold` are documented in the catalogue itself ("aliases:
        // xref", "alias: xfold") and were in neither this map nor
        // KNOWN_COMMANDS, so `tokenade xref --help` answered "unknown command
        // 'xref' — did you mean `read`?" about an alias the same run documents.
        "expand_ref" | "xref" => Some("expand-ref"),
        "expand_fold" | "xfold" => Some("expand-fold"),
        // Incompatibility scan aliases (primary renamed from `competitors`
        // to `incompatibilities`; old names kept as aliases).
        "competitors" | "detect-competitors" | "scan" | "incompat" => Some("incompatibilities"),
        _ => None,
    }
}

pub(crate) fn top_level_help_body() -> String {
    format!(
        "tokenade {VERSION} — Tokenade: universal token optimizer for AI coding agents\n\n\
USAGE:\n  tokenade <command> [args]\n\n\
COMMANDS:\n\
  raw <cmd...>       run a command WITHOUT compaction (bypass hook; aliases: bypass, noproxy)\n\
  filter <cmd...>    filter stdin as if produced by <cmd>\n\
  wrap <cmd>         run <cmd> through the auto-detected compactor (shell-passthrough; default for noisy commands; `exec-compact` alias)\n\
  index [--force] [--json]   build & cache the codebase symbol index (--force ignores the cache)\n\
  query <symbol>     locate a symbol without reading files [-o paths|count|outline] [--context N]\n\
  impact <file>      show refactor impact radius (dependents + symbols) [-o paths|count]\n\
  map                print a compact codebase structure map [-o paths|count]\n\
  skeleton <file>    print signatures-only skeleton of a file [-o paths|count] [--budget N]\n\
  semantic <query>   hybrid BM25 + dense embeddings, framework-aware (aliases: ask, sem) [-o paths|count|outline] [--budget N]\n\
                     (-o/--output-mode/--format: full [default] | paths | count | outline — return only what you need)\n\
                     (--budget N: cap the answer at N tokens; says how many results it withheld)\n\
                     (--json: machine-readable output — map, query, impact, pack, search)\n\
  web <url|file|->…  fetch/compact web pages → markdown (several URLs fetched concurrently; --prompt \"q1, q2\")\n\
  search \"<query>\"   web search: independent engines, merged + ranked (--answer, --json)\n\
                     also `search <file|->` to compact a saved results page\n\
  snapshot <file|->  compact a Playwright accessibility-tree snapshot\n\
  read <file|->      compact a file or piped content — auto-detects the format (THE content entry point)\n\
  style [auto|chat|coding|off]   response-style note added ahead of each user prompt (default auto)\n\
  read-mode [--json]   show the active fold read-mode ladder (TOKENADE_READ_MODE: aggressive/task/reference/entropy)\n\
  pack [--budget N] [--diff] [--base ref] [--out FILE] [--json] [PATH]   budget-bounded repo export (one fenced bundle, omitted files listed)\n\
  mcp-stats [--json]   MCP manifest rollup (servers/wrapped) + token-aware skills index\n\
  mistake <list|add|from-revert>   persistent 'bad direction' memory (records git reverts)\n\
  disclose <stash|read|search>   progressive disclosure: stash a big blob to a file, read/search by hash\n\
  expand-ref <hash…> [--prompt \"q1, q2\"|--grep <pat>|--raw]   ASK a folded blob what you need (RAG, several questions at once); --grep takes a REGEX (repeatable, matched per line); --raw dumps the bytes (aliases: xref)\n\
  expand-fold <id> [hash] [--max N]   expand a folded log run back to its raw lines (template_id from the fold banner; alias: xfold)\n\
  turn-summary [--max N] [file|-]   extractive summary of a conversation turn (top-N salient sentences)\n\
  add-compactor [--file f|--list]   install a custom TOML command compactor (run with --help for the format)\n\
  trust-filters [--list]            approve a repo's .tokenade/filters (project filters run only once trusted)\n\
  receipt [--json] [--verify <file>]   signed, tamper-evident savings receipt over the local ledger\n\
  cache-note [--saved N] [--fresh F]   prompt-cache-aware framing of token savings (net vs gross)\n\
  gain [--json|--history] [--by-source] [--recent]   show cumulative token savings\n\
  dashboard [--html|--json]   screenshot-ready savings overview (`dash` / `stats` aliases)\n\
  discover [--days N] [--json]   scan ~/.claude logs, surface top tools + install suggestions\n\
  healthcheck [--json]   diagnose the install (`health` / `doctor` aliases)\n\
  audit-claude-md [DIR] [--json] [--apply|--compress|--stale]   audit the CLAUDE.md cascade for bloat + cross-file dupes (`claudemd` alias)\n\
  semantic-status [--json]   per-project state of ~/.tokenade/semantic.db\n\
  watch              keep the symbol index fresh: incremental reindex on every save (Ctrl+C to stop)\n\
  hooks <status|tail [N]>   show installed hooks + recent activity / live tail\n\
  ensure-hooks [--quiet] [--force]   re-assert tokenade's hooks + shell proxy if something dropped them\n\
  codex-trust        re-apply the trust entries of tokenade's Codex hooks in ~/.codex/config.toml\n\
  audit-gains        detail the savings-ledger rows that break accounting rules (healthcheck `ledger-integrity`)\n\
  statusline [--json|off|on|status]   session ticker; `off` opts out (keeps your custom statusline), `on` re-enables\n\
  measure [--json]   scan project, report agent-loaded-file startup-token cost\n\
  execute --lang L (--script CODE | --script-file F)   think-in-code runner, returns only stdout (bash/sh/python/node/jq/awk/ruby/perl)
                      → returns only stdout, redacted + compacted + capped
                      → keeps raw data OUT of the agent's context\n\
  detect [--json]    print the detected AI agent\n\
  incompatibilities [--json]   check for tools that conflict with Tokenade &
                         propose fixes (aliases: competitors, detect-competitors, scan)
                         — flags each as redundant / overlapping / wasteful / compatible\n\
  shell-init [--shell sh|bash|zsh|fish|powershell|nushell|csh|cmd] [--all]   print the shell proxy fragment
                     (auto-detects from $SHELL / PSVersionTable) — strict POSIX, no bashisms.
                     Command list derived from the compactor registry, filtered to PATH-present
                     binaries (absent tools are never shadowed); --all skips the PATH filter:
                     sh/dash/bash/zsh: eval \"$(tokenade shell-init)\"  (also ~/.profile for servers)
                     fish:             tokenade shell-init --shell fish | source
                     pwsh/powershell:  Invoke-Expression (& tokenade shell-init --shell powershell | Out-String)
                     nushell:          tokenade shell-init --shell nushell o> ~/.tokenade/shellproxy.nu, then source it from config.nu
                     csh/tcsh, cmd:    aliases route via `tokenade shellwrap` (no shell functions)\n\
  shellwrap CMD [ARGS…]   TTY-aware alias target for csh/tcsh + cmd.exe; passthrough at a terminal,
                     `tokenade wrap` otherwise\n\
  install            register tokenade with the detected agent (MCP + hooks)
                     flags: --with-hook · --with-claude-md · --with-lsp-enforcement-hook · --with-brevity-skill · --only <agent> · --dry-run
                     --autocompact-pct N   compact at N% of the model's context window (10-95, default 70).
                                           Re-run with a different N to change it; upgrades keep the N you chose.
                     --no-autocompact      remove the threshold tokenade set (a hand-set value is left alone)\n\
  uninstall          full removal: undo integration + wipe ~/.tokenade data + remove/guide binary
                     flags: --dry-run (preview) · --keep-data · --headless; `uninstall aliases` removes only the shell proxy\n\
  unwrap-mcps        restore wrapped MCP servers to their original form (idempotent)
                     flags: --dry-run\n\
  evict-stale [--models]   prune 7-day-stale rows across all caches + zero-saved noise in gain.jsonl
                     + VACUUM SQLite DBs to reclaim disk\n\
  scrub-secret <substring>   replace every occurrence in tokenade-owned files + dedup.db
                     flags: --dry-run · --yes (required when not at a terminal) — emergency tool when a credential leaks\n\
  scrub-cache        re-apply built-in redact patterns to every persisted byte in dedup.db + stash/
                     (use after upgrade to clean up creds written before the in-pipeline redact fixes shipped)\n\
  redact <file…>     apply built-in secret-redaction patterns to arbitrary files (e.g. shared client data)
                     flags: --in-place (overwrite) · --dry-run (count only) — default writes <file>.redacted\n\
  upgrade            self-update tokenade in-place (aliases: self-update, selfupdate)
                     flags: --check (no swap) · --on / --off — toggle the 24h-TTL background updater\n\
  mcp-wrap [install|uninstall [<server>]|report|<bin> [<args>…]]   wrap
                     third-party MCP servers, compacting their responses;
                     `<bin>` is what `install` writes into MCP configs (no need
                     to invoke it by hand). Alias: mcp-proxy, the former name
                     (existing configs still launch it)\n\
  llm-proxy [install|uninstall|status]    rank 4 of the ladder: sit between an
                     agent and its provider, fold tool results already in the
                     history, and read the provider's own usage. `install`
                     sets it up only where it adds something the agent cannot
                     have otherwise (e.g. Codex with untrusted hooks); opt-in
                     per agent everywhere else\n\
  --version          print version\n\n\
YOUR OWN COMPACTORS:\n\
  A TOML filter tightens the output of your own CLIs (helm, ansible, scripts).\n\
  Run `tokenade add-compactor --file my.toml` to validate and install it in\n\
  ~/.tokenade/filters/ (project filters: .tokenade/filters/).\n\
  Schema: https://tokenade.net/en/docs/custom-compactors"
    )
}

pub(crate) fn print_help() {
    println!("{}", top_level_help_body());
}
