//! Accessibility-tree snapshots, as served by a browser automation tool.
//!
//! Deeply nested, hugely repetitive, and the useful part is the labelled
//! nodes — so the tree is re-parented and the noise collapsed.

use super::*;

/// Compact a Playwright `browser_snapshot` accessibility-tree dump.
///
/// `[ref=eN]` markers are PRESERVED: they are what `browser_click` /
/// `browser_type` target, and an icon button with no accessible name can ONLY
/// be addressed by its ref — dropping them would silently break interaction on
/// such elements (correctness > the ~16% extra savings stripping them gave).
/// The savings come from the structural noise instead: same-origin URL
/// prefixes, attribute noise, empty/single-child generic wrappers, and long
/// uniform list runs.
pub fn compact_a11y_tree(snapshot: &str) -> String {
    // Which refs the snapshot itself marked as responding to a click. Read from
    // the RAW input because `strip_attribute_noise` removes the marker, and
    // spent at the very END because a ref is what makes two otherwise identical
    // lines distinct: drop them before the folding passes and those passes
    // merge nodes that were never the same, re-attaching children to the wrong
    // parent. That is the one failure this compactor must never have.
    let clickable = refs_marked_clickable(snapshot);
    let snap = drop_url_lines_keeping_paths(snapshot);
    let snap = strip_attribute_noise(&snap);
    // Name folding first: it un-names containers whose name was pure
    // duplication, and an un-named wrapper is exactly what the next stage
    // exists to remove.
    let snap = drop_machine_metadata_names(&snap);
    let snap = fold_redundant_names(&snap);
    let snap = drop_empty_wrapper_nodes(&snap);
    // Before the wrapper collapse, not after: a link over its own `/url` line
    // is a TWO-line subtree, so the wrapper holding it does not look
    // single-child and survives. Inlining first turns it into one line and the
    // collapse then sees what is really there.
    let snap = inline_lone_url_children(&snap);
    let snap = collapse_single_child_generics(&snap);
    let snap = fold_uniform_runs(&snap);
    let snap = strip_dangling_colons(&snap);
    let snap = drop_refs_the_agent_cannot_click(&snap, &clickable);
    // Once more, and only here. `drop_refs_the_agent_cannot_click` is spent
    // LAST by design — a ref is what makes two otherwise identical lines
    // distinct, so removing them earlier merges nodes that were never the
    // same. But that means it CREATES contentless nodes after the pass that
    // removes them has run: `- img [ref=e11]` carries a ref while
    // `drop_empty_wrapper_nodes` looks at it, and comes out `- img` once the
    // ref is judged unclickable. Measured on the github fixture: 55 bare
    // images survived the whole chain for exactly this reason.
    let snap = drop_empty_wrapper_nodes(&snap);
    let snap = strip_dangling_colons(&snap);
    snap.trim().to_string()
}

pub fn compact_a11y_tree_with_savings(snapshot: &str) -> (String, crate::Savings) {
    let before = tokenizer::estimate(snapshot);
    let out = compact_a11y_tree(snapshot);
    let after = tokenizer::estimate(&out);
    (out, crate::Savings::new(before, after))
}
