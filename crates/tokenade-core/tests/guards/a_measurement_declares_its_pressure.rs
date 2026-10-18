//! **A measurement declares the pressure at which it measures.**
//!
//! `disclose::threshold_tokens()` multiplies its threshold by
//! `TOKENADE_PRESSURE`, a value from 0.2 to 1.0 that the hook entry points set
//! from the session's depth. In production that is right: a line delivered in
//! clear is re-billed as a cache read on every remaining turn, so the
//! threshold must tighten as the session grows longer.
//!
//! For a TEST SUITE it is poison, because the variable is inherited from the
//! environment of whoever runs the suite. MEASURED 2026-09-13, the suite run
//! from a shell wrapped by tokenade (`TOKENADE_PRESSURE=0.75`): five pages of
//! the corpus — `ikea_cat`, `bbc_article`, `fastapi_leaf`, `aljazeera_ar`,
//! `dogforum` — fell under the stash threshold and were delivered as a
//! SUMMARY instead of whole. `no_fabricated_token` dropped to **370 numbers
//! examined out of the 500 required**, and `uniform_holes_are_declared_once`
//! with it. A red that said nothing about the code, and seven bisection
//! passes to name it — including several false readings due to binaries
//! older than the tree.
//!
//! `.cargo/config.toml` sets `TOKENADE_PRESSURE = 1.0` with `force = true`
//! (cargo, not nextest: version 0.9.143 ignores an `env` key). This test is
//! the net on the other side: it does not check the FILE, it checks the
//! value that REALLY reaches a test process, whatever the runner. A guard
//! reading the declaration does not see what the process received.

/// The threshold seen by a test process is the one WRITTEN in the code, not a
/// fraction decided by the session of whoever runs the suite.
#[test]
fn the_suite_measures_at_the_declared_pressure() {
    let seen = std::env::var("TOKENADE_PRESSURE").unwrap_or_else(|_| "<unset>".into());
    let threshold = tokenade_core::disclose::threshold_tokens();
    assert_eq!(
        threshold,
        tokenade_core::disclose::DEFAULT_THRESHOLD_TOKENS,
        "this test process measures at a stash threshold of {threshold} \
         instead of {}. `TOKENADE_PRESSURE` is \"{seen}\" here — it must be \
         1.0, and `.cargo/config.toml` sets it with `force = true` precisely \
         to override the calling environment's. A suite measuring at its \
         runner's pressure does not measure the code.",
        tokenade_core::disclose::DEFAULT_THRESHOLD_TOKENS
    );
}
