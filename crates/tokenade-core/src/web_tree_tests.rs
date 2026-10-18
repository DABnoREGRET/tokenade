// The tests of `crate::web_tree`, moved out of the production file.
//
// Mounted by `include!`, not by `mod`: the tokens are spliced INTO
// `web_tree`, so every `super::` keeps the meaning it had. Split by
// `srcsweep::production_lines`.

#[cfg(test)]
mod boilerplate_guards {
    use super::*;

    /// Filler that is unmistakably prose, so the misfire guard's ratios are
    /// driven by the block under test and not by fixture noise.
    fn prose(marker: &str, n: usize) -> String {
        (0..n)
            .map(|i| format!("{marker} sentence {i} about the subject at hand. "))
            .collect()
    }

    /// NEGATIVE CONTROL for every guard below: a real navigation block, with no
    /// bulk and no headings, must still be dropped. Every relaxation added here
    /// is only safe while this holds.
    #[test]
    fn a_real_nav_is_still_dropped() {
        let html = format!(
            "<html><body><div class=\"main-nav\"><ul><li><a href=\"/a\">NAVLINK Home</a></li>\
             <li><a href=\"/b\">NAVLINK About</a></li></ul></div>\
             <article><h1>Title</h1><p>{}</p></article></body></html>",
            prose("ARTICLE", 120)
        );
        let md = select_and_convert(&html, false);
        assert!(md.contains("ARTICLE sentence 0"), "article lost: {md:.400}");
        assert!(!md.contains("NAVLINK"), "real nav survived: {md:.400}");
    }

    /// A wrapper that holds most of the page is a LAYOUT container, not chrome,
    /// even when its class spells a keyword — django ships
    /// `class="container sidebar-right"` around 95% of the page's text.
    ///
    /// The assertion that matters is the SECOND one. Before the proportional
    /// guard the wrapper was dropped, the whole-document misfire guard saw the
    /// output collapse and fell back to the UNSKIPPED walk — so the article came
    /// back, but so did every piece of genuine chrome on the page. Refusing just
    /// the wrapper keeps boilerplate-skip alive for everything inside it.
    #[test]
    fn a_bulk_wrapper_named_like_a_sidebar_survives_and_still_prunes_inside() {
        let html = format!(
            "<html><body><div class=\"container sidebar-right\">\
             <h1>Title</h1><p>{}</p>\
             <div class=\"main-nav\"><ul><li><a href=\"/a\">NAVLINK Home</a></li>\
             <li><a href=\"/b\">NAVLINK About</a></li></ul></div>\
             </div><p>{}</p></body></html>",
            prose("ARTICLE", 120),
            prose("OUTSIDE", 5)
        );
        let md = select_and_convert(&html, false);
        assert!(
            md.contains("ARTICLE sentence 0"),
            "bulk wrapper dropped: {md:.400}"
        );
        assert!(
            !md.contains("NAVLINK"),
            "wrapper was refused but the nav inside it survived too — the \
             all-or-nothing fallback fired instead of the per-block guard: {md:.400}"
        );
    }

    /// The override is PROPORTIONAL, not absolute. A documentation sidebar is a
    /// real navigation bar that is tens of thousands of characters long
    /// (kubernetes.io's `td-sidebar-nav` measures 21 561), so a flat "over
    /// ~5 000 chars is content" rule would resurrect it. Only its share of the
    /// page separates it from a layout wrapper.
    #[test]
    fn a_huge_but_minority_sidebar_is_still_dropped() {
        let html = format!(
            "<html><body><div class=\"td-sidebar-nav\"><ul>{}</ul></div>\
             <article><h1>Title</h1><p>{}</p></article></body></html>",
            (0..200)
                .map(|i| format!("<li><a href=\"/p{i}\">TOCLINK page {i} of the guide</a></li>"))
                .collect::<String>(),
            prose("ARTICLE", 900)
        );
        let md = select_and_convert(&html, false);
        assert!(md.contains("ARTICLE sentence 0"), "article lost: {md:.400}");
        assert!(
            !md.contains("TOCLINK"),
            "a >5000-char sidebar survived — the guard is measuring absolute \
             size instead of proportion: {md:.400}"
        );
    }

    /// Short/ambiguous keywords are matched on token boundaries, so a class that
    /// merely CONTAINS one as a substring is untouched. Regression guard for the
    /// `desktop` / `top`, `download` / `ad`, `navigable` / `nav` family.
    #[test]
    fn a_keyword_as_a_bare_substring_does_not_match() {
        for class in [
            "desktop-toolbar",
            "download-cta",
            "navigable-region",
            "gradient",
        ] {
            let html = format!(
                "<html><body><div class=\"{class}\"><p>{}</p></div>\
                 <article><p>{}</p></article></body></html>",
                prose("KEPT", 20),
                prose("ARTICLE", 120)
            );
            let md = select_and_convert(&html, false);
            assert!(
                md.contains("KEPT sentence 0"),
                "class={class:?} matched as a substring: {md:.300}"
            );
        }
    }

    /// A Tailwind utility sits in the same class attribute as the semantic
    /// component name and can spell a keyword — sportingnews.com's
    /// `md:shadow-dropdown-menu` names a box-shadow, not a menu. A match on the
    /// utility alone must not condemn the element.
    #[test]
    fn a_utility_class_alone_does_not_condemn() {
        let html = format!(
            "<html><body><div class=\"story-card md:shadow-dropdown-menu \
             md:bg-desktop-menu-arrow md:[filter:brightness(0)]\"><p>{}</p></div>\
             <article><p>{}</p></article></body></html>",
            prose("KEPT", 20),
            prose("ARTICLE", 120)
        );
        let md = select_and_convert(&html, false);
        assert!(
            md.contains("KEPT sentence 0"),
            "a Tailwind utility condemned a story card: {md:.400}"
        );
    }

    /// …but a utility must not LAUNDER a real one either: the semantic token is
    /// still there and still decides.
    #[test]
    fn a_utility_class_does_not_rescue_a_real_menu() {
        let html = format!(
            "<html><body><div class=\"menu md:shadow-dropdown-menu\">\
             <ul><li><a href=\"/a\">NAVLINK Home</a></li></ul></div>\
             <article><p>{}</p></article></body></html>",
            prose("ARTICLE", 120)
        );
        let md = select_and_convert(&html, false);
        assert!(
            !md.contains("NAVLINK"),
            "utility laundered a real menu: {md:.400}"
        );
    }
}
#[cfg(test)]
mod depth_guard {
    use super::*;

    fn nested(levels: usize, inner: &str) -> String {
        format!(
            "<html><body>{}{inner}{}</body></html>",
            "<div>".repeat(levels),
            "</div>".repeat(levels)
        )
    }

    /// The cap must sit far above any real document. MEASURED: the deepest page
    /// in the fidelity corpus nests 34 levels (wikipedia_art.html), so content at
    /// 150 has to come through untouched.
    #[test]
    fn a_deep_but_realistic_document_is_not_truncated() {
        let md = select_and_convert(&nested(150, "<p>DEEP CONTENT</p>"), false);
        assert!(
            md.contains("DEEP CONTENT"),
            "cap truncated a realistic page: {md:.300}"
        );
    }

    /// The whole point: a document nested far past anything a browser would show
    /// must return, not abort the process. Without the cap this recurses ~12 000
    /// frames through `build_signatures`, `collect_drops` and `block` and
    /// overflows the stack — which kills the process outright and so cannot be
    /// caught or reported.
    #[test]
    fn a_pathologically_nested_document_does_not_blow_the_stack() {
        let html = nested(12_000, "<p>BURIED</p>");
        let md = select_and_convert(&html, false);
        // What it returns is unconstrained — surviving is the contract.
        let _ = md;
        // The fragment entry point walks a different tree and must be bounded too.
        let _ = fragment_to_markdown(&html);
    }

    /// The guard is RAII, so a full walk must leave the counter at zero — a leak
    /// would silently shrink the budget for every later page on this thread.
    #[test]
    fn the_depth_counter_unwinds_to_zero() {
        let _ = select_and_convert(&nested(150, "<p>x</p>"), false);
        assert_eq!(WALK_DEPTH.with(|d| d.get()), 0, "depth counter leaked");
    }
}
#[cfg(test)]
mod fidelity {
    use super::*;

    fn md(html: &str) -> String {
        select_and_convert(html, false)
    }

    /// **A word the page does not contain.**
    ///
    /// The two shapes measured on the corpus on 2026-09-09, in their original
    /// markup. A fixture must REACH the rule, so these are the real tags and
    /// not a reconstructed case.
    #[test]
    fn no_token_is_fabricated_by_joining_two_neighbours() {
        // ikea.com/gb/en/cat: the link holding the non-breaking space yields an
        // empty string and took the group's only space with it.
        let out = md(
            "<html><body><p>explore our matching<a href=\"/a\">\u{a0}</a>\
             <a href=\"/b\">desks</a></p></body></html>",
        );
        assert!(!out.contains("matchingdesks"), "{out}");
        assert!(out.contains("matching "), "{out}");

        // stackoverflow.com: the `aria-hidden` counter and its accessible copy,
        // which carries the number AND its label.
        let out = md(
            "<html><body><p><span title=\"196 silver badges\" aria-hidden=\"true\">\
             <span class=\"badgecount\">196</span></span>\
             <span class=\"v-visible-sr\">196 silver badges</span></p></body></html>",
        );
        assert!(!out.contains("196196"), "{out}");
        assert!(out.contains("196 silver badges"), "{out}");
    }

    /// **A relative target says which host it lives under.**
    #[test]
    fn a_relative_reference_names_its_origin() {
        let md = "du texte⟨1⟩\n\n## Link targets\n⟨1⟩ /news/2026/7/21/x\n".to_string();
        let out = note_the_origin_of_relative_references(md, "https://www.aljazeera.net/a/b");
        assert!(
            out.contains("## Link targets — relative entries are under https://www.aljazeera.net"),
            "{out}"
        );
        // The entry is intact: it is the HEADING that gets annotated, because
        // it is what survives the outline when the page is too big.
        assert!(out.contains("⟨1⟩ /news/2026/7/21/x"), "{out}");
    }

    /// **A description the neighbourhood already says, word for word.**
    ///
    /// The shapes come from the corpus, not from imagination: the first is
    /// `ikea.com` (the thumbnail carries the department name, written just
    /// below), the second `investopedia.com` (the alt is repeated in the link
    /// label).
    #[test]
    fn an_alt_its_neighbour_repeats_verbatim_loses_the_repetition() {
        let md = "[image: Food container sets]\nFood container sets^19\n".to_string();
        let out = drop_alts_their_neighbourhood_already_says(md);
        assert!(out.starts_with("[image]\n"), "{out}");
        // What the neighbour says stays: it is the repetition that goes.
        assert!(out.contains("Food container sets^19"), "{out}");

        let md = "- [\\[image: Merrick Bank Review\\] Merrick Bank Review Updated Jul 20](https://x.test/m)\n"
            .to_string();
        let out = drop_alts_their_neighbourhood_already_says(md);
        assert!(
            out.contains("[\\[image\\] Merrick Bank Review Updated"),
            "{out}"
        );
    }

