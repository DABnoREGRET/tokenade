//! `net_macos` — net macos.
//!
//! Form-aware: keeps this command's specific detector, but folds its
//! output by detected shape — columnar table (surfaces problem rows),
//! key/value records (never cut mid-record), else head+tail (the final
//! outcome line is never decapitated). Replaces the blind head-slice.
const HEAD: usize = 60;

pub fn detect(text: &str) -> bool {
    text.contains(':') || text.contains('.')
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    // `net start` / `net user` / `net share` list NAMES — each line is an
    // answer. Measured 2026-10-03 on the corpus `windows/net_start_full`:
    // 88 services in 3.4 KB, 25 cut by the head/tail fold. Folded only past
    // the delivery threshold.
    if tk_base::delivery::fits(text) {
        return None;
    }
    crate::archetypes::form_aware(text, HEAD, 5, "net", "line(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detector_rejects_empty() { assert!(!detect("")); }

    #[test]
    fn detector_accepts_specific_format() { assert!(detect("inet 192.168.1.1/24")); }

    #[test]
    fn short_output_passes_through() {
        let s: String = std::iter::repeat_n("inet 192.168.1.1/24", 3).map(|l| format!("{l}\n")).collect();
        assert!(try_compact(&s).is_none());
    }

    #[test]
    fn long_output_folds_non_decapitating() {
        // Past the delivery threshold; under it the list is left whole.
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        let s: String = std::iter::repeat_n("inet 192.168.1.1/24", HEAD + 20).map(|l| format!("{l}\n")).collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("[tokenade:net"));
    }

    /// `net start`: every service named while the list fits.
    #[test]
    fn a_service_list_that_fits_is_left_whole() {
        let _g = tk_base::delivery::threshold_on_this_thread(5_000);
        let mut t = String::from("Ces services Windows sont démarrés :\n\n");
        for i in 0..88 {
            t.push_str(&format!("   Service number {i} of the machine\n"));
        }
        t.push_str("\nLa commande s'est terminée correctement.\n");
        assert!(try_compact(&t).is_none());
    }
}
