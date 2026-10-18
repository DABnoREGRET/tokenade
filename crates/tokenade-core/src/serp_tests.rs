// The tests of `crate::serp`, moved out of the production file.
//
// Mounted by `include!`: the tokens are spliced INTO `serp`, so every
// `super::` keeps its meaning. Split by `srcsweep::production_lines`.

#[cfg(test)]
mod chrome_tests {
    use super::cut_serp_chrome;

    /// Observed live: the snippet for the tree-sitter repo carried the
    /// engine's own "Searches related to …" block, which described the QUERY,
    /// not the page — and being longer, it beat the clean snippet from
    /// another engine in the cross-engine merge.
    #[test]
    fn a_snippet_stops_where_the_page_stops_being_described() {
        let got = cut_serp_chrome(
            "Tree-sitter can efficiently update the previous tree ... Searches related to rust tree-sitter incremental parsing api",
        );
        assert_eq!(got, "Tree-sitter can efficiently update the previous tree");

        assert_eq!(
            cut_serp_chrome("A real description. People also ask What is X?"),
            "A real description"
        );
        // Nothing to cut.
        let clean = "An ordinary description of an ordinary page.";
        assert_eq!(
            cut_serp_chrome(clean),
            "An ordinary description of an ordinary page"
        );
    }

    /// A marker index is found in a case-folded copy and used to slice the
    /// ORIGINAL. Fold with `to_lowercase` and a Turkish dotted capital makes
    /// the copy longer than the original, so the index lands mid-character.
    #[test]
    fn a_non_ascii_snippet_does_not_panic() {
        for s in [
            "İstanbul’da hava durumu — çok güzel. Related searches İstanbul",
            "日本語の説明がここにあります。 People also ask 何ですか",
            "Größe und Maße — ausführlich. Missing: foo",
            "Ω≈ç√∫˜µ≤≥÷ related searches x",
        ] {
            let out = cut_serp_chrome(s);
            assert!(!out.is_empty(), "cut everything from {s:?}");
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn brave_fixture() -> String {
        // Minimal faithful skeleton of a Brave SERP result block (captured
        // 2026-07-10): `data-type="web"` block, favicon link on
        // imgs.search.brave.com BEFORE the destination link, clean title in
        // the title div's `title` attribute, description under
        // generic-snippet → content.
        let block = |pos: usize, url: &str, title: &str, desc: &str| {
            format!(
                r##"<div class="snippet  svelte-jmfu5f" data-pos="{pos}" data-type="web" data-keynav="true"><div class="result-wrapper svelte-1rq4ngz"><div class="result-content svelte-1rq4ngz"><a href="{url}" target="_self" class="svelte-14r20fy l1"><div class="favicon-wrapper"><img src="https://imgs.search.brave.com/xyz/favicon.ico"/></div><cite class="snippet-url">example <span class="text-ellipsis">› path</span></cite><div class="title search-snippet-title line-clamp-1 svelte-14r20fy" title="{title}">{title}</div></a><div class="generic-snippet svelte-1cwdgg3"><div class="content desktop-default-regular t-primary svelte-1cwdgg3"><span class="t-secondary">July 1, 2026 -</span> {desc}</div></div></div></div></div>"##
            )
        };
        format!(
            r#"<html><head><meta property="og:site_name" content="Brave Search"/></head><body><nav><a href="/images?q=x">Images</a></nav>{}{}</body></html>"#,
            block(
                0,
                "https://crates.io/crates/ureq/0.9.0",
                "ureq - crates.io: Rust Package Registry",
                "A simple, safe HTTP client with enough prose in this description to satisfy the snippet extractor's minimum length."
            ),
            block(
                1,
                "https://docs.rs/ureq",
                "ureq - Rust",
                "Its features include cookies, JSON, HTTP proxies, HTTPS and charset decoding for everyday requests."
            ),
        )
    }

    #[test]
    fn ecosia_extract_parses_organic_results() {
        // Faithful skeleton captured 2026-07-10: organic-result block with a
        // result-link anchor (destination) + result-title anchor (title) +
        // result__description snippet.
        let block = |url: &str, title: &str, desc: &str| {
            format!(
                r##"<article data-test-id="organic-result" class="result web-result"><div class="result__body"><div class="result__header"><a data-test-id="result-link" href="{url}" class="result-info__link"><div data-test-id="result-source"><span class="result-info__domain">https://example.com</span></div></a><h2><a data-test-id="result-title" href="{url}" class="result-title">{title}</a></h2></div><p class="result__description">{desc}</p></div></article>"##
            )
        };
        let html = format!(
            r#"<html><body>{}{}</body></html>"#,
            block(
                "https://crates.io/crates/reqwest",
                "reqwest - crates.io",
                "An ergonomic, batteries-included HTTP Client for Rust with enough words for the snippet gate."
            ),
            block(
                "https://docs.rs/reqwest",
                "reqwest - Rust - Docs.rs",
                "The reqwest crate provides a convenient higher-level HTTP client for everyday requests."
            ),
        );
        let hits = ecosia_extract(&html);
        assert_eq!(hits.len(), 2, "two organic results: {hits:?}");
        assert_eq!(hits[0].url, "https://crates.io/crates/reqwest");
        assert_eq!(hits[0].title, "reqwest - crates.io");
        assert!(
            hits[0].snippet.contains("HTTP Client"),
            "snippet: {:?}",
            hits[0].snippet
        );
    }

    #[test]
    fn mojeek_extract_parses_title_anchors() {
        let html = r##"<html><body><ul class="results-standard">
          <li><a class="title" href="https://mojeek-example.org/a">First Result Title</a>
              <p class="s">First snippet with enough prose to satisfy the extractor length gate here.</p></li>
          <li><a class="title" href="https://mojeek-example.org/b">Second Result Title</a>
              <p class="s">Second snippet also long enough to pass the minimum snippet-length threshold.</p></li>
        </ul></body></html>"##;
        let hits = mojeek_extract(html);
        assert_eq!(hits.len(), 2, "two results: {hits:?}");
        assert_eq!(hits[0].url, "https://mojeek-example.org/a");
        assert_eq!(hits[0].title, "First Result Title");
        assert!(
            hits[0].snippet.contains("First snippet"),
            "snippet: {:?}",
            hits[0].snippet
        );
        // Non-title anchors (nav, favicon) must not surface.
        let nav = r#"<a href="https://mojeek.com/about">About</a>"#;
        assert!(mojeek_extract(nav).is_empty());
    }

    #[test]
    fn marginalia_extract_parses_json_api() {
        let body = r#"{"results":[
          {"url":"https://small-web.example/page1","title":"A Personal Site","description":"Long-tail content the mainstream engines bury."},
          {"url":"https://small-web.example/page2","title":"Another Small Page","description":"More niche content here."},
          {"url":"ftp://ignored","title":"non-http","description":"skipped"}
        ]}"#;
        let hits = marginalia_extract_json(body);
        assert_eq!(hits.len(), 2, "two http results, ftp skipped: {hits:?}");
        assert_eq!(hits[0].url, "https://small-web.example/page1");
        assert_eq!(hits[0].title, "A Personal Site");
        assert!(marginalia_extract_json("not json").is_empty());
        assert!(marginalia_extract_json(r#"{"results":[]}"#).is_empty());
    }

    #[test]
    fn brave_extract_parses_organic_blocks() {
        let html = brave_fixture();
        assert!(detect(&html), "og:site_name meta confirms Brave SERP");
        let hits = brave_extract(&html);
        assert_eq!(hits.len(), 2, "two organic blocks: {hits:?}");
        assert_eq!(hits[0].url, "https://crates.io/crates/ureq/0.9.0");
        assert_eq!(hits[0].title, "ureq - crates.io: Rust Package Registry");
        assert!(
            hits[0].snippet.contains("safe HTTP client"),
            "snippet extracted: {:?}",
            hits[0].snippet
        );
        // Favicon host never mistaken for the destination.
        assert!(!hits[0].url.contains("imgs.search.brave.com"));
        // The shared entry point routes Brave pages through this extractor —
        // nav links ("Images") must not surface as results.
        let listed = compact_with(&html, 10);
        assert!(!listed.contains("/images?q"), "nav leaked: {listed}");
        assert!(listed.contains("docs.rs/ureq"), "{listed}");
    }

    #[test]
    fn detect_recognises_google_serp() {
        let html = r#"<html><head><title>foo - Google Search</title></head><body>
            <div id="search"><div id="rso"><div>...</div></div></div>
        </body></html>"#;
        assert!(detect(html));
    }

    #[test]
    fn detect_recognises_bing_serp() {
        let html =
            r#"<html><body><ol id="b_results"><li class="b_algo">...</li></ol></body></html>"#;
        assert!(detect(html));
    }

    #[test]
    fn detect_recognises_ddg_serp() {
        let html = r#"<html><body><div class="results_links">...</div></body></html>"#;
        assert!(detect(html));
    }

    #[test]
    fn detect_rejects_random_page() {
        assert!(!detect(
            "<html><body><h1>Just a blog post</h1></body></html>"
        ));
    }

    #[test]
    fn detect_rejects_seo_prose_mentioning_engines() {
        // #709 regression: a plain text document discussing SEO topics
        // routinely names search engines and their URL paths. Pre-#709
        // any single engine-URL signal triggered SERP and the file got
        // routed through extract() — destroying the prose. Lock down
        // that engine-URL mentions ALONE never trigger.
        let prose = "Programmatic SaaS SEO playbook\n\n\
                     When optimizing for google.com/search and bing.com/search, \
                     ensure your sitemap is indexed. duckduckgo.com and \
                     search.brave.com also surface results similarly. \
                     For Google Search results, structured data matters.\n\
                     \n\
                     Tactics:\n\
                     1. Build topic clusters\n\
                     2. Internal linking\n\
                     3. Schema.org markup";
        assert!(
            !detect(prose),
            "prose mentioning google.com/search etc. must NOT trigger SERP detection"
        );
    }

    #[test]
    fn detect_rejects_markdown_doc_with_html_snippet() {
        // #710 — a markdown doc documenting SERP markup (e.g. a blog
        // post or a CHANGELOG entry describing this very fix) contains
        // literal `id="rso"` / `class="b_algo"` in code blocks. The
        // source-or-doc pre-filter short-circuits before the structural
        // signal check so docs aren't shredded by the result extractor.
        let md = "# SERP markup reference\n\
                  \n\
                  ## Google\n\
                  The results container uses `id=\"rso\"` as its anchor.\n\
                  Each organic hit has a wrapping div with the search id:\n\
                  \n\
                  ```html\n\
                  <div id=\"search\"><div id=\"rso\">\n\
                  ```\n\
                  \n\
                  ## Bing\n\
                  Bing uses `id=\"b_results\"` and `class=\"b_algo\"`.\n";
        assert!(
            !detect(md),
            "markdown documenting SERP markup must NOT trigger detection"
        );
    }

    #[test]
    fn detect_rejects_random_html_with_engine_url_string() {
        // A blog post about SEO that happens to be HTML still mustn't
        // trigger — the `<p>Google Search ranks pages</p>` content has
        // no SERP-shaped DOM structure.
        let html = "<html><body><article><h1>SEO 101</h1>\
                    <p>Google Search and bing.com/search both rank pages.</p>\
                    <p>This is a regular blog post, not a results page.</p>\
                    </article></body></html>";
        assert!(
            !detect(html),
            "blog post mentioning engines in prose must NOT trigger SERP"
        );
    }

    #[test]
    fn extract_pulls_titles_and_resolves_google_redirect() {
        let html = r#"<html><body><div id="rso">
            <a href="/url?q=https://rust-lang.org/&amp;sa=U"><h3>The Rust Programming Language</h3></a>
            <div>A language empowering everyone to build reliable and efficient software. Get started with rustup today.</div>
            <a href="https://doc.rust-lang.org/book/"><h3>The Rust Book</h3></a>
            <div>The official guide to learning Rust, written by the Rust community.</div>
        </div></body></html>"#;
        let r = extract(html);
        assert!(r.len() >= 2);
        assert!(r[0].url.starts_with("https://rust-lang.org"));
        assert_eq!(r[0].title, "The Rust Programming Language");
        assert!(r[0].snippet.contains("empowering"));
    }

    #[test]
    fn compact_dedups_same_host() {
        let html = r#"<div id="rso">
            <a href="https://example.com/a"><h3>Title A</h3></a><div>Snippet alpha that is plenty long to count as prose.</div>
            <a href="https://example.com/b"><h3>Title B</h3></a><div>Snippet beta that is plenty long to count as prose.</div>
            <a href="https://other.com/x"><h3>Title C</h3></a><div>Snippet gamma that is plenty long to count as prose.</div>
        </div>"#;
        let out = compact_with(html, 10);
        // example.com/a and example.com/b share host but DIFFER on first path
        // segment ("a" vs "b") — keep both. other.com is distinct.
        assert!(out.contains("Title A"));
        assert!(out.contains("Title B"));
        assert!(out.contains("Title C"));
    }

    #[test]
    fn compact_caps_at_limit() {
        let mut html = String::from(r#"<div id="rso">"#);
        for i in 0..30 {
            let _ = write!(
                html,
                r#"<a href="https://site{i}.com/path"><h3>Result number {i}</h3></a><div>Some snippet text long enough to be picked up by the extractor heuristic.</div>"#
            );
        }
        html.push_str("</div>");
        let out = compact_with(&html, 5);
        assert!(out.contains("Result number 0"));
        assert!(out.contains("Result number 4"));
        assert!(!out.contains("Result number 6"));
    }

    #[test]
    fn savings_reasonable() {
        // A realistic synthetic Google page: 10 organic results plus the
        // ~30 KB of script/CSS/footer chrome that real SERPs ship with.
        let big_js = "var a = ".to_string() + &"1234567890,".repeat(2000) + "0;";
        let big_css = ".cls{display:none;}".repeat(800);
        let footer_links = (0..200)
            .map(|i| format!(r#"<a href="https://google.com/preferences?h={i}">Settings</a>"#))
            .collect::<String>();
        let mut html = format!(
            r#"<!DOCTYPE html><html><head><title>rust async - Google Search</title>
            <script>{big_js}</script><style>{big_css}</style>
            </head><body><div id="searchform"><form>...</form></div>
            <div id="rso">"#
        );
        for i in 0..10 {
            let _ = write!(
                html,
                r#"<div class="g"><a href="https://rust-lang.org/page{i}"><h3>Async Rust resource {i}</h3></a>
                <cite>rust-lang.org › docs › async-{i}</cite>
                <div class="snippet">A deep dive into async/await in Rust, including how the executor model works and what the runtime does under the hood. {i}</div></div>"#
            );
        }
        let _ = write!(
            html,
            "</div><div id=\"botstuff\">{footer_links}</div></body></html>"
        );
        let (_out, sav) = compact_with_savings(&html);
        assert!(
            sav.percent() > 80.0,
            "expected >80% savings, got {:.1}%",
            sav.percent()
        );
    }

    #[test]
    fn extract_falls_back_to_markdown_on_non_serp() {
        let plain = "<html><body><h1>Hello</h1><p>Just an article.</p></body></html>";
        let out = compact(plain);
        // Should NOT contain the [tokenade:serp] banner.
        assert!(!out.contains("[tokenade:serp]"));
        assert!(out.contains("Hello"));
    }

    #[test]
    fn numeric_entities_decoded_decimal_and_hex() {
        assert_eq!(decode_entities("Wikip&#233;dia"), "Wikipédia");
        assert_eq!(decode_entities("a &#183; b"), "a · b");
        assert_eq!(decode_entities("it&#x2019;s"), "it’s");
        // Malformed / unknown left verbatim; no `&#` pays nothing.
        assert_eq!(decode_entities("plain text"), "plain text");
        assert_eq!(decode_entities("&#;bad"), "&#;bad");
    }

    /// Round 38 field report: an agent was served ten Polish weather results
    /// for a German question about air-conditioner noise, under a banner
    /// reading `✓ SEARCH OK … do NOT retry`. Every engine but one was handing
    /// back its own navigation bar. These are the real link shapes, verbatim.
    #[test]
    fn engine_chrome_is_never_a_result() {
        for u in [
            // Relative — mojeek and ecosia nav.
            "/images",
            "/about/",
            "/ai-chat?q=Klimager%C3%A4t",
            "/blogs",
            // Absolute, engine-owned — yahoo nav and preferences.
            "https://search.yahoo.com/preferences/preferences",
            "https://sports.yahoo.com/",
            "https://finance.yahoo.com/",
            "https://login.yahoo.com/",
            "https://www.ecosia.org/images?q=x",
            "https://www.mojeek.com/ads/",
            "https://www.bing.com/search?q=x",
            "https://duckduckgo.com/about",
            // Junk shapes.
            "",
            "#top",
            "javascript:void(0)",
        ] {
            assert!(!is_useful_url(u), "engine chrome must not survive: {u:?}");
        }
        // Real results, which must.
        for u in [
            "https://www.climia.de/21Grad/Lautstaerke-im-Alltag/",
            "https://www.coolblue.de/beratung/klimaanlage-lautstaerke.html",
            "http://example.org/page",
        ] {
            assert!(is_useful_url(u), "a real result must survive: {u:?}");
        }
    }

    /// A domain boundary, not a substring: the denylist must not swallow a
    /// legitimate host that merely CONTAINS an engine's name.
    #[test]
    fn engine_match_respects_the_domain_boundary() {
        assert!(is_useful_url("https://mybing.com.example.org/article"));
        assert!(is_useful_url("https://google.com.phish.example/x"));
        assert!(!is_useful_url("https://news.google.com/x"));
    }

    fn r(title: &str, url: &str) -> Result {
        Result {
            title: title.into(),
            url: url.into(),
            snippet: String::new(),
        }
    }

    #[test]
    fn a_navigation_bar_is_not_a_result_set() {
        // Verbatim from the yahoo SERP that shipped as "search results".
        let nav = vec![
            r("Yahoo", "https://www.yahoo.com/"),
            r("Settings", "https://search.yahoo.com/preferences"),
            r("Finance", "https://finance.yahoo.com/"),
            r("Sports", "https://sports.yahoo.com/"),
            r("Weather", "https://www.yahoo.com/news/weather"),
            r("Sign In", "https://login.yahoo.com/"),
        ];
        assert!(
            !plausible_for_query(&nav, "Klimagerät Geräuschpegel dB Schlafzimmer"),
            "a portal nav bar must not pass as results for an unrelated query"
        );
    }

    #[test]
    fn real_results_pass_including_german_compounds() {
        // `Klimageräte` / `Lautstärke` — inflected and compounded, never the
        // query verbatim. Prefix matching is what makes these count.
        let hits = vec![
            r(
                "Mobile Klimageräte: So laut sind sie wirklich im Alltag",
                "https://climia.de/x",
            ),
            r("Wie laut ist eine Klimaanlage?", "https://wolf-kt.de/y"),
            r(
                "Lautstärke mobiler Klimageräte: die Top 10",
                "https://example.de/z",
            ),
        ];
        assert!(plausible_for_query(
            &hits,
            "Klimagerät Geräuschpegel dB Schlafzimmer"
        ));
    }

    /// A query with nothing long enough to judge on must ABSTAIN, not reject.
    #[test]
    fn short_queries_are_not_judged() {
        let hits = vec![r("Anything at all", "https://example.org/a")];
        assert!(plausible_for_query(&hits, "cat dog"));
        assert!(
            !plausible_for_query(&[], "cat dog"),
            "empty is always a failure"
        );
    }

    /// HTTP 200, the query echoed in the title, a CAPTCHA where the results
    /// should be. Nothing upstream can tell — this is the only place that can.
    /// A German result reached the agent as `f&uuml;r ein Klimager&auml;t`.
    /// Letter entities are not exotic; they are every non-English page.
    #[test]
    fn accented_entities_are_decoded_in_delivered_text() {
        assert_eq!(
            decode_entities("Welche Dezibelwerte sind f&uuml;r ein Klimager&auml;t"),
            "Welche Dezibelwerte sind für ein Klimagerät"
        );
        assert_eq!(
            decode_entities("St&ouml;rt &szlig; ma&icirc;tre caf&eacute;"),
            "Stört ß maître café"
        );
        assert_eq!(
            decode_entities("&Auml;pfel &amp; &Uuml;bung"),
            "Äpfel & Übung"
        );
        // Numeric and named still compose.
        assert_eq!(decode_entities("&#233;t&eacute;"), "été");
    }

    #[test]
    fn a_bot_challenge_is_recognised_not_parsed() {
        let challenge = "<html><title>Klimagerät - Recherche</title><body>\
            Une dernière étape. Veuillez résoudre le défi ci-dessous pour continuer.\
            <div class=\"captcha\"></div></body></html>";
        assert!(looks_like_challenge(challenge));
        assert!(looks_like_challenge(
            "<html>Checking your browser before accessing. cf-challenge</html>"
        ));
        // One incidental mention is not a challenge — an article ABOUT captchas
        // must still be readable.
        assert!(!looks_like_challenge(
            "<html><h1>How CAPTCHA works</h1><p>A long article about the topic.</p></html>"
        ));
    }

    #[test]
    fn bing_decode_url_unwraps_ck_a_redirect() {
        // u = "a1" + base64url("https://www.postgresql.org/") (no padding).
        let href =
            "https://www.bing.com/ck/a?!&&p=abc&u=a1aHR0cHM6Ly93d3cucG9zdGdyZXNxbC5vcmcv&ntb=1";
        assert_eq!(bing_decode_url(href), "https://www.postgresql.org/");
        // Non-ck/a href passes through untouched.
        assert_eq!(
            bing_decode_url("https://direct.example/x"),
            "https://direct.example/x"
        );
        // Undecodable payload falls back to the input (never drops a result).
        let bad = "https://www.bing.com/ck/a?u=a1!!!notbase64";
        assert_eq!(bing_decode_url(bad), bad);
    }

    #[test]
    fn bing_extract_parses_blocks_and_decodes_urls() {
        let html = concat!(
            r#"<ol id="b_results"><li class="b_algo"><h2><a href="https://www.bing.com/ck/a?&amp;u=a1aHR0cHM6Ly9leGFtcGxlLm9yZy9kb2Nz&amp;ntb=1">Example Docs</a></h2>"#,
            r#"<div class="b_caption"><p>A long enough snippet describing the example documentation for the test.</p></div></li>"#,
            r#"<li class="b_algo"><h2><a href="https://direct.test/page">Direct Result Title</a></h2>"#,
            r#"<div class="b_caption"><p>Another snippet body that is comfortably past the length floor here.</p></div></li></ol>"#,
        );
        let hits = bing_extract(html);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].url, "https://example.org/docs");
        assert_eq!(hits[0].title, "Example Docs");
        assert!(hits[0].snippet.contains("example documentation"));
        assert_eq!(hits[1].url, "https://direct.test/page");
    }

    #[test]
    fn yahoo_deref_extracts_real_destination() {
        let href = "https://r.search.yahoo.com/_ylt=Ab/RV=2/RE=1/RO=10/RU=https%3a%2f%2fexample.com%2fa/RK=2/RS=xyz";
        assert_eq!(yahoo_deref(href), "https://example.com/a");
        // No /RU= → input returned unchanged.
        assert_eq!(
            yahoo_deref("https://plain.example/x"),
            "https://plain.example/x"
        );
    }

    #[test]
    fn yahoo_extract_scopes_title_to_h3_and_derefs() {
        let html = concat!(
            r#"<div class="algo-sr"><a href="https://r.search.yahoo.com/RU=https%3a%2f%2fblog.example%2fpost/RK=2/RS=z">"#,
            r#"<span class="fc">blog.example</span>https://blog.example › post"#,
            r#"<h3><span>The Real Article Title Here</span></h3></a>"#,
            r#"<div class="compText"><p>A snippet with enough words to clear the snippet extraction floor cleanly.</p></div></div>"#,
        );
        let hits = yahoo_extract(html);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].url, "https://blog.example/post");
        assert_eq!(hits[0].title, "The Real Article Title Here");
        assert!(hits[0].snippet.contains("snippet"));
    }
}
#[cfg(test)]
mod a_generic_class_name_is_not_an_engine {
    use super::detect;