    /// **The three tightenings, each paid for by reading the diff.**
    ///
    /// Each case here is a LOSS that an earlier version of the rule delivered,
    /// and that reading caught.
    #[test]
    fn three_things_that_look_already_said_and_are_not() {
        // 1. SCATTERED words are not a sentence said. slack.com lines up four
        //    different award badges; the fourth lost its name because its
        //    words were lying around.
        let md = "[image: Awarded Grid Leader in Small Business] and our Enterprise plan\n                  [image: Awarded Grid Leader in Enterprise]\n"
            .to_string();
        let out = drop_alts_their_neighbourhood_already_says(md.clone());
        assert_eq!(out, md, "scattered words were taken for a sentence");

        // 2. A word that is ONLY in a URL is not said. newegg.com:
        //    `[\[image: See Deals from Intel\]](…/Intel/BrandStore/…)`.
        let md =
            "[\\[image: See Deals from Intel\\]](https://x.test/Intel/BrandStore/Deals/from)\n"
                .to_string();
        let out = drop_alts_their_neighbourhood_already_says(md.clone());
        assert_eq!(out, md, "a URL word was read as neighbouring text");

        // 3. And even when said, a description that is its link's ONLY label
        //    stays: emptying it leaves a link of which nothing says where it
        //    leads any more.
        let md = "See Deals from Intel\n[\\[image: See Deals from Intel\\]](https://x.test/i)\n"
            .to_string();
        let out = drop_alts_their_neighbourhood_already_says(md.clone());
        assert_eq!(out, md, "a link's only label was emptied");
    }

    /// **The news card: link, empty line, heading.**
    ///
    /// The shape comes from the corpus — `latercera.com` writes it 73 times on
    /// its home page — and it is the example this function's comment cited
    /// without ever reaching it: the neighbour below was the EMPTY line, and
    /// the heading two lines away.
    #[test]
    fn a_press_card_says_its_headline_once() {
        let md = "[\\[image: Funcionaria PDI enfrenta a asaltantes\\]](/n/funcionaria-pdi)\n\n## Funcionaria PDI enfrenta a asaltantes⟨1⟩\n"
            .to_string();
        let out = drop_alts_their_neighbourhood_already_says(md);
        assert!(
            out.starts_with("[\\[image\\]](/n/funcionaria-pdi)"),
            "the repetition stayed: {out}"
        );
        // The address stays: it is the description that goes, not the link.
        assert!(out.contains("(/n/funcionaria-pdi)"), "{out}");
        assert!(
            out.contains("## Funcionaria PDI enfrenta a asaltantes⟨1⟩"),
            "{out}"
        );
    }

    /// Both sides of the new bound.
    #[test]
    fn a_heading_that_says_something_else_keeps_the_description() {
        // The neighbouring heading does NOT say the sentence: nothing names the
        // destination, the description stays.
        let md = "[\\[image: Funcionaria PDI enfrenta a asaltantes\\]](/n/a)\n\n## Otra noticia distinta⟨1⟩\n"
            .to_string();
        let out = drop_alts_their_neighbourhood_already_says(md.clone());
        assert_eq!(out, md, "a description was emptied without being repeated");

        // And the sole-label guard still holds when the repetition is NOT a
        // heading: a line of prose that mentions the sentence does not say
        // where the link leads.
        let md =
            "See Deals from Intel today\n\n[\\[image: See Deals from Intel\\]](https://x.test/i)\n"
                .to_string();
        let out = drop_alts_their_neighbourhood_already_says(md.clone());
        assert_eq!(out, md, "a link's only label was emptied");
    }

    #[test]
    fn a_blank_line_is_not_a_neighbour_in_either_direction() {
        // The heading ABOVE, separated by a blank line.
        let md = "## Food container sets⟨3⟩\n\n[\\[image: Food container sets\\]](/c/food)\n"
            .to_string();
        let out = drop_alts_their_neighbourhood_already_says(md);
        assert!(out.contains("[\\[image\\]](/c/food)"), "{out}");

        // At the document's edges, there is no neighbour at all.
        assert_eq!(super::nearest_prose_line(&["a", "b"], 0, -1), "");
        assert_eq!(super::nearest_prose_line(&["a", "b"], 1, 1), "");
        assert_eq!(super::nearest_prose_line(&["a", "", "", "b"], 0, 1), "b");
    }

    #[test]
    fn an_atx_heading_needs_its_space() {
        assert!(super::is_a_heading("# t"));
        assert!(super::is_a_heading("###### t"));
        assert!(!super::is_a_heading("####### t"));
        assert!(!super::is_a_heading("#t"));
        assert!(!super::is_a_heading("t # u"));
    }

    /// A link back INTO the page is its label: `#cite_ref-1` leads nowhere
    /// for the reader of the delivered text.
    #[test]
    fn a_same_page_anchor_is_its_label() {
        let filler =
            "Un paragraphe de remplissage assez long pour passer le plancher de deux cents \
            caractères du premier étage, et donc pour que la page soit bien rendue par l'arbre.";
        let html = format!(
            "<html><body><main><p>{filler}</p><ol><li><a href=\"#cite_ref-1\">↑</a> \
             \"Supported printers\". Retrieved 2018.</li></ol></main></body></html>"
        );
        let md = crate::web_tree::fragment_to_markdown(&html);
        assert!(md.contains("Supported printers"), "{md}");
        assert!(!md.contains("#cite_ref"), "{md}");
    }

    /// **`nav-item` names an ENTRY, not a bar.**
    ///
    /// The shape comes from `slack.com`, whose carousel of product promises is
    /// built from `c-switcher-nav-item__heading-row`.
    #[test]
    fn a_component_named_nav_item_is_not_a_navigation_bar() {
        let filler = "Un paragraphe de remplissage assez long pour passer le plancher de \
            deux cents caractères du premier étage, et donc pour que la règle de décor soit \
            bien celle qui tranche ici, sans la moindre ambiguïté sur le chemin emprunté.";
        let page = |class: &str| {
            format!(
                "<html><body><main><p>{filler}</p>\
                 <div class=\"{class}\"><p><strong>Summarize a conversation you missed</strong></p></div>\
                 </main></body></html>"
            )
        };
        // Neither of these two names carries ANOTHER scenery word: otherwise
        // the block would go for another reason and the test would prove
        // nothing (`sidebar-nav-link` failed on exactly that while writing it).
        for kept in ["c-switcher-nav-item__heading-row", "carousel-nav-link"] {
            let md = crate::web::html_to_markdown(&page(kept));
            assert!(
                md.contains("Summarize a conversation"),
                "`{kept}` was taken for a navigation bar: {md:.300}"
            );
        }
        // The other side of the bound: a real bar still goes, whether `nav` is
        // alone, at the head of a compound or at its tail.
        for dropped in [
            "nav",
            "nav nav-tabs",
            "tab-nav",
            "left-nav",
            "nav-x-body-top-bar",
        ] {
            let md = crate::web::html_to_markdown(&page(dropped));
            assert!(
                !md.contains("Summarize a conversation"),
                "`{dropped}` is no longer recognised as a bar: {md:.300}"
            );
        }
    }

    /// Adjacency is read WITHIN a class name, not across the space separating
    /// two: `class="… nav" id="item…"` stays a bar.
    #[test]
    fn adjacency_does_not_cross_a_class_boundary() {
        assert!(names_one_entry_of_a_navigation("nav", Some("item")));
        assert!(names_one_entry_of_a_navigation("nav", Some("link")));
        assert!(!names_one_entry_of_a_navigation("nav", Some("tabs")));
        assert!(!names_one_entry_of_a_navigation("nav", None));
        assert!(!names_one_entry_of_a_navigation("menu", Some("item")));
    }

    /// **The ARIA role is worth the element it is the equivalent of.**
    #[test]
    fn a_role_stands_for_the_element_it_names() {
        let page = |attrs: &str| {
            format!(
                "<html><body><main><p>Une phrase de corps assez longue pour que la page \
                 ne soit pas jugée maigre, et qui tient largement au-dessus du plancher \
                 de deux cents caractères que l'échelle d'extraction impose au premier \
                 étage avant de relâcher ses règles de décor.</p>\
                 <div {attrs}><p>Aller a la section suivante de ce document</p></div>\
                 </main></body></html>"
            )
        };
        for dropped in [
            "role=\"navigation\"",
            "role=\"banner\"",
            "role=\"contentinfo\"",
            "role=\"complementary\"",
            "role=\"search\"",
            // The attribute is a LIST in order of preference: the first counts.
            "role=\"navigation menubar\"",
            "role=\"NAVIGATION\"",
        ] {
            let md = crate::web::html_to_markdown(&page(dropped));
            assert!(
                !md.contains("section suivante"),
                "`{dropped}` was not discarded: {md:.300}"
            );
        }
        // The other side: a role without a discarded equivalent decides
        // nothing, and neither does an absent `role`.
        for kept in [
            "role=\"main\"",
            "role=\"region\"",
            "role=\"article\"",
            "class=\"x\"",
        ] {
            let md = crate::web::html_to_markdown(&page(kept));
            assert!(
                md.contains("section suivante"),
                "`{kept}` was taken for scenery: {md:.300}"
            );
        }
    }

    /// The table carries no new judgement: it is DERIVED from the exclusion
    /// list, and this test checks it rather than promising it.
    #[test]
    fn every_role_maps_onto_an_element_already_dropped() {
        let doc = scraper::Html::parse_fragment("<div role=\"navigation\"></div>");
        let div = doc
            .tree
            .nodes()
            .find(|n| el_name(*n) == Some("div"))
            .expect("the div");
        for role in [
            "navigation",
            "banner",
            "contentinfo",
            "complementary",
            "search",
            "form",
        ] {
            let html = format!("<div role=\"{role}\"></div>");
            let d = scraper::Html::parse_fragment(&html);
            let node = d.tree.nodes().find(|n| el_name(*n) == Some("div")).unwrap();
            let implied = element_a_landmark_role_stands_for(node)
                .unwrap_or_else(|| panic!("`{role}` is no longer in the table"));
            assert!(
                is_dropped(implied),
                "`{role}` -> `{implied}`, which is not dropped"
            );
        }
        assert_eq!(element_a_landmark_role_stands_for(div), Some("nav"));
    }

    /// **The `heading` role and its level, on both sides of each bound.**
    #[test]
    fn a_declared_heading_follows_the_level_the_page_gives() {
        let page = |div: &str| {
            format!(
                "<html><body><main><p>Une phrase de corps assez longue pour que la page ne \
                 soit pas jugée maigre, et qui tient largement au-dessus du plancher de deux \
                 cents caractères que l'échelle d'extraction impose au premier étage.</p>\
                 {div}</main></body></html>"
            )
        };
        let heading = |level: &str| {
            format!("<div role=\"heading\"{level}><button>Materials and care</button></div>")
        };
        for (attr, expected) in [
            (" aria-level=\"1\"", "# Materials and care"),
            (" aria-level=\"3\"", "### Materials and care"),
            (" aria-level=\"6\"", "###### Materials and care"),
            // ARIA allows a deeper level than markdown: clamped rather than
            // refused, a too-deep heading being better than a lost one.
            (" aria-level=\"9\"", "###### Materials and care"),
            (" aria-level=\"0\"", "# Materials and care"),
            (" aria-level=\"x\"", "## Materials and care"),
            // Without `aria-level`, the specification's default is 2.
            ("", "## Materials and care"),
        ] {
            let md = crate::web::html_to_markdown(&page(&heading(attr)));
            assert!(
                md.lines().any(|l| l == expected),
                "`{attr}` did not give `{expected}`: {md:.300}"
            );
        }

        // The other side: a role that is not `heading` does not fabricate one,
        // and a region that ALREADY carries a real heading keeps its own
        // rather than receiving a second one.
        let md = crate::web::html_to_markdown(&page(
            "<div role=\"region\" aria-level=\"3\"><p>Materials and care</p></div>",
        ));
        assert!(
            !md.contains("### Materials"),
            "a heading was invented: {md:.300}"
        );
        let md = crate::web::html_to_markdown(&page(
            "<div role=\"heading\" aria-level=\"3\"><h2>Materials and care</h2></div>",
        ));
        assert!(
            md.lines().any(|l| l == "## Materials and care"),
            "the real heading was replaced: {md:.300}"
        );
        assert!(!md.contains("### Materials"), "heading doubled: {md:.300}");
    }

