//! Web-content compression for AI coding agents.
//!
//! Two complementary entry points, both dependency-free Rust over `regex` +
//! `std`:
//!
//! - [`html_to_markdown`] — turn a raw HTML page (think `WebFetch` result) into
//!   compact markdown: strip script/style/nav/footer/svg noise, isolate the
//!   `<main>` / `<article>` block (Readability's "highest content density"
//!   idea, simplified), and convert structural tags to markdown.
//! - [`compact_a11y_tree`] — squeeze a Playwright/`browser_snapshot`
//!   accessibility-tree dump: drop the `[ref=eN]` markers the agent rarely
//!   needs for a read, collapse redundant wrappers, fold long uniform lists.
//!
//! Inspired (with attribution) by: mozilla/readability (Apache-2.0), kepano/
//! defuddle (MIT), jina-ai/reader (Apache-2.0), matthewwithanm/markdownify
//! (MIT), adbar/trafilatura (Apache-2.0). None of their code is copied — only
//! the heuristics that survived a line-by-line review are ported.
//!
//! A further batch of extraction techniques (code-fence sanctuary, table-cell
//! compression, nav/main ratio check, the 10%-isolation floor and the smaller
//! items around headings-in-links / SPA-shell signals / stack-guarded parsing)
//! is adapted, with attribution, from ruchit-p/essence (MIT),
//! `backend/src/format/markdown.rs`. Ported as heuristics into this one
//! converter, not copied as a second pipeline.

mod code_blocks;
mod elements;
mod entities;
// Re-exported rather than wrapped: `decode_entities` was only a second name
// for the same function, and the site readers called it by that name.
pub use entities::decode_entities;
mod absolute_links;
pub use absolute_links::resolve_links;
mod link_pruning;
use code_blocks::*;
use elements::*;
pub(crate) use elements::{cell_align_marker, md_link};
use entities::*;
pub(crate) use link_pruning::*;
mod a11y;
mod plain_text;
mod selectors;
mod structured;
use crate::tokenizer;
// The shared regex cache, reached through `use super::*` from the nineteen
// children. It used to live in the middle of the file; it belongs with the
// mods.
use crate::regexes::re;
pub use a11y::*;
pub use plain_text::*;
use regex::Regex;
pub use selectors::*;
use std::fmt::Write;
pub use structured::*;

mod cleanup;
mod jsonld;
mod navigation;
pub mod pipeline;
mod title;
use cleanup::*;
use jsonld::*;
use navigation::*;
use title::*;

/// Compress an HTML document to compact markdown.
///
/// Steps (each justified by a Readability/Defuddle heuristic):
///   1. strip every `<script|style|noscript|svg|iframe|template>` block
///   2. strip semantic chrome: `<header|footer|nav|aside|form|button>`
///   3. isolate `<main>`/`<article>` if present (else use `<body>`)
///   4. convert heading / paragraph / link / list / code tags to markdown
///   5. drop remaining tags, decode entities, collapse whitespace
pub fn html_to_markdown(html: &str) -> String {
    tidy_delivered_markdown(&html_to_markdown_inner(html))
}

/// The final grooming of the delivered markdown: what nobody reads, nobody
/// pays for.
///
/// Two rules, both LOSSLESS, measured on the 39 real pages of
/// `tests/fixtures/pages` by `tests/web_headroom.rs`:
///
/// - **repeated blank lines** (0.7% of what is still delivered): two empty
///   lines and five render the same markdown. Never INSIDE a code block, where
///   an empty line is content.
/// - **the anchor that repeats the link text** (0.6%): `[Configuration]`
///   `(#configuration)` teaches nobody anything. An anchor that NAMES another
///   section (`[see here](#known-limits)`) says where it leads and stays.
///   Strict on purpose: one more word on either side and the anchor speaks.
///
/// It applies HERE, on the output path, and not in a converter:
/// `html_to_markdown` has THREE returns (anti-bot challenge, thin page, normal
/// page) and both converters — the tree and the legacy regex — go through this
/// gate. A pass placed in only one of them would have been the usual defect in
/// reverse: proven on one consumer, absent from its sibling.
fn tidy_delivered_markdown(md: &str) -> String {
    let link = match regex::Regex::new(r"\[([^\]\n]+)\]\(#([^)\s]*)\)") {
        Ok(r) => r,
        Err(_) => return md.to_string(),
    };
    let mut out = String::with_capacity(md.len());
    let mut in_fence = false;
    let mut blanks = 0usize;
    for line in md.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
        }
        if in_fence {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if line.trim().is_empty() {
            blanks += 1;
            if blanks > 1 {
                continue;
            }
            out.push('\n');
            continue;
        }
        blanks = 0;
        let tidied = link.replace_all(line, |c: &regex::Captures| {
            if says_the_same_words(&c[1], &c[2]) {
                c[1].to_string()
            } else {
                c[0].to_string()
            }
        });
        out.push_str(&tidied);
        out.push('\n');
    }
    // `lines()` eats the line ending: do not invent one on a document that
    // had none.
    if !md.ends_with('\n') {
        while out.ends_with('\n') {
            out.pop();
        }
    }
    out
}

