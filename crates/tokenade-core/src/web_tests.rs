// The tests of `crate::web`, moved out of the production file.
//
// Mounted by `include!`, not by `mod`: the tokens are spliced INTO `web`, so
// every `super::` keeps the meaning it had. Split by
// `srcsweep::production_lines`.

#[cfg(test)]
mod doc_generator_tests {
    use super::*;

    #[test]
    fn permalink_anchors_do_not_leak_into_headings() {
        // Sphinx, godoc, Docusaurus, MkDocs and rustdoc all hang a "link to
        // this heading" anchor off titles, and the glyph lands INSIDE the
        // heading text. Measured on the doc corpus: 72 reached pkg.go.dev's
        // output, 16 docs.python.org's.
        let sphinx = r##"<html><body><main><h1>Coroutines and tasks<a class="headerlink" href="#coroutines-and-tasks" title="Link to this heading">¶</a></h1><p>This section outlines the high-level asyncio APIs used to work with coroutines and tasks, with enough prose to clear the extractor floor.</p></main></body></html>"##;
        let out = html_to_markdown(sphinx);
        assert!(!out.contains('¶'), "permalink glyph leaked: {out}");
        assert!(out.contains("Coroutines and tasks"));

        let godoc = r##"<html><body><main><h2>Documentation<a class="UnitDoc-idLink" href="#section-documentation" aria-label="Go to Documentation">¶</a></h2><p>Package http provides HTTP client and server implementations, with enough text to be treated as a real body by the extraction ladder.</p></main></body></html>"##;
        assert!(!html_to_markdown(godoc).contains('¶'));
    }

    #[test]
    fn an_alphanumeric_one_character_link_survives() {
        // The rule keys on "fragment link whose whole text is punctuation",
        // never on length — a one-character label can be content.
        let html = r##"<html><body><main><p>Jump to section <a href="#s1">1</a> or read the paragraph that follows, which exists only to give the extractor enough material to keep.</p></main></body></html>"##;
        assert!(html_to_markdown(html).contains('1'));
    }

    #[test]
    fn generator_prefixed_sidebars_are_stripped() {
        // `\bsidebar\b` does not match `sphinxsidebar` — no word boundary
        // between `x` and `s`. On docs.python.org/3/glossary the surviving
        // sidebar became the page's ENTIRE outline ("Previous topic / Next
        // topic / This page") with the glossary itself absent from it.
        let html = r##"<html><body><div class="sphinxsidebar"><h4>Previous topic</h4><p>Deprecations</p><h4>Next topic</h4><p>About this documentation</p></div><main><h1>Glossary</h1><p>The default Python prompt of the interactive shell, often seen for code examples which can be executed interactively in the interpreter. It also appears when entering the code for an indented code block, when within a pair of matching left and right delimiters, or when a decorator is being applied to a function or class definition.</p></main></body></html>"##;
        let out = html_to_markdown(html);
        assert!(
            !out.contains("Previous topic"),
            "prefixed sidebar survived: {out}"
        );
        assert!(out.contains("Glossary"));
    }

    #[test]
    fn a_shell_main_does_not_disable_boilerplate_stripping() {
        // miro.com isolates 298 471 bytes down to 409: `<main>` is a
        // client-rendered shell. The aggressive tier then produced ZERO
        // characters, the ladder fell back to the relaxed pass — which does no
        // boilerplate stripping at all — and the page shipped unpruned. 22 of
        // 175 corpus pages were taking that path.
        let mut html = String::from("<html><body><main><div id=\"root\"></div></main><div class=\"promo-banner\"><p>Sign up for 10 percent off today.</p></div>");
        for i in 0..14 {
            html.push_str(&format!(
                "<p>Real body paragraph {i} living OUTSIDE the shell main, which is where the actual content of such pages is.</p>"
            ));
        }
        html.push_str("</body></html>");
        let out = html_to_markdown(&html);
        assert!(out.contains("Real body paragraph 0"), "content lost: {out}");
        assert!(
            !out.contains("10 percent off"),
            "shell <main> disabled boilerplate stripping: {out}"
        );
    }

    #[test]
    fn a_believable_main_is_still_honoured() {
        // The guard is a ratio test, not a retreat from isolation: a real
        // `<main>` must still win over the surrounding chrome.
        let mut html = String::from("<html><body><nav>Home Products Support</nav><main>");
        for i in 0..14 {
            html.push_str(&format!(
                "<p>Article paragraph {i} carrying the page's actual editorial content.</p>"
            ));
        }
        html.push_str("</main><footer>Legal notices and copyright</footer></body></html>");
        let out = html_to_markdown(&html);
        assert!(out.contains("Article paragraph 0"));
        assert!(!out.contains("Legal notices"), "footer survived: {out}");
    }

    #[test]
    fn a_layout_container_is_not_boilerplate() {
        // newegg.com wraps its whole catalogue in
        // `<div class="row has-side-banner">` — `banner` matched the layout
        // MODIFIER, and the cut took 109 926 of 192 707 bytes: the product
        // grid and every facet. The aggressive tier then had too little left
        // to win, the ladder fell back to the relaxed pass, and the filter
        // panel became the page's entire outline. beds.co.uk lost its
        // catalogue the same way — 224 lines of products with prices.
        let mut html = String::from(
            "<html><body><div class=\"row has-side-banner\"><h1>Network Interface Cards</h1>",
        );
        for i in 0..14 {
            html.push_str(&format!(
                "<p>Product {i}: a 10-gigabit network adapter with a single RJ-45 port, priced competitively and stocked for immediate dispatch.</p>"
            ));
        }
        html.push_str("</div></body></html>");
        let out = html_to_markdown(&html);
        assert!(
            out.contains("Product 0"),
            "catalogue removed as boilerplate: {out}"
        );
        assert!(out.contains("Product 13"));
    }

    #[test]
    fn a_genuine_banner_is_still_removed() {
        // The guard is a SIZE test, not an amnesty: peripheral boilerplate
        // must still go.
        let mut html = String::from("<html><body><main><div class=\"promo-banner\"><p>Sign up for 10% off your first order today.</p></div>");
        for i in 0..14 {
            html.push_str(&format!(
                "<p>Article paragraph {i} carrying the real editorial content of this page, long enough to dominate the document.</p>"
            ));
        }
        html.push_str("</main></body></html>");
        let out = html_to_markdown(&html);
        assert!(
            !out.contains("10% off"),
            "peripheral banner survived: {out}"
        );
        assert!(out.contains("Article paragraph 0"));
    }

    #[test]
    fn commented_out_markup_is_not_content() {
        // Real page, real intent: 77neko5.com ships
        // `<!-- TEMPORARY HIDE THE FAQ SECTION -->` followed by the commented
        // FAQ. Nothing stripped comments, so `strip_all_tags` ended the
        // `<!--` "tag" at the first `>` and everything after it arrived as
        // live content — with a bare ` -->` line left behind.
        let html = r##"<html><body><main><h1>Product page</h1><p>Real body copy, long enough that the extraction ladder treats this as a genuine article rather than a failed isolation.</p><!-- TEMPORARY HIDE THE OLD CAMPAIGN --><!-- <div class="legacy"><h2>OLD CAMPAIGN DO NOT SHIP</h2><p>Buy now for $9.99, this offer expired in 2019.</p></div> --><p>More real copy after the hidden block.</p></main></body></html>"##;
        let out = html_to_markdown(html);
        assert!(
            !out.contains("OLD CAMPAIGN"),
            "hidden markup served as content: {out}"
        );
        assert!(
            !out.contains("expired in 2019"),
            "hidden copy served as content"
        );
        assert!(!out.contains("-->"), "comment terminator leaked: {out}");
        assert!(out.contains("Real body copy"));
        assert!(out.contains("More real copy"));
    }

    #[test]
    fn a_comment_opener_inside_a_script_does_not_eat_the_page() {
        // Comments are stripped AFTER script bodies: inside a script a browser
        // never opens a comment, so `var s = "<!--";` (and the legacy
        // `<script><!-- … //--></script>` idiom) must not swallow the rest.
        let html = r##"<html><body><main><h1>Title</h1><script>var marker = "<!--";</script><p>This paragraph must survive a JS string containing a comment opener, and it is long enough to clear the extraction floor comfortably.</p></main></body></html>"##;
        assert!(html_to_markdown(html).contains("must survive a JS string"));
    }

    #[test]
    fn butted_link_runs_become_one_per_line() {
        // standard.co.uk's business section arrived as a SINGLE
        // 10 357-character line: every headline present, none readable, and
        // useless as a RAG chunk or an outline preview. Listing cards are
        // built from inline `<a>`, so nothing emitted a block break.
        let mut html = String::from("<html><body><main><h2>Business News</h2>");
        for i in 0..8 {
            html.push_str(&format!(
                "<span><a href=\"/section\">Business News</a><a href=\"/story{i}\">Headline number {i} about markets and the economy</a></span>"
            ));
        }
        html.push_str("</main></body></html>");
        let out = html_to_markdown(&html);
        assert!(
            out.lines().all(|l| l.len() < 400),
            "listing still arrives as a wall: longest {}",
            out.lines().map(|l| l.len()).max().unwrap_or(0)
        );
        for i in 0..8 {
            assert!(
                out.contains(&format!("Headline number {i}")),
                "headline {i} lost"
            );
        }
    }

    #[test]
    fn links_separated_by_prose_stay_inline() {
        // The rule fires on `)[` adjacency only. Inline references inside a
        // sentence must not be broken apart.
        let html = r##"<html><body><main><p>See <a href="/a">the first guide</a> and then <a href="/b">the second guide</a>, or read <a href="/c">the third guide</a> before you continue with the rest of this paragraph.</p></main></body></html>"##;
        let out = html_to_markdown(html);
        assert!(out.contains("and then"), "prose lost");
        assert!(
            out.lines().any(|l| l.matches("](").count() >= 3),
            "prose links were split apart: {out}"
        );
    }

    #[test]
    fn a_thin_result_says_so_without_claiming_a_block() {
        // onwardvet.com (CrUX rank 100 000) delivered exactly `[image: Logo]`
        // — 13 characters from 8 KB — with no explanation. The note states
        // the MEASURED fact; it must not assert an empty page or a bot wall,
        // because a 404 and a parked domain reach the same branch.
        // It carries semantic elements (`<a>`, `<p>`), so the structural
        // two-of-three signal does NOT fire — this is the case that used to
        // fall through with no explanation at all.
        let mut shell = String::from("<html><head><script src=\"/app.js\"></script></head><body><div id=\"root\"><img src=\"/logo.png\" alt=\"Logo\"><a href=\"/\"></a><p></p></div>");
        shell.push_str(&"<!-- padding to clear the input floor -->".repeat(60));
        shell.push_str("</body></html>");
        let out = html_to_markdown(&shell);
        assert!(out.contains("[tokenade: thin-result]"), "no note: {out}");
        assert!(
            out.contains("rendered"),
            "note should name client-side rendering"
        );
        assert!(
            !out.contains("no body text"),
            "must not claim an emptiness it did not observe"
        );
        assert!(
            out.contains("Logo"),
            "the little that was recovered must survive"
        );
    }

    #[test]
    fn site_metadata_does_not_mask_an_empty_page() {
        // crates.io/crates/serde carries NO crate data in its HTML — 5 KB in
        // which the string "serde" appears once, inside an image URL. The
        // og:title/og:description are identical on every URL of the domain,
        // and prepending them pushed the body over the thin-result floor, so
        // the page was served as `# crates.io: Rust Package Registry` with no
        // hint that its own content was absent. The floor now measures what
        // the PAGE yielded, before site-level metadata is added.
        let mut html = String::from(
            r##"<html><head><meta property="og:title" content="crates.io: Rust Package Registry"><meta property="og:description" content="crates.io serves as a central registry for sharing crates, which are packages or libraries written in Rust that you can use to enhance your projects"><script src="/app.js"></script></head><body><div id="ember-app"><a href="/">home</a><p></p></div>"##,
        );
        html.push_str(&"<!-- app shell -->".repeat(120));
        html.push_str("</body></html>");
        let out = html_to_markdown(&html);
        assert!(
            out.contains("[tokenade: thin-result]"),
            "empty SPA page not flagged: {out}"
        );
        // The metadata is still served — it is the only thing there.
        assert!(out.contains("Rust Package Registry"));
    }

    #[test]
    fn a_normal_page_gets_no_thin_result_note() {
        let mut page = String::from(
            "<html><head><script src=\"/a.js\"></script></head><body><main><h1>Ownership</h1>",
        );
        for i in 0..12 {
            page.push_str(&format!("<p>Paragraph {i} of ordinary article prose, long enough that the extraction ladder treats it as a real body rather than a failed isolation.</p>"));
        }
        page.push_str("</main></body></html>");
        assert!(!html_to_markdown(&page).contains("thin-result"));
    }

    #[test]
    fn definition_terms_become_headings() {
        // A glossary / API reference is a `<dl>` with NO headings at all, so
        // the outline folder saw one flat section: docs.python.org/3/glossary
        // was served as `# Glossary (1894 lines)` plus a stash pointer, with
        // all 176 terms behind a second call. `<dt>` is promoted because the
        // TAG says it is a term being defined.
        let html = r##"<html><body><main><h1>Glossary</h1><dl><dt>abstract base class</dt><dd>Abstract base classes complement duck-typing by providing a way to define interfaces when other techniques would be clumsy or subtly wrong.</dd><dt>annotation</dt><dd>A label associated with a variable, a class attribute or a function parameter, used by convention as a type hint.</dd></dl></main></body></html>"##;
        let out = html_to_markdown(html);
        assert!(
            out.contains("#### abstract base class"),
            "term not promoted: {out}"
        );
        assert!(out.contains("#### annotation"));
        assert!(
            out.contains("complement duck-typing"),
            "definition body lost"
        );
    }

    #[test]
    fn short_negative_keywords_keep_both_boundaries() {
        // `ad`/`ads`/`nav` as bare substrings would swallow `download`,
        // `navigable`, `header-adjacent`. Only the long unambiguous words get
        // the relaxed prefix.
        let html = r##"<html><body><main><div class="download-panel"><p>Download the installer for your platform and follow the setup instructions provided in the guide below this paragraph.</p></div></main></body></html>"##;
        assert!(html_to_markdown(html).contains("Download the installer"));
    }
}
#[cfg(test)]
mod tests {

    /// Story grids built from `<div>` rather than `<article>` are
    /// invisible to the sibling rule. latercera keeps 56 of its 147 headings
    /// inside `<main>` and puts the rest in plain divs, so it lost 64 of 120
    /// headings. A `<main>` holding a MINORITY of the page's headings is one
    /// panel of a grid, not the content.
    #[test]
    fn a_main_holding_a_minority_of_headings_is_a_section() {
        let filler = "prose with commas, clauses and plenty of plain words so that no \
tier calls the page empty and the strip guards have room to do their work";
        let cards = (1..=9)
            .map(|i| {
                format!(
                    "<div class=\"card\"><h3>Story number {i} headline</h3><p>{filler}</p></div>"
                )
            })
            .collect::<String>();
        let doc = format!(
            "<html><body><main><h2>Featured</h2><p>{filler}</p></main>{cards}</body></html>"
        );
        let md = super::html_to_markdown(&doc);
        assert!(md.contains("Featured"), "isolated region kept: {md}");
        assert!(
            md.contains("Story number 9 headline"),
            "div cards kept: {md}"
        );

        // A page whose <main> holds MOST of its headings is left alone — this
        // must not become a licence to keep the whole document everywhere.
        let inside = (1..=9)
            .map(|i| format!("<h3>Inside heading {i}</h3><p>{filler}</p>"))
            .collect::<String>();
        let normal =
            format!("<html><body><nav><h3>Nav one</h3></nav><main>{inside}</main></body></html>");
        assert!(
            !super::html_to_markdown(&normal).contains("Nav one"),
            "isolation still trusted when main holds the headings"
        );
    }

