//! **Find the configurations, without inventing any.**
//!
//! Fifteen agents store their MCP servers in different places, and
//! the place depends on the system. A hand-written list is exactly what
//! lets the next agent slip through, so discovery starts from
//! the ENVIRONMENT (`McpDiscoveryEnv`) and makes it testable: the same code
//! answers for all three systems without needing three machines.

/// Which OS's config-path conventions to compute. Kept as an explicit
/// value (not `cfg!`) so `mcp-proxy install` discovery can be unit-tested
/// for every platform from a single Linux CI runner — the whole point of
/// the parametrised [`McpDiscoveryEnv`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsKind {
    Linux,
    Macos,
    Windows,
}

impl OsKind {
    /// The OS this binary is running on (maps `std::env::consts::OS`).
    pub fn current() -> Self {
        match std::env::consts::OS {
            "macos" => OsKind::Macos,
            "windows" => OsKind::Windows,
            _ => OsKind::Linux,
        }
    }
}

/// The filesystem context MCP-config discovery resolves against. Every base
/// dir is explicit so a test can point them at a tempdir and mock any OS —
/// production fills them from the real platform via [`Self::current`].
#[derive(Debug, Clone)]
pub struct McpDiscoveryEnv {
    pub home: std::path::PathBuf,
    /// Project dir — where per-project configs (`.mcp.json`, `.cursor/`)
    /// live. Usually the current working directory.
    pub cwd: std::path::PathBuf,
    pub os: OsKind,
}

impl McpDiscoveryEnv {
    /// Resolve from the real platform. `None` when the home dir can't be
    /// determined (headless/misconfigured env) — callers then no-op.
    pub fn current() -> Option<Self> {
        Some(Self {
            home: crate::platform::home_dir()?,
            cwd: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            os: OsKind::current(),
        })
    }

    /// OS-specific per-user config root (XDG on Linux, Application Support
    /// on macOS, `%APPDATA%` = `AppData/Roaming` on Windows — the same
    /// `HOME`-relative shape the rest of the codebase uses for Windows).
    fn config_root(&self) -> std::path::PathBuf {
        match self.os {
            OsKind::Linux => self.home.join(".config"),
            OsKind::Macos => self.home.join("Library").join("Application Support"),
            OsKind::Windows => self.home.join("AppData").join("Roaming"),
        }
    }
}

/// A discovered MCP config file for a known agent. `path` is a candidate —
/// callers filter to the ones that actually [`exist`](Path::exists).
#[derive(Debug, Clone)]
pub struct McpConfigSite {
    pub agent: String,
    pub path: std::path::PathBuf,
}