    #[test]
    fn a_marker_is_folded_onto_one_line_and_nothing_else() {
        // With no whitespace to fold, the string goes out as-is — the case of
        // the overwhelming majority of `alt`s.
        assert_eq!(
            super::one_line("Une description simple"),
            "Une description simple"
        );
        // Line break, tab, continuation indent: a single space, and the
        // sentence stays whole.
        assert_eq!(
            super::one_line("Two tables: the first\n    contains s1 on the\tstack"),
            "Two tables: the first contains s1 on the stack"
        );
        // The fold ADDS nothing and cuts nothing.
        assert_eq!(super::one_line(""), "");
        assert_eq!(super::one_line("a\n\n\nb"), "a b");
    }

    /// **The frame where the document lives is a fact, not a hypothesis.**
    #[test]
    fn a_declared_frame_is_named_and_resolved() {
        let page = |src: &str| {
            format!("<html><head><title>x</title></head><body><nav>a</nav><iframe src=\"{src}\"></iframe></body></html>")
        };
        assert_eq!(
            declared_frame_url(&page("https://contenu.test/vrai"), "https://portail.test/p"),
            Some("https://contenu.test/vrai".to_string())
        );
        // Resolved against the served address, like a relative reference.
        assert_eq!(
            declared_frame_url(&page("/vrai"), "https://portail.test/a/b"),
            Some("https://portail.test/vrai".to_string())
        );
        // And against the BASE when one is declared.
        let with_base = "<html><head><base href=\"https://cdn.test/v2/\"></head><body><iframe src=\"doc\"></iframe></body></html>";
        assert_eq!(
            declared_frame_url(with_base, "https://portail.test/p"),
            Some("https://cdn.test/v2/doc".to_string())
        );
        // A frameset's `<frame>` counts too.
        let frameset = "<html><frameset><frame src=\"/haut\"></frameset></html>";
        assert_eq!(
            declared_frame_url(frameset, "https://vieux.test/i"),
            Some("https://vieux.test/haut".to_string())
        );
    }

    /// And nothing when the frame leads nowhere: the first one that LEADS
    /// somewhere wins.
    #[test]
    fn a_frame_that_leads_nowhere_is_not_named() {
        for src in ["", "about:blank", "javascript:void(0)", "data:text/html,x"] {
            let only = format!("<html><body><iframe src=\"{src}\"></iframe></body></html>");
            assert_eq!(
                declared_frame_url(&only, "https://p.test/x"),
                None,
                "src={src}"
            );
        }
        assert_eq!(
            declared_frame_url("<html><body><p>rien</p></body></html>", "https://p.test/x"),
            None
        );
        // An `about:blank` ad before the real frame does not hide it.
        let two = "<html><body><iframe src=\"about:blank\"></iframe><iframe src=\"/vrai\"></iframe></body></html>";
        assert_eq!(
            declared_frame_url(two, "https://p.test/x"),
            Some("https://p.test/vrai".to_string())
        );
    }

    /// A frame nobody sees is not the document: Google Tag Manager's tracking
    /// pixel (under `<noscript>`, 0×0, hidden) was announced as a vendor
    /// site's frame. Each mark alone is enough; the real frame that follows
    /// stays named.
    #[test]
    fn a_frame_nobody_sees_is_not_named() {
        for unseen in [
            r#"<noscript><iframe src="https://www.googletagmanager.com/ns.html?id=GTM-X"></iframe></noscript>"#,
            r#"<iframe src="/pixel" width="0"></iframe>"#,
            r#"<iframe src="/pixel" height="1px"></iframe>"#,
            r#"<iframe src="/pixel" style="display: none"></iframe>"#,
            r#"<iframe src="/pixel" style="visibility:hidden"></iframe>"#,
            r#"<iframe hidden src="/pixel"></iframe>"#,
        ] {
            let page = format!(
                "<html><body>{unseen}<iframe src=\"/vrai\" width=\"800\"></iframe></body></html>"
            );
            assert_eq!(
                declared_frame_url(&page, "https://p.test/x"),
                Some("https://p.test/vrai".to_string()),
                "{unseen}"
            );
        }
    }

    /// **`<base href>` wins over the page address.**
    #[test]
    fn a_declared_base_wins_over_the_page_address() {
        let abs =
            "<html><head><base href=\"https://cdn.exemple.test/v2/\"></head><body>x</body></html>";
        assert_eq!(
            declared_base_href(abs, "https://www.exemple.test/page"),
            Some("https://cdn.exemple.test/v2/".to_string())
        );
        // A RELATIVE base is legal and resolves against the served address.
        let rel = "<html><head><BASE HREF='/v2/'></head><body>x</body></html>";
        assert_eq!(
            declared_base_href(rel, "https://www.exemple.test/a/b"),
            Some("https://www.exemple.test/v2/".to_string())
        );
        // Without quotes, and with no base at all.
        let bare = "<html><head><base href=https://x.test/ ></head></html>";
        assert_eq!(
            declared_base_href(bare, "https://y.test/p"),
            Some("https://x.test/".to_string())
        );
        assert_eq!(
            declared_base_href("<html><head></head></html>", "https://y.test/p"),
            None
        );
        // Outside `<head>`, it does not count: the spec only reads it there.
        let late = "<html><head></head><body><base href=\"https://z.test/\"></body></html>";
        assert_eq!(declared_base_href(late, "https://y.test/p"), None);
    }

    /// And silent in the three cases where there is nothing to say.
    /// **Below the references threshold, the origin was named nowhere.**
    ///
    /// The `## Link targets` table only exists from six links up; below that
    /// the addresses stay inline, and the origin guarantee therefore only held
    /// above a threshold nobody had stated. That is the case where the agent
    /// needs it most: a small page is often a documentation leaf, where
    /// nothing else says which host to resolve `../neighbour` under. MEASURED
    /// on the corpus: three pages receive it, including `django_leaf` and its
    /// **171 relative targets without any table**.
    #[test]
    fn a_small_page_with_inline_relative_targets_names_its_origin() {
        let md = "Voir [la suite](/chapitre-2) et [l'onglet](?tab=x).".to_string();
        let out = note_the_origin_of_relative_references(md, "https://exemple.test/une/page");
        assert!(
            out.starts_with("[tokenade: relative links below resolve under https://exemple.test]"),
            "got:\n{out}"
        );
        assert!(
            out.contains("[la suite](/chapitre-2)"),
            "the address moved:\n{out}"
        );
    }

    /// Three bounds, each with its reason. A fragment resolves within the page
    /// itself. A local file is distilled under an invented origin, and
    /// announcing it would be worse than staying quiet. And a page whose
    /// HEADING already carries the announcement does not receive it twice.
    #[test]
    fn the_inline_note_stays_silent_where_it_would_lie() {
        let frag = "Voir [plus bas](#install), rien d'autre.".to_string();
        assert_eq!(
            note_the_origin_of_relative_references(frag.clone(), "https://exemple.test/p"),
            frag,
            "a fragment needs no origin"
        );
        let rel = "Voir [la suite](/chapitre-2).".to_string();
        assert_eq!(
            note_the_origin_of_relative_references(rel.clone(), "https://example.invalid/x"),
            rel,
            "an invented origin is not announced"
        );
        let already = format!(
            "Corps [x](/y)\n\n{REFERENCES_HEADING} — relative entries are under https://exemple.test\n⟨1⟩ /y\n"
        );
        let out = note_the_origin_of_relative_references(already, "https://exemple.test/p");
        assert_eq!(
            out.matches("relative links below resolve").count(),
            0,
            "announcement doubled:\n{out}"
        );
    }

    #[test]
    fn nothing_is_said_without_a_relative_entry_or_an_origin() {
        let absolute = "## Link targets\n⟨1⟩ https://x.test/a\n".to_string();
        assert_eq!(
            note_the_origin_of_relative_references(absolute.clone(), "https://x.test/p"),
            absolute
        );
        let no_table = "juste du texte\n".to_string();
        assert_eq!(
            note_the_origin_of_relative_references(no_table.clone(), "https://x.test/p"),
            no_table
        );
        // A local file's stand-in is not announced as an origin.
        let local = "## Link targets\n⟨1⟩ /a/b\n".to_string();
        assert_eq!(
            note_the_origin_of_relative_references(local.clone(), "https://example.invalid/"),
            local
        );
        assert_eq!(
            note_the_origin_of_relative_references(local.clone(), "pas une url"),
            local
        );
    }

    /// **Two siblings that CSS separates do not weld.**
    ///
    /// The two shapes measured on 2026-09-09, and the counter-example that
    /// bounds the rule — without it, the rule would break the word it claims
    /// to repair.
    /// **An `<aside>` is discarded by its NAME, except when it is the fact
    /// sheet.**
    ///
    /// On the REAL pages, because the rule was measured on them: a synthetic
    /// fixture of bare links gets pruned by another rule and says nothing
    /// about this one.
    ///
    /// Both conditions are necessary and the measurement says so: `k8s_doc`'s
    /// language menu weighs 22,695 characters and contains "repository" and
    /// "owners" somewhere inside. The word count alone would admit it; the
    /// density discards it.
    #[test]
    fn a_package_panel_survives_the_aside_drop_but_a_language_menu_does_not() {
        let page = |n: &str| {
            std::fs::read_to_string(format!(
                "{}/tests/fixtures/pages/{n}.html",
                env!("CARGO_MANIFEST_DIR")
            ))
            .expect("the corpus page")
        };
        let out = crate::web::html_to_markdown(&page("rubygems_pkg"));
        for fact in ["MIT", "Source Code", "Bug Tracker"] {
            assert!(
                out.contains(fact),
                "\"{fact}\" went with the aside: the fact sheet is not delivered"
            );
        }
        let out = crate::web::html_to_markdown(&page("k8s_doc"));
        assert!(
            !out.contains("বাংলা"),
            "the language menu passed for a package fact sheet"
        );
        // TWO words, not one: "homepage" alone is a sidebar word as much as a
        // fact-sheet word, and the Guardian's region banner carries it. That
        // is what the threshold keeps out.
        let out = crate::web::html_to_markdown(&page("guardian_home"));
        assert!(
            !out.contains("You are viewing the International homepage"),
            "a region banner passed for a package fact sheet"
        );
    }

