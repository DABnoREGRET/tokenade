//! **What renders WITHIN a line.**
//!
//! The boundary between inline and block is not a list of names: the same
//! element renders in the flow of text or as a block depending on what it
//! CONTAINS, and a generic arm "every unknown name is inline" once sent
//! `<body>` down this path. The rule starts from the specification's POSITIVE
//! criterion.
//!
//! Welding matters just as much: `<i>A</i>, <i>B</i>` rendered `A , B`
//! produced **86 false losses out of 1,504**. We only separate between two
//! ELEMENTS, and only if the cut is a block or welds two words.

use super::*;

/// Does this element render WITHIN the line, here?
///
/// The listed names are HTML's inline elements. The second test is the one
/// that was missing: **an element HTML does not know is `display: inline`**
/// — that is the browser's rule, not a guess — and it fell into the generic
/// arm, which puts a line break on either side. MEASURED 2026-09-10 on the 39
/// corpus pages: `<fin-streamer>` ×96 on a Yahoo quote, `<gu-island>` ×133 at
/// the Guardian, `<woltlab-core-date-time>` ×59 on a forum, `<fa-icon>` ×46. A
/// sentence containing one arrived on THREE lines — "Le titre a gagne" /
/// "2,31 %" / " hier, apres une seance calme" — and the tail fragment opened
/// with a space.
///
/// The criterion comes from the specification: **a custom element's name
/// carries a hyphen**, and no HTML element carries one. An enumerated list
/// could not answer, the name being arbitrary; "every unknown name" answered
/// too much, and `<body>` went through that arm — a `<body>` without a block
/// child went through `inline`, which does not know `<iframe>`, and a page's
/// video disappeared.
///
/// The bound is the same as for `<a>` just above: an element that CARRIES
/// block content stays a block, otherwise a whole card would fit into a line.
pub(super) const PHRASING: [&str; 42] = [
    "a", "code", "img", "span", "strong", "b", "em", "i", "small", "sub", "sup", "abbr", "cite",
    "q", "mark", "time", "label", "math", "ruby", "rt", "rp", "rb", "rtc", "del", "ins", "s", "u",
    "var", "samp", "kbd", "dfn", "bdi", "bdo", "data", "output", "wbr", "tt", "big", "strike",
    "acronym", "progress", "meter",
];

pub(super) fn renders_inline_here(name: &str, n: NodeRef<Node>) -> bool {
    // **The list is the specification's PHRASING CONTENT**, not the names we
    // happened to meet. It named seventeen, and three missing families cut a
    // sentence into three — measured 2026-09-10 on hand-written pages,
    // because the corpus does not contain them:
    //  * `<math>` (MathML is INLINE by default): "La formule" / "a" / "2" /
    //    "b" … one line per symbol, on the pages where a formula lives;
    //  * `<ruby>`/`<rt>`, the Japanese reading annotation: "Le mot" / "漢字" /
    //    "かんじ" / "se lit ainsi";
    //  * `<del>`/`<ins>`, which every changelog writes: "Version" /
    //    "ancienne" / "nouvelle" / "du texte".
    // The names listed are the specification's, plus the four legacy forms
    // old pages still write (`tt`, `big`, `strike`, `acronym`) — a browser
    // renders them inline too.
    // `<progress>` and `<meter>` close the list: they are phrasing content
    // too, and their content is the FALLBACK a browser too old to draw them
    // reads — "70 %", "3 sur 10". Measured by hand 2026-09-10: "La tache est
    // a" / "70 %" / "et la charge a" / "3 sur 10" / "aujourd hui.", one
    // sentence on five lines.
    // `<del>`, `<ins>` and `<math>` are the three that can also wrap BLOCK
    // content — HTML allows them around a whole paragraph. The bound is the
    // one of `<a>` just above: what carries block content stays a block.
    if is_phrasing_element_name(name) {
        return !has_block_descendant(n);
    }
    false
}

/// **Is this name that of PHRASING content?** The question without the tree,
/// for the `page_loss` probe: it reads HTML with regexes and must ask the
/// same question as the rendering — a boundary between two BLOCK elements
/// separates, between two phrasing elements it does not. Two spellings of
/// "what renders within the line" would make the rule and its judge diverge,
/// as for [`is_machine_only_markup`].
///
/// **A CUSTOM ELEMENT** is one of them, and the specification gives the
/// criterion: its name carries a hyphen. Nothing else carries one, and
/// relying on "unknown name" sent `<body>` through this arm — a `<body>`
/// without a block child then went through `inline`, which does not know
/// `<iframe>`, and a page's video disappeared.
pub fn is_phrasing_element_name(name: &str) -> bool {
    PHRASING.contains(&name) || name.contains('-')
}

