//! What, in a page, is NOT the page — and which part of the tree is.
//!
//! A single subject: separating content from scenery. Nothing here RENDERS
//! anything; everything answers yes or no to "is this node substance?", and
//! `find_main_root` asks the same question at document scale. Rendering
//! (`block`, `inline_into`, the tables) stays with the parent, which consults
//! these predicates but does not write them.
//!
//! `use super::*` reaches the thread-local variables (`SKIP_BOILERPLATE`,
//! `ROOT_TEXT_LEN`, `DUP_DROPS`) and the shared accessors (`el_name`, `attr`,
//! `text_len`) — the parent's privacy is visible to its child, so no item had
//! to be widened for this split.

use super::hidden::*;
use super::*;

/// Priority list mirroring `web::MAIN_CONTENT_SELECTORS` plus `main`/`article`.
pub(super) const MAIN_SELECTORS: [&str; 21] = [
    "main",
    "article",
    "#readme",
    ".markdown-body",
    "#mw-content-text",
    ".mw-parser-output",
    ".docs-content",
    ".doc-content",
    ".prose",
    ".article-body",
    ".main-content",
    "#main-content",
    ".content",
    "#content",
    ".post-content",
    ".entry-content",
    ".article-content",
    ".page-content",
    "#inside",
    ".stories",
    ".itemlist",
];

/// Count `<h1>`..`<h6>` descendants of a node.
pub(super) fn heading_count(n: NodeRef<Node>) -> usize {
    n.descendants()
        .filter(|d| matches!(el_name(*d), Some("h1" | "h2" | "h3" | "h4" | "h5" | "h6")))
        .count()
}

/// First main-content candidate that is the document's ACTUAL content, not one
/// section of a multi-section front page. Isolate only when the candidate holds
/// the bulk of the text (an article stripped of chrome) OR most of the page's
/// headings (a doc/article). A `<main>` that holds a minority of both is a
/// section of a card grid — keep the whole body and let boilerplate-skip prune
/// chrome, mirroring the regex path's `outside_region_if_main_is_a_section`.
pub(super) fn find_main_root(doc: &Html) -> Option<NodeRef<'_, Node>> {
    let root = doc.tree.root();
    let body_len: usize = text_len(root);
    let total_headings = heading_count(root);
    for sel in MAIN_SELECTORS {
        let selector = match scraper::Selector::parse(sel) {
            Ok(s) => s,
            Err(_) => continue,
        };
        if let Some(el) = doc.select(&selector).next() {
            let text_len: usize = el.text().map(|t| t.trim().len()).sum();
            if text_len < 200 {
                continue;
            }
            // Isolate only when the candidate holds the MAJORITY of the page's
            // headings — the signature of "this is the content", not one section
            // of a card grid. A `<main>` with a minority of headings is a front
            // page section; keep the whole body so sibling cards survive (the
            // regex path's `outside_region_if_main_is_a_section`). Bulk-text
            // alone isolates only on genuinely heading-less pages, where the
            // heading signal can't be trusted.
            let holds_headings = total_headings > 0 && heading_count(*el) * 2 > total_headings;
            let heading_poor = total_headings <= 1;
            let is_bulk = text_len * 100 >= body_len * 55;
            if holds_headings || (heading_poor && is_bulk) {
                return Some(*el);
            }
        }
    }
    None
}

