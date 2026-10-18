// The tests of `fold_count`.
//
// Spliced in by `include!` from `fold_count.rs`. Those of
// `pick_compact_or_raw` followed their subject when it moved out of
// `process_util`, which stayed in the `tk-base` crate.

#[cfg(test)]
mod fold_accounting {
    #[test]
    fn pick_compact_or_raw_keeps_compact_when_smaller() {
        let raw = "x".repeat(100);
        let compacted = "y".repeat(40);
        let sav = crate::Savings::new(100, 40);
        let (out, new_sav) =
            crate::fold_count::pick_compact_or_raw(raw, compacted.clone(), sav, || {
                unreachable!("should not estimate")
            });
        assert_eq!(out, compacted);
        assert_eq!(new_sav.tokens_before, 100);
        assert_eq!(new_sav.tokens_after, 40);
    }
    #[test]
    fn pick_compact_or_raw_falls_back_to_raw_when_inflated() {
        let raw = "abc".to_string();
        let compacted = "[banner] abc [/banner]".to_string();
        // Inflation: 5→20 tokens, the typical "tiny output + heavy
        // banner" scenario.
        let sav = crate::Savings::new(5, 20);
        let (out, new_sav) =
            crate::fold_count::pick_compact_or_raw(raw.clone(), compacted, sav, || 5);
        assert_eq!(out, raw, "must serve raw when compactor inflated");
        // Savings reflect "no work done": tokens_before == tokens_after.
        assert_eq!(new_sav.tokens_before, 5);
        assert_eq!(new_sav.tokens_after, 5);
    }
    #[test]
    fn pick_compact_or_raw_equal_keeps_compact() {
        // Edge: when tokens_after == tokens_before (no win, no loss),
        // we prefer compact since it carries the structural banner
        // for the agent (per-format hint about what to expect).
        let raw = "data".to_string();
        let compacted = "[fmt] data".to_string();
        let sav = crate::Savings::new(10, 10);
        let (out, _) = crate::fold_count::pick_compact_or_raw(raw, compacted.clone(), sav, || {
            unreachable!("equal case shouldn't estimate")
        });
        assert_eq!(
            out, compacted,
            "equal-token case keeps compact (banner is signal)"
        );
    }
}