    /// `<main>` is the page's content on an ARTICLE page; on a FRONT
    /// page it is often one section of several. aljazeera.com wraps its
    /// featured block in `<main id="featured-news-container">` and puts every
    /// story card after it, so isolating discarded the page below the fold:
    /// 394 KB in, 5 870 B out, stopping at 17 %.
    #[test]
    fn story_cards_outside_main_are_not_discarded() {
        let filler = "prose with commas, clauses and plenty of plain words so that no \
tier is tempted to call this page empty and the strip guards have room to work";
        let doc = format!(
            "<html><body>\
<main id=\"featured-news-container\"><article><h2>The featured story</h2><p>{filler}</p></article></main>\
<article><h3>Somalia races to save Radio Mogadishu</h3>\
<p>A small team is working to digitise decades of Somali history, {filler}</p></article>\
<article><h3>A second card below the fold</h3><p>{filler}</p></article>\
</body></html>"
        );
        let md = super::html_to_markdown(&doc);
        assert!(
            md.contains("The featured story"),
            "isolated region kept: {md}"
        );
        assert!(
            md.contains("digitise decades of Somali history"),
            "sibling card kept: {md}"
        );
        assert!(
            md.contains("A second card below the fold"),
            "all siblings kept: {md}"
        );

        // An <article> that WRAPS <main> is not a sibling — appending it would
        // duplicate the page (docs.stripe.com; an earlier version doubled its body).
        let wrapped = format!(
            "<html><body><article><main><p>The only body text, {filler}</p></main></article></body></html>"
        );
        let md2 = super::html_to_markdown(&wrapped);
        assert_eq!(
            md2.matches("The only body text").count(),
            1,
            "not duplicated: {md2}"
        );
    }

    /// A class that TURNS AN AD OFF is not an ad. vogue.com wraps
    /// its story grid in `hide-read-more-ad`; `ad` matched the suffix and
    /// 20 970 bytes of headlines were deleted as advertising, which is why the
    /// page scored 51 % on headings while scoring fine on paragraphs.
    #[test]
    fn a_class_that_suppresses_ads_is_not_an_ad() {
        let filler = "<p>Prose with commas, clauses and plain words, long enough that the \
aggressive tier keeps the document and the cumulative strip guard does not refuse \
the whole strip on a fixture that is too small to judge.</p>";
        let page = |cls: &str| {
            format!(
                "<html><body><main><div class=\"{cls}\"><h3>Zendaya and Tom Holland \
Embrace Husband-Wife Style</h3></div><article>{filler}</article></main></body></html>"
            )
        };
        for negated in [
            "SummaryCollectionGridItems-x hide-read-more-ad",
            "content ad-free",
            "grid no-ads",
        ] {
            let md = super::html_to_markdown(&page(negated));
            assert!(
                md.contains("Husband-Wife Style"),
                "`{negated}` negates the keyword: {md}"
            );
        }
        // A real ad slot still goes.
        for real in ["ad-slot", "js-ad-slot", "ft-ad--sticky"] {
            let md = super::html_to_markdown(&page(real));
            assert!(
                !md.contains("Husband-Wife Style"),
                "`{real}` is a real ad slot: {md}"
            );
        }
    }

    /// Sphinx names every section `id="s-<slugified heading>"`, so a
    /// heading that mentions an API concept collides with the boilerplate
    /// keyword list. django's `related_name` documentation lived in
    /// `<section id="s-be-careful-with-related-name-and-related-query-name">`
    /// and was deleted whole as "related content".
    #[test]
    fn a_sphinx_section_id_is_never_boilerplate() {
        let body = "<p>The page needs enough prose here, with commas, clauses and plain \
words, that the aggressive tier is happy to keep the document and the cumulative \
strip guard does not refuse the whole strip on a fixture that is too small.</p>";
        // The word is `widget` and no longer `related`: `related` no longer
        // decides anything, the ARIA role does. The collision this test
        // guards is the same — an API heading slipped into a Sphinx id — and
        // it holds for any word in the list.
        let doc = format!(
            "<html><body><main><section id=\"s-be-careful-with-widget-name\">\
<p>'%(class)s' is replaced by the lowercased name of the child class.</p></section>\
<article>{body}</article></main></body></html>"
        );
        let md = super::html_to_markdown(&doc);
        assert!(
            md.contains("lowercased name of the child class"),
            "section kept: {md}"
        );

        // A real widget block, same keyword, no Sphinx id — still goes.
        let nav = format!(
            "<html><body><main><section class=\"widget-posts\">\
<p>'%(class)s' is replaced by the lowercased name of the child class.</p></section>\
<article>{body}</article></main></body></html>"
        );
        assert!(
            !super::html_to_markdown(&nav).contains("lowercased name of the child class"),
            "a real widget block still goes"
        );
    }

    /// `promo` is a semantic CSS class when an author writes it and a
    /// COMPONENT NAME when a CSS-in-JS generator does. The BBC's World front
    /// builds every story card as `ssrcss-1ny4qi9-Promo` /
    /// `ssrcss-1emzoi7-PromoHeadline`, so twenty-seven headlines — the page —
    /// were stripped as advertising. Matching it case-sensitively separates the
    /// two: `sim-promo__header` (investopedia) and `app-promo` (gov.uk) are
    /// real promo boxes and still go.
    #[test]
    fn promo_names_a_component_and_never_an_advert() {
        let page = |cls: &str| {
            format!(
                "<html><body><main><div class=\"{cls}\"><p>Families demand answers as \
Nigeria's boat disasters keep happening, a story the page exists to tell.</p></div>\
<article><p>Some other prose on the page, long enough with commas, clauses and \
plain words that the aggressive tier is happy to keep the document. It has to be \
substantially longer than the block being stripped, or the cumulative guard \
(MIN_KEPT_AFTER_STRIP_PERCENT) rejects the whole strip and the fixture tests \
nothing at all — which is how the first version of this test failed against \
correct behaviour, for the seventh time in this loop.</p></article>\
</main></body></html>"
            )
        };
        let bbc = super::html_to_markdown(&page("ssrcss-1ny4qi9-Promo e1vyq2e80"));
        assert!(
            bbc.contains("boat disasters"),
            "component name is not an advert: {bbc}"
        );

        // **And `app-promo` is no longer discarded either.** This half of the
        // test claimed the opposite — "a real ad box still goes" — and the
        // measurement contradicted it on the very page it cited: `gov_uk_home`
        // carries `app-promo`, but ALSO `homepage-section--promotion-slots`,
        // where its "Featured" section lives ("Find a job", "National
        // Insurance"). "promotion" contains "promo", so the substring rule
        // discarded the editorial content of a government home page. And on
        // `slack.com`, the whole page body is built from `c-promo__*`: 4,059
        // bytes, eight paragraphs out of nine. No length and no heading count
        // separates these blocks from a real ad; the word is removed, and ads
        // are still caught by `sponsor`, `advert`, `subscribe`, `signup`.
        let promo_box = super::html_to_markdown(&page("app-promo"));
        assert!(
            promo_box.contains("boat disasters"),
            "a component named `promo` is still taken for an ad: {promo_box}"
        );
        let advert = super::html_to_markdown(&page("sponsor-unit"));
        assert!(
            !advert.contains("boat disasters"),
            "a real ad gets through: {advert}"
        );
    }

    /// A `<li>` holding only an icon or an empty anchor converted to
    /// a bare `-`. 789 such lines out of 17 683 across the corpus: one line in
    /// twenty-two of everything we emit, saying nothing.
    #[test]
    fn list_items_with_nothing_in_them_are_dropped() {
        let md = super::html_to_markdown(
            "<html><body><article><ul>\
<li><a href=\"/a\"><img src=\"icon.png\"></a></li>\
<li>A real item with words in it</li>\
<li><img src=\"pixel.gif\"></li>\
</ul><p>Body prose long enough to clear the validity floor, with commas, several \
clauses, and plenty of plain words so no tier calls this page empty.</p></article></body></html>",
        );
        assert!(
            md.contains("A real item with words in it"),
            "real item kept: {md}"
        );
        assert!(
            !md.lines().any(|l| l.trim() == "-" || l.trim() == "*"),
            "no bare markers left: {md}"
        );
    }

    /// A heading over a list of links, joined to it by a line of spaces, is
    /// still a list block: Wikipedia's `External links` lost both its links.
    #[test]
    fn a_heading_does_not_turn_its_link_list_into_navigation() {
        let md = "## External links\n \n\
                  - [Gutenprint official website](http://gutenprint.sf.net/)\n\
                  - [Gutenprint official SourceForge.net project page](http://sf.net/projects/gimp-print/)\n ";
        let out = super::prune_link_dense_blocks(md);
        assert!(out.contains("Gutenprint official website"), "{out}");
        // The form the real page takes: the heading hoisted away, MediaWiki's
        // `[[edit]]` glued on instead.
        let md = "[[edit](/w/index.php?title=G&action=edit&section=6)]\n \n\
                  - [Gutenprint official website](http://gutenprint.sf.net/)\n\
                  - [Gutenprint official SourceForge.net project page](http://sf.net/projects/gimp-print/)\n ";
        let out = super::prune_link_dense_blocks(md);
        assert!(out.contains("Gutenprint official website"), "{out}");
    }

    #[test]
    fn a_sentence_with_two_links_is_not_navigation() {
        let prose = "Throughout this reference we'll use the \
[example blog models](../../topics/db/queries/#queryset-model-example) presented in the \
[database query guide](../../topics/db/queries/).";
        let kept = super::prune_link_dense_blocks(prose);
        assert!(
            kept.contains("Throughout this reference"),
            "prose kept: {kept}"
        );

        // Real navigation puts nothing but separators between its links, and
        // must still go.
        let nav = "[Home](/) · [About](/about) · [Jobs](/jobs) · [Press](/press)";
        assert_eq!(super::prune_link_dense_blocks(nav), "", "nav still pruned");
    }

    /// A page with no `og:` tags lost its NAME. The header only ever
    /// consulted `og:title`, so `stackoverflow.blog` articles arrived with
    /// their text intact and nothing saying what they were. 49 of the 147
    /// corpus pages were affected, documentation leaves among them.
    #[test]
    fn title_falls_back_to_the_title_element_without_repeating_the_h1() {
        let body = "<p>Body prose long enough to clear the validity floor, with commas, \
several clauses, and enough plain words that no tier is tempted to call it empty.</p>";

        // No og: tags at all — the <title> element must supply the name.
        let md = super::html_to_markdown(&format!(
            "<html><head><title>The future of development is full-stack - Stack Overflow</title>\
<meta name=\"description\" content=\"Founded in 2008, Stack Overflow.\"></head>\
<body><article>{body}</article></body></html>"
        ));
        assert!(
            md.starts_with("# The future of development is full-stack"),
            "{md}"
        );
        assert!(md.contains("> Founded in 2008"), "description kept: {md}");

        // The <title> carries a site suffix the <h1> does not. Exact
        // containment missed that and 13 pages grew a duplicate heading.
        let dup = super::html_to_markdown(&format!(
            "<html><head><title>First Steps - FastAPI</title>\
<meta name=\"description\" content=\"A tutorial description sentence.\"></head>\
<body><article><h1>First Steps</h1>{body}</article></body></html>"
        ));
        assert_eq!(
            dup.matches("First Steps").count(),
            1,
            "title said once: {dup}"
        );
        // Suppressing the repeated TITLE must not take the description with
        // it — that was all-or-nothing and cost 689 words across the corpus.
        assert!(
            dup.contains("> A tutorial description sentence."),
            "description kept: {dup}"
        );
    }

    /// A newline in HTML is whitespace, not a line break, but the
    /// pipeline carried it through: wherever a generator wrapped its markup we
    /// emitted a hard break mid-sentence. mandoc (OpenBSD/FreeBSD/illumos
    /// manuals) wraps every NAME line, so `man.openbsd.org/ssh` previewed as
    /// `` `ssh` — `` — the one line that says what the command is, cut in half.
    #[test]
    fn wrapped_source_markup_does_not_become_a_line_break() {
        let md = super::html_to_markdown(
            "<html><body><article><h2>NAME</h2>\n<p><code>ssh</code> &#x2014;\n    <span>OpenSSH remote login client</span></p>\n<p>A paragraph that the generator\nwrapped at some column, as generators do, with commas and enough words that the \naggressive tier is happy to keep it.</p></article></body></html>",
        );
        assert!(
            md.contains("`ssh` — OpenSSH remote login client"),
            "one sentence, one line: {md}"
        );
        assert!(
            md.contains("that the generator wrapped at some column"),
            "wrapped prose rejoined: {md}"
        );

        // `<pre>` is where newlines ARE the content, and plain text has no tags
        // to match — a blanket collapse would have flattened every RFC and
        // Gutenberg text the outline just learned to read.
        let code = super::html_to_markdown(
            "<html><body><article><pre>line one\nline two\nline three</pre>\
<p>Prose after the block, long enough to clear the floor, with commas, and more \
words besides so nothing here is borderline.</p></article></body></html>",
        );
        assert!(
            code.contains("line one\nline two"),
            "pre keeps its newlines: {code}"
        );
    }

    /// News and marketing sites make the whole card a link with the
    /// `<h2>` INSIDE it, so the pipeline emitted `[## Headline](/url)`: not a
    /// heading in any parser, including our own outline. 341 occurrences across
    /// 22 of the 147 corpus pages.
    #[test]
    fn a_heading_inside_a_link_becomes_a_heading() {
        let md = super::html_to_markdown(
            "<html><body><article><a href=\"/story\"><h2>New PM pledges to bring back hope</h2></a>\
<p>Some body prose long enough to matter, with commas, and more words after it so the \
extraction tier is not tempted to call this page empty at all.</p></article></body></html>",
        );
        assert!(
            md.contains("## [New PM pledges to bring back hope](/story)"),
            "heading hoisted out of the link: {md}"
        );
        assert!(!md.contains("[## "), "no marker left inside a label: {md}");
    }

    /// A label split across lines is not a link at all. Teaching `</h2>`
    /// to emit a break, and inside an anchor that produced the fragment
    /// `[## Trump slaps 50% tariffs on Canada…` with no closing bracket — 225
    /// of them on the corpus.
    #[test]
    fn link_labels_never_span_lines() {
        let md = super::html_to_markdown(
            "<html><body><article><a href=\"/x\"><h3>Title one</h3><span>tail</span></a>\
<p>Body prose that is long enough to clear the validity floor for the aggressive \
extraction tier, with several commas, and a good number of plain words.</p></article></body></html>",
        );
        let label_line = md.lines().find(|l| l.contains("Title one")).unwrap_or("");
        assert!(
            label_line.contains("](/x)"),
            "label and url on one line: {md}"
        );
    }