/// True if a container element's class/id marks it as boilerplate (nav, share,
/// cookie, sidebar, footer, related…). Mirrors `web::strip_negative_class_blocks`
/// but on the tree, so it can never mis-slice on a stray `>`. Applied only to
/// structural containers, and only in the selective walk.
pub(super) fn is_boilerplate(n: NodeRef<Node>) -> bool {
    let name = match el_name(n) {
        Some(x) => x,
        None => return false,
    };
    if !SKIP_BOILERPLATE.with(|c| c.get()) {
        return false;
    }
    if !matches!(
        name,
        "div"
            | "section"
            | "ul"
            | "ol"
            | "dl"
            | "aside"
            | "nav"
            | "span"
            | "p"
            | "form"
            | "header"
            | "footer"
            | "figure"
            // `<dialog>` is judged here like a `role="dialog"`: the same
            // declaration, carried by the element instead of the attribute.
            | "dialog"
    ) {
        return false;
    }
    // **A modal window DECLARES itself, and it is scenery.** `role="dialog"`
    // (or the `<dialog>` element) says the region is laid ON TOP of the page:
    // a notification, a sign-up prompt, a consent banner, a region picker.
    // MEASURED 2026-09-10 on the 39 pages: 28 regions declare it and **not
    // one carries content**.
    //
    // It goes through HERE and not through the exclusion list, because a
    // mislabelled container must stay recoverable: this function's CUMULATIVE
    // guard refuses the whole removal when it takes too much of the page, and
    // that is enough. A local `!spans_most_of_the_page(n)` was written, then
    // REMOVED: on a page whose window covers 75% of the text, the mutant that
    // deletes it changes nothing — the cumulative guard has already decided.
    // A guard no canary can fire is a precaution that reassures about a
    // different question.
    if el_name(n) == Some("dialog")
        || attr(n, "role")
            .and_then(|r| r.split_whitespace().next())
            .is_some_and(|r| {
                r.eq_ignore_ascii_case("dialog") || r.eq_ignore_ascii_case("alertdialog")
            })
    {
        return true;
    }
    let class = attr(n, "class").unwrap_or("");
    let id = attr(n, "id").unwrap_or("");
    if class.is_empty() && id.is_empty() {
        return false;
    }
    // Same three exemptions the regex path proved on the corpus
    // (`web::strip_negative_class_blocks`), so the two converters agree:
    //  1. A Sphinx section id (`id="s-<heading-slug>"`) names real content, not
    //     chrome — django's `related_name` docs lived in `id="s-be-careful-with-
    //     related-name">` and collided with `related`.
    if id.starts_with("s-") {
        return false;
    }
    //  2. A class that TURNS AN AD OFF is not an ad (vogue's story grid wears
    //     `hide-read-more-ad`; `ad-free`, `no-ads` likewise).
    if crate::web::negates_the_keyword(class) || crate::web::negates_the_keyword(id) {
        return false;
    }
    //  3. Tailwind-style UTILITY classes share the class attribute with the
    //     semantic component name, and a utility can spell a keyword — it names
    //     a shadow, a background image or a CSS filter, never the block's role.
    //     MEASURED 2026-07-22 on the fidelity corpus: sportingnews_art.html has
    //     16 elements whose ONLY keyword hit is a utility —
    //     `md:shadow-dropdown-menu` on 12 of them, plus
    //     `md:bg-desktop-menu-arrow`, `md:bg-mobile-sidebar-close`,
    //     `md:group-hover/nav:text-d1` and an arbitrary-value
    //     `md:[filter:brightness(0)_saturate(100%)…]`. Every one carries a `:`
    //     (a Tailwind variant prefix, or an arbitrary value), and no semantic
    //     component name ever does — a class attribute is whitespace-separated
    //     and `:` is not legal in a CSS identifier without escaping. So dropping
    //     colon-bearing tokens from the haystack removes the entire collision
    //     class and can cost nothing.
    let semantic_class = strip_utility_classes(class);
    let hay = format!("{semantic_class} {id}").to_ascii_lowercase();
    //  4. **`promo` is no longer a scenery word** — it used to be, lowercased,
    //     back when only the BBC cards' capitalised `Promo` had to survive.
    //     MEASURED 2026-09-10 on the 39 pages, this word decides by coin toss:
    //     it discards CONTENT on `slack.com` (the whole page body is built from
    //     `c-promo__*`, **4,059 bytes**, 8 paragraphs out of 9) and on
    //     `gov_uk_home` (the "Featured" section and its services), and it
    //     discards advertising on `investopedia_h` (326 B) and `ikea_product`
    //     (93 B). Ten to one in favour of content.
    //
    //     A discriminant was looked for and does not exist here: Investopedia's
    //     simulator ad is twenty words of prose under a heading, exactly like
    //     Slack's `c-promo__lede`; and the gov.uk card is ten, like IKEA's.
    //     Neither length nor heading count separates them. A word that decides
    //     nothing is better removed than kept on a hunch: real ads are still
    //     caught by `sponsor`, `advert`, `subscribe`, `signup` and the short
    //     family.
    // **`modal` was removed from this list.** A product page's collapsible
    // panel is called `pipf-product-details-modal__container` at IKEA, and
    // that is where the product description and its safety instructions live.
    // MEASURED on the 39 pages: ten carry a `modal` class, the word discards
    // scenery on NONE of them, and it discarded 953 bytes of content on one —
    // including "Do not use with sharp metal cutlery". The scenery it aimed at
    // is still caught by what really names it: `cookie`, `consent`,
    // `newsletter`, `subscribe`, `signup`, `popup`, and `form`, which the walk
    // discards whole. Same lesson as with `promo`: a design-system word is not
    // a scenery signal.
    //
    // **`teaser` was removed from this list.** The word comes from NEWS
    // vocabulary, where a teaser is an article's summary — that is, content —
    // and design systems use it to name an editorial card. MEASURED 2026-09-10
    // on the 39 pages: it hits three blocks. Two are the buying guides of an
    // IKEA category page ("How would you like to explore? By style, feature,
    // size or shape", "Helpful how-tos for more enjoyable dining", with their
    // six sections and their descriptions) and the third a recommendations
    // banner ("Ideas based on your recently viewed products"). **3,508 bytes
    // of content against 46 of scenery**, seventy-six to one. No
    // discriminant: both shapes are a heading followed by a sentence, and
    // neither length nor heading count separates them. Same outcome as
    // `promo` and `modal` — a word that cuts so badly against itself is better
    // removed. The banner it aimed at keeps its 46 bytes; that is the price,
    // and it is quantified.
    //
    // Curated from trafilatura OVERALL_DISCARD_XPATH + firecrawl
    // EXCLUDE_NON_MAIN_TAGS — only unambiguous chrome (nothing that names real
    // content, so front-page card sections are not dropped). `promo` is handled
    // case-sensitively above, so it is NOT in this lower-cased list.
    const STRONG: [&str; 24] = [
        "sidebar",
        "navbar",
        "sharing",
        "social",
        "newsletter",
        "cookie",
        "consent",
        "breadcrumb",
        "footer",
        "widget",
        "advert",
        "sponsor",
        "outbrain",
        "taboola",
        "paywall",
        "subscribe",
        "signup",
        "popup",
        "pagination",
        "masthead",
        "disqus",
        "skyscraper",
        "subnav",
        "lang-select",
    ];
    if STRONG.iter().any(|k| hay.contains(k)) {
        return !spans_most_of_the_page(n);
    }
    const SHORT: [&str; 12] = [
        "nav", "menu", "share", "banner", "byline", "ad", "ads", "comment", "filter", "filters",
        "facet", "facets",
    ];
    let short_hit = hay.split_whitespace().any(|name| {
        let toks: Vec<&str> = name
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|t| !t.is_empty())
            .collect();
        toks.iter().enumerate().any(|(i, tok)| {
            SHORT.contains(tok) && !names_one_entry_of_a_navigation(tok, toks.get(i + 1).copied())
        })
    });
    if short_hit {
        // Ambiguous tokens (ad/ads/nav/menu…) also appear in the class names of
        // genuine content sections — aljazeera's `container--section-more-ads`
        // holds 10 story cards. A real ad/nav/menu block has no headings; only
        // treat the container as boilerplate when it is content-poor.
        return heading_count(n) < 2 && !spans_most_of_the_page(n);
    }
    false
}

