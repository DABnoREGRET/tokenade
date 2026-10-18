//! **Where each agent stores its MCP configuration.**
//!
//! Fifteen agents, three systems, two formats. The function takes the system
//! as an ARGUMENT rather than reading it: that is what lets the
//! Windows and macOS paths be exercised from a Linux machine, and a path table
//! no test walks is wrong by the next agent.

use super::*;

impl Agent {
    pub fn integration(&self) -> Integration {
        match self {
            Agent::ClaudeCode => Integration {
                mcp: true,
                shell_hook: true,
                hook_name: Some("PostToolUse"),
            },
            // Codex: `install_codex_passive_hooks` wires FIVE lifecycle
            // hooks into `~/.codex/hooks.json` (SessionStart,
            // UserPromptSubmit, PreToolUse, PostToolUse, PreCompact) and
            // Codex loads them — a live `~/.codex/config.toml` carries a
            // `trusted_hash` for each. This arm used to sit in the
            // MCP-only group below, declaring `shell_hook: false`, which
            // is the opposite of what ships. Nothing consumed the field
            // (only `.mcp` is read) so the lie was free until a
            // compatibility matrix read it.
            Agent::Codex => Integration {
                mcp: true,
                shell_hook: true,
                hook_name: Some("PostToolUse"),
            },
            // Cursor: `install_cursor_hooks` writes a `preToolUse` entry
            // into `~/.cursor/hooks.json` that rewrites the shell command
            // through tokenade. It is filed under `PLUGIN_INSTALLS` purely
            // because its function signature matches that table's type —
            // and that misfiling is why the hook guard, which walks
            // HOOK_INSTALLS, never noticed Cursor declaring `shell_hook:
            // false`. A table grouped by SIGNATURE is not a table grouped
            // by CHANNEL.
            Agent::Cursor => Integration {
                mcp: true,
                shell_hook: true,
                hook_name: Some("PreToolUse"),
            },
            Agent::DevinDesktop
            | Agent::OpenCode
            | Agent::KiloCode
            | Agent::Hermes
            | Agent::Cline => Integration {
                mcp: true,
                shell_hook: false,
                hook_name: None,
            },
            // OpenClaw 2026.1.30 documents `mcp.servers.<name>` in
            // docs.openclaw.ai/cli/mcp, but the actual config schema in
            // the published CLI rejects `mcp` at root with
            // `Unrecognized key: "mcp"`. Until the runtime catches up
            // we fall back to wrap-only — same posture as Aider —
            // rather than write a config file OpenClaw immediately
            // refuses to load.
            Agent::OpenClaw => Integration {
                mcp: false,
                shell_hook: false,
                hook_name: None,
            },
            // Copilot: same story as Codex — `install_copilot_hooks` is a
            // row of `HOOK_INSTALLS`, so the CLI hooks ship, and the
            // declaration said otherwise.
            Agent::Copilot => Integration {
                mcp: true, // preview / draft as of May 2026
                shell_hook: true,
                hook_name: Some("PostToolUse"),
            },
            // Aider has no MCP server contract today. The installer
            // records the agent as present (so the dashboard knows) but
            // skips the MCP write step and surfaces the `tokenade wrap`
            // path instead. If aider adds MCP later, flip `mcp: true` and
            // add a config_path arm.
            Agent::Aider => Integration {
                mcp: false,
                shell_hook: false,
                hook_name: None,
            },
            // Pi (earendil-works): no MCP config contract; integration is the
            // shell proxy (`!` commands) + the AGENTS.md scaffold it reads.
            // Wrap-only, same posture as Aider.
            Agent::Pi => Integration {
                mcp: false,
                shell_hook: false,
                hook_name: None,
            },
            Agent::Qwen => Integration {
                mcp: true,
                shell_hook: true,
                hook_name: Some("BeforeTool"),
            },
            // Two products behind one name. The Antigravity IDE has no shell
            // hook; its CLI, `agy`, does — a native `PreToolUse` whose
            // `overwrite` rewrites the tool call's arguments before it runs.
            // Contract read out of the shipped `agy` binary and the config root
            // probed on a live one (2026-08-25); the install gates on
            // `~/.gemini/config/`, the CLI's own directory, so an IDE-only
            // machine is untouched.
            Agent::Antigravity => Integration {
                mcp: true,
                shell_hook: true,
                hook_name: Some("PreToolUse"),
            },
            Agent::Grok => Integration {
                mcp: true,
                shell_hook: true,
                hook_name: Some("PreToolUse"),
            },
            // T3 Code: orchestrator — no config surface of its own to wire;
            // coverage flows through the agents it drives (Claude hooks via
            // settingSources, Cursor/Grok/OpenCode via their own wiring).
            Agent::T3Code => Integration {
                mcp: false,
                shell_hook: false,
                hook_name: None,
            },
            // Droid: hooks verified against factory.ai docs (hooks-guide +
            // hooks-reference, 2026-07). The MCP surface has been VERIFIED since
            // 2026-08-21: `droid mcp add probe node /tmp/x.js` in a throwaway
            // HOME creates `~/.factory/mcp.json` with the standard `mcpServers`
            // shape (`{type, command, args, disabled}`). It stayed
            // `false` all that time on a "not verified yet" that
            // nobody had gone to verify, and droid's MCP cell came out
            // `Partial` for lack of a file to wrap.
            Agent::Droid => Integration {
                mcp: true,
                shell_hook: true,
                hook_name: Some("PreToolUse"),
            },
            // A VS Code extension is not a process tokenade can put a hook
            // in front of, and Copilot exposes no MCP surface of its own.
            // Both false, so `coverage()` answers `Uncovered` — the row
            // exists for rank 4 and must not read as a claim above it.
            Agent::CopilotVsCode => Integration {
                mcp: false,
                shell_hook: false,
                hook_name: None,
            },
            // Odysseus MCP is UI-configured only — there is no config file
            // on disk that tokenade can write into.
            Agent::Odysseus => Integration {
                mcp: false,
                shell_hook: false,
                hook_name: None,
            },
            Agent::Unknown => Integration {
                mcp: false,
                shell_hook: false,
                hook_name: None,
            },
        }
    }