/// Enumerate every known agent's MCP config file location for the given
/// (home, cwd, OS). Purely path arithmetic — does NOT touch disk, so the
/// caller decides whether to keep only existing files. Only agents whose
/// config is a JSON `mcpServers`/`mcp` document (the shape [`wrap_entry`]
/// understands) are listed; YAML/TOML agents (Hermes/Grok) are out of
/// scope for command-wrapping.
pub fn discover_mcp_config_sites(env: &McpDiscoveryEnv) -> Vec<McpConfigSite> {
    let h = &env.home;
    let cfg = env.config_root();
    let site = |agent: &str, path: std::path::PathBuf| McpConfigSite {
        agent: agent.to_string(),
        path,
    };
    // Codeium's home-anchored file, the same on every OS. Devin Desktop — a
    // Windsurf FORK, so literally this codebase — builds its MCP path as
    // `joinPath(homedir, ".codeium", "windsurf", "mcp_config.json")` with no
    // per-OS branch at all, and that is the directory that really appeared on
    // the Linux machine that has Devin and no Windsurf.
    let codeium_home = h.join(".codeium").join("windsurf").join("mcp_config.json");
    // Cline + VSCode-native live under the OS config-root's `Code/User`.
    let code_user = cfg.join("Code").join("User");
    // Claude Desktop's config, per OS. macOS keeps it in Application Support,
    // Windows in %APPDATA%, Linux under XDG config — `cfg` already resolves to
    // the right root on each, so one join covers all three.
    let claude_desktop = cfg.join("Claude").join("claude_desktop_config.json");
    // Qwen (and Antigravity's `~/.gemini/`) keep settings in a home dotdir on
    // EVERY OS, Windows included. Mirrors `detect::mcp_config_path`, where the
    // measurement is written down: `qwen -p` on Windows 10 creates
    // `%USERPROFILE%\.qwen`, never the dotless dir this used to wrap.
    let (gemini_home, qwen_home) = (h.join(".gemini"), h.join(".qwen"));
    // The Microsoft Store build of Claude Desktop (MSIX) does not read
    // `%APPDATA%\Claude`: Windows redirects the packaged app's AppData into
    // `%LOCALAPPDATA%\Packages\Claude_<publisher id>\LocalCache\Roaming\Claude`.
    // Measured 2026-10-06 on the Windows build VM (Claude 2.19675.0.0 from the
    // Store): the live `claude_desktop_config.json`, `config.json` and logs
    // are there, while `%APPDATA%\Claude` holds only an older install's files.
    let msix_claude: Vec<std::path::PathBuf> =
        std::fs::read_dir(h.join("AppData").join("Local").join("Packages"))
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.file_name().to_string_lossy().starts_with("Claude_"))
                    .map(|e| {
                        e.path()
                            .join("LocalCache")
                            .join("Roaming")
                            .join("Claude")
                            .join("claude_desktop_config.json")
                    })
                    .collect()
            })
            .unwrap_or_default();
    let mut sites = vec![
        // Claude Code: per-project + global.
        site("claude", env.cwd.join(".mcp.json")),
        site("claude", h.join(".claude.json")),
        // Claude DESKTOP — a different product from Claude Code, with its own
        // file, and the one MCP client most third-party servers are written
        // for. It was absent from this list.
        //
        // Measured 2026-08-18 on a macOS VM with the real app installed
        // (Claude 0.14.10, downloaded and mounted from the vendor dmg, run
        // once so it created its data dir): a third-party server planted in
        // `~/Library/Application Support/Claude/claude_desktop_config.json`
        // made `mcp-wrap install` answer "no MCP servers to wrap were found
        // across known agent configs". The desktop app has no shell and no
        // hook, so this file is the ONLY channel it can ever have, and it was
        // the one channel we did not look at.
        //
        // Confirmed on a SECOND OS the same day rather than assumed from the
        // first: the Windows build of the app (installed from the vendor's
        // `Claude-Setup-x64.exe`, run once) creates `%APPDATA%\Claude\` —
        // `config.json`, `window-state.json`, `Local State`, the usual
        // Electron set — which is exactly what `cfg.join("Claude")` resolves
        // to there. `claude_desktop_config.json` is NOT among them: the app
        // writes it only once a server is added through Settings → Developer,
        // so this is a candidate path and `mcp-wrap` wraps it only when it
        // exists. That is the difference between "the path is right" and
        // "the file is there", and only the first is being claimed.
        site("claude-desktop", claude_desktop),
        // Cursor: per-project + global dotdir.
        site("cursor", env.cwd.join(".cursor").join("mcp.json")),
        site("cursor", h.join(".cursor").join("mcp.json")),
        // Codex (OpenAI): single JSON everywhere.
        site("codex", h.join(".codex").join("config.json")),
        // Droid (Factory). This line was missing as long as `Integration.mcp`
        // said `false` on a "not verified yet"; the verification, done
        // on 2026-08-21, fits in one command: `droid mcp add probe node
        // /tmp/x.js` in a throwaway HOME creates `~/.factory/mcp.json` with the
        // standard `mcpServers` shape. droid's `PreToolUse` hook does not see
        // MCP results, so this was its only route and it was
        // closed.
        site("droid", h.join(".factory").join("mcp.json")),
        // GitHub Copilot: CLI home + the older XDG preview path.
        site("copilot", h.join(".copilot").join("mcp-config.json")),
        site("copilot", cfg.join("github-copilot").join("mcp.json")),
        // Cline is ABSENT for the same reason as OpenCode, KiloCode and
        // Hermes below: it runs a tokenade PLUGIN whose message builder folds
        // every `tool_result` block, MCP results included. It used to be
        // listed here anyway — so the one agent that had BOTH channels was the
        // one the rule was written for. The code and its own comment
        // disagreed, and the comment was right (one authority per
        // payload). Enforced by
        // `an_agent_with_a_plugin_is_not_also_mcp_wrapped`.
        // VSCode native MCP registry.
        site("vscode", code_user.join("mcp.json")),
        // Devin Desktop. Two sites, and the FIRST one is the one that
        // matters — it was found by reading the app instead of guessing from
        // its vendor.
        //
        // Devin is not a plain VSCode fork: its `product.json` carries
        // `win32AppUserModelId = "Exafunction.Windsurf"`, so it is a WINDSURF
        // fork, and Windsurf keeps MCP in Codeium's own file. The app states
        // the path itself, in a string it shows the user —
        // `"Devin configurations (~/.codeium/{0}/mcp_config.json)"` — and
        // builds it in code as
        // `joinPath(homedir, ".codeium", windsurfDirName, "mcp_config.json")`,
        // where `windsurfDirName` is `"windsurf"` for the stable quality.
        // Note `homedir`, not a per-OS config root: this path is the same on
        // all three systems.
        //
        // Confirmed on disk 2026-08-18 rather than taken from the strings:
        // the machine has Devin Desktop installed and **no Windsurf at all**,
        // and `~/.codeium/windsurf/` exists anyway — created the same minute
        // as `/usr/share/devin-desktop`, holding `user_settings.pb`, which is
        // the file the SAME code block names beside `mcp_config.json`.
        //
        site("devin-desktop", codeium_home.clone()),
        // Second site: being a VSCode descendant, Devin also carries VSCode's
        // native MCP registry (`mcpResource: … "mcp.json"` is in its bundle),
        // under its own product name. Measured on the same machine:
        // `~/.config/Devin/User/` holds `settings.json`, `languagepacks.json`,
        // `globalStorage/` and `workspaceStorage/` — the VSCode `User` layout,
        // byte for byte. Listed second because it is the inherited surface,
        // not the one Devin's own agent panel reads.
        site(
            "devin-desktop",
            cfg.join("Devin").join("User").join("mcp.json"),
        ),
        // OpenCode and KiloCode are deliberately ABSENT, though they
        // declare a perfectly wrappable JSON `mcp` key. Both run a
        // tokenade PLUGIN that folds every tool result inside the agent
        // (`tool.execute.after`), MCP results included — wrapping their
        // servers as well would put two authorities on the same bytes,
        // which is the one thing the one-authority-per-payload rule forbids.
        // Same reasoning keeps Hermes' YAML unimplemented.
        // The Qwen fork of Gemini CLI: `mcpServers` inside settings.json.
        site("qwen", qwen_home.join("settings.json")),
        // `~/.gemini/settings.json`: Gemini CLI's file (retired), which
        // `detect::mcp_config_path` still gives to Antigravity — kept under
        // ITS name, so that Antigravity does not lose a site it had.
        site("antigravity", gemini_home.join("settings.json")),
        // opencode, kilo-code, hermes and cline are DELIBERATELY absent, and
        // they were added here for one build on 2026-08-20 before
        // `an_agent_with_a_plugin_is_not_also_mcp_wrapped` refused it.
        //
        // All four run a tokenade PLUGIN, and a bridge folds every
        // `tool_result` inside the agent — MCP results included. Wrapping their
        // MCP servers as well is a second compactor on the same bytes: one
        // authority per payload, and the rollup double-count this project has already paid for.
        //
        // `detect::mcp_config_path` naming their file is not a contradiction.
        // It answers "where does this agent keep its MCP config", which an
        // operator may legitimately want to know; this list answers "where may
        // tokenade WRITE", and the plugin is the reason the two differ.
        // Antigravity keeps its OWN file, not `~/.gemini/settings.json`:
        // `~/.gemini/config/mcp_config.json`. Measured 2026-08-10 on a machine
        // with Antigravity installed — the file was there (empty, created by
        // the app), and its `language_server` binary carries both the literal
        // `mcp_config.json` and the MCP RPC handlers (`RefreshMcpServers`,
        // `McpServerSpec`). The agent is a compiled Go binary, so the schema
        // could not be read from source; it is the Codeium `mcp_config.json`
        // convention, the same one Windsurf uses and that `wrap_entry` already
        // parses. Stated rather than implied: the PATH is measured, the SHAPE
        // is inherited from the sibling product by the same vendor.
        site(
            "antigravity",
            gemini_home.join("config").join("mcp_config.json"),
        ),
        // TOML agents — handled by the line-surgical path in
        // `process_named_config`, not by `wrap_entry`.
        site("codex", h.join(".codex").join("config.toml")),
        site("grok", h.join(".grok").join("config.toml")),
    ];
    sites.extend(msix_claude.into_iter().map(|p| site("claude-desktop", p)));
    sites
}