/// **`nav-item` does not name a navigation bar: it names ONE ENTRY.**
///
/// The vocabulary comes from Bootstrap, which half the web copies: `nav` is
/// the bar, `nav-item` one of its entries, `nav-link` the anchor inside. On a
/// real bar, it is the CONTAINER that carries the signal and it goes whole —
/// its entries are therefore never asked this question. A `nav-item` class
/// that gets this far has, by construction, no ancestor judged to be scenery:
/// it is not a bar entry, it is a component that borrowed the word.
///
/// MEASURED 2026-09-10 on the 39 pages: the `nav` token hits 18 times. Only
/// one shape is followed by `item`, `c-switcher-nav-item__heading-row` on
/// `slack.com`, and it carries **the six product promises of the home page
/// carousel** — "Summarize a conversation you missed", "Get fast answers with
/// Claude", "Review code with Github Copilot". The other seventeen are real
/// bars (`tab-nav`, `left-nav`, `in-page-nav`, `common-nav`,
/// `nav nav-tabs`) and keep their verdict, word for word.
///
/// Two broader leads were measured and REFUSED on the same survey:
///  * removing `nav` from the short list returns Slack's carousel but also
///    IKEA's category bar (20 navigation words) and Newegg's tabs — one page
///    won, two lost;
///  * requiring the block to be LINK-DENSE separates nothing: `left-nav` has
///    198 words with 4 inside links, and the `tab-nav` tabs have no link.
pub(super) fn names_one_entry_of_a_navigation(tok: &str, next: Option<&str>) -> bool {
    tok == "nav" && matches!(next, Some("item") | Some("link"))
}

