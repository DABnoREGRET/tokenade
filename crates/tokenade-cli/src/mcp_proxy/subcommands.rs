//! **What is set up ONCE**: `install`, `uninstall`, `report`, `--help`.
//!
//! The proxy is a setting, not a call: the launch command of each MCP server
//! is wrapped in the agent's configuration, and every result is folded
//! afterwards with nothing per call. These four commands are therefore of a
//! different nature than the pump — they edit a configuration file and
//! report, they carry nothing.

// No `use super::*;`: setting it up asks nothing of the pump — which is
// precisely the proof that the two are of a different nature.

/// First positional (non-flag) token, if any — the server name for
/// `install`/`uninstall`.
pub(crate) fn first_positional(args: &[String]) -> Option<&str> {
    args.iter()
        .map(String::as_str)
        .find(|a| !a.starts_with('-'))
}

/// `tokenade mcp-wrap install [<server>]` — route a configured MCP server
/// (or all of them) through the compacting proxy. Rewrites the agent config
/// in place after a timestamped backup; idempotent.
///
/// (This doc block sat on `redact_mcp_text_items` until 2026-08-08, above that
/// function's own SECURITY comment — two descriptions stacked on one function
/// and the command they describe left undocumented.)
/// Flags each management subcommand actually implements. `--all` is real on
/// `uninstall` (its help documents it) and means the same as no name at all;
/// everything else here is `--help`, which the central interceptor takes first.
pub(crate) const MCP_INSTALL_FLAGS: &[&str] = &["--help", "-h"];
pub(crate) const MCP_UNINSTALL_FLAGS: &[&str] = &["--help", "-h", "--all"];
pub(crate) const MCP_REPORT_FLAGS: &[&str] = &["--help", "-h"];

pub(crate) fn cmd_mcp_proxy_install(args: &[String]) -> i32 {
    // `first_positional` skips every `-`-prefixed token, so an unimplemented
    // flag was DISCARDED and the subcommand ran with no name — which means
    // "all servers". MEASURED 2026-08-31 on a config holding two servers:
    // `mcp-wrap uninstall --zzbogus` restored BOTH and reported success at
    // exit 0. A typo aimed at one server rewrote the agent's MCP config for
    // every server it had. `--all` "worked" for the same reason it was
    // discarded, so the documented flag was never implemented either.
    if let Err(c) = crate::install_cli::validate_flags_and_positionals(
        args,
        "mcp-wrap install",
        MCP_INSTALL_FLAGS,
        1,
        "server name",
    ) {
        return c;
    }
    let name = first_positional(args);
    let reports = tokenade_core::mcp_wrap::wrap_mcp_servers(name);
    render_wrap_reports(&reports, name, true)
}

/// `tokenade mcp-wrap uninstall [<server>|--all]` — restore a wrapped MCP
/// server (or all of them) to its original launch command. A bare
/// `uninstall` and `uninstall --all` both mean "all". Idempotent.
pub(crate) fn cmd_mcp_proxy_uninstall(args: &[String]) -> i32 {
    if let Err(c) = crate::install_cli::validate_flags_and_positionals(
        args,
        "mcp-wrap uninstall",
        MCP_UNINSTALL_FLAGS,
        1,
        "server name",
    ) {
        return c;
    }
    let name = first_positional(args);
    let reports = tokenade_core::mcp_wrap::unwrap_mcp_servers(name);
    render_wrap_reports(&reports, name, false)
}

/// Render the outcome of a wrap/unwrap pass. Returns the process exit code:
/// 0 when something was (already) in the desired state, 1 on a hard error or
/// a named server that was found nowhere.
pub(crate) fn render_wrap_reports(
    reports: &[tokenade_core::mcp_wrap::WrapReport],
    name: Option<&str>,
    wrap: bool,
) -> i32 {
    let verb = if wrap { "wrapped" } else { "restored" };
    let mut total = 0usize;
    let mut already = 0usize;
    let mut errors = 0usize;
    for r in reports {
        if let Some(err) = &r.error {
            eprintln!("  ✗ {}: {err}", r.config_path);
            errors += 1;
            continue;
        }
        let touched: &Vec<String> = if wrap { &r.wrapped } else { &r.unwrapped };
        if !touched.is_empty() {
            total += touched.len();
            print!("  ✓ {} {} in {}", verb, touched.join(", "), r.config_path);
            if let Some(b) = &r.backup {
                print!(" (backup: {b})");
            }
            println!();
        }
        if wrap {
            // Both "already wrapped" and an in-place stale-wrapper refresh
            // mean the server is (now) in the wrapped state — treat as a
            // successful idempotent no-op for the summary.
            already += r.skipped_already_wrapped.len() + r.refreshed_wrappers.len();
        }
    }
    if errors > 0 && total == 0 {
        return 1;
    }
    if total == 0 {
        match name {
            Some(n) if already > 0 => {
                println!("⊙ '{n}' already {verb} — nothing to do (idempotent).");
                0
            }
            Some(n) => {
                eprintln!(
                    "no MCP server named '{n}' {} in any known agent config.",
                    if wrap {
                        "to wrap was found"
                    } else {
                        "is wrapped"
                    }
                );
                1
            }
            None if already > 0 => {
                println!("⊙ all configured MCP servers already {verb} (idempotent).");
                0
            }
            None => {
                println!(
                    "no MCP servers {} across known agent configs.",
                    if wrap {
                        "to wrap were found"
                    } else {
                        "are wrapped"
                    }
                );
                0
            }
        }
    } else {
        let scope = match name {
            Some(n) => format!("'{n}'"),
            None => format!("{total} MCP server(s)"),
        };
        if wrap {
            println!(
                "✓ {scope} now launch(es) through tokenade mcp-wrap \
                 (run `tokenade mcp-wrap uninstall` to reverse)."
            );
        } else {
            println!("✓ {scope} restored to the original launch command.");
        }
        0
    }
}

