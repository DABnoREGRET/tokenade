//! `systemd_analyze_verify` — systemd analyze verify.
//!
//! Form-aware: keeps this command's specific detector, but folds its
//! output by detected shape — columnar table (surfaces problem rows),
//! key/value records (never cut mid-record), else head+tail (the final
//! outcome line is never decapitated). Replaces the blind head-slice.
const HEAD: usize = 30;

pub fn detect(text: &str) -> bool {
    text.lines().any(|l| l.contains(':') && (l.contains("Warning") || l.contains("Error") || l.contains(".service")))
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    crate::archetypes::form_aware(text, HEAD, 5, "systemd-analyze-verify", "verify issue(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detector_rejects_empty() { assert!(!detect("")); }

    #[test]
    fn detector_accepts_specific_format() { assert!(detect("/lib/systemd/system/myservice.service:5: Unknown key name 'ExecStop'")); }

    #[test]
    fn short_output_passes_through() {
        let s: String = std::iter::repeat_n("/lib/systemd/system/myservice.service:5: Unknown key name 'ExecStop'", 3).map(|l| format!("{l}\n")).collect();
        assert!(try_compact(&s).is_none());
    }

    #[test]
    fn long_output_folds_non_decapitating() {
        let s: String = std::iter::repeat_n("/lib/systemd/system/myservice.service:5: Unknown key name 'ExecStop'", HEAD + 20).map(|l| format!("{l}\n")).collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("[tokenade:systemd-analyze-verify"));
    }
}
