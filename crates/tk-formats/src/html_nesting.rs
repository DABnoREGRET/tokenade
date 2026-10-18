//! A nesting ceiling applied to raw HTML, before it reaches the parser.
//!
//! ## Why this exists
//!
//! HTML tree construction is QUADRATIC in nesting depth, and that is the
//! specification's own shape, not a bug in the parser we use. Handling an end
//! tag runs "has an element in scope", which walks the stack of open elements;
//! at depth D each of the D end tags walks O(D), so the parse costs O(D²).
//!
//! MEASURED 2026-07-22 on a document that is nothing but nested `<div>`, in
//! release, timing `Html::parse_document` alone:
//!
//! | nesting | parse |
//! |---|---|
//! | 1 500 | 12 ms |
//! | 3 000 | 46 ms |
//! | 6 000 | 185 ms |
//! | 12 000 | 755 ms |
//!
//! Doubling the depth quadruples the time. The 12 000-level document is 132 kB
//! — so roughly 2 MB of `<div>` buys minutes of CPU from a single fetch. We
//! fetch URLs an agent chose and cannot vouch for, which makes that a denial of
//! service rather than a curiosity. Our own passes are NOT implicated: the same
//! measurement shows `tree_dedup` at 0 ms, because its depth guard bounds it.
//!
//! ## Why a ceiling is the right answer
//!
//! Browsers solve this the same way — Blink and Gecko both refuse to nest the
//! parser past a fixed limit — because past a few hundred levels no document is
//! describing structure any more. The deepest page in our fidelity corpus is 34
//! levels, and [`crate::web_tree`] already declines to WALK past 200, so
//! everything this module touches is already beyond what we would render.
//!
//! ## What it does, and what it deliberately does not
//!
//! Elements opened beyond the ceiling are dropped, along with their matching end
//! tags. TEXT IS NEVER DROPPED: over-deep content is re-parented, not discarded,
//! so the flattening cannot lose anything an agent would have read.
//!
//! On any document within the ceiling — every real page — this is a single O(n)
//! scan that returns the input borrowed, with no allocation and no rewriting.
//! That is the property that makes it safe to run unconditionally: the rewrite
//! path is unreachable for documents a browser would render as written.
//!
//! It is NOT an HTML parser and must never grow into one. It knows only what it
//! takes to count depth honestly: comments, doctypes, void and self-closing
//! elements, and the raw-text elements whose contents are not markup. Anything
//! it cannot make sense of is passed through untouched, because on this path a
//! false positive rewrites a real page and a false negative merely leaves a
//! pathological document slow.

use std::borrow::Cow;

/// How deep the parser is allowed to nest. Chosen to sit far above any real
/// document (deepest measured page: 34) and above the walk's own 200-level
/// refusal, so reaching it already means the document is not being rendered.
const MAX_NESTING: usize = 512;

/// How far back to look for the element an end tag closes. A linear search of
/// the whole stack would reintroduce the very quadratic this module exists to
/// remove; in a well-formed document the match is at the top, and a document
/// that interleaves its tags more than this deeply is malformed enough that
/// passing the tag through unchanged is the honest response.
const CLOSE_LOOKBACK: usize = 64;

/// Elements with no end tag. Counting them as open would inflate depth without
/// bound on a page of `<br>` or `<img>`.
/// **HTML's VOID elements, once for this repo.**
///
/// `pub` because there were TWO: `markdown::inline_html` carried a
/// second one, hand-written, with eight names out of fourteen — `area`, `base`,
/// `col`, `embed`, `param` and `track` were missing, so the inline rendering
/// waited for a closing tag on elements that never have one.
/// The specification has only one list; this repo now has only one too.
pub const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// Elements whose content is text, not markup. A `<script>` writing
/// `"<div>"` into the page must not count as an open element — that is the
/// mistake that would make this rewrite a real page.
const RAW_TEXT: &[&str] = &["script", "style", "textarea", "title"];

/// Cap nesting depth, borrowing the input unchanged when it is already within
/// the ceiling.
pub fn limit_nesting(html: &str) -> Cow<'_, str> {
    // Scan first and rewrite only if we must, so the common path allocates
    // nothing. The scan is the same walk as the rewrite, minus the output.
    if !exceeds_ceiling(html) {
        return Cow::Borrowed(html);
    }
    Cow::Owned(rewrite(html))
}

/// One open element: its name, and whether we emitted it. A suppressed element
/// stays on the stack so its end tag can be suppressed with it.
///
/// Nothing constructs it any more — the rewriter above tracks the same state in
/// a tuple. Kept because it names the two things that stack entry IS, which the
/// tuple does not; annotated so it stops costing a warning on every build.
#[allow(dead_code)]
struct Open<'a> {
    name: &'a str,
    emitted: bool,
}

