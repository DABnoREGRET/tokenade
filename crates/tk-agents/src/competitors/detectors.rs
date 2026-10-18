//! **One detector per tool** — and each knows where ITS tool settles.
//!
//! There is no generic detection: `rtk` is a binary on the PATH,
//! `claude-mem` an MCP entry, `claudeslim` a hooks group, `ccusage` a
//! global npm package. A single parameterised function would pass these
//! differences off as configuration details, when they are the only
//! thing each detector has to know.
//!
//! Each returns its SIGNALS rather than a boolean: "found" without saying where
//! allows neither displaying it, nor removing it, nor being visibly wrong.

use super::*;

// =======================================================================
// Per-competitor detectors
// =======================================================================

pub(crate) fn detect_rtk(ctx: &DetectionCtx) -> Vec<Signal> {
    let mut sigs = Vec::new();
    if let Some(p) = (ctx.which)("rtk") {
        // Detect installer: brew lives under /opt/homebrew or /usr/local/bin
        // typically; raw cp targets the same place. We can't tell apart 100%
        // reliably without probing the brew DB; default to RawCopy (uses
        // `rm` + maybe sudo) and let the installer surface the actual path.
        let needs_sudo = !p.starts_with(&ctx.home);
        sigs.push(Signal::Binary {
            path: p,
            installer: BinaryInstaller::RawCopy { needs_sudo },
        });
    }
    // Hook script that ships with rtk's installer.
    let hook_script = ctx
        .home
        .join(".claude")
        .join("hooks")
        .join("rtk-rewrite.sh");
    if hook_script.exists() {
        sigs.push(Signal::DataDir {
            path: hook_script,
            kind: DataKind::Telemetry,
        });
    }
    // `rtk init -g` : `RTK.md` + `@RTK.md` in the global CLAUDE.md. Detected
    // on its own — once the binary is gone nothing else names it, and the
    // agent keeps loading a lesson for a tool that no longer exists.
    let claude_dir = ctx.home.join(".claude");
    let importer = claude_dir.join("CLAUDE.md");
    if std::fs::read_to_string(&importer).is_ok_and(|s| s.lines().any(|l| l.trim() == "@RTK.md")) {
        sigs.push(Signal::RuleBlock {
            file: importer,
            open: "@RTK.md".into(),
            close: "@RTK.md".into(),
        });
    }
    let rules_file = claude_dir.join("RTK.md");
    if rules_file.exists() {
        sigs.push(Signal::RuleFile { path: rules_file });
    }
    // Hook entry in ~/.claude/settings.json that POINTS at rtk-rewrite.sh.
    let settings_path = ctx.home.join(".claude").join("settings.json");
    if let Some(v) = (ctx.read_json)(&settings_path) {
        for (event, matcher_groups) in iter_hook_groups(&v) {
            for (matcher, cmd) in matcher_groups {
                if cmd.contains("rtk") {
                    sigs.push(Signal::HookScript {
                        settings_path: settings_path.clone(),
                        event: event.clone(),
                        matcher: matcher.clone(),
                        command: cmd,
                    });
                }
            }
        }
    }
    sigs
}

pub(crate) fn detect_codegraph(ctx: &DetectionCtx) -> Vec<Signal> {
    let mut sigs = Vec::new();
    if let Some(p) = (ctx.which)("codegraph") {
        // npm-shim path looks like `.../bin/codegraph` with a symlink to
        // `lib/node_modules/<scope>/<pkg>/...`. Resolve & guess the package.
        let pkg = resolve_npm_global_package(&p).unwrap_or_else(|| "codegraph".to_string());
        sigs.push(Signal::Binary {
            path: p,
            installer: BinaryInstaller::NpmGlobal { package: pkg },
        });
    }
    // Per-project data dir (their default).
    let codegraph_dir = ctx.cwd.join(".codegraph");
    if codegraph_dir.is_dir() {
        sigs.push(Signal::DataDir {
            path: codegraph_dir,
            kind: DataKind::SymbolGraph,
        });
    }
    // MCP entry in ~/.claude.json or per-agent configs.
    for cfg in claude_config_candidates(&ctx.home) {
        if let Some(v) = (ctx.read_json)(&cfg) {
            if mcp_has_entry(&v, "codegraph") {
                sigs.push(Signal::McpEntry {
                    config_path: cfg,
                    entry_key: "mcpServers.codegraph".into(),
                });
            }
        }
    }
    sigs
}

