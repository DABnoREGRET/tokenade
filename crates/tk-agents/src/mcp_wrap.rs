//! Auto-wrap third-party MCP servers behind the `tokenade mcp-proxy`
//! transport so every response above the disclose threshold gets
//! stashed + summarised the same way our own outputs do.
//!
//! ## Why
//!
//! A user typically has more MCP servers than tokenade alone — GitHub,
//! Linear, Slack, custom social-media feeds, etc. Many of those
//! return multi-thousand-token responses that pollute the agent's
//! context. Without wrapping, tokenade only optimises the calls it
//! served itself; with wrapping, EVERY MCP response flows through
//! the disclose pipeline transparently.
//!
//! ## Approach: edit the agent config (Strat B in the design)
//!
//! Each supported agent (Claude Code, Cursor, Codex, Copilot) stores
//! its MCP servers in a JSON config file under either `mcpServers`
//! (most) or `mcp` (Codex). We rewrite each entry to spawn through
//! `tokenade mcp-proxy --`. Idempotent — re-running on a wrapped
//! entry is a no-op. The original `command` + `args` are preserved
//! under a `tokenadeOriginal` metadata key so [`unwrap`] can restore
//! them perfectly on `tokenade uninstall`.
//!
//! ## Shapes handled
//!
//! Object-style (Claude Code, Cursor, Copilot):
//! ```json
//! "mcpServers": { "github": { "command": "npx", "args": ["mcp-github"] } }
//! ```
//!
//! Array-style (Codex / OpenCode):
//! ```json
//! "mcp": { "github": { "command": ["npx", "mcp-github"] } }
//! ```
//!
//! Both are rewritten to call `tokenade mcp-proxy -- <orig args>` while
//! preserving the original `env` block and any additional metadata.
//!
//! Re-scan happens on every `tokenade-mcp` startup (Strat B): no daemon,
//! no file watcher, no OS-specific glue, no antivirus interference.

use serde_json::Value;
use std::path::Path;

// Seven subjects. `pub use` puts them back at the `mcp_wrap::` path the CLI spells.
mod config_io;
mod discover;
mod entry;
mod inspect;
mod purge;
mod sweep;
mod toml_edit;
use config_io::*;
pub use discover::*;
pub use entry::*;
pub use inspect::*;
pub use purge::*;
pub use sweep::*;
use toml_edit::*;

include!("mcp_wrap_tests.rs");
