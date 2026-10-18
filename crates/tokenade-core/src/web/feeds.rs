//! **XML that is not a page.**
//!
//! An Atom or RSS feed, a labelled export: the structure is the content, and
//! converting it like a page loses the fields. Per-record rendering keeps
//! each value's path, which makes the output searchable — a single-line feed
//! once made us count "1 entry" where there were five.

use super::*;

/// Route a fetched TEXT body to the right distiller: JSON payloads (API
/// specs, registry responses) get a structural outline instead of being
/// smeared through the HTML regex pipeline (measured: an OpenAPI spec
/// passed through whole at 5.7k tokens because nothing could segment it);
/// everything else takes the HTML path. The declared Content-Type wins;
/// otherwise a first-byte sniff + a real parse gates the JSON path, so
/// HTML that merely starts with `{` can't be misrouted.
/// Should this body be rendered as a FEED rather than as a page?
///
/// Recognised by CONTENT, not by URL shape: `site_readers::plan` matches the
/// conventional suffixes (`.rss`, `/feed`) and misses every feed served from a
/// plain path or a query string, while the authoritative signals — the
/// Content-Type and the root element — only exist once the bytes are in hand.
///
/// Lives here, shared, because it did not: the WebFetch hook routed feeds and
/// `distill_text` never learned to, so `tokenade web <feed>` ran the HTML regex
/// pipeline over XML and emitted every text node on its own line. Measured on
/// americansongwriter.com/feed: **237 KB in, 109 910 bytes out** — dates, GUIDs
/// and `https://wordpress.org/?v=6.9.5`, roughly 27 000 tokens of tag soup.
pub fn looks_like_feed(body: &str, content_type: Option<&str>) -> bool {
    let ct_says_feed = content_type.is_some_and(|ct| {
        let ct = ct
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        ct.contains("rss") || ct.contains("atom") || ct.ends_with("+xml") || ct == "application/xml"
    });
    ct_says_feed || body.trim_start().starts_with("<?xml")
}

/// Beyond this, an attribute value is a blob and not data to read: a base64
/// image, an SVG path, a signed token.
pub(super) const ATTR_VALUE_MAX_CHARS: usize = 200;

