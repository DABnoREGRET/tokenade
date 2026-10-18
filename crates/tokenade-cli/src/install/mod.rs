//! Install / scaffold / hook-command / MCP-config / shell-proxy code.
//! Extracted from main.rs (refactor split). Descendant module → can read
//! private crate-root items; moved items are pub(crate) so the root + sibling
//! modules can call them.
#![allow(unused_imports)]
use crate::hooks::*;
use crate::wrap::*;
use crate::*;
use std::fs;
use std::path::{Path, PathBuf};

// --- mechanical module split (no behaviour change) ---
pub(crate) mod capability;
mod claude_hooks;
pub(crate) mod claude_md;
pub(crate) mod gemini_hooks;
mod help_text;
mod hook_util;
pub(crate) mod json_util;
mod legacy_shim_removal;
mod lifecycle_hooks;
mod shell_proxy;
mod statusline_hosts;
pub(crate) mod uninstall;

pub(crate) use capability::*;
pub(crate) use claude_hooks::*;
pub(crate) use claude_md::*;
pub(crate) use gemini_hooks::*;
pub(crate) use help_text::*;
pub(crate) use hook_util::*;
pub(crate) use json_util::*;
pub(crate) use legacy_shim_removal::*;
pub(crate) use lifecycle_hooks::*;
pub(crate) use shell_proxy::*;
pub(crate) use statusline_hosts::*;
pub(crate) use uninstall::*;
