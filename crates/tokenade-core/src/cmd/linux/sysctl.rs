//! `sysctl -a` — kernel parameter dump (1000s of `key = value` lines).
//!
//! Compaction: fold the long tail. Keep all non-default-looking
//! values? Too hard heuristically. Just cap at 100 with `more elided`.


pub fn detect(text: &str) -> bool {
    crate::archetypes::any_output(text)
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    crate::archetypes::head_lines_with_tally(text, 100, "sysctl key(s) elided — grep for a specific subtree")
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use std::fmt::Write;
    use super::*;

    #[test]
    fn detects_sysctl_dump() {
        let mut s = String::new();
        for i in 0..15 {
            let _ = writeln!(s, "kernel.foo.bar.baz{i} = {i}");
        }
        assert!(detect(&s));
    }

    #[test]
    fn folds_long_list() {
        // Past the delivery threshold; under it the list is delivered whole.
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        let mut s = String::new();
        for i in 0..200 {
            let _ = writeln!(s, "kernel.foo.bar.{i} = {i}");
        }
        let out = compact(&s);
        assert!(out.contains("100 more"));
    }
}
