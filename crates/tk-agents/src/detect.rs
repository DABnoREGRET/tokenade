//! Auto-detect which AI coding agent(s) are present on this machine.
//!
//! The runtime detection ([`from_env`] / [`detect`]) tells us **who is
//! invoking us right now** — useful for adaptive behaviour during an
//! in-progress session. The installer detection ([`detect_all_present`])
//! tells us **which agents could benefit from STS at all** — by inspecting
//! the user's home directory for marker files of each known agent. The
//! one-command installer registers STS with every agent it finds.
//!
//! Per-agent integration capabilities live in [`Agent::integration`]:
//!
//! - **MCP**: every supported agent gets STS's MCP server registered in
//!   their config. This is the universal channel — any compactor reachable
//!   from MCP works.
//! - **Shell hook**: two agents expose a hook contract that lets STS
//!   intercept Bash output — Claude Code (PreToolUse/PostToolUse) and
//!   Gemini CLI (its tool-call hook, which STS uses to rewrite the command
//!   to `tokenade wrap`). When the hook is wired the user gets transparent
//!   compression with no `tokenade proxy` ceremony. Every other agent is
//!   wrap-only (manual `tokenade wrap`).
//!
//! ## Which agents are covered
//!
//! Deliberately NOT a table here. A hand-written matrix in this header is a
//! third source of truth next to [`Agent::integration`] and the installer
//! tables in tokenade-cli, and it went stale exactly the way the other two
//! did: it still showed Copilot as hook-less, and OpenCode/KiloCode/Hermes
//! at config paths the code stopped using.
//!
//! Ask the code instead:
//!
//! - [`Agent::coverage`] — what actually compacts for an agent today
//!   (`tokenade detect` prints it);
//! - `HOOK_INSTALLS` / `BEFORE_TOOL_INSTALLS` / `PLUGIN_INSTALLS` in
//!   `tokenade-cli::install_cli` — the tables that do the wiring, pinned to
//!   the declarations here by tests;
//! - [`Agent::mcp_config_path_on`] — per-OS config locations.
//!
//! Note the MCP column that used to be here described a channel the product
//! no longer writes: since the zero-MCP pivot, `Integration::mcp` says what
//! an agent *could* accept, not what tokenade installs.

use std::path::{Path, PathBuf};

// The agent's identity stays here; what it CAN DO, where it STORES its
// configuration, how we know it is THERE, and how we know it is the one
// calling us, are four distinct questions.
mod capabilities;
mod config_sites;
mod from_env;
mod presence;
pub use from_env::*;

