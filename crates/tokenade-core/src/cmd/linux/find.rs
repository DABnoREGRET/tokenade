//! `find` — find.
const HEAD: usize = 200;


pub fn detect(text: &str) -> bool {
    text.lines().any(|l| l.contains('/') || l.starts_with('.'))
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    super::git_listing::fold_paths(text, HEAD, "find", "find path(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_path() { assert!(detect("/path/to/file.txt")); }
    #[test] fn short_output_passes_through() {
        let s = "/path/to/file.txt\n/path/to/other.rs\n";
        assert!(try_compact(s).is_none());
    }
    #[test] fn long_output_folds() {
        let s: String = (0..205).map(|i| format!("/path/to/file{i}.txt\n")).collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("[tokenade:find]"));
        assert!(out.contains("(+5 more)"));
    }
}

#[cfg(test)]
mod platform_parity_tests {
    /// The macOS twins of these modules were literal copies that never received
    /// the tuning: a `HEAD` left at the scaffolded 60 and a generic `"line(s)"`
    /// noun. MEASURED 2026-08-04 — `find` kept 60 head paths on macOS against
    /// 200 on Linux, and every macOS module in the set carried the same 60.
    ///
    /// This pins the property that made it matter: the same input must fold the
    /// same way whichever platform module the dispatcher reaches.
    #[test]
    fn the_platform_twins_fold_identically() {
        let paths: String = (0..500)
            .map(|i| format!("crates/tokenade-core/src/m{i}.rs\n"))
            .collect();
        assert_eq!(
            crate::cmd::macos::find::try_compact(&paths),
            super::try_compact(&paths),
            "macOS `find` folds differently from linux"
        );
        // And the noun is the specific one, not the scaffolded placeholder.
        let out = super::try_compact(&paths).expect("should fold");
        assert!(out.contains("find path(s)"), "lost the specific noun: {out}");

        let table = "USER  SESSION  STATE\n".to_string()
            + &(0..80)
                .map(|i| format!("u{i}    s{i}    active\n"))
                .collect::<String>();
        assert_eq!(
            crate::cmd::macos::loginctl::try_compact(&table),
            crate::cmd::linux::loginctl::try_compact(&table),
            "macOS `loginctl` folds differently from linux"
        );
    }
}
