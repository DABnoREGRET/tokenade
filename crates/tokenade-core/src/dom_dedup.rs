//! Structural de-duplication of repeated sibling subtrees.
//!
//! Marketing pages duplicate content in the DOM for *presentational* reasons,
//! and the duplicates reach the agent as content:
//!
//! - **CSS marquees / tickers** repeat their track N times so the scroll loops
//!   seamlessly. healthline.com's homepage carries
//!   `Medically reviewed content · 130 medical reviewers…` **8 times**.
//! - **Responsive / themed image sets** ship one `<img>` per breakpoint or
//!   colour scheme, all with the same `alt`. vercel.com's homepage emits
//!   `[image: Build agents on infrastructure that thinks like them]` **4
//!   times** per hero, because `alt` is the only thing a reader can render.
//!
//! This runs as a pre-pass on the HTML, before any markdown conversion,
//! because the redundancy is a fact about the **container** — adjacent
//! siblings whose rendered contribution is byte-identical — not a judgement
//! about the text. Measuring the emitted text instead ("is this line
//! valuable?") is the failure mode this codebase has already disproven.
//!
//! Nothing informational is lost: the removed runs are byte-identical to the
//! copy that is kept, so the output carries the same content, once. The
//! multiplicity itself is presentational.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use tree_sitter::{Node, Parser};

/// The rules below — thresholds, protected tags, `Sig`, and the run picker —
/// are shared with [`crate::tree_dedup`], which applies them to the parsed
/// `scraper` tree instead of to tree-sitter's. Two live de-duplicators (regex
/// path and tree path) that each kept their own copy would drift, and the
/// corpus rationale recorded here would only describe one of them.
///
/// Minimum run length before a repeat is treated as presentational.
///
/// Two adjacent identical siblings happen in legitimate content (a pair of
/// equal cells, an intentional repetition); three or more is a generated
/// loop. Measured on the corpora: at 3 this never fired on genuine prose.
pub(crate) const MIN_RUN: usize = 3;

/// Longest sibling cycle considered.
///
/// The repeating unit is not always ONE element: healthline.com's marquee
/// track is a group of four cards (`Medically reviewed content`,
/// `130 medical reviewers`, `19 years of experience`, `50 million monthly
/// readers`) repeated **8 times** — 32 sibling divs with a period of 4. A
/// period-1-only scan sees no two adjacent siblings alike and does nothing.
pub(crate) const MAX_PERIOD: usize = 12;

/// Minimum signature length. Short repeats ("·", "—", "OK") are decorative
/// separators or terse cells where collapsing buys nothing and risks meaning.
pub(crate) const MIN_SIGNATURE_CHARS: usize = 12;

/// Parsing cost guard. Above this the page is pathological and the pre-pass
/// is skipped entirely rather than spending the budget.
const MAX_HTML_BYTES: usize = 4 * 1024 * 1024;

/// Element types that are NEVER de-duplicated.
///
/// Table rows and cells are positional: a `<tr>` whose text equals its
/// neighbour's is aligned against column headers, and dropping it silently
/// deletes a row of DATA. `pre`/`code` repeat lines legitimately. `option`
/// values are a set the user picks from.
pub(crate) const NEVER_DEDUP: &[&str] = &[
    "tr", "td", "th", "table", "thead", "tbody", "tfoot", "col", "colgroup", "pre", "code",
    "option", "select", "textarea", "script", "style", "svg", "path",
];

