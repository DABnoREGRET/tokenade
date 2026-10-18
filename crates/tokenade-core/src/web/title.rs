//! The page's title, and the echoes of it the body repeats.
//!
//! A converted page routinely carries its own title three times: the `<title>`
//! element, an `og:`/`twitter:` meta block, and an `<h1>` at the top of the
//! body. These passes pick one and drop the echoes.

use super::*;

/// Does the body already open with this header line, so prepending it would
/// say the same thing twice?
///
/// The old test was exact containment, which held while the header came only
/// from `og:title` — that tag usually carries the bare headline. Round 25 added
/// the `<title>` ELEMENT as a fallback, and a `<title>` carries the site name:
/// `First Steps - FastAPI` against an `<h1>` of `First Steps`, so exact
/// containment saw no repeat and 13 pages grew a duplicate heading.
///
/// Compares the title up to its site suffix — the first ` | `, ` - `, ` — ` or
/// ` · ` — which is the part a page actually repeats in its `<h1>`.
pub(super) fn head_zone_repeats_title(head_zone: &str, header_line: &str) -> bool {
    let title = header_line.trim_start_matches('#').trim();
    if title.is_empty() {
        return false;
    }
    if head_zone.contains(title) {
        return true;
    }
    let stem = [" | ", " - ", " — ", " · ", " – "]
        .iter()
        .filter_map(|sep| title.split(sep).next())
        .min_by_key(|s| s.len())
        .unwrap_or(title)
        .trim();
    // Too short a stem is not evidence of anything — `Home | Site` would
    // suppress the header on any page containing the word "Home".
    //
    // NOT normalised for entities/case/punctuation. Tried and REVERTED
    // 2026-07-22: it did suppress ebay.com's duplicate, but by dropping the
    // CLEAN meta title and keeping the page's own double-encoded
    // `Collectibles &amp; More` — a worse line, not a removed one — and the
    // corpus measure of duplicate leading titles did not move (4/60 before and
    // after). The right fix keeps the meta title and drops the repeated BODY
    // heading, which is the converter's call, not this predicate's.
    stem.chars().count() >= 8 && head_zone.contains(stem)
}

/// Comparison key for titles: entities folded, case folded, every non-alphanumeric
/// run collapsed to one space. Never applied to delivered text.
///
/// `char::is_alphanumeric` is Unicode-aware, so Japanese and Thai headings
/// compare on their own characters instead of collapsing to nothing — an
/// ASCII-only key made `mogg ｴﾗｰ` and `ページを表示できません` look identical.
/// `decode_entities` runs because a page can double-encode (ebay.com ships
/// `&amp;amp;`), leaving a literal `&amp;` in the body where the title has `&`.
pub(super) fn title_key(s: &str) -> String {
    decode_entities(s)
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Remove the body's opening heading when it says nothing the page title does
/// not already say.
///
/// Containment is tested in ONE direction — body heading ⊆ title — and that is
/// the whole safety argument: a heading whose words all appear in the title is
/// redundant, so dropping it loses nothing. The reverse would delete real
/// content, and the corpus has the case that proves it: otomoto.pl pairs the
/// title `ERROR: The request could not be satisfied` with an `<h1>` of
/// `403 ERROR`. The status code appears nowhere in the title, so `403 ERROR`
/// stays — a looser rule would have eaten it.
pub(super) fn drop_body_heading_echoing_title(body: &str, title: &str) -> String {
    /// Only the OPENING heading is a restatement of the title; the same words
    /// deeper in the document are a section that happens to share them.
    const SCAN_LINES: usize = 12;
    let tkey = title_key(title);
    if tkey.is_empty() {
        return body.to_string();
    }
    let mut out: Vec<&str> = Vec::with_capacity(body.lines().count());
    let mut dropped = false;
    for (i, line) in body.lines().enumerate() {
        if !dropped && i < SCAN_LINES {
            if let Some(rest) = line.trim_start().strip_prefix('#') {
                let text = rest.trim_start_matches('#').trim();
                let k = title_key(text);
                // A one- or two-character heading is not evidence of a repeat;
                // it would match inside almost any title.
                if k.chars().count() >= 3 && tkey.contains(&k) {
                    dropped = true;
                    continue;
                }
            }
        }
        out.push(line);
    }
    if !dropped {
        return body.to_string();
    }
    // Removing the line can leave the blank that followed it stacked on the one
    // before, which markdown renders as a gap.
    let joined = out.join("\n");
    let mut s = joined.trim_start_matches('\n').to_string();
    while s.contains("\n\n\n") {
        s = s.replace("\n\n\n", "\n\n");
    }
    s
}

/// The `<title>` element's text, entity-decoded and whitespace-collapsed.
///
/// **The document's FIRST `<title>` is not always the page's.** An `<svg>`
/// writes its own: it is the icon's accessible name, "fermer", "play",
/// "search". On a page that declares none in its `<head>`, the delivered
/// title line became `# fermer` — a glyph's name passed off as the article's.
/// None of the 39 corpus pages is in this case (measured 2026-09-10), the
/// shape comes from a hand-written page.
pub(super) fn page_title_element(html: &str) -> Option<String> {
    for c in re(r"(?is)<title\b[^>]*>(.*?)</title>").captures_iter(html) {
        let (Some(whole), Some(inner)) = (c.get(0), c.get(1)) else {
            continue;
        };
        // Inside an `<svg>` opened and not closed before this point: the title
        // belongs to the drawing. `None > None` is false and `Some(_) > None`
        // is true, which is exactly the question being asked.
        let before = &html[..whole.start()];
        if before.rfind("<svg") > before.rfind("</svg>") {
            continue;
        }
        let raw = strip_all_tags(inner.as_str());
        let t = decode_entities(&raw)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if !t.is_empty() {
            return Some(t);
        }
    }
    None
}

/// `# og:title` + `> description` header lines, when the page declares them.
pub(super) fn harvest_meta_header(html: &str) -> Option<String> {
    // `og:title`, then `twitter:title`, then the `<title>` ELEMENT. The last
    // fallback was missing and it costs an article its name:
    // `stackoverflow.blog/2026/07/21/the-future-of-development-is-full-stack/`
    // ships no `og:` tags at all, so the page arrived with its show notes
    // intact and nothing saying what it was — the header emitted the
    // description alone, which on that blog is the same boilerplate sentence
    // about Stack Overflow on every single post.
    let title = meta_content(html, "og:title")
        .or_else(|| meta_content(html, "twitter:title"))
        .or_else(|| page_title_element(html));
    let desc = meta_content(html, "og:description")
        .or_else(|| meta_content(html, "description"))
        .or_else(|| meta_content(html, "twitter:description"));
    match (title, desc) {
        (Some(t), Some(d)) => Some(format!("# {t}\n\n> {d}")),
        (Some(t), None) => Some(format!("# {t}")),
        (None, Some(d)) => Some(format!("> {d}")),
        (None, None) => None,
    }
}
