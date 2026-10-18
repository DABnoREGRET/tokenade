//! **What remains once the scenery is gone.**
//!
//! The result regexes, snippet extraction, chrome trimming, entity decoding.
//! This is the mechanical half, and it is kept apart from the engines because
//! it does NOT change with them: an `&amp;` decodes the same way at all
//! seven.

use super::*;

pub(crate) fn result_link_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Capture group 1 = href value, group 2 = inner anchor HTML (titles
        // often wrap an <h3>/<h2>). Non-greedy and tolerant of unquoted
        // attributes, single/double quotes, attribute spacing.
        Regex::new(r#"<a [^>]*?href=["']([^"']+)["'][^>]*>(.*?)</a>"#).unwrap()
    })
}

pub(crate) fn snippet_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Match a chunk of relatively prose-y inline text — alphabetic with
    // spaces — long enough to be a snippet. Stops at the next angle bracket.
    RE.get_or_init(|| Regex::new(r"([^<>]{40,400})").unwrap())
}

pub(crate) fn extract_snippet(tail: &str) -> String {
    // Stop at the next `<a href` so we never pull a neighbouring result's text
    // into the current snippet — that was the source of the cross-talk bug
    // surfaced by `compact_caps_at_limit` against densely packed SERP markup.
    let bounded = match tail.find("<a ") {
        Some(i) => &tail[..i],
        None => tail,
    };
    let plain = strip_tags(bounded);
    let plain = decode_entities(&plain);
    for cap in snippet_re().captures_iter(&plain) {
        let s = cap.get(1).map_or("", |m| m.as_str()).trim();
        if s.contains("Cookies") || s.contains("Sign in") || s.contains("Privacy") {
            continue;
        }
        let s = cut_serp_chrome(&collapse_ws(s));
        if s.split_whitespace().nth(5).is_some() {
            return truncate(&s, 260);
        }
    }
    String::new()
}

/// Cut a snippet where the page stops describing the RESULT and starts
/// describing the search — "… update the previous syntax tree ... Searches
/// related to rust tree-sitter".
///
/// Engines put these blocks next to a result, and the extractor glues them on.
/// They cost tokens, they say nothing about the page, and they made the
/// snippet LONGER — which is how the cross-engine merge picked them: it keeps
/// the longest description for a URL, so the one polluted with chrome beat the
/// clean one. Cutting here fixes the merge too, because length means something
/// again once the chrome is gone.
///
/// Only unambiguous multi-word markers: a bare "Videos" or "Images" is
/// ordinary prose on plenty of pages.
pub(crate) fn cut_serp_chrome(s: &str) -> String {
    const MARKERS: &[&str] = &[
        "searches related to",
        "related searches",
        "people also ask",
        "people also search for",
        "show results with",
        "missing:",
    ];
    // ASCII-lowercase, NOT `to_lowercase`: the latter can change a string's
    // byte length (`İ` lowercases to two chars), and an index found in the
    // lowered copy would then slice `s` off a char boundary and panic. Real
    // snippets are full of non-ASCII. The markers are ASCII, so folding only
    // ASCII bytes is both sufficient and length-preserving.
    let lower = s.to_ascii_lowercase();
    let cut = MARKERS
        .iter()
        .filter_map(|m| lower.find(m))
        .min()
        .unwrap_or(s.len());
    s[..cut]
        .trim_end()
        .trim_end_matches(['.', '·', '|', '-', '…'])
        .trim_end()
        .to_string()
}

/// Tags out, words as the page wrote them. An engine HIGHLIGHTS the query
/// inside a word — `<b>select</b>!`, `<b>B</b>-<b>tree</b>` — and a space for
/// every tag delivered `select ! block` and `B - tree` (2026-10-03), tokens no
/// page contains. An inline tag joins; a block tag still separates.
pub(crate) fn strip_tags(s: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"</?([A-Za-z][A-Za-z0-9]*)[^>]*>|<[^>]+>").unwrap())
        .replace_all(s, |c: &regex::Captures| {
            let inline = c.get(1).is_some_and(|n| {
                matches!(
                    n.as_str().to_ascii_lowercase().as_str(),
                    "b" | "strong"
                        | "em"
                        | "i"
                        | "u"
                        | "span"
                        | "mark"
                        | "a"
                        | "code"
                        | "small"
                        | "sup"
                        | "sub"
                        | "abbr"
                        | "wbr"
                )
            });
            if inline {
                ""
            } else {
                " "
            }
        })
        .to_string()
}

pub(crate) fn clean_text(s: &str) -> String {
    let t = strip_tags(s);
    let t = decode_entities(&t);
    collapse_ws(&t)
}

pub(crate) fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n).collect();
    out.push('…');
    out
}

/// Resolve common SERP wrapper redirects: Google's `/url?q=https://…&sa=…`,
/// DuckDuckGo's `/l/?uddg=…` and the bare `//host/path` schemeless form.
pub(crate) fn resolve_url(href: &str) -> String {
    let h = href.trim();
    if let Some(idx) = h.find("/url?") {
        let qs = &h[idx + 5..];
        for part in qs.split('&') {
            if let Some(v) = part
                .strip_prefix("q=")
                .or_else(|| part.strip_prefix("url="))
            {
                return crate::text_encoding::form_decode(v);
            }
        }
    }
    if let Some(idx) = h.find("uddg=") {
        return crate::text_encoding::form_decode(h[idx + 5..].split('&').next().unwrap_or(""));
    }
    if let Some(rest) = h.strip_prefix("//") {
        return format!("https://{rest}");
    }
    h.to_string()
}

#[cfg(test)]
mod a_highlight_is_not_a_space {
    use super::clean_text;

    #[test]
    fn a_highlight_inside_a_word_leaves_the_word_whole() {
        assert_eq!(clean_text("the <b>select</b>! block"), "the select! block");
        assert_eq!(
            clean_text("a <b>B</b>-<b>tree</b> on its key"),
            "a B-tree on its key"
        );
        // A block tag still separates.
        assert_eq!(
            clean_text("first<br>second</p><p>third"),
            "first second third"
        );
    }
}