pub(crate) fn detect_tokensave(ctx: &DetectionCtx) -> Vec<Signal> {
    let mut sigs = Vec::new();
    if let Some(p) = (ctx.which)("tokensave") {
        sigs.push(Signal::Binary {
            path: p,
            installer: BinaryInstaller::CargoInstall {
                krate: "tokensave".into(),
            },
        });
    }
    for cfg in claude_config_candidates(&ctx.home) {
        if let Some(v) = (ctx.read_json)(&cfg) {
            if mcp_has_entry(&v, "tokensave") {
                sigs.push(Signal::McpEntry {
                    config_path: cfg,
                    entry_key: "mcpServers.tokensave".into(),
                });
            }
        }
    }
    sigs
}

pub(crate) fn detect_codesight(ctx: &DetectionCtx) -> Vec<Signal> {
    let mut sigs = Vec::new();
    if let Some(p) = (ctx.which)("codesight") {
        sigs.push(Signal::Binary {
            path: p,
            installer: BinaryInstaller::NpmGlobal {
                package: "codesight".into(),
            },
        });
    }
    for cfg in claude_config_candidates(&ctx.home) {
        if let Some(v) = (ctx.read_json)(&cfg) {
            if mcp_has_entry(&v, "codesight") {
                sigs.push(Signal::McpEntry {
                    config_path: cfg,
                    entry_key: "mcpServers.codesight".into(),
                });
            }
        }
    }
    sigs
}