    #[test]
    fn two_inline_siblings_do_not_weld_into_one_word() {
        // stripe.com: field name and type badge, two `<span>`s inside an
        // `<h4>`. 54 fabricated words on that page alone.
        let out = md("<html><body><h4><span>amount</span><span>integer</span></h4></body></html>");
        assert!(!out.contains("amountinteger"), "{out}");
        assert!(out.contains("amount integer"), "{out}");

        // ikea: the specifications table, label then value.
        let out = md("<html><body><p><span>Length</span><span>21 cm</span></p></body></html>");
        assert!(out.contains("Length 21 cm"), "{out}");

        // docs.rs/serde: an ELEMENT followed by TEXT, the two halves of a
        // single word. Separating here would render `de serializing`.
        let out = md(
            "<html><body><p>a framework for <em><strong>de</strong></em>serializing data</p></body></html>",
        );
        assert!(out.contains("deserializing"), "{out}");
        assert!(!out.contains("de serializing"), "{out}");
    }

    /// And the bound that keeps the twin lossless: a prefix followed by a
    /// space, never free containment.
    #[test]
    fn a_twin_carries_a_prefix_but_never_a_digit_of_another_number() {
        assert!(twin_carries("196 silver badges", "196"));
        assert!(twin_carries("196", "196"));
        // `1` in `2100` is not a twin, it is a digit of another number — the
        // cut-on-a-separator bound, mirrored.
        assert!(!twin_carries("2100", "1"));
        assert!(!twin_carries("1960 votes", "196"));
    }

    /// The list of "read by a screen reader" classes was an EQUALITY on seven
    /// names; the eighth (`v-visible-sr`) went by silently.
    #[test]
    fn the_hidden_classes_are_matched_by_stem_not_by_equality() {
        let hidden = |c: &str| {
            let h = scraper::Html::parse_fragment(&format!("<span class=\"{c}\">x</span>"));
            let sel = scraper::Selector::parse("span").unwrap();
            let n = h.select(&sel).next().unwrap();
            is_visually_hidden(*n)
        };
        for c in [
            "sr-only",
            "visually-hidden",
            "v-visible-sr",
            "u-sr-only",
            "screen-reader-text",
            "a11y-hidden",
        ] {
            assert!(hidden(c), "class not recognised: {c}");
        }
        for c in ["sidebar", "usr-name", "answer", "srcset-holder"] {
            assert!(!hidden(c), "false positive: {c}");
        }
    }

    /// Gluing a superscript to its base changes the VALUE the agent reads.
    #[test]
    fn a_superscript_digit_does_not_merge_into_the_number() {
        assert!(
            md("<p>4<sup>2</sup> = 16</p>").contains("4^2"),
            "{}",
            md("<p>4<sup>2</sup> = 16</p>")
        );
        assert!(md("<p>H<sub>2</sub>O</p>").contains("H_2O"));
        // A footnote marker stays visible instead of gluing to the word.
        assert!(md("<p>fact<sup>[1]</sup></p>").contains("fact^[1]"));
    }

    /// …but an alphabetic superscript is prose, and `1st` beats `1^st`.
    #[test]
    fn an_alphabetic_superscript_stays_glued() {
        let out = md("<p>the 1<sup>st</sup> of May</p>");
        assert!(out.contains("1st"), "{out}");
        assert!(!out.contains("1^st"), "{out}");
    }

