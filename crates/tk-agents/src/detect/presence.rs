//! **Is this agent INSTALLED on this machine?**
//!
//! Presence markers — a directory, a settings file — rather
//! than a list. Being wrong one way offers an install to someone
//! who does not have the agent; the other way, we offer nothing and they believe
//! tokenade does not support it.

use super::*;

impl Agent {
    /// Marker files / dirs that prove this agent is installed on the
    /// machine. Used by [`detect_all_present`].
    pub fn presence_markers(&self, home: Option<&Path>) -> Vec<PathBuf> {
        let h = home;
        match self {
            // The FIRST marker is the one the user may have moved: Claude Code
            // reads `CLAUDE_CONFIG_DIR` when it is set and `~/.claude` only
            // otherwise. Looking solely at `~/.claude` answered "no AI coding
            // agent detected on this machine" to somebody running Claude Code —
            // measured 2026-08-31, and `install` then wrote nothing at all.
            Agent::ClaudeCode => vec![
                match h {
                    Some(home) => crate::platform::claude_config_dir_for(home),
                    None => resolve("~/.claude", Path::new(""), h),
                },
                resolve("~/.config/claude-code", Path::new(""), h),
            ],
            Agent::Cursor => vec![
                resolve("~/.cursor", Path::new(""), h),
                resolve("~/Library/Application Support/Cursor", Path::new(""), h),
                resolve("~/.config/Cursor", Path::new(""), h),
            ],
            Agent::Codex => vec![resolve("~/.codex", Path::new(""), h)],
            // Devin Desktop installs outside HOME (`/usr/share/devin-desktop`):
            // its markers are its config folders. `~/.codeium/windsurf`
            // is created by Devin even without Windsurf (a fork that kept the name).
            Agent::DevinDesktop => vec![
                resolve("~/.config/Devin", Path::new(""), h),
                resolve("~/.codeium/windsurf", Path::new(""), h),
                resolve("~/Library/Application Support/Devin", Path::new(""), h),
                // Windows: %APPDATA%\Devin
                resolve("~/AppData/Roaming/Devin", Path::new(""), h),
            ],
            Agent::OpenCode => vec![
                resolve("~/.config/opencode", Path::new(""), h),
                resolve("~/.opencode", Path::new(""), h),
                // macOS
                resolve("~/Library/Application Support/opencode", Path::new(""), h),
                // Windows: %APPDATA%\opencode
                resolve("~/AppData/Roaming/opencode", Path::new(""), h),
            ],
            // VS Code itself, because Copilot Chat ships INSIDE it: measured
            // on VS Code 1.134, `resources/app/extensions/copilot` is the
            // built-in `GitHub.copilot-chat` 0.62.0. So a VS Code install is
            // a Copilot install, and the marketplace extension only adds the
            // completions half. The per-user config root is the same three
            // roots every VS Code derivative uses.
            Agent::CopilotVsCode => vec![
                resolve("~/.config/Code/User", Path::new(""), h),
                resolve("~/Library/Application Support/Code/User", Path::new(""), h),
                resolve("~/AppData/Roaming/Code/User", Path::new(""), h),
            ],
            Agent::Copilot => vec![
                resolve("~/.copilot", Path::new(""), h),
                resolve("~/.config/github-copilot", Path::new(""), h),
                // Windows: GitHub CLI uses UserHomeDir so ~/.copilot maps correctly,
                // but some installs drop config under AppData/Local/github-copilot.
                resolve("~/AppData/Local/github-copilot", Path::new(""), h),
            ],
            Agent::KiloCode => vec![
                resolve("~/.config/kilo", Path::new(""), h),
                resolve("~/.local/share/kilo", Path::new(""), h),
                resolve("~/.kilocode", Path::new(""), h),
                // macOS
                resolve("~/Library/Application Support/kilo", Path::new(""), h),
                // Windows: %APPDATA%\kilo
                resolve("~/AppData/Roaming/kilo", Path::new(""), h),
            ],
            Agent::Hermes => vec![
                resolve("~/.hermes", Path::new(""), h),
                resolve("~/.config/hermes", Path::new(""), h),
            ],
            // Cline ships in TWO shapes and this list knew only one. The
            // VSCode extension keeps its state in the globalStorage tree; the
            // standalone CLI (`npm i -g cline`) keeps it in `~/.cline`, and
            // that is also where `cline plugin install` — the command
            // `install_cline_plugin` RUNS — puts our bridge. So the install
            // table supported the CLI while the presence walk could not see
            // it: measured 2026-08-18 on a machine with cline@3.0.55 and a
            // populated `~/.cline/plugins/_installed`, `install` answered
            // "no AI coding agent detected" for cline.
            Agent::Cline => vec![
                resolve("~/.cline", Path::new(""), h),
                resolve(
                    "~/.config/Code/User/globalStorage/saoudrizwan.claude-dev",
                    Path::new(""),
                    h,
                ),
                resolve(
                    "~/Library/Application Support/Code/User/globalStorage/saoudrizwan.claude-dev",
                    Path::new(""),
                    h,
                ),
                resolve(
                    "~/AppData/Roaming/Code/User/globalStorage/saoudrizwan.claude-dev",
                    Path::new(""),
                    h,
                ),
            ],
            // Aider stores its history + config under ~/.aider.* on
            // every OS. The pip-installed binary doesn't drop a
            // separate dotfile dir, so we look for either the history
            // file or the legacy ~/.aider dir.
            Agent::Aider => vec![
                resolve("~/.aider.chat.history.md", Path::new(""), h),
                resolve("~/.aider.input.history", Path::new(""), h),
                resolve("~/.aider", Path::new(""), h),
            ],
            // Pi: global state lives under ~/.pi/agent/ (settings.json,
            // sessions/, trust.json). Match that specific dir, NOT a bare
            // ~/.pi — the latter is generic enough to false-positive on
            // unrelated tools and would make tokenade scaffold for a pi that
            // isn't installed.
            Agent::Pi => vec![resolve("~/.pi/agent", Path::new(""), h)],
            Agent::OpenClaw => vec![
                resolve("~/.config/openclaw", Path::new(""), h),
                resolve("~/.openclaw", Path::new(""), h),
            ],
            Agent::Antigravity => {
                let mut v = vec![resolve("~/.gemini/antigravity", Path::new(""), h)];
                if cfg!(windows) {
                    v.push(resolve("~/gemini/antigravity", Path::new(""), h));
                }
                v
            }
            // Qwen Code keeps its own ~/.qwen/ dir (Windows ~/qwen/), with
            // settings.json + oauth_creds.json like its Gemini parent.
            // Same measurement as `mcp_config_path` above: `qwen -p` on Windows
            // creates `%USERPROFILE%\.qwen`. The dotless entries stay as extra
            // markers — they cost one `stat` each and match an older layout if
            // one ever existed.
            Agent::Qwen => {
                let (settings, oauth) = ("~/.qwen/settings.json", "~/.qwen/oauth_creds.json");
                vec![
                    resolve("~/.qwen", Path::new(""), h),
                    resolve(settings, Path::new(""), h),
                    resolve(oauth, Path::new(""), h),
                ]
            }
            Agent::Grok => vec![
                resolve("~/.grok", Path::new(""), h),
                resolve("~/.grok/bin/grok", Path::new(""), h),
            ],
            // Odysseus stores its workspace data under ODYSSEUS_DATA_DIR (env-only).
            // The fallback markers cover the default install paths on Linux/macOS
            // and a conventional Windows location.
            // T3 orchestrator: home-level state dir; per-project `.t3code/`
            // markers exist too but the presence walk here is home-based.
            //
            // The home dir it actually creates is `~/.t3`, not `~/.t3code` —
            // measured 2026-08-18 on a machine running it, which holds
            // `~/.t3/userdata/desktop-settings.json` and a `~/.t3/caches/`
            // carrying one JSON per agent it drives (cursor, grok, codex,
            // claudeAgent, opencode). `~/.t3code` was never created there, so
            // the marker matched nothing and T3 Code has been undetectable
            // for the life of the entry. Both are kept: a name this repo
            // invented costs nothing to keep, and being wrong once about
            // which one is real is a reason to stop guessing, not to guess
            // again.
            Agent::T3Code => vec![
                resolve("~/.t3", Path::new(""), h),
                resolve("~/.t3code", Path::new(""), h),
            ],
            Agent::Droid => vec![resolve("~/.factory", Path::new(""), h)],
            Agent::Odysseus => vec![
                resolve("~/.odysseus", Path::new(""), h),
                resolve("~/.config/odysseus", Path::new(""), h),
                resolve("~/AppData/Roaming/Odysseus", Path::new(""), h),
            ],
            Agent::Unknown => vec![],
        }
    }

