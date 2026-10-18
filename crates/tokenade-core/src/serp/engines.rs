//! **One extractor per engine.**
//!
//! Bing, Ecosia, Mojeek, DuckDuckGo, Marginalia, Brave, Yahoo: each has its
//! own markup, its own redirect wrappers and its own way of hiding the
//! destination URL. No selector is shared, and a single parameterised
//! function would pass these differences off as configuration.
//!
//! Unwrapping redirects is the part that breaks silently: an unwrapped
//! wrapper URL presents itself as a valid result, and the agent follows a
//! link that only leads back to the engine.

use super::*;

crate::regexes::lazy_regex!(fn brave_title_re() = r#"class="title search-snippet-title[^"]*"[^>]*\btitle="([^"]+)""#);

/// Extract organic results in document order. Engine-agnostic: we walk every
/// `<a href="…">…</a>` whose anchor text looks like a title (5-180 chars, not
/// a bare URL) and try to attach a snippet from the nearest sibling text.
pub fn extract(html: &str) -> Vec<Result> {
    // Brave first: its server-rendered result blocks are unambiguous, and
    // the generic anchor-walk below drowns them in nav links ("Images",
    // "Videos", "Goggles") and inline-SVG path data. Empty = not Brave →
    // fall through to the generic walk.
    let brave = brave_extract(html);
    if !brave.is_empty() {
        return brave;
    }
    let mut out = Vec::new();
    for cap in result_link_re().captures_iter(html) {
        let href_raw = cap.get(1).map_or("", |m| m.as_str());
        let title_html = cap.get(2).map_or("", |m| m.as_str());
        let title = clean_text(title_html);
        if title.len() < 5 || title.len() > 180 {
            continue;
        }
        if title.starts_with("http://") || title.starts_with("https://") {
            // The anchor text is itself a URL — typical of footer links.
            continue;
        }
        let url = resolve_url(href_raw);
        if !is_useful_url(&url) {
            continue;
        }
        // Snippet: text after the link until the next opening tag that's
        // structural enough to break the result block (h1/h2/h3/li/div with
        // class). Capped so we don't pull in pagination/footer.
        let after_idx = cap.get(0).map_or(0, |m| m.end());
        // `after_idx` is a regex match end → always a char boundary, but the
        // +2048 cap is a raw byte offset that can land mid-codepoint on
        // non-ASCII SERP HTML (international titles, emoji). Clamp to a char
        // boundary so the snippet slice never panics.
        let tail = crate::caps::truncate_at_char_boundary(&html[after_idx..], 2048);
        let snippet = extract_snippet(tail);
        out.push(Result {
            title,
            url,
            snippet,
        });
    }
    out
}

/// Ecosia organic-results extractor. Ecosia server-renders each hit as
/// `<article data-test-id="organic-result">` carrying an
/// `<a data-test-id="result-link" href="…">` (the destination — TWO such
/// anchors per result: title + breadcrumb, same href, deduped by the
/// caller) and a `class="result__description"` snippet. Ecosia is
/// Google-backed (validated 2026-07: `provider="GOOGLE"`), so its result
/// quality matches the mainstream while staying key-free and scrapeable
/// from a plain browser UA. Empty on any other layout.
pub fn ecosia_extract(html: &str) -> Vec<Result> {
    static LINK_RE: OnceLock<Regex> = OnceLock::new();
    static TITLE_RE: OnceLock<Regex> = OnceLock::new();
    let link_re = LINK_RE.get_or_init(|| {
        Regex::new(r#"data-test-id="result-link"[^>]*href="(https?://[^"]+)""#).unwrap()
    });
    // The visible title lives in the result-title anchor's text.
    let title_re = TITLE_RE
        .get_or_init(|| Regex::new(r#"data-test-id="result-title"[^>]*>(.*?)</a>"#).unwrap());
    const BLOCK: &str = "data-test-id=\"organic-result\"";
    let starts: Vec<usize> = html.match_indices(BLOCK).map(|(i, _)| i).collect();
    let mut out = Vec::new();
    for (n, &i) in starts.iter().enumerate() {
        let end = starts.get(n + 1).copied().unwrap_or(html.len());
        let block = crate::caps::truncate_at_char_boundary(&html[i..], end - i);
        let Some(url) = link_re
            .captures(block)
            .and_then(|c| c.get(1))
            .map(|m| decode_entities(m.as_str()))
        else {
            continue;
        };
        let title = title_re
            .captures(block)
            .and_then(|c| c.get(1))
            .map(|m| clean_text(m.as_str()))
            .unwrap_or_default();
        if title.len() < 3 || title.len() > 300 {
            continue;
        }
        let snippet = match block.find("result__description") {
            Some(d) => extract_snippet(crate::caps::truncate_at_char_boundary(&block[d..], 2048)),
            None => String::new(),
        };
        out.push(Result {
            title,
            url,
            snippet,
        });
    }
    out
}

/// Mojeek organic-results extractor. Mojeek is one of the few large engines
/// with its OWN crawler (~10B pages) — independent index, so it surfaces
/// long-tail results Google/Bing-backed engines miss. Each hit is
/// `<li>…<a class="title" href="…">TITLE</a>…<p class="s">SNIPPET</p>`.
/// Coverage is thinner than Bing and heavy-operator queries can 403, so it
/// earns its place by DIVERSIFICATION, not volume. Empty on 403/other.
pub fn mojeek_extract(html: &str) -> Vec<Result> {
    static A_RE: OnceLock<Regex> = OnceLock::new();
    let a_re = A_RE.get_or_init(|| {
        Regex::new(
            r#"<a\b[^>]*class="[^"]*\btitle\b[^"]*"[^>]*href="(https?://[^"]+)"[^>]*>(.*?)</a>"#,
        )
        .unwrap()
    });
    let mut out = Vec::new();
    for cap in a_re.captures_iter(html) {
        let url = decode_entities(cap.get(1).map_or("", |m| m.as_str()));
        let title = clean_text(cap.get(2).map_or("", |m| m.as_str()));
        if title.len() < 3 || title.len() > 300 {
            continue;
        }
        // Snippet: the `<p class="s">` that follows this anchor, bounded to
        // the next result anchor so we never bleed into the neighbour.
        let after = cap.get(0).map_or(0, |m| m.end());
        let tail = crate::caps::truncate_at_char_boundary(&html[after..], 2048);
        let snippet = match tail.find("class=\"s\"").or_else(|| tail.find("class=\"s ")) {
            Some(p) => extract_snippet(&tail[p..]),
            None => String::new(),
        };
        out.push(Result {
            title,
            url,
            snippet,
        });
    }
    out
}

/// DuckDuckGo HTML-endpoint extractor (`html.duckduckgo.com/html/?q=…`).
///
/// MEASURED reachable with our full `BROWSER_HEADERS` set (200 + 11 results);
/// it is the header set, not the handshake, that opens it. Each hit is a
/// `<a class="result__a" href="//duckduckgo.com/l/?uddg=<pct>&rut=…">Title</a>`
/// with a following `<a class="result__snippet">`. DDG returns 200 even on a
/// challenge (a `<form id='challenge-form'>`), so status alone is useless — the
/// challenge markers below catch it.
pub fn ddg_extract(html: &str) -> Vec<Result> {
    static A_RE: OnceLock<Regex> = OnceLock::new();
    static SNIP_RE: OnceLock<Regex> = OnceLock::new();
    let a_re = A_RE.get_or_init(|| {
        Regex::new(r#"(?is)<a\b[^>]*class="[^"]*result__a[^"]*"[^>]*href="([^"]+)"[^>]*>(.*?)</a>"#)
            .unwrap()
    });
    let snip_re =
        SNIP_RE.get_or_init(|| Regex::new(r#"(?is)result__snippet[^>]*>(.*?)</a>"#).unwrap());
    let mut out = Vec::new();
    for cap in a_re.captures_iter(html) {
        let raw = decode_entities(cap.get(1).map_or("", |m| m.as_str()));
        let url = ddg_unwrap(&raw);
        if !url.starts_with("http") {
            continue;
        }
        let title = clean_text(cap.get(2).map_or("", |m| m.as_str()));
        if title.len() < 3 || title.len() > 300 {
            continue;
        }
        let after = cap.get(0).map_or(0, |m| m.end());
        let tail = crate::caps::truncate_at_char_boundary(&html[after..], 2048);
        let snippet = snip_re
            .captures(tail)
            .map(|c| clean_text(c.get(1).map_or("", |m| m.as_str())))
            .unwrap_or_default();
        out.push(Result {
            title,
            url,
            snippet: truncate(&snippet, 300),
        });
    }
    out
}

/// Unwrap DuckDuckGo's `//duckduckgo.com/l/?uddg=<pct>&rut=…` redirect to the
/// real target. A bare protocol-relative or absolute external link passes
/// through (some hits are not wrapped).
pub(crate) fn ddg_unwrap(href: &str) -> String {
    if let Some(after) = href.split("uddg=").nth(1) {
        let enc = after.split('&').next().unwrap_or(after);
        let decoded = crate::text_encoding::form_decode(enc);
        if decoded.starts_with("http") {
            return decoded;
        }
    }
    if let Some(rest) = href.strip_prefix("//") {
        return format!("https://{rest}");
    }
    href.to_string()
}

/// Marginalia public-API extractor — `GET /public/search/{q}` returns
/// `{results:[{url,title,description}]}`. Marginalia indexes the
/// non-commercial / small-web long tail (personal sites, docs, forums)
/// that mainstream engines bury, so it is a QUALITY diversifier for
/// research queries. Empty on any non-JSON / error / zero-result body.
/// Marginalia's HTML results.
///
/// Its public JSON API (`api.marginalia.nu/public/search`) now answers 404, so
/// [`marginalia_extract_json`] targets nothing. The HTML at
/// `old-search.marginalia.nu` is alive and unusually well-structured — each hit
/// is a `<section data-ms-rank="N" class="card search-result">` carrying the
/// URL, an `<h2><a class="title">` and a `<p class="description">`.
///
/// Worth the parser: Marginalia is the only index here that crawls the small
/// non-commercial web, so it surfaces pages the mainstream engines structurally
/// cannot. `data-ms-rank` is the engine's OWN relevance score — kept, because a
/// native rank is better information than the order we happened to read them in.
pub fn marginalia_extract(html: &str) -> Vec<Result> {
    const BLOCK: &str = "class=\"card search-result\"";
    static TITLE_RE: OnceLock<Regex> = OnceLock::new();
    static URL_RE: OnceLock<Regex> = OnceLock::new();
    static DESC_RE: OnceLock<Regex> = OnceLock::new();
    let title_re = TITLE_RE.get_or_init(|| {
        Regex::new(r#"(?is)<a[^>]*class="title"[^>]*href="([^"]+)"[^>]*>(.*?)</a>"#).unwrap()
    });
    let url_re = URL_RE
        .get_or_init(|| Regex::new(r#"(?is)<div class="url"><a[^>]*href="([^"]+)""#).unwrap());
    let desc_re = DESC_RE
        .get_or_init(|| Regex::new(r#"(?is)<p[^>]*class="description"[^>]*>(.*?)</p>"#).unwrap());
    let starts: Vec<usize> = html.match_indices(BLOCK).map(|(i, _)| i).collect();
    let mut out = Vec::new();
    for (n, &i) in starts.iter().enumerate() {
        let end = starts.get(n + 1).copied().unwrap_or(html.len());
        let block = &html[i..end];
        // The title anchor carries both the href and the label; the `url` div is
        // the fallback when a hit has no title link.
        let (url, title) = match title_re.captures(block) {
            Some(c) => (
                resolve_url(c.get(1).map_or("", |m| m.as_str())),
                clean_text(c.get(2).map_or("", |m| m.as_str())),
            ),
            None => match url_re.captures(block) {
                Some(c) => {
                    let u = resolve_url(c.get(1).map_or("", |m| m.as_str()));
                    (u.clone(), u)
                }
                None => continue,
            },
        };
        if title.is_empty() || !is_useful_url(&url) {
            continue;
        }
        let snippet = desc_re
            .captures(block)
            .map(|c| clean_text(c.get(1).map_or("", |m| m.as_str())))
            .unwrap_or_default();
        out.push(Result {
            title,
            url,
            snippet: truncate(&snippet, 300),
        });
    }
    out
}

pub fn marginalia_extract_json(body: &str) -> Vec<Result> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return Vec::new();
    };
    let Some(items) = v.get("results").and_then(|r| r.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for it in items {
        let Some(url) = it.get("url").and_then(|u| u.as_str()) else {
            continue;
        };
        if !url.starts_with("http") {
            continue;
        }
        let title = clean_text(it.get("title").and_then(|t| t.as_str()).unwrap_or(""));
        let snippet = clean_text(it.get("description").and_then(|d| d.as_str()).unwrap_or(""));
        out.push(Result {
            title,
            url: url.to_string(),
            snippet: truncate(&snippet, 260),
        });
    }
    out
}

/// Brave Search organic-results extractor. Brave server-renders each hit
/// as `<div class="snippet …" data-pos="N" data-type="web">` carrying:
///   * the destination as the block's first external `href` (favicon and
///     thumbnail URLs live on `imgs.search.brave.com` — skipped),
///   * the clean FULL title in the title div's `title="…"` attribute
///     (`<div class="title search-snippet-title …" title="…">`) — no tag
///     stripping needed, no line-clamp ellipsis,
///   * the description under `class="generic-snippet"` → `class="content`.
/// Returns hits in page order; empty on any other layout so callers can
/// fall through to the generic walk.
pub fn brave_extract(html: &str) -> Vec<Result> {
    const BLOCK_MARKER: &str = "data-type=\"web\"";
    // Per-block scan cap: a Brave result block is ~5-15 KB of markup; 20 KB
    // covers it while keeping the worst case (marker in a huge page) cheap.
    const BLOCK_SCAN_CAP: usize = 20_000;
    let starts: Vec<usize> = html.match_indices(BLOCK_MARKER).map(|(i, _)| i).collect();
    let mut out = Vec::new();
    for (n, &i) in starts.iter().enumerate() {
        let hard_end = starts
            .get(n + 1)
            .copied()
            .unwrap_or(html.len())
            .min(i + BLOCK_SCAN_CAP);
        // `i` is a match start and `hard_end` either a match start or a
        // clamped raw offset — clamp the slice to a char boundary so
        // non-ASCII titles can never split a codepoint.
        let block = crate::caps::truncate_at_char_boundary(&html[i..], hard_end - i);
        let Some(url) = brave_block_url(block) else {
            continue;
        };
        let title = brave_title_re()
            .captures(block)
            .and_then(|c| c.get(1))
            .map(|m| collapse_ws(&decode_entities(m.as_str())))
            .unwrap_or_default();
        if title.len() < 5 || title.len() > 300 {
            continue;
        }
        let snippet = match block.find("class=\"generic-snippet") {
            Some(g) => extract_snippet(crate::caps::truncate_at_char_boundary(&block[g..], 2048)),
            None => String::new(),
        };
        out.push(Result {
            title,
            url,
            snippet,
        });
    }
    out
}

/// Bing organic-results extractor. Bing is the largest index INDEPENDENT of
/// Google (Ecosia is Google-backed), so it is the fleet's primary diversifier
/// — different crawl, different ranking, different long tail. Each hit is
/// `<li class="b_algo">…<h2><a href="…/ck/a?…&u=a1<base64url>…">TITLE</a>…
/// <div class="b_caption"><p>SNIPPET</p></div>`. Bing wraps every destination
/// in a `bing.com/ck/a?` redirect whose real URL is the `u` param: strip its
/// `a1` scheme-marker prefix and base64url-decode. Empty on the degraded
/// no-`b_algo` page Bing serves to under-fingerprinted requests (the caller's
/// full-browser-header fetch avoids it) so the rotation moves on.
pub fn bing_extract(html: &str) -> Vec<Result> {
    static A_RE: OnceLock<Regex> = OnceLock::new();
    let a_re = A_RE.get_or_init(|| {
        Regex::new(r#"<h2[^>]*>\s*<a\b[^>]*href="([^"]+)"[^>]*>(.*?)</a>"#).unwrap()
    });
    const BLOCK: &str = "class=\"b_algo\"";
    let starts: Vec<usize> = html.match_indices(BLOCK).map(|(i, _)| i).collect();
    let mut out = Vec::new();
    for (n, &i) in starts.iter().enumerate() {
        let end = starts.get(n + 1).copied().unwrap_or(html.len());
        let block = crate::caps::truncate_at_char_boundary(&html[i..], end - i);
        let Some(cap) = a_re.captures(block) else {
            continue;
        };
        let href = decode_entities(cap.get(1).map_or("", |m| m.as_str()));
        let url = bing_decode_url(&href);
        if !url.starts_with("http") {
            continue;
        }
        let title = clean_text(cap.get(2).map_or("", |m| m.as_str()));
        if title.len() < 3 || title.len() > 300 {
            continue;
        }
        let snippet = match block.find("class=\"b_caption") {
            Some(c) => extract_snippet(crate::caps::truncate_at_char_boundary(&block[c..], 2048)),
            None => String::new(),
        };
        out.push(Result {
            title,
            url,
            snippet,
        });
    }
    out
}

/// Unwrap a Bing `…/ck/a?…&u=a1<base64url>…` tracking link to its real
/// destination. Non-`ck/a` hrefs (Bing occasionally links direct) pass
/// through unchanged. On any decode failure returns the input so a bad
/// unwrap can never silently drop an otherwise-valid result.
pub(crate) fn bing_decode_url(href: &str) -> String {
    use base64::Engine as _;
    if !href.contains("/ck/a") {
        return href.to_string();
    }
    let Some(u) = href.split(['?', '&']).find_map(|p| p.strip_prefix("u=")) else {
        return href.to_string();
    };
    // The `u` value is `a1` (scheme marker for https) + base64url payload.
    let payload = u.strip_prefix("a1").unwrap_or(u);
    match base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')) {
        Ok(bytes) => String::from_utf8(bytes).unwrap_or_else(|_| href.to_string()),
        Err(_) => href.to_string(),
    }
}

/// Yahoo organic-results extractor. Yahoo's index is Bing-backed, so it does
/// not add index diversity — it earns its rotation slot as a FAILOVER host:
/// when Bing's own endpoint IP-throttles, `search.yahoo.com` is a separate
/// origin that still serves the same-quality results. Each organic hit lives
/// in a `<div class="algo-sr">` whose title anchor href is a
/// `r.search.yahoo.com/…/RU=<urlencoded-dest>/R[SK]=…` tracking redirect.
/// Empty on any other layout.
pub fn yahoo_extract(html: &str) -> Vec<Result> {
    static A_RE: OnceLock<Regex> = OnceLock::new();
    static H3_RE: OnceLock<Regex> = OnceLock::new();
    let a_re = A_RE.get_or_init(|| {
        Regex::new(r#"<a\b[^>]*href="(https://[^"]*?/RU=[^"]+)"[^>]*>(.*?)</a>"#).unwrap()
    });
    // The organic title lives in the `<h3>` NESTED inside the `/RU=` anchor,
    // after a favicon + domain + URL-breadcrumb preamble. Pull the `<h3>`
    // text so the breadcrumb never bleeds into the title.
    let h3_re = H3_RE.get_or_init(|| Regex::new(r"(?s)<h3\b[^>]*>(.*?)</h3>").unwrap());
    const BLOCK: &str = "algo-sr";
    let starts: Vec<usize> = html.match_indices(BLOCK).map(|(i, _)| i).collect();
    let mut out = Vec::new();
    for (n, &i) in starts.iter().enumerate() {
        let end = starts.get(n + 1).copied().unwrap_or(html.len());
        let block = crate::caps::truncate_at_char_boundary(&html[i..], end - i);
        let Some(cap) = a_re.captures(block) else {
            continue;
        };
        let url = yahoo_deref(&decode_entities(cap.get(1).map_or("", |m| m.as_str())));
        if !url.starts_with("http") {
            continue;
        }
        let inner = cap.get(2).map_or("", |m| m.as_str());
        let title = h3_re
            .captures(inner)
            .and_then(|c| c.get(1))
            .map(|m| clean_text(m.as_str()))
            .unwrap_or_default();
        if title.len() < 3 || title.len() > 300 {
            continue;
        }
        let snippet = match block.find("compText") {
            Some(c) => extract_snippet(crate::caps::truncate_at_char_boundary(&block[c..], 2048)),
            None => String::new(),
        };
        out.push(Result {
            title,
            url,
            snippet,
        });
    }
    out
}

/// Extract the real destination from a Yahoo `…/RU=<urlencoded>/R[SK]=…`
/// tracking href. Returns the input on any layout miss.
pub(crate) fn yahoo_deref(href: &str) -> String {
    let Some(after) = href.split("/RU=").nth(1) else {
        return href.to_string();
    };
    // The dest is percent-encoded and terminated by the next `/RK=` or `/RS=`.
    let enc = after
        .split("/RK=")
        .next()
        .and_then(|s| s.split("/RS=").next())
        .unwrap_or(after);
    crate::text_encoding::form_decode(enc)
}

/// First external destination link of a Brave result block — favicon /
/// thumbnail hosts and Brave-internal links are not destinations.
pub(crate) fn brave_block_url(block: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r#"href="(https?://[^"]+)""#).unwrap());
    for cap in re.captures_iter(block) {
        let u = decode_entities(cap.get(1).map_or("", |m| m.as_str()));
        if u.contains(".brave.com/") || u.contains("//brave.com") {
            continue;
        }
        return Some(u);
    }
    None
}