/// Every agent STS knows how to install into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub enum Agent {
    /// Anthropic Claude Code.
    ClaudeCode,
    /// Cursor IDE.
    Cursor,
    /// OpenAI Codex CLI.
    Codex,
    /// GitHub Copilot.
    Copilot,
    /// Kilo Code agent.
    KiloCode,
    /// Devin Desktop — the Windsurf (Codeium) fork, which kept its paths:
    /// `~/.codeium/windsurf/` (MCP, `memories/global_rules.md`), `.windsurfrules`
    /// and `AGENTS.md` on the project side. A VS Code fork, local terminal (node-pty).
    DevinDesktop,
    /// SST OpenCode (open-source successor to several earlier projects).
    OpenCode,
    /// Hermes / OpenCLI / other generic MCP-speaking agents — handled via
    /// a conservative best-effort config path.
    Hermes,
    /// Cline — VSCode extension (saoudrizwan.claude-dev). MCP config lives in
    /// VSCode globalStorage under the extension id.
    Cline,
    /// Aider — Python CLI pair-programmer. No native MCP today; the
    /// integration is via `tokenade wrap aider …` (and a marker so the
    /// installer can surface the wrap suggestion).
    Aider,
    /// Pi — earendil-works `@earendil-works/pi-coding-agent`. CLI coding
    /// agent: reads `AGENTS.md`, runs shell `!` commands (so compaction +
    /// gains flow through the shell proxy like any CLI agent), saves sessions
    /// as JSONL under `~/.pi/agent/sessions/`. No MCP config contract today →
    /// wrap-only, same posture as Aider.
    Pi,
    /// OpenClaw — multi-IDE coding agent. MCP-speaking; config path is
    /// best-effort (`~/.config/openclaw/mcp.json`) and overridable.
    OpenClaw,
    /// Qwen Code (`qwen` binary) — Alibaba's hard fork of Gemini CLI. Same
    /// `settings.json` schema and identical tool names (`run_shell_command`,
    /// `read_file`, `read_many_files`), just under `~/.qwen/` instead of
    /// `~/.gemini/`. MCP via `~/.qwen/settings.json` (`trust: true`); shell
    /// hook via the same `BeforeTool` contract, reusing the `gemini-hook`
    /// handler. Hook is best-effort: a fork that predates Gemini's hooks
    /// feature simply ignores the (harmless) settings entry.
    Qwen,
    /// Google Antigravity — Electron IDE built on Gemini infrastructure.
    /// MCP via `~/.gemini/settings.json` (`trust: true`).
    Antigravity,
    /// xAI Grok CLI — `grok` binary at `~/.grok/bin/grok`. MCP via
    /// `~/.grok/config.toml` under `[mcp_servers.<name>]` TOML sections.
    Grok,
    /// T3 Code (pingdotgg) — web GUI ORCHESTRATOR that drives existing
    /// agents (Claude via the Agent SDK with settingSources user/project/
    /// local, Codex app-server, Cursor CLI, OpenCode, Grok). Verified in
    /// source (2026-07): its Claude adapter spawns the real `claude` with
    /// user settings loaded, so tokenade's Claude hooks apply UNCHANGED —
    /// the wiring action for T3 is "ensure the Claude hooks exist", plus
    /// per-agent wiring for the other drivers it fronts. Caveat: a T3
    /// provider instance configured with a CUSTOM Claude homePath is an
    /// isolated environment — run `HOME=<that path> tokenade install` to
    /// cover it.
    T3Code,
    /// Factory Droid CLI — `droid` terminal agent (factory.ai). Exposes a
    /// Claude-Code-compatible hooks system in `~/.factory/hooks.json`
    /// (PreToolUse/PostToolUse/SessionStart/…, snake_case stdin payload,
    /// `hookSpecificOutput.updatedInput` honored). Shell tool is named
    /// `Execute` (not `Bash`).
    Droid,
    /// GitHub Copilot inside VS Code — the EXTENSION, not the `copilot` CLI.
    ///
    /// A separate agent because it is a separate product with a separate
    /// config file: the CLI reads `~/.copilot/settings.json`, the extension
    /// reads VS Code's own `settings.json`. Nothing tokenade writes for one
    /// is read by the other.
    ///
    /// Covered at rank 4 only, and deliberately: a VS Code extension exposes
    /// no shell hook and no MCP surface tokenade can write into, so
    /// `coverage()` stays `Uncovered` and the installer must not claim
    /// otherwise. What it DOES expose is a base-URL override, which is the
    /// whole of what rank 4 needs — see `llm_proxy::agent_matrix`.
    CopilotVsCode,
    /// Odysseus AI — PewDiePie's self-hosted workspace. MCP is configured via
    /// the Odysseus web UI (no config file on disk). Detected via
    /// `ODYSSEUS_DATA_DIR` env var.
    Odysseus,
    /// Unknown / none detected.
    Unknown,
}

/// What integration channels an agent supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Integration {
    /// Can STS register an MCP server in this agent's config?
    pub mcp: bool,
    /// Does this agent expose a shell hook that STS can wire?
    pub shell_hook: bool,
    /// Human-readable name of the agent's hook contract (when applicable).
    pub hook_name: Option<&'static str>,
}

