//! **Which part of the page is the page.**
//!
//! Find the main content, and isolate only if the candidate deserves it: a
//! `<main>` that carries only a minority of the text AND of the headings is a
//! SECTION of a home page, not the article. The thresholds here are all
//! measured, and each one has behind it a keyword that used to decide by
//! coin toss.

use super::*;

/// Boilerplate subtree removal by class/id keywords — the converged
/// trafilatura OVERALL_DISCARD / readability negative-regex / defuddle
/// nonContentPatterns list. Applied AFTER main-content isolation, BEFORE
/// tag conversion. Matches whole words inside class="…"/id="…" on div/
/// section/ul/ol/span containers and drops the matched subtree with a
/// depth-counting scan (regex-only pipeline: no DOM).
/// Does this opening tag NEGATE the boilerplate keyword it matched — a class
/// that suppresses ads rather than being one?
pub(crate) fn negates_the_keyword(tag: &str) -> bool {
    let re_neg = re(
        r"(?i)\b(?:hide|no|non|without|disable|suppress)[-_][a-z0-9_-]*ads?\b|\bads?[-_](?:free|off|hidden|none)\b",
    );
    re_neg.is_match(tag)
}

/// Backstop against a pathological page, not a quality knob: real pages top
/// out in the low hundreds of boilerplate blocks.
pub(super) const MAX_BOILERPLATE_BLOCKS: usize = 4096;

/// Share of the document a single negative-class block may remove before it is
/// treated as a layout container rather than boilerplate.
pub(crate) const MAX_BOILERPLATE_PERCENT: usize = 40;

/// Minimum share of the input that `<main>` isolation must keep for the
/// isolation to be believed.
pub(super) const MIN_ISOLATED_PERCENT: usize = 4;

/// essence's 10%-of-raw-HTML isolation floor (MIT — see module doc), applied
/// only when `<article>` is what matched — see the call site for why `<main>`
/// keeps the lower, corpus-calibrated floor above instead.
pub(super) const ARTICLE_ISOLATED_PERCENT: usize = 10;

/// Minimum share that must survive boilerplate stripping before the whole
/// strip is treated as a misfire on this page.
pub(super) const MIN_KEPT_AFTER_STRIP_PERCENT: usize = 20;

/// Strip an *unclosed* `<tag …>…<EOF>` block (no `</tag>` found). Used as a
/// safety net after `strip_block` so malformed pages (a common case in the
/// wild) don't leak script/style content into the markdown.
pub(super) fn strip_unclosed_block(s: &str, tag: &str) -> String {
    // `<tag\b...` not followed anywhere by `</tag>` — kill to EOF.
    // Conservative: only fires when the OPEN tag exists but the CLOSE doesn't.
    let open = re(&format!(r"(?is)<{t}\b[^>]*>", t = regex::escape(tag)));
    let close = re(&format!(r"(?is)</{t}>", t = regex::escape(tag)));
    if open.is_match(s) && !close.is_match(s) {
        // Strip everything from the first opener to end of string.
        if let Some(m) = open.find(s) {
            return s[..m.start()].to_string();
        }
    }
    s.to_string()
}

/// Strip every `<tag …>…</tag>` block (greedy, case-insensitive, multiline).
pub(super) fn strip_block(s: &str, tag: &str) -> String {
    // ONE scan covering both the self-closing `<tag/>` and the paired
    // `<tag …>…</tag>` forms. Was two separate regexes — two full-string scans
    // and two allocations per call — and strip_block runs ~15 times per page
    // (7 noise tags + 8 chrome tags), so on a megabyte front page the doubled
    // scan was a measurable slice of the isolation cost. The alternation is
    // equivalent: the self-closing branch matches exactly what the old
    // `<tag\b[^>]*/>` did (`[^>]` cannot cross a `>`), the paired branch exactly
    // what `<tag\b[^>]*>.*?</tag>` did.
    let r = re(&format!(
        r"(?is)<{t}\b[^>]*?(?:/>|>.*?</{t}>)",
        t = regex::escape(tag)
    ));
    r.replace_all(s, "").into_owned()
}

