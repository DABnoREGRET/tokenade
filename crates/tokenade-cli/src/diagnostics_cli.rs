//! Diagnostics + lifecycle-adjacent CLI commands: detect-competitors,
//! detect, hooks/hooks-status/hooks-tail, discover, and the uninstall
//! cleanup helpers (perform_cleanup, remove_*). Extracted from main.rs.
#![allow(unused_imports)]
use crate::hooks::*;
use crate::install::*;
use crate::wrap::*;
use crate::*;
mod cleanup;
mod discover;
mod hooks_status;
pub(crate) use cleanup::*;
pub(crate) use discover::*;
pub(crate) use hooks_status::*;

/// `tokenade incompatibilities [--json]` — read-only scan for other token
/// tools already on the host that overlap or conflict with Tokenade. Drives
/// the `tokenade install` fix-up pass (run by default; opt out with
/// `--no-cleanup`): shows what's there, flags each item with a suggested
/// fix, and (in interactive install) asks whether to migrate + remove.
pub(crate) fn cmd_detect_competitors(rest: &[String]) -> i32 {
    // #782 — pre-fix `competitors --bogus` silently ran the scan and
    // exited 0, hiding the typo. Same class as #781. Only `--json`
    // (and the centrally-stripped `--help`/`-h`) is allowed; no
    // positional arguments are accepted.
    for a in rest {
        if a.starts_with('-') {
            if a == "--json" {
                continue;
            }
            eprintln!(
                "tokenade incompatibilities: unknown flag '{a}' (the only flag here is `--json`)."
            );
            return 2;
        }
        eprintln!(
            "tokenade incompatibilities: unexpected positional argument '{a}' \
             (this command takes no positional arguments; try `tokenade incompatibilities --help`)."
        );
        return 2;
    }
    let want_json = rest.iter().any(|a| a == "--json");
    let ctx = competitors::DetectionCtx::default();
    let results = competitors::detect_all(&ctx);

    if want_json {
        let payload: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                serde_json::json!({
                    "name": r.competitor.name,
                    "class": format!("{:?}", r.competitor.class),
                    "one_liner": r.competitor.one_liner,
                    "rationale": r.competitor.rationale,
                    "signals": r.signals.iter().map(signal_to_json).collect::<Vec<_>>(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&payload).unwrap());
        return 0;
    }

    if results.is_empty() {
        println!("No incompatible or overlapping token tools detected.");
        // #976 — context-aware next step (mirrors #455 discover +
        // #456 measure). The pre-fix message ALWAYS said "Run
        // `tokenade install`", which is a dead-end for users who
        // already ran it (the very binary running this check).
        let hook_already_installed = crate::install_cli::any_agent_has_tokenade().unwrap_or(false);
        if hook_already_installed {
            println!(
                "Tokenade is already wired into your AI agent — nothing to do. \
                 Run `tokenade healthcheck` to confirm install state."
            );
        } else {
            println!("Run `tokenade install` to register Tokenade with your AI agents.");
        }
        return 0;
    }
    println!(
        "── Found {} potentially incompatible tool(s) ──",
        results.len()
    );
    println!();
    for r in &results {
        let badge = match r.competitor.class {
            competitors::Class::DropInBetter => "[redundant]",
            competitors::Class::MemoryHolder => "[overlapping]",
            competitors::Class::AntiPattern => "[wasteful]",
            competitors::Class::Adjacent => "[compatible]",
        };
        println!("{badge} {}", r.competitor.name);
        println!("  {}", r.competitor.one_liner);
        for s in &r.signals {
            println!("  → {}", signal_one_line(s));
        }
        println!("  fix: {}", r.competitor.rationale);
        println!();
    }
    println!(
        "Run `tokenade install` to interactively migrate & fix the \
[redundant] / [overlapping] tools (the fix-up pass is on by default; \
opt out with `--no-cleanup`)."
    );
    0
}

pub(crate) fn signal_one_line(s: &competitors::Signal) -> String {
    use competitors::Signal;
    match s {
        Signal::Binary { path, installer } => {
            let inst = match installer {
                competitors::BinaryInstaller::NpmGlobal { package } => {
                    format!("npm -g {package}")
                }
                competitors::BinaryInstaller::Homebrew { formula } => format!("brew {formula}"),
                competitors::BinaryInstaller::CargoInstall { krate } => {
                    format!("cargo install {krate}")
                }
                competitors::BinaryInstaller::RawCopy { needs_sudo } => {
                    if *needs_sudo {
                        "raw binary (needs sudo to remove)".into()
                    } else {
                        "raw binary".into()
                    }
                }
            };
            format!("binary at {} ({inst})", path.display())
        }
        Signal::McpEntry {
            config_path,
            entry_key,
        } => format!("MCP entry `{entry_key}` in {}", config_path.display()),
        Signal::HookScript {
            settings_path,
            event,
            matcher,
            command,
        } => format!(
            "hook in {} ({event}/{matcher}): {command}",
            settings_path.display()
        ),
        Signal::DataDir { path, kind } => {
            let k = match kind {
                competitors::DataKind::SymbolGraph => "symbol graph",
                competitors::DataKind::ObservationLog => "observation log",
                competitors::DataKind::KeyValueMemory => "key/value memory",
                competitors::DataKind::Telemetry => "telemetry/cache",
            };
            format!("data dir ({k}) at {}", path.display())
        }
        Signal::RuleBlock { file, open, .. } => {
            format!("rules block `{open}` in {}", file.display())
        }
        Signal::RuleFile { path } => format!("rules file {}", path.display()),
    }
}

pub(crate) fn signal_to_json(s: &competitors::Signal) -> serde_json::Value {
    use competitors::Signal;
    match s {
        Signal::Binary { path, installer } => serde_json::json!({
            "kind": "binary",
            "path": path.display().to_string(),
            "installer": format!("{installer:?}"),
        }),
        Signal::McpEntry {
            config_path,
            entry_key,
        } => serde_json::json!({
            "kind": "mcp_entry",
            "config_path": config_path.display().to_string(),
            "entry_key": entry_key,
        }),
        Signal::HookScript {
            settings_path,
            event,
            matcher,
            command,
        } => serde_json::json!({
            "kind": "hook_script",
            "settings_path": settings_path.display().to_string(),
            "event": event,
            "matcher": matcher,
            "command": command,
        }),
        Signal::DataDir { path, kind } => serde_json::json!({
            "kind": "data_dir",
            "path": path.display().to_string(),
            "data_kind": format!("{kind:?}"),
        }),
        Signal::RuleBlock { file, open, close } => serde_json::json!({
            "kind": "rule_block",
            "file": file.display().to_string(),
            "open": open,
            "close": close,
        }),
        Signal::RuleFile { path } => serde_json::json!({
            "kind": "rule_file",
            "path": path.display().to_string(),
        }),
    }
}

pub(crate) fn cmd_detect(rest: &[String]) -> i32 {
    // `--agent <name>` answers ABOUT a named agent instead of sniffing for one.
    //
    // Detection is authoritative when an agent is calling us, and that is
    // exactly what makes it useless for the question "where does agent X keep
    // its MCP config": asked from inside Claude Code, it answers about Claude
    // Code however the caller phrased it.
    //
    // Measured 2026-08-20: the compatibility bench carried its OWN table of
    // MCP config paths because it had no way to ask, and ten agents came back
    // `Partial` on the MCP box with "no MCP config path known for this agent"
    // — for four of them (opencode, kilo-code, hermes, cline) the product knew
    // the path perfectly well and `mcp-wrap install` rewrites it. A bench that
    // guesses what the product knows measures its own table. The rank-4 probe
    // learned this and now reads the path out of the install output; this is
    // the same lesson one rung down.
    //
    // Useful outside the bench too: an operator wiring an agent by hand has no
    // other way to be told where its file lives.
    let named = rest
        .iter()
        .position(|a| a == "--agent")
        .and_then(|i| rest.get(i + 1))
        .cloned();
    let rest: Vec<String> = {
        let mut out = Vec::new();
        let mut skip = false;
        for a in rest {
            if skip {
                skip = false;
                continue;
            }
            if a == "--agent" {
                skip = true;
                continue;
            }
            out.push(a.clone());
        }
        out
    };
    let rest = &rest[..];
    if let Err(code) = validate_simple_bool_flags(rest, "detect", &["--json"]) {
        return code;
    }
    let agent = match named {
        Some(name) => match detect::Agent::from_slug(&name) {
            Some(a) => a,
            None => {
                eprintln!("tokenade detect: unknown agent {name:?}");
                return 2;
            }
        },
        None => detect::detect(&root()),
    };
    // #625 — `--json` for CI parity with healthcheck/gain/dashboard/
    // discover/measure/semantic-status. Single-shot detection
    // (no list, no rolled-up totals) so the payload is flat:
    // `agent` + `mcp_config_hint`.
    if rest.iter().any(|a| a == "--json") {
        let payload = serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "agent": agent.as_str(),
            "coverage": agent.coverage().as_str(),
            "coverage_detail": agent.coverage().describe(),
            "mcp_config_hint": agent.mcp_config_hint(),
        });
        println!("{}", serde_json::to_string_pretty(&payload).unwrap());
        return 0;
    }
    println!("{}", agent.as_str());
    // The coverage line comes FIRST because it is the question being asked.
    // `mcp config hint` used to be the only thing printed here, and it has
    // answered about a channel the product stopped writing at the zero-MCP
    // pivot — so a covered user could read it and conclude nothing was
    // installed, and an uncovered one had nothing telling them so.
    eprintln!("coverage: {}", agent.coverage().describe());
    eprintln!(
        "mcp config hint: {} (third-party servers only)",
        agent.mcp_config_hint()
    );
    0
}

#[cfg(test)]
include!("diagnostics_cli_tests.rs");