    /// Block separation used to come from whatever whitespace the
    /// author left between tags, i.e. from nothing on a minified page.
    /// `news.ycombinator.com/item?id=1` is one line of HTML and its thread
    /// arrived as `…eating lunch on SHR[pg](user?id=pg)…`: the end of one
    /// comment welded to the byline of the next.
    #[test]
    fn minified_block_elements_still_separate() {
        let html = "<html><body><div><div>First comment body ends here.</div>\
<div><a href=\"user?id=pg\">pg</a> on Oct 9</div></div>\
<table><tr><td>row one cell</td></tr><tr><td>row two cell</td></tr></table></body></html>";
        let md = super::html_to_markdown(html);
        assert!(!md.contains("here.[pg]"), "comments must not weld: {md}");
        assert!(md.contains("First comment body ends here."), "{md}");
        assert!(md.contains("pg"), "{md}");
        // Inline elements must keep flowing.
        let inline = super::html_to_markdown(
            "<html><body><p>a <strong>bold</strong> and <span>spanned</span> sentence</p></body></html>",
        );
        assert!(inline.contains("a bold and spanned sentence"), "{inline}");
    }

    /// Both link pruners deleted newspaper front pages once the
    /// block-break fix put each headline on its own line. Label length is what
    /// separates a headline list from a nav sidebar.
    #[test]
    fn headline_lists_survive_the_link_pruners_and_nav_does_not() {
        use super::link_labels_read_like_content;

        let headlines =
            "- [Slowdown in Arctic ice melt ends after largest winter decline on record](/a)\n\
- [Man, 28, charged with murder of former MP Ann Widdecombe](/b)\n\
- [Reeves reacts as Burnham sacks first-ever female Chancellor](/c)";
        assert!(
            link_labels_read_like_content(headlines),
            "headlines are content"
        );

        let nav = "- [Cache](/x)\n- [UIEvent](/y)\n- [GainNode](/z)\n- [1000Mbps](/w)";
        assert!(!link_labels_read_like_content(nav), "nav is not content");

        // Both pruners must agree with that verdict.
        let long_nav = (0..8)
            .map(|i| format!("- [Cache{i}](/x{i})"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(super::prune_link_list_runs(&long_nav), "", "nav run pruned");
        let long_heads = (0..8)
            .map(|i| {
                format!(
                    "- [Slowdown in Arctic ice melt ends after winter decline number {i}](/a{i})"
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            super::prune_link_list_runs(&long_heads),
            long_heads,
            "headlines kept"
        );
        assert!(
            super::prune_link_dense_blocks(&long_heads.replace("- ", "")).contains("Arctic"),
            "link-dense headline block kept"
        );
    }

    /// man7.org appends a `top` self-link to every section heading,
    /// so `openat.2.html` outlined as `NAME top`, `SYNOPSIS top`, `ERRORS top`.
    /// The guard is the other half: MDN's anchor IS the heading text, and the
    /// first cut of the stripper emptied every heading on every MDN page.
    #[test]
    fn nav_anchors_leave_headings_that_are_only_an_anchor_alone() {
        use super::strip_permalink_anchors;

        let man7 = r##"<h2><a id="NAME" href="#NAME"></a>NAME &nbsp; <a href="#top_of_page"><span class="top-link">top</span></a></h2>"##;
        let out = strip_permalink_anchors(man7);
        assert!(out.contains("NAME"), "heading text must survive: {out}");
        assert!(!out.contains("top-link"), "nav widget must go: {out}");
        assert!(!out.contains(">top<"), "the word must go with it: {out}");

        // MDN: removing the anchor would leave nothing, so nothing is removed.
        let mdn = r##"<h2 id="x" class="heading"><a class="headerlink" href="#x">Fetch インターフェイス</a></h2>"##;
        assert_eq!(
            strip_permalink_anchors(mdn),
            mdn,
            "must not empty a heading"
        );

        // Sphinx `¶` in a heading, and an ordinary in-page link in prose.
        let sphinx = r##"<h1>QuerySet API<a class="headerlink" href="#q">¶</a></h1>"##;
        let stripped = strip_permalink_anchors(sphinx);
        assert!(stripped.contains("QuerySet API") && !stripped.contains("headerlink"));
        let prose = r##"<p>see <a href="#errors">the error table</a> below</p>"##;
        assert_eq!(
            strip_permalink_anchors(prose),
            prose,
            "body links untouched"
        );
    }

    /// Corpus-driven: every row is a page actually fetched and distilled
    /// (see the 14-page probe). `prose` is its measured prose-word count.
    /// Corpus-driven: every number below is a real page fetched and
    /// distilled by the 70-site probe. Keep the two classes non-overlapping.
    #[test]
    fn spa_shell_separates_real_shells_from_content_heavy_sites() {
        /// `n` tokens of filler — only the VOLUME is under test.
        fn body(md_tokens: usize) -> String {
            "mot ".repeat(md_tokens * 4)
        }

        // Banner is decisive whatever the volume — Notion's 18 KB shell.
        assert!(looks_like_spa_shell(
            "Notion\n\nJavaScript must be enabled in order to use Notion.",
            4_574,
            160
        ));
        // Real shells, measured distilled tokens. discord (208) is the worst.
        for (label, md, html) in [
            ("discord", 208, 11_068),
            ("linkedin", 174, 139_226),
            ("clickup", 151, 28_879),
            ("slack", 136, 9_170),
            ("trello", 119, 32_124),
            ("gmail", 100, 307_130),
            ("dropbox", 45, 19_075),
            ("linear", 4, 5_938),
            ("instagram", 2, 149_098),
        ] {
            assert!(
                looks_like_spa_shell(&body(md), html, md),
                "{label} should fire"
            );
        }
        // Real documents. The two that defeat the intuitive metrics:
        //   * bbc yields 2.72 %, BELOW Notion's 3.50 % — a ratio rule inverts.
        //   * lobsters/asahi are listings: 10 and 16 prose words, yet 2.4k and
        //     9.1k tokens of genuine headline-and-link content.
        for (label, md, html) in [
            ("ghissue", 601, 71_409),
            ("cloudflare", 759, 97_580),
            ("lobsters", 2_382, 14_977),
            ("bbc", 2_693, 99_160),
            ("asahi", 9_102, 39_307),
            ("substack", 2_111, 41_839),
            ("stackoverflow", 31_056, 200_000),
        ] {
            assert!(
                !looks_like_spa_shell(&body(md), html, md),
                "{label} must not fire"
            );
        }
        // Small pages are small because they ARE small: example.com, and any
        // bot-blocked stub, sit under the HTML guard and never fire.
        assert!(!looks_like_spa_shell(&body(46), 139, 46));
        assert!(!looks_like_spa_shell(&body(84), 947, 84));
    }
    use super::*;

    #[test]
    fn negative_class_blocks_and_link_dense_pruned() {
        let html = r##"<html><body><main>
<div class="article-body"><p>Real content here, with commas, plenty of prose, and substance that matters.
Enough of it, too: this paragraph exists so the page clears EXTRACT_VALID_MIN_CHARS and the
AGGRESSIVE tier is the one under test. Round 17 found this fixture below the floor, which meant
it had been asserting about the relaxed tier — the one that does no boilerplate stripping at all —
for as long as it had existed.</p></div>
<div class="social-share"><a href="/tw">Tweet</a> <a href="/fb">Share</a> <a href="/li">Post</a></div>
<div class="related-posts"><ul><li><a href="/a">Other article one</a></li><li><a href="/b">Other article two</a></li></ul></div>
<div class="related-posts" role="navigation"><ul><li><a href="/c">Declared navigation link</a></li></ul></div>
<p><a href="#content">Skip to content</a></p>
</main></body></html>"##;
        let md = html_to_markdown(html);
        assert!(md.contains("Real content here"), "prose survives: {md}");
        assert!(!md.contains("Tweet"), "share cluster dropped: {md}");
        // **`related` is no longer a scenery word.** It did catch Sphinx's
        // bar, and with it the front-page headlines of three news sites,
        // whose cards are called `story-card__related-stories`. What decides
        // now is what the page DECLARES: the second block above says
        // `role="navigation"` and goes; the first stays.
        assert!(md.contains("Other article one"), "related kept: {md}");
        assert!(
            !md.contains("Declared navigation link"),
            "declared navigation dropped: {md}"
        );
        assert!(
            !md.to_lowercase().contains("skip to content"),
            "skip link dropped: {md}"
        );
    }

    #[test]
    fn skeletal_main_falls_back_to_relaxed_pass() {
        // SPA shell: <main> is an empty mount point, the actual server-side
        // content sits in a sibling div. Aggressive isolation yields ~nothing
        // → relaxed pass recovers the prose.
        let prose = "Long-form article prose, with commas, that clearly matters. ".repeat(8);
        let html = format!(
            r##"<html><body><main id="app-root"></main><div class="content-shell"><p>{prose}</p></div></body></html>"##
        );
        let md = html_to_markdown(&html);
        assert!(
            md.contains("Long-form article prose"),
            "relaxed pass recovers: {md}"
        );
    }

    #[test]
    fn negative_class_overmatch_falls_back_to_relaxed_pass() {
        // The whole article lives under a container whose class matches a
        // negative keyword ("promo-layout") — aggressive pass deletes it all.
        let prose = "Substantial body text, with commas, sentence after sentence of it. ".repeat(8);
        let html = format!(
            r##"<html><body><article><div class="promo-layout"><p>{prose}</p></div></article></body></html>"##
        );
        let md = html_to_markdown(&html);
        assert!(
            md.contains("Substantial body text"),
            "relaxed pass recovers: {md}"
        );
    }

    #[test]
    fn openapi_spec_renders_endpoint_reference() {
        let spec = r#"{"openapi":"3.0.0","info":{"title":"Petstore","version":"1.0.7",
"description":"A sample API"},"paths":{"/pet/{petId}/uploadImage":{"post":{
"summary":"Uploads an image","parameters":[{"name":"petId","in":"path","required":true,
"schema":{"type":"integer"}}],"responses":{"200":{"description":"successful operation"}}}}}}"#;
        let v: serde_json::Value = serde_json::from_str(spec).unwrap();
        let md = json_to_markdown(&v);
        assert!(md.starts_with("# Petstore 1.0.7 (OpenAPI)"), "{md}");
        assert!(md.contains("## POST /pet/{petId}/uploadImage"), "{md}");
        assert!(
            md.contains("param `petId` (path, integer, required)"),
            "{md}"
        );
        assert!(md.contains("responses: 200 successful operation"), "{md}");
    }

    /// The outline is LOSSY on a uniform array, and both web
    /// channels depend on knowing exactly how lossy.
    ///
    /// `json_outline` emits the item count and the fields of the FIRST record;
    /// every later record is dropped. That is the right view when the agent
    /// asks what an API returns, and the wrong one when it fetched a listing
    /// to read it — which is why `cmd_web` and the WebFetch hook both stash
    /// the RAW body and append an `expand-ref` pointer. If this test ever
    /// starts failing because the outline got richer, revisit whether those
    /// pointers are still needed; do not just update the assertion.
    #[test]
    fn uniform_array_outline_keeps_only_the_first_record() {
        let items: Vec<String> = (0..50)
            .map(|i| format!(r#"{{"id":{i},"title":"post number {i}"}}"#))
            .collect();
        let v: serde_json::Value = serde_json::from_str(&format!("[{}]", items.join(","))).unwrap();
        let md = json_to_markdown(&v);
        assert!(md.contains("[50 item(s)]"), "count must survive: {md}");
        assert!(md.contains("post number 0"), "first record is kept: {md}");
        assert!(
            !md.contains("post number 7") && !md.contains("post number 49"),
            "later records are DROPPED — the stash pointer is load-bearing: {md}"
        );
    }

    #[test]
    fn generic_json_outlines_keys_and_arrays() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"name":"tokenade","versions":[{"num":"1.0"},{"num":"0.9"}],"meta":{"stars":42}}"#,
        )
        .unwrap();
        let md = json_to_markdown(&v);
        assert!(md.contains("- name: \"tokenade\""), "{md}");
        assert!(md.contains("- versions: [2 item(s)]"), "{md}");
        assert!(md.contains("- stars: 42"), "{md}");
    }

    #[test]
    fn distill_text_routes_json_by_sniff_and_html_otherwise() {
        let json = r#"{"a": 1}"#;
        assert!(distill_text(json, None).contains("- a: 1"));
    }

    /// Measured on the 147-page corpus: 14 pages shipped their name twice
    /// because the header/body comparison was a raw substring test.
    #[test]
    fn a_body_heading_that_only_restates_the_title_is_dropped() {
        let t = "Models | Django documentation";
        assert_eq!(
            drop_body_heading_echoing_title("# Models\n\nBody text.", t),
            "Body text."
        );
        // Case and separator differences must not save it.
        assert_eq!(
            drop_body_heading_echoing_title(
                "# Writing your docs\n\nx",
                "Writing Your Docs - MkDocs"
            ),
            "x"
        );
        // Smart quotes vs plain — the same headline typeset two ways.
        assert_eq!(
            drop_body_heading_echoing_title(
                "# 75 Thoughts I Had About The Odyssey\n\nx",
                "75 Thoughts I Had About \u{2018}The Odyssey\u{2019}"
            ),
            "x"
        );
    }

    /// Containment is one-directional ON PURPOSE. otomoto.pl pairs the title
    /// `ERROR: The request could not be satisfied` with an `<h1>` of
    /// `403 ERROR`: the status code is nowhere in the title, so dropping the
    /// heading would delete the only place it appears.
    #[test]
    fn a_body_heading_that_adds_information_is_kept() {
        let body = "# 403 ERROR\n\nx";
        assert_eq!(
            drop_body_heading_echoing_title(body, "ERROR: The request could not be satisfied"),
            body
        );
        // `Crate serde` says something `serde - Rust` does not.
        let c = "# Crate serde\n\nx";
        assert_eq!(drop_body_heading_echoing_title(c, "serde - Rust"), c);
    }

    /// Only the OPENING heading restates the title; the same words deeper in
    /// the document are a section that happens to share them.
    #[test]
    fn a_matching_heading_far_down_the_body_is_kept() {
        let body = format!("{}\n# Models\n\ntail", "filler\n".repeat(14));
        assert_eq!(
            drop_body_heading_echoing_title(&body, "Models | Django"),
            body
        );
    }

    /// Non-Latin scripts must compare on their own characters — an ASCII-only
    /// key collapsed `mogg ｴﾗｰ` and `ページを表示できません` to the same thing.
    #[test]
    fn unicode_headings_are_not_collapsed_to_empty() {
        let body = "# ページを表示できません\n\nx";
        assert_eq!(
            drop_body_heading_echoing_title(body, "mogg \u{ff74}\u{ff97}\u{ff70}"),
            body
        );
        assert!(!title_key("ページを表示できません").is_empty());
    }

    /// `["']([^"']*)["']` closed on EITHER quote, so a double-quoted value
    /// stopped at its first apostrophe. goodreads.com lost everything after
    /// `the world` in its `og:description`; an apostrophe is ordinary English,
    /// so this truncated silently wherever it appeared.
    #[test]
    fn an_apostrophe_does_not_truncate_a_double_quoted_attribute() {
        let html =
            r#"<meta property="og:description" content="the world's largest site for readers">"#;
        assert_eq!(
            meta_content(html, "og:description").as_deref(),
            Some("the world's largest site for readers")
        );
        // The mirror: a double quote inside a single-quoted value.
        let sq = r#"<meta name='description' content='he said "yes" twice'>"#;
        assert_eq!(
            meta_content(sq, "description").as_deref(),
            Some(r#"he said "yes" twice"#)
        );
    }

    /// Same defect, same fix, the other prose-carrying attribute — plus the
    /// link whose label group shifted when `ATTR_VALUE` took groups 1 and 2.
    #[test]
    fn apostrophes_survive_in_alt_text_and_links() {
        let md = html_to_markdown(
            r#"<p><img src="a.png" alt="A man's hat"></p><p><a href="/it's">A link's label</a></p>"#,
        );
        assert!(md.contains("[image: A man's hat]"), "alt truncated: {md}");
        assert!(md.contains("[A link's label](/it's)"), "link broken: {md}");
        // Content-Type wins even without a leading brace after whitespace.
        assert!(distill_text("  {\"b\": 2}", Some("application/json")).contains("- b: 2"));
        // Something that merely STARTS with '{' but isn't JSON → HTML path.
        let not_json = "{ hello <b>world</b>";
        assert!(distill_text(not_json, None).contains("hello world"));
        let html = "<html><body><main><p>Plain page.</p></main></body></html>";
        assert_eq!(distill_text(html, Some("text/html")), "Plain page.");
    }

    /// `robots.txt` is line-structured: `User-agent:` means nothing without the
    /// `Disallow:` lines under it. The HTML pipeline collapsed every newline,
    /// fusing a 49 KB file into one line (measured on etsy.com and python.org).
    #[test]
    fn plain_text_keeps_its_lines() {
        let robots = "User-agent: *\nDisallow: /private\nAllow: /\n";
        let out = distill_text(robots, Some("text/plain"));
        assert_eq!(out, robots, "plain text must survive byte for byte");
        assert_eq!(out.lines().count(), 3);
    }

    /// raw.githubusercontent serves `.md` as `text/plain`. Re-parsing markdown
    /// as HTML inlined its setext headings and broke its link syntax.
    #[test]
    fn raw_markdown_is_not_re_parsed_as_html() {
        let md = "Version 1.0\n===========\n\n- [a link](https://example.com/x)\n- `code`\n";
        assert_eq!(distill_text(md, Some("text/plain; charset=utf-8")), md);
    }

    /// Servers mislabel. A body that announces itself as HTML overrides the
    /// header, or one `text/plain`-serving CDN would ship us raw markup.
    #[test]
    fn a_mislabelled_html_body_still_takes_the_html_path() {
        let html = "<!doctype html><html><body><p>Plain page.</p></body></html>";
        assert_eq!(distill_text(html, Some("text/plain")), "Plain page.");
        let no_doctype = "<html><body><p>Plain page.</p></body></html>";
        assert_eq!(distill_text(no_doctype, Some("text/plain")), "Plain page.");
    }

    /// An unlabelled response is overwhelmingly HTML; guessing "plain" from the
    /// bytes would stop stripping real pages.
    #[test]
    fn an_unlabelled_body_keeps_the_html_path() {
        let html = "<html><body><p>Plain page.</p><nav>junk</nav></body></html>";
        assert_eq!(distill_text(html, None), "Plain page.");
    }

    #[test]
    fn simple_tables_become_pipe_tables() {
        let html = "<html><body><main><p>Intro paragraph with enough words to not be tiny, \
                    plus some more filler text here, and even more words to pass the validity \
                    floor of two hundred characters so the aggressive pass is kept as-is.</p>\
                    <table><tr><th>Flag</th><th>Default</th></tr>\
                    <tr><td>--max-time</td><td>8</td></tr>\
                    <tr><td>--retry</td><td>0|1</td></tr></table></main></body></html>";
        let md = html_to_markdown(html);
        assert!(md.contains("| Flag | Default |"), "{md}");
        assert!(md.contains("| --max-time | 8 |"), "{md}");
        assert!(md.contains(r"| --retry | 0\|1 |"), "pipe escaped: {md}");
        assert!(md.contains("| --- | --- |"), "{md}");
    }

    #[test]
    fn table_separator_row_carries_column_alignment() {
        let html = "<html><body><main><p>Intro paragraph with enough words to not be tiny, \
                    plus some more filler text here, and even more words to pass the validity \
                    floor of two hundred characters so the aggressive pass is kept as-is.</p>\
                    <table><tr><th align=\"left\">Name</th><th align=\"right\">Bytes</th>\
                    <th style=\"text-align:center\">Kind</th><th>Notes</th></tr>\
                    <tr><td>a.txt</td><td>12</td><td>file</td><td>-</td></tr>\
                    <tr><td>b.txt</td><td>34</td><td>file</td><td>-</td></tr></table>\
                    </main></body></html>";
        let md = html_to_markdown(html);
        assert!(
            md.contains("| :--- | ---: | :---: | --- |"),
            "alignment markers not carried through: {md}"
        );
    }

    #[test]
    fn nested_tables_degrade_to_text_not_garbage() {
        let prose = "Enough surrounding prose to keep the aggressive pass valid, with commas, \
                     and yet more words repeated over and over for the two hundred floor. "
            .repeat(2);
        let html = format!(
            "<html><body><main><p>{prose}</p><table><tr><td>\
             <table><tr><td>inner cell</td></tr></table></td><td>outer cell</td></tr></table>\
             </main></body></html>"
        );
        let md = html_to_markdown(&html);
        assert!(md.contains("inner cell"), "content preserved: {md}");
        assert!(md.contains("outer cell"), "content preserved: {md}");
        assert!(!md.contains("<table"), "no leaked tags: {md}");
    }

    #[test]
    fn genuinely_tiny_page_stays_tiny() {
        let html = "<html><body><main><p>Done.</p></main></body></html>";
        let md = html_to_markdown(html);
        assert_eq!(md, "Done.");
    }

    #[test]
    fn comma_rich_link_paragraphs_survive_prune() {
        // A short paragraph THICK with links but clearly prose (commas) must
        // NOT be pruned — the comma guard is what separates citations-in-prose
        // from nav clusters.
        let md_in = "See [alpha](a), [beta](b), and [gamma](c), which together, frankly, matter.";
        let out = prune_link_dense_blocks(md_in);
        assert_eq!(out, md_in);
    }

    #[test]
    fn truncate_multibyte_html_no_panic() {
        // 20k of CJK — proportional byte cut lands mid-codepoint pre-fix.
        let html = format!(
            "<html><body>{}</body></html>",
            "揭示了这个问题".repeat(3000)
        );
        let out = truncate_to_max_tokens(&html, 100);
        assert!(out.len() < html.len());
    }

    #[test]
    fn a_sitemap_is_a_url_list_not_a_document() {
        // `one.google.com/sitemap.xml` was passing through byte for byte:
        // 23 352 814 in, 23 352 815 out, ~5.8M tokens for 1 824 URLs. The bulk
        // is `<xhtml:link rel="alternate" hreflang=…>` repeated per locale, so
        // the same href appears many times over.
        let mut xml = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n",
        );
        for i in 0..3 {
            xml.push_str(&format!("<url><loc>https://ex.test/p{i}</loc>"));
            // the alternates that make these files enormous, all pointing back
            for _ in 0..20 {
                xml.push_str(&format!("<loc>https://ex.test/p{i}</loc>"));
            }
            xml.push_str("</url>\n");
        }
        xml.push_str("</urlset>");

        let md = crate::xml::sitemap_to_markdown(&xml).expect("a urlset is a sitemap");
        assert!(md.starts_with("> source: sitemap (3 URLs)"), "{md}");
        assert!(md.contains("https://ex.test/p2"));
        assert_eq!(
            md.matches("https://ex.test/p0").count(),
            1,
            "alternates must dedupe"
        );
        assert!(
            md.len() < xml.len() / 4,
            "no compaction: {} vs {}",
            md.len(),
            xml.len()
        );

        // A sitemap index is the other schema and carries the same element.
        let idx = "<?xml version=\"1.0\"?><sitemapindex><sitemap><loc>https://ex.test/s1.xml</loc></sitemap></sitemapindex>";
        assert!(crate::xml::sitemap_to_markdown(idx)
            .unwrap()
            .starts_with("> source: sitemap index (1 URLs)"));

        // Everything else keeps its existing path.
        assert!(
            crate::xml::sitemap_to_markdown("<?xml version=\"1.0\"?><rss><channel/></rss>")
                .is_none()
        );
        assert!(
            crate::xml::sitemap_to_markdown("<html><body>not xml at all</body></html>").is_none()
        );
    }

    /// The ~4 KB head-scan for the schema marker must clamp to a CHAR boundary:
    /// a multi-byte codepoint straddling byte offset 4096 once panicked a raw
    /// `&xml[..4096]`. A sitemap padded so a `é` sits across the boundary must
    /// parse, not crash.
    #[test]
    fn sitemap_head_scan_survives_a_multibyte_char_at_the_boundary() {
        let mut xml = String::from("<?xml version=\"1.0\"?><urlset><!--");
        // Run a 2-byte char PAST offset 4096 so byte 4096 lands mid-codepoint
        // (the exact input a raw `&xml[..4096]` panicked on). `<urlset` is still
        // in the head; the real <loc> follows, found by the full-body scan.
        while xml.len() < 5000 {
            xml.push('é');
        }
        xml.push_str("--><url><loc>https://ex.test/deep</loc></url></urlset>");
        // Must not panic; a <urlset> is still a sitemap.
        let md = crate::xml::sitemap_to_markdown(&xml).expect("still a urlset");
        assert!(md.contains("https://ex.test/deep"), "{md}");
    }

    #[test]
    fn truncate_to_max_tokens_short_circuits_below_cap() {
        let html = "<p>hello</p>";
        let t = truncate_to_max_tokens(html, 10_000);
        assert_eq!(t, html);
    }

    #[test]
    fn truncate_to_max_tokens_caps_long_input() {
        // Build a long-ish input.
        let html = "<p>".to_string() + &"abcdefghij ".repeat(2000) + "</p>";
        let t = truncate_to_max_tokens(&html, 100);
        assert!(t.len() < html.len(), "must actually trim");
        // Token estimate of the trimmed slice should be <= cap (+slack).
        let est = crate::tokenizer::estimate(t);
        assert!(est <= 130, "trimmed estimate {est} too far above cap (100)");
    }

    #[test]
    fn truncate_zero_means_no_cap() {
        let html = "<p>hello</p>";
        assert_eq!(truncate_to_max_tokens(html, 0), html);
    }

    #[test]
    fn truncate_lands_on_safe_boundary() {
        // The cut MUST land on `>` or `\n`, never mid-attribute.
        let html = "<a href=\"https://example.com\">link</a> ".repeat(500);
        let t = truncate_to_max_tokens(&html, 50);
        assert!(t.is_empty() || t.ends_with('>') || t.ends_with('\n'));
    }

    #[test]
    fn html_to_markdown_capped_marks_truncation() {
        // 20k chars of repeated paragraphs — well above any reasonable cap.
        let html = "<p>alpha beta gamma delta</p>".repeat(800);
        let (md, _sav, truncated) = html_to_markdown_capped(&html, 500);
        assert!(truncated, "capped path must report truncation");
        // Markdown should still contain some content from the prefix.
        assert!(md.contains("alpha"), "markdown content was empty: {md:?}");
    }

    #[test]
    fn html_to_markdown_capped_no_truncate_under_cap() {
        let html = "<p>tiny</p>";
        let (md, _sav, truncated) = html_to_markdown_capped(html, 1000);
        assert!(!truncated);
        assert!(md.contains("tiny"));
    }

    #[test]
    fn strips_script_and_style() {
        let html = "<html><head><style>body{color:red}</style></head><body><script>alert(1)</script><p>hello</p></body></html>";
        let md = html_to_markdown(html);
        assert!(md.contains("hello"));
        assert!(!md.contains("alert"));
        assert!(!md.contains("color:red"));
    }

    #[test]
    fn setext_headings_normalise_to_atx() {
        assert_eq!(
            setext_to_atx("Title\n===\nBody text."),
            "# Title\nBody text."
        );
        assert_eq!(
            setext_to_atx("Subtitle\n---\nMore text."),
            "## Subtitle\nMore text."
        );
        // A blank-line-separated `---` (this file's own banner/body separator
        // shape) must NOT be reinterpreted as a setext underline.
        assert_eq!(
            setext_to_atx("Body text.\n\n---\n\nMore."),
            "Body text.\n\n---\n\nMore."
        );
    }

    #[test]
    fn fenced_code_carries_a_language_hint_and_drops_the_gutter() {
        let html = "<html><body><main><p>Filler prose long enough to keep the aggressive tier, \
                    padded out with commas, clauses and more words past the two hundred floor.</p>\
                    <pre><code class=\"language-rust\">\
                    <span class=\"line-number\">1</span>fn main() {}\
                    </code></pre></main></body></html>";
        let md = html_to_markdown(html);
        assert!(md.contains("```rust"), "language hint missing: {md}");
        assert!(md.contains("fn main() {}"), "{md}");
        assert!(
            !md.contains("1fn main"),
            "gutter digit leaked into the code: {md}"
        );
    }

    #[test]
    fn tracking_pixel_is_dropped_even_with_alt_text() {
        let html = "<html><body><main><p>Filler prose long enough to keep the aggressive tier, \
                    padded out with commas, clauses and more words past the two hundred floor.</p>\
                    <img src=\"https://example.com/a/pixel.gif\" alt=\"page view\">\
                    </main></body></html>";
        let md = html_to_markdown(html);
        assert!(
            !md.contains("page view"),
            "tracking pixel alt text leaked: {md}"
        );
    }

    #[test]
    fn link_text_dedup_and_ui_noise_and_repeated_bullets() {
        let html = "<html><body><main><p>Filler prose long enough to keep the aggressive tier, \
                    padded out with commas, clauses and more words past the two hundred floor.</p>\
                    <p><a href=\"/apple\">Apple Apple</a></p>\
                    <p>Was this page helpful?</p>\
                    <ul><li>Loading</li><li>Loading</li><li>Loading</li><li>Loading</li></ul>\
                    </main></body></html>";
        let md = html_to_markdown(html);
        assert!(
            md.contains("[Apple](/apple)"),
            "link text not deduped: {md}"
        );
        assert!(!md.contains("Apple Apple"), "{md}");
        assert!(
            !md.contains("Was this page helpful"),
            "UI noise line survived: {md}"
        );
        assert_eq!(
            md.matches("- Loading").count(),
            2,
            "3rd+ repeated bullet not dropped: {md}"
        );
    }

    #[test]
    fn json_ld_mining_survives_a_large_payload_via_the_stack_guard() {
        // Padding the JSON-LD payload past the 500 KB guard threshold in
        // `parse_and_mine` exercises the spawned-thread path, not just the
        // inline one — proving the guard doesn't silently drop the mine.
        let padding = "\"x\":\"filler\",".repeat(40_000); // well over 500 KB
        let json = format!(
            "{{\"@context\":\"https://schema.org\",\"@type\":\"Article\",\
             \"description\":\"A description long enough to clear the mining \
             floor and prove the guarded thread path still recovers real prose.\",\
             {padding}\"end\":true}}"
        );
        let html = format!(
            "<html><body><script type=\"application/ld+json\">{json}</script>\
             <main></main></body></html>"
        );
        let md = html_to_markdown(&html);
        assert!(
            md.contains("prove the guarded thread path still recovers real prose"),
            "big JSON-LD payload not mined ({} chars out)",
            md.len()
        );
    }

    /// **A rescued article's title is a HEADING.**
    ///
    /// A JS shell renders nothing: the readable text is in the
    /// `application/ld+json`, and that is where it is fished out from.
    /// Everything arrived as paragraphs, so a page rescued this way left
    /// without a single heading — and a large page's outline, built on
    /// headings, had no section at all.
    #[test]
    fn a_rescued_article_opens_on_its_headline() {
        let body = "Le conseil municipal a vote mardi soir la renovation du reseau d'eau, un                     chantier de quatre ans dont le premier troncon sera engage des le printemps."
            .repeat(3);
        let json = format!(
            "{{\"@context\":\"https://schema.org\",\"@type\":\"NewsArticle\",\
             \"headline\":\"Le conseil vote la renovation du reseau d'eau\",\
             \"publisher\":{{\"@type\":\"Organization\",\
             \"name\":\"Le journal quotidien de la ville et de sa region\"}},\
             \"articleBody\":\"{body}\"}}"
        );
        let html = format!(
            "<html><body><script type=\"application/ld+json\">{json}</script>\
             <main></main></body></html>"
        );
        let md = html_to_markdown(&html);
        assert!(
            md.lines()
                .any(|l| l.trim() == "# Le conseil vote la renovation du reseau d'eau"),
            "the article's title does not open the page:\n{md}"
        );
        // The bound: `name` is carried by every entity of a page — here the
        // publisher. Turning them into headings would invent one per entity,
        // and the outline would name sections the page does not have.
        assert_eq!(
            md.lines()
                .filter(|l| l.trim_start().starts_with("# "))
                .count(),
            1,
            "more than one heading was invented:\n{md}"
        );
        // **And the headline does not have to be the FIRST mined field.** A
        // page blob commonly lists the site or the publisher before the
        // article; a rule "only if the output is empty" then leaves the page
        // without any heading, and the mutation that replaces it with "any
        // named field" survives as long as the fixture puts `headline` first.
        let listed = format!(
            "[{{\"@type\":\"WebSite\",\"name\":\"Le portail regional d'information continue\"}},{json}]"
        );
        let md_listed = html_to_markdown(&format!(
            "<html><body><script type=\"application/ld+json\">{listed}</script>\
             <main></main></body></html>"
        ));
        assert!(
            md_listed
                .lines()
                .any(|l| l.trim() == "# Le conseil vote la renovation du reseau d'eau"),
            "the article's title was lost because another entity came first:\n{md_listed}"
        );
        assert_eq!(
            md_listed
                .lines()
                .filter(|l| l.trim_start().starts_with("# "))
                .count(),
            1,
            "the site name became a heading:\n{md_listed}"
        );

        assert!(
            md.contains("Le journal quotidien de la ville"),
            "the publisher name vanished from the text — the bound then proves nothing, \
             since an absent `name` cannot become a heading:\n{md}"
        );
    }

    /// The measured catalogue: the values went out bare, without the name of
    /// the field carrying them, so `89.90` was no longer a price.
    #[test]
    fn an_xml_export_keeps_the_name_of_each_field() {
        let xml = "<?xml version=\"1.0\"?><catalogue>\
                   <produit><nom>Perceuse sans fil</nom><prix>89.90</prix><stock>12</stock></produit>\
                   <produit><nom>Scie sauteuse</nom><prix>64.50</prix><stock>3</stock></produit>\
                   </catalogue>";
        let md = super::labelled_xml_records(xml).expect("a record export");
        assert!(md.contains("prix: 89.90"), "the field is not named:\n{md}");
        assert!(md.contains("nom: Scie sauteuse"), "{md}");
        // A name that comes back opens a record: without this cut, the six
        // values read as a single product.
        assert!(
            md.contains("stock: 12\n\nnom: Scie sauteuse"),
            "the two products are not separated:\n{md}"
        );
    }

    /// An inventory where EVERYTHING is in the attributes: without them, this
    /// document has not one character of text, and it went out as "no
    /// readable text". Three items, zero words delivered.
    #[test]
    fn a_record_that_lives_in_its_attributes_is_read() {
        let xml = "<?xml version=\"1.0\"?><inventaire>\
                   <article ref=\"A-1\" nom=\"Perceuse sans fil\" prix=\"89.90\" stock=\"12\"/>\
                   <article ref=\"A-2\" nom=\"Scie sauteuse\" prix=\"64.50\" stock=\"3\"/>\
                   </inventaire>";
        let md = super::labelled_xml_records(xml).expect("an inventory in attributes");
        assert!(
            md.contains("article: ref=A-1, nom=Perceuse sans fil, prix=89.90, stock=12"),
            "the record is not read:\n{md}"
        );
        assert!(md.contains("ref=A-2"), "the second item is missing:\n{md}");
    }

    /// The attributes of a tag carrying text JOIN its value, and a container's
    /// open its record: without them, `154.40` has no currency and the order
    /// no id.
    #[test]
    fn an_attribute_joins_the_value_it_qualifies() {
        let xml = "<?xml version=\"1.0\"?><commandes>\
                   <commande id=\"C-100\" date=\"2026-09-01\"><client>Dupont</client>\
                   <total devise=\"EUR\">154.40</total></commande></commandes>";
        let md = super::labelled_xml_records(xml).expect("an order");
        assert!(md.contains("commande: id=C-100, date=2026-09-01"), "{md}");
        assert!(md.contains("total (devise=EUR): 154.40"), "{md}");
        assert!(md.contains("client: Dupont"), "{md}");
    }

    /// **Two fields with the same name, in two different places.** An order
    /// carries the customer's address and the delivery address; both have a
    /// `ville`, and the flat rendering gave two of them, without saying which
    /// is which.
    #[test]
    fn a_nested_field_says_where_it_comes_from() {
        let xml = "<?xml version=\"1.0\"?><commandes>\
                   <commande id=\"C-100\">\
                   <client><nom>Dupont</nom><ville>Lyon</ville></client>\
                   <livraison><ville>Villeurbanne</ville><delai>48h</delai></livraison>\
                   <total>154.40</total></commande></commandes>";
        let md = super::labelled_xml_records(xml).expect("a nested order");
        assert!(md.contains("client.ville: Lyon"), "{md}");
        assert!(md.contains("livraison.ville: Villeurbanne"), "{md}");
        // And the record stays in ONE piece: the empty line is the boundary
        // between two orders, not between two sub-objects of the same one.
        // Without this assertion, a boundary placed one level too low yields
        // the same output on a record that has only one sub-object.
        assert!(
            md.contains("client.ville: Lyon\nlivraison.ville: Villeurbanne"),
            "an empty line cuts the record in two:\n{md}"
        );
        // A field directly in the record has nothing to disambiguate:
        // prefixing it would give `commande.total`, longer and adding nothing.
        assert!(
            md.lines().any(|l| l.trim() == "total: 154.40"),
            "a top-level field was prefixed for nothing:\n{md}"
        );
    }

    /// The boundary between two records is the TAG's, no longer "a field name
    /// comes back": that guess put the empty line in the wrong place as soon
    /// as a record carried two sub-objects.
    #[test]
    fn each_record_opens_after_a_blank_line() {
        let xml = "<?xml version=\"1.0\"?><commandes>\
                   <commande id=\"C-100\"><client><nom>Dupont</nom></client></commande>\
                   <commande id=\"C-101\"><client><nom>Martin</nom></client></commande>\
                   </commandes>";
        let md = super::labelled_xml_records(xml).expect("two orders");
        assert!(
            md.contains("client.nom: Dupont\n\ncommande: id=C-101"),
            "the two records are not separated at the right place:\n{md}"
        );
    }

    /// Two bounds on what an attribute is NOT: a namespace is a schema
    /// address, and an oversized value is a blob (base64, path). The whole SVG
    /// is discarded at the root for the same reason.
    #[test]
    fn a_namespace_or_a_blob_is_not_data() {
        let blob = "M".repeat(super::ATTR_VALUE_MAX_CHARS + 1);
        let xml = format!(
            "<?xml version=\"1.0\"?><doc xmlns=\"urn:exemple\" xmlns:x=\"urn:x\">\
             <forme d=\"{blob}\" nom=\"un carre\"/></doc>"
        );
        let md = super::labelled_xml_records(&xml).expect("a document");
        assert!(
            md.contains("nom=un carre"),
            "the readable data vanished:\n{md}"
        );
        assert!(
            !md.contains("urn:exemple"),
            "a namespace is delivered:\n{md}"
        );
        assert!(!md.contains("MMMM"), "a blob is delivered:\n{md}");
        // The smallest ACCEPTED case, just under the bound.
        let ok = "M".repeat(super::ATTR_VALUE_MAX_CHARS);
        let xml = format!("<?xml version=\"1.0\"?><doc><forme d=\"{ok}\"/></doc>");
        assert!(
            super::labelled_xml_records(&xml).is_some_and(|m| m.contains(&ok)),
            "the bound refuses a value it must accept"
        );
        // And an SVG does not get through at all: its attributes are coordinates.
        let svg = "<?xml version=\"1.0\"?><svg viewBox=\"0 0 2 2\"><rect width=\"1\"/></svg>";
        assert!(
            super::labelled_xml_records(svg).is_none(),
            "an SVG was read as an export"
        );
    }

    /// **The bound, and it is what makes the rule safe.** A MIXED-content
    /// document — text next to an element, as in prose — would lose half its
    /// sentences if only the leaves were labelled. Rather than guessing the
    /// document's shape, the output is compared with the text the previous
    /// path returned: at the slightest difference, nothing is touched.
    #[test]
    fn a_document_with_mixed_content_is_left_alone() {
        let xml = "<?xml version=\"1.0\"?><article>\
                   <para>Un paragraphe avec du <emphasis>texte en relief</emphasis> au milieu, \
                   puis la suite de la phrase.</para></article>";
        assert!(
            super::labelled_xml_records(xml).is_none(),
            "a prose document was cut into fields, and half the text lost"
        );
    }

    /// Roots that already have their reader keep their route — otherwise a
    /// feed would arrive as `field: value` instead of its entries.
    #[test]
    fn a_feed_or_a_sitemap_keeps_its_own_reader() {
        for root in ["feed", "rss", "urlset", "sitemapindex", "html"] {
            let xml = format!(
                "<?xml version=\"1.0\"?><{root}><entry><title>Un titre assez long</title>\
                 </entry></{root}>"
            );
            assert!(
                super::labelled_xml_records(&xml).is_none(),
                "`{root}` was diverted from its reader"
            );
        }
    }

    #[test]
    fn readme_selector_rescues_a_page_with_no_main_or_article() {
        // GitHub-style repo page: the README content sits in a plain <div
        // id="readme">, no <main>/<article> anywhere — isolate_main() finds
        // nothing, so the selector-priority rescue must kick in instead of
        // falling straight to the whole noise-stripped document (which here
        // still carries the sidebar's nav-like link list).
        let sidebar = (0..30)
            .map(|i| format!("<a href=\"/repo{i}\">repo-{i}</a>"))
            .collect::<Vec<_>>()
            .join(" ");
        let html = format!(
            "<html><body>\
             <div class=\"sidebar\">{sidebar}</div>\
             <div id=\"readme\"><h1>tokenade</h1>\
             <p>Web-content compression for AI coding agents, ported to a real README \
             body with enough words to clear the two-hundred-character extraction floor \
             comfortably on its own, well past whatever a short one-line summary would \
             have managed, so the aggressive pass is accepted outright and the relaxed \
             fallback pass — which would pull in the whole document, sidebar and all — \
             never has a reason to fire in the first place.</p>\
             </div></body></html>"
        );
        let md = html_to_markdown(&html);
        assert!(
            md.contains("Web-content compression"),
            "readme content kept: {md}"
        );
        assert!(
            !md.contains("repo-0"),
            "sidebar excluded by the readme selector: {md}"
        );
    }

    #[test]
    fn article_isolation_floor_rejects_a_lone_card_off_a_grid() {
        // books.toscrape.com-style listing: many <article> product cards, no
        // <main>. `isolate_main` picks the FIRST <article> greedily, which
        // alone is far under 10 % of the page. essence's isolation floor
        // is scoped to `<article>` because it has no sibling-rescue
        // the way `<main>` does (`article_siblings` only fires once
        // `isolate_main_span` already picked `<main>`).
        //
        // The single card's OWN text comfortably clears the 200-char T1/T2
        // floor on its own, so this only fails without the new floor — a
        // weaker padding would have the OLD `EXTRACT_VALID_MIN_CHARS` ladder
        // rescue the other cards for an unrelated reason, proving nothing.
        let card = |i: usize| {
            format!(
                "<article><h3>Product {i}</h3><p>Description text for product number {i}, \
                 padded well past two hundred characters on its own so the ordinary thin-result \
                 fallback cannot be what rescues the other cards in this test — only the new \
                 article isolation floor can.</p></article>"
            )
        };
        let cards: String = (0..20).map(card).collect();
        let doc = format!("<html><body>{cards}</body></html>");
        let md = html_to_markdown(&doc);
        assert!(md.contains("Product 0"), "{md}");
        assert!(
            md.contains("Product 19"),
            "lone-card isolation should have been rejected: {md}"
        );
    }

    #[test]
    fn nav_ratio_accept_ladder_matches_essence_thresholds() {
        // main_chars alone clears the top rung.
        assert!(content_sufficiency(1001, 1001, 0));
        // total is generous and nav is a minority: middle rung accepts.
        assert!(content_sufficiency(400, 1500, 400));
        // total is generous but nav dominates AND main is below the bottom
        // rung too: every rung refuses.
        assert!(!content_sufficiency(150, 1500, 1200));
        // Bottom rung: 200 = 1000 / 5 is the floor, inclusive of "just over".
        assert!(content_sufficiency(201, 300, 0));
        assert!(!content_sufficiency(200, 300, 0));
    }

    #[test]
    fn thin_result_banner_names_chrome_domination_as_the_cause() {
        // A page whose visible text is mostly links inside <nav>, with a
        // near-empty <main> — the extraction ladder recovers nothing, and
        // the banner should say WHY: chrome, not "the page has no text".
        let nav_links = (0..40)
            .map(|i| format!("<a href=\"/l{i}\">Section heading number {i} in the site nav</a>"))
            .collect::<Vec<_>>()
            .join(" ");
        let html = format!(
            "<html><body><script>var x=1;</script>\
             <nav>{nav_links}</nav><main></main></body></html>"
        );
        let md = html_to_markdown(&html);
        assert!(md.contains("[tokenade: thin-result]"), "{md}");
        assert!(
            md.contains("sits in nav/header/footer/aside"),
            "no WHY clause: {md}"
        );
    }

    #[test]
    fn a_small_shell_that_yields_nothing_is_still_disclosed() {
        // archive.org/details/… serves a 1 872-byte JS bootstrap — under the
        // 2 048-byte "genuinely small page" exemption — and its whole delivery
        // was the site's generic <title>, with nothing saying the item page had
        // not been reached (MEASURED 2026-07-22).
        // Semantic elements present but EMPTY — the real page has markup, it
        // just has no text in it, so the hollowness tier correctly stays out of
        // the way and this branch is the one that must speak.
        let shell = "<html><head><title>Internet Archive: Digital Library</title></head>\
            <body><main><p></p><section></section></main><div id=\"app\"></div>\
            <script src=\"/app.js\"></script></body></html>";
        assert!(
            shell.len() < EMPTY_RESULT_MIN_HTML_BYTES,
            "fixture must be under the bar"
        );
        let md = html_to_markdown(shell);
        assert!(
            md.contains("[tokenade: thin-result]"),
            "small shell not disclosed: {md}"
        );
        assert!(
            md.contains("recovered 0 characters"),
            "must state the fact: {md}"
        );

        // The exemption still holds where it should: a small page that DID
        // yield text is not a failure and gets no banner.
        let small_but_real = "<html><head><title>Note</title></head><body><script>x()</script>\
            <main><p>This page is short but it genuinely says something, and that is not \
            a failure to report to anyone at all.</p></main></body></html>";
        assert!(small_but_real.len() < EMPTY_RESULT_MIN_HTML_BYTES);
        let md = html_to_markdown(small_but_real);
        assert!(
            !md.contains("thin-result"),
            "banner on a real small page: {md}"
        );
    }

    #[test]
    fn a_page_that_is_only_menus_says_so() {
        // bandcamp.com/discover's shape: the filter UI extracts cleanly and
        // clears the 200-char thin floor, so nothing warned that the listing
        // itself never arrived (MEASURED 2026-07-22: 506 delivered chars, all
        // of them genre and format labels, zero albums).
        let genres: String = [
            "electronic",
            "rock",
            "metal",
            "alternative",
            "hip-hop",
            "experimental",
            "punk",
            "folk",
            "pop",
            "ambient",
            "soundtrack",
            "world",
            "jazz",
            "acoustic",
            "funk",
        ]
        .iter()
        .map(|g| format!("<li><a href=\"/tag/{g}\">{g}</a></li>"))
        .collect();
        // The unit under test is the NOTE, not the extractor: on the real page
        // the genre lists survive extraction while `nav_main_char_split` counts
        // them as chrome, and it is precisely that disagreement the note
        // reports. Driving it through `html_to_markdown` would test whichever
        // branch the extractor happens to take instead.
        let html = format!(
            "<html><body><nav><ul>{genres}</ul><ul>{genres}</ul><ul>{genres}</ul>\
             <ul>{genres}</ul></nav><main><div id=\"results\"></div></main>\
             <script>load()</script></body></html>"
        );
        let body = "electronic rock metal alternative hip-hop experimental punk folk pop";
        let out = prepend_mostly_navigation_note(&html, body);
        assert!(
            out.contains("[tokenade: mostly-navigation]"),
            "no note: {out}"
        );
        // Prepended to, never replaced.
        assert!(out.contains("electronic"), "body dropped: {out}");

        // Same chrome, but the page's own text now dominates: no note.
        let article = format!(
            "<html><body><nav><ul>{genres}</ul></nav><main><article><h1>A real piece</h1><p>{}</p>\
             </article></main><script>x()</script></body></html>",
            "Substantial paragraph text that carries the page. ".repeat(40)
        );
        let out = prepend_mostly_navigation_note(&article, body);
        assert!(
            !out.contains("mostly-navigation"),
            "false positive on an article: {out}"
        );

        // A long extraction is never flagged, whatever the chrome share: enough
        // real text came back for the agent to judge it.
        let long = "Real recovered prose. ".repeat(80);
        let out = prepend_mostly_navigation_note(&html, &long);
        assert!(
            !out.contains("mostly-navigation"),
            "flagged a long body: {out}"
        );
    }

    #[test]
    fn a_code_fence_owns_its_line_even_mid_paragraph() {
        // docs.npmjs.com shape: prose, a sample, more prose, another sample —
        // all inside ONE <p>-ish flow. When the closing fence kept company on
        // its line the markdown was malformed and every fence-aware reader lost
        // parity from that point on (MEASURED: `markdown::outline` dropped 5 of
        // 7 sections of npm-install).
        let html = "<html><body><main><h1>Install</h1>\
            <p>Run this first, and read the rest of the paragraph afterwards:\
            <pre><code>npm install left-pad</code></pre>\
            then check the result before continuing to the second example:\
            <pre><code>npm ls left-pad</code></pre>\
            and that is the whole flow described here.</p>\
            <h2>Configuration</h2><p>More prose about configuring it well.</p>\
            </main></body></html>";
        let md = html_to_markdown(html);
        for l in md.lines() {
            let t = l.trim();
            if t.starts_with("```") {
                assert_eq!(
                    t.trim_start_matches('`').trim(),
                    t.trim_start_matches('`')
                        .trim_start()
                        .split(' ')
                        .next()
                        .unwrap_or("")
                        .trim(),
                    "a fence line carries trailing content: {l:?}\n--- full ---\n{md}"
                );
                assert!(
                    !t[3..].contains("```"),
                    "two fences on one line: {l:?}\n{md}"
                );
            }
        }
        // Parity must hold: an even number of fences, so a reader that toggles
        // on each one ends OUTSIDE a code block.
        let fences = md
            .lines()
            .filter(|l| l.trim_start().starts_with("```"))
            .count();
        assert_eq!(fences % 2, 0, "unbalanced fences ({fences}): {md}");
        assert!(
            md.contains("Configuration"),
            "heading after the samples was lost: {md}"
        );
    }

    #[test]
    fn code_samples_are_entity_decoded_so_an_agent_can_copy_them() {
        // Every doc site escapes its samples: npmjs/package/react shipped
        // `&lt;h1&gt;{count}&lt;/h1&gt;` to the agent (MEASURED 2026-07-22).
        // Code that cannot be pasted and run is not code.
        let html = "<html><body><main><h1>Sample</h1>\
            <pre><code>if (a &lt; b) &amp;&amp; &lt;div&gt;x&lt;/div&gt;</code></pre>\
            <p>Use <code>a &amp;&amp; b</code> to combine them, as shown above in \
            the fenced example, which is what the fix protects.</p></main></body></html>";
        let md = html_to_markdown(html);
        assert!(
            md.contains("if (a < b) && <div>x</div>"),
            "fenced sample not decoded: {md}"
        );
        assert!(md.contains("`a && b`"), "inline code not decoded: {md}");
        assert!(
            !md.contains("&lt;"),
            "an entity survived into the output: {md}"
        );
    }

    #[test]
    fn code_fence_sanctuary_protects_a_documented_title_tag() {
        // A hand-authored / malformed docs page: the example inside
        // <pre><code> is NOT entity-escaped, so `<title>`/`</title>` in the
        // sample are, byte for byte, exactly what the noise-tag strip at the
        // top of `html_to_markdown` looks for. Without the sanctuary this
        // example is deleted by the same regex that (correctly) deletes a
        // real page `<title>`.
        let html = "<html><body><main><h1>Setting a title</h1>\
            <p>Put this in your document head:</p>\
            <pre><code><title>My Page</title></code></pre>\
            <p>That is all it takes.</p></main></body></html>";
        let md = html_to_markdown(html);
        assert!(
            md.contains("My Page"),
            "code example must survive the noise strip: {md}"
        );
        assert!(
            md.contains("```"),
            "example must render as a fenced code block: {md}"
        );
    }

    #[test]
    fn isolates_main_content() {
        let html = r#"
            <html><body>
              <nav><a href="/">home</a></nav>
              <main><h1>Title</h1><p>Body of the article.</p></main>
              <footer>(c) 2026</footer>
            </body></html>
        "#;
        let md = html_to_markdown(html);
        assert!(md.contains("# Title"));
        assert!(md.contains("Body of the article."));
        assert!(!md.contains("(c) 2026"));
        assert!(!md.contains("home"));
    }

    #[test]
    fn converts_links_and_lists() {
        let html = r#"<ul><li>One</li><li><a href="https://example.com">Two</a></li></ul>"#;
        let md = html_to_markdown(html);
        assert!(md.contains("- One"));
        assert!(md.contains("[Two](https://example.com)"));
    }

    #[test]
    fn decodes_entities() {
        let md = html_to_markdown("<p>fish &amp; chips &mdash; tasty &#x2713;</p>");
        assert!(md.contains("fish & chips — tasty ✓"));
    }

    #[test]
    fn html_savings_are_real() {
        let html: String = format!(
            "<html><head><style>{}</style></head><body><main><h1>X</h1><p>{}</p></main></body></html>",
            "a{x:1}".repeat(2000),
            "lorem ipsum ".repeat(50)
        );
        let (_md, sav) = html_to_markdown_with_savings(&html);
        assert!(sav.percent() > 80.0, "savings {:?}", sav);
    }

    /// What is LEFT in a compacted a11y tree, priced in tokens rather than
    /// characters. Characters would say indentation is 21.6 % of the output;
    /// tokens say otherwise, because cl100k spends one token on a whole gutter.
    /// `#[ignore]`d — a measurement to decide from, not a gate.
    #[test]
    #[ignore = "measurement: what REMAINS in an a11y tree, in tokens"]
    fn what_is_left_in_a_compacted_a11y_tree() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/a11y");
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
            .expect("a11y fixtures")
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.to_string_lossy().ends_with(".a11y.txt"))
            .collect();
        files.sort();