/// Format a small USD figure. Sub-cent savings stay legible (4 dp);
/// larger ones round to cents.
pub(crate) fn fmt_usd(v: f64) -> String {
    if v < 0.01 {
        format!("${v:.4}")
    } else {
        format!("${v:.2}")
    }
}

/// `tokenade mcp-wrap report` — per-tool savings table for every wrapped
/// MCP server: tool | calls | bytes saved | tokens saved | $ saved. The $
/// column reuses the dashboard's per-model price table (real prices).
pub(crate) fn cmd_mcp_proxy_report(_args: &[String]) -> i32 {
    if let Err(c) = crate::install_cli::validate_flags_and_positionals(
        _args,
        "mcp-wrap report",
        MCP_REPORT_FLAGS,
        0,
        "",
    ) {
        return c;
    }
    let stats = tokenade_core::gain::mcp_tool_report();
    if stats.is_empty() {
        println!(
            "No wrapped-MCP tool activity recorded yet. Wrap a server with \
             `tokenade mcp-wrap install <server>` and use it, then re-run."
        );
        return 0;
    }
    let name_w = stats.iter().map(|s| s.tool.len()).max().unwrap_or(4).max(4);
    println!(
        "{:<name_w$}  {:>6}  {:>12}  {:>12}  {:>10}",
        "tool", "calls", "bytes", "tokens", "$ saved"
    );
    let (mut tc, mut bc, mut kc, mut uc) = (0u64, 0u64, 0u64, 0.0f64);
    for s in &stats {
        println!(
            "{:<name_w$}  {:>6}  {:>12}  {:>12}  {:>10}",
            s.tool,
            s.calls,
            s.bytes_saved,
            s.tokens_saved,
            fmt_usd(s.usd_saved)
        );
        tc += s.calls;
        bc += s.bytes_saved;
        kc += s.tokens_saved;
        uc += s.usd_saved;
    }
    println!(
        "{:<name_w$}  {:>6}  {:>12}  {:>12}  {:>10}",
        "TOTAL",
        tc,
        bc,
        kc,
        fmt_usd(uc)
    );
    0
}

pub(crate) fn mcp_proxy_help_text() -> &'static str {
    "tokenade mcp-wrap — wrap a third-party MCP server, compacting its responses.

USAGE:
  tokenade mcp-wrap install [<server>]   # route configured MCP server(s)
                                          # through the proxy (no name = all)
  tokenade mcp-wrap uninstall [<server>] # restore original launch command
                                          # (no name / --all = every wrapped)
  tokenade mcp-wrap report               # per-tool savings table (+ real $)
  tokenade mcp-wrap -- <bin> [<args>…]   # run as the proxy (agents do this)
  tokenade mcp-wrap <bin> [<args>…]      # leading `--` is optional

install/uninstall edit the agent's MCP config (Claude, Cursor, Codex,
Copilot, Windsurf, Cline, VSCode — across Linux/macOS/Windows paths) in
place: they write a timestamped `.bak` first, rewrite atomically, and are
idempotent + fully reversible. `report` prices saved tokens at the same
per-model rates as `tokenade dashboard`.

Spawns <bin> as a child MCP server, streams JSON-RPC between the
launching agent and the child verbatim, and intercepts every
`tools/call` response: any `result.content[].text` payload bigger
than ~25k tokens (TOKENADE_READ_DISCLOSE_THRESHOLD) gets stashed to
disk (~/.cache/tokenade/stash/) and
replaced with a structural summary the agent can drill into via
`tokenade expand-ref <hash>` if needed.

Image results are NOT passed through untouched: a screenshot whose long
edge is over the cap is resized down and re-encoded (the payload is only
swapped when it actually shrank), and a `text` item is appended to the
result saying the new size and how to turn it off. Set
TOKENADE_IMAGE_MAX_EDGE=0 in the wrapped server's environment to keep
full resolution.

This is what `tokenade install` rewrites every third-party MCP entry
to launch — wrap → compact → fold. Each agent boot spawns one proxy
per third-party MCP server; the proxy exits when its stdin closes.

You almost never run this manually. To inspect what tokenade did to
your agent's MCP config:
  tokenade healthcheck             # reports each `mcpServers.*` entry it
                                   # rewrote and the original target it
                                   # wraps

To restore the originals (e.g. before switching tools):
  tokenade unwrap-mcps             # surgical: only flips wrapped entries back
  tokenade uninstall               # full undo

Aliases: mcp-proxy, mcp_wrap, mcp_proxy"
}
