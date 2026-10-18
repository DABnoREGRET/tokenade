//! A CSS-selector subset, matched over raw HTML.
//!
//! Enough to answer `--select`/`--exclude`: tag, classes, id. No DOM here on
//! the regex path, so matching is textual and deliberately narrow.

/// Apply a CSS-selector-style exclusion to HTML before markdown
/// conversion. Selectors are matched naively against tag + class +
/// id (`.foo` class, `#bar` id, `tagname` plain tag). Nested AND
/// (`tag.cls#id`) is supported; descendant combinators (`a b`) are
/// NOT — they'd require a real parser. Multiple selectors are
/// comma-separated.
///
/// Returns HTML with matched elements (including their content) removed.
/// Best-effort: malformed input passes through unchanged.
pub fn exclude_selectors(html: &str, selectors: &str) -> String {
    let mut out = html.to_string();
    for sel in selectors
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        out = strip_by_selector(&out, sel);
    }
    out
}

/// Inner HTML of the FIRST element matching `sel` (`tag.class#id`), or `None`.
///
/// The inverse of [`strip_by_selector`], and it cannot share that function's
/// closer scan: stripping chrome gets away with "first `</tag>` wins" because
/// a `<nav>` rarely nests another `<nav>`, but an article body is nothing BUT
/// nested `<div>`s — the naive scan would cut it at the first paragraph
/// wrapper's close. So this one counts depth.
///
/// Class matching is token-exact (`class="article-body promo"` matches
/// `.article-body`, `class="article-body-ad"` does not) — a site reader keys
/// off one container name, and a substring match would silently pick up an
/// adjacent widget with a longer name.
pub fn extract_by_selector(html: &str, sel: &str) -> Option<String> {
    let (tag, classes, id) = selector_parts(sel);
    let tag_match = tag.unwrap_or("*");
    // `(\s[^>]*)?` — not `([^>]*)`: the loose form lets `<div` match the
    // opening of `<divider …>`, since `[^>]*` happily absorbs "ider".
    let tag_pat = if tag_match == "*" {
        r#"<([a-zA-Z][a-zA-Z0-9]*)(\s[^>]*)?>"#.to_string()
    } else {
        format!(r#"(?i)<({tag_match})(\s[^>]*)?>"#)
    };
    let opener_re = regex::Regex::new(&tag_pat).ok()?;
    let mut cursor = 0usize;
    while cursor < html.len() {
        let m = opener_re.find(&html[cursor..])?;
        let caps = opener_re.captures(m.as_str())?;
        let actual_tag = caps.get(1).map_or("", |c| c.as_str()).to_ascii_lowercase();
        let attrs = caps.get(2).map_or("", |c| c.as_str());
        let open_end = cursor + m.end();
        // `<div … />` closes itself: it has no inner HTML to return, and
        // treating it as an opener would swallow the rest of the document.
        if !(attrs_match(attrs, &classes, id) && !attrs.trim_end().ends_with('/')) {
            cursor = open_end;
            continue;
        }
        // Walk same-tag openers/closers from here, tracking depth.
        let scan_re = regex::Regex::new(&format!(r#"(?i)<(/?){actual_tag}(\s[^>]*)?>"#)).ok()?;
        let mut depth = 1usize;
        for t in scan_re.find_iter(&html[open_end..]) {
            if t.as_str()[1..2].starts_with('/') {
                depth -= 1;
                if depth == 0 {
                    return Some(html[open_end..open_end + t.start()].to_string());
                }
            } else if !t.as_str().trim_end().ends_with("/>") {
                depth += 1;
            }
        }
        // Unclosed container (truncated page): everything after the opener is
        // still the body, and a partial article beats no article.
        return Some(html[open_end..].to_string());
    }
    None
}

/// Token-exact `class` / `id` attribute match for [`extract_by_selector`].
pub(super) fn attrs_match(attrs: &str, classes: &[&str], id: Option<&str>) -> bool {
    let attr_value = |name: &str| -> Option<&str> {
        let at = attrs
            .find(&format!("{name}=\""))
            .map(|i| (i + name.len() + 2, '"'))
            .or_else(|| {
                attrs
                    .find(&format!("{name}='"))
                    .map(|i| (i + name.len() + 2, '\''))
            })?;
        let rest = &attrs[at.0..];
        Some(&rest[..rest.find(at.1)?])
    };
    let class_ok = classes.is_empty()
        || attr_value("class").is_some_and(|v| {
            classes
                .iter()
                .all(|c| v.split_whitespace().any(|tok| tok == *c))
        });
    let id_ok = id.is_none_or(|want| attr_value("id").is_some_and(|v| v.trim() == want));
    class_ok && id_ok
}

/// Decompose `tag.cls#id` → (tag, classes, id).
pub(super) fn selector_parts(sel: &str) -> (Option<&str>, Vec<&str>, Option<&str>) {
    let mut tag: Option<&str> = None;
    let mut classes: Vec<&str> = Vec::new();
    let mut id: Option<&str> = None;
    let mut idx = 0;
    let bytes = sel.as_bytes();
    while idx < bytes.len() {
        let c = bytes[idx];
        if c == b'.' || c == b'#' {
            idx += 1;
            let start = idx;
            while idx < bytes.len() && bytes[idx] != b'.' && bytes[idx] != b'#' {
                idx += 1;
            }
            if c == b'.' {
                classes.push(&sel[start..idx]);
            } else {
                id = Some(&sel[start..idx]);
            }
        } else {
            let start = idx;
            while idx < bytes.len() && bytes[idx] != b'.' && bytes[idx] != b'#' {
                idx += 1;
            }
            tag = Some(&sel[start..idx]);
        }
    }
    (tag, classes, id)
}

pub(super) fn strip_by_selector(html: &str, sel: &str) -> String {
    // Decompose `tag.cls#id` → (Option<tag>, Vec<cls>, Option<id>).
    let mut tag: Option<&str> = None;
    let mut class_filters: Vec<&str> = Vec::new();
    let mut id_filter: Option<&str> = None;
    let mut idx = 0;
    let bytes = sel.as_bytes();
    while idx < bytes.len() {
        let c = bytes[idx];
        if c == b'.' || c == b'#' {
            let kind = c;
            idx += 1;
            let start = idx;
            while idx < bytes.len() && bytes[idx] != b'.' && bytes[idx] != b'#' {
                idx += 1;
            }
            let val = &sel[start..idx];
            if kind == b'.' {
                class_filters.push(val);
            } else {
                id_filter = Some(val);
            }
        } else {
            let start = idx;
            while idx < bytes.len() && bytes[idx] != b'.' && bytes[idx] != b'#' {
                idx += 1;
            }
            tag = Some(&sel[start..idx]);
        }
    }
    // Default to any tag if no explicit one in selector.
    let tag_match = tag.unwrap_or("*");
    // Build a search loop: find `<TAG ...>` whose attrs match, then
    // remove from opener to matching closer.
    let tag_pat = if tag_match == "*" {
        r#"<([a-zA-Z][a-zA-Z0-9]*)([^>]*)>"#.to_string()
    } else {
        format!(r#"<({tag_match})([^>]*)>"#)
    };
    let opener_re = match regex::Regex::new(&tag_pat) {
        Ok(r) => r,
        Err(_) => return html.to_string(),
    };
    let mut out = String::with_capacity(html.len());
    let mut cursor = 0;
    while cursor < html.len() {
        let slice = &html[cursor..];
        let Some(m) = opener_re.find(slice) else {
            out.push_str(slice);
            break;
        };
        // Append any pre-match content.
        out.push_str(&slice[..m.start()]);
        let matched = m.as_str();
        let caps = opener_re.captures(matched).unwrap();
        let actual_tag = caps.get(1).map_or("", |c| c.as_str());
        let attrs = caps.get(2).map_or("", |c| c.as_str());
        // Check class / id filters.
        let class_ok = class_filters.iter().all(|c| {
            attrs.contains(&format!(" class=\"{c}\""))
                || attrs.contains(&format!(" class='{c}'"))
                || (attrs.contains("class=") && attrs.contains(c))
        });
        let id_ok = id_filter.is_none_or(|id| {
            attrs.contains(&format!(" id=\"{id}\"")) || attrs.contains(&format!(" id='{id}'"))
        });
        if !class_ok || !id_ok {
            // Not a match — keep the opener tag and resume.
            out.push_str(matched);
            cursor += m.end();
            continue;
        }
        // Find the matching closer. Naive nesting: search for first
        // `</TAG>` after this position. Doesn't handle nested same-tag
        // perfectly but is correct for typical chrome (nav/header).
        let closer = format!("</{actual_tag}>");
        let after_opener = cursor + m.end();
        match html[after_opener..].find(&closer) {
            Some(rel) => {
                cursor = after_opener + rel + closer.len();
            }
            None => {
                // Unclosed — drop the opener and bail.
                cursor = after_opener;
            }
        }
    }
    out
}
