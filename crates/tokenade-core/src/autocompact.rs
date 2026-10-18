//! Auto-compaction threshold sizing.
//!
//! Background: an agent re-reads its whole cached prefix EVERY turn (cache_read)
//! and re-creates it whenever the cache is invalidated (TTL expiry, the agent's
//! own auto-compaction). So the prefix SIZE is the master cost variable. By
//! default Claude Code only auto-compacts near the very top of the context
//! window (~95% — e.g. ~970k on a 1M model), which keeps the prefix needlessly
//! large for most of a long session. Telling the agent to compact EARLIER (at a
//! fraction of the window) keeps the prefix smaller throughout: cheaper
//! cache_read on every turn and cheaper re-creation on every expiry.
//!
//! This module only computes the target threshold; the agent-specific write
//! lives in the installer (Claude Code exposes `autoCompactWindow` /
//! `CLAUDE_CODE_AUTO_COMPACT_WINDOW`). Agents without such a knob are no-ops.

/// Default fraction (percent) of the context window at which to trigger
/// auto-compaction. 70% leaves headroom while keeping the prefix ~30% smaller
/// than the agent's native ~95% default.
pub const DEFAULT_PCT: u32 = 70;

/// Lowest threshold we will ever set — never compact absurdly early even if a
/// model is mis-detected as tiny.
pub const MIN_THRESHOLD: u64 = 50_000;

/// Context-window size (tokens) for a model identifier.
///
/// OVER-estimation is SAFE: Claude Code clamps the effective window to the
/// model's real one, so a too-large guess simply has no effect (it never
/// compacts *earlier* than the real window allows). UNDER-estimation is the
/// only hazard (compacts too early). Therefore unknown/unversioned models
/// default to the largest known window (1M), which caps harmlessly on a
/// smaller real model.
pub fn context_window_for_model(model: &str) -> u64 {
    // The LIVE table first (`model_costs`, refreshed conditionally once a day
    // and carrying new ids on release day), the hand-written table below only
    // as the offline fallback. A list of model names typed into this file is
    // wrong the day a model ships: `claude-opus-5` was absent from it while
    // being the lineup's default, and fell through to the 1M default — right
    // by luck, not by measurement.
    install_window(crate::model_costs::context_window_for_model(model), model)
}

/// The installer's choice, as a PURE function of what the live table said.
/// Separated so the direction can be tested without a cache on disk: a test
/// that calls the wrapper reads whatever `~/.tokenade/litellm-costs.json`
/// happens to hold on the machine running it, which is not a fixture.
fn install_window(live: Option<u64>, model: &str) -> u64 {
    // Unknown / unversioned ("sonnet", "opus", "default", "") — assume large;
    // the agent caps it to the real window, so this can only fail safe.
    live.or_else(|| known_context_window(model))
        .unwrap_or(1_000_000)
}

/// The hand-written model→window table: the OFFLINE FALLBACK, consulted only
/// when the daily LiteLLM table has no row for the id (no cache yet, an id no
/// upstream carries, a local deployment name). Without a default-direction
/// policy. `None` when
/// the identifier isn't recognized — callers pick the fail-safe direction
/// for THEIR use ([`context_window_for_model`] fails large, the gain
/// estimator fails small).
fn known_context_window(model: &str) -> Option<u64> {
    let m = model.to_ascii_lowercase();
    // 1M-context models (must be checked before the generic 200k families).
    if m.contains("opus-4-8") || m.contains("sonnet-5") || m.contains("fable-5") || m.contains("1m")
    {
        return Some(1_000_000);
    }
    // 200k-context models.
    if m.contains("haiku")
        || m.contains("sonnet-4")
        || m.contains("opus-4-6")
        || m.contains("opus-4-7")
        || m.contains("sonnet-3")
        || m.contains("opus-3")
    {
        return Some(200_000);
    }
    None
}

/// Context window used by the GAIN ESTIMATOR's compounding HORIZON (how many
/// turns an avoided token keeps paying before the counterfactual context
/// would have been auto-compacted anyway).
///
/// Same table as [`context_window_for_model`] but the OPPOSITE unknown-model
/// default: there, over-estimating is harmless (the agent clamps to the real
/// window); here, over-estimating the window INFLATES self-reported savings
/// (a longer horizon = more compounding credit). Savings claims must fail
/// CONSERVATIVE, so unknown/unversioned models assume the common 200k window.
pub fn estimator_window_for_model(model: &str) -> u64 {
    estimate_window(crate::model_costs::context_window_for_model(model), model)
}

