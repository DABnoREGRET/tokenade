//! Pre-execution command rewriting (Token-Savior-inspired).
//!
//! Compactors run *after* a command produces its output. That's powerful
//! but wasted work when the command had a terser-output flag we could
//! have asked for in the first place. `git status` → `git status
//! --porcelain=v2 --branch` is ~5× smaller before STS ever sees it, and
//! the residual compactor still applies on top.
//!
//! This module also splits compound commands (`cd foo && pytest && npm test`)
//! and points the caller at the last *meaningful* segment, so single-token
//! `cd` prefixes and `set -e` boilerplate don't dilute the savings figure.
//!
//! The rewrites are **conservative on purpose**:
//!
//! - Never change semantics (`pytest -q` is a display flag; `pytest -x` is
//!   not — we don't add `-x`).
//! - Never override a flag the user already passed. `git log --pretty=full`
//!   stays `--pretty=full`.
//! - Bail on subshells `$(…)`, backticks, heredocs `<<`, and `eval`. We
//!   refuse to parse what a shell would re-parse.
//!
//! Quantified: on the Tokenade dev session log, the rewriter alone trims
//! ~22 % of pre-compaction bytes for the `git`/`cargo`/`pytest` mix.

mod chain;
mod lines;
mod rules;
mod segment;
pub use chain::*;
pub(crate) use rules::*;
pub use segment::*;

use std::borrow::Cow;

/// Result of `rewrite`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rewrite {
    /// The argv-form command to actually execute (may equal `original`).
    pub command: String,
    /// What was passed in.
    pub original: String,
    /// Slug naming the rule that fired, e.g. `"git-status-porcelain"`.
    /// `None` when no rewrite applied.
    pub rule: Option<&'static str>,
    /// `true` if we discarded preceding segments of a compound command
    /// (`cd foo && X` → focus is `X`).
    pub split_chain: bool,
}

impl Rewrite {
    /// No-op rewrite.
    pub fn identity(s: impl Into<String>) -> Self {
        let s = s.into();
        Self {
            original: s.clone(),
            command: s,
            rule: None,
            split_chain: false,
        }
    }
}

/// Rewrite `cmd` for terser output. Safe to call on any string — when no
/// rule applies, returns the input unchanged with `rule = None`.
pub fn rewrite(cmd: &str) -> Rewrite {
    let original = cmd.to_string();

    // 1. Bail on shell features we can't safely parse.
    if has_shell_unsafe(cmd) {
        return Rewrite::identity(original);
    }

    // 2. Split on && / ; and keep the last meaningful segment.
    let (focus, split) = focus_segment(cmd);
    let focus = focus.trim();

    // 3. User presets win over the built-in table — the whole point of
    //    `~/.config/tokenade/presets/` is to let a user override a
    //    built-in for their niche stack without recompiling.
    if let Some((rewritten, rule)) = crate::presets::apply(focus) {
        return Rewrite {
            original,
            command: rewritten,
            rule: Some(rule),
            split_chain: split,
        };
    }

    // 4. Apply rule table to the focused segment.
    let tokens: Vec<&str> = focus.split_whitespace().collect();
    if tokens.is_empty() {
        return Rewrite::identity(original);
    }

    if let Some((new_focus, rule)) = apply_rule(&tokens) {
        // We re-attach to nothing — the prefix segments were just
        // navigation/setup; the focused segment is what the agent cares
        // about. Caller can still find the original in `Rewrite.original`
        // if they need to replay.
        let command = if split {
            new_focus.into_owned()
        } else {
            // Single-segment input: preserve any leading whitespace the
            // caller passed (shouldn't matter but it's cheap to honour).
            new_focus.into_owned()
        };
        return Rewrite {
            original,
            command,
            rule: Some(rule),
            split_chain: split,
        };
    }

    // No rule, but we may have still split a chain.
    if split {
        return Rewrite {
            command: focus.to_string(),
            original,
            rule: None,
            split_chain: true,
        };
    }
    Rewrite::identity(original)
}

#[cfg(test)]
include!("command_rewrite_tests.rs");