/// Strip Tailwind-style utility classes from a class attribute, leaving the
/// semantic component names `is_boilerplate` actually wants to match. A utility
/// is recognised by the `:` its variant prefix (`md:`, `hover:`,
/// `group-hover/nav:`) or arbitrary value (`[filter:…]`) always carries; a
/// hand-written component class never has one. See `is_boilerplate` exemption 3
/// for the measured collisions this removes.
pub(super) fn strip_utility_classes(class: &str) -> String {
    let mut out = String::with_capacity(class.len());
    for tok in class.split_ascii_whitespace().filter(|t| !t.contains(':')) {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(tok);
    }
    out
}

/// A boilerplate block is PERIPHERAL by definition, so a keyword match that
/// covers most of the region being walked is a LAYOUT WRAPPER, not chrome.
///
/// The regex path has refused such matches since newegg.com's
/// `<div class="row has-side-banner">` — where `banner` matched a layout
/// MODIFIER and the cut took the product grid with it — via
/// `web::MAX_BOILERPLATE_PERCENT`. The tree path shipped without the guard, and
/// the corpus shows the same misfire: MEASURED 2026-07-22, `django_leaf.html`
/// matches `sidebar` on `class="container sidebar-right"`, a wrapper holding
/// 95% of the page's text, and MDN's `layout__2-sidebars-inline` holds 56%
/// (`mdn_doc.html`) and 63% (`mdnjp_leaf.html`).
///
/// An ABSOLUTE character override ("nothing over ~5 000 chars is chrome") was
/// considered and REJECTED on the same measurement: 23 matched blocks in the
/// corpus carry more than 5 000 chars and nearly all are genuine chrome —
/// kubernetes.io's `td-sidebar-nav` is 21 561 chars of navigation links, MDN's
/// `left-sidebar` 11 840. A documentation site's table of contents is a real
/// navigation bar that IS tens of thousands of characters long, so absolute size
/// does not separate chrome from content. The proportion does.
///
/// Without this the whole-document misfire guard in `select_and_convert_parsed`
/// is the only backstop, and it is all-or-nothing: one over-broad wrapper makes
/// it abandon boilerplate-skip for the ENTIRE page. Refusing just the wrapper
/// lets the walk descend into it and still prune the real chrome inside.
pub(super) fn spans_most_of_the_page(n: NodeRef<Node>) -> bool {
    let root_len = ROOT_TEXT_LEN.with(|c| c.get());
    if root_len == 0 {
        // Unknown region size (`fragment_to_markdown` converts a region the
        // caller already selected): no proportion to test, so do not second-guess
        // the keyword.
        return false;
    }
    text_len(n) * 100 > root_len * crate::web::MAX_BOILERPLATE_PERCENT
}