/// What the scanner found at a `<`.
enum Tag<'a> {
    /// A comment, doctype or processing instruction — copied, never counted.
    Opaque,
    Open {
        name: &'a str,
        void: bool,
        raw_text: bool,
    },
    Close {
        name: &'a str,
    },
    /// A `<` that does not begin a tag, e.g. `a < b`.
    Literal,
}

/// Parse the tag beginning at `html[at]` (which must be `<`). Returns the tag
/// and the offset just past it.
fn tag_at(html: &str, at: usize) -> (Tag<'_>, usize) {
    let bytes = html.as_bytes();
    let rest = &html[at..];
    if rest.starts_with("<!--") {
        let end = html[at + 4..]
            .find("-->")
            .map(|i| at + 4 + i + 3)
            .unwrap_or(html.len());
        return (Tag::Opaque, end);
    }
    if rest.starts_with("<!") || rest.starts_with("<?") {
        let end = html[at..]
            .find('>')
            .map(|i| at + i + 1)
            .unwrap_or(html.len());
        return (Tag::Opaque, end);
    }
    let closing = rest.starts_with("</");
    let name_start = at + if closing { 2 } else { 1 };
    if name_start >= html.len() || !bytes[name_start].is_ascii_alphabetic() {
        return (Tag::Literal, at + 1);
    }
    let mut i = name_start;
    while i < html.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-') {
        i += 1;
    }
    let name = &html[name_start..i];
    // Skip attributes to the closing `>`, respecting quotes so a `>` inside an
    // attribute value does not end the tag early.
    let mut quote: Option<u8> = None;
    let mut last_solidus = false;
    while i < html.len() {
        let c = bytes[i];
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == b'"' || c == b'\'' => quote = Some(c),
            None if c == b'>' => {
                i += 1;
                break;
            }
            None => last_solidus = c == b'/',
        }
        if !matches!(c, b'/') && quote.is_none() && c != b'>' {
            last_solidus = false;
        }
        i += 1;
    }
    if closing {
        return (Tag::Close { name }, i);
    }
    let lower = name.to_ascii_lowercase();
    (
        Tag::Open {
            name,
            void: last_solidus || VOID.contains(&lower.as_str()),
            raw_text: RAW_TEXT.contains(&lower.as_str()),
        },
        i,
    )
}

/// Find the end of a raw-text element's content: the offset of its `</name`.
fn raw_text_end(html: &str, from: usize, name: &str) -> usize {
    let needle = format!("</{}", name.to_ascii_lowercase());
    let hay = html[from..].to_ascii_lowercase();
    hay.find(&needle).map(|i| from + i).unwrap_or(html.len())
}

/// Walk the tags, calling `step` with each depth change. Shared by the scan and
/// the rewrite so the two can never disagree about what the depth is.
fn walk_tags<'a>(html: &'a str, mut step: impl FnMut(usize, &Tag<'a>, usize, usize)) {
    let mut i = 0usize;
    let mut stack: Vec<&str> = Vec::new();
    while let Some(rel) = html[i..].find('<') {
        let at = i + rel;
        let (tag, end) = tag_at(html, at);
        match &tag {
            Tag::Open {
                name,
                void,
                raw_text,
            } => {
                step(stack.len(), &tag, at, end);
                if !void {
                    stack.push(name);
                }
                if *raw_text {
                    i = raw_text_end(html, end, name);
                    continue;
                }
            }
            Tag::Close { name } => {
                let hit = stack
                    .iter()
                    .rev()
                    .take(CLOSE_LOOKBACK)
                    .position(|o| o.eq_ignore_ascii_case(name));
                if let Some(back) = hit {
                    stack.truncate(stack.len() - back - 1);
                }
                step(stack.len(), &tag, at, end);
            }
            _ => step(stack.len(), &tag, at, end),
        }
        i = end;
    }
}

/// Does any element open beyond the ceiling? Pure scan, no allocation.
fn exceeds_ceiling(html: &str) -> bool {
    // Cheap reject: a document needs at least one tag per level, so it cannot
    // reach the ceiling without at least that many `<`. Skips the walk entirely
    // for short pages.
    if html.len() < MAX_NESTING {
        return false;
    }
    let mut over = false;
    walk_tags(html, |depth, tag, _, _| {
        if matches!(tag, Tag::Open { .. }) && depth >= MAX_NESTING {
            over = true;
        }
    });
    over
}

