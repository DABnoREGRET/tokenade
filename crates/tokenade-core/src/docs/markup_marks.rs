//! **Rendering as markdown what the markup DECLARES.**
//!
//! A heading, a list, a link: the document says it in its styles, not in
//! its text. Translating them is what separates a readable document from a wall of
//! paragraphs — and not translating them shows up in no byte
//! counter.

use super::*;

mod lists;
mod numbering;
pub(super) use lists::*;
pub(super) use numbering::*;

// ---- XML → text -----------------------------------------------------------

/// The compiled-pattern cache — the one in `regexes`, not a second one.
///
/// This file carried a COPY of it, word for word, including the comment of
/// the other that explains why there must be only one. Two caches
/// are two tables and two locks for the same pattern, and the only difference
/// between the two bodies was an `unwrap()` on the poisoned lock where
/// the other continues.
pub(super) fn re(p: &str) -> &'static Regex {
    crate::regexes::re(p)
}

/// Generic Office-XML → text: insert newlines around block-ish elements,
/// strip every tag, then squeeze whitespace.
/// Mark up headings as markdown BEFORE the tag stripper runs.
///
/// A document's own headings are the only free statement of its structure, and
/// everything downstream wants them: retrieval scopes to a section, the reader
/// sees an outline instead of a wall. The stripper deletes the tags that carry
/// that fact, so it has to happen here.
///
/// Three vocabularies, one job:
/// * OOXML `<w:p>` whose `<w:pPr>` names a heading style. The style name is
///   LOCALISED — French Word writes `Titre1`, German `berschrift1` — and 29 of
///   the 260 heading styles in our own corpus are `Titre*`, so matching only
///   `Heading*` would silently lose French documents. `outlineLvl` is the
///   language-independent fallback for custom styles.
/// * ODF `<text:h text:outline-level="N">` — already explicit.
/// * HTML `<h1>`..`<h6>` (EPUB chapters).
/// **A list lost its list status.** Three numbered steps of a
/// procedure arrived as three paragraphs, impossible to tell apart from the
/// sentence that introduces them — neither their number, nor where the list ends, nor
/// the order when it matters.
///
/// The marker is set IN the XML, before the tags are stripped, as
/// `mark_headings` sets its hashes. An ordered list returns the index the
/// document carries, never a recomputed index: renumbering would be fabrication,
/// and `text:start-value` like `w:start` are read when they are there.
///
/// `ordered` is the list of identifiers whose format declares a
/// NUMBERING. It comes from elsewhere in each schema — from another file of the
/// package for Word, from the automatic styles of the same file for OpenDocument —
/// so it is passed rather than guessed.
/// **A link's target went away with the tag.** « See the guide before
/// starting »: the guide's address was nowhere, and no reader can
/// find it again. The web route has always rendered its links; the documents route
/// threw them away, as it used to throw away an image's replacement text.
///
/// **Only links that carry a SCHEME are rendered.** An internal anchor
/// (`#section-3`) or a reference from one epub chapter to another
/// (`chapitre4.xhtml`) leads nowhere for whoever reads the delivered text: rendering it
/// adds noise and no information. And when the link text IS its
/// address, it is not written twice.
pub(super) fn mark_links(
    xml: &str,
    schema: Schema,
    rels: &std::collections::HashMap<String, String>,
) -> String {
    let wrap = |target: &str, inner: &str| -> String {
        if !has_scheme(target) {
            return inner.to_string();
        }
        format!("[{inner}]({target})")
    };
    let out = match schema {
        Schema::Odf => re(r#"(?s)<text:a\b[^>]*\bxlink:href="([^"]*)"[^>]*>(.*?)</text:a>"#)
            .replace_all(xml, |c: &regex::Captures| {
                wrap(&decode_entities(&c[1]), &c[2])
            })
            .into_owned(),
        Schema::Html => re(r#"(?s)<a\b[^>]*\bhref="([^"]*)"[^>]*>(.*?)</a>"#)
            .replace_all(xml, |c: &regex::Captures| {
                wrap(&decode_entities(&c[1]), &c[2])
            })
            .into_owned(),
        // Word does not put the address in the document: the link cites a relationship
        // (`r:id="rId7"`) and the target lives in another part of the package.
        Schema::Ooxml => re(r#"(?s)<w:hyperlink\b([^>]*)>(.*?)</w:hyperlink>"#)
            .replace_all(xml, |c: &regex::Captures| {
                let target = re(r#"\br:id="([^"]*)""#)
                    .captures(&c[1])
                    .and_then(|m| rels.get(&m[1]).cloned())
                    .unwrap_or_default();
                wrap(&target, &c[2])
            })
            .into_owned(),
    };
    // PowerPoint (DrawingML) sets the link on the RUN: `<a:r><a:rPr><a:hlinkClick
    // r:id="rId2"/></a:rPr><a:t>…</a:t></a:r>`, target in the slide's
    // relationships. Without it, `Visit our site today.` lost the address that the
    // `.odp` of the same slideshow keeps (2026-10-04).
    if schema != Schema::Ooxml || !out.contains("<a:hlinkClick") {
        return out;
    }
    // A run does not nest: `<a:r>…</a:r>` bounds it, and its link is in
    // ITS `a:rPr` — a pattern looking for the end of `a:rPr` crossed a
    // self-closing `a:rPr` to the next run, and swallowed `Visit `.
    re(r#"(?s)<a:r>(.*?)</a:r>"#)
        .replace_all(&out, |c: &regex::Captures| {
            let run = &c[1];
            let props = re(r#"(?s)^<a:rPr\b(?:[^>]*?/>|[^>]*>.*?</a:rPr>)"#)
                .find(run)
                .map_or("", |m| m.as_str());
            let target = re(r#"<a:hlinkClick\b[^>]*\br:id="([^"]*)""#)
                .captures(props)
                .and_then(|m| rels.get(&m[1]).cloned())
                .unwrap_or_default();
            if target.is_empty() {
                return c[0].to_string();
            }
            format!("<a:r>{}</a:r>", wrap(&target, &run[props.len()..]))
        })
        .into_owned()
}

/// A link is rendered only if it leads SOMEWHERE for the reader of the delivered
/// text. `https:`, `mailto:`, `ftp:` yes; `#anchor` and `chapitre4.xhtml` no.
pub(super) fn has_scheme(target: &str) -> bool {
    let Some(colon) = target.find(':') else {
        return false;
    };
    let scheme = &target[..colon];
    !scheme.is_empty()
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// `rId…` → target, from the relationships of a Word part.
pub(super) fn ooxml_link_targets(rels_xml: &str) -> std::collections::HashMap<String, String> {
    re(r#"(?s)<Relationship\b[^>]*\bId="([^"]*)"[^>]*\bTarget="([^"]*)""#)
        .captures_iter(rels_xml)
        .filter_map(|c| {
            Some((
                c.get(1)?.as_str().to_string(),
                decode_entities(c.get(2)?.as_str()),
            ))
        })
        .collect()
}

pub(super) fn mark_headings(xml: &str) -> String {
    // `w:p` blocks: look inside for a heading style, emit "## " if found.
    let para = re(r"(?s)<w:p\b[^>]*>(.*?)</w:p>");
    let style = re(r#"<w:pStyle\s+w:val="([^"]+)""#);
    let outline = re(r#"<w:outlineLvl\s+w:val="(\d+)""#);
    let out = para.replace_all(xml, |c: &regex::Captures| {
        let body = &c[1];
        let lvl = style
            .captures(body)
            .and_then(|m| heading_level_from_style(&m[1]))
            .or_else(|| {
                outline
                    .captures(body)
                    .and_then(|m| m[1].parse::<usize>().ok())
                    .map(|n| n + 1)
            });
        match lvl {
            Some(n) => format!("<w:p>{}{}</w:p>", md_hashes(n), body),
            None => c[0].to_string(),
        }
    });
    let odf = re(r#"(?s)(<text:h\b[^>]*text:outline-level="(\d+)"[^>]*>)"#);
    let out = odf.replace_all(&out, |c: &regex::Captures| {
        let n: usize = c[2].parse().unwrap_or(1);
        format!("{}{}", &c[1], md_hashes(n))
    });
    let out = odf_styled_headings(&out);
    let html = re(r"(?s)(<h([1-6])\b[^>]*>)");
    html.replace_all(&out, |c: &regex::Captures| {
        let n: usize = c[2].parse().unwrap_or(1);
        format!("{}{}", &c[1], md_hashes(n))
    })
    .into_owned()
}

/// An OpenDocument PARAGRAPH whose style is a heading style: the document
/// title is `<text:p text:style-name="Title">`, not a `text:h` — a `.docx`
/// converted to `.odt` lost its title's mark (2026-10-04). Direct formatting
/// puts an automatic style (`P1`) between the paragraph and the style it
/// derives from, so the parent's name counts too.
fn odf_styled_headings(xml: &str) -> std::borrow::Cow<'_, str> {
    if !xml.contains("<text:p ") {
        return std::borrow::Cow::Borrowed(xml);
    }
    let decl = re(r#"<style:style\b[^>]*>"#);
    let name = re(r#"\bstyle:name="([^"]+)""#);
    let parent = re(r#"\bstyle:parent-style-name="([^"]+)""#);
    let mut parents = std::collections::HashMap::new();
    for d in decl.find_iter(xml) {
        if let (Some(n), Some(p)) = (name.captures(d.as_str()), parent.captures(d.as_str())) {
            parents.insert(n[1].to_string(), p[1].to_string());
        }
    }
    // `_20_` is how ODF writes a space in a style name: `Heading_20_1`.
    let level = |style: &str| heading_level_from_style(&style.replace("_20_", " "));
    re(r#"(<text:p\b[^>]*\btext:style-name="([^"]+)"[^>]*>)"#).replace_all(
        xml,
        |c: &regex::Captures| match level(&c[2])
            .or_else(|| parents.get(&c[2]).and_then(|p| level(p)))
        {
            Some(n) => format!("{}{}", &c[1], md_hashes(n)),
            None => c[0].to_string(),
        },
    )
}

/// Markdown hashes for an outline level, capped at h6 like HTML.
pub(super) fn md_hashes(level: usize) -> String {
    format!("{} ", "#".repeat(level.clamp(1, 6)))
}

/// Outline level of a Word paragraph style name, or `None` if it is not a
/// heading. Word localises these, so match the known prefixes in the languages
/// our corpus actually contains rather than the English one only.
pub(super) fn heading_level_from_style(name: &str) -> Option<usize> {
    let l = name.to_lowercase();
    let stem = [
        "heading",    // en
        "title",      // en, the document's title
        "titel",      // de/nl/sv, the document's title
        "titre",      // fr
        "berschrift", // de ("Überschrift", often written without the umlaut)
        "ttulo",      // es/pt ("Título")
        "titolo",     // it
        "kop",        // nl
        "rubrik",     // sv
        "nagwek",     // pl
    ]
    .iter()
    .find(|s| l.starts_with(**s))?;
    let rest = l[stem.len()..].trim_matches(|c: char| c == '-' || c == '_' || c == ' ');
    if rest.is_empty() {
        return Some(1);
    }
    // "Heading2" → 2. A style like "HeadingChar" (a character style, not a
    // paragraph one) has no digits and must not become a heading.
    rest.parse::<usize>().ok().filter(|n| (1..=9).contains(n))
}

pub(super) fn xml_to_text(xml: &str) -> String {
    let xml = &mark_headings(xml);
    // **One break per list item, and one before a sub-list.** `</p>` and
    // `</li>` both break, so `<li><p>B</p></li>` left a blank line after every
    // item of a nested list, and `</li></ol></li>` one after the last; while
    // `<li>A<ol>` had no break at all and read `1. A  1. B` (2026-10-03,
    // LibreOffice and hand-written EPUB). HTML names only: `w:p`, `text:p`
    // and their empty paragraphs are untouched.
    let xml = &re(r"(?i)</p>(\s*</li>)").replace_all(xml, "$1");
    let xml = &re(r"(?i)</li>((?:\s*</(?:ol|ul)>)+\s*</li>)").replace_all(xml, "$1");
    let xml = &re(r"(?i)([^>\s])(\s*)<(ol|ul)\b").replace_all(xml, "$1\n$2<$3");
    // **A break or a tab INSIDE a paragraph is an empty element**, and the tag
    // stripper below erased it: `Name<w:tab/>Value<w:br/>Second line` read
    // `NameValueSecond line` — words the document does not contain.
    // Each schema writes its own: Word `w:br`/`w:cr`/`w:tab`, PowerPoint `a:br`,
    // OpenDocument `text:line-break`/`text:tab`/`text:s` (a run of spaces).
    // A Word `w:tab` WITH attributes is a tab STOP declared in `w:tabs`, not a
    // tab: only the bare element is one.
    let xml = &re(r"<(?:w:br|w:cr|a:br|text:line-break)\b[^>]*/>").replace_all(xml, "\n");
    // HTML (an EPUB chapter) writes `<br/>` — or `<br>` — inside a run:
    // `Line one<span><br/></span>line two` read `Line oneline two`.
    let xml = &re(r"(?i)<br\b[^>]*>").replace_all(xml, "\n");
    let xml = &re(r"<(?:w:tab|text:tab)\s*/>").replace_all(xml, "\t");
    let xml = &re(r"<text:s\b[^>]*/>").replace_all(xml, " ");
    let xml = &raised_and_lowered(xml);
    // Insert a newline after common paragraph/break markers from each schema.
    let breaks = re(r"</(w:p|w:br|text:p|text:h|a:p|p|h[1-6]|li|br|tr|row|sheetData|sheet)>");
    let s = breaks.replace_all(xml, "\n").into_owned();
    // **A text box is a block, anchored IN a paragraph.** Its own paragraphs
    // close with a break, but nothing opened one: `Second paragraph.<draw:
    // custom-shape><text:p>Box` read `Second paragraph.Box` — two blocks
    // welded into one word (`.odt`, 2026-10-04; the `.docx` got two spaces).
    // A break before it, only where text precedes it on the line: between
    // the frames of a slide it would add blank lines.
    let s = text_boxes_on_their_own_line(&s);
    // Strip remaining XML tags entirely.
    let tag = re(r"(?s)<[^>]+>");
    let s = tag.replace_all(&s, "").into_owned();
    decode_entities(&collapse_whitespace(&s))
}

fn text_boxes_on_their_own_line(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    for m in re(r"<(?:w:txbxContent|draw:text-box|draw:custom-shape)\b").find_iter(s) {
        out.push_str(&s[last..m.start()]);
        let line = &out[out.rfind('\n').map_or(0, |i| i + 1)..];
        if !re(r"<[^>]*>").replace_all(line, "").trim().is_empty() {
            out.push('\n');
        }
        last = m.start();
    }
    out.push_str(&s[last..]);
    out
}

/// **A raised or lowered figure is not a digit of the word before it.** A note
/// call `1648<sup>1</sup>` read `16481` — a year the book never printed — and
/// `10<sup>6</sup>` read `106`. Same rendering as the web reader
/// (`web_tree::embeds::raised_or_lowered`): `1648^1`, `10^6`, `H_2O`; letters
/// alone (`1<sup>st</sup>`) stay as they are. HTML/XHTML `<sup>`/`<sub>`, and
/// Word's run property `<w:vertAlign w:val="superscript|subscript"/>`.
fn raised_and_lowered(xml: &str) -> String {
    let mark = |raised: bool, inner: &str| {
        let t = re(r"(?s)<[^>]+>").replace_all(inner, "");
        let t = t.trim();
        if t.is_empty() || t.chars().all(char::is_alphabetic) {
            t.to_string()
        } else {
            format!("{}{t}", if raised { '^' } else { '_' })
        }
    };
    let mut out = std::borrow::Cow::Borrowed(xml);
    if out.contains("<sup") || out.contains("<sub") {
        out = std::borrow::Cow::Owned(
            re(r"(?is)<(sup|sub)\b[^>]*>(.*?)</(?:sup|sub)>")
                .replace_all(&out, |c: &regex::Captures| {
                    mark(c[1].eq_ignore_ascii_case("sup"), &c[2])
                })
                .into_owned(),
        );
    }
    // OpenDocument says it in an automatic STYLE: `<style:style
    // style:name="T1"><style:text-properties style:text-position="super 58%"/>`,
    // then `<text:span text:style-name="T1">2</text:span>`. The position is a
    // keyword or a signed percentage (`-33% 58%` is lowered).
    if out.contains("style:text-position") {
        let decl = re(
            r#"(?s)<style:style\b[^>]*?style:name="([^"]+)"[^>]*>\s*<style:text-properties\b[^>]*?style:text-position="([^"]+)""#,
        );
        let mut shifted: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
        for c in decl.captures_iter(&out) {
            let pos = c[2].split_whitespace().next().unwrap_or("");
            let raised = match pos {
                "super" => Some(true),
                "sub" => Some(false),
                p => p
                    .trim_end_matches('%')
                    .parse::<f32>()
                    .ok()
                    .filter(|v| *v != 0.0)
                    .map(|v| v > 0.0),
            };
            if let Some(r) = raised {
                shifted.insert(c[1].to_string(), r);
            }
        }
        if !shifted.is_empty() {
            let span =
                re(r#"(?s)<text:span\b[^>]*text:style-name="([^"]+)"[^>]*>(.*?)</text:span>"#);
            out = std::borrow::Cow::Owned(
                span.replace_all(&out, |c: &regex::Captures| match shifted.get(&c[1]) {
                    Some(&r) => mark(r, &c[2]),
                    None => c[0].to_string(),
                })
                .into_owned(),
            );
        }
    }
    if out.contains("w:vertAlign") {
        let run = re(r"(?s)<w:r\b[^>]*>.*?</w:r>");
        let align = re(r#"<w:vertAlign\b[^>]*w:val="(superscript|subscript)""#);
        let text = re(r"(?s)(<w:t\b[^>]*>)(.*?)(</w:t>)");
        out = std::borrow::Cow::Owned(
            run.replace_all(&out, |c: &regex::Captures| {
                let r = &c[0];
                match align.captures(r) {
                    Some(a) => {
                        let raised = &a[1] == "superscript";
                        text.replace_all(r, |t: &regex::Captures| {
                            format!("{}{}{}", &t[1], mark(raised, &t[2]), &t[3])
                        })
                        .into_owned()
                    }
                    None => r.to_string(),
                }
            })
            .into_owned(),
        );
    }
    out.into_owned()
}

pub(super) fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank_run = 0;
    for line in s.lines() {
        let t = line.trim_end();
        if t.is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                out.push('\n');
            }
        } else {
            blank_run = 0;
            out.push_str(t);
            out.push('\n');
        }
    }
    out.trim().to_string()
}

/// Decode the XML entities a document can carry.
///
/// NUMERIC references matter as much as the named ones: exporters routinely
/// escape every non-ASCII character, so a German cell arrives as
/// `Gesch&#228;ftsf&#252;hrer` and a named-only decoder hands the agent
/// mojibake — for the exact languages that need it most. Measured on a real
/// scraped export; the synthetic corpus missed it because LibreOffice writes
/// UTF-8 directly.
///
/// `&amp;` is decoded LAST: doing it first would turn `&amp;#228;` (a literal
/// "&#228;" in the source) into `ä`, inventing content that was never there.
pub(super) fn decode_entities(s: &str) -> String {
    let mut out = if s.contains("&#") {
        re(r"&#(x[0-9A-Fa-f]+|[0-9]+);")
            .replace_all(s, |c: &regex::Captures<'_>| {
                let raw = &c[1];
                let code = raw
                    .strip_prefix('x')
                    .or_else(|| raw.strip_prefix('X'))
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .or_else(|| raw.parse::<u32>().ok());
                match code.and_then(char::from_u32) {
                    Some(ch) => ch.to_string(),
                    // Unmappable code point: leave the reference verbatim
                    // rather than drop a character the reader may need.
                    None => c[0].to_string(),
                }
            })
            .into_owned()
    } else {
        s.to_string()
    };
    out = out
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ");
    out.replace("&amp;", "&")
}

#[cfg(test)]
mod html_lists_are_marked {
    use super::mark_html_lists;

    #[test]
    fn bullets_numbers_nesting_and_start() {
        let out = mark_html_lists(
            r#"<link href="s.css"/><ul><li>A<ul><li>A1</li></ul></li><li>B</li></ul><ol start="7"><li>Seven</li><li>Eight</li></ol>"#,
        );
        assert!(out.contains("<li>- A"), "{out}");
        assert!(out.contains("<li>  - A1"), "{out}");
        assert!(out.contains("<li>- B"), "{out}");
        assert!(
            out.contains("<li>7. Seven") && out.contains("<li>8. Eight"),
            "{out}"
        );
        assert!(
            out.contains(r#"<link href="s.css"/><ul>"#),
            "a <link> is not a list item: {out}"
        );
    }
}
