//! **What renders AS A BLOCK: paragraphs, lists, code blocks.**
//!
//! The heart of the rendering. A list carries its arity and its numbering —
//! renumbering an `<ol start="7">` from 1 is a FABRICATION, not a loss — and
//! a code block does not fold like prose.

use super::*;

/// Walk a node in block context, appending markdown to `out`.
/// The last non-empty line of `out` opens with a list marker (`- `, `N. `).
fn last_line_is_a_list_item(out: &str) -> bool {
    let Some(line) = out.trim_end_matches('\n').rsplit('\n').next() else {
        return false;
    };
    let body = line.trim_start();
    let digits = body.bytes().take_while(u8::is_ascii_digit).count();
    body.starts_with("- ") || (digits > 0 && body[digits..].starts_with(". "))
}

pub(super) fn block(n: NodeRef<Node>, out: &mut String, depth: usize) {
    // `depth` is LIST-NESTING depth (indentation), not recursion depth — it only
    // advances through `list_item`, so it cannot bound this walk. See
    // `MAX_WALK_DEPTH`.
    let _guard = match DepthGuard::enter() {
        Some(g) => g,
        None => return,
    };
    match n.value() {
        Node::Text(t) => {
            let text = collapse_ws(&t.text);
            // Text right after a list is not the list's: `<div><ul>…</ul>Tail
            // …</div>` wrote `Tail` on the line under the last bullet, which
            // Markdown reads as a lazy continuation of that ITEM. A blank line
            // ends the list first.
            if !text.trim().is_empty() && out.ends_with('\n') && last_line_is_a_list_item(out) {
                out.push('\n');
            }
            out.push_str(&text);
        }
        Node::Element(e) => {
            let name = e.name();
            // Recover content from elements the drop list would otherwise eat.
            if name == "svg" {
                if let Some(m) = mermaid_fence(n) {
                    out.push_str(&m);
                } else if let Some(m) = svg_illustration(n) {
                    out.push_str(&m);
                    out.push('\n');
                }
                return;
            }
            if name == "iframe" {
                if let Some(v) = video_link(n) {
                    out.push_str(&v);
                }
                return;
            }
            if dropped_at(n, name) {
                return;
            }
            if is_dup_drop(n) {
                return;
            }
            if is_boilerplate(n) {
                // Render what the unskipped walk WOULD have emitted here and
                // keep only its size: the misfire guard needs `full_vis`, not
                // the full string, so this replaces a whole second traversal
                // (F2). Skipping is off inside, exactly as in the full walk,
                // so nested boilerplate is counted the same way.
                let mut dropped = String::new();
                SKIP_BOILERPLATE.with(|c| c.set(false));
                block(n, &mut dropped, depth);
                SKIP_BOILERPLATE.with(|c| c.set(true));
                SKIPPED_VIS.with(|c| c.set(c.get() + vis_len(&dropped)));
                return;
            }
            // **A region that DECLARES itself a heading is one.** ARIA sets
            // the equivalence `role="heading"` + `aria-level` ≡ `<hN>`, as it
            // does for `role="navigation"` ≡ `<nav>`. Accordion components
            // write their label in a `<button>` wrapped in that role: the
            // button is discarded and the label went with it, leaving the
            // panel open on nothing.
            if let Some(level) = declares_itself_a_heading(n) {
                let text = collapse_ws(&subtree_text(n));
                let text = escape_atx_openers(text.trim());
                if !text.is_empty() {
                    out.push_str(&format!("\n\n{} {}\n\n", "#".repeat(level), text));
                }
                return;
            }
            match name {
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    let level = name[1..].parse::<usize>().unwrap_or(1);
                    let text = inline(n);
                    let text = text.trim();
                    if !text.is_empty() {
                        out.push_str(&format!("\n\n{} {}\n\n", "#".repeat(level), text));
                    }
                }
                // A paragraph is prose, but nothing stops its text from opening
                // with CommonMark's own heading marker, and `markdown::outline`
                // reads a `#` line as the document's structure. `<p># systemctl
                // daemon-reload</p>` — the Arch wiki's root prompt, 15 of them
                // on that one page — becomes an H1 that RE-PARENTS everything
                // after it: measured on a 12-section page, the outline reported
                // the real title as "(3 sections)" and opened a second
                // top-level document holding the other 9. Escape per LINE,
                // because a `<br>` puts several inside one paragraph, which is
                // exactly the archwiki shape.
                "p" => {
                    let mut body = String::new();
                    for c in n.children() {
                        block(c, &mut body, depth);
                    }
                    out.push_str("\n\n");
                    out.push_str(&escape_atx_openers(&body));
                    out.push_str("\n\n");
                }
                // A `<figure>` whose `<img>` carried no `alt` emits NOTHING for
                // the picture, so its `<figcaption>` is left standing alone and
                // reads as the section's own prose. On
                // en.wikipedia.org/wiki/Ada_Lovelace that is how "Sonnet titled
                // The Rainbow in Lovelace's own hand" became the preview for
                // `## Education`. Anchor the caption with a bare `[image]` so
                // `markdown::outline` can pass over it like any other figure.
                // Only when the alt produced no marker of its own — measured on
                // five real pages, that is 6 and 7 anchors on two Wikipedia
                // articles and ZERO on britannica, MDN and bbc.com, whose 27
                // alt-less images carry no caption at all. The caption text is
                // left verbatim: wrapping it instead is what a nested link in
                // the caption breaks.
                "figure" => {
                    let mut inner = String::new();
                    for c in n.children() {
                        block(c, &mut inner, depth);
                    }
                    let inner = inner.trim();
                    if !inner.is_empty() {
                        out.push_str("\n\n");
                        // …and only if the figure really carries a MEDIA,
                        // named for what it is.
                        // `<figure>` is the container of content DETACHED from
                        // the flow of the text, not of an illustration: HTML
                        // puts a list, a quote, a table, a listing in it.
                        // MEASURED 2026-09-10 on the 39 pages: `rust_book`
                        // carries five that are Rust code blocks and
                        // `yahoofin` one that is a list of analyst
                        // recommendations — six `[image]` claiming an
                        // illustration the page does not have. The Guardian's
                        // four figures all carry an `<img>` or an `<svg>` and
                        // keep theirs.
                        if !inner.contains("[image:") {
                            if let Some(mark) = figure_marker(n) {
                                out.push_str(mark);
                                out.push('\n');
                            }
                        }
                        out.push_str(inner);
                        out.push_str("\n\n");
                    }
                }
                "br" => out.push('\n'),
                "hr" => out.push_str("\n\n"),
                "ul" | "ol" => {
                    let ordered = name == "ol";
                    // **An ordered list does not always start at 1.** HTML says
                    // so in two ways — `<ol start="7">` for the list and
                    // `<li value="12">` for an item — and renumbering from 1
                    // does not lose a number: it INVENTS one. An agent asked
                    // for step 7 was reading step 1, on exactly the kind of
                    // page where a procedure is split into pieces.
                    let mut idx = attr(n, "start")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(1);
                    list_items_into(n, out, depth, ordered, &mut idx);
                    out.push('\n');
                }
                "li" => list_item(n, out, depth, false, 1),
                "dl" => {
                    for c in n.children() {
                        block(c, out, depth);
                    }
                }
                // `<summary>` is the LABEL of its `<details>`: the same thing as
                // a `<dt>` for its `<dd>`, and the page renders it as the only
                // visible line while the block is collapsed. Rendered as a
                // paragraph, it became indistinguishable from the answer it
                // announces, and a large page's OUTLINE carried none of its
                // sections — on `pages/go_pkg`, 25 examples of which not one
                // was named.
                //
                // A `<summary>` that ALREADY carries a heading keeps its own:
                // prefixing a second one would render `#### ### Question`.
                "summary"
                    if !n.descendants().any(|d| {
                        matches!(el_name(d), Some("h1" | "h2" | "h3" | "h4" | "h5" | "h6"))
                    }) =>
                {
                    let t = inline(n);
                    let t = t.trim();
                    if !t.is_empty() {
                        out.push_str(&format!("\n#### {t}\n"));
                    }
                }
                "dt" => {
                    let t = inline(n);
                    let t = t.trim();
                    if !t.is_empty() {
                        out.push_str(&format!("\n#### {t}\n"));
                    }
                }
                "dd" => {
                    let t = inline_lines(n).join("\n");
                    if !t.is_empty() {
                        out.push_str(&format!("\n{t}\n"));
                    }
                }
                "pre" => {
                    let lang = fence_lang(n).unwrap_or_default();
                    out.push_str(&code_fence(&pre_text(n), &lang));
                }
                "blockquote" => {
                    let mut inner = String::new();
                    for c in n.children() {
                        block(c, &mut inner, depth);
                    }
                    out.push_str("\n\n");
                    for line in inner.trim().lines() {
                        out.push_str("> ");
                        out.push_str(line);
                        out.push('\n');
                    }
                    out.push('\n');
                }
                "table" if is_a_layout_table(n) => {
                    // Not a table: its cells ARE the document. Recurse as
                    // blocks, like the anchor that wraps a card.
                    //
                    // Boilerplate sorting is NOT suspended in there, and it
                    // took a measurement to see it: a cell went through
                    // `inline`, which does not consult `is_boilerplate`, so
                    // rendering it as blocks suddenly subjects it to a sort it
                    // has never seen. The suspension was written, then
                    // REMOVED — on the corpus's 10 table pages it did not
                    // change a byte, and the hand-built case
                    // (`<div class="comment">` in a cell) is already caught by
                    // the misfire guard, which falls back to the unsorted pass
                    // as soon as the sort has eaten most of the page.
                    out.push('\n');
                    for c in n.children() {
                        block(c, out, depth);
                    }
                    out.push('\n');
                }
                "table" => {
                    out.push_str("\n\n");
                    // **The caption says what the table IS.** `<caption>` is
                    // the only place where a page names its table, and nothing
                    // rendered it: `table_to_md` starts from the `<tr>`s, and
                    // the caption is not one. A report or documentation page
                    // thus delivered columns without a subject. It comes out
                    // above, in italics, because that is what a markdown table
                    // can carry — it has no caption of its own.
                    if let Some(cap) = table_caption(n) {
                        out.push_str(&format!("*{cap}*\n\n"));
                    }
                    out.push_str(&table_to_md(n));
                    out.push_str("\n\n");
                }
                // An anchor that wraps block content (the whole card is a link,
                // `<a><div><h3>Title</h3>…</a>`) must recurse as blocks or the
                // nested heading is flattened into a link label and the page's
                // section structure collapses. Inline anchors render inline.
                "a" if has_block_descendant(n) => {
                    let mut buf = String::new();
                    for c in n.children() {
                        block(c, &mut buf, depth);
                    }
                    // Keep the card's link on its heading (matches the regex
                    // path's hoist_headings_out_of_links): a movie/product
                    // listing's whole point is the per-item URLs.
                    match attr(n, "href")
                        .map(str::trim)
                        .filter(|h| !h.is_empty() && !h.starts_with("javascript:"))
                    {
                        Some(href) => out.push_str(&linkify_first_heading(&buf, href)),
                        None => out.push_str(&buf),
                    }
                }
                // Inline elements appearing at block level: render inline.
                // An UNKNOWN element is one of them — see `renders_inline_here`.
                _ if renders_inline_here(name, n) => {
                    // `inline` starts from a FRESH buffer, so the boundary an
                    // empty element puts there cannot reach what precedes.
                    // This is where `matchingdesks` was being fabricated.
                    let piece = inline(n);
                    if piece.is_empty() {
                        if holds_only_whitespace(n) {
                            push_break(out);
                        }
                    } else {
                        // The SAME question as in the generic arm, and it has
                        // to be asked again here for the same reason: `inline`
                        // starts from a fresh buffer, so a boundary placed
                        // inside cannot see what precedes. Two sibling
                        // `<span>`s directly under a `<p>` go through here.
                        if welds_a_token(out, &piece) && prev_sibling_is_an_element(n) {
                            out.push(' ');
                        }
                        out.push_str(&piece);
                    }
                }
                // Structural block containers: separate and recurse.
                _ => {
                    out.push('\n');
                    for c in n.children() {
                        block(c, out, depth);
                    }
                    out.push('\n');
                }
            }
        }
        _ => {
            for c in n.children() {
                block(c, out, depth);
            }
        }
    }
}