/// Try to find `<main>` or `<article>` and return its inner HTML, else None.
/// Everything outside the isolated region, when `<main>` holds a MINORITY of
/// the page's headings.
///
/// Corpus: the share of headings inside `<main>` has median **93 %**; the tail
/// below 50 % is zoom (0/14), vercel (6/30), aljazeera_ar (9/25) and latercera
/// (56/147) — pages whose `<main>` is one panel of a grid. Requires at least 8
/// headings so a short page cannot trip it on a rounding accident.
///
/// The region is stripped independently by the caller, against its own size, so
/// it cannot change the denominator of the isolated region's guards — the
/// coupling that once broke four unrelated pages.
/// `s` with the isolated `<main>`/`<article>` region (byte span `main`) removed,
/// but ONLY when that region holds a minority of the page's headings — the
/// signature of a `<main>` that is one section of a front page rather than the
/// content. `None` otherwise.
pub(super) fn outside_region_if_main_is_a_section(
    s: &str,
    main: std::ops::Range<usize>,
) -> Option<String> {
    const MIN_HEADINGS: usize = 8;
    let (start, end) = (main.start, main.end);
    let re_h = re(r"(?i)<h[1-6]\b");
    let total = re_h.find_iter(s).count();
    if total < MIN_HEADINGS {
        return None;
    }
    if re_h.find_iter(&s[start..end]).count() * 2 >= total {
        return None;
    }
    let mut out = String::with_capacity(s.len() - (end - start));
    out.push_str(&s[..start]);
    out.push_str(&s[end..]);
    Some(out)
}

/// `<article>` elements that lie OUTSIDE the isolated region (byte span `main`).
///
/// `None` when there are none, or when they overlap the isolated region — an
/// `<article>` that WRAPS `<main>` is not a sibling, and appending it duplicates
/// the whole page (docs.stripe.com does exactly that; an earlier version doubled its body
/// from 267 to 535 lines).
pub(super) fn article_siblings(s: &str, main: std::ops::Range<usize>) -> Option<String> {
    let (start, end) = (main.start, main.end);
    let re_art = re(r"(?is)<article\b[^>]*>.*?</article>");
    let mut out = String::new();
    for m in re_art.find_iter(s) {
        if m.start() < end && m.end() > start {
            continue; // overlaps the isolated region
        }
        out.push_str(m.as_str());
    }
    (!out.is_empty()).then_some(out)
}

/// Byte spans of the `<main>`/`<article>` element the isolator selects — the
/// WHOLE element and its INNER content — from ONE regex pass. The isolation, the
/// section-vs-page test and the article-sibling rescue previously each re-ran
/// the same `.*?` regex over the whole (often megabyte-sized) document, three
/// times per page — the dominant cost of the isolation phase on large pages.
/// Returns the tag too, so the caller can size the isolation floor to WHICH
/// selector fired (see [`ARTICLE_ISOLATED_PERCENT`]).
pub(super) fn find_main(
    s: &str,
) -> Option<(&'static str, std::ops::Range<usize>, std::ops::Range<usize>)> {
    for tag in ["main", "article"] {
        let r = re(&format!(
            r"(?is)<{t}\b[^>]*>(.*?)</{t}>",
            t = regex::escape(tag)
        ));
        if let Some(c) = r.captures(s) {
            let whole = c.get(0).unwrap();
            let inner = c.get(1).unwrap();
            return Some((tag, whole.start()..whole.end(), inner.start()..inner.end()));
        }
    }
    None
}

/// essence's main-content selector priority list (MIT — see module doc),
/// consulted ONLY by [`isolate_by_priority_selectors`] as a rescue between
/// "`<main>`/`<article>` isolation failed or wasn't there" and "give up and
/// use the whole document" — never ahead of a main/article isolation that
/// already succeeded, so it cannot change behaviour on any page this file's
/// existing corpus calibration already covers.
///
/// `main article` and `[role='main'] article` from the source list are not
/// included: they are descendant-combinator selectors, and
/// [`extract_by_selector`] (this file's selector engine, also the public
/// `exclude_selectors` API) deliberately supports only a single compound
/// selector — no combinators, see its own doc comment. `[role='main']` alone
/// is likewise not included: attribute selectors aren't a primitive
/// `extract_by_selector` has. Widening a shared, public selector engine to
/// chase three rarely-hit entries was judged not worth the risk to
/// `exclude_selectors` callers outside this file.
pub(super) const MAIN_CONTENT_SELECTORS: [&str; 19] = [
    "#readme",
    ".markdown-body",
    "#mw-content-text",
    ".mw-parser-output",
    ".docs-content",
    ".doc-content",
    ".prose",
    ".article-body",
    ".main-content",
    "#main-content",
    ".content",
    "#content",
    ".post-content",
    ".entry-content",
    ".article-content",
    ".page-content",
    "#inside",
    ".stories",
    ".itemlist",
];