/// Remove runs of `MIN_RUN`+ adjacent sibling elements that render identically.
///
/// Returns `html` unchanged when the parser is unavailable, the page is
/// oversized, or nothing repeats — the caller can treat this as a no-op pass.
pub fn dedup_repeated_siblings(html: &str) -> String {
    // Kill switch. Also the only honest way to A/B the pass: comparing two
    // LIVE fetches attributes the site's own A/B variants to the code —
    // airtable.com served a different homepage between two runs and the
    // diff read as a 99 % regression that did not exist.
    if std::env::var("TOKENADE_DOM_DEDUP").is_ok_and(|v| v == "0" || v == "off") {
        return html.to_string();
    }
    if html.len() > MAX_HTML_BYTES {
        return html.to_string();
    }
    let mut parser = Parser::new();
    if parser
        .set_language(&tree_sitter_html::LANGUAGE.into())
        .is_err()
    {
        return html.to_string();
    }
    let Some(tree) = parser.parse(html, None) else {
        return html.to_string();
    };

    let mut cuts: Vec<(usize, usize)> = Vec::new();
    // Signatures are memoised across the whole walk. Without this each node's
    // text is re-walked once per ANCESTOR level — O(nodes x depth), which on
    // a deep marketing page cost +39 % wall clock for a 2.7 % token win.
    let mut sigs_by_id: HashMap<usize, Sig> = HashMap::new();
    build_signatures(tree.root_node(), html, &mut sigs_by_id);
    collect_cuts(tree.root_node(), html, &mut cuts, &sigs_by_id);
    if cuts.is_empty() {
        return html.to_string();
    }

    // Apply back-to-front so earlier byte offsets stay valid. Ranges are
    // produced by a pre-order walk over disjoint sibling runs, but a parent
    // run can enclose a child run; sorting + skipping enclosed ranges keeps
    // the splice total-order safe.
    cuts.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(cuts.len());
    for (start, end) in cuts {
        match merged.last_mut() {
            Some((_, prev_end)) if start < *prev_end => {
                // Enclosed or overlapping: extend rather than splice twice.
                if end > *prev_end {
                    *prev_end = end;
                }
            }
            _ => merged.push((start, end)),
        }
    }
    if std::env::var("TOKENADE_DOM_DEDUP_DEBUG").is_ok() {
        for (start, end) in &merged {
            let preview: String = html[*start..*end].chars().take(160).collect();
            eprintln!(
                "[dom_dedup] cut {}..{} ({} B): {}",
                start,
                end,
                end - start,
                preview
            );
        }
    }
    let mut out = String::with_capacity(html.len());
    let mut cursor = 0;
    for (start, end) in merged {
        if start >= cursor && end <= html.len() {
            out.push_str(&html[cursor..start]);
            cursor = end;
        }
    }
    out.push_str(&html[cursor..]);
    out
}

/// Pick the presentational duplicate runs in ONE sibling list.
///
/// Shared by both de-duplicators: `dom_dedup` feeds it tree-sitter siblings and
/// splices byte ranges, `tree_dedup` feeds it `scraper` siblings and skips node
/// ids. Returns `(start, period, cycles)` per run; the caller keeps the first
/// cycle and drops `start + period .. start + period * cycles`.
pub(crate) fn pick_runs(sigs: &[Sig], protected: &[bool]) -> Vec<(usize, usize, usize)> {
    let n = sigs.len();
    let mut runs = Vec::new();
    let mut i = 0;
    while i < n {
        let mut best: Option<(usize, usize)> = None;
        // Smallest period first, so a run of identical siblings is reported as
        // period 1 rather than as some multiple of it.
        for period in 1..=MAX_PERIOD.min((n - i) / 2) {
            if (i..i + period).any(|x| protected[x]) {
                break;
            }
            let cycle_len: usize = sigs[i..i + period].iter().map(|s| s.chars).sum();
            if cycle_len < MIN_SIGNATURE_CHARS {
                continue;
            }
            let mut cycles = 1;
            while i + (cycles + 1) * period <= n
                && !(i + cycles * period..i + (cycles + 1) * period).any(|x| protected[x])
                && (0..period).all(|m| sigs[i + m] == sigs[i + cycles * period + m])
            {
                cycles += 1;
            }
            // A run of images whose whole contribution is one `alt` line is
            // redundant at TWO copies: no agent is better off reading
            // `[image: X]` twice in a row. Text needs a third copy before we
            // call it generated rather than intentional.
            //
            // This MUST hold for every element in the run, not just the first
            // cycle — the ones being DELETED are the ones that matter. Card
            // markup routinely sets the image `alt` to the card's own title, so
            // the pair matched, the first element was image-only, and the
            // relaxation deleted the HEADING — 424 words of real content off
            // resend.com/blog, the reverse of the intent.
            let needed =
                if cycles < MIN_RUN && (i..i + cycles * period).all(|x| sigs[x].is_image_only()) {
                    2
                } else {
                    MIN_RUN
                };
            if cycles >= needed {
                best = Some((period, cycles));
                break;
            }
        }
        if let Some((period, cycles)) = best {
            runs.push((i, period, cycles));
            i += cycles * period;
            continue;
        }
        i += 1;
    }
    runs
}