    /// Path (or `None`) where this agent's MCP server registration lives.
    /// Project-relative paths are resolved against `root`; `~`-prefixed
    /// paths against the user's home dir. Returns `None` for `Unknown`.
    ///
    /// **OS-aware**: many agents store their config in different places on
    /// macOS / Linux / Windows. We branch on `cfg!(target_os)` to pick
    /// the right canonical path. When the convention is the same across
    /// platforms we leave a single arm. See `docs/CROSS-PLATFORM.md` for
    /// the full matrix and the verification status of each entry.
    /// Where this agent keeps its MCP config, ON A GIVEN OS.
    ///
    /// The OS is a PARAMETER, not `cfg!`. It used to be compile-time, which
    /// meant a Linux build could not answer "where does Cline keep its
    /// config on macOS" — and `mcp_wrap::discover_mcp_config_sites`, which
    /// takes the OS at runtime, could therefore never be checked against
    /// this module off-host. Two path tables for the same files, no way to
    /// compare them: exactly how the macOS/Windows arms drifted unnoticed.
    pub fn mcp_config_path_on(
        &self,
        os: crate::mcp_wrap::OsKind,
        root: &Path,
        home: Option<&Path>,
    ) -> Option<PathBuf> {
        use crate::mcp_wrap::OsKind;
        let _ = &os;
        let rel: &str = match self {
            // Claude Code uses the same POSIX-style path on every OS (the
            // Anthropic CLI does its own home resolution).
            Agent::T3Code => return None, // orchestrator — no own MCP surface
            Agent::ClaudeCode => ".mcp.json",
            // Measured 2026-08-21: `droid mcp add probe node /tmp/x.js` in a
            // throwaway HOME writes here, standard `mcpServers` shape.
            Agent::Droid => "~/.factory/mcp.json",
            // Cursor: per-project on macOS/Linux, per-user-globalStorage
            // on Windows. The per-project file is the one users version
            // alongside their code, so we keep that as default everywhere
            // and let `--global` override.
            Agent::Cursor => ".cursor/mcp.json",
            // Codex (OpenAI): `~/.codex/config.toml`, `[mcp_servers.<name>]`.
            // This used to say `config.json`, which Codex does not read —
            // measured on codex-cli 0.144.5 in an isolated CODEX_HOME: a
            // `{"mcpServers":{…}}` config.json yields "No MCP servers
            // configured yet", the same entry as `[mcp_servers.x]` in
            // config.toml is listed and enabled.
            Agent::Codex => "~/.codex/config.toml",
            // Devin Desktop (a Windsurf fork): `joinPath(homedir, ".codeium",
            // "windsurf", "mcp_config.json")` in its bundle, with no per-OS
            // branch — the same path on all three systems.
            Agent::DevinDesktop => "~/.codeium/windsurf/mcp_config.json",
            // OpenCode (sst): MCP lives inside the main config file as a
            // top-level `mcp` key. Go's os.UserConfigDir() returns:
            //   Linux   → ~/.config/
            //   macOS   → ~/Library/Application Support/
            //   Windows → %AppData% (AppData/Roaming relative to USERPROFILE)
            Agent::OpenCode => {
                if os == OsKind::Macos {
                    "~/Library/Application Support/opencode/opencode.json"
                } else if os == OsKind::Windows {
                    "~/AppData/Roaming/opencode/opencode.json"
                } else {
                    "~/.config/opencode/opencode.json"
                }
            }
            // GitHub Copilot CLI: `~/.copilot/mcp-config.json` (default;
            // overridable via the COPILOT_HOME env var). GitHub uses
            // os.UserHomeDir() so the path is uniform across all OSes.
            // Source: docs.github.com/en/copilot/…/add-mcp-servers
            Agent::Copilot => "~/.copilot/mcp-config.json",
            // Kilo Code CLI (`@kilocode/cli`) — fork of SST OpenCode;
            // follows the same os.UserConfigDir() convention.
            //   Linux   → ~/.config/kilo/kilo.json
            //   macOS   → ~/Library/Application Support/kilo/kilo.json
            //   Windows → AppData/Roaming/kilo/kilo.json
            Agent::KiloCode => {
                if os == OsKind::Macos {
                    "~/Library/Application Support/kilo/kilo.json"
                } else if os == OsKind::Windows {
                    "~/AppData/Roaming/kilo/kilo.json"
                } else {
                    "~/.config/kilo/kilo.json"
                }
            }
            // Hermes (NousResearch): `~/.hermes/config.yaml` is documented
            // as the canonical path on all platforms; the CLI uses
            // os.UserHomeDir() + ".hermes/".
            Agent::Hermes => "~/.hermes/config.yaml",
            // Cline (VSCode extension `saoudrizwan.claude-dev`). MCP
            // config lives in VSCode globalStorage; the path is OS-
            // dependent because VSCode's User dir lives in different
            // places per platform.
            Agent::Cline => {
                if os == OsKind::Macos {
                    "~/Library/Application Support/Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json"
                } else if os == OsKind::Windows {
                    // Resolved against HOME. On Windows the canonical
                    // location is %APPDATA%\Code\... — we fall back to
                    // the user profile + AppData/Roaming path which
                    // matches `os == OsKind::Windows` HOME → USERPROFILE.
                    "~/AppData/Roaming/Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json"
                } else {
                    "~/.config/Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json"
                }
            }
            // Aider has no MCP server today — the installer records
            // presence + surfaces the `tokenade wrap aider …` recipe
            // instead of writing a config file.
            Agent::Aider => return None,
            // Pi has no MCP config contract today — wrap-only (see Integration).
            Agent::Pi => return None,
            // Odysseus MCP is registered via the Odysseus web UI — there
            // is no config file on disk that the installer can write.
            // Copilot's VS Code extension registers MCP servers through
            // VS Code's own `mcp.json`, which is VS Code's surface and not
            // Copilot's — nothing here writes it.
            Agent::CopilotVsCode => return None,
            Agent::Odysseus => return None,
            // OpenClaw 2026.1.30 docs reference `~/.openclaw/openclaw.json`
            // with `mcp.servers.<name>`, but the shipped CLI rejects the
            // `mcp` key at root. Skipped here until upstream lands MCP —
            // the installer will surface `tokenade wrap openclaw …` as
            // the wrap-only fallback (see `Integration` for OpenClaw).
            Agent::OpenClaw => return None,
            // Antigravity / Qwen Code: `~/.gemini/` and `~/.qwen/`, WITH the
            // dot, on every OS including Windows.
            //
            // This said the dot is dropped on Windows and cited
            // google-gemini.github.io/gemini-cli/docs/get-started/configuration.html
            // for it. Measured 2026-08-18 on a Windows 10 box: running
            // `gemini --version` (0.55.1) creates `%USERPROFILE%\.gemini`, and
            // one `qwen -p` (0.21.13) creates `%USERPROFILE%\.qwen`. Neither
            // dotless directory is ever created. So `mcp-wrap install` was
            // writing a Windows user's MCP entry into a file their agent does
            // not read, and the presence marker for Gemini matched nothing at
            // all — the agent was undetectable on Windows while `install
            // --only gemini-cli`, pointed at it by hand, wrote its hooks to
            // the dotted directory and worked.
            //
            // Documentation that disagrees with the running binary loses.
            Agent::Antigravity => "~/.gemini/settings.json",
            Agent::Qwen => "~/.qwen/settings.json",
            // Grok (xAI): `~/.grok/config.toml` on Linux and macOS.
            // Windows path is undocumented (beta as of May 2026); we use
            // the same dotdir convention and will correct it once xAI
            // ships a stable Windows installer.
            Agent::Grok => "~/.grok/config.toml",
            Agent::Unknown => return None,
        };
        Some(resolve(rel, root, home))
    }

