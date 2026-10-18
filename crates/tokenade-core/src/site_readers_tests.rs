// The tests of `crate::site_readers`, moved out of the production file.
//
// Mounted by `include!`: the tokens are spliced INTO `site_readers`, so every
// `super::` keeps its meaning. Split by `srcsweep::production_lines`.

#[cfg(test)]
mod tests {
    use super::*;

    /// stackoverflow.com and the `*.stackexchange.com` domains must return the
    /// SAME plan: it is the same family of sites and it was already the same
    /// code, under two names. The guard exists so that the merge does not
    /// silently come undone — two names that diverge never say so.
    #[test]
    fn the_stack_exchange_family_shares_one_plan() {
        // The URLs carry a query AND a fragment: without them, cleaning or
        // not cleaning yields the same plan and the guard says nothing about
        // the cleanup. The fixture must land where the behaviour changes.
        let so = plan("https://stackoverflow.com/questions/12345/why?sort=votes#answer-9");
        let se = plan("https://unix.stackexchange.com/questions/12345/why?sort=votes#answer-9");
        let (so, se) = match (so, se) {
            (Some(a), Some(b)) => (a, b),
            _ => panic!("one of the family's two hosts returns no plan"),
        };
        assert!(
            matches!(so.kind, Kind::StackExchange) && matches!(se.kind, Kind::StackExchange),
            "the two hosts of the same family took different routes"
        );
        assert_eq!(
            so.fetch_url, "https://stackoverflow.com/questions/12345/why",
            "the plan must refetch the same page WITHOUT its query or fragment"
        );
        assert_eq!(
            se.fetch_url,
            "https://unix.stackexchange.com/questions/12345/why"
        );
    }

    /// The Atom feed is the source for BOTH address shapes, and they are told
    /// apart by their shape alone. The traps are `/user/x/comments/`
    /// (a listing of one person's comments, not a thread) and the query, which
    /// is noise on a thread and the request itself on a listing.
    #[test]
    fn reddit_routes_threads_and_listings_to_their_own_readers() {
        let kind_of =
            |u: &str| plan(u).map(|p| (p.fetch_url, matches!(p.kind, Kind::RedditThread)));

        for (url, want) in [
            (
                "https://www.reddit.com/r/rust/comments/1v7lpif/slug/?sort=top",
                "https://www.reddit.com/r/rust/comments/1v7lpif/slug/.rss",
            ),
            (
                "https://www.reddit.com/comments/1v7lpif",
                "https://www.reddit.com/comments/1v7lpif/.rss",
            ),
        ] {
            let (got, is_thread) = kind_of(url).unwrap_or_else(|| panic!("no plan for {url}"));
            assert!(is_thread, "{url} should be a thread");
            assert_eq!(got, want, "{url}");
        }

        for (url, want) in [
            (
                "https://www.reddit.com/r/rust/",
                "https://www.reddit.com/r/rust/.rss",
            ),
            (
                "https://www.reddit.com/r/rust/top/?t=week",
                "https://www.reddit.com/r/rust/top/.rss?t=week",
            ),
            (
                "https://sh.reddit.com/r/rust/search?q=async&restrict_sr=1",
                "https://www.reddit.com/r/rust/search/.rss?q=async&restrict_sr=1",
            ),
            // The one a `contains("/comments/")` test used to misroute.
            (
                "https://www.reddit.com/user/spez/comments/",
                "https://www.reddit.com/user/spez/comments/.rss",
            ),
        ] {
            let (got, is_thread) = kind_of(url).unwrap_or_else(|| panic!("no plan for {url}"));
            assert!(!is_thread, "{url} should be a listing");
            assert_eq!(got, want, "{url}");
        }

        // Pages with no submission list must keep falling through to generic.
        for url in [
            "https://www.reddit.com/r/rust/about/rules",
            "https://www.reddit.com/r/rust/wiki/index",
            "https://www.reddit.com/settings",
        ] {
            assert!(plan(url).is_none(), "{url} should have no reader");
        }
    }

    #[test]
    fn reddit_listing_renders_submissions_and_skips_promoted() {
        let html = r##"<html><head><title>rust</title></head><body>
<div class="thing link" data-type="link" data-author="alice" data-score="422"
     data-comments-count="94" data-subreddit-prefixed="r/rust" data-domain="self.rust"
     data-permalink="/r/rust/comments/abc/first/" data-url="https://www.reddit.com/r/rust/comments/abc/first/">
  <a class="title may-blank" href="/r/rust/comments/abc/first/">First post title</a>
</div>
<div class="thing link promoted" data-type="link" data-author="adco" data-score="1"
     data-promoted="true" data-permalink="/r/rust/comments/zzz/ad/">
  <a class="title may-blank" href="/x">Buy this thing</a>
</div>
<div class="thing link" data-type="link" data-author="bob" data-score="309"
     data-comments-count="55" data-subreddit-prefixed="r/rust" data-domain="github.com"
     data-permalink="/r/rust/comments/def/syn/" data-url="https://github.com/dtolnay/syn">
  <a class="title may-blank" href="/r/rust/comments/def/syn/">Syn 3.0.0 released</a>
</div>
<a href="https://old.reddit.com/r/rust/?count=25&amp;after=t3_def" rel="nofollow next">next</a>
</body></html>"##;
        let md = reddit_listing(html).expect("listing parsed");
        assert!(md.contains("## First post title") && md.contains("## Syn 3.0.0 released"));
        assert!(md.contains("u/alice") && md.contains("422 pts") && md.contains("94 comments"));
        // Advertising is not part of the listing the agent asked for.
        assert!(!md.contains("Buy this thing"), "promoted leaked: {md}");
        assert!(md.contains("> 2 submissions"), "{md}");
        // A self-post has no outbound link; a link post carries one.
        assert!(!md.contains("data-url"));
        assert!(md.contains("https://github.com/dtolnay/syn"), "{md}");
        assert!(md.contains("[next page: https://old.reddit.com/r/rust/?count=25&after=t3_def]"));

        // An interstitial has neither submissions nor comments -> fail open.
        assert!(reddit_listing("<html><title>Please wait</title><body>…</body></html>").is_none());
    }

    /// Search is the one listing old.reddit does not build out of `thing`
    /// divs. The trap is the class token: `\b` also matches the hit's own
    /// `search-result-meta` CHILD, which cut every entry short before its
    /// score, author and date.
    #[test]
    fn reddit_search_results_keep_their_meta_line() {
        let html = r##"<html><head><title>rust: search results - async</title></head><body>
<div class="listing search-result-listing">
<div class=" search-result search-result-link has-linkflair " data-fullname="t3_1kegysp">
 <header class="search-result-header">
  <a href="https://old.reddit.com/r/rust/comments/1kegysp/feather/" class="search-title may-blank">I built Feather</a>
 </header>
 <div class="search-result-meta">
  <span class="search-score">838 points</span>
  <a href="/x" class="search-comments may-blank">184 comments</a>
  <span class="search-time">submitted <time datetime="2025-05-04T10:45:36+00:00">1 year ago</time></span>
  <span class="search-author">by <a href="/user/rs" class="author may-blank">rs</a></span>
  <span>to <a href="https://old.reddit.com/r/rust/" class="search-subreddit-link may-blank">r/rust</a></span>
 </div>
 <div class="search-result-body"><div class="md"><p>Body text.</p></div></div>
</div>
</div></body></html>"##;
        let md = reddit_listing(html).expect("search parsed");
        assert!(md.contains("> 1 results"), "{md}");
        assert!(md.contains("## I built Feather"), "{md}");
        assert!(
            md.contains("r/rust • u/rs • 838 points • 184 comments • 2025-05-04"),
            "meta lost to the child-div trap: {md}"
        );
        assert!(md.contains("https://old.reddit.com/r/rust/comments/1kegysp/feather/"));
    }

    /// `/user/<name>/comments/` has no submissions on it at all, so the reader
    /// has to recognise the reply shape or it hands the page back unread.
    #[test]
    fn a_user_comment_page_renders_replies_with_the_thread_they_are_in() {
        let html = r##"<html><head><title>overview for spez</title></head><body>
<div class=" thing comment " data-type="comment" data-author="spez"
     data-subreddit-prefixed="r/announcements" data-permalink="/r/announcements/comments/abc/x/def/">
  <p class="parent"><a href="/r/announcements/comments/abc/x/" class="title">21 years of Reddit</a></p>
  <span class="score unvoted" title="132">132 points</span>
  <form class="usertext"><div class="md"><p>Thanks for being here.</p></div></form>
</div>
<div class=" thing comment " data-type="comment" data-author="spez">
  <form class="usertext"><div class="md"></div></form>
</div>
</body></html>"##;
        let md = reddit_listing(html).expect("comment listing parsed");
        assert!(md.contains("## in: 21 years of Reddit"), "{md}");
        assert!(md.contains("u/spez • 132 pts • r/announcements"), "{md}");
        assert!(md.contains("Thanks for being here."));
        // A removed comment is an empty body, not an entry.
        assert!(md.contains("> 1 comments"), "{md}");
    }

    fn tiktok_html(scope: &str) -> String {
        format!(
            r#"<html><body><script id="__UNIVERSAL_DATA_FOR_REHYDRATION__" type="application/json">{{"__DEFAULT_SCOPE__":{scope}}}</script></body></html>"#
        )
    }

