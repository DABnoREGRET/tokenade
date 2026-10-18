//! Dispatcher for `jq`.
//! Extracted from `filter.rs` in the L108 refactor.

use crate::compress::generic;
use crate::compress::dedup_lines;
use crate::argv::has_any_flag;
use crate::ansi::strip_ansi;

pub fn dispatch(argv: &[String], output: &str) -> String {
    // Raw-output → not JSON; just compact whitespace-aggressively.
    if has_any_flag(argv, &["-r", "--raw-output", "-R", "--raw-input"]) {
        let s = strip_ansi(output);
        return dedup_lines(&s);
    }
    // Otherwise the output is JSON. Auto detects + compacts.
    let (_, compacted, _) = crate::auto::compact(output);
    if compacted.len() < output.len() {
        compacted
    } else {
        generic(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn raw_output_strips_and_dedups() {
        // -r / --raw-output emits plain strings (not JSON). Strip ANSI
        // + dedup duplicate lines.
        let body = "\x1b[32malice\x1b[0m\nbob\nbob\nbob\ncarol\n";
        for flag in &["-r", "--raw-output"] {
            let out = dispatch(&argv(&["jq", flag, ".name"]), body);
            assert!(!out.contains("\x1b["), "{flag} must strip ANSI");
            assert_eq!(
                out.matches("bob").count(),
                1,
                "{flag} must dedup; got: {out:?}"
            );
            assert!(out.contains("alice") && out.contains("carol"));
        }
    }

    #[test]
    fn raw_input_also_strips_and_dedups() {
        // -R / --raw-input: same path (non-JSON output expected).
        let body = "line1\nline1\nline1\nline2\n";
        for flag in &["-R", "--raw-input"] {
            let out = dispatch(&argv(&["jq", flag, "split(\",\")"]), body);
            assert_eq!(
                out.matches("line1").count(),
                1,
                "{flag} must dedup; got: {out:?}"
            );
        }
    }

    #[test]
    fn default_routes_through_auto_when_shrinks() {
        // Default jq output IS JSON → auto's JSON compactor takes
        // over. Build a 30-element array so auto's elider triggers.
        let body = format!(
            "[\n{}\n]\n",
            (0..30).map(|i| format!("  {{\"id\":{i}}}")).collect::<Vec<_>>().join(",\n")
        );
        let out = dispatch(&argv(&["jq", ".[]"]), &body);
        // auto's JSON compactor folds the array into a lossless TOON block.
        assert!(out.contains("toon"));
    }

    #[test]
    fn default_falls_back_to_generic_when_auto_cannot_shrink() {
        // Tiny JSON that auto can't compact → falls to generic
        // (which preserves content).
        let body = "1\n2\n3\n";
        let out = dispatch(&argv(&["jq", "."]), body);
        assert!(!out.is_empty());
        // Content preserved.
        assert!(out.contains('1') && out.contains('2') && out.contains('3'));
    }
}