        // Each candidate rewrites the COMPACTED output; the delta is what the
        // transformation would be worth if it were added to the pipeline.
        let candidates: Vec<(&str, fn(&str) -> String)> = vec![
            ("dedent_to_one_space", |s: &str| {
                s.lines()
                    .map(|l| {
                        let ind = l.len() - l.trim_start().len();
                        format!("{}{}\n", " ".repeat(ind / 2), l.trim_start())
                    })
                    .collect()
            }),
            ("drop_all_indent", |s: &str| {
                s.lines().map(|l| format!("{}\n", l.trim_start())).collect()
            }),
            ("drop_url_lines", |s: &str| {
                s.lines()
                    .filter(|l| !l.trim_start().starts_with("- /url:"))
                    .map(|l| format!("{l}\n"))
                    .collect()
            }),
            // The whole remaining headroom is in the `/url:` lines, so the
            // question is how much of it is recoverable WITHOUT telling the
            // agent less about where a link goes.
            ("url_drop_self_anchor", |s: &str| {
                let lines: Vec<&str> = s.lines().collect();
                let mut out: Vec<String> = Vec::with_capacity(lines.len());
                for (i, l) in lines.iter().enumerate() {
                    let t = l.trim_start();
                    if let Some(u) = t.strip_prefix("- /url:") {
                        let u = u.trim().trim_matches('"');
                        if let Some(frag) = u.strip_prefix('#') {
                            let name = lines[..i]
                                .iter()
                                .rev()
                                .take(3)
                                .find_map(|p| super::a11y_quoted_name(p));
                            if name
                                .as_deref()
                                .is_some_and(|n| super::says_the_same_words(n, frag))
                            {
                                continue;
                            }
                        }
                    }
                    out.push((*l).to_string());
                }
                out.join("\n")
            }),
            ("url_drop_derivable_from_text", |s: &str| {
                let lines: Vec<&str> = s.lines().collect();
                let mut out: Vec<String> = Vec::with_capacity(lines.len());
                for (i, l) in lines.iter().enumerate() {
                    let t = l.trim_start();
                    if let Some(u) = t.strip_prefix("- /url:") {
                        let u = u.trim().trim_matches('"');
                        let last = u.rsplit('/').next().unwrap_or("");
                        let name = lines[..i]
                            .iter()
                            .rev()
                            .take(3)
                            .find_map(|p| super::a11y_quoted_name(p));
                        if !last.is_empty()
                            && name
                                .as_deref()
                                .is_some_and(|n| super::says_the_same_words(n, last))
                        {
                            continue;
                        }
                    }
                    out.push((*l).to_string());
                }
                out.join("\n")
            }),
            ("url_strip_query", |s: &str| {
                s.lines()
                    .map(|l| {
                        let t = l.trim_start();
                        if t.starts_with("- /url:") {
                            if let Some(q) = l.find('?') {
                                return format!("{}\n", &l[..q]);
                            }
                        }
                        format!("{l}\n")
                    })
                    .collect()
            }),
            ("url_fold_repeats", |s: &str| {
                let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
                let mut out: Vec<String> = Vec::new();
                for l in s.lines() {
                    let t = l.trim_start();
                    if let Some(u) = t.strip_prefix("- /url:") {
                        if !seen.insert(u.trim().to_string()) {
                            continue;
                        }
                    }
                    out.push(l.to_string());
                }
                out.join("\n")
            }),
            // A ref is a handle for CLICKING. A paragraph, a cell, a term, a
            // piece of code and a plain image are not clickable, so their refs
            // are a handle to nothing.
            ("drop_refs_on_non_interactive", |s: &str| {
                const INTERACTIVE: &[&str] = &[
                    "link",
                    "button",
                    "textbox",
                    "checkbox",
                    "radio",
                    "combobox",
                    "listbox",
                    "option",
                    "menuitem",
                    "menuitemcheckbox",
                    "menuitemradio",
                    "tab",
                    "switch",
                    "slider",
                    "spinbutton",
                    "searchbox",
                    "textarea",
                    "input",
                    "select",
                    "summary",
                    "treeitem",
                    "gridcell",
                    "menu",
                    "menubar",
                    "dialog",
                    "alertdialog",
                    "form",
                ];
                let ref_re = super::re(r"\s*\[ref=e\d+\]");
                s.lines()
                    .map(|l| {
                        let t = l.trim_start();
                        let role = t
                            .strip_prefix("- ")
                            .unwrap_or(t)
                            .split([' ', ':', '['])
                            .next()
                            .unwrap_or("");
                        if INTERACTIVE.contains(&role) {
                            return format!("{l}\n");
                        }
                        format!("{}\n", ref_re.replace_all(l, ""))
                    })
                    .collect()
            }),
            ("drop_refs_denylist_keeping_wrappers", |s: &str| {
                let ref_re = super::re(r"\s*\[ref=e\d+\]");
                s.lines()
                    .map(|l| {
                        let t = l.trim_start();
                        let role = t
                            .strip_prefix("- ")
                            .unwrap_or(t)
                            .split([' ', ':', '['])
                            .next()
                            .unwrap_or("");
                        if super::A11Y_NOT_CLICKABLE.contains(&role) && !l.contains("[cursor=") {
                            return format!("{}\n", ref_re.replace_all(l, ""));
                        }
                        format!("{l}\n")
                    })
                    .collect()
            }),
            ("drop_all_refs", |s: &str| {
                let ref_re = super::re(r"\s*\[ref=e\d+\]");
                s.lines()
                    .map(|l| format!("{}\n", ref_re.replace_all(l, "")))
                    .collect()
            }),
            ("lift_single_text_child", |s: &str| {
                let lines: Vec<&str> = s.lines().collect();
                let ind = |l: &str| l.len() - l.trim_start().len();
                let mut out: Vec<String> = Vec::with_capacity(lines.len());
                let mut i = 0;
                while i < lines.len() {
                    let l = lines[i];
                    let only_child = l.trim_end().ends_with(':')
                        && i + 1 < lines.len()
                        && ind(lines[i + 1]) > ind(l)
                        && (i + 2 >= lines.len() || ind(lines[i + 2]) <= ind(l))
                        && super::a11y_plain_text(lines[i + 1].trim_start()).is_some();
                    if only_child {
                        let text = super::a11y_plain_text(lines[i + 1].trim_start()).unwrap();
                        out.push(format!("{} {text}", l.trim_end()));
                        i += 2;
                        continue;
                    }
                    out.push(l.to_string());
                    i += 1;
                }
                out.join("\n")
            }),
            ("join_sibling_text_runs", |s: &str| {
                let lines: Vec<&str> = s.lines().collect();
                let ind = |l: &str| l.len() - l.trim_start().len();
                let mut out: Vec<String> = Vec::with_capacity(lines.len());
                let mut i = 0;
                while i < lines.len() {
                    let l = lines[i];
                    if let Some(t) = super::a11y_plain_text(l.trim_start()) {
                        let mut parts = vec![t.to_string()];
                        let mut j = i + 1;
                        while j < lines.len()
                            && ind(lines[j]) == ind(l)
                            && super::a11y_plain_text(lines[j].trim_start()).is_some()
                        {
                            parts.push(
                                super::a11y_plain_text(lines[j].trim_start())
                                    .unwrap()
                                    .to_string(),
                            );
                            j += 1;
                        }
                        out.push(format!("{}- {}", " ".repeat(ind(l)), parts.join(" ")));
                        i = j;
                        continue;
                    }
                    out.push(l.to_string());
                    i += 1;
                }
                out.join("\n")
            }),
        ];

