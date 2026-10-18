//! `tree` — recursive directory listing as an ASCII art tree.
//!
//! The signal is the *shape* of the hierarchy (which directories exist,
//! how they nest) plus the trailing `N directories, M files` summary.
//! The noise is a flat directory holding hundreds of sibling leaf files:
//! a generic head-slice would decapitate the bottom of the tree —
//! exactly where the summary and the last directories live.
//!
//! We fold long *runs of consecutive leaf siblings at the same depth*
//! into a count, keeping a few at each end so the alphabetical bounds
//! survive. Every directory line (anything with deeper children below
//! it), the root, and the summary footer are kept verbatim.
//!
//! Detection is purely structural: the box-drawing connectors
//! `├── ` / `└── ` and the `│   ` / `    ` indent blocks are emitted
//! identically in every locale, so nothing here matches English text.

use std::fmt::Write;

/// How many consecutive same-depth leaf siblings before we fold.
const FOLD_THRESHOLD: usize = 16;
/// How many to keep at each end of a folded run.
const KEEP_EACH_END: usize = 6;

const CONNECTORS: [&str; 2] = ["├── ", "└── "];

/// Returns the char offset of the entry name (i.e. the indent width) and
/// the name itself, if `line` is a tree branch line. `None` for the root
/// line and the summary footer (no connector).
fn branch_name_offset(line: &str) -> Option<usize> {
    for c in CONNECTORS {
        if let Some(byte_idx) = line.find(c) {
            // Everything up to and including the connector is indentation.
            let prefix = &line[..byte_idx + c.len()];
            return Some(prefix.chars().count());
        }
    }
    None
}

pub fn detect(text: &str) -> bool {
    // At least three branch lines — enough to be an actual tree and not a
    // stray box-drawing character in some other output.
    text.lines()
        .filter(|l| CONNECTORS.iter().any(|c| l.contains(c)))
        .count()
        >= 3
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }

    let lines: Vec<&str> = text.lines().collect();
    // Per-line depth (indent width / 4); `None` for non-branch lines
    // (root, blank lines, footer).
    let depths: Vec<Option<usize>> = lines
        .iter()
        .map(|l| branch_name_offset(l).map(|w| w / 4))
        .collect();

    // A branch line is a *leaf* when the next branch line is not deeper.
    // Computed in a single backward pass (O(n)) to stay fast on the 16MB
    // cap: track the depth of the next branch line seen so far.
    let mut leaf = vec![false; lines.len()];
    let mut next_branch_depth: Option<usize> = None;
    for i in (0..lines.len()).rev() {
        if let Some(d) = depths[i] {
            leaf[i] = next_branch_depth.is_none_or(|nd| nd <= d);
            next_branch_depth = Some(d);
        }
    }
    let is_leaf = |i: usize| leaf[i];

    let total_leaves = leaf.iter().filter(|&&l| l).count();
    if total_leaves < FOLD_THRESHOLD {
        // Nothing worth folding — the central guard would strip our banner.
        return Some(crate::archetypes::passthrough(text));
    }

    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    let mut folded_total = 0usize;
    while i < lines.len() {
        // Try to start a run of same-depth consecutive leaf siblings.
        if is_leaf(i) {
            let d = depths[i].unwrap();
            let mut j = i;
            while j < lines.len() && is_leaf(j) && depths[j] == Some(d) {
                j += 1;
            }
            let run = j - i;
            if run > FOLD_THRESHOLD {
                for k in i..i + KEEP_EACH_END {
                    out.push_str(lines[k]);
                    out.push('\n');
                }
                let hidden = run - 2 * KEEP_EACH_END;
                folded_total += hidden;
                // Indent the banner to the run's depth (4 chars/level),
                // using the `│  `-style filler so it lines up visually.
                let indent: String = "│   ".repeat(d);
                let _ = writeln!(out, "{indent}... ({hidden} sibling entries folded) ...");
                for k in j - KEEP_EACH_END..j {
                    out.push_str(lines[k]);
                    out.push('\n');
                }
                i = j;
                continue;
            }
        }
        out.push_str(lines[i]);
        out.push('\n');
        i += 1;
    }

    if folded_total == 0 {
        // Leaves were plentiful but scattered (no single long run) — leave
        // the tree intact rather than emit an identical copy.
        return Some(crate::archetypes::passthrough(text));
    }
    Some(out.trim_end().to_string())
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    fn big_flat_tree() -> String {
        // root + one dir holding 100 leaf files + a sibling dir, then footer.
        let mut t = String::from("project\n");
        t.push_str("├── src\n");
        for n in 0..100 {
            // last leaf in the dir uses └──, rest ├──
            let conn = if n == 99 { "└──" } else { "├──" };
            let _ = writeln!(t, "│   {conn} file_{n:03}.rs");
        }
        t.push_str("└── README.md\n");
        t.push_str("\n2 directories, 101 files\n");
        t
    }

    #[test]
    fn detector_rejects_non_tree() {
        assert!(!detect(""));
        assert!(!detect("just\nsome\nplain\ntext\n"));
    }

    #[test]
    fn detector_accepts_tree() {
        assert!(detect(&big_flat_tree()));
    }

    #[test]
    fn folds_large_flat_dir_keeps_structure_and_footer() {
        let t = big_flat_tree();
        let out = try_compact(&t).unwrap();
        // Structure kept.
        assert!(out.contains("├── src"), "dir line kept");
        assert!(out.contains("└── README.md"), "sibling dir kept");
        // Footer never decapitated.
        assert!(out.contains("2 directories, 101 files"), "summary kept");
        // Bounds of the folded run survive.
        assert!(out.contains("file_000.rs"), "first leaf kept");
        assert!(out.contains("file_099.rs"), "last leaf kept");
        // Middle folded away into a count.
        assert!(out.contains("sibling entries folded"));
        assert!(!out.contains("file_050.rs"), "middle leaf folded");
        assert!(out.len() < t.len(), "must shrink");
    }

    #[test]
    fn small_tree_passes_through() {
        let t = "project\n├── a.rs\n├── b.rs\n└── c.rs\n\n0 directories, 3 files\n";
        let out = try_compact(t).unwrap();
        assert!(out.contains("a.rs") && out.contains("c.rs"));
        assert!(out.contains("0 directories, 3 files"));
    }

    #[test]
    fn nested_dirs_not_folded_as_leaves() {
        // Each dir holds exactly one child → every dir is a directory, not a
        // leaf; nothing should fold even with many of them.
        let mut t = String::from("root\n");
        let mut indent = String::new();
        for n in 0..30 {
            let _ = writeln!(t, "{indent}└── level_{n}");
            indent.push_str("    ");
        }
        t.push_str("\n30 directories, 0 files\n");
        let out = try_compact(&t).unwrap();
        // No fold banner: these are all directories (chain), not sibling leaves.
        assert!(!out.contains("folded"), "directory chain must not fold");
    }
}
