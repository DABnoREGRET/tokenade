//! **The page has no body: go and fetch the prose elsewhere.**
//!
//! A single-page application shell carries, in HTML, only an empty
//! `<div id=root>`: all the text is in a JSON payload, a `<noscript>`, or the
//! metadata. Rendering the shell would return NOTHING, and without saying so.
//! This module recognises the shape and recovers the material the browser
//! would have painted.

use super::*;

/// Inner content of every `<noscript>` block, concatenated. Discourse (and
/// other crawler-friendly JS apps) serve the full server-rendered content
/// there — the T3 rescue tier converts it when the DOM tiers came up empty.
pub(super) fn harvest_noscript(html: &str) -> String {
    let r = re(r"(?is)<noscript\b[^>]*>(.*?)</noscript>");
    let mut out = String::new();
    for c in r.captures_iter(html) {
        let inner = c.get(1).map(|m| m.as_str()).unwrap_or_default();
        // "Please enable JavaScript" one-liners aren't content.
        if inner.len() > 200 {
            out.push_str(inner);
            out.push('\n');
        }
    }
    out
}

/// Does this page look like an EMPTY single-page-app shell rather than a
/// document?
///
/// Measured over a 70-page corpus (45 judgeable after dropping 4xx and
/// bot-challenge responses: 15 shells, 30 documents), two intuitive signals
/// were tested and BOTH lost:
///
///   * TEXT YIELD is worse than useless — it is inverted. bbc.com/news, a real
///     newsroom, yields 2.72 % while Notion's empty shell yields 3.50 %:
///     content-heavy sites legitimately ship tens of KB of app scaffolding, so
///     any ratio catching the shell also condemns the newspaper.
///   * PROSE-WORD COUNT (words in sentence-length lines) fails on LISTING
///     pages: lobste.rs and asahi.com distil to 9.5 KB and 36 KB of real
///     content carried by headlines and links, not sentences — 10 and 16
///     "prose words". Legitimate, readable, and indistinguishable from a shell
///     by that measure.
///
/// What survives is the crudest signal: the ABSOLUTE volume of distilled text.
/// A shell has nothing to distil whatever its shape. On the corpus the classes
/// do not overlap — worst shell 208 tokens (discord), worst document 601
/// (a GitHub issue) — so the floor sits in the gap with margin on both sides.
///
/// The size guard keeps a legitimately tiny page (example.com, a bot-blocked
/// stub) out: it distils to little because it IS little, not because a
/// framework swallowed it.
///
/// This is a diagnostic, not a routing decision: firing it does NOT mean we
/// can serve the page better. Without a dedicated reader for the domain we
/// have nothing to offer either, and intercepting would be strictly worse
/// than letting the native fetcher try. It exists to name the domains that
/// deserve a reader next.
///
/// KNOWN MISS: a shell that ships unrelated prose — miro.com serves 655 words
/// of product and cookie copy around an empty board. Separating "has text"
/// from "has the text you asked for" needs a different signal (e.g. the Notion
/// tell: `og:url` pointing at the site root from a deep page), not a lower
/// floor, which would start condemning short real pages.
///
/// essence (MIT — see module doc) flags `__NEXT_DATA__` / `__NUXT__` /
/// `data-server-rendered` presence as shell evidence, which is backwards —
/// those mean the content was ALREADY server-rendered. Audited: nothing in
/// this file has ever done that (`harvest_embedded_prose` MINES
/// `__NEXT_DATA__` as a rescue source, never reads it as a negative signal),
/// so there is no such check to remove here. essence's proposed replacement
/// signal — an absolute "body text < 100 chars" floor, with no size
/// condition on the input — is also NOT adopted: it is the same naive idea
/// this function's OWN corpus test already falsified above ("a legitimately
/// tiny page... distils to little because it IS little"); dropping the
/// `html_tokens` size guard to add it would reopen exactly that false
/// positive. `looks_like_spa_shell` also has no raw HTML to inspect for
/// `#root`/`#app`/`#__next` even if the signal were adopted — only the
/// converted markdown and two token counts reach it, and its signature is
/// public API for `web_pipeline.rs` (out of scope here) and stays frozen.
pub fn looks_like_spa_shell(markdown: &str, html_tokens: usize, md_tokens: usize) -> bool {
    const JS_BANNERS: [&str; 4] = [
        "javascript must be enabled",
        "please enable javascript",
        "enable javascript to continue",
        "you need to enable javascript to run this app",
    ];
    // Banners are decisive on their own — they ARE the page.
    let lc = markdown.to_lowercase();
    if JS_BANNERS.iter().any(|b| lc.contains(b)) {
        return true;
    }
    html_tokens >= 1_000 && md_tokens < 300
}

