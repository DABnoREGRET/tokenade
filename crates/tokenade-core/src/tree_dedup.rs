//! Structural sibling de-duplication ON THE PARSED TREE.
//!
//! Same contract as [`crate::dom_dedup`] — drop runs of adjacent siblings whose
//! rendered contribution is identical, because CSS marquees and responsive
//! image sets repeat content for presentational reasons — but computed on the
//! `scraper` tree the converter has ALREADY parsed, and applied as a walk-time
//! skip instead of a byte splice.
//!
//! Why: `dom_dedup` runs tree-sitter over the raw HTML as a pre-pass, a second
//! full parse of every page in a second parser. Measured on the 147-page
//! corpus it was **2.4 s of 6.2 s (39 %)** while changing the output of only
//! **10 pages** — the cost is the parse, not the detection, so no prefilter can
//! recover it (a repeated-text prefilter passes 131/147 pages). Folding the
//! same analysis into the existing parse removes the whole second parse.
//!
//! The thresholds, the protected tag list and the image-only relaxation are the
//! ones `dom_dedup` earned on the corpus; see that module for why each exists.
//!
//! `dom_dedup`'s 4 MiB bail-out is kept, in the caller
//! (`web_tree::MAX_DEDUP_BYTES`). Its original reason — the cost of a
//! tree-sitter parse — does not apply here, but it also bounds the signature
//! map and the depth of the two recursive walks below, and neither of those
//! wants to meet a 30 MB generated document.

use ego_tree::NodeRef;
use scraper::Node;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use crate::dom_dedup::{pick_runs, Sig, NEVER_DEDUP};

/// Node ids whose subtree is a duplicate copy and must not be walked.
pub type DropSet = std::collections::HashSet<ego_tree::NodeId>;

/// Find every presentational duplicate run under `root`.
///
/// One post-order pass builds every signature (folding a parent's from its
/// children's, so each node is visited once — the quadratic version cost
/// `dom_dedup` 78 ms/page before it was fixed), then one pass per sibling list
/// picks the runs.
pub fn duplicate_subtrees(root: NodeRef<Node>) -> DropSet {
    let mut sigs: HashMap<ego_tree::NodeId, Sig> = HashMap::new();
    build_signatures(root, &mut sigs);
    let mut drops = DropSet::new();
    collect_drops(root, &sigs, &mut drops);
    drops
}

use crate::web_tree::el_name;

/// Whitespace-normalised word digest — the unit both text nodes and `alt`
/// values reduce to, so two copies that differ only in indentation match.
fn word_digest(raw: &str) -> (u64, usize) {
    let mut hasher = DefaultHasher::new();
    let mut chars = 0usize;
    for word in raw.split_whitespace() {
        word.hash(&mut hasher);
        chars += word.chars().count();
    }
    (if chars == 0 { 0 } else { hasher.finish() }, chars)
}

fn build_signatures(node: NodeRef<Node>, out: &mut HashMap<ego_tree::NodeId, Sig>) -> Sig {
    // Same unbounded-nesting hazard as the markdown walk, and this pass runs
    // FIRST (`parse_for_select`), so capping only `web_tree` would leave the
    // stack overflow reachable through the shipped entry point. `MAX_DEDUP_BYTES`
    // is not a bound here: 4 MiB of `<div>` is ~800 000 levels of nesting.
    // An unanalysed subtree simply never gets de-duplicated — the walk still
    // renders it, so the failure mode is "misses a repeat", not lost content.
    let _guard = match crate::web_tree::DepthGuard::enter() {
        Some(g) => g,
        None => return Sig::default(),
    };
    match node.value() {
        Node::Text(t) => {
            let (digest, chars) = word_digest(&t.text);
            return Sig {
                digest,
                chars,
                text_chars: chars,
            };
        }
        Node::Element(e) => {
            // Script/style bodies are not content and must not make two
            // otherwise-identical cards look different.
            if matches!(e.name(), "script" | "style") {
                return Sig::default();
            }
        }
        // Document / Fragment carry the children — fold them below. Comments,
        // doctypes and PIs render nothing.
        Node::Document | Node::Fragment => {}
        _ => return Sig::default(),
    }

    let mut hasher = DefaultHasher::new();
    let mut chars = 0usize;
    let mut text_chars = 0usize;
    // `alt` is the ONLY thing an image contributes, which is exactly why four
    // `<img>` differing solely in `src`/`width` render as four identical lines.
    // `src`/`class`/`width` are deliberately excluded.
    if let Node::Element(e) = node.value() {
        if let Some(alt) = e.attr("alt") {
            let (digest, n) = word_digest(alt);
            if n > 0 {
                digest.hash(&mut hasher);
                chars += n;
            }
        }
    }
    for child in node.children() {
        let sig = build_signatures(child, out);
        if sig.chars == 0 {
            continue;
        }
        sig.digest.hash(&mut hasher);
        chars += sig.chars;
        text_chars += sig.text_chars;
    }
    let sig = Sig {
        digest: if chars == 0 { 0 } else { hasher.finish() },
        chars,
        text_chars,
    };
    out.insert(node.id(), sig);
    sig
}