/// Elements dropped whole — chrome and non-content. The upstream selection
/// already strips most of these; this is defence in depth on the tree side.
pub(super) fn is_dropped(name: &str) -> bool {
    matches!(
        name,
        "script"
            | "style"
            | "noscript"
            // `<noframes>` and `<noembed>` are RAW TEXT to the parser — their
            // content is not a tree, it is a string that CONTAINS markup, and
            // it went out as-is: `<body><p>Votre navigateur…<a
            // href="contenu.html">la page de contenu</a></p>` in the middle of
            // the delivered markdown. The reader sees tags, which a page never
            // shows.
            //
            // Discarded like `<noscript>`: they are fallbacks for a browser
            // that cannot do something, and the frameset page has a better
            // answer — the thin-result note names the frame's address, which
            // is what should be requested.
            | "noframes"
            | "noembed"
            // **A MathML `<annotation>` is NEVER rendered by a browser**: it
            // is the formula's SOURCE ENCODING, the TeX or content MathML of
            // which the presentation next to it is the drawn form. Its text
            // went out as-is in the middle of the prose —
            // `{\displaystyle a^{2}+b^{2}=c^{2}.}` glued to the sentence —
            // markup the page does not show, exactly like `<noframes>` just
            // above. The formula itself is rendered by the `math` arm of
            // `inline_into`, which READS that TeX instead of letting it
            // leak.
            | "annotation"
            | "annotation-xml"
            | "svg"
            | "iframe"
            | "template"
            | "title"
            | "head"
            | "header"
            | "footer"
            | "nav"
            | "aside"
            | "form"
            | "button"
            | "input"
            | "select"
            | "option"
            | "canvas"
            | "audio"
            | "video"
    )
}

/// `is_dropped` by NAME alone is wrong for `<header>`/`<footer>`: HTML5 scopes
/// both to their nearest sectioning ancestor, so they are PAGE chrome only when
/// that ancestor is the page itself. Inside an `<article>`/`<section>` a
/// `<header>` holds THAT ITEM's title.
///
/// MEASURED 2026-07-22 on huggingface.co/models: all 30 result cards wrap the
/// model id in `<article><header><h4>org/model</h4></header>`, so dropping by
/// name deleted every row's identifier and produced a listing of bare metadata
/// ("Text Generation • 4B • Updated 4 days ago • 432k") that no agent can act
/// on. A listing without item names is not a compacted listing, it is a lost
/// one — and nothing in the output said so.
fn is_a_component_slot(n: NodeRef<Node>) -> bool {
    let Node::Element(e) = n.value() else {
        return false;
    };
    e.attrs()
        .any(|(k, _)| k == "slot" || k.starts_with("v-slot") || k.starts_with('#'))
}

pub(super) fn dropped_at(n: NodeRef<Node>, name: &str) -> bool {
    if is_machine_only_microdata(n) {
        return true;
    }
    if is_a_hidden_tooltip(n) {
        return true;
    }
    // A page can DECLARE what a region is, instead of leaving it to be
    // guessed from its class name: that is the ARIA role, and it is worth
    // exactly the element it is the equivalent of.
    let name = element_a_landmark_role_stands_for(n).unwrap_or(name);
    if matches!(name, "header" | "footer") {
        return !n
            .ancestors()
            .any(|a| matches!(el_name(a), Some("article") | Some("section")));
    }
    if name == "aside" && is_a_metadata_panel(n) {
        return false;
    }
    // **A `<template>` carrying a `slot` is content, not a template.** An HTML
    // `<template>` is inert, hence its discard by name; but a Vue component's
    // (`<page><template slot="contents">`, `v-slot`, `#name`) is PROJECTED
    // into the component, which displays it. Wiki.js serves the whole body of
    // its pages this way: wiki.servarr.com returned 0 characters out of 10 KB
    // (2026-10-04, field survey of "domains without a reader").
    if name == "template" && is_a_component_slot(n) {
        return false;
    }
    // A checkbox is a STATE the page shows (a task list, a feature table's
    // `Done` column), not a form control to fill: dropped by name, a checked
    // and an unchecked box both came out as nothing (2026-10-04, after
    // html-to-markdown-rs `retain checkbox-only cells`).
    if name == "input" && is_a_checkbox(n) {
        return false;
    }
    is_dropped(name)
}