/// The items of a list — **and real markup puts them elsewhere than in direct
/// children**.
///
/// The loop only looked at children named `li`, so a child with another name
/// silently took its WHOLE subtree with it. MEASURED 2026-09-11 on
/// `pages/1000__www_franceinfo_fr`: each section of the home page is written
/// `<ul><li>the main card</li><div class="…secondary-contents">
/// <li>…</li><li>…</li></div></ul>`, and the four secondary cards of each
/// section never arrived — **107 article titles missing** from the delivered
/// page, including "Dans le Nord, la pétanque est une affaire aussi sérieuse
/// qu'à Marseille". The HTML is invalid and a browser shows them anyway.
///
/// Three cases, and the third is the function's reason for being:
///  * an `li` is an item;
///  * a `ul`/`ol` is a nested list, rendered one level deeper;
///  * any other element is a WRAPPER the markup slipped in there: we descend
///    into it if it carries `li`s, and otherwise render it anyway — text in a
///    list is still text.
pub(super) fn list_items_into(
    parent: NodeRef<Node>,
    out: &mut String,
    depth: usize,
    ordered: bool,
    idx: &mut usize,
) {
    let _guard = match DepthGuard::enter() {
        Some(g) => g,
        None => return,
    };
    for c in parent.children() {
        match el_name(c) {
            Some("li") => {
                if let Some(v) = attr(c, "value").and_then(|v| v.trim().parse::<usize>().ok()) {
                    *idx = v;
                }
                list_item(c, out, depth, ordered, *idx);
                *idx += 1;
            }
            Some("ul") | Some("ol") => block(c, out, depth + 1),
            Some(_) => {
                if c.descendants().any(|d| el_name(d) == Some("li")) {
                    list_items_into(c, out, depth, ordered, idx);
                } else {
                    block(c, out, depth);
                }
            }
            // **Text written straight into a list is still text.** `<ul>Loose
            // text</ul>` (no `<li>`) is invalid HTML a browser shows anyway; it
            // was dropped whole here (html-to-markdown-rs, « render itemless
            // lists as text », 2026-10-04).
            None => {
                let text = inline(c);
                let text = text.trim();
                if !text.is_empty() {
                    out.push('\n');
                    out.push_str(&"  ".repeat(depth));
                    out.push_str(text);
                }
            }
        }
    }
}