pub(crate) fn detect_claude_mem(ctx: &DetectionCtx) -> Vec<Signal> {
    let mut sigs = Vec::new();
    if let Some(p) = (ctx.which)("claude-mem") {
        sigs.push(Signal::Binary {
            path: p,
            installer: BinaryInstaller::NpmGlobal {
                package: "claude-mem".into(),
            },
        });
    }
    // Real layout (verified upstream, ~79k stars):
    //   ~/.claude-mem/claude-mem.db        SQLite (observations)
    //   ~/.claude-mem/chroma/              ChromaDB vector store
    // Override: CLAUDE_MEM_DATA_DIR.
    let root = std::env::var_os("CLAUDE_MEM_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| ctx.home.join(".claude-mem"));
    if root.is_dir() {
        sigs.push(Signal::DataDir {
            path: root,
            kind: DataKind::ObservationLog,
        });
    }
    sigs
}

pub(crate) fn detect_agentmemory(ctx: &DetectionCtx) -> Vec<Signal> {
    let mut sigs = Vec::new();
    if let Some(p) = (ctx.which)("agentmemory") {
        sigs.push(Signal::Binary {
            path: p,
            installer: BinaryInstaller::NpmGlobal {
                package: "@agentmemory/agentmemory".into(),
            },
        });
    }
    for candidate in [
        ctx.home.join(".agentmemory"),
        ctx.home.join(".iii-engine"),
        ctx.home.join(".config").join("agentmemory"),
    ] {
        if candidate.is_dir() {
            sigs.push(Signal::DataDir {
                path: candidate,
                kind: DataKind::KeyValueMemory,
            });
            break;
        }
    }
    sigs
}

pub(crate) fn detect_claude_context(ctx: &DetectionCtx) -> Vec<Signal> {
    let mut sigs = Vec::new();
    for cfg in claude_config_candidates(&ctx.home) {
        if let Some(v) = (ctx.read_json)(&cfg) {
            if mcp_has_entry(&v, "claude-context") || mcp_has_entry(&v, "claude_context") {
                sigs.push(Signal::McpEntry {
                    config_path: cfg,
                    entry_key: "mcpServers.claude-context".into(),
                });
            }
        }
    }
    sigs
}

pub(crate) fn detect_token_optimizer_alexgreensh(ctx: &DetectionCtx) -> Vec<Signal> {
    let mut sigs = Vec::new();
    if let Some(p) = (ctx.which)("token-optimizer") {
        sigs.push(Signal::Binary {
            path: p,
            installer: BinaryInstaller::NpmGlobal {
                package: "token-optimizer".into(),
            },
        });
    }
    // Their plugin dir if installed via the Claude Code plugin marketplace.
    let plugin_dir = ctx
        .home
        .join(".claude")
        .join("plugins")
        .join("token-optimizer");
    if plugin_dir.is_dir() {
        sigs.push(Signal::DataDir {
            path: plugin_dir,
            kind: DataKind::Telemetry,
        });
    }
    sigs
}

pub(crate) fn detect_claudeslim(ctx: &DetectionCtx) -> Vec<Signal> {
    let mut sigs = Vec::new();
    if let Some(p) = (ctx.which)("claudeslim") {
        let needs_sudo = !p.starts_with(&ctx.home);
        sigs.push(Signal::Binary {
            path: p,
            installer: BinaryInstaller::RawCopy { needs_sudo },
        });
    }
    // The proxy is meant to run as a systemd unit on localhost:8086. We
    // can't probe that without poking a port; the env var is the signal.
    if std::env::var("ANTHROPIC_BASE_URL").is_ok_and(|v| v.contains("localhost:8086")) {
        sigs.push(Signal::DataDir {
            path: PathBuf::from("ANTHROPIC_BASE_URL=localhost:8086"),
            kind: DataKind::Telemetry,
        });
    }
    sigs
}

pub(crate) fn detect_lazy_mcp(ctx: &DetectionCtx) -> Vec<Signal> {
    let mut sigs = Vec::new();
    if let Some(p) = (ctx.which)("lazy-mcp") {
        sigs.push(Signal::Binary {
            path: p,
            installer: BinaryInstaller::CargoInstall {
                krate: "lazy-mcp".into(),
            },
        });
    }
    for cfg in claude_config_candidates(&ctx.home) {
        if let Some(v) = (ctx.read_json)(&cfg) {
            if mcp_has_entry(&v, "lazy-mcp") || mcp_has_entry(&v, "lazy_mcp") {
                sigs.push(Signal::McpEntry {
                    config_path: cfg,
                    entry_key: "mcpServers.lazy-mcp".into(),
                });
            }
        }
    }
    sigs
}

pub(crate) fn detect_caveman(ctx: &DetectionCtx) -> Vec<Signal> {
    let mut sigs = Vec::new();
    if let Some(p) = (ctx.which)("caveman") {
        sigs.push(Signal::Binary {
            path: p,
            installer: BinaryInstaller::NpmGlobal {
                package: "caveman".into(),
            },
        });
    }
    // Skill files live under ~/.claude/skills/caveman/.
    let skill_dir = ctx.home.join(".claude").join("skills").join("caveman");
    if skill_dir.is_dir() {
        sigs.push(Signal::DataDir {
            path: skill_dir,
            kind: DataKind::Telemetry,
        });
    }
    sigs
}

/// Ponytail — a "minimal code" behavioural skill, shipped as a Claude Code
/// plugin (skills + lifecycle hooks). Detect the skill dir and any plugin
/// install dir whose name contains `ponytail`. Telemetry-only: the plugin
/// holds no user data, so removal is a clean uninstall.
pub(crate) fn detect_ponytail(ctx: &DetectionCtx) -> Vec<Signal> {
    let mut sigs = Vec::new();
    let skill_dir = ctx.home.join(".claude").join("skills").join("ponytail");
    if skill_dir.is_dir() {
        sigs.push(Signal::DataDir {
            path: skill_dir,
            kind: DataKind::Telemetry,
        });
    }
    let plugins = ctx.home.join(".claude").join("plugins");
    for dir in [plugins.clone(), plugins.join("marketplaces")] {
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                if e.file_name()
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .contains("ponytail")
                {
                    sigs.push(Signal::DataDir {
                        path: e.path(),
                        kind: DataKind::Telemetry,
                    });
                }
            }
        }
    }
    sigs
}

pub(crate) fn detect_ccusage(ctx: &DetectionCtx) -> Vec<Signal> {
    let mut sigs = Vec::new();
    if let Some(p) = (ctx.which)("ccusage") {
        sigs.push(Signal::Binary {
            path: p,
            installer: BinaryInstaller::NpmGlobal {
                package: "ccusage".into(),
            },
        });
    }
    sigs
}

pub(crate) fn detect_graphify(ctx: &DetectionCtx) -> Vec<Signal> {
    let mut sigs = Vec::new();
    if let Some(p) = (ctx.which)("graphify") {
        // Pip package name has a gotcha: `graphifyy` (double-y). Pure `graphify`
        // on PyPI is an unrelated project.
        sigs.push(Signal::Binary {
            path: p,
            installer: BinaryInstaller::NpmGlobal {
                package: "graphifyy".into(),
            },
        });
    }
    let out = ctx.cwd.join("graphify-out");
    if out.is_dir() {
        sigs.push(Signal::DataDir {
            path: out,
            kind: DataKind::SymbolGraph,
        });
    }
    sigs
}

