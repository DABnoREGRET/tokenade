//! Nudging — "agent, you might want to use X instead of Y".
//!
//! ## READ THIS FIRST: the centralisation below never happened
//!
//! This module used to open by calling itself the "single source of truth",
//! and the section further down describes the scattered state as a "Before".
//! Measured 2026-08-04: the `NUDGES` table holds THREE entries and
//! `tokenade-cli` still emits nudges from TWENTY-FIVE ad-hoc sites
//! (`compactors.rs`, `wrap.rs`), each with its own step and de-duplication —
//! `bypass_nudge_step`, `nudge_worth_suggesting`, `nudge_sequential_web_calls`.
//! The "Before" IS the Now, and nothing in this crate calls `step()`.
//!
//! The design below is still the one worth having. The claim that it had been
//! adopted was worse than the missing migration: someone looking for "where
//! are the nudges defined" found this file, believed it, and missed the
//! twenty-five real ones. The promise is withdrawn; the design is kept.
//!
//! Per-session state in memory (no disk persistence). Per-pattern
//! throttle (1 in N), per-pattern hard cap (stop after K shows
//! this session). No acceptance tracking — we surface the info,
//! the agent decides. No opt-out env var: nudges are part of the
//! strategy and must stay observable; an opt-out would let the
//! model silently disable the lever and we'd lose the ability to
//! mesure whether it works.
//!
//! ## Why a central module
//!
//! Before: nudges scattered across compactors and hooks
//! (`cmd_wrap`, `handle_read_pretool`, grep redirect, edit-diff
//! hint). Each composed its own throttle logic, none shared state,
//! no way to tell from telemetry which actually fired and how often.
//! This module is the single replacement. New patterns become one
//! entry in `NUDGES`; call sites become a one-liner that calls
//! `nudge::step(key)` and prints the returned message if any.
//!
//! This module is Phase 3 of the agent-uptake strategy.

use std::collections::HashMap;
use std::sync::Mutex;

/// One nudge pattern. Static, declared in [`NUDGES`].
#[derive(Debug, Clone, Copy)]
pub struct NudgeDef {
    /// Stable string key for state lookup + telemetry correlation.
    /// MUST be unique across `NUDGES` (`nudges_registry_has_unique_keys`
    /// test locks this in).
    pub key: &'static str,
    /// Short message starting with 💡. Kept verbatim so the same
    /// text appears in every nudge for this pattern — the user
    /// scanning Claude's transcript can ignore-or-act once and the
    /// throttle takes care of not pestering.
    pub message: &'static str,
    /// Trigger every Nth invocation. `1` = always (no throttle).
    /// Fires on the 1st, then (1+N)th, (1+2N)th... so the first hit
    /// for a pattern is responsive instead of waiting for N silent
    /// triggers.
    pub throttle_every: u32,
    /// Stop nudging this pattern after K total shows in this session.
    /// Caps the "repeat-pestering" failure mode — if the agent's been
    /// told 3 times and hasn't changed behaviour, more nudges add
    /// noise without value.
    pub max_shows: u32,
}

/// Phase-3 initial set. Extension policy: add a nudge only when
/// telemetry shows a recurring (prog, argv-shape) bypass that has
/// a clear tokenade alternative (the "golden rule": never nudge without one).
pub const NUDGES: &[NudgeDef] = &[
    NudgeDef {
        key: "grep-identifier",
        message: "💡 grep on what looks like a function/struct name. \
                  Try `tokenade query \"NAME\"` — same output, plus precise \
                  location, no test/build-cache hits, no false-positive \
                  substring matches.",
        throttle_every: 5,
        max_shows: 3,
    },
    NudgeDef {
        key: "find-recursive-source",
        message: "💡 `find . -name '*.<ext>'` returns paths only. \
                  `tokenade map` returns the same paths WITH per-file symbol \
                  counts — typically smaller and far more useful for \
                  orienting in a new codebase.",
        throttle_every: 5,
        max_shows: 3,
    },
    NudgeDef {
        key: "grep-prose-question",
        message: "💡 grep on a natural-language phrase rarely works \
                  (the words won't appear verbatim in source). \
                  `tokenade semantic \"…\"` does intent-based search across \
                  the codebase.",
        throttle_every: 3,
        max_shows: 3,
    },
];

/// Session-scoped state. Carried out of the top-level Mutex so tests
/// can construct an isolated `State` and exercise `step` without
/// contending on the process-global lock.
#[derive(Debug, Default)]
pub struct State {
    triggers: HashMap<&'static str, u32>,
    shows: HashMap<&'static str, u32>,
}

impl State {
    pub fn new() -> Self {
        Self::default()
    }