    #[test]
    fn tiktok_renders_video_and_profile_from_the_rehydration_payload() {
        let video = tiktok_html(
            r#"{"webapp.video-detail":{"statusCode":0,"itemInfo":{"itemStruct":{
              "id":"7","desc":"A caption  with  spaces #tag","createTime":"1763312306",
              "author":{"uniqueId":"khaby.lame","nickname":"Khabane lame","signature":"bio line"},
              "stats":{"playCount":11600000,"diggCount":774900,"commentCount":7092,"shareCount":0},
              "challenges":[{"title":"tag"}],
              "music":{"title":"original sound","authorName":"Khabane lame"}}}}}"#,
        );
        let md = tiktok_page(&video).expect("video parsed");
        assert!(md.contains("# A caption with spaces #tag"), "{md}");
        assert!(md.contains("@khaby.lame") && md.contains("Khabane lame"));
        assert!(
            md.contains("2025-11-16"),
            "createTime as a string must still date: {md}"
        );
        assert!(
            md.contains("11.6M plays")
                && md.contains("774.9k likes")
                && md.contains("7.1k comments")
        );
        // A zero count is absence, not a fact worth a line.
        assert!(!md.contains("shares"), "{md}");
        assert!(md.contains("Hashtags: #tag") && md.contains("Sound: original sound"));
        // The caption fits the heading, so it must not be printed twice.
        assert_eq!(md.matches("A caption with spaces").count(), 1, "{md}");

        // TikTok answers 200 with a full shell for a deleted post and says so
        // only in the payload; rendering it would serve an empty page as real.
        let gone = tiktok_html(r#"{"webapp.video-detail":{"statusCode":10204,"itemInfo":{}}}"#);
        assert!(tiktok_page(&gone).is_none());

        let profile = tiktok_html(
            r#"{"webapp.user-detail":{"statusCode":0,"userInfo":{
              "user":{"uniqueId":"nasa","nickname":"NASA","signature":"Making it possible."},
              "stats":{"followerCount":235600,"followingCount":20,"heartCount":872100,"videoCount":16}}}}"#,
        );
        let md = tiktok_page(&profile).expect("profile parsed");
        assert!(md.contains("# NASA (@nasa) on TikTok"), "{md}");
        assert!(md.contains("235.6k followers") && md.contains("16 videos"));
        assert!(md.contains("Making it possible."));
    }

    /// The list pages are React shells; the reader forwards the site's own
    /// filter string to the search API, which speaks the same query language.
    #[test]
    fn github_issue_lists_become_search_api_queries() {
        let url_of = |u: &str| plan(u).map(|p| p.fetch_url);
        assert_eq!(
            url_of("https://github.com/rust-lang/rust/issues").as_deref(),
            Some("https://api.github.com/search/issues?q=repo%3Arust-lang%2Frust+is%3Aissue+is%3Aopen&per_page=30")
        );
        assert_eq!(
            url_of("https://github.com/rust-lang/rust/pulls").as_deref(),
            Some("https://api.github.com/search/issues?q=repo%3Arust-lang%2Frust+is%3Apr+is%3Aopen&per_page=30")
        );
        assert_eq!(
            url_of("https://github.com/rust-lang/rust/issues?q=is%3Aissue+label%3AA-lints").as_deref(),
            Some("https://api.github.com/search/issues?q=repo%3Arust-lang%2Frust+is%3Aissue+label%3AA-lints&per_page=30")
        );
        // A filter that already pins a repo must not gain a second `repo:`
        // qualifier — the two would AND together and match nothing.
        assert_eq!(
            url_of("https://github.com/a/b/issues?q=repo%3Ac%2Fd+is%3Aopen").as_deref(),
            Some("https://api.github.com/search/issues?q=repo%3Ac%2Fd+is%3Aopen&per_page=30")
        );
        // The DETAIL arms must keep their URLs.
        assert!(url_of("https://github.com/a/b/issues/12")
            .is_some_and(|u| u == "https://api.github.com/repos/a/b/issues/12"));
        assert!(url_of("https://github.com/a/b/pull/12")
            .is_some_and(|u| u == "https://api.github.com/repos/a/b/issues/12"));
    }

    #[test]
    fn github_issue_search_renders_results_and_folds_on_an_empty_set() {
        let body = r#"{"total_count":11398,"items":[
          {"number":160052,"title":"a diagnostic is wrong","state":"open","comments":3,
           "updated_at":"2026-07-28T10:00:00Z","user":{"login":"mouse"},
           "labels":[{"name":"A-diagnostics"},{"name":"T-compiler"}],
           "html_url":"https://github.com/rust-lang/rust/issues/160052"},
          {"number":9,"title":"a merged pr","state":"closed","comments":0,
           "updated_at":"2026-07-01T10:00:00Z","user":{"login":"bob"},"labels":[],
           "pull_request":{"merged_at":"2026-07-01T11:00:00Z"},
           "html_url":"https://github.com/rust-lang/rust/pull/9"}]}"#;
        let md = github_issue_search(body).expect("results parsed");
        assert!(md.contains("# 11398 matching") && md.contains("> showing 2"));
        assert!(md.contains("## #160052 a diagnostic is wrong"), "{md}");
        assert!(
            md.contains(
                "open • by mouse • 3 comments • updated 2026-07-28 • A-diagnostics, T-compiler"
            ),
            "{md}"
        );
        // `state` alone calls a merged PR "closed"; the page does not.
        assert!(md.contains("merged • by bob"), "{md}");

        // Zero matches may mean "the API did not understand this filter", so
        // the generic page — which does understand it — must win.
        assert!(github_issue_search(r#"{"total_count":0,"items":[]}"#).is_none());
        // Rate-limited / malformed: no `items` at all.
        assert!(github_issue_search(r#"{"message":"API rate limit exceeded"}"#).is_none());
    }

    /// The credit assiette hinges on this split, so pin both families.
    #[test]
    fn html_page_readers_are_separated_from_api_readers() {
        // Fetch a real page of the same content -> honest html->markdown credit.
        for k in [
            Kind::RedditThread,
            Kind::RedditListing,
            Kind::TikTok,
            Kind::StackExchange,
            Kind::YouTube,
        ] {
            assert!(fetches_html_page(&k));
        }
        assert!(!fetches_html_page(&Kind::GithubIssueSearch));
        // Fetch an alternate machine representation -> flat reliability credit;
        // crediting the payload would invent a saving the agent never paid.
        for k in [
            Kind::GithubIssue,
            Kind::GithubBlob("rust"),
            Kind::GithubActionsRun,
            Kind::GitlabThread,
            Kind::NotionPage,
            Kind::HnItem,
            Kind::CratesApi,
            Kind::Mastodon,
            Kind::Bluesky,
            Kind::Lemmy,
            Kind::XPost,
            Kind::DiscourseTopic,
        ] {
            assert!(!fetches_html_page(&k));
        }
    }

    #[test]
    fn feed_and_huggingface_plans() {
        for u in [
            "https://blog.rust-lang.org/feed.xml",
            "https://news.ycombinator.com/rss",
            "https://example.com/blog/atom",
            "https://example.com/x.rss",
        ] {
            assert!(
                matches!(plan(u).map(|p| p.kind), Some(Kind::FeedDocument)),
                "{u}"
            );
        }
        // Bare and owner-qualified model paths both resolve; datasets have
        // their own endpoint.
        let m = plan("https://huggingface.co/bert-base-uncased").expect("bare model");
        assert_eq!(
            m.fetch_url,
            "https://huggingface.co/api/models/bert-base-uncased"
        );
        let m = plan("https://huggingface.co/google-bert/bert-base-uncased").expect("model");
        assert_eq!(
            m.fetch_url,
            "https://huggingface.co/api/models/google-bert/bert-base-uncased"
        );
        let d = plan("https://huggingface.co/datasets/rajpurkar/squad").expect("dataset");
        assert_eq!(
            d.fetch_url,
            "https://huggingface.co/api/datasets/rajpurkar/squad"
        );
        // Site chrome must not be mistaken for a model.
        for u in [
            "https://huggingface.co/docs",
            "https://huggingface.co/pricing",
        ] {
            assert!(plan(u).is_none(), "{u}");
        }
        // Google Docs deliberately has NO reader: `export?format=txt` 404s on
        // a published doc (that form exists only for /document/d/{fileId}) and
        // the /pub page sits behind a cookie-consent wall. MEASURED on two real
        // published documents — do not re-add without new evidence.
        assert!(plan("https://docs.google.com/document/d/e/2PACX-abc/pub").is_none());
    }

    #[test]
    fn product_facts_reads_json_ld_and_rejects_category_pages() {
        let ld = r#"<html><head><script type="application/ld+json">
            {"@context":"https://schema.org","@graph":[
              {"@type":"WebPage","name":"ignore me"},
              {"@type":"Product","name":"WooCommerce Subscriptions","sku":"WOO508",
               "brand":{"@type":"Brand","name":"Woo"},
               "description":"Let customers subscribe.",
               "aggregateRating":{"ratingValue":"3.21","reviewCount":112},
               "offers":{"@type":"Offer","price":"248.00","priceCurrency":"EUR",
                         "availability":"https://schema.org/InStock"}}]}
            </script></head></html>"#;
        let md = product_facts(ld).expect("json-ld product");
        assert!(md.starts_with("# WooCommerce Subscriptions"), "{md}");
        assert!(md.contains("**248.00 EUR**"), "{md}");
        // schema.org spells availability as a URL; only the leaf is useful.
        assert!(
            md.contains("InStock") && !md.contains("schema.org/InStock"),
            "{md}"
        );
        assert!(
            md.contains("brand: Woo") && md.contains("3.21/5 (112 reviews)"),
            "{md}"
        );
        assert!(md.contains("sku: WOO508"), "{md}");

        // OpenGraph-only page, no ld+json at all.
        let og = r#"<meta property="og:type" content="product">
            <meta property="og:title" content="Tree Runners">
            <meta property="product:price:amount" content="98">
            <meta property="product:price:currency" content="USD">"#;
        let md = product_facts(og).expect("og product");
        assert!(
            md.starts_with("# Tree Runners") && md.contains("**98 USD**"),
            "{md}"
        );

        // `product.group` is a CATEGORY listing — IKEA labels its "all
        // products" page that way, and a substring match served it as an
        // article for sale. Exact match only.
        assert!(product_facts(
            r#"<meta property="og:type" content="product.group">
            <meta property="og:title" content="Tous nos produits">"#
        )
        .is_none());
        assert!(product_facts(r#"<meta property="og:type" content="website">"#).is_none());
        assert!(product_facts("<html><body>no product here</body></html>").is_none());
    }

    /// Regressions from the 2026-07-19 audit. Each case aborted, panicked or
    /// returned wrong data against real-world input.
    #[test]
    fn audit_regressions_cycles_boundaries_and_types() {
        // A cycle among CONTAINER blocks: they emit no text and do not deepen,
        // so the size cap never fired and the recursion overflowed the stack —
        // an abort, not a catchable panic, on remote server data.
        let cyclic = format!(
            "{}{AUX_SEP}{}",
            notion_root_part("b"),
            r#"{"recordMap":{"block":{
              "b":{"value":{"value":{"id":"b","type":"column_list","content":["c"]}}},
              "c":{"value":{"value":{"id":"c","type":"column","content":["b"]}}}
            }}}"#
        );
        assert!(
            notion_page(&cyclic).is_none(),
            "cycle must terminate, not abort"
        );

        // Multi-byte quote char: `&after[1..]` split inside it.
        let atom = r#"<feed><title>T</title><entry><title>P</title>
            <link href=»https://a/1»/></entry></feed>"#;
        let _ = feed_document(atom); // must not panic

        // `to_lowercase()` changes byte length; offsets taken from it no longer
        // index `xml`. A Turkish title silently dropped every entry.
        let tr = "<?xml version=\"1.0\"?><rss><channel><title>İİİİİİİİ</title>\
            <item><title>Birinci</title><link>https://a/1</link></item>\
            <item><title>İkinci</title><link>https://a/2</link></item></channel></rss>";
        let md = feed_document(tr).expect("Turkish feed must still parse");
        assert!(md.contains("Birinci") && md.contains("İkinci"), "{md}");

        // WordPress publishes the full post in `content:encoded`.
        let we = r#"<rss><channel><title>B</title><item><title>P</title>
            <content:encoded><![CDATA[<p>Full post body</p>]]></content:encoded>
            </item></channel></rss>"#;
        assert!(feed_document(we)
            .expect("wp feed")
            .contains("Full post body"));

        // `contains("Product")` matched ProductGroup — the schema.org type for
        // the very category listings the og:type check rejects.
        let group = r#"<script type="application/ld+json">
            {"@type":"ProductGroup","name":"Tous nos produits","brand":{"name":"IKEA"}}
            </script>"#;
        assert!(product_facts(group).is_none(), "ProductGroup is a category");

        // Blob URLs carry `#L10` / `?plain=1`; both 404 on raw.githubusercontent.
        for u in [
            "https://github.com/rust-lang/rust/blob/master/README.md#L10-L20",
            "https://github.com/rust-lang/rust/blob/master/README.md?plain=1",
        ] {
            assert_eq!(
                plan(u).expect("blob").fetch_url,
                "https://raw.githubusercontent.com/rust-lang/rust/master/README.md",
                "{u}"
            );
        }

        // Relative feed href resolves against the page's directory.
        assert_eq!(
            declared_feed_url(
                r#"<link type="application/rss+xml" href="feed.xml">"#,
                "https://x.dev/blog/"
            )
            .as_deref(),
            Some("https://x.dev/blog/feed.xml")
        );
        // Feed URLs commonly carry a query string.
        for u in [
            "https://x.dev/feed/?utm_source=rss",
            "https://x.dev/index.rss?x=1",
        ] {
            assert!(
                matches!(plan(u).map(|p| p.kind), Some(Kind::FeedDocument)),
                "{u}"
            );
        }
    }

    #[test]
    fn feed_discovery_resolves_and_is_index_only() {
        let html = r#"<html><head>
            <link rel="alternate" type="application/rss+xml" href="/rss" title="News">
            </head></html>"#;
        assert_eq!(
            declared_feed_url(html, "https://www.independent.co.uk/").as_deref(),
            Some("https://www.independent.co.uk/rss")
        );
        // Absolute, protocol-relative and bare-relative hrefs all resolve.
        for (href, want) in [
            ("https://x.dev/f.xml", "https://x.dev/f.xml"),
            ("//cdn.x.dev/f.xml", "https://cdn.x.dev/f.xml"),
            ("feed.xml", "https://a.dev/feed.xml"),
        ] {
            let h = format!(r#"<link type="application/atom+xml" href="{href}">"#);
            assert_eq!(
                declared_feed_url(&h, "https://a.dev/").as_deref(),
                Some(want),
                "{href}"
            );
        }
        assert!(declared_feed_url("<html><head></head></html>", "https://a.dev/").is_none());

        // Substituting a site's LATEST items is only honest at the top of the
        // site — never for a specific document the user asked for.
        for u in [
            "https://a.dev/",
            "https://a.dev",
            "https://a.dev/news",
            "https://a.dev/news/",
        ] {
            assert!(is_site_index(u), "{u}");
        }
        for u in [
            "https://a.dev/news/world/story-b123.html",
            "https://a.dev/2026/07/19/post",
        ] {
            assert!(!is_site_index(u), "{u}");
        }
    }

    #[test]
    fn a_titleless_microblog_feed_still_renders_its_posts() {
        // Mastodon's shape: a toot has no title, so every item carries only
        // description/link/pubDate. The reader skipped titleless items, so a
        // valid 20-item feed rendered as None and the profile fell back to a
        // 188-char shell (MEASURED 2026-07-22 on mastodon.social/@Gargron).
        let rss = r#"<?xml version="1.0"?><rss version="2.0"><channel>
            <title>Eugen Rochko</title><description>Public posts</description>
            <item>
                <link>https://mastodon.social/@Gargron/116948121322707734</link>
                <pubDate>Sun, 19 Jul 2026 18:45:14 +0000</pubDate>
                <description>&lt;p&gt;It's so quiet and peaceful here today.&lt;/p&gt;</description>
            </item>
            <item>
                <link>https://mastodon.social/@Gargron/116948121322707735</link>
                <pubDate>Mon, 20 Jul 2026 09:00:00 +0000</pubDate>
                <description>&lt;p&gt;Second post with different words entirely.&lt;/p&gt;</description>
            </item>
        </channel></rss>"#;
        let md = feed_document(rss).expect("titleless feed must render");
        assert!(md.contains("quiet and peaceful"), "post body lost: {md}");
        assert!(
            md.contains("different words entirely"),
            "second post lost: {md}"
        );
        assert!(md.contains("[2 entries]"), "entries miscounted: {md}");
        // The derived heading must come from the post, not be a placeholder.
        assert!(
            !md.contains("(untitled)"),
            "fell back to a placeholder: {md}"
        );
    }

    #[test]
    fn feed_document_renders_rss_and_atom_and_rejects_non_feeds() {
        let rss = r#"<?xml version="1.0"?><rss version="2.0"><channel>
            <title>Rust Blog</title><description>News</description>
            <item><title><![CDATA[Rust 1.80]]></title>
                <link>https://blog.rust-lang.org/1.80</link>
                <pubDate>Thu, 25 Jul 2024 00:00:00 +0000</pubDate>
                <description>&lt;p&gt;Release notes&lt;/p&gt;</description></item>
            <item><title>Second</title><link>https://x/2</link></item>
            </channel></rss>"#;
        let md = feed_document(rss).expect("rss");
        // The channel title must win over the first item's.
        assert!(md.starts_with("# Rust Blog"), "{md}");
        assert!(md.contains("## Rust 1.80"), "CDATA not unwrapped:\n{md}");
        assert!(md.contains("<https://blog.rust-lang.org/1.80>"), "{md}");
        assert!(
            md.contains("Release notes") && !md.contains("<p>"),
            "html kept:\n{md}"
        );
        assert!(md.contains("[2 entries]"), "{md}");

        // Atom keeps its link in an ATTRIBUTE, not the element body.
        let atom = r#"<feed xmlns="http://www.w3.org/2005/Atom"><title>A</title>
            <entry><title>Post</title><link href="https://a/1"/>
            <updated>2026-07-19</updated><summary>S</summary></entry></feed>"#;
        let md = feed_document(atom).expect("atom");
        assert!(
            md.contains("## Post") && md.contains("<https://a/1>"),
            "{md}"
        );
        assert!(md.contains("2026-07-19"), "{md}");

        // A feed shape with no entries is not a feed; nor is arbitrary XML.
        assert!(feed_document(r#"<rss><channel><title>T</title></channel></rss>"#).is_none());
        assert!(feed_document("<html><body>hi</body></html>").is_none());
    }

    #[test]
    fn huggingface_repo_renders_api_and_card() {
        let api = r#"{"id":"google-bert/bert-base-uncased","pipeline_tag":"fill-mask",
            "downloads":78430360,"likes":2710,"cardData":{"license":"apache-2.0"},
            "tags":["transformers","bert"]}"#;
        let card = "---\nlanguage: en\nlicense: apache-2.0\n---\n# BERT base model\nBody.";
        let md = huggingface_repo(&format!("{api}{AUX_SEP}{card}")).expect("hf");
        assert!(md.starts_with("# google-bert/bert-base-uncased"), "{md}");
        assert!(
            md.contains("fill-mask · 78430360 downloads · 2710 likes · license: apache-2.0"),
            "{md}"
        );
        assert!(md.contains("tags: transformers, bert"), "{md}");
        // Front matter duplicates the API fields and is stripped.
        assert!(
            md.contains("# BERT base model") && !md.contains("language: en"),
            "{md}"
        );
        assert!(huggingface_repo("{}").is_none());

        // A published doc exports plain text; an unpublished one yields the
        // sign-in HTML page, which must NOT be served as the document.
    }

    #[test]
    fn github_blob_actions_and_gitlab_plans() {
        let b = plan("https://github.com/rust-lang/rust/blob/master/library/core/src/option.rs")
            .expect("blob");
        assert_eq!(
            b.fetch_url,
            "https://raw.githubusercontent.com/rust-lang/rust/master/library/core/src/option.rs"
        );
        let a = plan("https://github.com/actions/runner/actions/runs/29667487879").expect("run");
        assert!(a.fetch_url.ends_with("/actions/runs/29667487879"));
        assert!(a.aux_url.expect("jobs").ends_with("/jobs"));
        // GitLab keys projects by URL-ENCODED full path, nested groups included.
        let g = plan("https://gitlab.com/gitlab-org/sub/gitlab/-/merge_requests/7").expect("mr");
        assert_eq!(
            g.fetch_url,
            "https://gitlab.com/api/v4/projects/gitlab-org%2Fsub%2Fgitlab/merge_requests/7"
        );
        assert!(plan("https://gitlab.com/gitlab-org/gitlab").is_none());
    }

    #[test]
    fn github_blob_fences_and_rejects_missing_files() {
        let md = github_blob("fn main() {}\n", "rust").expect("renders");
        assert!(
            md.starts_with("```rust\n") && md.contains("fn main() {}"),
            "{md}"
        );
        // raw.githubusercontent answers a plain-text 404 body.
        assert!(github_blob("404: Not Found", "rust").is_none());
        assert!(github_blob("   ", "rust").is_none());
        // An unknown extension gets a bare fence, never a wrong highlight.
        assert!(github_blob("x\n", "")
            .expect("renders")
            .starts_with("```\n"));
    }

    /// The language rides inside the `Kind` because [`render`] never sees the
    /// path. Until it did, every blob came back on a bare fence.
    #[test]
    fn a_blob_plan_carries_the_fence_language_of_its_extension() {
        let lang_of = |u: &str| match plan(u).map(|p| p.kind) {
            Some(Kind::GithubBlob(l)) => l,
            other => panic!("{u} is not a blob plan: {}", other.is_some()),
        };
        assert_eq!(
            lang_of("https://github.com/o/r/blob/main/src/lib.rs"),
            "rust"
        );
        assert_eq!(
            lang_of("https://github.com/o/r/blob/main/a/b/setup.py"),
            "python"
        );
        assert_eq!(
            lang_of("https://github.com/o/r/blob/main/Cargo.toml"),
            "toml"
        );
        assert_eq!(
            lang_of("https://github.com/o/r/blob/main/README.md"),
            "markdown"
        );
        // No extension, and one nobody highlights: a bare fence, not a guess.
        assert_eq!(lang_of("https://github.com/o/r/blob/main/LICENSE"), "");
        assert_eq!(lang_of("https://github.com/o/r/blob/main/x.zzz"), "");
    }

    #[test]
    fn actions_run_surfaces_only_the_failed_steps() {
        let head = r#"{"name":"CI","run_number":42,"status":"completed",
            "conclusion":"failure","head_branch":"main","event":"push",
            "head_commit":{"message":"fix: thing\nbody"}}"#;
        let jobs = r#"{"jobs":[
            {"name":"build","conclusion":"success","steps":[{"name":"compile","conclusion":"success"}]},
            {"name":"test","conclusion":"failure","steps":[
                {"name":"setup","conclusion":"success"},
                {"name":"cargo test","conclusion":"failure"}]}]}"#;
        let md = github_actions_run(&format!("{head}{AUX_SEP}{jobs}")).expect("renders");
        assert!(md.contains("# CI · run #42"), "{md}");
        assert!(
            md.contains("completed (failure) · on main · triggered by push"),
            "{md}"
        );
        assert!(
            md.contains("commit: fix: thing") && !md.contains("body"),
            "{md}"
        );
        assert!(md.contains("- **test** — failure"), "{md}");
        assert!(md.contains("FAILED step: cargo test"), "{md}");
        // A green job's steps are noise and must stay out.
        assert!(!md.contains("compile"), "{md}");
        assert!(github_actions_run("{}").is_none());
    }

    #[test]
    fn gitlab_thread_renders_and_declares_the_missing_comments() {
        let issue = r#"{"title":"Expand variables","iid":500000,"state":"opened",
            "author":{"username":"alice"},"labels":["bug","pages"],
            "description":"Steps to reproduce.","user_notes_count":7}"#;
        let md = gitlab_thread(issue).expect("renders");
        assert!(md.starts_with("# Expand variables"), "{md}");
        assert!(
            md.contains("Issue !500000 · opened · opened by @alice"),
            "{md}"
        );
        assert!(md.contains("labels: bug, pages"), "{md}");
        // The 401 on /notes is disclosed, not silently dropped.
        assert!(md.contains("[7 comments not shown"), "{md}");

        let mr = r#"{"title":"Fix","iid":7,"state":"merged","author":{"username":"bob"},
            "source_branch":"fix","target_branch":"main"}"#;
        let md = gitlab_thread(mr).expect("renders");
        assert!(
            md.contains("MR !7 · merged") && md.contains("fix → main"),
            "{md}"
        );
        assert!(gitlab_thread(r#"{"message":"404 Not found"}"#).is_none());
    }

    #[test]
    fn github_plan_maps_issues_and_prs_to_the_rest_api() {
        for (url, num) in [
            ("https://github.com/rust-lang/rust/issues/100000", "100000"),
            ("https://github.com/rust-lang/rust/pull/129021", "129021"),
        ] {
            let p = plan(url).expect("should plan");
            assert_eq!(
                p.fetch_url,
                format!("https://api.github.com/repos/rust-lang/rust/issues/{num}")
            );
            assert!(p
                .aux_url
                .expect("comments")
                .ends_with("/comments?per_page=30"));
        }
        // What is not a numbered thread is no longer generic for all that: a
        // repository's home page has had its own reader since then.
        assert!(matches!(
            plan("https://github.com/rust-lang/rust").map(|p| p.kind),
            Some(Kind::GithubRepo)
        ));
        // The unnumbered LIST is not unplanned either — it is the React shell
        // the search reader exists for; assert the routing, not an absence
        // that stopped being true.
        assert!(matches!(
            plan("https://github.com/rust-lang/rust/issues").map(|p| p.kind),
            Some(Kind::GithubIssueSearch)
        ));
        // A blob URL is NOT unplanned — it has its own reader now; assert the
        // routing rather than an absence that stopped being true.
        assert!(matches!(
            plan("https://github.com/rust-lang/rust/blob/main/README.md").map(|p| p.kind),
            Some(Kind::GithubBlob(_))
        ));
    }

    #[test]
    fn github_issue_renders_thread_and_degrades_gracefully() {
        let head = r#"{"title":"Rust is Beautiful","number":100000,"state":"closed",
            "user":{"login":"Mathspy"},"body":"100,000 issues filled with love.",
            "comments":255,"labels":[{"name":"C-enhancement"}]}"#;
        let comments = r#"[{"user":{"login":"chop0"},"body":"shush"},
                           {"user":{"login":"a"},"body":"  "}]"#;
        let md = github_issue(&format!("{head}{AUX_SEP}{comments}")).expect("renders");
        assert!(md.starts_with("# Rust is Beautiful"), "{md}");
        assert!(
            md.contains("Issue #100000 · closed · opened by @Mathspy"),
            "{md}"
        );
        assert!(md.contains("labels: C-enhancement"), "{md}");
        assert!(md.contains("**@chop0**\nshush"), "{md}");
        // Empty comment bodies are skipped, and the tail is accounted for.
        assert!(!md.contains("**@a**"), "{md}");
        assert!(md.contains("[254 more comments]"), "{md}");

        // `pull_request` marks a PR — same endpoint serves both.
        let pr = r#"{"title":"T","number":1,"state":"open","user":{"login":"x"},
            "pull_request":{"url":"…"}}"#;
        assert!(github_issue(pr).expect("renders").contains("PR #1 · open"));
        // Rate-limited / not-found bodies carry no title → generic pipeline.
        assert!(github_issue(r#"{"message":"API rate limit exceeded"}"#).is_none());
        assert!(github_issue("not json").is_none());
    }

    #[test]
    fn notion_urls_plan_via_the_public_api() {
        // Slug words contribute stray hex letters (cafe/dead/…) — the id is
        // the TAIL 32, and it must come back hyphenated 8-4-4-4-12.
        let p =
            plan("https://app.notion.com/p/Cafe-Deed-Skills-SEO-36b82f65b36f8094bc77fe4377891822")
                .expect("notion page URL should plan");
        assert_eq!(p.fetch_url, "https://www.notion.so/api/v3/loadPageChunk");
        let body = p.post_body.expect("notion must POST");
        assert!(
            body.contains("\"pageId\":\"36b82f65-b36f-8094-bc77-fe4377891822\""),
            "id not hyphenated: {body}"
        );
        for u in [
            "https://www.notion.so/36b82f65b36f8094bc77fe4377891822",
            "https://myteam.notion.site/Some-Page-36b82f65b36f8094bc77fe4377891822",
        ] {
            assert!(plan(u).is_some(), "should plan: {u}");
        }
        // No id in the path → no opinion, generic pipeline handles it.
        assert!(plan("https://www.notion.so/pricing").is_none());
        assert!(plan("https://app.notion.com/").is_none());
    }

    #[test]
    fn notion_pins_the_requested_root_and_renders_databases() {
        // `queryCollection` drags in the workspace's parent pages: they are
        // `page` blocks nothing lists as a child, so the "orphan page"
        // heuristic alone renders the WRONG document. The pinned id wins.
        let chunk = r#"{"recordMap":{"block":{
            "parent":{"value":{"value":{"id":"parent","type":"page",
                "properties":{"title":[["Wiki d'accueil"]]}}}},
            "want":{"value":{"value":{"id":"want","type":"page",
                "properties":{"title":[["La bonne page"]]},"content":["cv","col"]}}},
            "cv":{"value":{"value":{"id":"cv","type":"collection_view",
                "collection_id":"c1","view_ids":["v1"],
                "format":{"collection_pointer":{"spaceId":"s1"}}}}},
            "col":{"value":{"value":{"id":"col","type":"column_list","content":["t"]}}},
            "t":{"value":{"value":{"id":"t","type":"text",
                "properties":{"title":[["texte en colonne"]]}}}}
        }},"cursor":{"stack":[]}}"#;
        // Rows come back from the second endpoint, parented to the collection.
        let rows = r#"{"recordMap":{"collection":{"c1":{"value":{"value":{"id":"c1",
                "name":[["Feuille de route"]],
                "schema":{"title":{"name":"Nom","type":"title"},
                          "Ab1":{"name":"Statut","type":"select"}}}}}},
            "block":{
            "r1":{"value":{"value":{"id":"r1","type":"page","parent_id":"c1",
                "parent_table":"collection",
                "properties":{"title":[["Zebre"]],"Ab1":[["Livre"]]}}}},
            "r2":{"value":{"value":{"id":"r2","type":"page","parent_id":"c1",
                "parent_table":"collection",
                "properties":{"title":[["Alpha"]],"Ab1":[["En cours"]]}}}}
        }}}"#;
        let payload = format!(
            "{}{AUX_SEP}{chunk}{AUX_SEP}{rows}",
            notion_root_part("want")
        );
        let md = notion_page(&payload).expect("should render");
        assert!(md.starts_with("# La bonne page"), "wrong root:\n{md}");
        assert!(
            !md.contains("Wiki d'accueil"),
            "parent page leaked in:\n{md}"
        );
        assert!(
            md.contains("**Feuille de route** (database, 2 rows)"),
            "no db name:\n{md}"
        );
        assert!(
            md.contains("| Nom | Statut |"),
            "title column must lead:\n{md}"
        );
        // Sorted by title so repeated fetches render identically.
        assert!(
            md.find("| Alpha |") < md.find("| Zebre |"),
            "rows must be title-sorted:\n{md}"
        );
        // A column wrapper contributes no line and must not indent its prose.
        assert!(
            md.contains("\n\ntexte en colonne"),
            "column prose indented:\n{md}"
        );
    }

    #[test]
    fn notion_chunk_and_collection_requests_are_well_formed() {
        let b = notion_chunk_body("aa-bb", 2, r#"{"stack":[["x"]]}"#);
        assert!(
            b.contains(r#""chunkNumber":2"#) && b.contains(r#""stack":[["x"]]"#),
            "{b}"
        );
        assert_eq!(
            notion_next_cursor(r#"{"cursor":{"stack":[["a"]]}}"#).as_deref(),
            Some(r#"{"stack":[["a"]]}"#)
        );
        // Drained page → no further chunk.
        assert!(notion_next_cursor(r#"{"cursor":{"stack":[]}}"#).is_none());
        assert!(notion_next_cursor("{}").is_none());

        let chunk = r#"{"recordMap":{"block":{
            "a":{"value":{"value":{"id":"a","type":"collection_view","collection_id":"c1",
                "view_ids":["v1"],"format":{"collection_pointer":{"spaceId":"s1"}}}}},
            "b":{"value":{"value":{"id":"b","type":"collection_view","collection_id":"c1",
                "view_ids":["v9"],"format":{"collection_pointer":{"spaceId":"s1"}}}}},
            "c":{"value":{"value":{"id":"c","type":"text"}}}
        }}}"#;
        let qs = notion_collection_queries(chunk);
        // Two views onto ONE database → one query, not two.
        assert_eq!(qs.len(), 1, "{qs:?}");
        assert!(
            qs[0].contains(r#""id":"c1""#) && qs[0].contains(r#""spaceId":"s1""#),
            "{}",
            qs[0]
        );
    }

    #[test]
    fn notion_page_renders_blocks_and_rejects_errors() {
        let body = r#"{"recordMap":{"block":{
            "root":{"value":{"value":{"id":"root","type":"page",
                "properties":{"title":[["Titre"]]},"content":["h","b","c"]}}},
            "h":{"value":{"value":{"id":"h","type":"sub_header",
                "properties":{"title":[["Section"]]}}}},
            "b":{"value":{"value":{"id":"b","type":"bulleted_list",
                "properties":{"title":[["voir "],["ici",[["a","https://x.dev"]]]]}}}},
            "c":{"value":{"value":{"id":"c","type":"code",
                "properties":{"title":[["let x = 1;"]],"language":[["Rust"]]}}}}
        }}}"#;
        let md = notion_page(body).expect("should render");
        assert!(md.starts_with("# Titre"), "root title missing:\n{md}");
        assert!(md.contains("### Section"), "heading missing:\n{md}");
        assert!(
            md.contains("- voir ici <https://x.dev>"),
            "inline link lost:\n{md}"
        );
        assert!(
            md.contains("```rust\nlet x = 1;\n```"),
            "code block lost:\n{md}"
        );
        // A private/deleted page answers with an error object, no recordMap.
        assert!(notion_page(r#"{"errorId":"x","name":"UnauthorizedError"}"#).is_none());
        assert!(notion_page("not json").is_none());
    }

    #[test]
    fn plan_maps_reddit_hosts_to_the_atom_feed() {
        for host in [
            "www.reddit.com",
            "reddit.com",
            "old.reddit.com",
            "np.reddit.com",
        ] {
            let p = plan(&format!(
                "https://{host}/r/rust/comments/abc123/some_title/?sort=top"
            ))
            .expect(host);
            assert_eq!(
                p.fetch_url,
                "https://www.reddit.com/r/rust/comments/abc123/some_title/.rss"
            );
            assert!(matches!(p.kind, Kind::RedditThread));
        }
        // A subreddit front page is not unplanned either — new-reddit walls it
        // and the listing reader took it over; the URLs with no submission list
        // are the ones that still fall through.
        assert!(matches!(
            plan("https://www.reddit.com/r/rust/").map(|p| p.kind),
            Some(Kind::RedditListing)
        ));
        assert!(plan("https://www.reddit.com/r/rust/about/rules").is_none());
    }

    /// **A GitHub repository's home page**, the most requested address and the
    /// only one that had no reader: the delivered page gave "Uh oh! There was
    /// an error while loading" twice and not one line of README.
    #[test]
    fn plan_maps_a_github_repo_home_to_the_api_and_its_readme() {
        let p = plan("https://github.com/rust-lang/regex").expect("plan");
        assert_eq!(p.fetch_url, "https://api.github.com/repos/rust-lang/regex");
        assert_eq!(
            p.aux_url.as_deref(),
            Some("https://raw.githubusercontent.com/rust-lang/regex/HEAD/README.md")
        );
        assert!(matches!(p.kind, Kind::GithubRepo));
        // Neighbouring shapes keep THEIR reader, and the segments GitHub
        // reserves for itself are not accounts.
        assert!(matches!(
            plan("https://github.com/rust-lang/regex/issues/42").map(|p| p.kind),
            Some(Kind::GithubIssue)
        ));
        assert!(plan("https://github.com/features/actions").is_none());
        assert!(plan("https://github.com/rust-lang").is_none());
    }

    #[test]
    fn a_github_repo_renders_its_facts_then_its_readme() {
        let api = r#"{"full_name":"rust-lang/regex","description":"An implementation of regular expressions for Rust.","stargazers_count":4019,"forks_count":531,"open_issues_count":79,"language":"Rust","license":{"spdx_id":"Apache-2.0"},"pushed_at":"2026-08-10T16:19:27Z","homepage":"https://docs.rs/regex","topics":["automata","regex"],"archived":false}"#;
        let md = render(
            &Kind::GithubRepo,
            &format!("{api}{AUX_SEP}regex\n=====\n\nCe crate cherche des motifs dans du texte."),
        )
        .expect("the card");
        assert!(md.starts_with("# rust-lang/regex"), "{md}");
        assert!(md.contains("4019 stars · 531 forks"), "{md}");
        assert!(md.contains("Apache-2.0"), "{md}");
        assert!(md.contains("topics: automata, regex"), "{md}");
        assert!(md.contains("## README"), "{md}");
        assert!(md.contains("Ce crate cherche des motifs"), "{md}");
    }

    /// Two bounds. The API answers `{"message":"Not Found"}` — or the rate
    /// limit — without `full_name`, and serving that message as a repository
    /// would deliver an error as content. And `raw` answers "404: Not Found" as
    /// PLAIN TEXT when the repository has no `README.md`.
    #[test]
    fn an_error_body_is_not_a_repo_and_a_missing_readme_is_not_one_either() {
        // The PURE function, not `render`: the 60-character floor would catch
        // a bogus card ("# ?" and three zero counters) and the test would pass
        // without proving anything about this particular guard.
        assert!(forges::github_repo(r#"{"message":"Not Found"}"#).is_none());
        assert!(forges::github_repo(
            r#"{"message":"API rate limit exceeded for 1.2.3.4.","documentation_url":"https://docs.github.com/rest"}"#
        )
        .is_none());
        let md = render(
            &Kind::GithubRepo,
            &format!(
                "{}{AUX_SEP}404: Not Found",
                r#"{"full_name":"a/b","description":"Un depot sans README du tout ici.","stargazers_count":1,"forks_count":0,"open_issues_count":0}"#
            ),
        )
        .expect("the card");
        assert!(!md.contains("404"), "raw's 404 passed for a README:\n{md}");
        assert!(!md.contains("## README"), "{md}");
    }

    #[test]
    fn plan_maps_crates_io_to_registry_api() {
        let p = plan("https://crates.io/crates/serde").unwrap();
        assert_eq!(p.fetch_url, "https://crates.io/api/v1/crates/serde");
        let p = plan("https://crates.io/crates/serde/1.0.228?tab=versions").unwrap();
        assert_eq!(p.fetch_url, "https://crates.io/api/v1/crates/serde");
        assert!(plan("https://crates.io/").is_none());
        assert!(plan("https://example.com/crates/serde").is_none());
    }

    /// **A feed cut at the cap no longer reads like a complete feed.**
    /// `[25 entries]` on a 300-reply thread was indistinguishable from a
    /// thread that has 25.
    #[test]
    fn a_feed_cut_at_the_cap_says_how_many_it_left() {
        let entry = |i: usize| {
            format!(
                "<entry><title>Entree numero {i}</title><link href=\"https://x.test/{i}\"/>\
                 <summary>Le corps de l'entree numero {i}, assez long pour compter.</summary>\
                 </entry>"
            )
        };
        let many: String = (0..60).map(entry).collect();
        let md = render(
            &Kind::FeedDocument,
            &format!("<feed><title>Un flux long</title>{many}</feed>"),
        )
        .expect("the feed");
        assert!(
            md.contains("of 60 entries — the rest was cut at the reader's cap"),
            "the cap is silent:\n{}",
            md.chars().rev().take(200).collect::<String>()
        );
        // The bound: a feed that fits whole does not announce itself as cut.
        let short = render(
            &Kind::FeedDocument,
            &format!(
                "<feed><title>Un flux court</title>{}{}</feed>",
                entry(1),
                entry(2)
            ),
        )
        .expect("the short feed");
        assert!(short.contains("[2 entries]"), "{short}");
    }

    /// **The Atom feed is the only anonymous source left, and it carries the
    /// COMMENTS.** The body has the shape reddit really serves: the
    /// submission, then one entry per comment titled `/u/<author> on
    /// <thread>`. Measured 2026-09-11, 5 entries on an r/rust thread.
    #[test]
    fn a_reddit_thread_comes_from_its_atom_feed_with_its_comments() {
        let atom = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
<category term="rust" label="r/rust"/>
<updated>2026-09-10T09:09:24+00:00</updated>
<title>This Week in Rust #668 : rust</title>
<entry><author><name>/u/b-dillo</name></author>
<content type="html">&lt;div class="md"&gt;&lt;p&gt;Le billet de la semaine, avec assez de texte pour que la page ne soit pas jugee trop mince par le plancher generique du lecteur.&lt;/p&gt;&lt;/div&gt;</content>
<link href="https://www.reddit.com/r/rust/comments/1wc8sbm/this_week_in_rust_668/"/>
<updated>2026-09-10T09:09:24+00:00</updated><title>This Week in Rust #668</title></entry>
<entry><author><name>/u/torsten_dev</name></author>
<content type="html">&lt;div class="md"&gt;&lt;p&gt;La reponse d'un lecteur, assez longue pour que le rendu ne soit pas jugee vide par le plancher du lecteur.&lt;/p&gt;&lt;/div&gt;</content>
<link href="https://www.reddit.com/r/rust/comments/1wc8sbm/this_week_in_rust_668/n8x9/"/>
<title>/u/torsten_dev on This Week in Rust #668</title></entry>
</feed>"#;
        let md = render(&Kind::RedditThread, atom).expect("the thread");
        assert!(md.contains("This Week in Rust #668"), "{md}");
        assert!(
            md.contains("/u/b-dillo") || md.contains("Le billet de la semaine"),
            "{md}"
        );
        assert!(
            md.contains("torsten_dev") && md.contains("La reponse d'un lecteur"),
            "the feed's comments are not rendered:\n{md}"
        );
    }

    /// A listing renders its feed title and its entries.
    #[test]
    fn a_reddit_listing_comes_from_its_atom_feed() {
        let atom = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
<title>The Rust Programming Language</title>
<subtitle>A place for all things related to the Rust programming language.</subtitle>
<entry><author><name>/u/llogiq</name></author>
<content type="html">&lt;p&gt;Mystified about strings? Borrow checker has you in a headlock? Seek help here, il y a assez de texte pour passer le plancher.&lt;/p&gt;</content>
<link href="https://www.reddit.com/r/rust/comments/1w9f7fr/hey_rustaceans/"/>
<title>Hey Rustaceans! Got a question? Ask here (37/2026)!</title></entry>
</feed>"#;
        let md = render(&Kind::RedditListing, atom).expect("the listing");
        assert!(md.contains("Hey Rustaceans"), "{md}");
        assert!(md.contains("The Rust Programming Language"), "{md}");
    }

    #[test]
    fn reddit_thread_parses_title_op_and_comments() {
        let html = r##"<html><head><meta property="og:title" content="My thread title" /></head>
<body>
<div class="thing link" data-type="link" data-author="opuser" data-score="775" data-subreddit-prefixed="r/rust"></div>
<div class="expando"><form class="usertext"><div class="md"><p>OP body text with details.</p></div></form></div>
<a href="/comments" class="bylink comments may-blank">42 comments</a>
<div class=" thing comment " data-type="comment" data-author="alice" data-subreddit-prefixed="r/rust">
  <span class="score unvoted" title="15">15 points</span>
  <form class="usertext"><div class="md"><p>First comment, quite insightful.</p></div></form>
</div>
<div class=" thing comment " data-type="comment" data-author="bob">
  <span class="score unvoted" title="3">3 points</span>
  <form class="usertext"><div class="md"><p>Second comment with <code>code</code>.</p></div></form>
</div>
</body></html>"##;
        let md = reddit_thread(html).unwrap();
        assert!(md.starts_with("# My thread title"), "{md}");
        assert!(
            md.contains("r/rust • by u/opuser • 775 pts • 42 comments"),
            "{md}"
        );
        assert!(md.contains("OP body text with details."), "{md}");
        assert!(
            md.contains("**u/alice** (15 pts): First comment, quite insightful."),
            "{md}"
        );
        assert!(
            md.contains("**u/bob** (3 pts): Second comment with `code`."),
            "{md}"
        );
    }

    #[test]
    fn reddit_keeps_the_op_when_the_expando_carries_extra_classes() {
        // Live old.reddit markup (2026-07): `class="expando
        // expando-uninitialized"`. The exact-match regex silently dropped the
        // OP selftext on EVERY self-post; on a thread with no comments the
        // render then fell under the thin-parse floor and the whole reader
        // failed open — the "reddit.com · reader broke · 10 tok" row.
        let html = r##"<html><head><meta property="og:title" content="Fresh thread" /></head>
<body>
<div class="thing link" data-type="link" data-author="opuser" data-score="1" data-subreddit-prefixed="r/rust" data-comments-count="0"></div>
<div class="expando expando-uninitialized"><form class="usertext"><div class="md"><p>The whole point of the post.</p></div></form></div>
</body></html>"##;
        let md = reddit_thread(html).expect("renders");
        assert!(
            md.contains("The whole point of the post."),
            "OP dropped:\n{md}"
        );
        // …and a zero-comment thread is terse by nature, not a failed parse.
        assert!(
            render(&Kind::RedditThread, html).is_some(),
            "0-comment thread must survive the floor:\n{md}"
        );
        // A page with NO submission tag at all is still a failed parse.
        assert!(render(&Kind::RedditThread, "<html><body>walled</body></html>").is_none());
    }

    #[test]
    fn article_plans_match_article_paths_and_reject_indexes() {
        for (url, via) in [
            (
                "https://www.britannica.com/biography/John-F-Kennedy",
                "britannica.com (article)",
            ),
            (
                "https://www.britannica.com/place/Iceland",
                "britannica.com (article)",
            ),
            // Section subpage of a split article.
            (
                "https://www.britannica.com/biography/John-F-Kennedy/Assassination",
                "britannica.com (article)",
            ),
            (
                "https://www.cbsnews.com/news/coca-cola-fairlife-milk-cyberattack/",
                "cbsnews.com (article)",
            ),
            (
                "https://www.ign.com/articles/pokemon-tcg-pitch-black-valuable-cards",
                "ign.com (article)",
            ),
            (
                "https://www.academia.edu/44869599/A_Survey_on_Machine_Learning",
                "academia.edu (abstract)",
            ),
        ] {
            let p = plan(url).unwrap_or_else(|| panic!("no plan for {url}"));
            assert_eq!(p.via, via, "{url}");
            assert!(matches!(p.kind, Kind::Article(_)), "{url}");
            // These readers re-fetch the page itself — query/fragment dropped.
            assert!(
                p.fetch_url.starts_with("https://") && !p.fetch_url.contains('?'),
                "{url}"
            );
        }
        // Indexes, listings and non-article shapes must fall through: there is
        // no article on them to extract, and rendering one empty would be
        // strictly worse than the generic pipeline.
        for url in [
            "https://www.britannica.com/",
            "https://www.cbsnews.com/",
            "https://www.cbsnews.com/live/",
            "https://www.ign.com/news",
            "https://www.academia.edu/about",
            "https://www.academia.edu/search?q=ml",
        ] {
            assert!(
                plan(url).is_none(),
                "{url} must not claim an article reader"
            );
        }
        // Query strings are stripped, not passed through.
        let p = plan("https://www.ign.com/articles/some-slug?utm_source=x").expect("plan");
        assert!(
            p.fetch_url.ends_with("/articles/some-slug"),
            "{}",
            p.fetch_url
        );
    }

    #[test]
    fn masterclass_reads_the_next_payload_and_rejects_the_soft_404() {
        let page = |article: &str| {
            format!(
                r##"<html><head><title>x</title></head><body><div class="col-lg-8">grid</div>
<script id="__NEXT_DATA__" type="application/json">{{"props":{{"pageProps":{{{article}}}}}}}</script>
</body></html>"##
            )
        };
        let article = r#""article":{"title":"How to Write a Horror Story in 7 Steps",
"author":"MasterClass","updatedAt":"2021-12-08T08:31:50.888-08:00","creationDate":"2019-12-06T11:02:35.690-08:00",
"readTime":274.2,"category":{"name":"Writing"},"descriptionPlainText":"The horror genre extends back to ancient folktales.",
"articleSubsections":[
{"title":"What Is Horror?","description":"<p>Horror is a genre that taps into fear.</p>"},
{"title":"Want to Learn More About Writing?","description":"<p>Get a MasterClass Annual Membership today.</p>"}]}"#;
        let md = masterclass_article(&page(article)).expect("renders");
        assert!(
            md.starts_with("# How to Write a Horror Story in 7 Steps"),
            "{md}"
        );
        // readTime is SECONDS (274.2) — 5 min, never 274.
        assert!(
            md.contains("by MasterClass • 2021-12-08 • Writing • 5 min read"),
            "{md}"
        );
        assert!(md.contains("ancient folktales"), "lede dropped:\n{md}");
        assert!(
            md.contains("## What Is Horror?") && md.contains("taps into fear"),
            "{md}"
        );
        // The closing membership pitch is a section like any other in the
        // payload; its title varies by category, its sales copy does not.
        assert!(
            !md.contains("Annual Membership"),
            "promo section kept:\n{md}"
        );

        // Soft-404: masterclass answers 200 with a full page for a dead slug,
        // and its payload has no `article` at all. Refusing it here is what
        // stops the reader from serving navigation as an article.
        let dead = page(r#""status":404,"experiments":{}"#);
        assert!(
            masterclass_article(&dead).is_none(),
            "soft-404 must not render"
        );
        // No payload / no sections → fail open rather than serve a bare header.
        assert!(masterclass_article("<html><body>no next data</body></html>").is_none());
        assert!(
            masterclass_article(&page(r#""article":{"title":"T","articleSubsections":[]}"#))
                .is_none(),
            "header with no body must not render"
        );

        assert!(matches!(
            plan("https://www.masterclass.com/articles/how-to-write-a-horror-story")
                .map(|p| p.kind),
            Some(Kind::MasterclassArticle)
        ));
        assert!(plan("https://www.masterclass.com/").is_none());
        assert!(plan("https://www.masterclass.com/categories/writing").is_none());
    }

    #[test]
    fn indeed_renders_job_cards_and_only_claims_search_pages() {
        let html = r##"<html><head><title>Generative Ai Llm, Paris (75) : plus de 1 000 emplois | Indeed</title>
<link rel="canonical" href="https://fr.indeed.com/q-generative-ai-llm-l-paris-emplois.html"/></head>
<body><div data-tn="mosaic-provider-jobcards">card chrome</div>
<script>window.mosaic.providerData["mosaic-provider-jobcards"] = {"metaData":{"mosaicProviderJobCardsModel":{"results":[
{"displayTitle":"Data Scientist Suptech (H/F)","company":"BANQUE DE FRANCE","formattedLocation":"Paris (75)",
"jobTypes":["Temps plein","CDI"],"formattedRelativeTime":"il y a 7 jours","jobkey":"c5ab9d781e5491af",
"remoteLocation":false,"salarySnippet":{"text":"De 48 000 € à 52 000 € par an"},
"snippet":"<ul><li>Au sein de l'ACPR, equipe agile.</li></ul>"},
{"displayTitle":"Ingénieur IA","company":"Getraline","formattedLocation":"Versailles (78)","jobkey":"e6af385bf70e3405",
"remoteLocation":true,"extractedSalary":{"min":40000,"max":-1,"type":"YEARLY"},"snippet":"<p>Pôle IA.</p>"}]}}};</script>
</body></html>"##;
        let md = render(&Kind::IndeedJobs, html).expect("renders");
        assert!(
            md.contains("> 2 offers on this page"),
            "count must be the real one:\n{md}"
        );
        assert!(md.contains("## Data Scientist Suptech (H/F)"), "{md}");
        assert!(md.contains("BANQUE DE FRANCE · Paris (75) · Temps plein/CDI · De 48 000 € à 52 000 € par an · il y a 7 jours"), "{md}");
        assert!(
            md.contains("equipe agile"),
            "snippet html not rendered:\n{md}"
        );
        // Locale host from the canonical link — fr.indeed.com, not www.
        assert!(
            md.contains("https://fr.indeed.com/viewjob?jk=c5ab9d781e5491af"),
            "{md}"
        );
        // `max: -1` is "no upper bound", not a salary of minus one.
        assert!(md.contains("from 40000") && !md.contains("-1"), "{md}");
        assert!(md.contains("remote"), "{md}");

        // Routing: search pages yes, in both URL shapes and any locale host…
        for u in [
            "https://fr.indeed.com/q-generative-ai-llm-l-paris-(75)-emplois.html?vjk=e6af385",
            "https://www.indeed.com/jobs?q=engineer&l=Paris",
            "https://uk.indeed.com/q-data-scientist-l-london-jobs.html",
        ] {
            assert!(
                matches!(plan(u).map(|p| p.kind), Some(Kind::IndeedJobs)),
                "{u} must route to the job-cards reader"
            );
        }
        // …career-advice articles no: measured at 8.6% chrome, the generic
        // pipeline already wins there.
        assert!(plan(
            "https://www.indeed.com/career-advice/career-development/certifications-in-demand"
        )
        .is_none());
        assert!(plan("https://www.indeed.com/").is_none());
        // No payload (a logged-out wall, a layout change) → fail open.
        assert!(render(&Kind::IndeedJobs, "<html><body>nothing</body></html>").is_none());
    }

    #[test]
    fn article_reader_prefers_the_page_headline_and_published_date() {
        // IGN shape: SEO og:title differing from the real headline, JSON-LD
        // WITHOUT articleBody, dateModified later than datePublished, and a
        // schema.org Product blob inlined in the article container.
        let html = r##"<html><head>
<meta property="og:title" content="From a $1,000 Golden Mega Darkrai to a Cheap Rare" />
<meta property="article:published_time" content="2026-07-20T09:13:43.305Z" />
<script type="application/ld+json">{"@type":"NewsArticle","headline":"Top 10 Most Valuable Pitch Black Cards",
"author":{"@type":"Person","name":"Sara Heritage"},"datePublished":"2026-07-20T09:13:43Z",
"dateModified":"2026-07-20T09:14:29Z"}</script></head><body>
<h1>chrome headline</h1>
<div class="jsx-2870106660 article-content page-0"><p>The expansion is all about Dark-types.</p>
<script type="application/ld+json">{"@type":"Product","name":"Mega Darkrai ex","offers":{"price":996.69}}</script>
<p>Not everyone is a fan of gold cards, but the pull is a lottery win all the same.</p></div>
<section class="recommends">Recommends — In This Article — Where to Watch</section>
</body></html>"##;
        let md = render(&Kind::Article(&IGN), html).expect("renders");
        assert!(
            md.starts_with("# Top 10 Most Valuable Pitch Black Cards"),
            "{md}"
        );
        assert!(
            !md.contains("From a $1,000 Golden"),
            "SEO og:title used as title:\n{md}"
        );
        assert!(md.contains("Sara Heritage • 2026-07-20"), "{md}");
        assert!(
            !md.contains("09:14:29"),
            "dateModified served as the date:\n{md}"
        );
        assert!(
            md.contains("Dark-types") && md.contains("lottery win"),
            "{md}"
        );
        // The inlined JSON-LD Product blob must not survive as prose — without
        // the script strip it read as 30 raw JSON objects on a live page.
        assert!(
            !md.contains("996.69") && !md.contains("@type"),
            "script leaked:\n{md}"
        );
        assert!(!md.contains("Where to Watch"), "tail chrome leaked:\n{md}");
    }

    #[test]
    fn britannica_fences_the_article_and_declares_a_partial_one() {
        // Quizzes/FAQ/related sit OUTSIDE the comment fence; `wordcount` is the
        // whole split article, of which this page holds a slice.
        let html = r##"<html><head><script type="application/ld+json">
{"@type":"Article","headline":"John F. Kennedy | Biography, Family, Presidency, & Facts",
"author":[{"@type":"Person","name":"William Manchester"}],"datePublished":"1999-07-28T00:00:00Z",
"dateModified":"2026-07-17T00:00:00Z","wordcount":3478}</script></head><body>
<h1 class="tw:inline">John F. Kennedy</h1>
<div class="faq">Top Questions — who shot him?</div>
<!--[BEFORE-ARTICLE]--><span class="marker before-article"></span>
<section data-level="1" id="ref1"><p class="topic-paragraph">Kennedy was the 35th president of the United States.</p></section>
<section data-level="2" id="ref3866"><p class="topic-paragraph">He was born in Brookline, Massachusetts.</p></section>
<!--[AFTER-ARTICLE]--><span class="marker after-article"></span>
<div class="quizzes">Quizzes — American History Quiz</div>
</body></html>"##;
        let md = render(&Kind::Article(&BRITANNICA), html).expect("renders");
        // The page's own h1, not the keyword-stuffed JSON-LD headline.
        assert!(md.starts_with("# John F. Kennedy\n"), "{md}");
        assert!(
            !md.contains("Biography, Family, Presidency"),
            "SEO headline used:\n{md}"
        );
        assert!(
            md.contains("35th president") && md.contains("Brookline"),
            "{md}"
        );
        assert!(
            !md.contains("Top Questions") && !md.contains("American History Quiz"),
            "content outside the fence leaked:\n{md}"
        );
        assert!(
            md.contains("[partial article") && md.contains("3478 words"),
            "split article not declared:\n{md}"
        );
        // A reference work revises in place: the 2026 revision is the useful
        // date, not the 1999 first publication.
        assert!(md.contains("2026-07-17") && !md.contains("1999"), "{md}");
    }

    #[test]
    fn cbsnews_uses_article_body_and_academia_says_it_is_only_an_abstract() {
        let cbs = r##"<html><head><script type="application/ld+json">
{"@type":"NewsArticle","headline":"Can you spot the AI image?","datePublished":"2026-07-17T09:54:00-0400",
"dateModified":"2026-07-17T21:06:00-0400","author":[{"@type":"Person","name":"Kerry Breen"}],
"articleBody":"A new quiz asks readers to tell real photographs from generated ones. The tool was built by a verification startup, and most people score worse than they expect on it."}
</script></head><body><section class="content__body"><p>ignored, the structured body wins</p>
<section class="content__body--footer">The Free Press promo</section></section></body></html>"##;
        let md = render(&Kind::Article(&CBSNEWS), cbs).expect("renders");
        assert!(md.starts_with("# Can you spot the AI image?"), "{md}");
        assert!(md.contains("Kerry Breen • 2026-07-17"), "{md}");
        assert!(md.contains("verification startup"), "{md}");
        assert!(!md.contains("Free Press"), "footer promo leaked:\n{md}");

        let acad = r##"<html><head><script type="application/ld+json">
{"@type":"ScholarlyArticle","headline":"A Survey on Machine Learning Techniques",
"author":{"@type":"Person","name":"IJRASET Publication"},"datePublished":"2021-01-01",
"abstract":"Machine learning has developed since the 1990s and continues to expand as the volume of records and documents grows, which makes analysis and processing harder every year."}
</script></head><body><div>Related papers — References — FAQs — Cited by</div></body></html>"##;
        let md = render(&Kind::Article(&ACADEMIA), acad).expect("renders");
        assert!(md.contains("since the 1990s"), "{md}");
        assert!(!md.contains("Related papers"), "chrome leaked:\n{md}");
        // The paper itself is a PDF behind a wall — never let this read as the
        // full text.
        assert!(
            md.contains("abstract only"),
            "missing partiality note:\n{md}"
        );
    }

    #[test]
    fn a_registry_timestamp_is_cut_on_a_char_boundary() {
        // Both dates here put a multibyte character across byte 10, which is
        // where the ten-character date is cut. `&d[..10]` PANICS on that —
        // measured: "end byte index 10 is not a char boundary; it is inside
        // 'é' (bytes 9..11 of string)". A panic in a reader loses the whole
        // response, so the cut has to find a boundary rather than assume one.
        let body = r#"{"crate":{"name":"x","updated_at":"2026-09-0é7T00:00:00Z"},
"versions":[{"num":"1.0.0","created_at":"2026-09-0é7T00:00:00Z","yanked":false}]}"#;
        let md = crates_api(body).expect("must render, not panic");
        assert!(md.starts_with("# x (crates.io)"), "{md}");
        assert!(md.contains("updated: 2026-09-0"), "date lost: {md}");

        // Anti-vacuity: an ordinary ASCII date is still cut to exactly the
        // ten characters of the calendar day, so "keep the whole string" and
        // "keep nothing" both fail here.
        let ok = r#"{"crate":{"name":"y","updated_at":"2026-09-07T16:51:35Z"},
"versions":[{"num":"2.0.0","created_at":"2026-09-05T01:02:03Z","yanked":false}]}"#;
        let md = crates_api(ok).unwrap();
        assert!(md.contains("updated: 2026-09-07"), "{md}");
        assert!(
            !md.contains("2026-09-07T"),
            "the time survived the cut: {md}"
        );
        assert!(md.contains("(2026-09-05)"), "version date lost: {md}");
        assert!(!md.contains("2026-09-05T"), "version time survived: {md}");
    }

    #[test]
    fn crates_api_renders_description_links_versions() {
        let body = r#"{"crate":{"name":"serde","description":"A generic serialization framework",
"max_stable_version":"1.0.228","newest_version":"1.0.228","downloads":1149658129,
"recent_downloads":230714873,"updated_at":"2025-09-27T16:51:35Z",
"repository":"https://github.com/serde-rs/serde","documentation":"https://docs.rs/serde","homepage":null},
"keywords":[{"id":"serde"},{"id":"no_std"}],"categories":[{"id":"encoding"}],
"versions":[{"num":"1.0.228","created_at":"2025-09-27T16:51:35Z","yanked":false},
{"num":"1.0.227","created_at":"2025-09-25T01:02:03Z","yanked":true}]}"#;
        let md = crates_api(body).unwrap();
        assert!(md.starts_with("# serde (crates.io)"), "{md}");
        assert!(md.contains("A generic serialization framework"), "{md}");
        assert!(md.contains("latest stable: 1.0.228"), "{md}");
        assert!(
            md.contains("Repository: https://github.com/serde-rs/serde"),
            "{md}"
        );
        assert!(md.contains("Keywords: serde, no_std"), "{md}");
        assert!(md.contains("1.0.227 (2025-09-25) [yanked]"), "{md}");
    }

    #[test]
    fn plan_maps_hn_item_to_algolia() {
        let p = plan("https://news.ycombinator.com/item?id=8863").unwrap();
        assert_eq!(p.fetch_url, "https://hn.algolia.com/api/v1/items/8863");
        assert!(matches!(p.kind, Kind::HnItem));
        assert!(plan("https://news.ycombinator.com/newest").is_none());
    }

    #[test]
    fn plan_maps_discourse_topic_shape_to_json() {
        let p = plan("https://users.rust-lang.org/t/some-topic/12345/7?u=x").unwrap();
        assert_eq!(
            p.fetch_url,
            "https://users.rust-lang.org/t/some-topic/12345.json"
        );
        assert!(matches!(p.kind, Kind::DiscourseTopic));
        assert!(plan("https://users.rust-lang.org/latest").is_none());
    }

    #[test]
    fn hn_item_renders_story_and_nested_comments() {
        let body = r#"{"title":"My story","points":104,"author":"dhouston",
"created_at":"2007-04-05T00:00:00Z","url":"https://example.com/x",
"text":"<p>Story text here.</p>","children":[
 {"author":"alice","text":"<p>Top comment.</p>","children":[
   {"author":"bob","text":"<p>Nested reply.</p>","children":[]}]},
 {"author":"carol","text":"<p>Second thread.</p>","children":[]}]}"#;
        let md = hn_item(body).unwrap();
        assert!(md.starts_with("# My story"), "{md}");
        assert!(md.contains("104 pts • by dhouston • 2007-04-05"), "{md}");
        assert!(md.contains("- **alice**: Top comment."), "{md}");
        assert!(md.contains("  - **bob**: Nested reply."), "{md}");
        assert!(md.contains("- **carol**: Second thread."), "{md}");
    }

    #[test]
    fn discourse_topic_renders_op_and_replies() {
        let body = r#"{"title":"Welcome to the forum","posts_count":2,"post_stream":{"posts":[
 {"username":"notriddle","cooked":"<p>This forum is for help and discussion.</p>"},
 {"username":"alice","cooked":"<p>A helpful reply with <code>code</code>.</p>"}]}}"#;
        let md = discourse_topic(body).unwrap();
        assert!(md.starts_with("# Welcome to the forum"), "{md}");
        assert!(
            md.contains("This forum is for help and discussion."),
            "{md}"
        );
        assert!(
            md.contains("**alice**: A helpful reply with `code`."),
            "{md}"
        );
    }

    #[test]
    fn plan_maps_youtube_urls_to_watch() {
        for u in [
            "https://www.youtube.com/watch?v=aircAruvnKk",
            "https://youtu.be/aircAruvnKk",
            "https://m.youtube.com/watch?v=aircAruvnKk&t=30s",
            "https://www.youtube.com/shorts/aircAruvnKk",
        ] {
            let p = plan(u).expect(u);
            assert_eq!(p.fetch_url, "https://www.youtube.com/watch?v=aircAruvnKk");
            assert!(matches!(p.kind, Kind::YouTube));
        }
        assert!(plan("https://www.youtube.com/feed/subscriptions").is_none());
    }

    #[test]
    fn plan_maps_x_status_to_syndication() {
        for u in [
            "https://x.com/jack/status/20",
            "https://twitter.com/jack/status/20?s=21",
            "https://mobile.twitter.com/jack/statuses/20",
        ] {
            let p = plan(u).expect(u);
            assert_eq!(
                p.fetch_url,
                "https://cdn.syndication.twimg.com/tweet-result?id=20&token=a&lang=en"
            );
            assert!(matches!(p.kind, Kind::XPost));
        }
        assert!(plan("https://x.com/jack").is_none());
    }

    #[test]
    fn extract_js_object_handles_nested_braces_and_strings() {
        let html = r#"<script>var ytInitialPlayerResponse = {"a":{"b":"}not end"},"c":1}; more"#;
        let obj = extract_js_object(html, "ytInitialPlayerResponse").unwrap();
        assert_eq!(obj, r#"{"a":{"b":"}not end"},"c":1}"#);
    }

    #[test]
    fn youtube_watch_renders_metadata_and_description() {
        let html = r#"<html><script>ytInitialPlayerResponse = {"videoDetails":{
"title":"But what is a neural network?","author":"3Blue1Brown","viewCount":"23640414",
"lengthSeconds":"1120","shortDescription":"What are the neurons, why are there layers?"},
"captions":{"x":1},"microformat":{"playerMicroformatRenderer":{"publishDate":"2017-10-05"}}};</script></html>"#;
        let md = youtube_watch(html).unwrap();
        assert!(md.starts_with("# But what is a neural network?"), "{md}");
        assert!(md.contains("3Blue1Brown • 23640414 views • 18m40s"), "{md}");
        assert!(md.contains("published 2017-10-05"), "{md}");
        assert!(md.contains("What are the neurons"), "{md}");
        assert!(md.contains("transcript needs the YouTube player"), "{md}");
    }

    #[test]
    fn youtube_video_id_parses_all_url_shapes() {
        assert_eq!(
            youtube_video_id("https://www.youtube.com/watch?v=aircAruvnKk&t=5s").as_deref(),
            Some("aircAruvnKk")
        );
        assert_eq!(
            youtube_video_id("https://youtu.be/aircAruvnKk").as_deref(),
            Some("aircAruvnKk")
        );
        assert_eq!(
            youtube_video_id("https://www.youtube.com/shorts/aircAruvnKk").as_deref(),
            Some("aircAruvnKk")
        );
    }

    #[test]
    fn pick_en_caption_prefers_manual_and_absolutizes_url() {
        let list = r#"{"captions":[
{"label":"English (auto-generated)","languageCode":"en","url":"/api/v1/captions/x?label=auto"},
{"label":"English","languageCode":"en","url":"/api/v1/captions/x?label=English"},
{"label":"Français","languageCode":"fr","url":"/api/v1/captions/x?label=fr"}]}"#;
        assert_eq!(
            pick_en_caption_url(list, "https://inv.example/").as_deref(),
            Some("https://inv.example/api/v1/captions/x?label=English")
        );
        // No English track → None.
        let fr_only = r#"{"captions":[{"label":"Français","languageCode":"fr","url":"/x"}]}"#;
        assert!(pick_en_caption_url(fr_only, "https://inv.example").is_none());
    }

    #[test]
    fn pick_en_piped_caption_prefers_manual() {
        let streams = r#"{"subtitles":[
{"url":"https://p/cap?code=en&auto=1","code":"en","autoGenerated":true},
{"url":"https://p/cap?code=en","code":"en","autoGenerated":false},
{"url":"https://p/cap?code=fr","code":"fr","autoGenerated":false}]}"#;
        assert_eq!(
            pick_en_piped_caption_url(streams).as_deref(),
            Some("https://p/cap?code=en")
        );
        assert!(pick_en_piped_caption_url(r#"{"subtitles":[]}"#).is_none());
    }

    #[test]
    fn timedtext_xml_parses_and_dedupes() {
        let xml = "<?xml version=\"1.0\"?><transcript>\
<text start=\"0\" dur=\"2\">Hello world</text>\
<text start=\"2\" dur=\"2\">Hello world</text>\
<text start=\"4\" dur=\"2\">second &amp; last</text></transcript>";
        assert_eq!(
            timedtext_xml_to_transcript(xml).as_deref(),
            Some("Hello world second & last")
        );
        assert!(timedtext_xml_to_transcript("<html>no captions</html>").is_none());
    }

    #[test]
    fn vtt_to_transcript_cleans_timings_tags_and_rolling_dupes() {
        let vtt = "WEBVTT\nKind: captions\nLanguage: en\n\n\
1\n00:00:00.000 --> 00:00:02.000\nHello <c>there</c> world\n\n\
2\n00:00:02.000 --> 00:00:04.000\nHello there world\n\n\
3\n00:00:04.000 --> 00:00:06.000\nsecond &amp; final line\n";
        let t = vtt_to_transcript(vtt).unwrap();
        assert_eq!(t, "Hello there world second & final line");
        // Non-VTT (anti-bot HTML) → None.
        assert!(vtt_to_transcript("<!DOCTYPE html><html>blocked</html>").is_none());
    }

    #[test]
    fn x_post_renders_text_author_and_stats() {
        let body = r#"{"text":"just setting up my twttr","favorite_count":308348,
"conversation_count":1200,"created_at":"2006-03-21T20:50:14.000Z",
"user":{"name":"jack","screen_name":"jack"}}"#;
        let md = x_post(body).unwrap();
        assert!(md.starts_with("# Post by jack (@jack)"), "{md}");
        assert!(
            md.contains("308348 likes • 1200 replies • 2006-03-21"),
            "{md}"
        );
        assert!(md.contains("just setting up my twttr"), "{md}");
        assert!(md.contains("replies are not available"), "{md}");
    }

    #[test]
    fn plan_maps_bluesky_lemmy_mastodon() {
        let b = plan("https://bsky.app/profile/bsky.app/post/3mqcp5qjdfs26").unwrap();
        assert!(b
            .fetch_url
            .contains("getPostThread?uri=at://bsky.app/app.bsky.feed.post/3mqcp5qjdfs26"));
        assert!(matches!(b.kind, Kind::Bluesky));
        let l = plan("https://lemmy.world/post/49253586").unwrap();
        assert_eq!(l.fetch_url, "https://lemmy.world/api/v3/post?id=49253586");
        assert!(l
            .aux_url
            .as_deref()
            .unwrap()
            .contains("comment/list?post_id=49253586"));
        let m =
            plan("https://fosstodon.org/@macrumors@mastodon.social/116898059312518260").unwrap();
        assert_eq!(
            m.fetch_url,
            "https://fosstodon.org/api/v1/statuses/116898059312518260"
        );
        assert!(m.aux_url.as_deref().unwrap().ends_with("/context"));
        // Shape guards: a normal /@handle profile (no id) is not a Mastodon post.
        assert!(plan("https://fosstodon.org/@macrumors").is_none());
    }

    #[test]
    fn mastodon_renders_post_and_replies() {
        let status = r#"{"content":"<p>Hello <b>fediverse</b></p>","favourites_count":12,
"reblogs_count":3,"created_at":"2026-01-02T10:00:00Z",
"account":{"acct":"alice@fosstodon.org","display_name":"Alice"}}"#;
        let ctx = r#"{"ancestors":[],"descendants":[
{"content":"<p>nice post</p>","account":{"acct":"bob"}},
{"content":"<p>agreed</p>","account":{"acct":"carol"}}]}"#;
        let body = format!("{status}{AUX_SEP}{ctx}");
        let md = mastodon_status(&body).unwrap();
        assert!(
            md.starts_with("# Post by Alice (@alice@fosstodon.org)"),
            "{md}"
        );
        assert!(md.contains("12 favs • 3 boosts • 2026-01-02"), "{md}");
        assert!(md.contains("Hello fediverse"), "{md}");
        assert!(md.contains("**@bob**: nice post"), "{md}");
    }

    #[test]
    fn bluesky_renders_post_and_replies() {
        let body = r#"{"thread":{"post":{
"author":{"handle":"bsky.app","displayName":"Bluesky"},
"record":{"text":"Personnel news: big update","createdAt":"2026-01-03T00:00:00Z"},
"likeCount":956,"replyCount":49},
"replies":[{"post":{"author":{"handle":"a.bsky.social"},"record":{"text":"congrats!"}}},
{"post":{"author":{"handle":"b.bsky.social"},"record":{"text":"great news"}}}]}}"#;
        let md = bluesky_thread(body).unwrap();
        assert!(md.starts_with("# Post by Bluesky (@bsky.app)"), "{md}");
        assert!(md.contains("956 likes • 49 replies • 2026-01-03"), "{md}");
        assert!(md.contains("Personnel news"), "{md}");
        assert!(md.contains("**@a.bsky.social**: congrats!"), "{md}");
    }

    #[test]
    fn lemmy_renders_post_and_comments() {
        let post = r#"{"post_view":{"post":{"name":"Audiobooks keep me sane","body":"On my commute."},
"creator":{"name":"commuter"},"community":{"name":"books"},
"counts":{"score":1201,"comments":233}}}"#;
        let comments = r#"{"comments":[
{"comment":{"content":"Any recommendations?"},"creator":{"name":"reader"},"counts":{"score":40}},
{"comment":{"content":"Try Project Hail Mary"},"creator":{"name":"fan"},"counts":{"score":88}}]}"#;
        let body = format!("{post}{AUX_SEP}{comments}");
        let md = lemmy_post(&body).unwrap();
        assert!(md.starts_with("# Audiobooks keep me sane"), "{md}");
        assert!(
            md.contains("by commuter • c/books • 1201 pts • 233 comments"),
            "{md}"
        );
        assert!(md.contains("On my commute."), "{md}");
        assert!(
            md.contains("**fan** (88 pts): Try Project Hail Mary"),
            "{md}"
        );
    }

    #[test]
    fn render_rejects_thin_output() {
        // A bot-wall page with no parseable thread → None → generic fallback.
        assert!(render(&Kind::RedditThread, "<html><body>blocked</body></html>").is_none());
        assert!(render(&Kind::CratesApi, "{}").is_none());
        assert!(render(&Kind::CratesApi, "not json").is_none());
    }

    // ---- LIVE validation harness (network!) ------------------------------
    // Run manually: `cargo test -p tokenade-core site_readers -- --ignored`
    // Ignored by default so CI stays hermetic. Each test fetches the plan's
    // alternate representation LIVE and asserts a content marker, catching
    // layout/API drift the synthetic fixtures can't.

    fn live_fetch(url: &str) -> String {
        crate::net::http_get(
            url,
            &[
                (
                    "User-Agent",
                    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                     (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
                ),
                // Same locale/TLD derivation the product path uses, so the live
                // harness exercises the request shape we actually ship.
                (
                    "Accept-Language",
                    crate::web_fetch::accept_language_for(url),
                ),
            ],
            20,
        )
        .map(|r| r.text())
        .unwrap_or_default()
    }

    fn live_check(url: &str, marker: &str) {
        let p = plan(url).expect("plan");
        let body = live_fetch(&p.fetch_url);
        assert!(!body.is_empty(), "live fetch failed for {}", p.fetch_url);
        let md = render(&p.kind, &body).expect("render");
        assert!(
            md.contains(marker),
            "marker {marker:?} missing from live render of {url} (drift?):\n{}",
            md.chars().take(600).collect::<String>()
        );
    }

    #[test]
    #[ignore = "live network — run with --ignored"]
    fn live_crates_io_reader() {
        live_check("https://crates.io/crates/serde", "serialization");
    }

    #[test]
    #[ignore = "live network — run with --ignored"]
    fn live_hn_reader() {
        live_check("https://news.ycombinator.com/item?id=8863", "Dropbox");
    }

    #[test]
    #[ignore = "live network — run with --ignored"]
    fn live_discourse_reader() {
        live_check(
            "https://users.rust-lang.org/t/welcome-to-the-rust-programming-language-users-forum/77411",
            "help, discussion, and announcements",
        );
    }

    #[test]
    #[ignore = "live network — run with --ignored"]
    fn live_youtube_reader() {
        live_check("https://www.youtube.com/watch?v=aircAruvnKk", "3Blue1Brown");
    }

    #[test]
    #[ignore = "live network — run with --ignored"]
    fn live_x_reader() {
        live_check("https://x.com/jack/status/20", "just setting up my twttr");
    }

    #[test]
    #[ignore = "live network — run with --ignored"]
    fn live_bluesky_reader() {
        // Evergreen: the Bluesky team account's pinned-ish post id may rotate;
        // assert structure, not content.
        let p = plan("https://bsky.app/profile/bsky.app/post/3mqcp5qjdfs26").expect("plan");
        let body = live_fetch(&p.fetch_url);
        if body.contains("NotFound") {
            return; // post aged out — skip rather than false-fail
        }
        let md = render(&p.kind, &body).expect("render");
        assert!(
            md.contains("@bsky.app"),
            "no author:\n{}",
            md.chars().take(300).collect::<String>()
        );
    }

    #[test]
    #[ignore = "live network — run with --ignored"]
    fn live_lemmy_reader() {
        // Fetch a current post id from the instance, then read it.
        let list = live_fetch("https://lemmy.world/api/v3/post/list?limit=1&sort=Active");
        let id = serde_json::from_str::<serde_json::Value>(&list)
            .ok()
            .and_then(|v| v.pointer("/posts/0/post/id").and_then(|i| i.as_i64()))
            .expect("post id");
        let p = plan(&format!("https://lemmy.world/post/{id}")).expect("plan");
        let body = live_fetch(&p.fetch_url);
        let aux = p.aux_url.as_deref().map(live_fetch).unwrap_or_default();
        let combined = format!("{body}{AUX_SEP}{aux}");
        let md = render(&p.kind, &combined).expect("render");
        assert!(
            md.starts_with("# "),
            "no title:\n{}",
            md.chars().take(200).collect::<String>()
        );
    }

    #[test]
    #[ignore = "live network — run with --ignored"]
    fn live_reddit_reader() {
        // The LISTING, not a thread: it is the shape whose feed carries the
        // most, and the marker is structural. A thread only renders its
        // submission since 2026-09-11 — reddit serves its comments to nobody
        // without an account — so the former "## Comments" assertion could
        // only fail, on a perfectly read feed.
        let p = plan("https://www.reddit.com/r/rust/").expect("plan");
        assert!(p.fetch_url.ends_with("/.rss"), "{}", p.fetch_url);
        let body = live_fetch(&p.fetch_url);
        assert!(!body.is_empty(), "live fetch failed for {}", p.fetch_url);
        let md = render(&p.kind, &body).expect("render");
        assert!(
            md.contains("## "),
            "no entries:\n{}",
            md.chars().take(600).collect::<String>()
        );
        assert!(
            md.contains("reddit.com/r/rust/comments/"),
            "no submission links"
        );
    }
}
