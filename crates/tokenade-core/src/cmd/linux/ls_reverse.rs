//! `ls -r` — short listing in reversed sort order.
//!
//! Real command-specific compactor (was: generic `line_cap(80)`). `-r` only
//! reverses the sort; the shape is still one entry per line, so it delegates
//! to the shared short-listing engine in [`super::ls_basic`]: head kept
//! verbatim in (reversed) order, tail folded into a per-extension count
//! summary with a true total — labelled `ls -r`.
//!
//! Multilingual: structural extension grouping only; accented / CJK names
//! survive verbatim, no English word matched.

pub fn detect(text: &str) -> bool {
    super::ls_basic::detect(text)
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    super::ls_basic::fold_listing(text, "ls -r")
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    fn build(n: usize) -> String {
        let mut s = String::new();
        for i in 0..n {
            let _ = writeln!(s, "entrée-é{i:04}.yaml");
        }
        s
    }

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("   \n"));
    }

    #[test]
    fn try_compact_some_or_none() {
        let s = "sample output line\nsecond line\n";
        let _ = try_compact(s);
        let _ = compact(s);
        let _ = compact_with_savings(s);
    }

    #[test]
    fn quality_label_total_order_and_locale_preserved() {
        let out = compact(&build(120));
        assert!(out.contains("[tokenade:ls -r]"), "wrong banner label: {out}");
        assert!(out.contains("120 entries"), "true total lost: {out}");
        let head_end = out.find("… (+").unwrap();
        let head = &out[..head_end];
        let p0 = head.find("entrée-é0000.yaml").expect("head[0] missing");
        let p1 = head.find("entrée-é0001.yaml").expect("head[1] missing");
        assert!(p0 < p1, "head order not preserved: {head}");
        assert!(out.contains(".yaml ×"), "tail composition lost under locale: {out}");
        assert!(!out.contains("entrée-é0119.yaml"), "deep entry should fold: {out}");
    }

    #[test]
    fn savings_on_large_listing() {
        let (_, sav) = compact_with_savings(&build(3000));
        let pct = 100 * (sav.tokens_before - sav.tokens_after) / sav.tokens_before;
        assert!(pct >= 70, "large ls -r only saved {pct}%");
    }
}
