//! `rustup` — Rust toolchain manager status & list output.
//!
//! rustup's text is sectioned: `rustup show` prints `Default host:`,
//! `installed toolchains`, `active toolchain`, and an `installed targets:`
//! block that on a multi-target setup runs to 80+ indented lines; `rustup
//! target/component/toolchain list` print one entry per line, most of them
//! the long not-installed tail. The signal is the section structure plus
//! every *active / default / installed / override* entry; the noise is the
//! long run of plain available entries.
//!
//! We fold long runs of plain list entries (indented continuation lines or
//! bare target-triple lines) into a count, always surfacing any entry that
//! carries a state marker, and keep section headers / underlines / host /
//! summary lines verbatim. rustup's labels are emitted in English in every
//! locale (it is not localised, like rustc / cargo / libtest), so matching
//! them by text is locale-safe.

use std::fmt::Write;

/// Consecutive plain entries before a run is worth folding.
const FOLD_THRESHOLD: usize = 12;
/// How many entries to keep at the head of a folded run (they are sorted,
/// so the head preserves the alphabetical lower bound).
const KEEP_HEAD: usize = 6;

/// rustup-signature phrases (never localised).
const SIGNATURES: [&str; 6] = [
    "installed toolchains",
    "active toolchain",
    "Default host:",
    "installed targets",
    "no active toolchain",
    "rustup home:",
];

/// State markers that make a list entry signal — always kept verbatim.
const STATE_MARKERS: [&str; 4] = ["(active", "(default", "(installed", "(override"];

pub fn detect(text: &str) -> bool {
    if text.trim().is_empty() {
        return false;
    }
    // Signatures must be in the HEAD; the weaker `(installed)`/`(default)`
    // markers can be deep in a long `rustup target list`, so they count
    // only when the document's first line already looks like rustup output
    // (a bare target triple / list entry). Without the anchor, a marker in
    // chained output (`rustup show; cargo build`) claimed the whole doc.
    let head: String = text.lines().take(15).collect::<Vec<_>>().join("\n");
    if SIGNATURES.iter().any(|s| head.contains(s)) {
        return true;
    }
    let first_is_entry = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .is_some_and(is_list_entry);
    first_is_entry && STATE_MARKERS.iter().any(|m| text.contains(m))
}

/// A list-entry line eligible for folding: an indented continuation line
/// (section body) or a bare target-triple-ish token line.
fn is_list_entry(line: &str) -> bool {
    if line.trim().is_empty() {
        return false;
    }
    line.starts_with(' ') || line.starts_with('\t') || looks_like_triple(line.trim())
}

/// `aarch64-apple-darwin`, `x86_64-pc-windows-gnu`, … — at least three
/// hyphen-separated lowercase/digit segments in the first token.
fn looks_like_triple(s: &str) -> bool {
    let head = s.split_whitespace().next().unwrap_or("");
    let segs: Vec<&str> = head.split('-').collect();
    segs.len() >= 3
        && segs.iter().all(|seg| {
            !seg.is_empty()
                && seg
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        })
}

fn has_state_marker(line: &str) -> bool {
    STATE_MARKERS.iter().any(|m| line.contains(m))
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    let lines: Vec<&str> = text.lines().collect();
    let total_entries = lines.iter().filter(|l| is_list_entry(l)).count();
    if total_entries < FOLD_THRESHOLD {
        // Small status output (a plain `rustup show` / `toolchain list`) —
        // nothing worth folding; the central guard would strip our banner.
        return Some(crate::archetypes::passthrough(text));
    }

    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    let mut folded_total = 0usize;
    while i < lines.len() {
        // Start a run of consecutive plain (no state marker) list entries.
        if is_list_entry(lines[i]) && !has_state_marker(lines[i]) {
            let mut j = i;
            while j < lines.len() && is_list_entry(lines[j]) && !has_state_marker(lines[j]) {
                j += 1;
            }
            let run = j - i;
            if run > FOLD_THRESHOLD {
                for k in i..i + KEEP_HEAD {
                    out.push_str(lines[k]);
                    out.push('\n');
                }
                let hidden = run - KEEP_HEAD;
                folded_total += hidden;
                let _ = writeln!(out, "  ... ({hidden} more entries folded) ...");
                i = j;
                continue;
            }
        }
        out.push_str(lines[i]);
        out.push('\n');
        i += 1;
    }
    if folded_total == 0 {
        // Entries plentiful but scattered (no single long run) — leave it
        // intact rather than emit an identical copy.
        return Some(crate::archetypes::passthrough(text));
    }
    Some(out.trim_end().to_string())
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    fn rustup_show_many_targets() -> String {
        let mut t = String::from("Default host: x86_64-unknown-linux-gnu\n");
        t.push_str("rustup home:  /home/u/.rustup\n\n");
        t.push_str("installed toolchains\n--------------------\n");
        t.push_str("stable-x86_64-unknown-linux-gnu (active, default)\n");
        t.push_str("nightly-x86_64-unknown-linux-gnu\n\n");
        t.push_str("active toolchain\n----------------\n");
        t.push_str("name: stable-x86_64-unknown-linux-gnu\n");
        t.push_str("installed targets:\n");
        for i in 0..40 {
            let _ = writeln!(t, "  target-{i:02}-unknown-linux-gnu");
        }
        t
    }

    #[test]
    fn detector_rejects_empty_and_plain() {
        assert!(!detect(""));
        assert!(!detect("just some unrelated text\n"));
    }

    #[test]
    fn detector_accepts_show() {
        assert!(detect(&rustup_show_many_targets()));
    }

    #[test]
    fn folds_long_target_block_keeps_sections() {
        let t = rustup_show_many_targets();
        let out = try_compact(&t).unwrap();
        // Section structure preserved.
        assert!(out.contains("Default host: x86_64-unknown-linux-gnu"));
        assert!(out.contains("installed toolchains"));
        assert!(out.contains("active toolchain"));
        assert!(out.contains("installed targets:"));
        // Active toolchain (state marker) surfaced.
        assert!(out.contains("stable-x86_64-unknown-linux-gnu (active, default)"));
        // Long plain target run folded.
        assert!(out.contains("more entries folded"));
        assert!(out.contains("target-00-unknown-linux-gnu"), "head bound kept");
        assert!(!out.contains("target-30-unknown-linux-gnu"), "middle folded");
        assert!(out.len() < t.len(), "must shrink");
    }

    #[test]
    fn target_list_folds_available_surfaces_installed() {
        // `rustup target list` — flat triple-per-line, most not installed.
        let mut t = String::new();
        for i in 0..30 {
            let _ = writeln!(t, "avail-{i:02}-unknown-linux-gnu");
        }
        t.push_str("x86_64-unknown-linux-gnu (installed)\n");
        let out = try_compact(&t).unwrap();
        assert!(out.contains("x86_64-unknown-linux-gnu (installed)"), "installed surfaced");
        assert!(out.contains("more entries folded"));
        assert!(out.len() < t.len());
    }

    #[test]
    fn short_status_passes_through() {
        let t = "stable-x86_64-unknown-linux-gnu (active, default)\n\
                 nightly-x86_64-unknown-linux-gnu\n";
        // Detected (state marker) but too few entries to fold → passthrough.
        let out = try_compact(t).unwrap();
        assert!(out.contains("active, default"));
    }
}