/// `<input type="checkbox">` (or `radio`): rendered `[x]` / `[ ]`.
pub(super) fn is_a_checkbox(n: NodeRef<Node>) -> bool {
    matches!(n.value(), Node::Element(e) if e.attr("type").is_some_and(|t| t.eq_ignore_ascii_case("checkbox") || t.eq_ignore_ascii_case("radio")))
}

/// The words by which a PACKAGE page names its facts.
///
/// A short and deliberately plain list: these are the labels registries
/// write, not words chosen to catch two pages.
pub(super) const PACKAGE_FACT_WORDS: &[&str] = &[
    "license",
    "licence",
    "source code",
    "repository",
    "bug tracker",
    "changelog",
    "maintainer",
    "owners",
    "homepage",
    "downloads",
    "dependencies",
];

/// Is this `<aside>` the page's FACT SHEET rather than its sidebar?
///
/// `<aside>` is discarded by name, along with `nav` and `form`, and that is
/// right for 20 of the corpus's 21 blocks. The twenty-first is the metadata
/// block of a package page — licence, repository, bug tracker — which
/// RubyGems, PyPI and pkg.go.dev thus lost entirely.
///
/// **Three SHAPE discriminants were measured and refuted before this one**:
/// "it carries label → value pairs" admits 26,211 characters including the
/// language menu and rejects both targets; "HTML5 carries it", the
/// refinement already used for `<header>`, concerns only 4 blocks, all
/// scenery; "it contains prose" wins `go_pkg` but admits 5,500 characters of
/// chrome and still rejects RubyGems. RubyGems' block is a LIST OF LINKS with
/// a fact inside: no shape property separates it from a navigation bar, only
/// the VOCABULARY does.
///
/// **A single condition, because a second was not justified.** The census on
/// the TEXT alone said a density was also needed — two words in 22,695
/// characters of language menu are two words that fell there by chance. But
/// the k8s menu is already discarded by its class, and **removing the density
/// changes NOTHING on the 43 pages**: measured, rendering by rendering. The
/// file says it twenty lines above — a guard no canary can fire is a
/// precaution that reassures about a different question — so this one goes,
/// and the measurement stays written here to bring it back the day a page
/// makes it fire.
pub(super) fn is_a_metadata_panel(n: NodeRef<Node>) -> bool {
    let text = subtree_text(n);
    let lower = text.to_lowercase();
    let hits = PACKAGE_FACT_WORDS
        .iter()
        .filter(|w| lower.contains(**w))
        .count();
    hits >= 2
}

/// **A region's ARIA role SAYS what it is.**
///
/// A navigation bar is written `<nav>`, or `<div role="navigation">` — the
/// ARIA specification sets the equivalence, and a page that writes it declares
/// it itself instead of leaving it to be guessed from a class word. The table
/// therefore contains NO new judgement: each role is paired with the element
/// `is_dropped` already discards, and a role without a discarded equivalent
/// (`main`, `article`, `region`) decides nothing.
///
/// The first token only: the attribute is a LIST of roles in order of
/// preference, and the first one counts.
pub(super) fn element_a_landmark_role_stands_for(n: NodeRef<Node>) -> Option<&'static str> {
    // **A TABLE is data, and the class-name scenery rule already refuses to
    // judge one.** The role inherits the same restraint: measured on the 39
    // pages, Wikipedia wraps its end-of-article navboxes in
    // `<div role="navigation">` around a `<table>`, and taking them cost
    // **16,082 bytes and thirteen points of structural fidelity** on that
    // page — the ratchet said so, and it was right: those tables name Rust's
    // compilers, libraries and environments. No other region of the corpus
    // declared as navigation carries a table.
    if n.descendants().any(|d| el_name(d) == Some("table")) {
        return None;
    }
    let role = attr(n, "role")?
        .split_whitespace()
        .next()?
        .to_ascii_lowercase();
    let implied = match role.as_str() {
        "navigation" => "nav",
        "banner" => "header",
        "contentinfo" => "footer",
        "complementary" => "aside",
        "search" | "form" => "form",
        _ => return None,
    };
    debug_assert!(is_dropped(implied), "{implied} is not in the dropped list");
    Some(implied)
}
