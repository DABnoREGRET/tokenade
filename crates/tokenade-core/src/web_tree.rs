//! Tree-walk HTML → markdown, built on `scraper` (html5ever + ego-tree).
//!
//! This replaces the regex conversion chain in `web::convert_body` when
//! `TOKENADE_WEB_TREE=1`. It converts an already-SELECTED HTML region (the
//! main/article isolation + boilerplate strip still run upstream in the regex
//! pipeline for now) into markdown. The output shape matches what the regex
//! converters emit — headings `# `, flat `- ` bullets, `[image: alt]`,
//! `[text](url)`, fenced code — so the shared post-passes in `convert_body`
//! (link-density prune, UI-noise filter, whitespace squeeze) work unchanged.
//!
//! Why a tree: every `<tag\b[^>]*>` regex ends at the first `>`, which a literal
//! `>` inside a quoted attribute (Tailwind `[&>a]:hidden`) or a merged table
//! cell defeats. html5ever does spec-correct error recovery — the same thing a
//! browser does — so malformed HTML that broke the regex path parses cleanly.
//! The table walk is colspan/rowspan aware (P0.1), which the regex path is not.

use ego_tree::NodeRef;
use scraper::{Html, Node};
use std::cell::Cell;

/// Prefix a backslash to any line CommonMark would read as an ATX heading
/// (`#` through `######`, then a space, a tab, or end of line).
///
/// Only that marker. A leading `-` or `>` degrades the line to a list item or
/// a block quote, which changes how it reads and nothing else, while a `#`
/// re-parents every section that follows it. Escaping those too would put a
/// backslash on real pages for no structural gain: measured across the 39
/// captured page fixtures, one paragraph opens with `*` and none with `>`.
/// How many `#` open this line, when that count is a legal ATX heading level.
///
/// Callers add the condition their own question needs: `escape_atx_openers`
/// also demands the separator CommonMark requires, `linkify_first_heading`
/// demands text to put in the link.
fn atx_level(line: &str) -> Option<usize> {
    let hashes = line.len() - line.trim_start_matches('#').len();
    (1..=6).contains(&hashes).then_some(hashes)
}

fn escape_atx_openers(text: &str) -> std::borrow::Cow<'_, str> {
    fn opens_a_heading(line: &str) -> bool {
        // CommonMark allows up to three leading spaces, but this emitter never
        // indents paragraph text, so the marker is only ever at column 0.
        match atx_level(line) {
            Some(hashes) => line[hashes..]
                .chars()
                .next()
                .is_none_or(|c| c == ' ' || c == '\t'),
            None => false,
        }
    }
    if !text.split('\n').any(opens_a_heading) {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut escaped = String::with_capacity(text.len() + 8);
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            escaped.push('\n');
        }
        if opens_a_heading(line) {
            escaped.push('\\');
        }
        escaped.push_str(line);
    }
    std::borrow::Cow::Owned(escaped)
}