fn collect_drops(node: NodeRef<Node>, sigs: &HashMap<ego_tree::NodeId, Sig>, drops: &mut DropSet) {
    let _guard = match crate::web_tree::DepthGuard::enter() {
        Some(g) => g,
        None => return,
    };
    let elements: Vec<NodeRef<Node>> = node.children().filter(|c| el_name(*c).is_some()).collect();
    if !elements.is_empty() {
        let sigs_of: Vec<Sig> = elements
            .iter()
            .map(|e| sigs.get(&e.id()).copied().unwrap_or_default())
            .collect();
        let protected: Vec<bool> = elements
            .iter()
            .map(|e| el_name(*e).is_some_and(|t| NEVER_DEDUP.contains(&t)))
            .collect();
        for (i, period, cycles) in pick_runs(&sigs_of, &protected) {
            // Keep one cycle, drop the rest.
            for e in &elements[i + period..i + cycles * period] {
                drops.insert(e.id());
            }
        }
    }

    // Not into a subtree just marked as a duplicate: it will never be walked,
    // so any run inside it is analysis nobody reads.
    for child in node.children() {
        if drops.contains(&child.id()) {
            continue;
        }
        collect_drops(child, sigs, drops);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scraper::Html;

    fn md(html: &str) -> String {
        crate::web_tree::select_and_convert(html, false)
    }

    #[test]
    fn collapses_a_marquee_track() {
        let unit = "<div><span>Medically reviewed content</span><span>130 medical reviewers in network</span></div>";
        let html = format!("<section>{}</section>", unit.repeat(8));
        let out = md(&html);
        assert_eq!(out.matches("130 medical reviewers").count(), 1);
        assert!(out.contains("Medically reviewed content"));
    }

    #[test]
    fn collapses_responsive_image_sets_sharing_one_alt() {
        let html = r#"<div>
            <img alt="Build agents on infrastructure that thinks like them" src="a.png">
            <img alt="Build agents on infrastructure that thinks like them" src="b.png">
            <img alt="Build agents on infrastructure that thinks like them" src="c.png">
            <img alt="Build agents on infrastructure that thinks like them" src="d.png">
        </div>"#;
        assert_eq!(
            md(html).matches("Build agents on infrastructure").count(),
            1
        );
    }

    #[test]
    fn collapses_a_multi_card_marquee_cycle() {
        let cards = "<div><span>Medically reviewed content</span></div>\
                     <div><span>130 medical reviewers in network</span></div>\
                     <div><span>19 years of experience</span></div>\
                     <div><span>50 million monthly readers</span></div>";
        let html = format!("<section>{}</section>", cards.repeat(8));
        let out = md(&html);
        for card in [
            "Medically reviewed content",
            "130 medical reviewers in network",
            "19 years of experience",
            "50 million monthly readers",
        ] {
            assert_eq!(out.matches(card).count(), 1, "{card} should survive once");
        }
    }

    #[test]
    fn keeps_a_cycle_that_repeats_only_twice() {
        let cards = "<div><span>Durable Orchestration here</span></div>\
                     <div><span>Sandboxed Environments here</span></div>";
        let html = format!("<section>{}</section>", cards.repeat(2));
        assert_eq!(md(&html).matches("Durable Orchestration").count(), 2);
    }

    #[test]
    fn never_collapses_table_rows() {
        let row = "<tr><td>Same cell text here</td></tr>";
        let html = format!("<table><tbody>{}</tbody></table>", row.repeat(5));
        let doc = Html::parse_document(&html);
        assert!(duplicate_subtrees(doc.tree.root()).is_empty());
    }
}
