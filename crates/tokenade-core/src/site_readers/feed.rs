//! RSS / Atom documents, and the feed a page declares.
//!
//! A feed is not a page: served through the generic HTML path it becomes tag
//! soup of dates and GUIDs, one text node per line.

use super::*;

pub fn declared_feed_url(html: &str, page_url: &str) -> Option<String> {
    let tag = re(r#"(?is)<link\b[^>]*type\s*=\s*["']application/(?:rss|atom)\+xml["'][^>]*>"#)
        .find(html)?
        .as_str();
    let caps = re(&format!(r#"(?is)href\s*=\s*{}"#, crate::web::ATTR_VALUE)).captures(tag)?;
    let href = crate::web::quoted_value(&caps, 1)?.trim();
    if href.is_empty() {
        return None;
    }
    if href.starts_with("http://") || href.starts_with("https://") {
        return Some(href.to_string());
    }
    let (scheme, host) = crate::url::scheme_host(page_url)?;
    Some(if let Some(rest) = href.strip_prefix("//") {
        format!("{scheme}://{rest}")
    } else if href.starts_with('/') {
        format!("{scheme}://{host}{href}")
    } else {
        // Relative to the page's DIRECTORY, not the site root: `/blog/` +
        // `feed.xml` is `/blog/feed.xml`.
        let dir = crate::url::scheme_host(page_url)
            .and_then(|_| host_path(page_url))
            .map(|(_, p)| match p.rfind('/') {
                Some(i) => p[..=i].to_string(),
                None => "/".to_string(),
            })
            .unwrap_or_else(|| "/".to_string());
        format!("{scheme}://{host}{dir}{href}")
    })
}

/// Text of the first `<tag>…</tag>` inside `xml`, entity-decoded and stripped
/// of CDATA wrappers and any inline HTML. Deliberately a scan rather than a
/// parser: feeds in the wild are frequently malformed, and a strict parser
/// rejects whole documents over one unescaped ampersand.
pub(super) fn xml_tag(xml: &str, tag: &str) -> Option<String> {
    // `<title>` must not match `<title2>`; require a terminator after the name.
    let open = format!("<{tag}");
    let start = xml.find(&open)?;
    let rest = &xml[start + open.len()..];
    if !rest.starts_with('>') && !rest.starts_with(' ') && !rest.starts_with('\t') {
        return None;
    }
    let body_start = start + open.len() + rest.find('>')? + 1;
    let end = xml[body_start..].find(&format!("</{tag}>"))? + body_start;
    let raw = &xml[body_start..end];
    let raw = raw
        .trim()
        .trim_start_matches("<![CDATA[")
        .trim_end_matches("]]>")
        .trim();
    // Feed bodies routinely carry DOUBLY-encoded HTML (`&lt;p&gt;`). Decoding
    // must come first: `fragment_to_markdown` strips tags before decoding
    // entities, so a single pass leaves real `<p>` markup behind (MEASURED on
    // a Rust Blog item).
    let decoded = crate::web::decode_entities(raw);
    let text = crate::web::fragment_to_markdown(&decoded);
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Atom links live in an attribute (`<link href="…"/>`), RSS ones in the body.
pub(super) fn feed_entry_link(entry: &str) -> Option<String> {
    if let Some(l) = xml_tag(entry, "link") {
        return Some(l);
    }
    let i = entry.find("<link")?;
    let seg = &entry[i..entry[i..].find('>')? + i];
    let h = seg.find("href=")?;
    let after = &seg[h + 5..];
    let quote = after.chars().next()?;
    // `&after[1..]` split inside a multi-byte quote char and panicked.
    let after = &after[quote.len_utf8()..];
    let end = after.find(quote)?;
    Some(after[..end].to_string())
}

/// Render an RSS 2.0 or Atom feed as a dated entry list.
///
/// Mirrors the shape the Lea backend's `feed_parsing` settled on: an entry is
/// worth keeping when it has a title, and the fields that matter are title,
/// link, date and summary. Validity is the same test it uses — a feed with no
/// entries is not a feed, so we return `None` and the generic pipeline runs.
/// A heading for a titleless feed entry: the first sentence-ish run of its
/// content, tags stripped. Bounded so a long post cannot produce a heading that
/// is itself the post.
pub(super) fn feed_entry_headline(entry: &str) -> Option<String> {
    let raw = xml_tag(entry, "description")
        .or_else(|| xml_tag(entry, "content:encoded"))
        .or_else(|| xml_tag(entry, "summary"))
        .or_else(|| xml_tag(entry, "content"))?;
    let text = crate::web::decode_entities(&re(r"(?is)<[^>]+>").replace_all(&raw, " "));
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return None;
    }
    let head: String = text.chars().take(90).collect();
    // Cut at the last word boundary so the heading does not end mid-word, and
    // say so with an ellipsis when the post is longer than the heading.
    Some(if text.chars().count() > 90 {
        let cut = head.rfind(' ').unwrap_or(head.len());
        format!("{}…", head[..cut].trim_end())
    } else {
        head
    })
}

pub(super) fn feed_document(xml: &str) -> Option<String> {
    // ASCII-only lowercasing: `to_lowercase()` is Unicode-aware and CHANGES
    // byte length (`İ` 2→3, `K` U+212A 3→1), so offsets found in it no longer
    // line up with `xml` — a Turkish channel title silently misaligned every
    // entry slice. Tag names are ASCII, so this is both correct and
    // length-preserving.
    let lower = xml.to_ascii_lowercase();
    if !(lower.contains("<rss") || lower.contains("<feed") || lower.contains("<rdf:rdf")) {
        return None;
    }
    // The channel title sits before the first entry; taking it from the whole
    // document would otherwise pick up the first item's title on some feeds.
    let head_end = lower
        .find("<item")
        .or_else(|| lower.find("<entry"))
        .unwrap_or(xml.len());
    let title = xml_tag(&xml[..head_end], "title").unwrap_or_else(|| "Feed".into());
    let mut out = format!("# {title}");
    if let Some(d) =
        xml_tag(&xml[..head_end], "subtitle").or_else(|| xml_tag(&xml[..head_end], "description"))
    {
        out.push_str(&format!("\n\n{d}"));
    }

    let (open, close) = if lower.contains("<item") {
        ("<item", "</item>")
    } else {
        ("<entry", "</entry>")
    };
    let mut entries = 0usize;
    let mut cursor = 0usize;
    let mut body = String::new();
    while entries < FEED_MAX_ENTRIES && body.len() < FEED_MAX_CHARS {
        let Some(s) = lower[cursor..].find(open) else {
            break;
        };
        let s = cursor + s;
        let Some(e) = lower[s..].find(close) else {
            break;
        };
        let e = s + e + close.len();
        let entry = &xml[s..e];
        cursor = e;
        // A microblog post has no title — that is the FORMAT, not a broken
        // entry. Dropping the item for a missing heading threw away the post
        // itself: MEASURED 2026-07-22 on mastodon.social/@Gargron.rss, a valid
        // 20-item RSS where every item carries `description`/`link`/`pubDate`
        // and no `title`, so all 20 were skipped, `entries` stayed 0 and the
        // whole feed rendered as None. Every Mastodon instance, and any feed of
        // titleless posts, was invisible for the same reason.
        //
        // The heading falls back to the first line of the content, which is
        // what a reader of that post would call it; the content is emitted
        // below either way, so nothing depends on the fallback being pretty.
        let title = xml_tag(entry, "title").or_else(|| feed_entry_headline(entry));
        entries += 1;
        match title {
            Some(t) => body.push_str(&format!("\n\n## {t}")),
            // No title AND no content to derive one from: keep the entry (its
            // date and link are still facts) rather than deleting it.
            None => body.push_str("\n\n## (untitled)"),
        }
        let date = xml_tag(entry, "pubDate")
            .or_else(|| xml_tag(entry, "published"))
            .or_else(|| xml_tag(entry, "updated"))
            .or_else(|| xml_tag(entry, "dc:date"));
        if let Some(d) = date {
            body.push_str(&format!("\n{d}"));
        }
        if let Some(l) = feed_entry_link(entry) {
            body.push_str(&format!("\n<{l}>"));
        }
        // `content:encoded` carries the full post when present; the summary
        // fields are the fallback.
        let summary = xml_tag(entry, "description")
            .or_else(|| xml_tag(entry, "summary"))
            .or_else(|| xml_tag(entry, "content:encoded"))
            .or_else(|| xml_tag(entry, "content"));
        if let Some(s) = summary {
            let s: String = s.chars().take(600).collect();
            body.push_str(&format!("\n\n{s}"));
        }
    }
    if entries == 0 {
        return None;
    }
    out.push_str(&body);
    // **A silent cap reads like a complete feed.** The loop stops at the entry
    // count or at the character budget, and the final line said
    // `[25 entries]` — indistinguishable from a feed holding exactly 25. The
    // case is ordinary since reddit threads go through their Atom feed: a
    // 300-reply thread arrived as a 25-reply one. What remains after the
    // cursor is countable, so the line says it.
    let left = lower[cursor..].matches(open).count();
    if left > 0 {
        out.push_str(&format!(
            "\n\n[{entries} of {} entries — the rest was cut at the reader's cap]",
            entries + left
        ));
    } else {
        out.push_str(&format!("\n\n[{entries} entries]"));
    }
    Some(out)
}

pub(super) fn feed_plan(url: &str) -> Option<Plan> {
    Some(Plan::get(url.to_string(), Kind::FeedDocument, "feed"))
}

/// The feed a page declares in `<link rel="alternate" type="application/rss+xml">`,
/// absolutised against `page_url`.
///
/// This answers "where are this site's articles?" using the index the site
/// publishes itself, instead of a hand-written reader per publisher. It is a
/// NARROW lever, measured: of 15 sites whose home page distils to nothing,
/// 4 declare a feed and 2 of those feeds actually resolve (ign.com and
/// cbsnews.com declare URLs that 404 on their own servers). Where it does
/// fire the gain is large — independent.co.uk 212 → 2 668 tokens,
/// techradar.com 203 → 3 393.
/// Does `feed_url` belong to THIS page rather than to the whole site?
///
/// `is_site_index` exists because serving "the site's latest" to someone who
/// asked for one article answers a different question. That reasoning does not
/// apply when the declared feed is DERIVED FROM the page's own path:
/// `/@Gargron` declares `/@Gargron.rss`, which is that profile's posts and
/// nothing else. Same shape for a WordPress author/category page
/// (`/author/x/feed/`), a subreddit (`/r/x/.rss`) and a forum topic.
///
/// MEASURED 2026-07-22 on mastodon.social/@Gargron: the profile distils to a
/// 188-char shell (name, bio and counters from `og:` tags — the posts arrive by
/// XHR), while the feed it declares carries 20 full posts, session-less.
///
/// The page path must be more than `/`: at the root every path is a prefix, and
/// the site-index branch already covers that case.
pub(crate) fn feed_is_page_scoped(feed_url: &str, page_url: &str) -> bool {
    let path = |u: &str| -> String {
        let after = u.split("://").nth(1).unwrap_or(u);
        let p = after.find('/').map(|i| &after[i..]).unwrap_or("/");
        p.split(['?', '#'])
            .next()
            .unwrap_or("/")
            .trim_end_matches('/')
            .to_string()
    };
    let (page, feed) = (path(page_url), path(feed_url));
    if page.len() <= 1 {
        return false;
    }
    // Same host only: a feed on another domain is someone else's content.
    match (
        crate::url::scheme_host(page_url),
        crate::url::scheme_host(feed_url),
    ) {
        (Some((_, a)), Some((_, b))) if a == b => {}
        _ => return false,
    }
    feed.starts_with(&page)
}

/// Is `url` a site ROOT or a shallow section index, rather than a specific
/// document?
///
/// Feed substitution is only honest at the top of a site: serving "the site's
/// latest articles" to someone who asked for one specific article would answer
/// a different question than the one posed.
pub fn is_site_index(url: &str) -> bool {
    let Some((_, _)) = crate::url::scheme_host(url) else {
        return false;
    };
    let Some((_, path)) = host_path(url) else {
        return false;
    };
    let clean = path.split(['?', '#']).next().unwrap_or("");
    clean.split('/').filter(|s| !s.is_empty()).count() <= 1
}

/// Entries rendered from a feed, and the cap on their combined size.
pub(crate) const FEED_MAX_ENTRIES: usize = 30;
pub(crate) const FEED_MAX_CHARS: usize = 24_000;
