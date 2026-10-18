//! `cache_frame` — honest, prompt-cache-aware framing of token savings.
//!
//! Tokenade's headline number ("N tokens saved") is only the *gross* story.
//! With prompt caching, the economics are subtler, and saying so builds trust
//! (the framing comes from a competitor comparison):
//!
//!   * **Fresh, uncacheable tool output** (a one-off `git log`, a web fetch, a
//!     command whose output changes every run) is where compaction pays full
//!     freight: every token folded is a token the model never has to read.
//!   * **A cached re-read** of identical context bills at the cache-read rate
//!     regardless of size — on Anthropic models, reading 50k cached tokens and
//!     reading 500 cached tokens cost nearly the same flat pittance. Compacting
//!     already-cached context saves little, and can even *cost* by busting the
//!     cache and forcing a re-write.
//!
//! So net savings ≈ gross savings on the *fresh* fraction of folded output.
//! This module makes that explicit. Pure: arithmetic + wording, no IO.

/// Net tokens actually saved once you account for the cached fraction.
///
/// `raw_saved` is the gross tokens folded; `fresh_fraction` in `[0.0, 1.0]` is
/// the share of that output the model would have read *uncached* (so where the
/// saving is real). The cached remainder would have billed at a flat
/// cache-read rate either way, so it contributes ~nothing to net savings.
pub fn net_fresh_savings(raw_saved: u64, fresh_fraction: f64) -> u64 {
    let f = fresh_fraction.clamp(0.0, 1.0);
    (raw_saved as f64 * f).round() as u64
}

/// A one-line trustworthy footer for savings displays. Scales its wording to
/// the magnitude so a tiny number doesn't get an overblown caption.
pub fn footer(raw_saved: u64) -> String {
    if raw_saved == 0 {
        return "no fresh output folded yet — savings accrue on uncacheable tool output.".into();
    }
    "net savings land on fresh, uncacheable tool output; a cached re-read bills \
     ~flat regardless of size, so already-cached context saves little."
        .into()
}

/// A compact, accurate disclosure pairing gross and net under an assumed
/// fresh fraction — what a `--cache-note` flag prints.
pub fn disclosure(raw_saved: u64, fresh_fraction: f64) -> String {
    let net = net_fresh_savings(raw_saved, fresh_fraction);
    let pct = (fresh_fraction.clamp(0.0, 1.0) * 100.0).round() as u64;
    format!(
        "gross {raw_saved} tok folded · ~{net} tok net at {pct}% fresh\n{}",
        footer(raw_saved)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn net_scales_with_fresh_fraction() {
        assert_eq!(net_fresh_savings(1000, 1.0), 1000);
        assert_eq!(net_fresh_savings(1000, 0.5), 500);
        assert_eq!(net_fresh_savings(1000, 0.0), 0);
    }

    #[test]
    fn net_clamps_out_of_range_fraction() {
        assert_eq!(net_fresh_savings(1000, 2.0), 1000);
        assert_eq!(net_fresh_savings(1000, -1.0), 0);
    }

    #[test]
    fn footer_distinguishes_zero() {
        assert!(footer(0).contains("no fresh output"));
        assert!(footer(500).contains("cached re-read"));
    }

    #[test]
    fn disclosure_mentions_gross_net_and_fraction() {
        let d = disclosure(1000, 0.6);
        assert!(d.contains("1000"));
        assert!(d.contains("600"));
        assert!(d.contains("60%"));
    }
}