pub(super) fn list_item(
    n: NodeRef<Node>,
    out: &mut String,
    depth: usize,
    ordered: bool,
    idx: usize,
) {
    // The `ul`/`ol` arm calls this directly, so the duplicate guard has to be
    // repeated here: ikea's shoppable-image overlay is nine `<li>` sharing one
    // "Product information page" label.
    if is_dup_drop(n) {
        return;
    }
    // A list item that wraps a heading is a content card (news/story/product
    // grids build these as `<li><h2><a>Title</a></h2><p>…</p></li>`). Flattening
    // it into a `- [Title](url)…` bullet both loses the heading and turns the
    // list into a link-dense run that the shared link-list pruner then deletes.
    // Promote: recurse as blocks so the heading stays a heading (matches the
    // regex path, which converts headings globally before link handling).
    // A DECLARED heading counts as a real one: an accordion item puts its
    // label in a `<div role="heading">` around a `<button>`, and without this
    // equality the promotion did not happen — the label stayed in the
    // bullet's label, where the discarded button took it along.
    // **`<article>` is the specification's word for "self-contained
    // composition"**, and a navigation bar never puts one in its entries. An
    // item carrying one is therefore a CARD, just like one carrying a
    // heading: unfolded as blocks, it is no longer written as a bullet made of
    // a single link, and the link-run pruner — which works on the rendered
    // text and does not have the tree — can no longer mistake it for a menu
    // entry.
    if n.descendants().any(|d| {
        matches!(
            el_name(d),
            Some("h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "article")
        ) || declares_itself_a_heading(d).is_some()
    }) {
        for c in n.children() {
            block(c, out, depth);
        }
        return;
    }
    let indent = "  ".repeat(depth);
    let marker = if ordered {
        format!("{idx}. ")
    } else {
        "- ".to_string()
    };
    // An item of several PARAGRAPHS (`<li><p>…</p><p>…</p></li>`): the first
    // is the bullet, the others its continuation — flattened, two sentences
    // of a step read as one (2026-10-04, after html-to-markdown-rs's
    // paragraph-boundary fixes). One paragraph, or none, stays a tight item.
    // A `<div>` child is a paragraph of the item too (`<li><div>A</div><div>B
    // </div></li>` read `A B` on one line).
    // Only one that holds text: yahoo's rating list opens each item on an
    // empty colour-dot `<div>`, which made the label empty.
    let is_para =
        |c: NodeRef<Node>| matches!(el_name(c), Some("p" | "div")) && !inline(c).trim().is_empty();
    let paragraphs = n.children().filter(|c| is_para(*c)).count();
    let mut continuation: Vec<String> = Vec::new();
    // Inline label = everything except nested lists (and, in a multi-paragraph
    // item, the paragraphs after the first).
    let mut label = String::new();
    let mut seen_p = 0usize;
    // Inline content met AFTER a block of the item is part of that block's
    // continuation, in source order: sent back to the label it read above the
    // block it follows (k8s: `or use the following command:` above both
    // commands).
    let mut tail = String::new();
    for c in n.children() {
        if matches!(el_name(c), Some("ul") | Some("ol")) {
            continue;
        }
        // A quotation, a table, a disclosure, a definition list or a figure in
        // an item is a block of its own: flattened into the label, a quote read
        // as the item's own words and a table as `Pricing Plan Cost Pro 9`,
        // which cost under which plan unsaid.
        let own_block = if matches!(
            el_name(c),
            Some("blockquote" | "table" | "details" | "dl" | "figure")
        ) {
            let mut quote = String::new();
            block(c, &mut quote, depth);
            Some(quote)
        } else if paragraphs >= 2 && is_para(c) {
            seen_p += 1;
            (seen_p >= 2).then(|| inline(c))
        } else {
            None
        };
        if let Some(text) = own_block {
            push_trimmed(&mut continuation, &std::mem::take(&mut tail));
            let text = text.trim();
            if !text.is_empty() {
                continuation.push(text.to_string());
            }
            continue;
        }
        let run = if continuation.is_empty() {
            &mut label
        } else {
            &mut tail
        };
        if el_name(c) == Some("br") {
            run.push('\n');
        } else {
            inline_into(c, run);
        }
    }
    push_trimmed(&mut continuation, &tail);
    let hang = " ".repeat(indent.len() + marker.len());
    // `<br>` lines of the label hang under it, like a continuation.
    let label = label
        .split('\n')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(&format!("\n{hang}"));
    // Leading newline only — a TIGHT list (no blank line between items), matching
    // the regex converter so the shared `collapse_repeated_list_items` post-pass
    // (which needs byte-identical CONSECUTIVE bullet lines) still fires. The
    // enclosing `ul`/`ol` adds the terminating newline; nested lists lead with
    // their own.
    out.push('\n');
    out.push_str(&indent);
    out.push_str(&marker);
    out.push_str(&label);
    for para in &continuation {
        out.push('\n');
        for line in para.lines() {
            out.push('\n');
            if !line.is_empty() {
                out.push_str(&hang);
                out.push_str(line);
            }
        }
    }
    // Nested lists one level deeper.
    for c in n.children() {
        if matches!(el_name(c), Some("ul") | Some("ol")) {
            block(c, out, depth + 1);
        }
    }
}

/// An inline run as one continuation block: its `<br>` lines trimmed, empty
/// ones dropped.
fn push_trimmed(blocks: &mut Vec<String>, text: &str) {
    let text = text
        .split('\n')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if !text.is_empty() {
        blocks.push(text);
    }
}

/// `class="language-rust"` / `lang-python` / `highlight-go` on the `<pre>` or an
/// immediate `<code>` child → the fence info string.
pub(super) fn fence_lang(pre: NodeRef<Node>) -> Option<String> {
    fn from_class(class: &str) -> Option<String> {
        for tok in class.split_whitespace() {
            for pfx in ["language-", "lang-", "highlight-"] {
                if let Some(rest) = tok.strip_prefix(pfx) {
                    if !rest.is_empty() {
                        return Some(rest.to_string());
                    }
                }
            }
        }
        None
    }
    if let Some(c) = attr(pre, "class").and_then(from_class) {
        return Some(c);
    }
    for c in pre.children() {
        if el_name(c) == Some("code") {
            if let Some(l) = attr(c, "class").and_then(from_class) {
                return Some(l);
            }
        }
    }
    None
}

/// Text content of a `<pre>`, whitespace PRESERVED (code is whitespace-sensitive).
pub(super) fn pre_text(pre: NodeRef<Node>) -> String {
    let mut out = String::new();
    fn rec(n: NodeRef<Node>, out: &mut String) {
        match n.value() {
            Node::Text(t) => out.push_str(&t.text),
            Node::Element(e) => {
                if e.name() == "br" {
                    out.push('\n');
                }
                for c in n.children() {
                    rec(c, out);
                }
            }
            _ => {
                for c in n.children() {
                    rec(c, out);
                }
            }
        }
    }
    rec(pre, &mut out);
    out
}
