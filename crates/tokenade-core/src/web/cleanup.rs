//! Pure text passes over the markdown a page converted to.
//!
//! Every function here is `&str -> String` with no I/O, no config and no
//! shared state: the noise a real HTML page leaves behind (permalink anchors,
//! nav-only link runs, list items that render empty, headings smuggled inside
//! links) removed one named pass at a time. They were interleaved with the
//! converter in `web.rs`, which made a 6600-line file out of two jobs.
//!
//! `use super::*` reaches the converter's own helpers; nothing here reaches
//! back the other way.

use super::*;

/// `Title\n===\n` → `# Title`, `Title\n---\n` → `## Title` — only when the
/// underline is adjacent (no blank line between) and at least 3 characters
/// of a single repeated `=`/`-`, so an existing `---` horizontal rule after
/// a blank line (this file's own thin-result/challenge banners use exactly
/// that) is never mistaken for a setext underline.
pub(super) fn setext_to_atx(s: &str) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim();
        if !trimmed.is_empty() && i + 1 < lines.len() {
            let next = lines[i + 1].trim();
            if next.len() >= 3 && next.chars().all(|c| c == '=') {
                out.push(format!("# {trimmed}"));
                i += 2;
                continue;
            }
            if next.len() >= 3 && next.chars().all(|c| c == '-') {
                out.push(format!("## {trimmed}"));
                i += 2;
                continue;
            }
        }
        out.push(lines[i].to_string());
        i += 1;
    }
    out.join("\n")
}