/// Elements a browser lays out on their own line. Rendering one inside an
/// inline context (a table cell, a heading, a list item, a link label) has to
/// keep the boundary, or two sentences become one word.
///
/// The chrome names (`nav`, `header`, `form`, …) are deliberately absent: they
/// never reach here, `is_dropped` removes them first, and listing them would
/// suggest this is the place that decides.
pub(super) fn is_block_level(name: &str) -> bool {
    matches!(
        name,
        "p" | "div"
            | "section"
            | "article"
            | "main"
            | "blockquote"
            | "pre"
            | "hr"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "ul"
            | "ol"
            | "li"
            | "dl"
            | "dt"
            | "dd"
            | "table"
            | "thead"
            | "tbody"
            | "tfoot"
            | "tr"
            | "td"
            | "th"
            | "caption"
            | "figure"
            | "figcaption"
            | "address"
            | "details"
            | "summary"
    )
}

/// The separator between two blocks flattened into one line: ONE space, never
/// a leading one. A cell that opened with a stray space made the first column
/// of the table look indented and cost a byte on every row.
/// **Would gluing these two make a word the page does not contain?**
///
/// Two inline siblings that CSS separates arrived welded: stripe.com renders
/// `<span>amount</span><span>integer</span>` inside an `<h4>`, and the
/// delivered heading said `#### amountinteger` — an identifier that does not
/// exist, on an API reference, the very document where an agent copies field
/// names. MEASURED 2026-09-09: **54 fabricated words on that page alone**,
/// plus `Length21 cm Width15 cm Height6 cm` on an ikea product page.
///
/// The rule the generic arm already holds for BLOCK elements ("a token the
/// page never contained"), one level down.
pub(super) fn welds_a_token(out: &str, piece: &str) -> bool {
    out.chars().next_back().is_some_and(|c| c.is_alphanumeric())
        && piece.chars().next().is_some_and(|c| c.is_alphanumeric())
}

/// **Is the left neighbour an ELEMENT, and not text?**
///
/// That is the whole difference between two boxes CSS spaces apart and a word
/// that markup cuts in two. `docs.rs/serde` carries
/// `<em><strong>de</strong></em>serializing`: an element followed by TEXT,
/// and the two halves of a single word. Separating there would render
/// `de serializing`, the exact corruption we claim to fix — measured on the
/// corpus, this case exists and this bound is what spares it.
pub(super) fn prev_sibling_is_an_element(n: NodeRef<Node>) -> bool {
    n.prev_siblings()
        .find(|s| match s.value() {
            Node::Text(t) => t.text.chars().any(|c| !c.is_whitespace()),
            Node::Element(_) => true,
            _ => false,
        })
        .is_some_and(|s| matches!(s.value(), Node::Element(_)))
}

pub(super) fn push_break(out: &mut String) {
    if !out.is_empty() && !out.ends_with(char::is_whitespace) {
        out.push(' ');
    }
}

/// Render a node's subtree as a single inline string (no block breaks). Used for
/// heading/list-item/table-cell/dt/dd/link labels.
pub(super) fn inline(n: NodeRef<Node>) -> String {
    let mut out = String::new();
    inline_into(n, &mut out);
    out
}

