//! Search-engine results page (SERP) compactor — Google, Bing, DuckDuckGo,
//! Brave, Kagi, Yandex.
//!
//! `web::html_to_markdown` already strips HTML chrome, but a SERP is special:
//! after the strip we get a long, weakly-structured list interleaved with ads,
//! "people also ask", knowledge panels, image strips, footer-link spam, and
//! related searches — typically 50-200 KB of HTML for what an agent actually
//! needs (the top ~10 organic results: title, URL, snippet).
//!
//! This module detects SERP HTML and emits a one-line-per-result markdown
//! listing, deduped by hostname+path so a single domain can't dominate. Falls
//! back to returning the input unchanged when the page is clearly not a SERP
//! (caller can then route to `web::html_to_markdown`).

mod engines;
mod quality;
mod scrub;
pub use engines::*;
pub use quality::*;
pub(crate) use scrub::*;

use crate::{tokenizer, Savings};
use regex::Regex;
use std::collections::HashSet;
use std::fmt::Write;
use std::sync::OnceLock;

/// One extracted organic result.
#[derive(Debug, Clone)]
pub struct Result {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Return `true` if `html` smells like a SERP from a major engine. Used both
/// as a stand-alone check and by [`auto::detect`].
///
/// #709 — only HTML-structural class/id signals trigger. Pre-#709 the list
/// also included engine URL substrings like "google.com/search" / "Google
/// Search" / "duckduckgo.com" — each by itself enough to confirm SERP.
/// Those strings appear in any prose document about SEO / search engines /
/// web research, and a single mention misrouted plain `.txt` content
/// through extract() (returns near-empty on non-HTML) → 90%+ data loss.
/// Production: `cat /…/programmatic-saas-seo-playbook.txt` saved 6891
/// tokens via auto:serp because the file mentioned `google.com/search`
/// once. HTML-structural signals (`class="b_algo"`, `id="rso"`, …) are
/// quoted attribute values and only appear in actual HTML, so they're
/// both necessary AND sufficient for SERP confirmation.
pub fn detect(html: &str) -> bool {
    // #710 — defense in depth following #709. Even with the engine-URL
    // list removed, markdown / source docs that document SERP markup
    // verbatim ("the results container is `id=\"rso\"`", or an HTML
    // code block showing `<div class="b_algo">`) would still trigger
    // on the structural signals. Mirror the stealth.rs source-or-doc
    // pre-filter (#395) which short-circuits detection on inputs that
    // look like Rust / Python / markdown / log output. Real HTML
    // returns false from this check (it starts with `<!DOCTYPE` /
    // `<html>`) so genuine SERP pages still flow through.
    if crate::stealth::looks_like_source_or_doc(html) {
        return false;
    }
    let head: &str = html.get(..html.len().min(8192)).unwrap_or(html);
    // #285 — the doc comment above claims these signals are "both necessary
    // AND sufficient" because they are quoted attribute values. That holds for
    // the engine's OWN results markup and fails for the four generic names: an
    // ordinary site carries `id="search"` on its search box, `id="links"` on a
    // nav container, `class="snippet"` on a teaser card and `class="search-result"`
    // on an on-site search widget. Measured over saved `.html` pages: 27 were
    // claimed as SERP and 25 of them on `id="search"` ALONE — news articles
    // delivered as a numbered link list, 96692 -> 525 bytes, the article text
    // gone. So a generic name now needs a second signal to stand.
    let engine = [
        "id=\"rso\"",              // Google
        "data-async-context",      // Google
        "id=\"b_results\"",        // Bing
        "class=\"b_algo\"",        // Bing
        "class=\"results_links",   // DuckDuckGo
        "class=\"result__title\"", // DuckDuckGo
        "content=\"Brave Search\"",
        "class=\"serp-item\"", // Yandex
    ];
    if engine.iter().any(|s| head.contains(*s)) {
        return true;
    }
    let generic = [
        "id=\"search\"",
        "id=\"links\"",
        "class=\"snippet\"",
        "class=\"search-result\"",
    ];
    generic.iter().filter(|s| head.contains(**s)).count() >= 2
}

/// Compact a SERP page into a deduped top-N result list. Defaults to 10
/// organic hits, which matches Google's first page and what an agent
/// realistically uses to answer a question.
pub fn compact(html: &str) -> String {
    compact_with(html, 10)
}

pub fn compact_with(html: &str, limit: usize) -> String {
    let results = extract(html);
    if results.is_empty() {
        // Fall back to a markdown render so the caller still gets *something*.
        return crate::web::html_to_markdown(html);
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = String::new();
    let mut count = 0usize;
    for r in results {
        let key = dedup_key(&r.url);
        if !seen.insert(key) {
            continue;
        }
        let _ = write!(out, "{}. {}\n   {}\n", count + 1, r.title, r.url);
        if !r.snippet.is_empty() {
            let _ = writeln!(out, "   {}", r.snippet);
        }
        count += 1;
        if count >= limit {
            break;
        }
    }
    out.trim_end().to_string()
}

pub fn compact_with_savings(html: &str) -> (String, Savings) {
    let before = tokenizer::estimate(html);
    let out = compact(html);
    let after = tokenizer::estimate(&out);
    (out, Savings::new(before, after))
}

#[cfg(test)]
include!("serp_tests.rs");