thread_local! {
    /// Gates `is_boilerplate`. The unskipped fallback walk sets it false so the
    /// two-walk guard in `select_and_convert` can measure how much skipping cost.
    static SKIP_BOILERPLATE: Cell<bool> = const { Cell::new(true) };
    /// Visible chars the skip walk dropped: each subtree refused by
    /// `is_boilerplate` is rendered ONCE with skipping off and its visible
    /// length accumulated here, so `select_and_convert` reconstructs the full
    /// walk's size (`out_vis + skipped_vis`) instead of walking the tree a
    /// second time. Every node is still visited exactly once overall.
    static SKIPPED_VIS: Cell<usize> = const { Cell::new(0) };
    /// Subtrees that are presentational duplicates of an earlier sibling run
    /// (`tree_dedup`). Removed from the walk exactly as the tree-sitter
    /// pre-pass used to remove them from the source bytes — so they are NOT
    /// boilerplate-skipped content and must not count toward `SKIPPED_VIS`.
    static DUP_DROPS: std::cell::RefCell<std::rc::Rc<crate::tree_dedup::DropSet>> =
        std::cell::RefCell::new(std::rc::Rc::new(crate::tree_dedup::DropSet::new()));
    /// Trimmed text length of the subtree the current `walk` was handed, or 0
    /// when unknown (`fragment_to_markdown` converts a region the CALLER already
    /// selected, so there is no whole-page proportion to measure against). Read
    /// by `spans_most_of_the_page`; 0 disables that guard.
    static ROOT_TEXT_LEN: Cell<usize> = const { Cell::new(0) };
    /// Current recursion depth of the block/inline walk. See `MAX_WALK_DEPTH`.
    static WALK_DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// Hard bound on how deep `block`/`inline_into`/`has_block_descendant` will
/// recurse. These are mutually recursive over the DOM, and NOTHING upstream
/// bounds nesting: html5ever builds its tree iteratively and happily accepts a
/// document that nests `<div>` tens of thousands deep, so a hostile or merely
/// broken page turns into a stack overflow — which aborts the process, not just
/// the request, and so cannot be caught.
///
/// MEASURED 2026-07-22 on the 40-page fidelity corpus: the deepest real document
/// is wikipedia_art.html at 34 levels, then fastapi_leaf.html at 30 and
/// ikea_cat.html at 29. 200 is ~6x the worst real page, so the cap can only be
/// reached by a document no reader was going to make sense of anyway. Content
/// below the cap is dropped silently — that is the point: a truncated page beats
/// a killed process.
///
/// NOTE this is NOT covered by the explicit stack introduced for the JS engine
/// (`js_render`); that bounds QuickJS's own recursion, not our tree walk.
const MAX_WALK_DEPTH: usize = 200;

/// RAII depth counter. `enter` returns `None` once `MAX_WALK_DEPTH` is reached,
/// and the caller must return without descending. A guard rather than a manual
/// decrement so an early `return` from any of `block`'s many arms cannot leak a
/// level and slowly starve the budget on a long page.
pub(crate) struct DepthGuard;

impl DepthGuard {
    pub(crate) fn enter() -> Option<Self> {
        WALK_DEPTH.with(|d| {
            let v = d.get();
            if v >= MAX_WALK_DEPTH {
                return None;
            }
            d.set(v + 1);
            Some(DepthGuard)
        })
    }
}

impl Drop for DepthGuard {
    fn drop(&mut self) {
        WALK_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

/// Trimmed text length of a subtree — the measure `find_main_root` compares
/// candidates with, shared so the isolation test and the boilerplate size guard
/// can never disagree about how big a block is.
fn text_len(n: NodeRef<Node>) -> usize {
    n.descendants()
        .filter_map(|d| match d.value() {
            Node::Text(t) => Some(t.text.trim().len()),
            _ => None,
        })
        .sum()
}

/// Non-whitespace char count — the size measure the misfire guard compares.
fn vis_len(s: &str) -> usize {
    s.chars().filter(|c| !c.is_whitespace()).count()
}

/// The tree converter (html5ever + our tree-walk) is the DEFAULT path. It passes
/// every hard fidelity floor on the 40-page battery, improves 36/38 pages, and
/// eliminates the regex converter's whole malformed-HTML bug class.
/// `TOKENADE_WEB_TREE=0` is the
/// escape hatch back to the legacy regex path for a page it mis-handles (a small
/// number of e-commerce spec *modals* the boilerplate ladder prunes — a
/// modal=boilerplate trade-off that is right on the vast majority of pages).
pub fn web_tree_enabled() -> bool {
    !matches!(
        std::env::var("TOKENADE_WEB_TREE").ok().as_deref(),
        Some("0") | Some("false") | Some("FALSE") | Some("off")
    )
}

/// Installs the duplicate-drop set for ONE tree and clears it on drop.
///
/// `NodeId` is an index into the tree that produced it, so a set left behind by
/// an earlier call is not merely stale — applied to the NEXT document it would
/// silently delete unrelated subtrees. A guard makes that impossible even if a
/// walk panics, and makes "no dedup" mean an EMPTY set rather than whatever the
/// previous page installed.
struct DupGuard;

impl DupGuard {
    fn install(drops: std::rc::Rc<crate::tree_dedup::DropSet>) -> Self {
        DUP_DROPS.with(|d| *d.borrow_mut() = drops);
        DupGuard
    }
}

impl Drop for DupGuard {
    fn drop(&mut self) {
        DUP_DROPS.with(|d| *d.borrow_mut() = std::rc::Rc::new(crate::tree_dedup::DropSet::new()));
    }
}

/// Convert an HTML fragment (a selected region) to markdown.
pub fn fragment_to_markdown(html: &str) -> String {
    // The code sanctuary (`web::sanctuary_code_blocks`) marks stashed `<pre>`/
    // `<code>` with `\x00CODE_FENCE_i\x00`, but html5ever replaces NUL in text
    // with U+FFFD per the HTML5 tokenizer spec, which would destroy those
    // placeholders and drop every code block. Swap NUL for a Private-Use char
    // across the parse, then swap back so the shared `restore_code_blocks`
    // still matches. (U+E057 collisions in real content are negligible.)
    const SENTINEL: char = '\u{E057}';
    let swapped = crate::html_nesting::limit_nesting(html).replace('\x00', "\u{E057}");
    let doc = Html::parse_fragment(&swapped);
    // This walks a DIFFERENT tree from any `select_and_convert` call, so it must
    // not inherit that tree's node ids.
    let _dup = DupGuard::install(std::rc::Rc::new(crate::tree_dedup::DropSet::new()));
    let mut out = String::new();
    block(doc.tree.root(), &mut out, 0);
    if out.contains(SENTINEL) {
        out = out.replace(SENTINEL, "\x00");
    }
    out
}

/// Full tree path: parse, isolate main content (when `aggressive`), then walk —
/// skipping boilerplate subtrees during the walk rather than mutating the DOM.
/// Replaces the regex SELECTION as well as conversion for the flagged path.
pub fn select_and_convert(html: &str, aggressive: bool) -> String {
    select_and_convert_parsed(&parse_for_select(html), aggressive)
}

/// A parsed document plus its duplicate-run analysis, ready for one or more
/// `select_and_convert_parsed` passes.
pub struct Doc {
    tree: Html,
    /// Computed ONCE per parse: the ladder runs the aggressive tier and then,
    /// when that comes back skeletal, the relaxed tier over the SAME tree, and
    /// recomputing every signature for the second pass threw away the parse-once
    /// win (F4) at the analysis step.
    drops: std::rc::Rc<crate::tree_dedup::DropSet>,
}

/// The largest input de-duplication will analyse — `dom_dedup`'s guard, kept.
/// The old bail-out was about the cost of a tree-sitter parse this module does
/// not pay, but it also bounds the signature map and the depth of two recursive
/// walks on a pathological document, which is worth keeping on its own.
const MAX_DEDUP_BYTES: usize = 4 * 1024 * 1024;

/// Parse once, with the NUL→sentinel swap `select_and_convert` relies on. The
/// aggressive and relaxed tiers differ only by ROOT, so the same parse serves
/// both (F4) — html5ever is the single most expensive step on a large page.
pub fn parse_for_select(html: &str) -> Doc {
    let tree =
        Html::parse_document(&crate::html_nesting::limit_nesting(html).replace('\x00', "\u{E057}"));
    let drops = if dedup_enabled() && html.len() <= MAX_DEDUP_BYTES {
        crate::tree_dedup::duplicate_subtrees(tree.tree.root())
    } else {
        crate::tree_dedup::DropSet::new()
    };
    Doc {
        tree,
        drops: std::rc::Rc::new(drops),
    }
}

/// `select_and_convert` on an already-parsed document.
pub fn select_and_convert_parsed(doc: &Doc, aggressive: bool) -> String {
    const SENTINEL: char = '\u{E057}';
    let root = if aggressive {
        find_main_root(&doc.tree).unwrap_or_else(|| doc.tree.tree.root())
    } else {
        doc.tree.tree.root()
    };
    // Misfire guard (mirrors the regex path's MIN_KEPT_AFTER_STRIP): if
    // boilerplate-skip removed most — or ALL — of the content, the keyword set
    // matched a content wrapper (a content section named like chrome, or a
    // whole-body wrapper as on react.dev), so fall back to the unskipped walk.
    // Dropping real content is far worse than keeping some chrome. Runs even
    // when the skip walk is empty, which is exactly when it's most needed.
    //
    // The full walk it compares against is RECONSTRUCTED, not re-walked (F2):
    // the skip walk accounts for every subtree it dropped, so
    // `full_vis == out_vis + skipped_vis`. A real second walk happens only when
    // the ratio lands near the threshold — where the fallback string is needed
    // anyway — so the common case pays one traversal instead of two.
    //
    // Presentational duplicate runs are found on THIS parse instead of a second
    // full parse in tree-sitter (see `tree_dedup`).
    let _dup = DupGuard::install(doc.drops.clone());
    SKIPPED_VIS.with(|c| c.set(0));
    let mut out = walk(root, true);
    let skipped_vis = SKIPPED_VIS.with(|c| c.get());
    if skipped_vis > 0 {
        let out_vis = vis_len(&out);
        let full_vis = out_vis + skipped_vis;
        // Margin above 0.6 so a rounding-level discrepancy between the
        // reconstruction and a real walk can never flip the verdict: inside the
        // band we re-walk and apply the exact original test.
        if (out_vis as f64) < (full_vis as f64) * 0.65 {
            let full = walk(root, false);
            if (out_vis as f64) < (vis_len(&full) as f64) * 0.6 {
                out = full;
            }
        }
    }
    out = trim_leading_residue(&out);
    if out.contains(SENTINEL) {
        out = out.replace(SENTINEL, "\x00");
    }
    out
}

/// Kill switch shared with the legacy pre-pass, so `TOKENADE_DOM_DEDUP=0`
/// disables de-duplication on both paths.
pub fn dedup_enabled() -> bool {
    !std::env::var("TOKENADE_DOM_DEDUP").is_ok_and(|v| v == "0" || v == "off")
}

/// Trim residue above the first heading (defuddle's findContentStart): byline,
/// share, "Published/Updated …" lines that sit between the page top and the
/// article's title. Conservative — only trims when EVERY non-empty line before
/// the first heading (found within the first dozen lines) is clearly residue, so
/// a real lead paragraph or intro is never dropped.
fn trim_leading_residue(md: &str) -> String {
    let lines: Vec<&str> = md.lines().collect();
    let mut first_heading = None;
    let mut seen = 0;
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim();
        if t.is_empty() {
            continue;
        }
        if t.starts_with('#') {
            first_heading = Some(i);
            break;
        }
        seen += 1;
        if seen > 12 {
            break;
        }
    }
    let hi = match first_heading {
        Some(i) if i > 0 => i,
        _ => return md.to_string(),
    };
    for l in &lines[..hi] {
        let t = l.trim();
        if t.is_empty() {
            continue;
        }
        let low = t.to_lowercase();
        let words = t.split_whitespace().count();
        let residue = low.starts_with("by ")
            || low.starts_with("published")
            || low.starts_with("updated")
            || low.contains("share this")
            || (t.starts_with('[') && words <= 3);
        if !residue {
            return md.to_string();
        }
    }
    lines[hi..].join("\n")
}

/// What the page shows to nobody — see `web_tree/hidden.rs`.
mod hidden;
/// The scenery, and the root that carries the substance — see
/// `web_tree/prune.rs`.
mod prune;
pub use hidden::*;
use prune::*;

/// Six more subjects. The trunk keeps the shared accessors (`el_name`,
/// `attr`, `text_len`, `collapse_ws`) and the walk's entry point.
mod blocks;
mod embeds;
mod frames;
mod inline;
mod post_passes;
mod tables;
use blocks::*;
pub use embeds::*;
pub use frames::*;
pub use inline::*;
pub use post_passes::*;
pub(crate) use tables::*;

/// One walk of the subtree, with boilerplate-skip on or off.
fn walk(root: NodeRef<Node>, skip_boilerplate: bool) -> String {
    SKIP_BOILERPLATE.with(|c| c.set(skip_boilerplate));
    // One extra allocation-free descendant pass, so `is_boilerplate` can ask what
    // FRACTION of the walked region a candidate holds. Cheap next to the walk
    // itself, and it is the only way to tell a chrome block from a layout
    // wrapper that merely spells a keyword — see `spans_most_of_the_page`.
    ROOT_TEXT_LEN.with(|c| c.set(text_len(root)));
    let mut out = String::new();
    block(root, &mut out, 0);
    SKIP_BOILERPLATE.with(|c| c.set(true));
    ROOT_TEXT_LEN.with(|c| c.set(0));
    out
}

/// The element's tag name; `None` on text, comments and the rest. Shared with
/// `tree_dedup`, which used to carry an exact copy.
pub(crate) fn el_name<'a>(n: NodeRef<'a, Node>) -> Option<&'a str> {
    match n.value() {
        Node::Element(e) => Some(e.name()),
        _ => None,
    }
}