/// A node's inline text cut into its LINES at the `<br>` it holds directly —
/// the same lines a `<p>` keeps (its `br` is a block-level newline). Rendered
/// with `inline` alone, `first<br>second` read `first second`.
pub(super) fn inline_lines(n: NodeRef<Node>) -> Vec<String> {
    let mut lines = vec![String::new()];
    for c in n.children() {
        if matches!(c.value(), Node::Element(e) if e.name() == "br") {
            lines.push(String::new());
        } else if let Some(line) = lines.last_mut() {
            inline_into(c, line);
        }
    }
    lines
        .iter()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

pub(super) fn inline_into(n: NodeRef<Node>, out: &mut String) {
    let _guard = match DepthGuard::enter() {
        Some(g) => g,
        None => return,
    };
    match n.value() {
        Node::Text(t) => out.push_str(&collapse_ws(&t.text)),
        Node::Element(e) => {
            let name = e.name();
            // A removed DUPLICATE leaves nothing behind: its twin is right
            // next to it and already carries its boundaries. Measured by
            // trying the opposite — `push_break` on the three removals
            // rendered "(opens in a new tab )" on gov.uk, a space before the
            // closing parenthesis where the browser shows none.
            if dropped_at(n, name) || is_dup_drop(n) || is_aria_hidden_twin(n) {
                return;
            }
            // **A formula renders ONCE, and TeX is the only one of the three
            // forms that says it.** Wikipedia writes the same `a^2+b^2=c^2`
            // three times: the presentation MathML, the TeX `<annotation>`,
            // and the fallback image's `alt`. Flattening the presentation does
            // not render the formula — `a 2 + b 2 = c 2 .`, where the exponent
            // has VANISHED: it is no longer an equation, it is a sequence of
            // symbols, and nothing tells the reader the 2 was a superscript.
            //
            // The TeX is declared by the page in two ways, both in the
            // specification: the `alttext` attribute of `<math>` and
            // `<annotation encoding="application/x-tex">`. Rendered VERBATIM
            // between `$…$` — the markdown convention for mathematics —
            // because rewriting what the page wrote is a fabrication:
            // Wikipedia's `{\displaystyle …}` is valid TeX and it stays.
            if name == "math" {
                if let Some(tex) = math_tex(n) {
                    out.push('$');
                    out.push_str(&tex);
                    out.push('$');
                    return;
                }
                // No declared TeX: the presentation MathML is all the page has,
                // and it is better than nothing.
            }
            match name {
                "br" => out.push(' '),
                "input" if super::prune::is_a_checkbox(n) => {
                    out.push_str(if e.attr("checked").is_some() {
                        "[x] "
                    } else {
                        "[ ] "
                    });
                }
                "img" => {
                    // Tracking-pixel beacons sometimes carry a plausible alt;
                    // drop them by filename (matches the regex path).
                    let src = e.attr("src").unwrap_or("");
                    let is_pixel = [
                        "s_1x2.gif",
                        "pixel.gif",
                        "spacer.gif",
                        "blank.gif",
                        "clear.gif",
                    ]
                    .iter()
                    .any(|p| src.contains(p));
                    if !is_pixel {
                        if let Some(alt) = e.attr("alt").map(str::trim).filter(|a| !a.is_empty()) {
                            // **A formula's fallback image IS that formula.**
                            // Wikipedia puts, after the `<math>`, an `<img>`
                            // whose `alt` is the TeX word for word: it is what
                            // a browser that does not draw MathML sees. The
                            // formula was just rendered right before; the mark
                            // would deliver it a second time.
                            if !alt_repeats_a_formula(n, alt) {
                                out.push_str(&crate::markdown::image_marker(Some(&one_line(alt))));
                            }
                        }
                    }
                }
                "a" => {
                    let label = {
                        let mut s = String::new();
                        for c in n.children() {
                            inline_into(c, &mut s);
                        }
                        s.trim().to_string()
                    };
                    if label.is_empty() {
                        // A link whose content is ONLY whitespace still
                        // separated its neighbours: that is in fact all it
                        // did. See `holds_only_whitespace`.
                        if holds_only_whitespace(n) {
                            push_break(out);
                        }
                        return;
                    }
                    match e
                        .attr("href")
                        .map(str::trim)
                        // `#cite_ref-1` leads back into the page the reader
                        // already holds — the label alone, as `links_to_md`
                        // (web/elements.rs) has always done. A Wikipedia
                        // bibliography came out as `[↑](#cite_ref-1)` ×N.
                        .filter(|h| {
                            !h.is_empty() && !h.starts_with("javascript:") && !h.starts_with('#')
                        }) {
                        Some(href) => out.push_str(&crate::web::md_link(&label, href)),
                        None => out.push_str(&label),
                    }
                }
                "code" => {
                    let mut s = String::new();
                    for c in n.children() {
                        inline_into(c, &mut s);
                    }
                    out.push_str(&inline_code(&s));
                }
                // `<q>` is drawn WITH its quotation marks by every browser (the
                // marks are CSS, not text). Without them a Wikipedia reference
                // ran its quoted passage into the citation: `ed.). It should
                // now be clear…` — 11 `<q>` on two corpus pages.
                "q" => {
                    let mut s = String::new();
                    for c in n.children() {
                        inline_into(c, &mut s);
                    }
                    let quoted = s.trim_start().starts_with(['"', '“', '«', '\'', '‘']);
                    if quoted || s.trim().is_empty() {
                        out.push_str(&s);
                    } else {
                        out.push('"');
                        out.push_str(&s);
                        out.push('"');
                    }
                }
                "sup" | "sub" => {
                    let mut s = String::new();
                    for c in n.children() {
                        inline_into(c, &mut s);
                    }
                    out.push_str(&raised_or_lowered(name == "sup", &s));
                }
                _ => {
                    // A BLOCK element inside an inline context still ends a
                    // line in a browser, so its neighbours must not touch.
                    // Without this, `<td><p>a.</p><p>b.</p></td>` rendered
                    // `a.b.` — a token the page never contained, exactly the
                    // fabrication `is_aria_hidden_twin` exists to prevent.
                    let breaks = is_block_level(name);
                    if breaks {
                        push_break(out);
                    }
                    let mut s = String::new();
                    for c in n.children() {
                        inline_into(c, &mut s);
                    }
                    if !breaks && welds_a_token(out, &s) && prev_sibling_is_an_element(n) {
                        out.push(' ');
                    }
                    out.push_str(&s);
                    if breaks {
                        push_break(out);
                    }
                }
            }
        }
        _ => {
            for c in n.children() {
                inline_into(c, out);
            }
        }
    }
}