/// An XML export renders its values WITH the name of the field that carries
/// them.
///
/// A feed and a sitemap have their own reader; any other XML fell through to
/// the HTML pipeline, whose tag stripping leaves only the bare values.
/// MEASURED 2026-09-10 on a three-product catalogue: the output is
/// `Perceuse sans fil` / `89.90` / `12` / the description, one paragraph per
/// value — and nothing says that `89.90` is a price or that `12` is a stock.
/// It is the defect the comment of `xml_document_shortcut` already describes
/// for feeds ("109,910 bytes of dates and GUIDs, one text node per line"),
/// fixed there and not here.
///
/// **The rule only applies if it loses NOTHING.** Only LEAVES are labelled
/// (`<name>…</name>` with no tag inside); a mixed-content document — text next
/// to an element, as in prose — would lose half of it. Rather than guessing
/// the document's shape, the output is compared with the text tag stripping
/// would have returned: at the slightest difference, `None`, and the document
/// goes back through the previous path.
pub fn labelled_xml_records(xml: &str) -> Option<String> {
    let root = re(r"(?is)<\s*([A-Za-z_][\w.:-]*)").captures(xml)?;
    let root = root.get(1)?.as_str().to_ascii_lowercase();
    let root = root.rsplit(':').next().unwrap_or(&root);
    // Roots that have their own reader, XHTML which is a page, and SVG whose
    // attributes are coordinates: nobody wants to read a three-thousand
    // character `d="M12 3.5c…"` labelled "d".
    if matches!(
        root,
        "html" | "feed" | "rss" | "urlset" | "sitemapindex" | "svg"
    ) {
        return None;
    }
    let tag =
        re(r#"(?is)<(/?)([A-Za-z_][\w.:-]*)((?:\s+[^=\s/>]+\s*=\s*(?:"[^"]*"|'[^']*'))*)\s*(/?)>"#);
    let mut lines: Vec<String> = Vec::new();
    let mut kept = String::new();
    // The path from the root. `stack[0]` is the root, `stack[1]` is the
    // record; what follows is what DISAMBIGUATES the field.
    let mut stack: Vec<String> = Vec::new();
    // The open tag of which we do not yet know whether it carries text or
    // children.
    let mut pending: Option<(String, usize)> = None;
    let mut open_record = false;
    let mut text_from = 0usize;
    let push = |lines: &mut Vec<String>, open_record: &mut bool, line: String| {
        if *open_record && !lines.is_empty() {
            lines.push(String::new());
        }
        *open_record = false;
        lines.push(line);
    };
    for c in tag.captures_iter(xml) {
        let closing = !c.get(1).map_or("", |m| m.as_str()).is_empty();
        let name = c.get(2).map_or("", |m| m.as_str());
        let name = name.rsplit(':').next().unwrap_or(name).to_string();
        let self_closing = !c.get(4).map_or("", |m| m.as_str()).is_empty();
        let text = xml
            .get(text_from..c.get(0).map_or(0, |m| m.start()))
            .unwrap_or("");
        text_from = c.get(0).map_or(0, |m| m.end());

        if closing {
            // A leaf: the tag being closed is the one that was opened.
            if let Some((open_name, at)) = pending.take() {
                if open_name == name {
                    let value = decode_entities(text);
                    let value = value.trim().to_string();
                    if !value.is_empty() {
                        kept.push_str(&value);
                        // The attributes of the OPENING tag: a closing tag
                        // carries none, and reading them there made
                        // `total (devise=EUR)` disappear.
                        let attrs = attrs_at_offset(xml, at, tag);
                        let path = field_path(&stack, &name);
                        let line = match attrs {
                            Some(a) => format!("{path} ({a}): {value}"),
                            None => format!("{path}: {value}"),
                        };
                        push(&mut lines, &mut open_record, line);
                    }
                }
            }
            // A record closes: the next one opens after an empty line. It is
            // an EXACT boundary, whereas "the field name comes back" was a
            // guess that put the line in the wrong place.
            if stack.len() == 2 {
                open_record = true;
            }
            stack.pop();
            continue;
        }

        // An opening tag following an opening tag: the previous one is a
        // container, and its attributes (a record's id) are worth a line.
        if let Some((open_name, at)) = pending.take() {
            if let Some(a) = attrs_at_offset(xml, at, tag) {
                let path = field_path_of_parent(&stack, &open_name);
                push(&mut lines, &mut open_record, format!("{path}: {a}"));
            }
        }
        stack.push(name.clone());
        if self_closing {
            if let Some(a) = readable_attrs(c.get(3).map_or("", |m| m.as_str())) {
                let path = field_path(&stack, &name);
                push(&mut lines, &mut open_record, format!("{path}: {a}"));
            }
            if stack.len() == 2 {
                open_record = true;
            }
            stack.pop();
        } else {
            pending = Some((name, c.get(0).map_or(0, |m| m.start())));
        }
    }
    if lines.is_empty() {
        return None;
    }
    // **Nothing was lost**, compared with the text the previous path returned.
    // The comparison is on the TEXT alone: attributes are added, they replace
    // nothing, so they cannot hide a loss.
    let plain: String = decode_entities(&strip_all_tags(xml))
        .split_whitespace()
        .collect();
    let ours: String = kept.split_whitespace().collect();
    if plain != ours {
        return None;
    }
    Some(format!("> source: xml\n\n{}", lines.join("\n").trim()))
}

/// A field's name, prefixed by what DISAMBIGUATES it.
///
/// `stack[0]` is the root and `stack[1]` the record: neither tells two fields
/// apart. What follows does. MEASURED 2026-09-10 on an orders export:
/// `<client><ville>Lyon</ville></client>` and
/// `<livraison><ville>Villeurbanne</ville></livraison>` rendered `ville:`
/// twice and nothing said which one was the customer's address.
pub(super) fn field_path(stack: &[String], name: &str) -> String {
    if stack.len() <= 2 {
        return name.to_string();
    }
    stack[2..].join(".")
}

/// The path of a CONTAINER, whose name is already on the stack.
pub(super) fn field_path_of_parent(stack: &[String], name: &str) -> String {
    if stack.len() <= 1 {
        return name.to_string();
    }
    let mut parts: Vec<&str> = stack[2..].iter().map(String::as_str).collect();
    parts.push(name);
    if parts.len() == 1 {
        name.to_string()
    } else {
        parts.join(".")
    }
}

/// The same, re-read at the offset of a tag already passed.
pub(super) fn attrs_at_offset(xml: &str, at: usize, tag: &regex::Regex) -> Option<String> {
    let c = tag.captures(xml.get(at..)?)?;
    readable_attrs(c.get(3).map_or("", |m| m.as_str()))
}

/// What an attribute can bring to a reader — and the three forms that bring
/// nothing: a namespace is a schema address, an oversized value is a blob
/// (base64, path, token), an empty value is nothing.
pub(super) fn readable_attrs(attrs: &str) -> Option<String> {
    let pair = re(r#"(?is)([^=\s/>]+)\s*=\s*("([^"]*)"|'([^']*)')"#);
    let mut shown: Vec<String> = Vec::new();
    for a in pair.captures_iter(attrs) {
        let key = a.get(1).map_or("", |m| m.as_str());
        let value = a.get(3).or_else(|| a.get(4)).map_or("", |m| m.as_str());
        let value = decode_entities(value);
        let value = value.trim();
        if key.eq_ignore_ascii_case("xmlns")
            || key.to_ascii_lowercase().starts_with("xmlns:")
            || value.is_empty()
            || value.chars().count() > ATTR_VALUE_MAX_CHARS
        {
            continue;
        }
        shown.push(format!("{key}={value}"));
    }
    (!shown.is_empty()).then(|| shown.join(", "))
}