/// `content=` of a `<meta>` whose name/property equals `key` (both attribute
/// orders), entity-decoded.
pub fn meta_content(html: &str, key: &str) -> Option<String> {
    let k = regex::escape(key);
    for pat in [
        format!(
            r#"(?is)<meta\b{TAG_ATTRS}?(?:name|property)\s*=\s*["']{k}["']{TAG_ATTRS}?content\s*=\s*{ATTR_VALUE}"#
        ),
        format!(
            r#"(?is)<meta\b{TAG_ATTRS}?content\s*=\s*{ATTR_VALUE}{TAG_ATTRS}?(?:name|property)\s*=\s*["']{k}["']"#
        ),
    ] {
        if let Some(c) = re(&pat).captures(html) {
            // `continue`, not `?`: a match whose value is missing must fall
            // through to the OTHER attribute order, not abandon the lookup.
            let Some(raw) = quoted_value(&c, 1) else {
                continue;
            };
            let v = decode_entities(raw).trim().to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

/// Prose mined from embedded page state: JSON-LD blocks (schema.org fields)
/// first, then `__NEXT_DATA__` string values that look like copy. Capped —
/// this is a rescue tier for thin pages, not a firehose.
pub(super) fn harvest_embedded_prose(html: &str, body: &str) -> String {
    const CAP_CHARS: usize = 4_000;
    // **A CMS body is not a rescue scrap.**
    //
    // The cap above says "this is a rescue tier for thin pages, not a
    // firehose", and that is true of a `description` or a `headline`. When
    // `text`/`articleBody` carries MARKUP, it is not a scrap: it is the
    // article, stored by the CMS in its structured data. MEASURED on the 39
    // pages: only one is in this case, `nhs_leaf`, and it puts **9,117
    // characters** there — symptoms, risk factors, treatments, complications
    // — for 1,843 characters delivered. Cutting them at 4,000 would return
    // half a health page. What bounds a page is the DELIVERY cap downstream,
    // not a tier constant; this one only guards against a pathological blob.
    const CAP_CMS_BODY_CHARS: usize = 40_000;
    // The rescue tier only opened on an almost empty body (400 characters). A
    // page whose markup carries the standfirst and whose article lives in
    // `hasPart[].text` goes above that threshold and so NEVER received its
    // article. The CMS body now has its own gate, independent of how thin the
    // rendered body is.
    let thin = body.chars().count() < EMBED_RESCUE_MAX_CHARS;
    let mut seen = std::collections::HashSet::new();
    let mut out = String::new();
    // **A rescued article's title is a HEADING.**
    //
    // When the rendered body is a JS shell, the readable text is still there
    // in the page's `application/ld+json`, and that is what this pass pulls
    // out. Everything arrived as paragraphs: `headline` — the article's title
    // — could no longer be told apart from the body, and a page rescued this
    // way left WITHOUT a single heading. A large page's outline is built on
    // headings; it therefore had no section at all, and a reader could not
    // tell where the article begins.
    //
    // `headline` only, and only once: `name` is carried by every entity of a
    // page (the publisher, the logo, each breadcrumb) and turning them into
    // headings would invent a dozen. A second `headline` — a page listing
    // several articles — stays a paragraph: nothing here says which one is
    // the page's subject, and two level-1 headings would claim two.
    let mut headed = false;
    let mut push = |key: &str, s: &str, out: &mut String| {
        // A value that CARRIES markup is converted like HTML, not stripped.
        // `strip_all_tags` removes the tag and puts nothing in its place:
        // `<li>feeling very tired</li><li>peeing more than usual</li>` became
        // `feeling very tiredpeeing more than usual`, a made-up word at every
        // boundary, and the list lost its shape. It is the same case as the
        // content of a `<noscript>`, which is HTML inside a string and already
        // goes through the converter.
        let converted;
        let clean = if carries_markup(s) {
            converted = convert_body(s, false);
            converted.trim()
        } else {
            converted = decode_entities(&strip_all_tags(s));
            converted.trim()
        };
        if clean.chars().count() >= 25 && seen.insert(clean.to_string()) {
            // The FIRST `headline`, wherever it is — not "the one that opens
            // the output". A page blob commonly lists a `WebSite` or an
            // `Organization` BEFORE the article, and requiring first place
            // then rendered the page without any heading.
            if key == "headline" && !headed {
                headed = true;
                out.push_str("# ");
            }
            out.push_str(clean);
            out.push_str("\n\n");
        }
    };
    // JSON-LD: name/headline/description/articleBody/text at any depth.
    let ld =
        re(r#"(?is)<script\b[^>]*type\s*=\s*["']application/ld\+json["'][^>]*>(.*?)</script>"#);
    for c in ld.captures_iter(html) {
        if out.chars().count() > CAP_CHARS {
            break;
        }
        for (key, s) in parse_and_mine(c.get(1).unwrap().as_str().to_string(), mine_jsonld) {
            // On a page that is NOT thin, only the CMS body is lifted: its
            // `description` and `headline` repeat what the markup already
            // shows, and adding them would double the page.
            if !thin && !(matches!(key.as_str(), "text" | "articleBody") && carries_markup(&s)) {
                continue;
            }
            push(&key, &s, &mut out);
        }
    }
    // The page STATE: `__NEXT_DATA__` (Next.js) or Inertia.js's `data-page`
    // attribute (Laravel, Rails: `<div id="app" data-page="{…}">`, JSON
    // entity-escaped). Prose long enough to be text, and lists of links
    // (`mine_page_state`), site-agnostic.
    if thin && out.chars().count() < CAP_CHARS {
        let nd = re(r#"(?is)<script\b[^>]*id\s*=\s*["']__NEXT_DATA__["'][^>]*>(.*?)</script>"#);
        let inertia = re(r#"(?is)<[a-z][^>]*\sdata-page\s*=\s*"([^"]*)""#);
        let state = nd
            .captures(html)
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
            .or_else(|| {
                inertia
                    .captures(html)
                    .and_then(|c| c.get(1))
                    .map(|m| decode_entities(m.as_str()))
            });
        if let Some(state) = state {
            for (key, s) in parse_and_mine(state, mine_page_state) {
                push(&key, &s, &mut out);
            }
        }
    }
    // essence's setext→ATX normalisation (MIT — see module doc), applied
    // HERE specifically: this text is mined out of a JSON field (a CMS body
    // often stores its OWN copy as markdown source), so unlike the rest of
    // this file's output — which is built from `<hN>` tags and never
    // produces setext form — this is the one place literal `Title\n===`
    // source markdown can actually arrive.
    let mut s: String = setext_to_atx(out.trim_end());
    let cap = if thin { CAP_CHARS } else { CAP_CMS_BODY_CHARS };
    if s.chars().count() > cap {
        s = s.chars().take(cap).collect();
    }
    s
}