/// MCP files of agents REMOVED from the product (Windsurf outside Linux, Reasonix;
/// Gemini CLI's stays an Antigravity site).
///
/// Never wrapped again — those agents no longer exist — but an
/// earlier install may have written there: without these paths, `uninstall` and
/// `mcp-proxy uninstall` would leave behind entries that launch
/// `tokenade`. Cleanup only.
fn legacy_mcp_config_sites(env: &McpDiscoveryEnv) -> Vec<std::path::PathBuf> {
    let (h, cfg) = (&env.home, env.config_root());
    let mut v = vec![
        h.join(".reasonix").join("config.toml"),
        cfg.join("reasonix").join("config.toml"),
    ];
    match env.os {
        OsKind::Macos => v.push(cfg.join("Windsurf").join("User").join("mcp_config.json")),
        OsKind::Windows => v.push(cfg.join("Codeium").join("windsurf").join("mcp_config.json")),
        OsKind::Linux => {}
    }
    v
}

/// Does tokenade know where this agent keeps its MCP servers?
///
/// The static half of `discover_mcp_config_sites` — agent name only, no
/// filesystem — so `detect::Agent::coverage()` can answer "rank 3 is available
/// here" without turning a pure function into a probe. Derived from the site
/// table itself rather than a second list, because a second list is how the
/// two would drift.
///
/// Deliberately platform-independent: it asks whether the PRODUCT has a site
/// for this agent, not whether this machine does. `McpDiscoveryEnv` needs a
/// home and a cwd it will never read here, so it gets placeholders.
/// The agent whose MCP config lives at `path`, if discovery knows it.
pub fn agent_for_config_path(path: &std::path::Path) -> Option<String> {
    let env = McpDiscoveryEnv::current()?;
    let canon = |p: &std::path::Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let want = canon(path);
    discover_mcp_config_sites(&env)
        .into_iter()
        .find(|s| canon(&s.path) == want)
        .map(|s| s.agent)
}