    /// The shape that cost the most: an ordinary article page whose header
    /// carries the site's own search box. Before #285 this was delivered as a
    /// numbered list of navigation links and the article text was gone.
    const NEWS_PAGE: &str = r#"<!DOCTYPE html><html><head><title>Artikel</title></head>
<body><header><form id="search" action="/suche"><input name="q"></form></header>
<article><h1>Kindersitze im Test</h1><p>Der ADAC hat 27 Kindersitze geprueft.</p>
<p>Zwei fielen durch, weil der Gurt bei einem Seitenaufprall riss.</p></article>
<footer><a href="/impressum">Impressum</a></footer></body></html>"#;

    #[test]
    fn a_site_search_box_alone_is_not_a_serp() {
        assert!(NEWS_PAGE.contains("id=\"search\""));
        assert!(!detect(NEWS_PAGE));
    }

    #[test]
    fn a_nav_container_named_links_alone_is_not_a_serp() {
        let page = r#"<!DOCTYPE html><html><body><nav id="links"><a href="/a">A</a></nav>
<article><p>Ein langer Artikel ueber Automatisierung.</p></article></body></html>"#;
        assert!(!detect(page));
    }

    #[test]
    fn an_engine_signal_alone_still_confirms() {
        for s in [
            "<div id=\"rso\">",
            "<li class=\"b_algo\">",
            "<h2 class=\"result__title\">",
            "<div class=\"serp-item\">",
        ] {
            let page = format!("<!DOCTYPE html><html><body>{s}</body></html>");
            assert!(detect(&page), "{s} should confirm on its own");
        }
    }

    #[test]
    fn two_generic_names_together_still_confirm() {
        let page = r#"<!DOCTYPE html><html><body><div id="search">
<div class="search-result"><a href="https://x.test">X</a></div></div></body></html>"#;
        assert!(detect(page));
    }
}