/// What actually compacts for this agent TODAY.
///
/// `Integration` answers "what could we register", which stopped being the
/// same question at the zero-MCP pivot: the product no longer writes MCP
/// config for anyone, so an `mcp: true` agent can be entirely uncovered
/// while a "wrap-only" one is covered by a plugin. This enum answers the
/// question a user actually asks — *is my output being compacted, and by
/// what* — and both of its live arms are pinned to their installer tables
/// by tests in tokenade-cli.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    /// Lifecycle hooks are wired; `&str` is the event we compact on.
    Hook(&'static str),
    /// A tool-result compaction plugin is installed in the agent.
    Plugin,
    /// Orchestrator: coverage comes from the agents it drives.
    ViaDrivenAgents,
    /// No hook and no plugin, but tokenade knows where this agent keeps its
    /// MCP servers and can route them through the compacting proxy — rank 3
    /// of the coverage ladder (hook, plugin, MCP proxy, manual).
    ///
    /// Distinct from `Uncovered` because the two call for opposite gestures.
    /// `Uncovered` says "compact a command by hand"; this says "one command
    /// wires a whole channel". Reporting the second as the first is the
    /// failure to avoid — a channel that exists, is not offered, and
    /// shows up as a zero.
    McpWrappable,
    /// No hook, no plugin, no MCP site — but the agent runs its commands
    /// through a shell, and the universal shell proxy (rc files plus
    /// `$BASH_ENV` for non-interactive `bash -c`) folds their output without
    /// the agent knowing tokenade exists. Rank 2 of the ladder.
    ///
    /// This variant exists because the binary Covered/Uncovered forced the
    /// product to say two opposite things about the same agent: `coverage()`
    /// reported aider as `Uncovered` ("nothing automatic, wrap by hand") while
    /// `install` told the very same user "it's wired through the universal
    /// shell proxy instead". Both sentences shipped, and one of them had to be
    /// false. Neither was: aider's `/run` IS folded automatically, and its
    /// `/add` — which drops a whole file into the conversation without going
    /// near a shell — is not.
    ///
    /// So the honest statement is PARTIAL, and the type now carries it. Do not
    /// widen this to "any CLI agent": it is a claim that a specific agent's
    /// command path goes through a shell, and it belongs to an agent only once
    /// somebody has checked that it does.
    ShellProxy,
    /// Nothing automatic — `tokenade wrap <cmd>` only.
    Uncovered,
    /// We could not tell which agent is running. NOT the same as uncovered:
    /// the agent may well be wired, we simply have no name to check against.
    AgentUnknown,
}

impl Coverage {
    /// One line, plain enough to print at a user without further context.
    /// An uncovered agent says so and says what to do instead: a bare
    /// "0" on a dashboard reads as a broken install, which is the whole
    /// reason this type exists.
    pub fn describe(&self) -> String {
        match self {
            Coverage::Hook(ev) => {
                format!("hooks ({ev}) — command output is compacted and the saving is measured")
            }
            Coverage::Plugin => "plugin — tool results are compacted inside the agent".to_string(),
            Coverage::ViaDrivenAgents => {
                "orchestrator — coverage comes from the agents it drives; wire those".to_string()
            }
            Coverage::McpWrappable => {
                "no hook or plugin — but this agent's MCP servers can be routed through \
                 the compacting proxy: run `tokenade mcp-wrap install`"
                    .to_string()
            }
            Coverage::ShellProxy => {
                "no hook or plugin — but this agent runs its commands through a shell, so \
                 the universal shell proxy already compacts their output; anything it reads \
                 WITHOUT a shell is not covered"
                    .to_string()
            }
            Coverage::Uncovered => {
                "not wired — run `tokenade wrap <cmd>` to compact a command by hand".to_string()
            }
            Coverage::AgentUnknown => "no agent recognised here — coverage cannot be reported; \
                 `tokenade wrap <cmd>` compacts a command by hand either way"
                .to_string(),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Coverage::Hook(_) => "hook",
            Coverage::Plugin => "plugin",
            Coverage::ViaDrivenAgents => "via-driven-agents",
            Coverage::McpWrappable => "mcp-wrappable",
            Coverage::ShellProxy => "shell-proxy",
            Coverage::Uncovered => "uncovered",
            Coverage::AgentUnknown => "agent-unknown",
        }
    }
}