pub fn agent_has_known_mcp_site(agent: &str) -> bool {
    let env = McpDiscoveryEnv {
        home: std::path::PathBuf::from("/"),
        cwd: std::path::PathBuf::from("/"),
        os: OsKind::Linux,
    };
    discover_mcp_config_sites(&env)
        .iter()
        .any(|s| s.agent == agent)
}

/// Existing MCP config files across every known agent for the current
/// platform. The shared entry point for the bulk wrap/unwrap passes and
/// the targeted `mcp-proxy install`/`uninstall`.
/// The MCP configuration files of ONE agent, those that exist.
///
/// **Why one agent, and not all.** As soon as an agent folds its MCP
/// results through a hook, ALSO wrapping its servers behind the proxy folds
/// twice: one more process per server for a second pass that
/// yields nothing, and a gain counted twice on the dashboard. The proxy and
/// the hook are two routes to the same fold, never two stages of the same
/// route.
pub fn config_paths_for_agent(agent: &str) -> Vec<std::path::PathBuf> {
    let Some(env) = McpDiscoveryEnv::current() else {
        return Vec::new();
    };
    discover_mcp_config_sites(&env)
        .into_iter()
        .filter(|s| s.agent == agent)
        .map(|s| s.path)
        .filter(|p| p.exists())
        .collect()
}

/// [`config_paths`] plus the files of retired agents — for the passes
/// that UNDO (unwrap, removal of our entry), never for those that do.
pub(super) fn cleanup_config_paths() -> Vec<std::path::PathBuf> {
    let Some(env) = McpDiscoveryEnv::current() else {
        return Vec::new();
    };
    let mut paths = config_paths();
    for p in legacy_mcp_config_sites(&env) {
        if p.exists() && !paths.contains(&p) {
            paths.push(p);
        }
    }
    paths
}

pub(super) fn config_paths() -> Vec<std::path::PathBuf> {
    let Some(env) = McpDiscoveryEnv::current() else {
        return Vec::new();
    };
    discover_mcp_config_sites(&env)
        .into_iter()
        .map(|s| s.path)
        .filter(|p| p.exists())
        .collect()
}
