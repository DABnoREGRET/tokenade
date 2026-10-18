//! **What is EMBEDDED in the binary.**
//!
//! The plugins shipped with tokenade (Hermes, Pi, opencode, Cline, OpenClaw)
//! and the tables that say where each installer puts them. These are bytes,
//! not logic: keeping them together prevents a plugin from being updated
//! without its install table knowing.

use super::*;

/// The Hermes bridge plugin, embedded so the installer can drop it on disk
/// (no pip, no packaging). It runs under Hermes' own Python interpreter.
pub(crate) const HERMES_PLUGIN_INIT: &str = include_str!("../../assets/hermes_plugin/__init__.py");
pub(crate) const HERMES_PLUGIN_YAML: &str = include_str!("../../assets/hermes_plugin/plugin.yaml");

/// The Pi compaction extension, embedded so the installer drops it on disk.
/// Loaded by Pi's jiti loader under Pi's own Node runtime (no compile, no dep).
pub(crate) const PI_EXTENSION_TS: &str = include_str!("../../assets/pi_extension/tokenade.ts");

/// The opencode compaction plugin, embedded so the installer drops it on disk.
/// opencode auto-discovers `{plugin,plugins}/*.ts` under its config dir.
pub(crate) const OPENCODE_PLUGIN_TS: &str =
    include_str!("../../assets/opencode_plugin/tokenade.ts");

/// The OpenClaw agent-tool-result middleware plugin (3 files). Unlike Pi /
/// opencode, OpenClaw registers a plugin via `openclaw plugins install <dir>`
/// rather than auto-discovering a directory, so the installer writes the
/// package into a tokenade-owned dir then runs that command.
pub(crate) const CLINE_PLUGIN_TS: &str = include_str!("../../assets/cline_plugin/tokenade.ts");
pub(crate) const CLINE_PLUGIN_PKG: &str = include_str!("../../assets/cline_plugin/package.json");

pub(crate) const OPENCLAW_PLUGIN_INDEX: &str =
    include_str!("../../assets/openclaw_plugin/index.js");
pub(crate) const OPENCLAW_PLUGIN_PKG: &str =
    include_str!("../../assets/openclaw_plugin/package.json");
pub(crate) const OPENCLAW_PLUGIN_MANIFEST: &str =
    include_str!("../../assets/openclaw_plugin/openclaw.plugin.json");

/// Per-agent install dispatch: wire every detected agent's hook/plugin/
/// extension in the SAME order `cmd_install` always processed them (wrap-only
/// notes → Hermes/Pi/opencode/OpenClaw/Cursor/Kilo plugins → Claude Code hooks
/// → Codex/Grok hooks → CLAUDE.md scaffold → Claude PreToolUse Bash-rewrite →
/// Gemini/Qwen BeforeTool hooks). Every `println!`/`eprintln!` and side effect
/// is preserved verbatim. Returns `mcp_ok` (== `targets.len()`), the count the
/// final report prints.
/// Agents whose install is exactly "call the plugin installer, then report".
///
/// Seven `if targets.contains(…) { if let Some(h) = home { match … } }` blocks
/// said this, 14 lines each, differing only in the four values below. The
/// shape was verified identical before folding: normalising away the agent,
/// the installer and the strings left ONE hash for all seven.
/// Agents whose hook install is exactly "call the installer, map the outcome".
///
/// Four blocks said this, 16 lines each. Verified identical before folding:
/// normalising away the agent, the installer and the strings left ONE hash for
/// the four — and a DIFFERENT one for `Codex` (38 l.), which is why Codex keeps
/// its own block below instead of being forced into this table.
/// The gemini-cli family (Qwen Code, since Gemini CLI itself left the
/// product): one tool hook, one settings schema, one row each.
///
/// Qwen Code is a hard fork of Gemini CLI, and its 26 lines here were a
/// byte-for-byte copy of Gemini's — a normalized-shape diff of the two blocks
/// came back with ONE differing line, the installer name. The write side was
/// already shared (`install::gemini_hooks::install_before_tool_hook`); only the
/// reporting was not.
/// The last field is the PHRASE, as in [`HOOK_INSTALLS`]: these two hosts do
/// not listen to the same event name. Gemini CLI speaks its own dialect
/// (`BeforeTool`), Qwen Code speaks Claude Code's (`PreToolUse`) — see
/// `HookDialect::for_config_dir`. The line announced "BeforeTool hook" for
/// both, so qwen's install boasted of an event it no longer writes: an
/// operator who goes to check does not find it, and the same mistake had
/// already been fixed for grok.
pub(crate) const BEFORE_TOOL_INSTALLS: &[(detect::Agent, BeforeToolInstaller, &str, &str)] = &[(
    detect::Agent::Qwen,
    install_qwen_cli_hook as BeforeToolInstaller,
    "Qwen Code",
    "PreToolUse hook",
)];