    /// Host-OS shorthand for [`Agent::mcp_config_path_on`].
    pub fn mcp_config_path(&self, root: &Path, home: Option<&Path>) -> Option<PathBuf> {
        self.mcp_config_path_on(crate::mcp_wrap::OsKind::current(), root, home)
    }

    /// Wire format the agent's MCP config file uses. The installer reads
    /// this to decide which serialisation path to take.
    pub fn mcp_config_format(&self) -> McpConfigFormat {
        match self {
            // Kilo Code CLI is forked from SST OpenCode; both read
            // `config.mcp.<name>` per the `@kilocode/cli` 7.2.1 binary.
            Agent::OpenCode | Agent::KiloCode => McpConfigFormat::JsonOpenCodeMcp,
            Agent::Hermes => McpConfigFormat::YamlHermesMcpServers,
            Agent::OpenClaw => McpConfigFormat::JsonOpenClawMcp,
            // Antigravity (~/.gemini/settings.json) and Qwen write `trust: true`
            // to auto-approve tool calls.
            Agent::Antigravity | Agent::Qwen => McpConfigFormat::JsonGeminiSettings,
            Agent::Codex | Agent::Grok => McpConfigFormat::TomlMcpServers,
            _ => McpConfigFormat::JsonMcpServers,
        }
    }
}