fn attr<'a>(n: NodeRef<'a, Node>, key: &str) -> Option<&'a str> {
    match n.value() {
        Node::Element(e) => e.attr(key),
        _ => None,
    }
}

/// True if the subtree contains a block-level element — used to decide whether
/// an anchor is an inline link or a card wrapping block content.
fn has_block_descendant(n: NodeRef<Node>) -> bool {
    // Recurses independently of `block`, so it needs its own bound: it is called
    // from the `<a>` arm and would otherwise blow the stack on a deep anchor
    // before `block` ever got the chance to refuse.
    let _guard = match DepthGuard::enter() {
        Some(g) => g,
        None => return false,
    };
    for c in n.children() {
        if let Some(name) = el_name(c) {
            if matches!(
                name,
                "h1" | "h2"
                    | "h3"
                    | "h4"
                    | "h5"
                    | "h6"
                    | "p"
                    | "div"
                    | "section"
                    | "article"
                    | "ul"
                    | "ol"
                    | "li"
                    | "table"
                    | "blockquote"
                    | "pre"
                    | "figure"
                    | "dl"
                    | "header"
                    | "footer"
                    | "main"
            ) {
                return true;
            }
            if has_block_descendant(c) {
                return true;
            }
        }
    }
    false
}

/// Collapse HTML whitespace to single spaces while PRESERVING a single boundary
/// space, so "foo <b>bar</b>" stays "foo bar" and does not glue into "foobar".
fn collapse_ws(s: &str) -> String {
    let leading = s.starts_with(|c: char| c.is_whitespace());
    let trailing = s.ends_with(|c: char| c.is_whitespace());
    let core = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if core.is_empty() {
        return if s.is_empty() {
            String::new()
        } else {
            " ".into()
        };
    }
    let mut out = String::new();
    if leading {
        out.push(' ');
    }
    out.push_str(&core);
    if trailing {
        out.push(' ');
    }
    out
}

// A `<p>` whose text opens with `#` used to reach `markdown::outline` as a
// heading and re-parent every section after it. See `escape_atx_openers`.

// A block element rendered inside an inline context — a table cell, a heading,
// a list item, a link label — still ends a line in a browser. Flattening it
// without a separator FABRICATES a word the page never contained.
//
// MEASURED 2026-09-07 on the 39 real pages of `tests/fixtures/pages`:
// **7 pages, 191 lines** carried a weld, including
// `### EnvironmentSlowdown in Arctic ice melt` (guardian, a section label glued
// to its title), `created.If the Deployment` (kubernetes.io) and `l'actuIl y a
// 0 sec` (franceinfo, a weld in the middle of a word). The fix costs
// **+212 bytes out of 1,076,604**, i.e. 0.02%.
#[cfg(test)]
include!("web_tree_tests.rs");
