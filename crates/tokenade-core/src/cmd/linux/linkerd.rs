//! `linkerd` — shared compactor for Linkerd CLI subcommand output.
//!
//! Linkerd CLI subcommands produce tables, install manifests, and
//! short status messages. The shared compactor caps at LINKERD_LINE_CAP
//! non-empty lines.
//!
//! All generic `linkerd_*` subcommand files delegate to this module.

const LINKERD_LINE_CAP: usize = 60;

pub fn detect(text: &str) -> bool {
    crate::archetypes::any_output(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    Some(compact(text))
}

pub fn compact(text: &str) -> String {
    crate::archetypes::line_cap(text, LINKERD_LINE_CAP, "row(s)")
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_short() {
        let s = "Linkerd control plane is healthy\n";
        let out = compact(s);
        assert!(out.contains("healthy"), "short output must pass through");
    }

    #[test]
    fn rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("  \n"));
    }
}