    /// Backwards-compatible alias kept for the older `cmd_install` call
    /// site that printed a hint to stderr. Prefer
    /// [`Agent::mcp_config_path`] in new code.
    ///
    /// This is USER-FACING — `tokenade detect` prints it under the coverage
    /// line, and `detect --json` carries it. So an arm that names a manual
    /// fallback contradicts the line directly above it. Measured 2026-08-08:
    ///
    ///   coverage: plugin — tool results are compacted inside the agent
    ///   mcp config hint: (no MCP — run `tokenade wrap pi …`)
    ///
    /// Pi and OpenClaw both ship a plugin (`has_plugin()`, `PLUGIN_INSTALLS`),
    /// so "wrap it by hand" was advice to redo work already automatic — the
    /// same staleness `install` carried for OpenClaw, in a second surface. An arm here says which
    /// channel IS live, never a manual one.
    pub fn mcp_config_hint(&self) -> &'static str {
        match self {
            Agent::ClaudeCode => ".mcp.json",
            Agent::T3Code => "(none — drives other agents)",
            Agent::Cursor => ".cursor/mcp.json",
            Agent::Codex => "~/.codex/config.toml",
            Agent::DevinDesktop => "~/.codeium/windsurf/mcp_config.json",
            // The HINT and the PATH named different files until 2026-08-20:
            // this said `mcp.json`, `mcp_config_path` writes `opencode.json`,
            // and the hint is what an operator wiring by hand is told to open.
            // Advice that names a file the product never touches is worse than
            // no advice. `the_hint_and_the_path_name_the_same_file` pins it.
            Agent::OpenCode => "~/.config/opencode/opencode.json",
            Agent::CopilotVsCode => "(none — VS Code owns the MCP surface)",
            Agent::Copilot => "~/.copilot/mcp-config.json",
            Agent::KiloCode => "~/.config/kilo/kilo.json",
            // Same divergence, same day: `mcp_config_path` writes
            // `~/.hermes/config.yaml`.
            Agent::Hermes => "~/.hermes/config.yaml",
            Agent::Cline => "~/.config/Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json",
            Agent::Aider => "(no MCP — shell output goes through the universal shell proxy)",
            Agent::Pi => "(no MCP — tool results are compacted by the tokenade extension)",
            Agent::OpenClaw => "(no MCP yet — tool results are compacted by the tokenade plugin)",
            Agent::Droid => "~/.factory/mcp.json",
            Agent::Odysseus => "(UI-configured — register tokenade MCP in Odysseus settings)",
            Agent::Antigravity => "~/.gemini/settings.json",
            Agent::Qwen => "~/.qwen/settings.json",
            Agent::Grok => "~/.grok/config.toml",
            Agent::Unknown => ".mcp.json",
        }
    }
}
