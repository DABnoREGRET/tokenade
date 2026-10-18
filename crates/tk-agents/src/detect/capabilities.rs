//! **What each agent can do NATIVELY.**
//!
//! Read a PDF, display an image, intercept a read or a web
//! search: where the host already does it, tokenade has nothing to add — and
//! stepping in replaces a capability with one of ours, a worse one. One table per
//! question, because a capability present in one agent is missing in its
//! neighbour.

use super::*;

impl Agent {
    /// Does this agent's file-read tool render PDFs (and similar binary
    /// documents) NATIVELY — i.e. it can page through and "see" the
    /// document itself, so tokenade extracting the text and injecting it
    /// is redundant (and, for the Read hook, actively harmful: rewriting
    /// the tool input strips the agent's own pagination params)?
    ///
    /// Whitelist, not blacklist: only return true for agents we have
    /// CONFIRMED read PDFs natively. For every other agent the safe
    /// default is `false` → tokenade extracts the text, because without
    /// it the agent just gets an unreadable-binary error. Claude Code 2.x
    /// renders PDFs natively (measured: doc-pdf bench — intercepting cost
    /// +32%/+225% turns vs native). Extend this list only with evidence.
    pub fn reads_pdf_natively(&self) -> bool {
        matches!(self, Agent::ClaudeCode)
    }

    /// Image formats this agent can actually RENDER to its vision model.
    ///
    /// Everything else must be converted (we re-encode to png) or the agent
    /// gets an unusable blob: a `.tiff` scan or a `.bmp` screenshot is a real
    /// file users have, and no mainstream agent renders either. Conservative on
    /// purpose — claiming a format the agent cannot display costs a wasted
    /// round-trip, while converting one it could have shown costs only a
    /// re-encode.
    ///
    /// png/jpeg are universal. gif/webp are Claude Code's documented extras.
    pub fn renders_image_ext(&self, ext: &str) -> bool {
        let e = ext.trim_start_matches('.').to_ascii_lowercase();
        match e.as_str() {
            "png" | "jpg" | "jpeg" => true,
            "gif" | "webp" => matches!(self, Agent::ClaudeCode),
            _ => false,
        }
    }

    /// TRUE when we can intercept the agent's file-READ tool (a PreToolUse
    /// Read hook), so tokenade extends that tool in place: the agent just
    /// Reads a `.pdf`/`.docx`/… and gets text back.
    ///
    /// FALSE for agents we only reach through the shell (Codex, Cursor, Grok,
    /// Aider, Pi, …) or through MCP alone (Devin Desktop, Antigravity). There the
    /// capability exists but has to be INVOKED — `tokenade read <file>` — so
    /// their scaffold must show the command, not claim their Read changed.
    /// Mis-stating this is worse than silence: telling an agent its Read
    /// handles PDFs when it does not sends it into a loop on an unreadable
    /// binary.
    pub fn hooks_read(&self) -> bool {
        matches!(
            self,
            Agent::ClaudeCode
                | Agent::OpenCode
                | Agent::KiloCode
                | Agent::Copilot
                | Agent::Hermes
                | Agent::Cline
                | Agent::OpenClaw
        )
    }

    /// Does this agent's native web SEARCH get intercepted by our hook?
    ///
    /// Narrower than [`hooks_read`]: it needs a PreToolUse hook AND a native
    /// search tool to intercept. Everywhere else the five-engine union exists
    /// but has to be CALLED — `tokenade search "<query>"` — and the scaffold
    /// must say so, because an agent that is not told simply never searches
    /// with us. That was the state of the world until this predicate existed:
    /// hooked agents got the union, everyone else got nothing.
    pub fn hooks_websearch(&self) -> bool {
        matches!(self, Agent::ClaudeCode | Agent::OpenCode | Agent::Hermes)
    }

    /// All known agents (used by the installer).
    pub fn all() -> &'static [Agent] {
        &[
            Agent::ClaudeCode,
            Agent::Cursor,
            Agent::Codex,
            Agent::DevinDesktop,
            Agent::OpenCode,
            Agent::KiloCode,
            Agent::Copilot,
            Agent::Hermes,
            Agent::Cline,
            Agent::Aider,
            Agent::Pi,
            Agent::OpenClaw,
            Agent::Qwen,
            Agent::Antigravity,
            Agent::Grok,
            Agent::T3Code,
            Agent::Droid,
            Agent::CopilotVsCode,
            Agent::Odysseus,
        ]
    }

    /// What integration channels this agent supports.
    /// Which channel covers this agent today. See [`Coverage`].
    pub fn coverage(&self) -> Coverage {
        let i = self.integration();
        if i.shell_hook {
            if let Some(ev) = i.hook_name {
                return Coverage::Hook(ev);
            }
        }
        if self.has_plugin() {
            return Coverage::Plugin;
        }
        // Rank 3 of the coverage ladder. `mcp_wrap` knew
        // where an agent kept its MCP servers before this type existed, and
        // `coverage()` still answered "nothing covers you", because it modelled
        // two channels of a five-rank ladder. An agent that one command can
        // wire was being told to compact by hand.
        //
        // A property of the AGENT, not of the machine: this asks whether
        // tokenade knows a config site for it, not whether that file exists
        // today. Keeping it pure is deliberate — `coverage()` is called from
        // the dashboard, `gain` and the statusline, and a filesystem probe in
        // there would make the same agent report differently between two
        // commands on one machine.
        if crate::mcp_wrap::agent_has_known_mcp_site(self.as_str()) {
            return Coverage::McpWrappable;
        }
        match self {
            Agent::T3Code => Coverage::ViaDrivenAgents,
            // "nothing covers unknown" is not a sentence a user can act on:
            // the honest statement is that we could not tell WHICH agent is
            // running, which is a different problem with a different fix.
            Agent::Unknown => Coverage::AgentUnknown,
            // Measured: aider's `cmd_run` shells out, so the universal shell
            // proxy folds that output. Its `/add` path does not go near a
            // shell, which is exactly why this is `ShellProxy` and not
            // `Plugin`. Odysseus deliberately stays `Uncovered`: nobody has
            // checked how it runs commands, and guessing here would put the
            // product back to claiming a coverage it never verified.
            Agent::Aider => Coverage::ShellProxy,
            _ => Coverage::Uncovered,
        }
    }

    /// Does tokenade ship a tool-result compaction PLUGIN for this agent?
    ///
    /// The third install channel, and since the zero-MCP pivot the live one
    /// for most of this list — `Integration` only models MCP registration
    /// and shell hooks, so an agent covered by a plugin reads as uncovered
    /// there. OpenClaw is the sharpest case: `integration()` calls it
    /// wrap-only, and it gets a plugin.
    ///
    /// Kept in sync with `PLUGIN_INSTALLS` (tokenade-cli) by
    /// `every_agent_with_a_plugin_installer_declares_it`, with ONE
    /// documented exception: Cursor sits in that table because its
    /// installer matches the `PluginInstaller` signature, not because it
    /// gets a plugin. It gets a `preToolUse` hook in `~/.cursor/hooks.json`
    /// — see its arm in `integration()`.
    pub fn has_plugin(&self) -> bool {
        matches!(
            self,
            Agent::Cline
                | Agent::Hermes
                | Agent::KiloCode
                | Agent::OpenClaw
                | Agent::OpenCode
                | Agent::Pi
        )
    }
}