pub(crate) fn detect_context_mode(ctx: &DetectionCtx) -> Vec<Signal> {
    let mut sigs = Vec::new();
    if let Some(p) = (ctx.which)("context-mode") {
        sigs.push(Signal::Binary {
            path: p,
            installer: BinaryInstaller::NpmGlobal {
                package: "context-mode".into(),
            },
        });
    }
    // Per-adapter data dirs (one per supported AI agent). Override:
    // CONTEXT_MODE_DIR.
    let override_dir = std::env::var_os("CONTEXT_MODE_DIR").map(PathBuf::from);
    let candidates: Vec<PathBuf> = if let Some(d) = override_dir {
        vec![d]
    } else {
        vec![
            ctx.home.join(".claude").join("context-mode"),
            ctx.home.join(".codex").join("context-mode"),
            ctx.home
                .join(".config")
                .join("opencode")
                .join("context-mode"),
            ctx.home.join(".omp").join("context-mode"),
        ]
    };
    for c in candidates {
        if c.is_dir() {
            sigs.push(Signal::DataDir {
                path: c,
                kind: DataKind::ObservationLog,
            });
        }
    }
    for cfg in claude_config_candidates(&ctx.home) {
        if let Some(v) = (ctx.read_json)(&cfg) {
            if mcp_has_entry(&v, "context-mode") {
                sigs.push(Signal::McpEntry {
                    config_path: cfg,
                    entry_key: "mcpServers.context-mode".into(),
                });
            }
        }
    }
    sigs
}

pub(crate) fn detect_token_optimizer_mcp(ctx: &DetectionCtx) -> Vec<Signal> {
    let mut sigs = Vec::new();
    if let Some(p) = (ctx.which)("token-optimizer-mcp") {
        sigs.push(Signal::Binary {
            path: p,
            installer: BinaryInstaller::NpmGlobal {
                package: "token-optimizer-mcp".into(),
            },
        });
    }
    if let Some(p) = (ctx.which)("token-optimizer-daemon") {
        sigs.push(Signal::Binary {
            path: p,
            installer: BinaryInstaller::NpmGlobal {
                package: "token-optimizer-mcp".into(),
            },
        });
    }
    for cfg in claude_config_candidates(&ctx.home) {
        if let Some(v) = (ctx.read_json)(&cfg) {
            // Convention key in their README is `token-optimizer`.
            if mcp_has_entry(&v, "token-optimizer") || mcp_has_entry(&v, "token-optimizer-mcp") {
                sigs.push(Signal::McpEntry {
                    config_path: cfg,
                    entry_key: "mcpServers.token-optimizer".into(),
                });
            }
        }
    }
    sigs
}

pub(crate) fn detect_mcp_codebase_index(ctx: &DetectionCtx) -> Vec<Signal> {
    let mut sigs = Vec::new();
    if let Some(p) = (ctx.which)("mcp-codebase-index") {
        sigs.push(Signal::Binary {
            path: p,
            installer: BinaryInstaller::NpmGlobal {
                package: "mcp-codebase-index".into(),
            },
        });
    }
    let pkl = ctx.cwd.join(".codebase-index-cache.pkl");
    if pkl.is_file() {
        sigs.push(Signal::DataDir {
            path: pkl,
            kind: DataKind::SymbolGraph,
        });
    }
    for cfg in claude_config_candidates(&ctx.home) {
        if let Some(v) = (ctx.read_json)(&cfg) {
            if mcp_has_entry(&v, "codebase-index") {
                sigs.push(Signal::McpEntry {
                    config_path: cfg,
                    entry_key: "mcpServers.codebase-index".into(),
                });
            }
        }
    }
    sigs
}

pub(crate) fn detect_claude_code_memory_setup(ctx: &DetectionCtx) -> Vec<Signal> {
    let mut sigs = Vec::new();
    // No binary — this is an Obsidian-vault setup. The signal IS the vault.
    // Common paths people set up:
    for candidate in [
        ctx.home.join("vault"),
        ctx.home.join("Documents").join("vault"),
        ctx.home.join("Obsidian").join("vault"),
    ] {
        let permanent = candidate.join("permanent");
        if permanent.is_dir() {
            sigs.push(Signal::DataDir {
                path: permanent,
                kind: DataKind::ObservationLog,
            });
            break;
        }
    }
    sigs
}