/// Walk the tree, recording the byte ranges of duplicate siblings to drop.
fn collect_cuts(
    node: Node,
    src: &str,
    cuts: &mut Vec<(usize, usize)>,
    sigs_by_id: &HashMap<usize, Sig>,
) {
    let mut walker = node.walk();
    let children: Vec<Node> = node.children(&mut walker).collect();

    // Sibling elements only — text nodes between them are whitespace or the
    // parent's own prose, and neither participates in a presentational loop.
    let elements: Vec<Node> = children
        .iter()
        .copied()
        .filter(|c| c.kind() == "element")
        .collect();

    if elements.is_empty() {
        for child in children {
            collect_cuts(child, src, cuts, sigs_by_id);
        }
        return;
    }

    // Signatures are computed once per sibling: the naive version recomputed
    // one per comparison, which is quadratic in the subtree size on the very
    // pages that repeat the most.
    let sigs: Vec<Sig> = elements
        .iter()
        .map(|e| sigs_by_id.get(&e.id()).copied().unwrap_or_default())
        .collect();
    let protected: Vec<bool> = elements.iter().map(|e| is_protected(*e, src)).collect();

    for (i, period, cycles) in pick_runs(&sigs, &protected) {
        // Keep one cycle, drop the rest.
        let keep_end = elements[i + period - 1].end_byte();
        let run_end = elements[i + cycles * period - 1].end_byte();
        cuts.push((keep_end, run_end));
    }

    for child in children {
        collect_cuts(child, src, cuts, sigs_by_id);
    }
}

/// True when this element (or its tag) must never be collapsed.
fn is_protected(el: Node, src: &str) -> bool {
    tag_name(el, src).is_some_and(|t| NEVER_DEDUP.contains(&t.as_str()))
}

fn tag_name(el: Node, src: &str) -> Option<String> {
    let mut walker = el.walk();
    let start = el.children(&mut walker).find(|c| {
        c.kind() == "start_tag" || c.kind() == "self_closing_tag" || c.kind() == "script_start_tag"
    })?;
    let mut w2 = start.walk();
    let name = start.children(&mut w2).find(|c| c.kind() == "tag_name")?;
    Some(src[name.byte_range()].to_ascii_lowercase())
}

/// What this subtree contributes to the reader's output, normalised.
///
/// Text nodes plus `alt` attributes — `alt` is included because it is the
/// ONLY thing an image contributes, which is exactly why four `<img>` tags
/// differing solely in `src`/`width` render as four identical lines.
/// `src`, `class`, `width` etc. are deliberately excluded: two images that
/// differ only by breakpoint are the same image to an agent.
/// A subtree's rendered contribution, reduced to a 64-bit digest plus its
/// length in characters.
///
/// Materialising the text itself is what made the pass expensive: the root's
/// signature IS the whole page, and building one String per element at every
/// depth costs O(bytes x depth). Digests compare just as well and the length
/// is all the size threshold needs.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct Sig {
    pub(crate) digest: u64,
    /// Characters this subtree contributes, text and `alt` together.
    pub(crate) chars: usize,
    /// Characters contributed by TEXT alone. Zero with `chars > 0` means the
    /// subtree renders as `[image: …]` and nothing else.
    pub(crate) text_chars: usize,
}

