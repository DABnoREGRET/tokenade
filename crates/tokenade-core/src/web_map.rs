//! Web crawl/map — discover URLs reachable from a start page: same-registrable-
//! domain anchors on the page plus `<loc>` entries in the site's `/sitemap.xml`.
//!
//! Net-new capability. It touches no existing extraction path, so it carries no
//! regression risk to `web`/`read`. Kept deliberately shallow (one hop + the
//! sitemap) — a recursive frontier crawl is a larger, opt-in follow-up.

use scraper::{Html, Selector};
use std::collections::BTreeSet;

/// Discover up to `max` same-domain URLs from `start_url`: anchors on the page
/// plus `<loc>` entries in `/sitemap.xml`. Deduped, fragment-stripped, sorted.
pub fn discover_urls(start_url: &str, max: usize) -> Vec<String> {
    let base = match reqwest::Url::parse(start_url) {
        Ok(u) => u,
        Err(_) => return Vec::new(),
    };
    let root = base
        .host_str()
        .map(crate::url::registrable_domain)
        .unwrap_or_default();
    if root.is_empty() {
        return Vec::new();
    }
    let mut found: BTreeSet<String> = BTreeSet::new();

    if let Some(f) = crate::web_fetch::fetch(
        start_url,
        &crate::web_fetch::FetchOpts::page("15", 5_000_000),
    ) {
        let html = String::from_utf8_lossy(&f.bytes);
        collect_page_links(&html, &base, &root, &mut found);
    }

    if let Ok(sm) = base.join("/sitemap.xml") {
        if let Some(f) = crate::web_fetch::fetch(
            sm.as_str(),
            &crate::web_fetch::FetchOpts::page("15", 10_000_000),
        ) {
            if f.status == 200 {
                let xml = String::from_utf8_lossy(&f.bytes);
                collect_sitemap_locs(&xml, &base, &root, &mut found);
            }
        }
    }

    found.into_iter().take(max).collect()
}

fn same_domain(u: &reqwest::Url, root: &str) -> bool {
    matches!(u.scheme(), "http" | "https")
        && u.host_str().map(crate::url::registrable_domain).as_deref() == Some(root)
}

/// Light normalisation: drop the fragment; everything else is left intact so a
/// caller can fetch the URL verbatim.
fn normalize(mut u: reqwest::Url) -> String {
    u.set_fragment(None);
    u.to_string()
}

fn collect_page_links(html: &str, base: &reqwest::Url, root: &str, out: &mut BTreeSet<String>) {
    let doc = Html::parse_document(&crate::html_nesting::limit_nesting(html));
    let sel = match Selector::parse("a[href]") {
        Ok(s) => s,
        Err(_) => return,
    };
    for a in doc.select(&sel) {
        if let Some(href) = a.value().attr("href") {
            if let Ok(u) = base.join(href) {
                if same_domain(&u, root) {
                    out.insert(normalize(u));
                }
            }
        }
    }
}

fn collect_sitemap_locs(xml: &str, base: &reqwest::Url, root: &str, out: &mut BTreeSet<String>) {
    // `<loc>URL</loc>` — covers both `<urlset>` and `<sitemapindex>` entries.
    for seg in xml.split("<loc>").skip(1) {
        if let Some(end) = seg.find("</loc>") {
            let decoded = seg[..end].trim().replace("&amp;", "&");
            if let Ok(u) = base.join(&decoded) {
                if same_domain(&u, root) {
                    out.insert(normalize(u));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_links_same_domain_only() {
        let base = reqwest::Url::parse("https://ex.com/a").unwrap();
        let mut out = BTreeSet::new();
        collect_page_links(
            r#"<a href="/b">b</a><a href="https://other.com/x">x</a><a href="https://ex.com/c#f">c</a><a href="mailto:z@ex.com">z</a>"#,
            &base,
            "ex.com",
            &mut out,
        );
        assert!(out.contains("https://ex.com/b"), "{out:?}");
        assert!(out.contains("https://ex.com/c"), "fragment strip: {out:?}");
        assert!(!out.iter().any(|u| u.contains("other.com")), "{out:?}");
        assert!(!out.iter().any(|u| u.contains("mailto")), "{out:?}");
    }

    #[test]
    fn sitemap_locs_parsed_and_filtered() {
        let base = reqwest::Url::parse("https://ex.com/").unwrap();
        let mut out = BTreeSet::new();
        collect_sitemap_locs(
            "<urlset><url><loc>https://ex.com/p1</loc></url>\
             <url><loc>https://ex.com/p2?a=1&amp;b=2</loc></url>\
             <url><loc>https://other.com/z</loc></url></urlset>",
            &base,
            "ex.com",
            &mut out,
        );
        assert!(out.contains("https://ex.com/p1"), "{out:?}");
        assert!(
            out.iter().any(|u| u.contains("p2?a=1&b=2")),
            "entity decode: {out:?}"
        );
        assert!(!out.iter().any(|u| u.contains("other.com")), "{out:?}");
    }

    #[test]
    fn subdomain_counts_as_same_registrable_domain() {
        let base = reqwest::Url::parse("https://www.ex.com/").unwrap();
        let mut out = BTreeSet::new();
        collect_page_links(
            r#"<a href="https://blog.ex.com/post">p</a>"#,
            &base,
            "ex.com",
            &mut out,
        );
        assert!(out.contains("https://blog.ex.com/post"), "{out:?}");
    }
}