/// What every row of [`BEFORE_TOOL_INSTALLS`] does.
pub(super) type BeforeToolInstaller = fn(&str, Option<&Path>, bool) -> HookOutcome;

pub(crate) const HOOK_INSTALLS: &[(detect::Agent, HookInstaller, &str, &str)] = &[
    (
        detect::Agent::Grok,
        install_grok_hooks as HookInstaller,
        "Grok",
        // The label named four events we install two of, on an agent
        // where the other two cannot deliver anything. See
        // `install_grok_hooks` for what grok 1.0.0 honours and what it ignores.
        "hooks (SessionStart + PreCompact — grok honours no substitution)",
    ),
    (
        detect::Agent::Droid,
        install_droid_hooks as HookInstaller,
        "Droid",
        "hooks (Pre/PostToolUse on Execute, SessionStart/…)",
    ),
    (
        detect::Agent::Copilot,
        install_copilot_hooks as HookInstaller,
        "Copilot",
        "CLI hooks (Pre/PostToolUse/SessionStart/…)",
    ),
    // The `agy` CLI, which is a different product from the Antigravity IDE
    // they share a detection seed with. The installer gates on
    // `~/.gemini/config/` — the CLI's own directory — so an IDE-only machine
    // is untouched, and that gate is the discriminator between the two.
    (
        detect::Agent::Antigravity,
        install_antigravity_cli_hooks as HookInstaller,
        "Antigravity CLI",
        "PreToolUse command rewrite (~/.gemini/config/hooks.json)",
    ),
];

/// What every row of [`HOOK_INSTALLS`] does: write the agent-side hook set.
pub(super) type HookInstaller = fn(&str, Option<&str>, Option<&Path>, bool) -> HookOutcome;

pub(crate) const PLUGIN_INSTALLS: &[(detect::Agent, PluginInstaller, &str, &str)] = &[
    (detect::Agent::Hermes, install_hermes_plugin as PluginInstaller, "hermes", "installed read_file/MCP compaction plugin (~/.hermes/plugins/tokenade) + enabled in config.yaml"),
    (detect::Agent::Pi, install_pi_extension as PluginInstaller, "pi", "installed tool-result compaction extension (~/.pi/agent/extensions/tokenade.ts)"),
    // aider has no bridge: what we install is a LAUNCHER, the only thing this
    // agent exposes (`load:` → `/run`, verified at runtime).
    (detect::Agent::Aider, install_aider_startup as PluginInstaller, "aider", "startup command → `tokenade map` (~/.aider.conf.yml `load:`)"),
    (detect::Agent::OpenCode, install_opencode_plugin as PluginInstaller, "opencode", "installed tool-result compaction plugin (~/.config/opencode/plugin/tokenade.ts)"),
    (detect::Agent::OpenClaw, install_openclaw_plugin as PluginInstaller, "openclaw", "installed tool-result compaction plugin (~/.tokenade/openclaw-plugin, registered via `openclaw plugins install`)"),
    (detect::Agent::Cline, install_cline_plugin as PluginInstaller, "cline", "installed tool-result folding plugin (~/.tokenade/cline-plugin, registered via `cline plugin install`)"),
    (detect::Agent::Cursor, install_cursor_hooks as PluginInstaller, "cursor", "installed preToolUse shell-rewrite + postToolUse MCP-folding hooks (~/.cursor/hooks.json)"),
    (detect::Agent::KiloCode, install_kilo_plugin as PluginInstaller, "kilo", "installed tool-result compaction plugin (~/.config/kilo/plugins/tokenade.ts)"),
];

/// What every row of [`PLUGIN_INSTALLS`] does: write the agent-side plugin.
pub(super) type PluginInstaller = fn(&Path, bool) -> Result<bool, String>;