/// The estimator's choice, as a pure function of what the live table said.
fn estimate_window(live: Option<u64>, model: &str) -> u64 {
    let table = known_context_window(model).unwrap_or(200_000);
    // The live table is consulted here too, but only ever DOWNWARDS. Its
    // `max_input_tokens` is the model's MAXIMUM, which a session need not
    // have: `claude-opus-5` reads 1 000 000 upstream while a session without
    // the 1M beta runs at 200 000, and crediting the larger one would inflate
    // every savings figure fivefold.
    match live {
        Some(live) => live.min(table),
        None => table,
    }
}

/// Threshold in tokens = `pct`% of `window`, clamped to `[MIN_THRESHOLD,
/// 95% of window]` (never set it above what the native default already does).
pub fn compaction_threshold(window: u64, pct: u32) -> u64 {
    let pct = pct.clamp(10, 95) as u64;
    let t = window.saturating_mul(pct) / 100;
    t.max(MIN_THRESHOLD).min(window.saturating_mul(95) / 100)
}

/// Convenience: threshold for a model string at a given percent.
pub fn threshold_for_model(model: &str, pct: u32) -> u64 {
    compaction_threshold(context_window_for_model(model), pct)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Both directions are tested on the PURE functions: the live table
    // lives in `~/.tokenade/`, and a test that reads it answers differently depending
    // on the machine running it. What is tested here is the RULE, not the
    // content of the day.

    #[test]
    fn the_live_window_wins_over_the_hand_written_table() {
        // `claude-sonnet-4-6` is written 200k above and is 1M upstream
        // (measured 2026-09-09): it is the exact case the hand-written table misses.
        assert_eq!(
            install_window(Some(1_000_000), "claude-sonnet-4-6"),
            1_000_000
        );
        assert_eq!(install_window(Some(200_000), "claude-haiku-4-5"), 200_000);
    }

    #[test]
    fn the_hand_written_table_is_the_offline_fallback() {
        assert_eq!(install_window(None, "claude-opus-4-8"), 1_000_000);
        assert_eq!(install_window(None, "claude-sonnet-5"), 1_000_000);
        assert_eq!(install_window(None, "claude-haiku-4-5"), 200_000);
        assert_eq!(install_window(None, "claude-opus-4-6"), 200_000);
    }

    #[test]
    fn unknown_defaults_to_large_window_fails_safe() {
        // The agent brings the threshold down to its real window: too big can only
        // do nothing, too small compacts too early.
        assert_eq!(install_window(None, "sonnet"), 1_000_000);
        assert_eq!(install_window(None, "default"), 1_000_000);
        assert_eq!(install_window(None, ""), 1_000_000);
    }

    #[test]
    fn estimator_takes_the_live_window_only_downwards() {
        // An upstream window LARGER than what we know is not credited:
        // `max_input_tokens` is a maximum, not the session's window.
        assert_eq!(
            estimate_window(Some(1_000_000), "claude-haiku-4-5"),
            200_000
        );
        assert_eq!(estimate_window(Some(1_000_000), "claude-opus-5"), 200_000);
        // Smaller, it is taken: that is the cautious direction.
        assert_eq!(estimate_window(Some(32_000), "un-modele-local"), 32_000);
    }

    #[test]
    fn estimator_window_fails_small_on_unknown() {
        assert_eq!(estimate_window(None, "claude-opus-4-8"), 1_000_000);
        assert_eq!(estimate_window(None, "claude-haiku-4-5"), 200_000);
        assert_eq!(estimate_window(None, ""), 200_000);
        assert_eq!(estimate_window(None, "default"), 200_000);
    }

    #[test]
    fn pct_and_floor_are_clamped() {
        assert_eq!(compaction_threshold(1_000_000, 70), 700_000);
        assert_eq!(compaction_threshold(1_000_000, 40), 400_000);
        // Never under the floor, never above what the default does.
        assert_eq!(compaction_threshold(200_000, 1), MIN_THRESHOLD);
        assert_eq!(compaction_threshold(1_000_000, 99), 950_000);
    }
}