pub(crate) fn detect_mcp_memory_service(ctx: &DetectionCtx) -> Vec<Signal> {
    let mut sigs = Vec::new();
    // Multi-bin tool — both npm and uvx ship variants. The MCP entry is
    // the most reliable signal.
    for cfg in claude_config_candidates(&ctx.home) {
        if let Some(v) = (ctx.read_json)(&cfg) {
            if mcp_has_entry(&v, "memory") || mcp_has_entry(&v, "mcp-memory-service") {
                sigs.push(Signal::McpEntry {
                    config_path: cfg,
                    entry_key: "mcpServers.memory".into(),
                });
            }
        }
    }
    // Default data path per their docs.
    let candidates = [
        ctx.home.join(".mcp-memory-service"),
        ctx.home.join(".config").join("mcp-memory-service"),
    ];
    for c in candidates {
        if c.is_dir() {
            sigs.push(Signal::DataDir {
                path: c,
                kind: DataKind::KeyValueMemory,
            });
            break;
        }
    }
    sigs
}

pub(crate) fn detect_headroom(ctx: &DetectionCtx) -> Vec<Signal> {
    let mut sigs = Vec::new();
    // `headroom` CLI is installed by `pip install headroom` and lands on PATH.
    if let Some(p) = (ctx.which)("headroom") {
        sigs.push(Signal::Binary {
            path: p,
            installer: BinaryInstaller::RawCopy { needs_sudo: false },
        });
    }
    // Python package data dir — headroom stores its TOIN cache here.
    for candidate in [
        ctx.home.join(".headroom"),
        ctx.home.join(".config").join("headroom"),
        ctx.home.join(".local").join("share").join("headroom"),
    ] {
        if candidate.is_dir() {
            sigs.push(Signal::DataDir {
                path: candidate,
                kind: DataKind::Telemetry,
            });
            break;
        }
    }
    // MCP proxy mode — registers under "headroom" in agent configs.
    for cfg in claude_config_candidates(&ctx.home) {
        if let Some(v) = (ctx.read_json)(&cfg) {
            if mcp_has_entry(&v, "headroom") {
                sigs.push(Signal::McpEntry {
                    config_path: cfg,
                    entry_key: "mcpServers.headroom".into(),
                });
            }
        }
    }
    sigs
}

pub(crate) fn detect_gsd(ctx: &DetectionCtx) -> Vec<Signal> {
    let mut sigs = Vec::new();
    // npx-installed (`npx get-shit-done-cc`) — no global binary ever lands
    // on PATH. The footprint is the ~/.claude tree.
    // Slash-command pack: ~/.claude/commands/gsd/*.md (/gsd:help et al).
    let commands = ctx.home.join(".claude").join("commands").join("gsd");
    if commands.is_dir() {
        sigs.push(Signal::DataDir {
            path: commands,
            kind: DataKind::Telemetry,
        });
    }
    // Newer releases ship as skills instead: ~/.claude/skills/gsd-*/
    let skills = ctx.home.join(".claude").join("skills");
    if let Ok(entries) = std::fs::read_dir(&skills) {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().starts_with("gsd-") && e.path().is_dir() {
                sigs.push(Signal::DataDir {
                    path: e.path(),
                    kind: DataKind::Telemetry,
                });
                break;
            }
        }
    }
    // SHA-256 install manifest written by the installer (drives its updates).
    let manifest = ctx.home.join(".claude").join("gsd-manifest.json");
    if manifest.is_file() {
        sigs.push(Signal::DataDir {
            path: manifest,
            kind: DataKind::Telemetry,
        });
    }
    // SessionStart hooks (update check + statusline) in settings.json.
    let settings_path = ctx.home.join(".claude").join("settings.json");
    if let Some(v) = (ctx.read_json)(&settings_path) {
        for (event, matcher_groups) in iter_hook_groups(&v) {
            for (matcher, cmd) in matcher_groups {
                if cmd.contains("gsd") {
                    sigs.push(Signal::HookScript {
                        settings_path: settings_path.clone(),
                        event: event.clone(),
                        matcher: matcher.clone(),
                        command: cmd,
                    });
                }
            }
        }
    }
    sigs
}

