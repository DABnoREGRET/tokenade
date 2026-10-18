//! **The delivery threshold, and the one rule that reads it.**
//!
//! Above `threshold_tokens()` an output is not delivered inline: it is stashed
//! and replaced by a summary. That makes it the budget every LOSSY cut must be
//! measured against — below it the whole rendering would have been delivered,
//! so a cut only buys the re-fetch the agent then makes. Measured 2026-10-02
//! three times over: a verbose `docker system df -v`, grep hits cut
//! to ten per file, a 25-entry markdown index cut to 20.
//! Here rather than in `tokenade-core::disclose` because the format compactors
//! of `tk-formats` must ask the same question and cannot depend on the core.

/// Default disclosure threshold. Above this token count, switch from
/// direct return to summary + stash. User-configurable via the env
/// var `TOKENADE_DISCLOSE_THRESHOLD`. See the
/// `feedback_5k_token_disclosure_threshold` memory entry for the
/// rationale.
pub const DEFAULT_THRESHOLD_TOKENS: usize = 5000;

/// Context-pressure multiplier — set by the hook entrypoints via the
/// `TOKENADE_PRESSURE` env var (a 0.2–1.0 float computed from the session's
/// turn depth). Rationale: an inline line's TRUE cost grows with the
/// session — it re-bills as cache-read on every remaining turn and brings
/// auto-compaction closer — so stash thresholds should TIGHTEN as the
/// session deepens. 1.0 (no-op) when unset/invalid. Disable adaptivity
/// entirely with TOKENADE_ADAPTIVE_THRESHOLDS=0 at the hook side.
pub fn pressure_factor() -> f64 {
    std::env::var("TOKENADE_PRESSURE")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .map(|f| f.clamp(0.2, 1.0))
        .unwrap_or(1.0)
}

/// Token threshold for this process. Reads `TOKENADE_DISCLOSE_THRESHOLD`
/// at every call so operators can flip the knob without restarting
/// long-lived MCP servers.
pub fn threshold_tokens() -> usize {
    if let Some(n) = THRESHOLD_ON_THIS_THREAD.with(std::cell::Cell::get) {
        return n;
    }
    if let Ok(v) = std::env::var("TOKENADE_DISCLOSE_THRESHOLD") {
        if let Ok(n) = v.parse::<usize>() {
            return n;
        }
    }
    (DEFAULT_THRESHOLD_TOKENS as f64 * pressure_factor()) as usize
}

thread_local! {
    static THRESHOLD_ON_THIS_THREAD: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// A threshold for THIS thread only, until the guard drops. Tests used to set
/// `TOKENADE_DISCLOSE_THRESHOLD` for the whole process, and every test that
/// reads the threshold in parallel inherited `20` — three of them red in one
/// full run (2026-10-03), green alone.
pub fn threshold_on_this_thread(tokens: usize) -> ThresholdGuard {
    ThresholdGuard(THRESHOLD_ON_THIS_THREAD.with(|c| c.replace(Some(tokens))))
}

/// Restores the thread's previous threshold override on drop.
pub struct ThresholdGuard(Option<usize>);

impl Drop for ThresholdGuard {
    fn drop(&mut self) {
        THRESHOLD_ON_THIS_THREAD.with(|c| c.set(self.0));
    }
}

/// Does this full rendering fit under the delivery threshold — i.e. would it
/// be delivered whole? A compactor that holds a lossy cut back while this is
/// true loses nothing it was going to deliver.
pub fn fits(rendering: &str) -> bool {
    crate::tokenizer::estimate(rendering) <= threshold_tokens()
}