    /// A destination that ends early hands the agent a truncated URL with no
    /// sign it was cut. Query strings carry spaces and parens routinely.
    #[test]
    fn a_destination_with_a_space_or_paren_survives_as_one_link() {
        let out = md(r#"<p><a href="https://x.test/a b?q=(1)">t</a></p>"#);
        assert!(out.contains("[t](<https://x.test/a b?q=(1)>)"), "{out}");
        // The ordinary case keeps the plain form — no gratuitous churn.
        let plain = md(r#"<p><a href="https://x.test/a">t</a></p>"#);
        assert!(plain.contains("[t](https://x.test/a)"), "{plain}");
    }

    /// A `]` in the label ends the label early, with the same consequence.
    #[test]
    fn a_bracket_in_the_label_cannot_end_the_link() {
        let out = md(r#"<p><a href="https://x.test/">a] b</a></p>"#);
        assert!(out.contains(r"[a\] b](https://x.test/)"), "{out}");
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    /// `select_and_convert` exactly as it was before F2: two unconditional full
    /// walks, ratio tested at 0.6. The A/B reference for the one-walk version.
    fn legacy_select_and_convert(html: &str, aggressive: bool) -> String {
        const SENTINEL: char = '\u{E057}';
        let swapped = html.replace('\x00', "\u{E057}");
        let doc = Html::parse_document(&swapped);
        let root = if aggressive {
            find_main_root(&doc).unwrap_or_else(|| doc.tree.root())
        } else {
            doc.tree.root()
        };
        let _dup = DupGuard::install(std::rc::Rc::new(if dedup_enabled() {
            crate::tree_dedup::duplicate_subtrees(doc.tree.root())
        } else {
            crate::tree_dedup::DropSet::new()
        }));
        let mut out = walk(root, true);
        let full = walk(root, false);
        if (vis_len(&out) as f64) < (vis_len(&full) as f64) * 0.6 {
            out = full;
        }
        out = trim_leading_residue(&out);
        if out.contains(SENTINEL) {
            out = out.replace(SENTINEL, "\x00");
        }
        out
    }

    /// F2 guardrail: dropping the second walk must leave the distilled output
    /// BYTE-IDENTICAL on every page of the 147-page corpus, both tiers.
    ///     cargo test -p tokenade-core --lib one_walk -- --ignored --nocapture
    #[test]
    #[ignore = "needs the local 147-page corpus"]
    fn one_walk_output_is_byte_identical_to_two_walks() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../experiments/cf-clearance/corpus");
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        let mut checked = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            let html = match std::fs::read_to_string(&path) {
                Ok(h) => h,
                Err(_) => continue,
            };
            for aggressive in [true, false] {
                assert_eq!(
                    select_and_convert(&html, aggressive),
                    legacy_select_and_convert(&html, aggressive),
                    "output diverged on {path:?} (aggressive={aggressive})"
                );
            }
            checked += 1;
        }
        assert!(checked > 0, "no corpus pages read from {dir:?}");
        println!("byte-identical on {checked} pages × 2 tiers");
    }

    /// Two header rows name the columns together; a short rowspan label is
    /// read in every row it covers; a long one is not copied.
    #[test]
    fn a_two_row_header_merges_and_a_rowspan_labels_each_row() {
        let html = r#"<table>
<tr><th rowspan="2">Backend</th><th colspan="2">Latency</th></tr>
<tr><th>p50</th><th>p99</th></tr>
<tr><td rowspan="2">postgres</td><td>5</td><td>22</td></tr>
<tr><td>6</td><td>25</td></tr>
<tr><td rowspan="2">a long note that spans two rows of this table and more</td><td>1</td><td>2</td></tr>
<tr><td>3</td><td>4</td></tr>
</table>"#;
        let md = fragment_to_markdown(html);
        assert!(
            md.contains("| Backend | Latency p50 | Latency p99 |\n| --- | --- | --- |\n"),
            "got:\n{md}"
        );
        assert!(
            md.contains("| postgres | 5 | 22 |\n| postgres | 6 | 25 |"),
            "got:\n{md}"
        );
        assert!(md.contains("|  | 3 | 4 |"), "a long cell was copied:\n{md}");
    }

    #[test]
    fn span_table_columns_align() {
        let html = r#"<table>
<tr><th colspan="2">Header spans 2</th></tr>
<tr><td rowspan="2">R1-2</td><td>B1</td></tr>
<tr><td>B2</td></tr>
</table>"#;
        let md = fragment_to_markdown(html);
        // Header row spans two columns; separator has two columns; the rowspan
        // cell pushes B2 into the second column of the last row — and labels
        // that row too: a spanned cell is read in every row it covers.
        assert!(md.contains("| Header spans 2 |  |"), "got:\n{md}");
        assert!(md.contains("| --- | --- |"), "got:\n{md}");
        assert!(md.contains("| R1-2 | B1 |"), "got:\n{md}");
        assert!(md.contains("| R1-2 | B2 |"), "got:\n{md}");
    }

    #[test]
    fn dangling_quote_attribute_does_not_leak() {
        // The newegg pattern: a stray `>` inside a quoted attr, plus a dangling
        // quote. html5ever recovers; no attribute text leaks into output.
        let html = r#"<h1 class="max-md:[&gt;a]:hidden">Real Title</h1><a title="x" ">link</a>"#;
        let md = fragment_to_markdown(html);
        assert!(md.contains("# Real Title"), "got:\n{md}");
        assert!(!md.contains("a]:hidden"), "leak:\n{md}");
    }

    #[test]
    fn nul_code_placeholder_survives_parse() {
        // html5ever turns NUL into U+FFFD; the tree path must round-trip the
        // sanctuary's `\x00CODE_FENCE_i\x00` markers or all code blocks vanish.
        let html = "<p>before</p>\u{0}CODE_FENCE_0\u{0}<p>after</p>";
        let md = fragment_to_markdown(html);
        assert!(
            md.contains("\u{0}CODE_FENCE_0\u{0}"),
            "placeholder lost: {md:?}"
        );
    }

    #[test]
    fn trims_leading_byline_residue() {
        let md = "By Jane Doe\nPublished June 1\n\n# Real Title\n\nBody text here.";
        let out = trim_leading_residue(md);
        assert!(out.starts_with("# Real Title"), "got:\n{out}");
        assert!(!out.contains("By Jane"), "got:\n{out}");
    }

    #[test]
    fn keeps_leading_prose_before_heading() {
        let md = "This is a genuine intro paragraph with several words.\n\n# Title\n\nBody.";
        assert_eq!(trim_leading_residue(md), md);
    }

    /// A page with its OWN `## References` keeps it as the only one: ours is
    /// named apart (a Wikipedia article came out with two, 2026-10-03).
    #[test]
    fn the_link_table_does_not_take_the_pages_own_title() {
        let links: String = (0..7)
            .map(|i| format!("[l{i}](http://x.test/{i}) "))
            .collect();
        let md = format!("{links}\n\n## References\n1. A book, 2018.\n");
        let out = compact_citations(&md);
        assert_eq!(out.matches("## References").count(), 1, "{out}");
        assert!(out.contains(REFERENCES_HEADING), "{out}");
    }

    #[test]
    fn citation_compaction_dedups_and_hoists() {
        let md = "[a](http://x) [b](http://y) [a2](http://x) [c](http://z) [d](http://w) [e](http://v) [f](http://u)";
        let out = compact_citations(md);
        assert!(out.contains("## Link targets"), "got:\n{out}");
        assert!(out.contains("a⟨1⟩"), "got:\n{out}");
        assert!(out.contains("a2⟨1⟩"), "same url must reuse ⟨1⟩:\n{out}");
        assert!(out.contains("⟨1⟩ http://x"), "got:\n{out}");
        assert_eq!(
            out.matches("http://x").count(),
            1,
            "url once, in refs only:\n{out}"
        );
    }

    /// **A label can contain an ESCAPED bracket, and it was the only link on
    /// the page to keep its address in the clear.**
    ///
    /// The image mark is rendered `[\[image\]](url)`: its label carries escaped
    /// brackets, and the class `[^\]\n]+` refuses them — the backslash changes
    /// nothing. Every other address on the page became `⟨N⟩`, these stayed
    /// whole. MEASURED 2026-09-10 on the `latercera.com` home page: **73
    /// addresses, 9,287 bytes, 20% of what that page renders**, each already
    /// delivered by its card's title just below.
    #[test]
    fn an_escaped_bracket_in_a_label_still_gets_a_number() {
        let card = |n: usize| format!("[\\[image\\]](/a/{n})\n\n## Titre {n}[lien](/a/{n})\n\n");
        let md: String = (1..=7).map(card).collect();
        let out = compact_citations(&md);
        assert!(out.contains("\\[image\\]⟨1⟩"), "got:\n{out}");
        // The SAME address as the neighbouring heading: a single number for both.
        assert!(out.contains("Titre 1lien⟨1⟩"), "got:\n{out}");
        assert_eq!(
            out.matches("/a/1)").count(),
            0,
            "address still in the clear:\n{out}"
        );
        assert_eq!(out.matches("⟨1⟩ /a/1").count(), 1, "a single entry:\n{out}");
        // Nothing invented, nothing lost: seven cards, seven addresses.
        assert_eq!(out.matches("## Link targets").count(), 1);
        for n in 1..=7 {
            assert!(
                out.contains(&format!("⟨{n}⟩ /a/{n}")),
                "missing {n}:\n{out}"
            );
        }
    }

    /// **A literal bracket in prose does not open a label.**
    ///
    /// arXiv puts EVERY article's dateline in brackets:
    /// `[Submitted on 12 Jun 2017 (<a>v1</a>), last revised …]`. The label
    /// started at the opening bracket — it could not contain a `]`, but
    /// nothing forbade it a `[` — so the link swallowed the prose, the opening
    /// bracket vanished from the page and the closing one stayed alone at the
    /// end of the line.
    #[test]
    fn a_literal_bracket_in_prose_does_not_open_a_label() {
        let md = "[Submitted on 12 Jun 2017 ([v1](/abs/1706v1)), last revised 2 Aug 2023] \
                  [a](/1) [b](/2) [c](/3) [d](/4) [e](/5) [f](/6)";
        let out = compact_citations(md);
        let first = out.lines().next().unwrap_or_default();
        assert!(
            first.starts_with("[Submitted on 12 Jun 2017 (v1⟨1⟩), last revised 2 Aug 2023]"),
            "the prose was swallowed by the label:\n{first}"
        );
        // The bracket count pairs up, which no content assertion says: it is
        // the SHAPE that was broken.
        assert_eq!(
            first.matches('[').count(),
            first.matches(']').count(),
            "unpaired brackets:\n{first}"
        );
    }

    /// The bound: an UNescaped `]` still closes the label. Without it, a label
    /// could swallow the next bracket and take text with it.
    #[test]
    fn an_unescaped_bracket_still_closes_the_label() {
        let md = "[a](http://x) [b](http://y) [c](http://z) [d](http://w) \
                  [e](http://v) [f](http://u) [g]suite](http://t)";
        let out = compact_citations(md);
        // `[g]suite](http://t)` is not a link: the label closes on the bare
        // `]`, and what follows is not an address. Nothing is numbered and
        // nothing is eaten — that is exactly what the widened class must NOT
        // change, otherwise a label would swallow the next bracket.
        assert!(
            out.contains("[g]suite](http://t)"),
            "the text moved:\n{out}"
        );
        assert!(
            !out.contains("http://t\n"),
            "address wrongly hoisted:\n{out}"
        );
        assert_eq!(
            out.matches("⟨").count(),
            6 + 6,
            "six links, six references:\n{out}"
        );
    }

    /// **A hidden tooltip goes; a hidden tab stays.**
    ///
    /// Both carry the `hidden` attribute. Only the first is pointed to by an
    /// `aria-describedby`, that is, declared by the page as the DESCRIPTION of
    /// another element. Without this second condition, the corpus's IKEA
    /// product page loses 1,137 bytes of real description (measured) — `hidden`
    /// alone does not tell a tab from a tooltip.
    #[test]
    fn a_hidden_tooltip_goes_and_a_hidden_tab_stays() {
        let html = "<html><body>\
            <a aria-describedby=\"t1\">Jane Doe</a>\
            <div hidden id=\"t1\"><p>Department of Pharmacology, Cincinnati</p></div>\
            <div hidden id=\"tab2\"><p>Made of durable plastic that keeps its shape</p></div>\
            </body></html>";
        let md = crate::web::html_to_markdown(html);
        assert!(
            !md.contains("Department of Pharmacology"),
            "the tooltip pointed to by aria-describedby stayed:\n{md}"
        );
        assert!(
            md.contains("durable plastic"),
            "the hidden tab, which nothing describes, was lost:\n{md}"
        );
    }

    /// The bound in the other direction: the reference must target THIS id.
    /// An `aria-describedby` naming something else hides nothing.
    #[test]
    fn a_describedby_pointing_elsewhere_keeps_the_block() {
        let html = "<html><body>\
            <a aria-describedby=\"autre\">Lien</a>\
            <div hidden id=\"t1\"><p>Texte que rien ne décrit</p></div>\
            </body></html>";
        let md = crate::web::html_to_markdown(html);
        assert!(md.contains("Texte que rien ne décrit"), "got:\n{md}");
    }

    #[test]
    fn citation_compaction_noop_below_threshold() {
        let md = "just [one](http://x) link";
        assert_eq!(compact_citations(md), md);
    }

    #[test]
    fn backtick_escalation() {
        // inline code containing a backtick escalates to double backticks
        assert_eq!(inline_code("a`b"), "``a`b``");
        // content starting with a backtick gets padded
        assert_eq!(inline_code("`x"), "`` `x ``");
        // a fence around code containing ``` uses 4 backticks
        let f = code_fence("```\nx\n```", "sh");
        assert!(f.contains("````sh"), "got: {f:?}");
    }

    #[test]
    fn mermaid_svg_recovered() {
        let html =
            r#"<div><svg id="mermaid-1"><text>A --> B</text><text>B --> C</text></svg></div>"#;
        let md = fragment_to_markdown(html);
        assert!(md.contains("```mermaid"), "got:\n{md}");
        assert!(md.contains("A --> B"), "got:\n{md}");
    }

    #[test]
    fn drops_tracking_pixel_images() {
        let html =
            r#"<p><img src="/b/ss/spacer.gif" alt="tracking"><img src="/real.jpg" alt="Real"></p>"#;
        let md = fragment_to_markdown(html);
        assert!(!md.contains("tracking"), "pixel not dropped:\n{md}");
        assert!(md.contains("[image: Real]"), "real image lost:\n{md}");
    }

    /// An alt-less `<img>` emits nothing, which used to leave its
    /// `<figcaption>` standing as ordinary prose — and `markdown::outline`
    /// then previewed the section with a description of a picture.
    #[test]
    fn a_caption_whose_image_had_no_alt_gets_an_anchor() {
        let html = r#"<figure><img src="/a.jpg"><figcaption>Sonnet in her own hand</figcaption></figure><p>From 1832 her abilities emerged.</p>"#;
        let md = fragment_to_markdown(html);
        assert!(
            md.contains("[image]\nSonnet in her own hand"),
            "caption must be anchored, directly under the marker:\n{md}"
        );
    }

    /// Only when the image produced no marker of its own. An `alt` already
    /// stands above the caption, and a second anchor would be a line of pure
    /// cost on every captioned figure on the page.
    #[test]
    fn a_caption_under_an_alt_bearing_image_gets_no_extra_anchor() {
        let html = r#"<figure><img src="/a.jpg" alt="Watercolour portrait"><figcaption>Painted c. 1840</figcaption></figure>"#;
        let md = fragment_to_markdown(html);
        assert!(
            md.contains("[image: Watercolour portrait]"),
            "alt lost:\n{md}"
        );
        assert!(!md.contains("[image]"), "redundant anchor added:\n{md}");
    }

    /// A `<figure>` with no caption at all is left exactly as it was — the
    /// anchor exists to mark a caption, not to announce every picture.
    #[test]
    fn a_captionless_alt_less_figure_stays_silent() {
        let html = r#"<figure><img src="/a.jpg"></figure><p>Body text.</p>"#;
        let md = fragment_to_markdown(html);
        assert!(!md.contains("[image"), "nothing to anchor:\n{md}");
    }

    #[test]
    fn video_iframe_canonicalized() {
        let html = r#"<iframe src="//www.youtube-nocookie.com/embed/abc123"></iframe>"#;
        let md = fragment_to_markdown(html);
        assert!(
            md.contains("[YouTube video](https://www.youtube-nocookie.com/embed/abc123)"),
            "got:\n{md}"
        );
    }

    #[test]
    fn heading_and_list_and_link() {
        let html = r#"<h2>Sec</h2><ul><li>one</li><li><a href="/p">two</a></li></ul>"#;
        let md = fragment_to_markdown(html);
        assert!(md.contains("## Sec"), "got:\n{md}");
        assert!(md.contains("- one"), "got:\n{md}");
        assert!(md.contains("[two](/p)"), "got:\n{md}");
    }

    /// theverge.com drop cap: the initial is styled in its own span, the whole
    /// word repeated `sr-only` for assistive tech. Emitting both FABRICATED
    /// `WhileWhile` — a token the page does not contain, which then reached the
    /// index and the embedder.
    #[test]
    fn a_drop_cap_and_its_screen_reader_twin_are_emitted_once() {
        let html = r#"<p><span aria-hidden="true"><span class="big">W</span>hile</span><span class="sr-only">While</span><span> GPUs have been around.</span></p>"#;
        let md = fragment_to_markdown(html);
        assert!(!md.contains("WhileWhile"), "fabricated token: {md}");
        assert!(
            md.contains("While GPUs have been around."),
            "text lost: {md}"
        );
        assert_eq!(
            md.matches("While").count(),
            1,
            "kept exactly one copy: {md}"
        );
    }

    /// Stack Exchange writes the author and date of each comment twice: once
    /// as `d-none` microdata for robots, once in the clear for the reader. The
    /// agent received both.
    #[test]
    fn hidden_microdata_is_not_delivered_twice() {
        let html = r#"<p><span class="d-none" itemprop="name">Ada Lovelace</span>&ndash; <a href="/u/1">Ada Lovelace</a><span class="d-none" itemprop="datePublished">2021-07-19 17:04:42 +00:00</span><span>Commented Jul 19, 2021</span></p>"#;
        let md = fragment_to_markdown(html);
        assert_eq!(
            md.matches("Ada Lovelace").count(),
            1,
            "author delivered twice: {md}"
        );
        assert!(
            !md.contains("2021-07-19 17:04:42"),
            "machine date delivered: {md}"
        );
        assert!(md.contains("Commented Jul 19, 2021"), "date lost: {md}");
    }

    /// **BOTH conditions matter.** A tab or a collapsed panel carries `hidden`
    /// or `display:none` and is painted on click: IKEA stores 489 bytes of
    /// dimensions there, Newegg 2,718 bytes of seller ratings. Measured: the
    /// rule written on `display:none` alone took them away.
    #[test]
    fn a_collapsed_panel_without_microdata_is_kept() {
        let html = r#"<div hidden role="tabpanel"><p>Width: 21 cm. Weight: 0.76 kg.</p></div>"#;
        let md = fragment_to_markdown(html);
        assert!(md.contains("Width: 21 cm"), "collapsed content lost: {md}");
        let html = r#"<div style="opacity:0;display:none"><p>Sold and shipped by: Techome. 87 total ratings.</p></div>"#;
        let md = fragment_to_markdown(html);
        assert!(md.contains("Sold and shipped by"), "panel lost: {md}");
    }

    /// And the other half of the pair: VISIBLE microdata is content,
    /// `itemprop` or not.
    #[test]
    fn visible_microdata_is_kept() {
        let html = r#"<p itemprop="description">The only copy of this sentence.</p>"#;
        let md = fragment_to_markdown(html);
        assert!(md.contains("The only copy of this sentence."), "got:\n{md}");
    }

    /// The mirror order (accessible copy first) is equally common.
    #[test]
    fn the_twin_is_found_on_either_side() {
        let html = r#"<p><span class="visually-hidden">Menu</span><span aria-hidden="true">Menu</span></p>"#;
        let md = fragment_to_markdown(html);
        assert_eq!(md.matches("Menu").count(), 1, "got:\n{md}");
    }

    /// The rule is "already carried by a twin", NOT "honour aria-hidden".
    /// Measured: 35 154 of 46 876 chars inside `aria-hidden` on the 147-page
    /// corpus appear nowhere else, so dropping unpaired ones destroys content.
    #[test]
    fn unpaired_aria_hidden_text_is_kept() {
        let html = r#"<p><span aria-hidden="true">Only copy of this sentence.</span></p>"#;
        let md = fragment_to_markdown(html);
        assert!(
            md.contains("Only copy of this sentence."),
            "content lost: {md}"
        );
    }

    /// Same text, no visually-hidden twin — two real repetitions stay two.
    #[test]
    fn a_plain_repeated_sibling_is_not_treated_as_a_twin() {
        let html = r#"<p><span aria-hidden="true">Go</span><span>Go</span></p>"#;
        let md = fragment_to_markdown(html);
        assert_eq!(md.matches("Go").count(), 2, "got:\n{md}");
    }
}
#[cfg(test)]
mod paragraph_text_does_not_forge_headings {
    use super::*;

    #[test]
    fn a_paragraph_opening_with_a_hash_is_escaped() {
        let md = fragment_to_markdown("<p># systemctl daemon-reload</p>");
        assert!(
            md.contains("\\# systemctl daemon-reload"),
            "the marker must be escaped, got:\n{md}"
        );
        assert!(
            !md.lines().any(|l| l.starts_with("# ")),
            "no line may still read as a heading, got:\n{md}"
        );
    }

    #[test]
    fn every_atx_level_is_escaped_and_seven_hashes_is_not_a_heading() {
        for level in 1..=6 {
            let hashes = "#".repeat(level);
            let md = fragment_to_markdown(&format!("<p>{hashes} deep</p>"));
            assert!(
                md.contains(&format!("\\{hashes} deep")),
                "level {level} must be escaped, got:\n{md}"
            );
        }
        // CommonMark stops at six, so a seventh hash is already prose and a
        // backslash there would be noise on a page that never risked anything.
        let md = fragment_to_markdown("<p>####### seven</p>");
        assert!(
            md.contains("####### seven") && !md.contains("\\#######"),
            "seven hashes is not a heading, got:\n{md}"
        );
    }

    #[test]
    fn a_hash_with_no_separator_is_left_alone() {
        // `#include`, `#1`, `#rustlang`: CommonMark needs a space after the
        // marker, so these were never headings and escaping them would put a
        // backslash into ordinary prose.
        for text in ["#include \"stdio.h\"", "#1 in sales", "#rustlang is fun"] {
            let md = fragment_to_markdown(&format!("<p>{text}</p>"));
            assert!(
                md.contains(text) && !md.contains('\\'),
                "{text:?} must pass through untouched, got:\n{md}"
            );
        }
    }

    #[test]
    fn a_bare_hash_on_its_own_line_is_a_heading() {
        // `#` alone is a valid empty ATX heading, so end-of-line counts as the
        // separator just as a space does.
        let md = fragment_to_markdown("<p>#</p>");
        assert!(md.contains("\\#"), "got:\n{md}");
    }

    #[test]
    fn a_hash_after_a_line_break_is_escaped_too() {
        // The archwiki shape: one paragraph, several lines, the marker on a
        // line that is not the first. Escaping only the paragraph's opening
        // line would leave this one forging a heading.
        let md = fragment_to_markdown("<p>Run this:<br># systemctl edit unit</p>");
        assert!(
            md.contains("\\# systemctl edit unit"),
            "a marker after <br> must be escaped, got:\n{md}"
        );
    }

    #[test]
    fn prose_with_no_marker_is_returned_untouched() {
        let md = fragment_to_markdown("<p>An ordinary sentence about C# and F#.</p>");
        assert!(
            md.contains("An ordinary sentence about C# and F#.") && !md.contains('\\'),
            "got:\n{md}"
        );
    }

    #[test]
    fn a_real_heading_is_still_a_heading() {
        let md = fragment_to_markdown("<h2>Real section</h2><p>body</p>");
        assert!(
            md.lines().any(|l| l.trim() == "## Real section"),
            "escaping must not touch what <hN> emits, got:\n{md}"
        );
    }
}
#[cfg(test)]
mod a_block_inside_an_inline_context_keeps_its_boundary {
    use super::*;

    /// The case that produced the measurement: two paragraphs in a layout
    /// cell. A markdown table cannot carry a line break, so the boundary
    /// becomes ONE space — not nothing.
    #[test]
    fn two_paragraphs_in_one_cell_do_not_weld() {
        // TWO columns: a single-column table is not one and goes out unfolded
        // (`is_a_layout_table`), which would no longer send this cell through
        // `inline` — the path this test exists to hold.
        let md = fragment_to_markdown(
            "<table><tr><td><p>First sentence ends here.</p>\
             <p>Second sentence starts here.</p></td><td>Note</td></tr></table>",
        );
        assert!(
            md.contains("here. Second"),
            "the two sentences are welded, got:\n{md}"
        );
        // Anti-vacuity: the boundary does not open with a stray space. A rule
        // that pushes a space UNCONDITIONALLY passes the test above and fails
        // here, and it would indent the first column of every row of every
        // table.
        assert!(
            md.contains("| First sentence"),
            "the cell opens with a stray space, got:\n{md}"
        );
    }

    /// The counter-example that shapes the rule: `<span>` is INLINE, and a
    /// space invented here would break a word. That is already why
    /// `subtree_text` separates nothing.
    #[test]
    fn an_inline_span_still_touches_its_neighbour() {
        let md = fragment_to_markdown(
            "<table><tr><td><span>W</span>hile you wait</td><td>Note</td></tr></table>",
        );
        assert!(
            md.contains("While you wait"),
            "a space was invented inside a word, got:\n{md}"
        );
    }

    /// The rule does not only hold for cells: headings, list items, link
    /// labels go through the SAME `inline`. Tested on two of them, because
    /// "proven on one consumer, not on its sibling" is the most frequent
    /// defect in this repository.
    #[test]
    fn a_heading_and_a_list_item_keep_the_boundary_too() {
        let md = fragment_to_markdown("<h2><div>Environment</div><div>Arctic ice melt</div></h2>");
        assert!(
            md.contains("Environment Arctic ice melt"),
            "heading welded, got:\n{md}"
        );
        // Since 2026-10-04 the second paragraph is the item's CONTINUATION (a
        // stronger boundary than a space): what matters here is still that no
        // word is welded.
        let md = fragment_to_markdown("<ul><li><p>alpha ends</p><p>beta starts</p></li></ul>");
        assert!(
            md.contains("alpha ends") && md.contains("beta starts") && !md.contains("endsbeta"),
            "list item welded, got:\n{md}"
        );
    }

    /// A block followed by BARE TEXT: the CLOSING boundary is the only one
    /// that can hold it. Without it, opening before each block is enough as
    /// long as blocks follow one another — and `<div>Label</div>value` still
    /// welds.
    #[test]
    fn a_block_followed_by_bare_text_keeps_the_boundary() {
        // Two columns, same reason as above: a single-column table goes out
        // unfolded and no longer goes through `inline`.
        let md = fragment_to_markdown(
            "<table><tr><td><div>Alpha</div>tail text</td><td>Note</td></tr></table>",
        );
        assert!(
            md.contains("Alpha tail text"),
            "the bare text welded to the block before it, got:\n{md}"
        );
    }

    /// The boundary never OPENS with a space. Cells and headings go through a
    /// `.trim()` that would erase it silently, but the block-level inline arm
    /// (`out.push_str(&inline(n))`) does not trim: that is where the leading
    /// space survives all the way to the rendered markdown.
    #[test]
    fn the_boundary_never_opens_with_a_space() {
        // The fixture used to be `<span><div>Alpha</div>tail</span>`. Since a
        // phrasing element that CARRIES block content stays a block — the
        // bound holds for the whole list and no longer only for custom
        // elements — this `<span>` no longer goes through `inline` and no
        // longer exercised the rule. The table cell, however, still goes
        // through it: it is the path by which this boundary is really reached.
        let md = fragment_to_markdown(
            "<table><tr><td><div>Alpha</div>tail</td><td>Note</td></tr></table>",
        );
        assert!(
            md.contains("| Alpha tail |"),
            "the boundary opens with a stray space, got:\n{md:?}"
        );
    }

    /// A real data table does not move: the rule separates blocks INSIDE a
    /// cell, it does not touch the columns.
    #[test]
    fn a_real_data_table_is_unchanged() {
        let md = fragment_to_markdown(
            "<table><tr><th>Name</th><th>Value</th></tr><tr><td>alpha</td><td>1</td></tr></table>",
        );
        assert!(md.contains("| Name | Value |"), "{md}");
        assert!(md.contains("| alpha | 1 |"), "{md}");
    }
}
#[cfg(test)]
mod a_one_column_table_is_not_a_table {
    use super::*;

    /// The old web: the whole page is a one-column `<table>`. Its cells ARE
    /// the document and go out as blocks.
    #[test]
    fn a_layout_wrapper_is_unwrapped() {
        let md = fragment_to_markdown(
            "<table id=\"wrap\"><tr><td><h2>Un titre</h2>\
             <p>Un paragraphe qui porte le contenu de la page.</p></td></tr></table>",
        );
        assert!(
            md.contains("## Un titre"),
            "the heading was flattened into a cell, got:\n{md}"
        );
        assert!(
            !md.contains("---"),
            "the output still asserts a table the page did not put, got:\n{md}"
        );
    }

    /// The bound, on the side that must STAY a table: two columns without
    /// `<th>`, that is `10000__natiga_qalubiaedu_org` (subject, grade) and
    /// `sportingnews_art`. Without this assertion, the rule "every table
    /// without `<th>` unfolds" passes the test above and destroys these two
    /// pages.
    #[test]
    fn two_columns_without_a_header_stay_a_table() {
        let md = fragment_to_markdown(
            "<table><tr><td>Mathématiques</td><td>60</td></tr>\
             <tr><td>Sciences</td><td>40</td></tr></table>",
        );
        assert!(
            md.contains("| Mathématiques | 60 |"),
            "a real table without a header was unfolded, got:\n{md}"
        );
    }

    /// The other bound: `<th>` is a DECLARATION by the page. One column, but
    /// the page itself says that column means something.
    #[test]
    fn one_column_with_a_header_stays_a_table() {
        let md = fragment_to_markdown(
            "<table><tr><th>Étape</th></tr><tr><td>Ouvrir la porte</td></tr>\
             <tr><td>Fermer la porte</td></tr></table>",
        );
        assert!(
            md.contains("| Étape |"),
            "a header declared by the page was thrown away, got:\n{md}"
        );
    }

    /// A table whose rows are in a NESTED table has no rows of its own:
    /// `table_to_md` rendered `inline(table)`, which flattens the whole
    /// subtree — including the real table inside.
    #[test]
    fn a_frame_around_a_real_table_keeps_the_real_table() {
        let md = fragment_to_markdown(
            "<table><tr><td><table><tr><th>A</th><th>B</th></tr>\
             <tr><td>un</td><td>deux</td></tr></table></td></tr></table>",
        );
        assert!(
            md.contains("| un | deux |"),
            "the inner table was flattened by its frame, got:\n{md}"
        );
    }
}
#[cfg(test)]
mod a_pipe_in_a_cell_is_the_page_s_own {
    use super::*;

    /// Two columns, and the first carries a bar. The row must keep its two
    /// columns WITHOUT the page's character changing.
    #[test]
    fn the_row_keeps_its_columns_and_the_character() {
        let md = fragment_to_markdown(
            "<table><tr><th>Type</th><th>Sens</th></tr>             <tr><td>string | number</td><td>une union</td></tr></table>",
        );
        assert!(
            md.contains(r"string \| number"),
            "the page's bar is not escaped, got:\n{md}"
        );
        assert!(
            !md.contains('\u{a6}'),
            "a look-alike was delivered instead of the bar, got:\n{md}"
        );
        let row = md
            .lines()
            .find(|l| l.contains("une union"))
            .expect("the data row");
        let mut cols = 1;
        let mut escaped = false;
        for c in row.trim().trim_matches('|').chars() {
            match c {
                '\\' => escaped = !escaped,
                '|' if !escaped => cols += 1,
                _ => escaped = false,
            }
        }
        assert_eq!(cols, 2, "the row changed its number of columns: {row}");
    }
}
#[cfg(test)]
mod a_summary_is_the_label_of_its_details {
    use super::*;

    /// The shape of a FAQ: the question is the `<summary>`, the answer is the
    /// body. Rendered as a paragraph, the question was indistinguishable from
    /// the answer it announces.
    #[test]
    fn a_question_becomes_a_heading() {
        let md = fragment_to_markdown(
            "<details><summary>Comment annuler mon abonnement ?</summary>             <p>Ouvrez la page de facturation.</p></details>",
        );
        assert!(
            md.contains("#### Comment annuler mon abonnement ?"),
            "the label is not a heading, got:\n{md}"
        );
        assert!(
            md.contains("Ouvrez la page de facturation."),
            "the block's body disappeared, got:\n{md}"
        );
    }

    /// The bound: a `<summary>` that ALREADY carries a heading keeps its own.
    /// Without this condition, the output says `#### ### Question`.
    #[test]
    fn a_summary_that_carries_its_own_heading_keeps_it() {
        let md = fragment_to_markdown(
            "<details><summary><h3>Une question</h3></summary><p>Une réponse.</p></details>",
        );
        // Compared LINE BY LINE, never with `contains`: `inline()` of an `<h3>`
        // renders its bare text, so the wrong rendering is `#### Une question`,
        // which CONTAINS "### Une question". The first version of this test
        // passed with and without the guard.
        assert!(
            md.lines().any(|l| l.trim() == "### Une question"),
            "the page's heading was lost or renumbered, got:\n{md}"
        );
        assert!(
            !md.lines().any(|l| l.trim().starts_with("#### ")),
            "a second heading was placed on a heading, got:\n{md}"
        );
    }

    /// And an empty label invents no section: `pages/go_pkg` carries one, all
    /// markup and without a word of text.
    #[test]
    fn an_empty_summary_opens_no_section() {
        let md = fragment_to_markdown("<details><summary> </summary><p>Un corps.</p></details>");
        assert!(
            !md.contains("####"),
            "an empty section was opened, got:\n{md}"
        );
    }
}
#[cfg(test)]
mod a_figure_marker_means_an_image {
    use super::*;

    /// An image without a description keeps its mark: it is the only thing
    /// that says there is an illustration there.
    #[test]
    fn an_image_without_an_alt_is_still_announced() {
        let md = fragment_to_markdown(
            "<figure><img src=\"a.jpg\"><figcaption>Une légende</figcaption></figure>",
        );
        assert!(
            md.lines().any(|l| l.trim() == "[image]"),
            "the illustration is no longer announced, got:\n{md}"
        );
    }

    /// A code listing in a figure is not an image — five in
    /// `pages/rust_book` are exactly that.
    #[test]
    fn a_code_listing_is_not_an_illustration() {
        let md = fragment_to_markdown(
            "<figure><pre><code>fn main() {}</code></pre>\
             <figcaption>Listing 1-1</figcaption></figure>",
        );
        assert!(
            !md.contains("[image]"),
            "a code block was announced as an illustration, got:\n{md}"
        );
        assert!(md.contains("fn main()"), "the code disappeared, got:\n{md}");
    }

    /// An `<svg>` counts: that is how a page draws a diagram.
    #[test]
    fn a_drawing_counts_as_an_illustration() {
        let md = fragment_to_markdown(
            "<figure><svg viewBox=\"0 0 2 2\"><rect width=\"1\" height=\"1\"/></svg>\
             <figcaption>Le schéma</figcaption></figure>",
        );
        assert!(
            md.lines().any(|l| l.trim() == "[image]"),
            "a drawing is not announced, got:\n{md}"
        );
    }
}
#[cfg(test)]
mod a_figure_names_the_medium_it_holds {
    use super::*;

    /// Two figures of `pages/wikipedia_art` are `<video>`s. "image" named the
    /// wrong thing there; saying nothing would leave their caption floating
    /// without a subject.
    #[test]
    fn a_video_is_named_a_video() {
        let md = fragment_to_markdown(
            "<figure><video><source src=\"a.webm\"></video>\
             <figcaption>Une conférence sur Rust</figcaption></figure>",
        );
        assert!(
            md.lines().any(|l| l.trim() == "[video]"),
            "the video is not named, got:\n{md}"
        );
        assert!(
            !md.contains("[image]"),
            "a video is announced as an image, got:\n{md}"
        );
    }

    /// And the image wins when the figure carries both: it is a video's
    /// poster, and `[image: …]` or `[image]` is what the rest of the pipeline
    /// knows how to read.
    #[test]
    fn an_image_beside_a_video_still_says_image() {
        let md = fragment_to_markdown(
            "<figure><video poster=\"p.jpg\"><source src=\"a.webm\"></video>\
             <img src=\"p.jpg\"><figcaption>Une légende</figcaption></figure>",
        );
        assert!(
            md.lines().any(|l| l.trim() == "[image]"),
            "the figure's image is no longer announced, got:\n{md}"
        );
    }
}
#[cfg(test)]
mod an_unknown_element_is_inline {
    use super::*;

    /// The measured case: Yahoo wraps every number in `<fin-streamer>`, in the
    /// middle of a sentence. The sentence arrived on THREE lines.
    #[test]
    fn a_custom_element_does_not_break_the_sentence() {
        let md = fragment_to_markdown(
            "<p>Le titre a gagne <fin-streamer value=\"2.31\">2,31 %</fin-streamer> hier, \
             apres une seance calme.</p>",
        );
        assert!(
            md.contains("Le titre a gagne 2,31 % hier, apres une seance calme."),
            "the sentence was cut around the unknown element, got:\n{md}"
        );
    }

    /// The bound: an unknown element that CARRIES block content stays a block
    /// — otherwise a `<gu-island>`'s whole card would fit into one line and
    /// its heading would stop being a heading.
    #[test]
    fn a_custom_element_that_wraps_blocks_stays_a_block() {
        let md = fragment_to_markdown(
            "<gu-island name=\"Card\"><h2>Un titre de carte</h2>\
             <p>Le corps de la carte, qui tient sur sa propre ligne.</p></gu-island>",
        );
        assert!(
            md.lines().any(|l| l.trim() == "## Un titre de carte"),
            "the card's heading was flattened into a line, got:\n{md}"
        );
    }

    /// A name WITHOUT a hyphen keeps its handling, and this test exists
    /// because the first rule — "every unknown name" — sent `<body>` through
    /// the inline arm: a `<body>` without a block child then went through
    /// `inline`, which does not know `<iframe>`, and the page's video
    /// disappeared. `video_iframe_canonicalized` is what said so.
    #[test]
    fn a_container_without_a_hyphen_keeps_its_own_path() {
        let md = fragment_to_markdown(
            "<body><iframe src=\"//www.youtube.com/embed/abc123\"></iframe></body>",
        );
        assert!(
            md.contains("[YouTube video](https://www.youtube.com/embed/abc123)"),
            "the container went through the inline arm and the video disappeared, got:\n{md}"
        );
    }

    /// And a KNOWN block still breaks: without this assertion, a rule that
    /// rendered everything inline would pass the two tests above.
    #[test]
    fn a_known_block_still_breaks_the_line() {
        let md = fragment_to_markdown("<p>Avant</p><div>Un bloc</div><p>Apres</p>");
        assert!(
            md.lines().any(|l| l.trim() == "Un bloc"),
            "the block was pulled into the neighbouring line, got:\n{md}"
        );
    }
}
#[cfg(test)]
mod a_frameset_page_says_where_its_document_is {
    use super::*;

    /// The content of a `<noframes>` is RAW TEXT to the parser: a string that
    /// CONTAINS markup. It went out as-is, tags included, into the delivered
    /// markdown — which a page never shows.
    #[test]
    fn the_fallback_markup_never_reaches_the_reader() {
        let md = fragment_to_markdown(
            "<noframes><body><p>Votre navigateur ne gere pas les cadres. \
             Allez a <a href=\"contenu.html\">la page de contenu</a>.</p></body></noframes>",
        );
        assert!(
            !md.contains("<p>") && !md.contains("<a href") && !md.contains("<body>"),
            "markup is delivered as text:\n{md}"
        );
    }

    /// ALL the frames, not only the first: a `<frameset cols="20%,80%">` opens
    /// with its menu, and naming only that one sends the reader into the
    /// navigation instead of the document.
    #[test]
    fn every_frame_that_leads_somewhere_is_named() {
        let html = "<html><head><title>Vieux site</title></head>\
                    <frameset cols=\"20%,80%\"><frame src=\"menu.html\">\
                    <frame src=\"contenu.html\"></frameset></html>";
        let frames = declared_frame_urls(html, "https://vieux.test/index.html");
        assert_eq!(
            frames,
            vec![
                "https://vieux.test/menu.html".to_string(),
                "https://vieux.test/contenu.html".to_string(),
            ],
            "not all frames are named"
        );
        // And the first stays reachable alone, for the "thin page" path.
        assert_eq!(
            declared_frame_url(html, "https://vieux.test/index.html").as_deref(),
            Some("https://vieux.test/menu.html")
        );
    }
}

#[cfg(test)]
mod text_after_a_list_is_its_own_paragraph {
    /// `<div><ul>…</ul>Tail…</div>` put `Tail` on the line under the last
    /// bullet: a lazy continuation of that item in Markdown.
    #[test]
    fn a_blank_line_ends_the_list_before_loose_text() {
        let md = crate::web_tree::fragment_to_markdown(
            "<div><ul><li>Step one of the process</li></ul>Tail text after the list ends here.</div>",
        );
        assert!(
            md.contains("- Step one of the process\n\nTail text"),
            "{md:?}"
        );
    }
}

#[cfg(test)]
mod checkbox_and_pre_cells {
    /// A checked and an unchecked box both came out as nothing; a `<pre>` in a
    /// cell broke its row in three. Both after html-to-markdown-rs's fixes of
    /// the same shapes (2026-10-04).
    const PAGE: &str = "<html><body><main><h1>Feature matrix</h1>\
        <p>A page long enough for the extractor to read it as the body of the document.</p>\
        <table><thead><tr><th>Task</th><th>Done</th><th>Notes</th></tr></thead><tbody>\
        <tr><td>Write docs</td><td><input type=\"checkbox\" checked disabled></td><td>ok</td></tr>\
        <tr><td>Ship release</td><td><input type=\"checkbox\" disabled></td><td><pre>cargo build\ncargo a|b</pre></td></tr>\
        </tbody></table>\
        <ul><li><input type=\"checkbox\" class=\"task-list-item-checkbox\" checked disabled> Write the parser</li>\
        <li><input type=\"text\" value=\"typed\"> not a box</li></ul></main></body></html>";

    #[test]
    fn a_checkbox_says_its_state_and_a_cell_keeps_its_row() {
        let md = crate::web::html_to_markdown(PAGE);
        assert!(md.contains("| Write docs | [x] | ok |"), "{md}");
        assert!(
            md.contains("| Ship release | [ ] | `cargo build`<br>`cargo a\\|b` |"),
            "{md}"
        );
        assert!(md.contains("- [x] Write the parser"), "{md}");
        assert!(
            !md.contains("typed"),
            "a text field is still a form control:\n{md}"
        );
        assert!(!md.contains("```"), "{md}");
    }
}

#[cfg(test)]
mod multi_paragraph_items {
    /// `<li><p>…</p><p>…</p></li>`: the second paragraph continues the item
    /// at its content column; a one-paragraph item stays tight.
    #[test]
    fn a_second_paragraph_continues_its_item() {
        let page = "<html><body><main><h1>Install</h1><p>A page long enough for the extractor to read it as the body of a document.</p>\
            <ol><li><p>Download the archive.</p><p>Check its signature before unpacking it.</p></li><li><p>Run the installer.</p></li></ol></main></body></html>";
        let md = crate::web::html_to_markdown(page);
        assert!(
            md.contains("1. Download the archive.\n\n   Check its signature before unpacking it.\n2. Run the installer."),
            "{md}"
        );
    }
}

#[cfg(test)]
mod a_code_block_in_a_list_item {
    /// `<li><p>…</p><pre>…</pre></li>`: the fence sits at the item's content
    /// column, so the list does not end there; the next item stays an item.
    #[test]
    fn the_fence_stays_inside_its_item() {
        let page = "<html><body><main><h1>Setup</h1><p>A page long enough for the extractor to read it as the body of a document.</p>\
            <ol><li><p>Configure the server:</p><pre><code>port = 8080\nhost = \"0.0.0.0\"</code></pre></li><li>Start it.</li></ol></main></body></html>";
        let md = crate::web::html_to_markdown(page);
        assert!(
            md.contains("\n   ```\n   port = 8080\n   host = \"0.0.0.0\"\n   ```\n"),
            "{md}"
        );
        assert!(md.contains("2. Start it."), "{md}");
    }
}

#[cfg(test)]
mod a_cell_keeps_its_lines {
    /// `<br>` and a list inside a cell are LINES of that cell: `<br>`-joined,
    /// not run together with a space.
    #[test]
    fn br_and_list_items_are_cell_lines() {
        let page = "<html><body><main><h1>Contacts</h1><p>A page long enough for the extractor to read it as the body of a document.</p>\
            <table><tr><th>Key</th><th>Value</th></tr><tr><td>addr</td><td>1 Main St<br>Springfield</td></tr>\
            <tr><td>tags</td><td><ul><li>alpha</li><li>beta</li></ul></td></tr><tr><td>note</td><td>plain <b>bold</b> text</td></tr></table></main></body></html>";
        let md = crate::web::html_to_markdown(page);
        assert!(md.contains("| addr | 1 Main St<br>Springfield |"), "{md}");
        assert!(md.contains("| tags | alpha<br>beta |"), "{md}");
        assert!(
            md.contains("| note | plain bold text |")
                || md.contains("| note | plain **bold** text |"),
            "{md}"
        );
    }
}

#[cfg(test)]
mod an_item_keeps_its_lines {
    /// `<br>` in a list item or a `<dd>` is a line, and a `<blockquote>` in an
    /// item is a block under it — all three were flattened into one line.
    #[test]
    fn br_and_quote_inside_items() {
        let page = "<html><body><main><h1>Steps</h1><p>A page long enough for the extractor to read it as the body of a document.</p>\
            <ul><li>Name: Ada<br>Born: 1815</li><li>Quote:<blockquote><p>That brain of mine is something more than merely mortal.</p></blockquote></li><li>Last</li></ul>\
            <dl><dt>Term</dt><dd>first<br>second</dd></dl></main></body></html>";
        let md = crate::web::html_to_markdown(page);
        assert!(md.contains("- Name: Ada\n  Born: 1815\n"), "{md}");
        assert!(
            md.contains(
                "- Quote:\n\n  > That brain of mine is something more than merely mortal.\n"
            ),
            "{md}"
        );
        assert!(md.contains("\n- Last"), "{md}");
        assert!(md.contains("first\nsecond"), "{md}");
    }
}

#[cfg(test)]
mod a_q_keeps_its_marks {
    /// A browser draws `<q>` with its quotation marks; the text must carry them.
    #[test]
    fn q_is_quoted_once() {
        let page = "<html><body><main><h1>Refs</h1><p>A page long enough for the extractor to read it as the body of a document.</p>\
            <p>Smith (2010). <q>It should now be clear.</q> Retrieved.</p><p>Jones: <q>\"Already quoted\"</q> done.</p></main></body></html>";
        let md = crate::web::html_to_markdown(page);
        assert!(
            md.contains("Smith (2010). \"It should now be clear.\" Retrieved."),
            "{md}"
        );
        assert!(md.contains("Jones: \"Already quoted\" done."), "{md}");
    }
}

#[cfg(test)]
mod a_list_without_items_keeps_its_text {
    /// `<ul>text</ul>` (no `<li>`) is shown by a browser; it was dropped.
    /// Whitespace between ordinary items adds nothing.
    #[test]
    fn loose_text_in_a_list_is_kept() {
        let page = "<html><body><main><h1>Probe</h1><p>A page long enough for the extractor to read it as the body of a document.</p>\
            <ul>Loose text with no list item</ul><ul>\n  <li>alpha</li>\n  <li>beta</li>\n</ul></main></body></html>";
        let md = crate::web::html_to_markdown(page);
        assert!(md.contains("Loose text with no list item"), "{md}");
        assert!(md.contains("- alpha\n- beta"), "{md}");
    }
}

#[cfg(test)]
mod a_block_in_an_item_stays_a_block {
    /// A table, a disclosure or a second `<div>` in a list item is a block
    /// under it, not words run into its label.
    #[test]
    fn table_details_and_divs_hang_under_the_item() {
        let page = "<html><body><main><h1>Probe</h1><p>A page long enough for the extractor to read it as the body of a document.</p>\
            <ul><li>Pricing<table><tr><th>Plan</th><th>Cost</th></tr><tr><td>Pro</td><td>9</td></tr></table></li><li>After table</li></ul>\
            <ul><li>Intro<details><summary>More options</summary><p>Hidden detail text.</p></details></li></ul>\
            <ul><li><div>Block A in item</div><div>Block B in item</div></li></ul>\
            <ol><li><p>Run the first step.</p><p>Then the second.</p>and read what follows it.</li></ol>\
            <ol><li><p>Install it.</p><div class=highlight><pre><code class=language-shell>make install</code></pre></div><p>Then check it.</p></li><li>Next step</li></ol></main></body></html>";
        let md = crate::web::html_to_markdown(page);
        assert!(
            md.contains(
                "- Pricing\n\n  | Plan | Cost |\n  | --- | --- |\n  | Pro | 9 |\n- After table"
            ),
            "{md}"
        );
        assert!(
            md.contains("  Hidden detail text.") && !md.contains("Intro More options"),
            "{md}"
        );
        assert!(
            md.contains("- Block A in item\n\n  Block B in item"),
            "{md}"
        );
        // Words after a block of the item follow it, not the label above it.
        assert!(
            md.contains(
                "1. Run the first step.\n\n   Then the second.\n\n   and read what follows it."
            ),
            "{md}"
        );
        // A code block in the item sits at its content column: at column 0 it
        // ended the list, and `Next step` restarted at 1.
        assert!(md.contains("1. Install it.\n\n   ```shell\n   make install\n   ```\n\n   Then check it.\n2. Next step"), "{md}");
    }
}

#[cfg(test)]
mod a_component_slot_is_content {
    /// Wiki.js serves its whole page body in `<template slot="contents">`,
    /// projected into its `<page>` component; a bare `<template>` stays inert.
    #[test]
    fn a_slot_template_is_read_and_a_bare_one_is_not() {
        let page = "<html><body><div id=\"root\"><page title=\"Sonarr\"><template slot=\"contents\"><div><h1>Getting Started</h1>\
            <p>Sonarr is a PVR for Usenet and BitTorrent users. It can monitor multiple RSS feeds for new episodes of your favorite shows.</p>\
            <p>It can also be configured to automatically upgrade the quality of files already downloaded when a better format becomes available.</p></div></template>\
            <template v-slot:footer><p>Last edited by a maintainer of this wiki.</p></template></page>\
            <template id=\"row\"><p>INERT-ROW-GABARIT</p></template></div></body></html>";
        let md = crate::web::html_to_markdown(page);
        assert!(
            md.contains("Getting Started") && md.contains("monitor multiple RSS feeds"),
            "{md}"
        );
        assert!(md.contains("Last edited by a maintainer"), "{md}");
        assert!(!md.contains("INERT-ROW-GABARIT"), "{md}");
    }
}

#[cfg(test)]
mod a_page_state_lists_its_items {
    /// Inertia.js keeps the whole page in `data-page` (entity-escaped JSON): a
    /// catalogue of short product names is a list of links, not prose to mine.
    #[test]
    fn an_inertia_catalogue_reads_as_links() {
        let products: Vec<String> = [
            "PES Master Plus",
            "Notion Life OS",
            "Procreate Brushes",
            "Lightroom Presets",
        ]
        .iter()
        .enumerate()
        .map(|(i, n)| {
            format!(
                r#"{{"name":"{n}","url":"https://s{i}.example.test/l/p{i}","price_cents":1500}}"#
            )
        })
        .collect();
        let state = format!(
            r#"{{"component":"Discover/Index","props":{{"title":"Discover","flags":"2023-10-16; risk_in_requirements_beta=v1; retrieve_tax_forms_beta=v1; other_beta=v2;","nav":[{{"name":"3D"}},{{"name":"Audio"}}],"search_results":{{"products":[{}]}}}}}}"#,
            products.join(",")
        );
        let escaped = state.replace('&', "&amp;").replace('"', "&quot;");
        let page = format!(
            r#"<html><head><title>Discover</title></head><body><div id="app" data-page="{escaped}"></div><script src="/app.js"></script></body></html>"#
        );
        let md = crate::web::html_to_markdown(&page);
        assert!(
            md.contains("- [PES Master Plus](https://s0.example.test/l/p0)"),
            "{md}"
        );
        assert!(
            md.contains("- [Lightroom Presets](https://s3.example.test/l/p3)"),
            "{md}"
        );
        assert!(
            !md.contains("beta=v1"),
            "a configuration string is not prose: {md}"
        );
    }
}