/// Rebuild the document with over-deep elements dropped, text preserved.
fn rewrite(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut copied = 0usize;
    // Which still-open elements were suppressed, so their end tags go too.
    let mut suppressed: Vec<bool> = Vec::new();
    walk_tags(html, |depth, tag, at, end| {
        // Everything between the previous tag and this one is text.
        out.push_str(&html[copied..at]);
        copied = end;
        match tag {
            Tag::Open { void, .. } => {
                let drop = depth >= MAX_NESTING;
                if !void {
                    suppressed.push(drop);
                }
                if !drop {
                    out.push_str(&html[at..end]);
                }
            }
            Tag::Close { .. } => {
                // `walk_tags` has already popped the stack to `depth`, so the
                // element this closes is the one recorded just above it.
                let drop = suppressed.len() > depth && suppressed[depth];
                suppressed.truncate(depth);
                if !drop {
                    out.push_str(&html[at..end]);
                }
            }
            _ => out.push_str(&html[at..end]),
        }
    });
    out.push_str(&html[copied..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nested(levels: usize, inner: &str) -> String {
        format!(
            "<html><body>{}{inner}{}</body></html>",
            "<div>".repeat(levels),
            "</div>".repeat(levels)
        )
    }

    /// The property that makes this safe to run on every page: a document within
    /// the ceiling is not rewritten, not copied, not touched.
    #[test]
    fn a_document_within_the_ceiling_is_borrowed_untouched() {
        for (n, html) in [
            (
                "plain",
                "<html><body><p>bonjour</p></body></html>".to_string(),
            ),
            ("real-deepest", nested(34, "<p>deepest real page</p>")),
            // `nested` adds three levels of its own — `html`, `body`, and the
            // inner `<p>` — so the deepest element here opens at
            // MAX_NESTING - 2. Writing MAX_NESTING - 2 divs instead would put
            // that `<p>` exactly AT the ceiling and this case would (correctly)
            // be rewritten.
            ("just-under", nested(MAX_NESTING - 4, "<p>still under</p>")),
        ] {
            let mut depth = 0usize;
            walk_tags(&html, |d, t, _, _| {
                if matches!(t, Tag::Open { .. }) {
                    depth = depth.max(d);
                }
            });
            assert!(
                matches!(limit_nesting(&html), Cow::Borrowed(_)),
                "{n}: rewrote a document within the ceiling (max depth seen {depth}, ceiling {MAX_NESTING})"
            );
        }
    }

    /// The whole point: text survives the flattening. Dropping the over-deep
    /// wrappers must never drop what they wrapped.
    #[test]
    fn over_deep_nesting_is_flattened_and_the_text_survives() {
        let html = nested(4_000, "<p>BURIED</p>");
        let out = limit_nesting(&html);
        assert!(matches!(out, Cow::Owned(_)), "should have been rewritten");
        assert!(out.contains("BURIED"), "flattening lost the content");
        assert!(
            out.matches("<div>").count() <= MAX_NESTING,
            "still {} divs deep",
            out.matches("<div>").count()
        );
        // Opens and closes stay balanced, or the parser inherits a mess.
        assert_eq!(out.matches("<div>").count(), out.matches("</div>").count());
    }

    /// A `<script>` that writes markup into the page is text, not structure.
    /// Counting it would rewrite real pages, which is the failure this module
    /// most needs to avoid.
    #[test]
    fn markup_inside_raw_text_elements_is_not_counted_as_depth() {
        let bomb = "<div>".repeat(2_000);
        let html =
            format!("<html><body><script>var s = \"{bomb}\";</script><p>x</p></body></html>");
        assert!(
            matches!(limit_nesting(&html), Cow::Borrowed(_)),
            "script contents counted as nesting"
        );
    }

    /// Void elements never close, so counting them as open would make any page
    /// with enough `<br>` look pathological.
    #[test]
    fn void_and_self_closing_elements_do_not_accumulate_depth() {
        let html = format!(
            "<html><body>{}</body></html>",
            "<br><img src=x>".repeat(2_000)
        );
        assert!(matches!(limit_nesting(&html), Cow::Borrowed(_)));
        let html = format!("<html><body>{}</body></html>", "<span/>".repeat(2_000));
        assert!(matches!(limit_nesting(&html), Cow::Borrowed(_)));
    }

    /// A `>` inside an attribute value does not end the tag, and a bare `<` in
    /// prose is not a tag at all.
    #[test]
    fn attribute_values_and_bare_angle_brackets_do_not_confuse_the_scan() {
        let html = "<html><body><p title=\"a > b\">a < b</p></body></html>";
        assert!(matches!(limit_nesting(html), Cow::Borrowed(_)));
        assert_eq!(limit_nesting(html), html);
    }

    /// Comments and doctypes carry no structure and must not be counted.
    #[test]
    fn comments_and_doctypes_are_opaque() {
        let html = format!(
            "<!doctype html><!-- {} --><html><body><p>x</p></body></html>",
            "<div>".repeat(2_000)
        );
        assert!(matches!(limit_nesting(&html), Cow::Borrowed(_)));
    }
}