/// Do two labels say the same words? `"Return value"` versus `return_value`
/// versus `return-value`: yes. Strict on purpose — one more word on either
/// side and the target says something.
///
/// **The only definition.** It existed in THREE copies: the rule shipped
/// here, `slug_eq` under `#[cfg(test)]` in this same file (for the headroom
/// candidates of the a11y output, where it compares an accessible name with a
/// URL fragment), and `same_words` in `tests/web_headroom.rs` (for the
/// candidates on the markdown). Three spellings of the same question, two of
/// which served to QUANTIFY what the third ships: they could have diverged
/// without anything saying so, and the figure would then have described a
/// rule that is not the one running.
pub fn says_the_same_words(text: &str, fragment: &str) -> bool {
    let norm = |s: &str| {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    let (a, b) = (norm(text), norm(fragment));
    !a.is_empty() && a == b
}

fn html_to_markdown_inner(html: &str) -> String {
    if html.trim_start().starts_with("<?xml") {
        if let Some(md) = pipeline::xml_document_shortcut(html) {
            return md;
        }
    }
    // **Presentational duplicate guard.** CSS marquees repeat their track so
    // the scroll loops, and responsive image sets ship one `<img>` per
    // breakpoint sharing one `alt` — both reach the agent as content repeated
    // 4-8×. Structural, so it runs on the HTML before any conversion: the
    // redundancy is a property of the container (adjacent siblings that
    // render identically), and scoring the emitted text instead is the
    // failure mode this file has already disproven.
    // Tree path: the same de-duplication runs inside the converter's own parse
    // (`tree_dedup`), so this second full parse in tree-sitter is skipped —
    // it was 39 % of distill wall-clock.
    let deduped = if crate::web_tree::web_tree_enabled() {
        html.to_string()
    } else {
        crate::dom_dedup::dedup_repeated_siblings(html)
    };
    let html: &str = &deduped;
    // **Anti-bot challenge guard.** If the page is actually a Cloudflare /
    // hCaptcha / Datadome / Akamai challenge, summarising it as if it were
    // article content is worse than useless — the agent will hallucinate a
    // "no information" answer. Prepend a clear `[tokenade: challenge=…]` banner
    // plus the 3-tier stealth remediation, so the next call routes correctly.
    // **The wall AND the origin error, in the same place.** `edge_error` is
    // the twin of `stealth` — its own doc says so: "a bot wall says *prove
    // you are human*, this module covers the other way a fetch returns
    // something that is not the page". The first was wired in here, the
    // second had only ONE caller, the hook that intercepts the agent's
    // WebFetch. The `web` route, the one the product recommends and the #1
    // family in the usage ranking, therefore never consulted it: `tokenade
    // web` on a `DEPLOYMENT_NOT_FOUND` page returned `# 404 /
    // DEPLOYMENT_NOT_FOUND`, exactly what that module exists to prevent. The
    // same lesson in the other direction — count the callers of a general
    // mechanism.
    //
    // The wall goes first: it is more specific, and a Cloudflare challenge
    // page also carries Cloudflare error markers.
    let preamble = crate::stealth::detect_and_remediate_any(html)
        .map(|(_kind, msg)| format!("{msg}\n\n---\n\n"))
        .or_else(|| {
            crate::edge_error::detect(html)
                .map(|kind| format!("{}\n---\n\n", crate::edge_error::note(kind)))
        })
        .unwrap_or_default();
    // Harvest the rescue sources BEFORE the noise strip deletes them:
    // `<noscript>` (Discourse-style crawler render lives there) and embedded
    // page state (`<script type="application/ld+json">`, `__NEXT_DATA__`) —
    // on JS-first pages that's where the actual copy is.
    let noscript_inner = harvest_noscript(html);
    let meta_header = harvest_meta_header(html);
    // Code-fence sanctuary (essence, MIT — see module doc): stash `<pre>`/
    // `<code>` BEFORE the noise-tag strip, which would otherwise delete a
    // documented `<title>`/`<script>` example living inside a code sample
    // exactly like it deletes the real element.
    let (stashed, code_stash) = sanctuary_code_blocks(html);

    let s = pipeline::strip_markup_noise(stashed);
    let mut body = pipeline::extraction_ladder(&s, html, &code_stash, &noscript_inner);
    // How much the PAGE itself yielded, before site-level metadata is added.
    // `og:title`/`og:description` are the same on every URL of a domain, so
    // counting them as recovered content lets a page that gave up NOTHING
    // clear the thin-result floor: crates.io/crates/serde has no crate data in
    // its HTML at all (5 KB, the string "serde" appears once, inside an image
    // URL) and was served as `# crates.io: Rust Package Registry` with no hint
    // that the page's own content was absent.
    let page_recovered = body.chars().count();
    if let Some(header) = meta_header {
        body = pipeline::prepend_meta_header(body, &header);
    }
    // A REAL challenge interstitial has (almost) no extractable content; when
    // the ladder recovered a substantial body, the detector hit a stray
    // mention (a login form's captcha, a cookie-notice script) and the
    // banner would be a misleading false positive — suppress it.
    if !(preamble.is_empty() || body.chars().count() >= CHALLENGE_BANNER_MAX_BODY_CHARS) {
        return format!("{preamble}{body}");
    }
    if let Some(note) = pipeline::thin_result_note(html, page_recovered, &body) {
        return note;
    }
    // Citation compaction (tree path only): hoist repeated/long URLs into a
    // References block after ALL link-density pruning has run. Flag-gated, so
    // production output is unchanged. This is the normal-page return; the
    // challenge branch above carries a preamble and too few links to compact.
    let body = if crate::web_tree::web_tree_enabled() {
        // The neighbourhood is judged on the FINAL output, so after the links
        // are moved out: that is where the lines are what the agent will read.
        crate::web_tree::drop_alts_their_neighbourhood_already_says(
            crate::web_tree::compact_citations(&body),
        )
    } else {
        body
    };
    prepend_mostly_navigation_note(html, &body)
}

/// Below this input size a near-empty result is simply a small page, not a
/// failure worth explaining.
const EMPTY_RESULT_MIN_HTML_BYTES: usize = 2048;

/// Extracted-body size above which a detected anti-bot challenge is treated
/// as a false positive (real block pages are near-empty).
const CHALLENGE_BANNER_MAX_BODY_CHARS: usize = 2_000;

/// Below this many extracted chars, a main-content isolation is treated as
/// failed and the relaxed pass runs (mirrors the ~200-char validity floor
/// readability-style extractors converge on).
const EXTRACT_VALID_MIN_CHARS: usize = 200;

/// Below this many extracted chars, the page is thin enough that mining the
/// embedded state (JSON-LD, `__NEXT_DATA__`) is worth appending — JS-first
/// sites keep their real copy there, not in the served DOM.
const EMBED_RESCUE_MAX_CHARS: usize = 400;

/// Attribute matcher that survives `>` inside quoted values
/// (`media="(max-width >= 40rem)"` is a real-world case that a naive
/// `[^>]*` splits mid-tag, leaking `= 40rem)" rel="stylesheet"…` as text).
const TAG_ATTRS: &str = r#"(?:[^>"']|"[^"]*"|'[^']*')*"#;

/// One attribute VALUE, delimited the way HTML actually delimits it: a value
/// opened with `"` runs to the next `"` and may contain `'` freely, and the
/// mirror for `'`.
///
/// The idiom this replaces — `["']([^"']*)["']` — opened on either quote and
/// closed on either, so it stopped at the first quote character of EITHER kind.
/// Measured on goodreads.com:
/// `content="…Goodreads, the world's largest site for readers…"` yielded
/// `…Goodreads, the world`. An apostrophe is ordinary English, so this silently
/// truncated descriptions and `alt` text across the corpus.
///
/// `regex` has no backreferences, so the two quotings are spelled out and the
/// value lands in one of TWO groups — read it with [`quoted_value`].
pub(crate) const ATTR_VALUE: &str = r#"(?:"([^"]*)"|'([^']*)')"#;

/// The group that actually matched in an [`ATTR_VALUE`] alternation, given the
/// 1-based index of its FIRST group.
pub(crate) fn quoted_value<'a>(c: &regex::Captures<'a>, first: usize) -> Option<&'a str> {
    c.get(first)
        .or_else(|| c.get(first + 1))
        .map(|m| m.as_str())
}

// Six more subjects out of the trunk. The file keeps only the shared
// primitives (`TAG_ATTRS`, `ATTR_VALUE`, `quoted_value`) and the `mod`s.
mod clickable;
mod convert;
mod feeds;
mod isolate;
mod prune_tree;
mod spa_rescue;
pub(crate) use clickable::*;
pub use convert::*;
pub use feeds::*;
pub(crate) use isolate::*;
use prune_tree::*;
pub use spa_rescue::*;

#[cfg(test)]
include!("web_tests.rs");
