//! **What the page DECLARES points elsewhere.**
//!
//! A `<frame>`, a `<base href>`: the substance is not in this document, and
//! rendering it without saying so delivers an empty page that looks complete.
//! Naming the targets leaves the caller the choice of fetching them.

/// The mark the distiller puts down when it recovered almost nothing. Spelled
/// once: what the page says about itself is hooked onto it.
pub const THIN_RESULT_MARK: &str = "[tokenade: thin-result]";

/// **The document is in a FRAME, and the page says so.**
///
/// When a page renders almost nothing, the diagnostic lists three possible
/// causes — client-side rendering, anti-bot block, stub page — and keeps
/// quiet about the only FACT the document carries: the address of the frame
/// where the content lives. MEASURED 2026-09-09 on a 4 KB portal that is
/// nothing but a navigation bar and an `<iframe src="https://contenu…/vrai">`:
/// the note points to "the text is rendered client-side" while the address
/// to fetch is one line away.
///
/// Same move as `declared_feed_url`, which already names the feed of a page
/// distilled to nothing: we do not fetch it, we say where it is.
///
/// The address is resolved against the document's base — a `src="/vrai"`
/// without an origin can no more be followed than a relative reference.
pub fn declared_frame_urls(html: &str, url: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let base = declared_base_href(html, url).unwrap_or_else(|| url.to_string());
    let Ok(base) = reqwest::Url::parse(&base) else {
        return out;
    };
    let lower = html.to_ascii_lowercase();
    let mut at = 0usize;
    loop {
        let next = match (lower[at..].find("<iframe "), lower[at..].find("<frame ")) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => return out,
        };
        let start = at + next;
        let Some(rel_end) = lower[start..].find('>') else {
            return out;
        };
        let end = start + rel_end;
        at = end + 1;
        let tag = &html[start..end];
        if is_an_unseen_frame(tag, &lower[..start]) {
            continue;
        }
        let Some(src) = attr_value(tag, "src") else {
            continue;
        };
        let src = src.trim();
        // An EMPTY `src` resolves to the page itself, so naming it would send
        // the reader back to where they already are.
        //
        // `about:blank`, `javascript:` and `data:` have NO test of their own:
        // the scheme filter three lines below already rejects them, and the
        // mutation that removed their explicit case survived — it could not
        // change anything. A precaution that cannot fire reassures about a
        // different question than the one it asks.
        if src.is_empty() {
            continue;
        }
        let Ok(u) = base.join(src) else { continue };
        if !matches!(u.scheme(), "http" | "https") {
            continue;
        }
        out.push(u.to_string());
        if out.len() == MAX_FRAMES_NAMED {
            break;
        }
    }
    out
}

/// **A frame nobody sees is not the document.** Google Tag Manager's tracking
/// pixel — `<noscript><iframe src="…/ns.html?id=GTM-…" height="0" width="0"
/// style="display:none;visibility:hidden">` — was announced as "the document
/// may be in a FRAME" on a vendor site whose page is only a menu
/// (2026-10-04, field survey): a false lead written as a fact. Discarded: a
/// frame under a still-open `<noscript>` (served only to script-less
/// browsers, never to the rendering the note describes), hidden (`hidden`,
/// `display:none`, `visibility:hidden`), or 0 or 1 pixel in size.
fn is_an_unseen_frame(tag: &str, lower_before: &str) -> bool {
    let in_noscript = lower_before
        .rfind("<noscript")
        .is_some_and(|o| lower_before[o..].find("</noscript").is_none());
    let style: String = attr_value(tag, "style")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let tiny = |name: &str| {
        attr_value(tag, name).is_some_and(|v| matches!(v.trim().trim_end_matches("px"), "0" | "1"))
    };
    in_noscript
        // `hidden` is a BOOLEAN attribute, most often written without a value.
        || tag.to_ascii_lowercase().split(|c: char| c.is_whitespace() || c == '/').any(|w| w == "hidden" || w.starts_with("hidden="))
        || style.contains("display:none")
        || style.contains("visibility:hidden")
        || tiny("width")
        || tiny("height")
}

/// How many frames to name at most. A frameset document carries two or three;
/// beyond that it is a grid, and the list stops helping.
pub(super) const MAX_FRAMES_NAMED: usize = 3;

/// The FIRST frame that leads somewhere, for whoever wants only one.
pub fn declared_frame_url(html: &str, url: &str) -> Option<String> {
    declared_frame_urls(html, url).into_iter().next()
}

/// **`<base href>` wins over the page address.**
///
/// That is the HTML rule: when a document declares a base, ALL its relative
/// URLs resolve against it and not against the address that served it.
/// Without this, the relative-origin note announced an origin the page itself
/// contradicts — an invention, which is worse than the silence it replaced.
/// None of the 39 corpus pages declares one, so it is rare; "rare" is not
/// "wrong without consequence".
///
/// A RELATIVE base (`<base href="/v2/">`) is legal and resolves against the
/// page address, hence the `join`.
pub fn declared_base_href(html: &str, url: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    // A base only counts inside `<head>`, and the first `<base href>` wins —
    // later ones are ignored by the spec.
    let head_end = lower.find("</head>").unwrap_or(lower.len().min(65_536));
    let head = &lower[..head_end];
    let at = head.find("<base ")?;
    let tag_end = head[at..].find('>')? + at;
    let tag = &html[at..tag_end];
    let href = attr_value(tag, "href")?;
    let base = reqwest::Url::parse(url).ok()?;
    base.join(href.trim()).ok().map(|u| u.to_string())
}

/// The value of an attribute in an opening tag, single or double quotes.
/// Written here rather than as a regex: the tag is already cut out.
pub(super) fn attr_value(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let at = lower.find(&format!("{name}="))? + name.len() + 1;
    let rest = &tag[at..];
    let quote = rest.chars().next()?;
    if quote == '"' || quote == '\'' {
        let end = rest[1..].find(quote)? + 1;
        Some(rest[1..end].to_string())
    } else {
        let end = rest
            .find(|c: char| c.is_whitespace() || c == '>')
            .unwrap_or(rest.len());
        Some(rest[..end].to_string())
    }
}

/// The `https://host` of a page URL, or nothing if it does not name one.
pub(super) fn page_origin(url: &str) -> Option<String> {
    let u = reqwest::Url::parse(url).ok()?;
    if !matches!(u.scheme(), "http" | "https") {
        return None;
    }
    let host = u.host_str()?;
    // The stand-in for a local file. Reserved by RFC 2606 to never resolve,
    // so announcing it as an origin would be an invention.
    if host == "example.invalid" {
        return None;
    }
    match u.port() {
        Some(p) => Some(format!("{}://{host}:{p}", u.scheme())),
        None => Some(format!("{}://{host}", u.scheme())),
    }
}