pub(crate) fn detect_mcp_memory_keeper(ctx: &DetectionCtx) -> Vec<Signal> {
    let mut sigs = Vec::new();
    if let Some(p) = (ctx.which)("mcp-memory-keeper") {
        sigs.push(Signal::Binary {
            path: p,
            installer: BinaryInstaller::NpmGlobal {
                package: "mcp-memory-keeper".into(),
            },
        });
    }
    for cfg in claude_config_candidates(&ctx.home) {
        if let Some(v) = (ctx.read_json)(&cfg) {
            if mcp_has_entry(&v, "memory-keeper") {
                sigs.push(Signal::McpEntry {
                    config_path: cfg,
                    entry_key: "mcpServers.memory-keeper".into(),
                });
            }
        }
    }
    // Project-scoped SQLite — `<cwd>/.memory-keeper.db` is the convention.
    let db = ctx.cwd.join(".memory-keeper.db");
    if db.is_file() {
        sigs.push(Signal::DataDir {
            path: db,
            kind: DataKind::KeyValueMemory,
        });
    }
    sigs
}

/// lean-ctx (npm `lean-ctx-bin`) writes a « MANDATORY: use `ctx_read`
/// instead of Read » table into every agent's global rules — a block in the
/// files it shares, a file of its own elsewhere. Paths read on a machine it
/// was installed on (lean-ctx 3.8.4, rules v11), plus the global addresses
/// of the other agents it supports.
pub(crate) fn detect_lean_ctx(ctx: &DetectionCtx) -> Vec<Signal> {
    let mut sigs = Vec::new();
    if let Some(p) = (ctx.which)("lean-ctx") {
        let package = resolve_npm_global_package(&p).unwrap_or_else(|| "lean-ctx-bin".into());
        sigs.push(Signal::Binary {
            path: p,
            installer: BinaryInstaller::NpmGlobal { package },
        });
    }
    let h = &ctx.home;
    for shared in [
        h.join(".claude").join("CLAUDE.md"),
        h.join(".codex").join("AGENTS.md"),
        h.join(".codex").join("instructions.md"),
        h.join(".gemini").join("GEMINI.md"),
        h.join(".qwen").join("QWEN.md"),
        h.join(".config").join("opencode").join("AGENTS.md"),
        h.join(".config").join("kilo").join("AGENTS.md"),
        h.join(".factory").join("AGENTS.md"),
        h.join(".pi").join("agent").join("AGENTS.md"),
        h.join(".copilot").join("copilot-instructions.md"),
        h.join(".copilot").join("instructions.md"),
        h.join(".codeium")
            .join("windsurf")
            .join("memories")
            .join("global_rules.md"),
    ] {
        if let Some(sig) = lean_ctx_block(&shared) {
            sigs.push(sig);
        }
    }
    for own in [
        h.join(".cursor").join("rules").join("lean-ctx.mdc"),
        h.join(".gemini")
            .join("antigravity")
            .join("rules")
            .join("lean-ctx.md"),
        h.join(".claude").join("rules").join("lean-ctx.md"),
        h.join(".kiro").join("steering").join("lean-ctx.md"),
        h.join(".cline").join("rules").join("lean-ctx.md"),
        h.join(".kilo").join("rules").join("lean-ctx.md"),
        h.join(".roo").join("rules").join("lean-ctx.md"),
    ] {
        if own.is_file() {
            sigs.push(Signal::RuleFile { path: own });
        }
    }
    for cfg in claude_config_candidates(h) {
        if let Some(v) = (ctx.read_json)(&cfg) {
            if mcp_has_entry(&v, "lean-ctx") {
                sigs.push(Signal::McpEntry {
                    config_path: cfg,
                    entry_key: "mcpServers.lean-ctx".into(),
                });
            }
        }
    }
    let data = h.join(".config").join("lean-ctx");
    if data.is_dir() {
        sigs.push(Signal::DataDir {
            path: data,
            kind: DataKind::Telemetry,
        });
    }
    sigs
}

/// `# lean-ctx — …` heading (when it is there), `<!-- lean-ctx-rules-vN -->`,
/// …, `<!-- /lean-ctx -->`. The heading sits ABOVE the marker, so it opens
/// the block — left behind it would title whatever follows.
fn lean_ctx_block(path: &Path) -> Option<Signal> {
    let text = std::fs::read_to_string(path).ok()?;
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let at = lines
        .iter()
        .position(|l| l.starts_with("<!-- lean-ctx") && !l.starts_with("<!-- /lean-ctx"))?;
    let close = "<!-- /lean-ctx -->";
    if !lines[at..].contains(&close) {
        return None;
    }
    let open = match at.checked_sub(1).map(|i| lines[i]) {
        Some(prev) if prev.starts_with("# lean-ctx") => prev,
        _ => lines[at],
    };
    Some(Signal::RuleBlock {
        file: path.to_path_buf(),
        open: open.to_string(),
        close: close.to_string(),
    })
}