impl Sig {
    /// True when the whole contribution is image `alt` text.
    pub(crate) fn is_image_only(&self) -> bool {
        self.text_chars == 0 && self.chars > 0
    }
}

/// Compute every node's signature in ONE post-order pass.
///
/// The obvious version — ask each element for its signature when its parent
/// examines it — re-walks every subtree once per ancestor level. That is
/// O(bytes x depth), and it cost **78 ms per page** on a release build,
/// tripling the whole reader (33 ms -> 109 ms). Folding each node's digest
/// from its children's digests makes it linear: every node is visited once.
///
/// A parent's digest therefore commits to its children's STRUCTURE as well as
/// their text. That is stricter than comparing flattened text, and strictness
/// is the safe direction here: two subtrees that collide must be identically
/// shaped, not merely similar-reading.
fn build_signatures(node: Node, src: &str, out: &mut HashMap<usize, Sig>) -> Sig {
    match node.kind() {
        "text" => {
            let raw = &src[node.byte_range()];
            let mut hasher = DefaultHasher::new();
            let mut chars = 0usize;
            for word in raw.split_whitespace() {
                word.hash(&mut hasher);
                chars += word.chars().count();
            }
            return Sig {
                digest: if chars == 0 { 0 } else { hasher.finish() },
                chars,
                text_chars: chars,
            };
        }
        // Script and style bodies are not content and must not make two
        // otherwise-identical cards look different.
        "script_element" | "style_element" => return Sig::default(),
        "attribute" => {
            let Some(alt) = alt_value(node, src) else {
                return Sig::default();
            };
            let mut hasher = DefaultHasher::new();
            let mut chars = 0usize;
            for word in alt.split_whitespace() {
                word.hash(&mut hasher);
                chars += word.chars().count();
            }
            return Sig {
                digest: if chars == 0 { 0 } else { hasher.finish() },
                chars,
                text_chars: 0,
            };
        }
        _ => {}
    }

    let mut hasher = DefaultHasher::new();
    let mut chars = 0usize;
    let mut text_chars = 0usize;
    let mut walker = node.walk();
    for child in node.children(&mut walker) {
        let sig = build_signatures(child, src, out);
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
    if node.kind() == "element" {
        out.insert(node.id(), sig);
    }
    sig
}

fn alt_value(attr: Node, src: &str) -> Option<String> {
    let mut walker = attr.walk();
    let children: Vec<Node> = attr.children(&mut walker).collect();
    let name = children.iter().find(|c| c.kind() == "attribute_name")?;
    if !src[name.byte_range()].eq_ignore_ascii_case("alt") {
        return None;
    }
    let value = children
        .iter()
        .find(|c| c.kind() == "quoted_attribute_value" || c.kind() == "attribute_value")?;
    let raw = &src[value.byte_range()];
    Some(raw.trim_matches(['"', '\'']).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapses_a_marquee_track() {
        // healthline.com repeats its credibility ticker 8× so the CSS scroll
        // loops without a seam.
        let unit = "<div><span>Medically reviewed content</span><span>130 medical reviewers in network</span></div>";
        let html = format!("<section>{}</section>", unit.repeat(8));
        let out = dedup_repeated_siblings(&html);
        assert_eq!(out.matches("130 medical reviewers").count(), 1);
        assert!(out.contains("Medically reviewed content"));
    }

    #[test]
    fn collapses_responsive_image_sets_sharing_one_alt() {
        // vercel.com ships one <img> per breakpoint; `alt` is all a reader
        // can render, so four tags become four identical lines.
        let html = r#"<div>
            <img alt="Build agents on infrastructure that thinks like them" src="a.png" width="1284">
            <img alt="Build agents on infrastructure that thinks like them" src="b.png" width="1284">
            <img alt="Build agents on infrastructure that thinks like them" src="c.png" width="2721">
            <img alt="Build agents on infrastructure that thinks like them" src="d.png" width="2721">
        </div>"#;
        let out = dedup_repeated_siblings(html);
        assert_eq!(out.matches("Build agents on infrastructure").count(), 1);
    }

    #[test]
    fn collapses_a_multi_card_marquee_cycle() {
        // healthline.com's real shape: the repeating unit is a GROUP of four
        // distinct cards, repeated 8×. No two adjacent siblings are alike, so
        // a period-1-only scan sees nothing to do.
        let cards = "<div><span>Medically reviewed content</span></div>\
                     <div><span>130 medical reviewers in network</span></div>\
                     <div><span>19 years of experience</span></div>\
                     <div><span>50 million monthly readers</span></div>";
        let html = format!("<section>{}</section>", cards.repeat(8));
        let out = dedup_repeated_siblings(&html);
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
        // Two passes over a group of distinct items is a layout (two columns,
        // a tab strip mirroring its panels), not a generated loop.
        let cards = "<div><span>Durable Orchestration here</span></div>\
                     <div><span>Sandboxed Environments here</span></div>";
        let html = format!("<section>{}</section>", cards.repeat(2));
        let out = dedup_repeated_siblings(&html);
        assert_eq!(out.matches("Durable Orchestration here").count(), 2);
    }

    #[test]
    fn keeps_identical_table_rows() {
        // The row is DATA: it is aligned against column headers, and its
        // text matching a neighbour's says nothing about whether it matters.
        let row = "<tr><td>Included in every plan</td><td>Included in every plan</td></tr>";
        let html = format!("<table><tbody>{}</tbody></table>", row.repeat(4));
        let out = dedup_repeated_siblings(&html);
        assert_eq!(out.matches("Included in every plan").count(), 8);
    }

    #[test]
    fn keeps_repeated_lines_inside_code() {
        let line = "<code>console.log(\"retrying now\");</code>";
        let html = format!("<pre>{}</pre>", line.repeat(5));
        let out = dedup_repeated_siblings(&html);
        assert_eq!(out.matches("retrying now").count(), 5);
    }

    #[test]
    fn keeps_a_pair() {
        // Two adjacent equal siblings occur in genuine content; only a run of
        // three or more is a generated loop.
        let html = "<div><p>The same sentence appears twice here.</p><p>The same sentence appears twice here.</p></div>";
        let out = dedup_repeated_siblings(html);
        assert_eq!(out.matches("appears twice here").count(), 2);
    }

    #[test]
    fn keeps_distinct_siblings() {
        let html = "<ul><li>Durable Orchestration</li><li>Sandboxed Environments</li><li>AI Model Gateway</li></ul>";
        let out = dedup_repeated_siblings(html);
        assert!(out.contains("Durable Orchestration"));
        assert!(out.contains("Sandboxed Environments"));
        assert!(out.contains("AI Model Gateway"));
    }

    #[test]
    fn short_repeats_are_left_alone() {
        // Decorative separators carry no content either way; collapsing them
        // buys nothing and the threshold keeps the pass conservative.
        let html = "<div><span>·</span><span>·</span><span>·</span><span>·</span></div>";
        let out = dedup_repeated_siblings(html);
        assert_eq!(out.matches('·').count(), 4);
    }

    #[test]
    fn malformed_html_is_returned_not_mangled() {
        // tree-sitter is error-tolerant by design; assert we do not panic or
        // truncate when the input is broken.
        for bad in ["<div><p>unclosed", "<<>>", "", "plain text, no tags at all"] {
            let out = dedup_repeated_siblings(bad);
            assert!(out.contains(bad.trim()) || out == bad);
        }
    }
}