/// Drop runs of ≥`MIN_RUN` consecutive list items that are links and nothing
/// else — subreddit sidebars, footer sitemaps, mega-nav dumps. Per-block
/// link-density (`prune_link_dense_blocks`) misses these because each item
/// is its own line inside one huge block.
/// Drop permalink anchors — `<a href="#heading">¶</a>` and friends.
///
/// Every documentation generator hangs a "link to this heading" anchor off its
/// titles, and its glyph lands inside the heading text: `# Glossary¶`,
/// `## Documentation¶`. Measured on the doc corpus: 72 leaked into
/// pkg.go.dev's output, 16 into docs.python.org's — Sphinx, godoc, Docusaurus,
/// MkDocs and rustdoc all do it, so this is not one site's quirk.
///
/// The rule is structural, not a class-name list (which differ per generator:
/// `headerlink`, `UnitDoc-idLink`, `hash-link`): an anchor pointing at a
/// FRAGMENT whose entire visible text is one punctuation glyph carries no
/// information. A one-character LABEL that is alphanumeric is left alone.
/// Remove HTML comments before anything else looks at the markup.
///
/// Nothing stripped them: `strip_all_tags` treats `<!-- …` as a tag and ends it
/// at the FIRST `>`, so a commented-out block leaks everything after that `>`
/// as text and drops a bare ` -->` line. Measured on a synthetic product page,
/// a disabled campaign came back as live content — an `## OLD CAMPAIGN DO NOT
/// SHIP` heading and `Buy now for $9.99, this offer expired in 2019` — and
/// html.duckduckgo.com opened its output with `-->` from a commented-out
/// doctype.
///
/// Serving markup the site DISABLED is a correctness bug, not noise: expired
/// prices, unpublished drafts and dead campaigns are exactly what sits in
/// comments.
///
/// An unterminated comment takes everything to EOF, matching
/// `strip_unclosed_block`'s reasoning for `<script>`: a browser would not
/// render it either.
pub(super) fn strip_html_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(open) = rest.find("<!--") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 4..];
        match after.find("-->") {
            Some(close) => rest = &after[close + 3..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

pub(super) fn strip_permalink_anchors(s: &str) -> String {
    // The closure below edits ONLY when the anchor's href is a fragment, so a
    // document with no `href="#` at all cannot produce a single replacement —
    // skipping the scan there is byte-identical, and the scan is one of the
    // most expensive regexes in the chain (quote-aware attribute alternation
    // over the whole document).
    if !s.contains("href=\"#") && !s.contains("href='#") {
        return strip_classed_nav_anchors(s);
    }
    let re_a = re(r#"(?is)<a\b((?:[^>"']|"[^"]*"|'[^']*')*)>\s*([^<>\s]{1,2})\s*</a>"#);
    let out = re_a
        .replace_all(s, |c: &regex::Captures| {
            let attrs = c.get(1).map(|m| m.as_str()).unwrap_or("");
            let label = c.get(2).map(|m| m.as_str()).unwrap_or("");
            let fragment_link = attrs.contains("href=\"#") || attrs.contains("href='#");
            let glyph_only = !label.is_empty()
                && label
                    .chars()
                    .all(|ch| !ch.is_alphanumeric() && !ch.is_whitespace());
            if fragment_link && glyph_only {
                String::new()
            } else {
                c.get(0).map(|m| m.as_str().to_string()).unwrap_or_default()
            }
        })
        .into_owned();
    strip_classed_nav_anchors(&out)
}

/// ASCII-case-insensitive substring test that allocates nothing.
///
/// `needle` must be ASCII-lowercase. The first-byte filter keeps this linear in
/// practice on the miss path, which is the whole point of the prefilters that
/// call it: building a lowercase copy of a 300 KB page to answer "no" would
/// give back most of what the prefilter saves.
pub(super) fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    let (h, n) = (haystack.as_bytes(), needle.as_bytes());
    if n.is_empty() || h.len() < n.len() {
        return n.is_empty();
    }
    let first = n[0];
    h.windows(n.len())
        .any(|w| w[0].to_ascii_lowercase() == first && w.eq_ignore_ascii_case(n))
}

/// Documentation generators put a WORDED self-link in every heading, and unlike
/// the `¶`/`#` glyphs above it survives as text. man7.org renders every section
/// as `<h2>…NAME <a href="#top_of_page"><span class="top-link">top</span></a>`,
/// so `tokenade web man7.org/…/openat.2.html` outlined thirteen sections named
/// `NAME top`, `SYNOPSIS top`, `ERRORS top` — the word appended to every
/// heading in the document.
///
/// Keyed on the CLASS, not on the visible word: `headerlink` (Sphinx, MkDocs),
/// `hash-link` (Docusaurus), `top-link` (man7), `permalink` (GitHub). A word
/// list would have to guess at "top", "link", "edit", "¶" in every UI language;
/// the class names are a generator convention and do not translate.
///
/// Two guards, both earned. It runs only INSIDE `<h1>`–`<h6>`, and it refuses
/// any edit that would leave the heading with no text at all. MDN writes
/// `<h2 class="heading"><a class="heading-anchor" href="#…">Fetch
/// インターフェイス</a></h2>` — the anchor IS the heading — and the first cut of
/// this function (which matched a bare `anchor` class, document-wide) emptied
/// every heading on every MDN page in the corpus: eight `##` with nothing after
/// them. A permalink widget is by definition something a heading has BESIDES
/// its text, so "removing it must not empty the heading" is the invariant, not
/// a special case.
pub(super) fn strip_classed_nav_anchors(s: &str) -> String {
    // Same reasoning as above: an edit requires one of the four generator class
    // names to be present somewhere, so their joint absence makes the whole
    // heading scan a no-op. Cheap substring search vs a document-wide
    // `<h1-6>…</h1-6>` regex plus a tag-strip per heading.
    //
    // Case-INSENSITIVE, because the test it stands in for lowercases the markup
    // first (`strip_nav_anchors_in_heading`): a `class="HeaderLink"` must not
    // slip past the prefilter and silently change the page's output. Done
    // in place rather than by lowercasing the document — the MISS is the common
    // case this prefilter exists for, so it must not allocate a copy of the page.
    if !NAV_CLASSES.iter().any(|c| contains_ignore_ascii_case(s, c)) {
        return s.to_string();
    }
    let heading = re(r"(?is)<h([1-6])\b[^>]*>(.*?)</h[1-6]>");
    heading
        .replace_all(s, |c: &regex::Captures| {
            let whole = c.get(0).map(|m| m.as_str()).unwrap_or("");
            let inner = c.get(2).map(|m| m.as_str()).unwrap_or("");
            let cut = strip_nav_anchors_in_heading(inner);
            let survives = re(r"(?is)<[^>]*>")
                .replace_all(&cut, "")
                .chars()
                .any(|ch| ch.is_alphanumeric());
            if cut == inner || !survives {
                return whole.to_string();
            }
            whole.replacen(inner, &cut, 1)
        })
        .into_owned()
}

pub(super) fn strip_nav_anchors_in_heading(s: &str) -> String {
    // The inner group accepts text and any tag whose NAME is not `a`, so the
    // match cannot run across a nested or following anchor. The `regex` crate
    // has no look-around, hence the explicit "tag name is not a" alternation
    // rather than `(?!</?a\b)`.
    let re_a = re(
        r#"(?is)<a\b((?:[^>"']|"[^"]*"|'[^']*')*)>((?:[^<]|</?(?:[^aA/\s>][a-z0-9]*|[aA][a-z0-9]+)[^>]*>){0,200}?)</a>"#,
    );
    re_a.replace_all(s, |c: &regex::Captures| {
        let attrs = c.get(1).map(|m| m.as_str()).unwrap_or("");
        let inner = c.get(2).map(|m| m.as_str()).unwrap_or("");
        let keep = || c.get(0).map(|m| m.as_str().to_string()).unwrap_or_default();
        if !(attrs.contains("href=\"#") || attrs.contains("href='#")) {
            return keep();
        }
        let markup = format!("{attrs} {inner}").to_ascii_lowercase();
        if !NAV_CLASSES.iter().any(|c| markup.contains(c)) {
            return keep();
        }
        // The label an agent would read, once the wrapper tags are gone.
        let text = re(r"(?is)<[^>]*>").replace_all(inner, "");
        let text = text.replace("&nbsp;", " ");
        if text.trim().chars().count() > 20 {
            return keep();
        }
        String::new()
    })
    .into_owned()
}

/// Drop list items that have nothing in them.
///
/// A `<li>` holding only an icon, a tracking pixel or an empty anchor converts
/// to a bare `-` on a line of its own. Measured across the 147-page corpus:
/// **789 such lines out of 17 683**, one line in twenty-two of everything we
/// emit. FastAPI's tutorial carried 82, the Stripe API reference 68, and the
/// BBC's World front 27 — a column of dashes where the page has a navigation
/// rail made of images.
///
/// They cost tokens and say nothing: an empty list item conveys no more than
/// its absence. Blank lines they leave behind are folded by
/// `collapse_whitespace` downstream.
pub(super) fn drop_empty_list_items(md: &str) -> String {
    let empty = re(r"^\s*[-*+]\s*$");
    md.lines()
        .filter(|l| !empty.is_match(l))
        .collect::<Vec<_>>()
        .join("\n")
}

/// essence's link-text dedup (MIT — see module doc): `[Apple Apple](url)` →
/// `[Apple](url)`. Only collapses a label that is exactly two identical
/// halves split on whitespace — not a substring/prefix match, so "the the"
/// (a real typo some page actually contains) is left alone unless it is the
/// WHOLE label.
pub(super) fn dedup_repeated_link_text(md: &str) -> String {
    let r = re(r"\[([^\]]+)\]\(([^)]+)\)");
    r.replace_all(md, |c: &regex::Captures| {
        let text = c.get(1).unwrap().as_str();
        let url = c.get(2).unwrap().as_str();
        let words: Vec<&str> = text.split_whitespace().collect();
        if words.len() >= 2 && words.len().is_multiple_of(2) {
            let (a, b) = words.split_at(words.len() / 2);
            if a == b {
                return format!("[{}]({url})", a.join(" "));
            }
        }
        c.get(0).unwrap().as_str().to_string()
    })
    .into_owned()
}

/// essence's repeated-list-item collapse (MIT — see module doc): once a
/// `- text` bullet has appeared identically twice IN A ROW, drop the 3rd and
/// later repeats — a skeleton/placeholder row a JS list widget repeated
/// rather than three separately meaningful bullets that happen to read the
/// same.
pub(super) fn collapse_repeated_list_items(md: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut last: Option<&str> = None;
    let mut run = 0usize;
    for line in md.lines() {
        if let Some(item) = line.trim().strip_prefix("- ") {
            if Some(item) == last {
                run += 1;
                if run >= 3 {
                    continue;
                }
            } else {
                last = Some(item);
                run = 1;
            }
        } else {
            last = None;
            run = 0;
        }
        out.push(line);
    }
    out.join("\n")
}

/// `[## Headline](/url)` → `## [Headline](/url)`.
///
/// News and marketing sites make the whole card a link and put the `<h2>`/`<h3>`
/// INSIDE it, so heading conversion runs first and link conversion then wraps
/// the result. What comes out is a heading marker inside link text, which is
/// not a heading in any markdown parser — including our own outline, which is
/// how a BBC article ends up with no visible structure while carrying 44 of
/// them.
///
/// Measured before writing it: **341 occurrences across 22 of the 147 corpus
/// pages** (bbc, guardian, vogue, aljazeera, yahoo finance, sportingnews,
/// docomo, ikea…). Not a long-tail curiosity.
///
/// Only fires when the link OPENS the line — an inline `see [## x](y)` mid
/// sentence is not a heading and is left alone.
pub(super) fn hoist_headings_out_of_links(md: &str) -> String {
    let re_h = re(r"^(\s*)\[(#{1,6})\s+([^\]]*)\]\(([^)]*)\)\s*$");
    // Mid-line the marker cannot be hoisted — yahoo finance builds a video row
    // as `- [02:49](url) [### Title](url)`, and moving the heading would break
    // the row apart. But `###` inside a link LABEL says nothing at all: it is
    // not a heading there and never will be. Drop the marker and keep the
    // text. Requires the space after the hashes, so `[#hashtag](…)` is safe.
    let re_inline = re(r"\[#{1,6}\s+");
    md.lines()
        .map(|line| match re_h.captures(line) {
            Some(c) => format!("{} [{}]({})", &c[2], c[3].trim(), &c[4]),
            None => re_inline.replace_all(line, "[").into_owned(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Break a line of BUTTED-TOGETHER markdown links into one per line.
///
/// Listing pages build each card from inline `<a>` elements, so nothing in the
/// regex pipeline emits a block break between them and the whole section
/// arrives as one line. standard.co.uk's business section came back as a
/// single 10 357-character line reading
/// `[Business News](…)[Oil prices surpass 90 dollars…](…)[Business News](…)…`
/// — every headline present, none of them readable, and useless as a RAG
/// chunk or as an outline preview.
///
/// Fires only on `)(` adjacency — a link closing and the next opening with
/// NOTHING between them, three or more times. Links separated by prose are
/// inline references in a sentence and are left alone.
pub(super) fn split_butted_link_runs(md: &str) -> String {
    let boundary = re(r"\)\[");
    md.lines()
        .map(|line| {
            if boundary.find_iter(line).count() < 3 {
                return line.to_string();
            }
            boundary.replace_all(line, ")\n[").into_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Do the link LABELS in this text read like headlines rather than nav items?
///
/// Both pruners need it, and for the same reason: link density says a block is
/// mostly links, which is equally true of a sidebar and of a newspaper front
/// page. Length of the label is what separates them.
pub(super) fn link_labels_read_like_content(s: &str) -> bool {
    let label = re(r"\[([^\]]*)\]\([^)]*\)");
    let mut w: Vec<usize> = label
        .captures_iter(s)
        .map(|c| c[1].split_whitespace().count())
        .filter(|n| *n > 0)
        .collect();
    if w.is_empty() {
        return false;
    }
    w.sort_unstable();
    w[w.len() / 2] >= MIN_CONTENT_LABEL_WORDS
}

/// A heading is a PROMISE: something is said under it. When the run
/// `prune_link_list_runs` just cut away WAS that something, the heading is left
/// standing over nothing — the reader is told a section exists and handed an
/// empty one, which is the one failure the pruner can produce that no amount of
/// re-reading recovers from.
///
/// MEASURED 2026-09-10 over the 39 pages of `tests/fixtures/pages`: the pruner
/// cuts 22 runs, and exactly THREE of them sit directly under a heading —
/// `## More on GOV.UK` (gov_uk_home, ten government services), `### Examples`
/// (go_pkg, an index of anchors) and `## SHOP BY Speed` (newegg_cat). Only the
/// first two are left EMPTY; newegg's heading still has a search field under it
/// and is not touched.
///
/// Un-pruning such a run was measured and REJECTED: of those three, only
/// gov.uk's is content — go_pkg's is a table of contents and newegg's a facet
/// list — so "a heading above it means it is content" buys one page and costs
/// two.
///
/// Returns the index in `out` of the heading to drop. `rest` is what follows
/// the pruned run, and a heading that is DEEPER than this one is its child, so
/// the section is not empty: `## A` followed by `### A.1` keeps `## A`.
fn heading_left_over_nothing(out: &[&str], rest: &[&str]) -> Option<usize> {
    let k = out.iter().rposition(|l| !l.trim().is_empty())?;
    let level = heading_level(out[k])?;
    for line in rest {
        if line.trim().is_empty() {
            continue;
        }
        return match heading_level(line) {
            Some(next) if next <= level => Some(k),
            _ => None,
        };
    }
    Some(k)
}

/// The same promise made by a LABEL: `Hidden categories:` on the line just
/// above the run, introducing it and nothing else. Wikipedia's maintenance
/// categories were pruned and the label was delivered alone (2026-10-03).
///
/// Narrow on purpose: a short line, ending in `:`, not a list item, not a
/// sentence that happens to end a paragraph with a colon (one under 60
/// characters has no room to be one).
fn label_left_over_nothing(out: &[&str]) -> Option<usize> {
    let k = out.iter().rposition(|l| !l.trim().is_empty())?;
    let t = out[k].trim();
    let label = t.ends_with(':') && t.chars().count() < 60 && !t.starts_with(['-', '*', '#']);
    label.then_some(k)
}

/// The ATX heading level of a line, `None` when the line is not a heading.
fn heading_level(line: &str) -> Option<usize> {
    let t = line.trim_start();
    let hashes = t.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) && t[hashes..].starts_with(' ') {
        Some(hashes)
    } else {
        None
    }
}

pub(super) fn prune_link_list_runs(md: &str) -> String {
    const MIN_RUN: usize = 6;
    // A run of link-only lines is a sidebar OR a headline list, and the
    // difference is the length of the LABELS. Measured over every run this
    // function prunes across the 147-page corpus: nav sits at a median of 1-3
    // words per label (`[Cache]`, `[UIEvent]`, `[1000Mbps]`, `[pics]`) and
    // tops out at 6 (`Art6 categoriesView allView all`, Wikipedia's
    // maintenance categories). Content starts at 7 and runs to 14 — the
    // Guardian's front page (`Slowdown in Arctic ice melt ends after largest
    // winter decline on record`) and a page of Arabic exam-result titles.
    // Both were being deleted whole.
    let link_only = re(r"^\s*(?:[-*]\s*)+(?:-?\s*\[[^\]]*\]\([^)]*\)[\s,;·|]*)+$");
    let lines: Vec<&str> = md.lines().collect();
    let mut out: Vec<&str> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        if link_only.is_match(lines[i]) {
            let mut j = i;
            while j < lines.len() && link_only.is_match(lines[j]) {
                j += 1;
            }
            if j - i < MIN_RUN || link_labels_read_like_content(&lines[i..j].join("\n")) {
                out.extend_from_slice(&lines[i..j]);
            } else if let Some(k) = heading_left_over_nothing(&out, &lines[j..]) {
                out.truncate(k);
            } else if let Some(k) = label_left_over_nothing(&out) {
                out.truncate(k);
            }
            i = j;
        } else {
            out.push(lines[i]);
            i += 1;
        }
    }
    out.join("\n")
}

#[cfg(test)]
mod a_heading_must_not_stand_over_nothing {
    use super::prune_link_list_runs;

    fn nav(n: usize) -> String {
        (0..n)
            .map(|i| format!("- [Cache{i}](/x{i})"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_heading_of_a_pruned_section_goes_with_it() {
        let md = format!("## More on GOV.UK\n\n{}\n", nav(10));
        let out = prune_link_list_runs(&md);
        assert!(
            !out.contains("More on GOV.UK"),
            "heading left over nothing: {out:?}"
        );
    }

    /// A label introducing the run goes with it, like a heading.
    #[test]
    fn the_label_of_a_pruned_run_goes_with_it() {
        let md = format!(
            "Prose that stays.\n\nHidden categories: \n{}\n\nTail.\n",
            nav(10)
        );
        let out = prune_link_list_runs(&md);
        assert!(
            !out.contains("Hidden categories"),
            "label left over nothing: {out:?}"
        );
        assert!(
            out.contains("Prose that stays.") && out.contains("Tail."),
            "{out:?}"
        );
        // A kept run keeps its label.
        let md = format!("Hidden categories:\n{}\n", nav(3));
        assert!(prune_link_list_runs(&md).contains("Hidden categories:"));
    }

    #[test]
    fn a_heading_whose_section_still_says_something_stands() {
        let md = format!("## Speed\n\n{}\n\nSearch Within:\n", nav(10));
        let out = prune_link_list_runs(&md);
        assert!(out.contains("## Speed"), "heading dropped: {out:?}");
        assert!(out.contains("Search Within:"), "body dropped: {out:?}");
    }

    #[test]
    fn a_parent_heading_is_not_emptied_by_its_child() {
        let md = format!("## A\n\n{}\n\n### A.1\n\nprose\n", nav(10));
        let out = prune_link_list_runs(&md);
        assert!(out.contains("## A\n"), "parent heading dropped: {out:?}");
        assert!(out.contains("### A.1"), "child heading dropped: {out:?}");
    }

    #[test]
    fn a_sibling_heading_leaves_the_section_empty() {
        let md = format!("### Examples\n\n{}\n\n### Constants\n\nprose\n", nav(10));
        let out = prune_link_list_runs(&md);
        assert!(!out.contains("### Examples"), "heading kept: {out:?}");
        assert!(out.contains("### Constants"), "sibling dropped: {out:?}");
    }

    #[test]
    fn a_run_too_short_to_prune_leaves_its_heading_alone() {
        // The smallest run the pruner refuses to cut (MIN_RUN is 6), so the
        // heading is never orphaned in the first place. The case just above it
        // is `the_heading_of_a_pruned_section_goes_with_it`.
        let md = format!("## More on GOV.UK\n\n{}\n", nav(5));
        let out = prune_link_list_runs(&md);
        assert!(
            out.contains("## More on GOV.UK"),
            "heading dropped: {out:?}"
        );
        assert!(out.contains("Cache4"), "run dropped: {out:?}");
    }

    #[test]
    fn a_paragraph_is_not_a_heading() {
        let md = format!("Something else entirely\n\n{}\n", nav(10));
        let out = prune_link_list_runs(&md);
        assert!(
            out.contains("Something else entirely"),
            "prose eaten: {out:?}"
        );
    }
}