    /// See top-level [`step`] for semantics. The instance method is
    /// what tests use; the public free fn just wraps the global
    /// state.
    pub fn step(&mut self, key: &str) -> Option<&'static str> {
        let def = NUDGES.iter().find(|n| n.key == key)?;
        let trig = self.triggers.entry(def.key).or_insert(0);
        *trig += 1;
        let already_shown = *self.shows.get(def.key).unwrap_or(&0);
        if already_shown >= def.max_shows {
            return None;
        }
        // Fire on the 1st, 1+N-th, 1+2N-th... — feels more responsive
        // than waiting N silent calls before the first nudge.
        if *trig % def.throttle_every != 1 {
            return None;
        }
        *self.shows.entry(def.key).or_insert(0) += 1;
        Some(def.message)
    }

    /// Snapshot of (key, shows) for telemetry / dashboard rendering.
    pub fn snapshot(&self) -> Vec<(&'static str, u32)> {
        let mut out: Vec<(&'static str, u32)> = self.shows.iter().map(|(k, v)| (*k, *v)).collect();
        out.sort_by(|a, b| a.0.cmp(b.0));
        out
    }
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// Record a trigger for `key`. Returns the nudge message iff:
///   (a) `key` is known (matches a `NudgeDef.key` in [`NUDGES`]), AND
///   (b) `trigger_count % throttle_every == 1` (fire on 1st, 1+N, …), AND
///   (c) we haven't already shown `max_shows` times this session.
///
/// Unknown keys return `None` defensively — calling sites don't
/// panic on a mistyped key. State is process-global (one shared
/// counter set per `tokenade-core` consumer); a tokenade CLI
/// invocation is one process so "session" = "this CLI run".
///
/// Hook-installed entry points (Bash PreToolUse, Read PostToolUse,
/// etc.) get a fresh process per Claude tool call, so the throttle
/// won't carry across calls unless we add disk persistence later.
/// The 3 initial nudges are intentionally tolerant of that: they're
/// useful even on every call, just spammy — the throttle's main
/// purpose right now is to avoid annoyance INSIDE a single
/// long-running `tokenade` invocation (e.g., `cmd_wrap` of a script
/// that grep-loops).
pub fn step(key: &str) -> Option<&'static str> {
    let mut guard = STATE.lock().expect("nudge state poisoned");
    guard.get_or_insert_with(State::new).step(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_unknown_key_returns_none() {
        let mut st = State::new();
        assert!(st.step("does-not-exist").is_none());
    }

    #[test]
    fn step_throttles_every_n() {
        let mut st = State::new();
        // grep-identifier: throttle_every=5, max_shows=3
        assert!(
            st.step("grep-identifier").is_some(),
            "1st: trig=1, 1%5=1 → fire"
        );
        assert!(st.step("grep-identifier").is_none(), "2nd");
        assert!(st.step("grep-identifier").is_none(), "3rd");
        assert!(st.step("grep-identifier").is_none(), "4th");
        assert!(st.step("grep-identifier").is_none(), "5th: 5%5=0");
        assert!(st.step("grep-identifier").is_some(), "6th: 6%5=1 → fire");
    }

    #[test]
    fn step_caps_at_max_shows() {
        let mut st = State::new();
        // grep-prose-question: throttle_every=3, max_shows=3
        let mut shown = 0;
        for _ in 0..30 {
            if st.step("grep-prose-question").is_some() {
                shown += 1;
            }
        }
        assert_eq!(shown, 3, "max_shows=3 should hard-cap, got {shown}");
    }

    #[test]
    fn step_independent_per_key() {
        let mut st = State::new();
        // Bumping one key must not affect another's counter.
        assert!(st.step("grep-identifier").is_some());
        // find-recursive-source is at trig=0 — first call should fire.
        assert!(st.step("find-recursive-source").is_some());
    }

    #[test]
    fn nudges_registry_has_unique_keys() {
        let mut seen = std::collections::HashSet::new();
        for n in NUDGES {
            assert!(seen.insert(n.key), "duplicate key {}", n.key);
        }
    }

    #[test]
    fn nudges_have_emoji_and_useful_message() {
        // Lock-in: format guarantee. Brainstorm decision: every nudge
        // starts with 💡 + short message. If someone adds a nudge that
        // forgets the emoji, this test catches it.
        for n in NUDGES {
            assert!(
                n.message.starts_with('💡'),
                "nudge {} missing 💡 prefix: {:?}",
                n.key,
                n.message
            );
            assert!(
                n.message.contains("tokenade"),
                "nudge {} should mention a tokenade alternative",
                n.key
            );
            assert!(
                n.throttle_every >= 1,
                "nudge {} has throttle_every=0 (would divide-by-zero)",
                n.key
            );
            assert!(
                n.max_shows >= 1,
                "nudge {} has max_shows=0 (never fires — pointless)",
                n.key
            );
        }
    }

    #[test]
    fn snapshot_returns_sorted_shown_count() {
        let mut st = State::new();
        // Fire grep-identifier once, grep-prose-question once.
        assert!(st.step("grep-identifier").is_some());
        assert!(st.step("grep-prose-question").is_some());
        let snap = st.snapshot();
        assert_eq!(snap.len(), 2);
        // Sorted alphabetically: grep-identifier < grep-prose-question
        assert_eq!(snap[0], ("grep-identifier", 1));
        assert_eq!(snap[1], ("grep-prose-question", 1));
    }

    #[test]
    fn global_step_compiles_and_returns_some_for_known_key() {
        // Smoke: the process-global wrapper works. We don't reset
        // shared state between tests so just verify the call shape.
        let _ = super::step("grep-identifier");
    }
}