        let mut totals: Vec<(String, usize)> = candidates
            .iter()
            .map(|(n, _)| ((*n).to_string(), 0))
            .collect();
        let mut base_total = 0usize;
        for f in &files {
            let snap = std::fs::read_to_string(f).expect("read");
            let compacted = compact_a11y_tree(&snap);
            let base = tokenizer::estimate(&compacted);
            base_total += base;
            for (k, (name, transform)) in candidates.iter().enumerate() {
                let after = tokenizer::estimate(&transform(&compacted));
                totals[k].1 += after;
                println!(
                    "{:<34} {name:<24} {base:>7} -> {after:>7} ({:+.1}%)",
                    f.file_name().unwrap().to_string_lossy(),
                    -100.0 * (base as f64 - after as f64) / base as f64
                );
            }
        }
        println!("--- corpus, on top of the {base_total} tokens shipped today ---");
        for (name, after) in totals {
            println!(
                "{name:<24} {after:>8}  ({:.1}% further)",
                100.0 * (base_total - after) as f64 / base_total as f64
            );
        }
    }

    /// A nameless image announces nothing and can be acted on by nobody.
    ///
    /// Counted across the four snapshot fixtures: 77 nameless images and 33
    /// punctuation separators that reached the tree as text nodes.
    #[test]
    fn a_nameless_image_and_a_separator_are_dropped() {
        let snap = "- main:\n  - img\n  - \u{b7}\n  - text \"kept\"\n";
        let out = compact_a11y_tree(snap);
        assert!(!out.contains("img"), "nameless img kept: {out}");
        assert!(!out.contains('\u{b7}'), "separator kept: {out}");
        assert!(out.contains("kept"), "content lost: {out}");
    }

    /// The alt text IS the content.
    #[test]
    fn a_named_image_survives() {
        let snap = "- main:\n  - img \"Wikipedia logo\"\n";
        let out = compact_a11y_tree(snap);
        assert!(out.contains("Wikipedia logo"), "alt text lost: {out}");
    }

    /// A ref is what `browser_click` targets, and an element with no
    /// accessible name can ONLY be addressed by its ref.
    ///
    /// The `[cursor=pointer]` marker is load-bearing and this test learned it
    /// the hard way: a bare `[ref=e3]` is dropped by
    /// `drop_refs_the_agent_cannot_click` long after this pass, so a fixture
    /// without the marker proves nothing about this rule.
    #[test]
    fn a_clickable_nameless_image_survives() {
        let snap = "- main:\n  - img [ref=e3] [cursor=pointer]\n";
        let out = compact_a11y_tree(snap);
        assert!(out.contains("[ref=e3]"), "addressable img dropped: {out}");
    }

    /// An image whose ref turns out to be unclickable is still contentless.
    ///
    /// `drop_refs_the_agent_cannot_click` is spent LAST by design: a ref is
    /// what makes two otherwise identical lines distinct, so removing them
    /// earlier merges nodes that were never the same. The consequence is that
    /// it CREATES contentless nodes after the pass that removes them has run
    /// — `- img [ref=e9]` carries a ref while `drop_empty_wrapper_nodes`
    /// looks at it and comes out `- img` once e9 is judged unclickable.
    /// Measured on the github fixture: 55 bare images survived the whole
    /// chain for exactly this reason.
    #[test]
    fn an_image_whose_ref_is_unclickable_does_not_survive_as_a_bare_role() {
        // Only e2 is marked clickable, so e9 is stripped at the end.
        let snap = "- main:\n  - button [ref=e2] [cursor=pointer]\n  - img [ref=e9]\n";
        let out = compact_a11y_tree(snap);
        assert!(
            !out.lines().any(|l| l.trim() == "- img"),
            "a bare image survived the ref judgement: {out}"
        );
        assert!(
            out.contains("[ref=e2]"),
            "the clickable button was lost: {out}"
        );
    }

    /// A malformed `img` that carries children keeps them — because the
    /// caller re-parents, not because this pass spares it.
    #[test]
    fn a_nameless_image_that_has_children_keeps_them() {
        let snap = "- main:\n  - img:\n    - text \"caption\"\n    - text \"credit\"\n";
        let out = compact_a11y_tree(snap);
        assert!(
            out.contains("caption") && out.contains("credit"),
            "children stranded or lost: {out}"
        );
    }

    /// Containers are decided by the wrapper list, not here. TWO children on
    /// purpose: a single-child container is collapsed by a different rule.
    #[test]
    fn a_container_with_children_is_not_a_contentless_node() {
        let snap = "- list:\n  - listitem:\n    - text \"one\"\n    - text \"two\"\n";
        let out = compact_a11y_tree(snap);
        assert!(
            out.contains("one") && out.contains("two"),
            "content lost: {out}"
        );
        assert!(out.contains("listitem"), "container dropped: {out}");
    }

    #[test]
    fn a11y_preserves_refs_for_interaction() {
        // Refs are interaction handles (browser_click targets a [ref=eN]); a
        // nameless icon button can ONLY be addressed by ref, so a CLICKABLE
        // role must keep its ref whatever else is stripped.
        //
        // This test shipped asserting that a `heading` keeps its ref too. That
        // stopped being true when `drop_refs_the_agent_cannot_click` landed —
        // deliberately: `heading` and `main` are on A11Y_NOT_CLICKABLE, where a
        // ref is a handle to nothing and costs tokens on every line. The
        // attribute was removed from this function rather than the expectation
        // updated, which silenced it (a `mod tests` fn with no `#[test]` never
        // runs, and `cargo build --release` does not compile test code, so the
        // release gate could not see it either). Restored, asserting the
        // contract the code actually has.
        let snap = "- main [ref=e1]:\n  - heading \"X\" [level=1] [ref=e2]\n  - button [ref=e3]\n";
        let out = compact_a11y_tree(snap);
        assert!(
            out.contains("[ref=e3]"),
            "nameless button ref kept (only handle): {out}"
        );
        assert!(
            !out.contains("[ref=e2]"),
            "a heading is not a click target — its ref is dead weight: {out}"
        );
        assert!(out.contains("\"X\""));
    }

    #[test]
    fn a11y_folds_long_link_runs() {
        let mut snap = String::from("- navigation:\n");
        for i in 0..10 {
            let _ = writeln!(snap, "  - link \"L{i}\"");
        }
        let out = compact_a11y_tree(&snap);
        assert!(out.contains("more links"));
        // First three preserved
        assert!(out.contains("L0") && out.contains("L1") && out.contains("L2"));
    }

    // ---- Adversarial: aggressive structural compaction --------------------

    #[test]
    fn a11y_drops_empty_generic_wrappers() {
        let snap = "- generic [ref=e1]:\n  - generic [ref=e2]:\n    - heading \"Title\" [ref=e3]\n";
        let out = compact_a11y_tree(snap);
        // The two empty wrappers should be gone, but the heading remains.
        assert!(out.contains("Title"));
        assert!(
            !out.contains("- generic"),
            "empty generic wrappers not dropped:\n{out}"
        );
    }

    #[test]
    fn a11y_keeps_named_wrappers() {
        // `generic "main content"` carries a name — keep it.
        let snap = "- generic \"main content\":\n  - text \"hello\"\n";
        let out = compact_a11y_tree(snap);
        assert!(out.contains("\"main content\""));
    }

    #[test]
    fn a11y_strips_attribute_noise() {
        let snap = "- heading \"X\" [level=2] [expanded=true]\n- button \"Click\" [pressed=false] [disabled=true]\n";
        let out = compact_a11y_tree(snap);
        assert!(out.contains("\"X\""));
        assert!(out.contains("\"Click\""));
        assert!(!out.contains("[level="));
        assert!(!out.contains("[expanded="));
        assert!(!out.contains("[pressed="));
        assert!(!out.contains("[disabled="));
    }

    #[test]
    fn a11y_strips_role_prefixes() {
        let snap = "- paragraph: Hello world\n- listitem: Bullet text\n- text: plain";
        let out = compact_a11y_tree(snap);
        assert!(out.contains("Hello world"));
        assert!(out.contains("Bullet text"));
        assert!(out.contains("plain"));
        assert!(!out.contains("paragraph:"));
        assert!(!out.contains("listitem:"));
    }

    #[test]
    fn a11y_children_move_up_when_their_wrapper_goes() {
        // Indentation IS the tree. A dropped wrapper must hand its slot to its
        // children, or they read as children of the sibling above them.
        let snap = "- generic [ref=e1]:\n  - link \"A\" [ref=e2]:\n    - /url: /a\n  - banner [ref=e3]:\n    - heading \"B\" [ref=e4]\n";
        let out = compact_a11y_tree(snap);
        let banner = out
            .lines()
            .find(|l| l.contains("banner"))
            .expect("banner kept");
        let link = out.lines().find(|l| l.contains("link")).expect("link kept");
        assert_eq!(
            banner.len() - banner.trim_start().len(),
            link.len() - link.trim_start().len(),
            "banner and link were siblings and must stay siblings:\n{out}"
        );
    }

    #[test]
    fn a_link_carrying_only_its_address_becomes_one_line() {
        let snap =
            "- list:\n  - listitem:\n    - link \"Donate\" [ref=e2]:\n      - /url: /wiki/Donate\n";
        let out = compact_a11y_tree(snap);
        let l = out
            .lines()
            .find(|l| l.contains("Donate\""))
            .expect("link kept");
        assert!(l.contains("/url: /wiki/Donate"), "address lost: {out}");
        assert_eq!(
            out.lines().filter(|l| l.contains("/url")).count(),
            1,
            "the address must not also survive on its own line:\n{out}"
        );
        assert!(!out.contains("listitem"), "lone item wrapper kept: {out}");
        assert!(out.contains("- list:"), "the list itself must stay: {out}");
    }

    #[test]
    fn a_node_with_more_than_its_address_keeps_the_nested_form() {
        // Indentation is what says these two belong to the link. Inlining the
        // address here would leave the image looking like the link's sibling.
        let snap = "- link \"Home\" [ref=e2]:\n  - /url: /home\n  - img \"logo\"\n";
        let out = compact_a11y_tree(snap);
        let url = out.lines().find(|l| l.contains("/url")).expect("url kept");
        assert!(
            url.trim_start().starts_with("- /url"),
            "address was inlined although the node has other children:\n{out}"
        );
    }

    #[test]
    fn a_list_item_holding_several_things_keeps_its_wrapper() {
        let snap = "- list:\n  - listitem:\n    - link \"A\" [ref=e2]\n    - text \"and\"\n  - listitem:\n    - link \"B\" [ref=e3]\n";
        let out = compact_a11y_tree(snap);
        assert_eq!(
            out.matches("listitem").count(),
            1,
            "only the multi-child item may keep its wrapper:\n{out}"
        );
    }

    #[test]
    fn an_outbound_link_keeps_the_host_the_page_does_not() {
        // Eliding every host turns an external link into what reads as an
        // internal path. Only the page's own origin may be elided.
        let mut snap = String::new();
        for i in 0..4 {
            let _ = writeln!(
                snap,
                "- link \"L{i}\":\n  - /url: https://example.com/page{i}"
            );
        }
        snap.push_str("- link \"Out\":\n  - /url: https://other.example.org/deep\n");
        let out = compact_a11y_tree(&snap);
        assert!(!out.contains("example.com"), "own origin elided:\n{out}");
        assert!(out.contains("/page0"), "own path kept:\n{out}");
        assert!(
            out.contains("https://other.example.org/deep"),
            "outbound link must stay recognisably outbound:\n{out}"
        );
    }

    #[test]
    fn a_citation_metadata_blob_is_not_page_content() {
        let snap = "- generic \"ctx_ver=Z39.88-2004&rft_val_fmt=info%3Aofi%2Ffmt%3Akev%3Amtx%3Abook&rft.btitle=Web+Accessibility\":\n  - text: cited\n- heading \"Real\"\n";
        let out = compact_a11y_tree(snap);
        assert!(!out.contains("ctx_ver"), "machine blob kept:\n{out}");
        assert!(out.contains("cited"), "its visible content must survive");
        assert!(out.contains("Real"));
    }

    #[test]
    fn a_one_by_one_table_is_page_layout_not_data() {
        let layout = "- table [ref=e1]:\n  - rowgroup [ref=e2]:\n    - row [ref=e3]:\n      - cell [ref=e4]:\n        - heading \"Content\" [ref=e5]\n";
        let out = compact_a11y_tree(layout);
        assert!(!out.contains("- table"), "layout scaffolding kept:\n{out}");
        assert!(out.contains("Content"));
        let data = "- table [ref=e1]:\n  - rowgroup [ref=e2]:\n    - row [ref=e3]:\n      - cell \"A\" [ref=e4]\n      - cell \"B\" [ref=e5]\n    - row [ref=e6]:\n      - cell \"1\" [ref=e7]\n      - cell \"2\" [ref=e8]\n";
        let out = compact_a11y_tree(data);
        assert!(
            out.contains("- table") && out.contains("- row"),
            "a real data table must keep its shape:\n{out}"
        );
    }

    #[test]
    fn a_name_computed_from_its_subtree_is_said_once() {
        // Playwright computes `button "Save"` FROM the text node inside it, so
        // the snapshot says "Save" twice.
        let snap = "- button \"Save\" [ref=e1]:\n  - Save\n";
        let out = compact_a11y_tree(snap);
        assert_eq!(out.matches("Save").count(), 1, "said twice:\n{out}");
        assert!(out.contains("[ref=e1]"), "the click handle must survive");
        assert!(
            !out.trim_end().ends_with(':'),
            "a node with no children must not promise any:\n{out}"
        );
    }

    #[test]
    fn a11y_large_snapshot_savings_structural() {
        // Realistic React-style snapshot: nested generics + per-leaf attrs.
        // Refs are now PRESERVED (interaction handles), so the win is purely
        // structural (dropped wrappers, attribute noise, role prefixes, folded
        // runs) — a solid >25% cut with every meaningful text intact.
        let mut snap = String::from("- generic [ref=e1]:\n");
        for section in 0..20 {
            let _ = write!(
                snap,
                "  - generic [ref=e{}]:\n    - generic [ref=e{}]:\n      - heading \"Section {section}\" [level=2] [ref=e{}] [expanded=true]\n      - button \"Act {section}\" [ref=b{section}]\n      - paragraph [ref=e{}]: This is the body text of section {section} explaining things in detail.\n      - generic [ref=e{}]:\n        - listitem [ref=e{}]: First bullet\n        - listitem [ref=e{}]: Second bullet\n        - listitem [ref=e{}]: Third bullet\n",
                section * 10 + 2, section * 10 + 3, section * 10 + 4,
                section * 10 + 5, section * 10 + 6, section * 10 + 7,
                section * 10 + 8, section * 10 + 9
            );
        }
        let (out, sav) = compact_a11y_tree_with_savings(&snap);
        // Quality: every section title must be in the output verbatim.
        for s in 0..20 {
            assert!(
                out.contains(&format!("Section {s}")),
                "lost heading for section {s}:\n{out}"
            );
        }
        assert!(
            sav.percent() > 25.0,
            "expected >25% structural savings (refs kept), got {:.1}%\n{out}",
            sav.percent()
        );
        // Every ref an agent could act on survives; the ones it could not are
        // gone. This snapshot carries one button per section and nothing else
        // that responds to a click — so exactly those twenty refs remain. The
        // regression guarded against is the old behaviour that stripped ALL
        // refs, interactive ones included.
        for s in 0..20 {
            assert!(
                out.contains(&format!("[ref=b{s}]")),
                "lost the button ref for section {s}:\n{out}"
            );
        }
        assert_eq!(
            out.matches("[ref=e").count(),
            0,
            "a heading, a paragraph and a list item are not click targets:\n{out}"
        );
    }

    #[test]
    fn a11y_never_inflates_tiny_input() {
        let snap = "- heading \"X\"\n";
        let (_, sav) = compact_a11y_tree_with_savings(snap);
        assert!(sav.tokens_after <= sav.tokens_before + 2);
    }

    // --- linkify_to_citations --------------------------------------------------

    #[test]
    fn linkify_fewer_than_3_links_is_passthrough() {
        let md = "See [foo](https://a.com) and [bar](https://b.com).";
        assert_eq!(linkify_to_citations(md), md);
    }

    #[test]
    fn linkify_three_links_builds_citation_section() {
        let md = "[alpha](https://one.com) [beta](https://two.com) [gamma](https://three.com)";
        let out = linkify_to_citations(md);
        assert!(out.contains("alpha[1]"), "inline citation missing: {out}");
        assert!(out.contains("beta[2]"), "inline citation missing: {out}");
        assert!(out.contains("gamma[3]"), "inline citation missing: {out}");
        assert!(
            out.contains("## Link targets"),
            "references section missing: {out}"
        );
        assert!(out.contains("[1] https://one.com"), "ref 1 missing: {out}");
        assert!(out.contains("[2] https://two.com"), "ref 2 missing: {out}");
        assert!(
            out.contains("[3] https://three.com"),
            "ref 3 missing: {out}"
        );
    }

    #[test]
    fn linkify_deduplicates_same_url() {
        let md = "[a](https://same.com) [b](https://other.com) [c](https://same.com)";
        let out = linkify_to_citations(md);
        // "same.com" always gets [1]; "other.com" gets [2].
        assert!(out.contains("a[1]"), "a should be [1]: {out}");
        assert!(out.contains("c[1]"), "c reuses [1] for same url: {out}");
        assert!(out.contains("b[2]"), "b should be [2]: {out}");
        let refs = out.split("## Link targets").nth(1).unwrap_or("");
        assert_eq!(
            refs.trim().lines().count(),
            2,
            "expected 2 unique refs: {out}"
        );
    }

    // --- exclude_selectors -----------------------------------------------------

    #[test]
    fn exclude_selectors_strips_tag() {
        let html = "<div>keep</div><script>remove</script><p>also keep</p>";
        let out = exclude_selectors(html, "script");
        assert!(
            !out.contains("remove"),
            "script content should be stripped: {out}"
        );
        assert!(
            out.contains("keep"),
            "non-targeted content must survive: {out}"
        );
    }

    #[test]
    fn exclude_selectors_strips_class() {
        let html = r#"<p class="ads">ad text</p><p>real text</p>"#;
        let out = exclude_selectors(html, ".ads");
        assert!(
            !out.contains("ad text"),
            "class-matched element must be removed: {out}"
        );
        assert!(
            out.contains("real text"),
            "unmatched element must survive: {out}"
        );
    }

    #[test]
    fn exclude_selectors_multiple_comma_separated() {
        let html = "<nav>nav content</nav><aside>aside content</aside><main>main content</main>";
        let out = exclude_selectors(html, "nav, aside");
        assert!(!out.contains("nav content"), "nav should be removed: {out}");
        assert!(
            !out.contains("aside content"),
            "aside should be removed: {out}"
        );
        assert!(out.contains("main content"), "main must survive: {out}");
    }

    #[test]
    fn extract_by_selector_counts_depth_and_matches_class_tokens() {
        // The whole reason this can't reuse the strip scan: an article body is
        // nested divs, and "first </div> wins" would return only "one".
        let html = r#"<body><div class="chrome">nav</div>
<div class="article-body extra"><p>one</p><div class="ad">x</div><p>two</p></div>
<div class="article-body-promo">promo</div></body>"#;
        let got = extract_by_selector(html, "div.article-body").expect("matches");
        assert!(
            got.contains("one") && got.contains("two"),
            "cut at first close: {got}"
        );
        assert!(
            !got.contains("nav") && !got.contains("promo"),
            "leaked siblings: {got}"
        );

        // Token-exact: `.article-body` must NOT match `class="article-body-promo"`.
        let only_promo = r#"<div class="article-body-promo">promo</div>"#;
        assert!(extract_by_selector(only_promo, "div.article-body").is_none());

        // `<div` must not match the opening of `<divider>`.
        let divider =
            r#"<divider class="article-body">x</divider><div class="article-body">y</div>"#;
        assert_eq!(
            extract_by_selector(divider, "div.article-body").as_deref(),
            Some("y")
        );

        // id selector, and a self-closing element is not a container.
        assert_eq!(
            extract_by_selector(r#"<section id="main">body</section>"#, "#main").as_deref(),
            Some("body")
        );
        assert!(extract_by_selector(r#"<div class="a"/>after"#, "div.a").is_none());

        // Truncated page: a partial body beats no body.
        let cut = r#"<div class="article-body"><p>begins"#;
        assert!(extract_by_selector(cut, "div.article-body").is_some_and(|s| s.contains("begins")));
        assert!(extract_by_selector("<p>nothing</p>", "div.article-body").is_none());
    }
}
#[cfg(test)]
mod the_delivered_markdown_is_tidied_once_at_the_exit {
    use super::*;

    /// MEASURED 2026-09-07 on the 39 real pages of `tests/fixtures/pages`:
    /// 302,027 → 298,078 tokens delivered, **−1.3%**, and ZERO fidelity drop
    /// across the 78 measurements of `web_fidelity` (39 pages × `<p>` text and
    /// structural text), checked against the baseline BEFORE regenerating it.
    #[test]
    fn blank_runs_collapse_outside_fences_and_survive_inside() {
        let md = "a\n\n\n\n\nb\n\n```\nx\n\n\n\ny\n```\n\n\n\nc\n";
        let out = tidy_delivered_markdown(md);
        assert!(
            out.starts_with("a\n\nb"),
            "blank lines were not collapsed: {out:?}"
        );
        assert!(
            out.contains("x\n\n\n\ny"),
            "an empty line INSIDE a code block is content: {out:?}"
        );
        assert!(out.ends_with("\n\nc\n"), "{out:?}");
    }

    #[test]
    fn an_anchor_that_repeats_the_text_goes_and_one_that_names_something_stays() {
        let out = tidy_delivered_markdown(
            "See [Configuration](#configuration) and [Return value](#return-value).\n\
             Aussi [voir ici](#limites-connues) et [Docs](https://example.org/docs).\n",
        );
        assert!(
            out.contains("See Configuration and Return value."),
            "the anchor that repeats the text must go: {out:?}"
        );
        assert!(
            out.contains("[voir ici](#limites-connues)"),
            "an anchor that NAMES another section says where it leads: {out:?}"
        );
        assert!(
            out.contains("[Docs](https://example.org/docs)"),
            "a link to another page is not concerned: {out:?}"
        );
    }

    /// The tidying must not invent a line ending: several callers compare
    /// outputs byte for byte.
    #[test]
    fn a_document_without_a_final_newline_keeps_none() {
        assert_eq!(tidy_delivered_markdown("abc"), "abc");
        assert_eq!(tidy_delivered_markdown("abc\n"), "abc\n");
    }

    /// It applies to all THREE returns of `html_to_markdown`, not only to the
    /// one in front of us — that is why it wraps the function instead of
    /// living in a converter.
    #[test]
    fn every_exit_of_html_to_markdown_passes_through_it() {
        let md = html_to_markdown(
            "<html><body><main><h1>T</h1><p>Assez de texte pour que l'échelle \
             d'extraction traite ceci comme un article et non comme une isolation \
             ratée, avec de quoi tenir la longueur minimale attendue.</p>\
             <p>Voir <a href=\"#configuration\">Configuration</a>.</p></main></body></html>",
        );
        assert!(
            md.contains("Voir Configuration."),
            "the normal output did not go through the tidying: {md}"
        );
        assert!(
            !md.contains("\n\n\n"),
            "three consecutive line breaks remain: {md:?}"
        );
    }
}
#[cfg(test)]
mod a_cms_body_is_html_in_a_string {
    use super::{carries_markup, harvest_embedded_prose};

    fn page(ld: &str, body: &str) -> String {
        format!(
            "<html><body><script type=\"application/ld+json\">{ld}</script><main>{body}</main></body></html>"
        )
    }

    #[test]
    fn a_block_element_is_markup_and_a_lone_chevron_is_not() {
        assert!(carries_markup("<p>Une phrase entière.</p>"));
        assert!(carries_markup("<ul><li>un</li><li>deux</li></ul>"));
        assert!(carries_markup("une ligne<br>une autre"));
        // The bound: a title can carry an angle bracket, and an emphasis in
        // the middle of a sentence does not need converting.
        assert!(!carries_markup("Comparer a < b puis b < c"));
        assert!(!carries_markup("un mot <em>en emphase</em> dans la phrase"));
    }

    #[test]
    fn a_markup_bearing_body_reaches_a_page_that_is_not_thin() {
        let ld = r#"{"@type":"WebPage","description":"Un chapeau qui redit le balisage de la page.",
            "hasPart":[{"@type":"WebPageElement","text":"<p>Le corps de l'article commence ici et dit quelque chose.</p><ul><li>premier point</li><li>second point</li></ul>"}]}"#;
        let body = "<p>Un chapeau de plus de quatre cents caractères, pour que la page ne soit pas maigre. \
            Il faut du texte, et encore du texte, parce que le seuil de l'étage de secours est à quatre cents \
            caractères et que la porte du corps de CMS doit s'ouvrir SANS lui. Ce paragraphe existe pour cela \
            et pour rien d'autre, il occupe la place et il la tient jusqu'au bout de la mesure demandée, \
            très largement au-dessus du seuil, sans la moindre ambiguïté sur le côté de la borne où il tombe.</p>";
        let mined = harvest_embedded_prose(&page(ld, body), body);
        assert!(
            mined.contains("Le corps de l'article commence ici"),
            "CMS body not lifted: {mined:?}"
        );
        assert!(
            !mined.contains("Un chapeau qui redit"),
            "the standfirst was doubled: {mined:?}"
        );
        // Conversion, not stripping: two items, two lines.
        assert!(
            !mined.contains("premier pointsecond"),
            "list boundary glued back together: {mined:?}"
        );
    }

    #[test]
    fn structured_data_without_markup_adds_nothing_to_a_page_that_speaks() {
        let ld = r#"{"@type":"NewsArticle","headline":"Un titre qui redit celui de la page",
            "description":"Un resume qui redit le chapeau de la page et rien de plus."}"#;
        let body = "<p>Un chapeau de plus de quatre cents caractères, pour que la page ne soit pas maigre. \
            Il faut du texte, et encore du texte, parce que le seuil de l'étage de secours est à quatre cents \
            caractères et que la porte du corps de CMS doit rester FERMÉE ici. Ce paragraphe existe pour cela \
            et pour rien d'autre, il occupe la place et il la tient jusqu'au bout de la mesure demandée, \
            très largement au-dessus du seuil, sans la moindre ambiguïté sur le côté de la borne où il tombe.</p>";
        assert_eq!(harvest_embedded_prose(&page(ld, body), body), "");
    }

    #[test]
    fn a_thin_page_still_gets_its_blurbs() {
        // The original rescue tier: below the threshold, everything is lifted,
        // including what carries no tag.
        let ld = r#"{"@type":"NewsArticle","headline":"Un titre d'article assez long pour compter",
            "description":"Un resume qui est la seule chose lisible de cette page."}"#;
        let mined = harvest_embedded_prose(&page(ld, ""), "");
        assert!(mined.contains("# Un titre d'article"), "{mined:?}");
        assert!(mined.contains("Un resume qui est la seule"), "{mined:?}");
    }
}
#[cfg(test)]
mod an_origin_error_is_named_on_the_web_route {
    /// **`edge_error` had only ONE caller, and it was not `web`.**
    ///
    /// The module exists to prevent exactly this: "the agent then reads a
    /// gateway error as the article". It was wired to the hook that intercepts
    /// the agent's WebFetch, and not to the command the product recommends —
    /// so `tokenade web` on a `DEPLOYMENT_NOT_FOUND` page returned `# 404`
    /// followed by the marker, without a word. The same lesson, in the other
    /// direction.
    #[test]
    fn a_vercel_error_page_says_what_it_is() {
        let html = "<!doctype html><html><head><title>404</title></head>\
                    <body><h1>404</h1><p>DEPLOYMENT_NOT_FOUND</p></body></html>";
        let md = super::html_to_markdown(html);
        assert!(
            md.contains("[tokenade: EDGE ERROR — Vercel"),
            "the origin error is not named:\n{md}"
        );
        // The body stays: the banner EXPLAINS, it does not replace.
        assert!(md.contains("DEPLOYMENT_NOT_FOUND"), "{md}");
    }

    /// **The bound, and it holds at the ROUTE level.** An article that TALKS
    /// about 502 errors contains their words. The detector only fires on an
    /// ambiguous marker if the page is short; wiring it here must not undo
    /// that guard, and nothing else would have said so.
    #[test]
    fn an_article_about_gateway_errors_gets_no_banner() {
        let body = "<p>Quand un serveur intermédiaire ne joint pas l'origine, il répond \
                    Error 502 — c'est la signature d'une panne en amont et pas d'une URL \
                    fausse. Un 503 Service Unavailable dit autre chose : la machine est \
                    joignable mais refuse le travail, souvent parce qu'elle est saturée.</p>"
            .repeat(6);
        let html = format!(
            "<!doctype html><html><head><title>Comprendre les erreurs de passerelle</title>\
             </head><body><h1>Comprendre les erreurs de passerelle</h1>{body}</body></html>"
        );
        let md = super::html_to_markdown(&html);
        assert!(
            !md.contains("EDGE ERROR"),
            "an article ABOUT errors was taken for an error:\n{md:.400}"
        );
        assert!(md.contains("Error 502"), "the text moved:\n{md:.400}");
    }
}