/// First [`MAIN_CONTENT_SELECTORS`] entry that both matches and clears the
/// same isolation floor bare `<main>` isolation uses ([`MIN_ISOLATED_PERCENT`]).
pub(super) fn isolate_by_priority_selectors(noise_stripped: &str) -> Option<String> {
    for sel in MAIN_CONTENT_SELECTORS {
        if let Some(m) = extract_by_selector(noise_stripped, sel) {
            if m.len() * 100 >= noise_stripped.len() * MIN_ISOLATED_PERCENT {
                return Some(m);
            }
        }
    }
    None
}

/// Escape a literal `>` that sits inside a quoted attribute value.
///
/// The tag matchers in this module are all `<tag\b[^>]*>`; a `>` inside a
/// quoted value (legal HTML5, and produced by Tailwind/shadcn arbitrary
/// variants such as `[&>a]:hidden` or `data-[state=open]>svg`) ends the match
/// early and leaks the attribute tail as body text. Called after the
/// script/style/comment strips, so every `<…>` remaining is an element tag.
///
/// A `<` not followed by a tag-name char is left as literal text (the HTML5
/// tokenizer rule), so a stray `<` in prose does not swallow the rest of the
/// document. Only whole ASCII bytes are substituted and every other byte is
/// copied verbatim, so the result stays valid UTF-8.
pub(super) fn escape_stray_gt_in_tags(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len() + 8);
    let mut i = 0;
    while i < b.len() {
        let starts_tag = b[i] == b'<'
            && b.get(i + 1)
                .is_some_and(|&n| n.is_ascii_alphabetic() || matches!(n, b'/' | b'!' | b'?'));
        if !starts_tag {
            out.push(b[i]);
            i += 1;
            continue;
        }
        out.push(b'<');
        i += 1;
        // A quote opens a value ONLY in the attribute-value position — right
        // after `=` (whitespace allowed between). A `"` anywhere else in a tag
        // is a stray char, not a delimiter; treating it as one lets a dangling
        // quote (`title="x" ">`, seen on newegg.com) open a phantom value that
        // swallows the real `>` and every tag after it. This mirrors the HTML5
        // tokenizer's before-attribute-name vs before-attribute-value states.
        let mut quote = 0u8;
        let mut expect_value = false;
        while i < b.len() {
            let c = b[i];
            if quote != 0 {
                if c == quote {
                    quote = 0;
                    out.push(c);
                } else if c == b'>' {
                    out.extend_from_slice(b"&gt;");
                } else {
                    out.push(c);
                }
            } else if c == b'>' {
                out.push(b'>');
                i += 1;
                break;
            } else if (c == b'"' || c == b'\'') && expect_value {
                quote = c;
                expect_value = false;
                out.push(c);
            } else if c == b'=' {
                expect_value = true;
                out.push(c);
            } else if c.is_ascii_whitespace() {
                out.push(c);
            } else {
                expect_value = false;
                out.push(c);
            }
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// Rename `<header>`/`<footer>` to `<div>` when they sit inside an
/// `<article>`/`<section>`, so the page-chrome strip cannot delete an item's own
/// title. HTML5 scopes both tags to their nearest sectioning ancestor; the strip
/// that follows only knows tag names, so the scoping has to be resolved here.
///
/// A single linear scan tracking sectioning depth — no regex, because the thing
/// being decided (ancestry) is exactly what a regex cannot see.
pub(super) fn descope_item_chrome(s: &str) -> String {
    // Nothing to do on the overwhelming majority of pages.
    if !s.contains("<article") && !s.contains("<section") {
        return s.to_string();
    }
    let b = s.as_bytes();
    // Bytes in, bytes out: emitting through `b as char` reinterpreted every
    // byte above 0x7F as its Latin-1 codepoint, so one string literal or one
    // comment marker on a line was enough to hand back mojibake for every
    // non-ASCII character on it. The byte scan itself is sound - UTF-8 is
    // self-synchronising and every delimiter here is ASCII.
    let mut out = Vec::with_capacity(s.len());
    let mut depth: usize = 0;
    let mut i = 0;
    // Tag name starting at `at`, plus whether it closes. `None` when this `<` is
    // not a tag opener (a stray `<` in text).
    let read = |at: usize| -> Option<(String, bool, usize)> {
        let close = b.get(at + 1) == Some(&b'/');
        let mut j = at + 1 + usize::from(close);
        let start = j;
        while j < b.len() && (b[j].is_ascii_alphanumeric()) {
            j += 1;
        }
        if j == start {
            return None;
        }
        Some((s[start..j].to_ascii_lowercase(), close, j))
    };
    while i < b.len() {
        if b[i] != b'<' {
            out.push(b[i]);
            i += 1;
            continue;
        }
        match read(i) {
            Some((name, close, after)) => {
                match name.as_str() {
                    "article" | "section" => {
                        if close {
                            depth = depth.saturating_sub(1);
                        } else {
                            depth += 1;
                        }
                        out.extend_from_slice(&b[i..after]);
                    }
                    // Inside a sectioning element this is the ITEM's header —
                    // keep it, stripped of its identity so the page-chrome pass
                    // walks past it.
                    "header" | "footer" if depth > 0 => {
                        out.extend_from_slice(if close { b"</div" } else { b"<div" });
                    }
                    _ => out.extend_from_slice(&b[i..after]),
                }
                i = after;
            }
            None => {
                out.push(b'<');
                i += 1;
            }
        }
    }
    crate::text_encoding::utf8_owned(out)
}

/// `- x`, `* x`, `+ x`, `12. x`, `3) x`.
fn is_list_item_line(body: &str) -> bool {
    let digits = body.bytes().take_while(u8::is_ascii_digit).count();
    let rest = &body[digits..];
    if digits > 0 {
        return rest.starts_with(". ") || rest.starts_with(") ");
    }
    body.starts_with("- ") || body.starts_with("* ") || body.starts_with("+ ")
}

pub(super) fn collapse_whitespace(s: &str) -> String {
    // Trim each line; collapse runs of blank lines to at most one.
    //
    // A LIST line keeps its leading indentation: it is the nesting. Squeezed
    // to one space, `  - Debian` under `- Linux` became ` - Debian`, which
    // CommonMark reads as a SIBLING of Linux — the sub-list was flattened on
    // every page. Other lines are squeezed as before: indentation inherited
    // from the HTML source would otherwise leak in, and four spaces open a
    // code block.
    let mut out = String::with_capacity(s.len());
    let mut blank_run = 0;
    // Where the content of the last list item starts (indent + marker): a
    // paragraph after a blank line, indented EXACTLY there, is that item's
    // continuation (`1. Step one` / blank / `   Step one, part two`) and keeps
    // its indent — squeezed to one space it left the list. Same for a line
    // right under the item (a `<br>` in it): CommonMark reads both as the
    // item's, a blank line or not.
    let mut item_content_at: Option<usize> = None;
    for line in s.lines() {
        let body = line.trim_start_matches([' ', '\t']);
        let lead = line.len() - body.len();
        let continuation = !body.is_empty() && item_content_at == Some(lead);
        if is_list_item_line(body) {
            let digits = body.bytes().take_while(u8::is_ascii_digit).count();
            item_content_at = Some(lead + digits + 2);
        } else if !body.is_empty() && !continuation {
            item_content_at = None;
        }
        let squeezed = if is_list_item_line(body) || continuation {
            format!(
                "{}{}",
                &line[..line.len() - body.len()],
                re(r"[ \t]+").replace_all(body, " ")
            )
        } else {
            re(r"[ \t]+").replace_all(line, " ").into_owned()
        };
        let l = squeezed.trim_end();
        if l.is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                out.push('\n');
            }
        } else {
            blank_run = 0;
            out.push_str(l);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod a_nested_list_keeps_its_nesting {
    use super::collapse_whitespace;

    /// The squeeze took `  - Debian` to ` - Debian`, which CommonMark reads
    /// as a sibling of `- Linux`; a prose line still loses its stray indent.
    #[test]
    fn list_indentation_survives_and_other_indentation_does_not() {
        let out = collapse_whitespace(
            "- Linux\n  - Debian  and   co\n    2. second\n      stray   prose\n",
        );
        assert_eq!(
            out,
            "- Linux\n  - Debian and co\n    2. second\n stray prose\n"
        );
    }

    /// A paragraph that continues an item, after a blank line and at exactly
    /// the item's content column, keeps that column; anywhere else it doesn't.
    #[test]
    fn an_item_continuation_keeps_its_column() {
        let out = collapse_whitespace(
            "1. Step one\n\n   Step one, part two\n2. Step two\n\n      not at the column\n",
        );
        assert_eq!(
            out,
            "1. Step one\n\n   Step one, part two\n2. Step two\n\n not at the column\n"
        );
    }
}