/// Wire format the agent's config file uses + which top-level key holds
/// the MCP server map + the per-server schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpConfigFormat {
    /// `{ "mcpServers": { "<name>": { "command": "tokenade", "args": ["mcp"] } } }`
    /// — Claude Code, Cursor, Codex, Devin Desktop, Copilot, Kilo Code.
    JsonMcpServers,
    /// `{ "mcp": { "<name>": { "type": "local", "command": ["tokenade", "mcp"], "enabled": true } } }`
    /// — OpenCode (sst). Note: `command` is an *array*, not a string.
    JsonOpenCodeMcp,
    /// `mcp_servers:\n  <name>:\n    command: tokenade\n    args: [mcp]`
    /// — Hermes (NousResearch). YAML.
    YamlHermesMcpServers,
    /// `{ "mcp": { "servers": { "<name>": { "command": "tokenade", "args": ["mcp"] } } } }`
    /// — OpenClaw. Same as JsonMcpServers but the server map lives under
    /// `mcp.servers` rather than the top-level `mcpServers` key.
    JsonOpenClawMcp,
    /// `{ "mcpServers": { "<name>": { "command": "tokenade", "args": ["mcp"], "trust": true } } }`
    /// — Qwen, Antigravity (Gemini CLI's format). Same shape as JsonMcpServers but with an extra `trust: true`
    /// field required to auto-approve tool calls without a per-call prompt.
    JsonGeminiSettings,
    /// `[mcp_servers.<name>]\ncommand = "tokenade"\nargs = ["mcp"]\nenabled = true`
    /// — Grok CLI (`~/.grok/config.toml`). Standard TOML section format.
    /// TOML with `[mcp_servers.<name>]` sections. Shared by Grok and
    /// Codex — measured on codex-cli 0.144.5: `codex mcp add` writes
    /// exactly this shape, and a `mcpServers` JSON file next to it is
    /// ignored (`codex mcp list` reports none).
    TomlMcpServers,
}

impl Agent {
    /// Short, stable identifier — used in `tokenade gain` logs and the
    /// dashboard.
    pub fn as_str(&self) -> &'static str {
        match self {
            Agent::ClaudeCode => "claude-code",
            Agent::Cursor => "cursor",
            Agent::Codex => "codex",
            Agent::Copilot => "copilot",
            Agent::KiloCode => "kilo-code",
            Agent::DevinDesktop => "devin-desktop",
            Agent::OpenCode => "opencode",
            Agent::Hermes => "hermes",
            Agent::Cline => "cline",
            Agent::Aider => "aider",
            Agent::Pi => "pi",
            Agent::OpenClaw => "openclaw",
            Agent::Qwen => "qwen-code",
            Agent::Antigravity => "antigravity",
            Agent::Grok => "grok",
            Agent::T3Code => "t3-code",
            Agent::Droid => "droid",
            Agent::CopilotVsCode => "copilot-vscode",
            Agent::Odysseus => "odysseus",
            Agent::Unknown => "unknown",
        }
    }

    /// Inverse of [`as_str`](Self::as_str): parse a canonical slug back to an
    /// `Agent`. Returns `None` for an unrecognized/empty slug so callers can
    /// fall through to environment/filesystem sniffing. Reuses `as_str` so the
    /// mapping stays single-sourced.
    pub fn from_slug(s: &str) -> Option<Agent> {
        if s.is_empty() {
            return None;
        }
        Agent::all().iter().copied().find(|a| a.as_str() == s)
    }
}

#[cfg(test)]
include!("detect_tests.rs");
