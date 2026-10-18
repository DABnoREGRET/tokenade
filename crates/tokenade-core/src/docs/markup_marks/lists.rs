//! Lists: bullets and numbers of each schema (Word, PowerPoint, OpenDocument,
//! HTML), and what says a list is numbered.

use super::*;

pub(in crate::docs) fn mark_lists(
    xml: &str,
    ordered: &std::collections::HashSet<String>,
    numbering: &NumberStyles,
) -> String {
    let out = mark_ooxml_lists(xml, ordered, numbering);
    mark_html_lists(&mark_odf_lists(&out, ordered, numbering))
}

/// HTML (the chapters of an EPUB): `<ul>`/`<ol>`/`<li>`, the third spelling of
/// the list the two others already mark. Unmarked, an EPUB's list came back as
/// bare lines with its nesting gone, where the `.docx` and `.odt` of the SAME
/// source read `- Phase one` / `  - Design review` (2026-10-03). Same walk as
/// the ODF one: exact names, a stack per open list, `start` honoured.
pub(in crate::docs) fn mark_html_lists(xml: &str) -> String {
    if !xml.contains("<li") {
        return xml.to_string();
    }
    let value_re = re(r#"(?is)\bvalue\s*=\s*["']?(\d+)"#);
    let named = |t: &str, name: &str| {
        t.strip_prefix('<')
            .filter(|r| r.len() >= name.len() && r[..name.len()].eq_ignore_ascii_case(name))
            .is_some_and(|r| r[name.len()..].starts_with(['>', ' ', '\t', '\n', '\r', '/']))
    };
    // (ordered, counter, format) per open list.
    let mut stack: Vec<(bool, usize, NumberStyle)> = Vec::new();
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(at) = rest.find('<') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let Some(close) = tail.find('>') else {
            out.push_str(tail);
            return out;
        };
        let tag = &tail[..=close];
        out.push_str(tag);
        if named(tag, "ul") || named(tag, "ol") {
            if !tag.ends_with("/>") {
                // `type`, `start`, `list-style-type`: the format the page
                // DECLARES (`<ol type="a">` came out `1. 2.`, 2026-10-04).
                let style = html_list_style(tag);
                stack.push((named(tag, "ol"), style.start.saturating_sub(1), style));
            }
        } else if named(tag, "/ul") || named(tag, "/ol") {
            stack.pop();
        } else if named(tag, "li") {
            let depth = stack.len().saturating_sub(1);
            if let Some((ordered, n, style)) = stack.last_mut() {
                let marker = if *ordered {
                    // `<li value="9">` sets its number, and the following ones start from it.
                    if let Some(v) = value_re
                        .captures(tag)
                        .and_then(|c| c[1].parse::<usize>().ok())
                    {
                        *n = v.saturating_sub(1);
                    }
                    *n += 1;
                    label(Some(style), *n)
                } else {
                    "- ".to_string()
                };
                out.push_str(&"  ".repeat(depth.min(6)));
                out.push_str(&marker);
            }
        }
        rest = &tail[close + 1..];
    }
    out.push_str(rest);
    out
}

/// **PowerPoint: the bullet comes from the PLACEHOLDER, not the paragraph.**
///
/// A slide paragraph almost never says it has a bullet: the
/// layout gives one to every body placeholder (`<p:ph idx="1"/>`, `type="body"`
/// or `"obj"`), and `<a:pPr lvl="N">` only says its level. Without reading it, the
/// outline of a slide came out as flat lines — `Build`, `Linux`,
/// `Debian` at the same rank — where the `.odp` of the same file nested
/// (2026-10-03). What wins, in order: `<a:buNone/>` (no bullet),
/// `<a:buAutoNum>` (numbered), `<a:buChar>` (bullet, even outside a body), then
/// the placeholder. Titles, dates, footers and numbers carry none.
pub(in crate::docs) fn mark_pptx_bullets(slide: &str) -> String {
    if !slide.contains("<a:p>") && !slide.contains("<a:p ") {
        return slide.to_string();
    }
    let lvl_re = re(r#"<a:pPr\b[^>]*\blvl="(\d+)""#);
    re(r"(?s)<p:sp\b.*?</p:sp>")
        .replace_all(slide, |shape: &regex::Captures| {
            let shape = &shape[0];
            let ph = re(r"<p:ph\b[^>]*>").find(shape).map(|m| m.as_str());
            let body_placeholder =
                ph.is_some_and(|t| match re(r#"\btype="([^"]*)""#).captures(t) {
                    Some(c) => matches!(&c[1], "body" | "obj"),
                    None => true,
                });
            let mut counters = [0usize; 10];
            re(r"(?s)<a:p\b[^>]*>(.*?)</a:p>")
                .replace_all(shape, |p: &regex::Captures| {
                    let body = &p[1];
                    if !body.contains("<a:t>") && !body.contains("<a:t ") {
                        return p[0].to_string();
                    }
                    let lvl = lvl_re
                        .captures(body)
                        .and_then(|c| c[1].parse::<usize>().ok())
                        .unwrap_or(0)
                        .min(8);
                    let marker = if body.contains("<a:buNone") {
                        return p[0].to_string();
                    } else if let Some(auto) = re(r"<a:buAutoNum\b[^>]*>").find(body) {
                        // Its type (`alphaLcParenR`) and its start (`startAt`).
                        let style = drawingml_auto_num(auto.as_str());
                        if counters[lvl] == 0 {
                            counters[lvl] = style.start.saturating_sub(1);
                        }
                        counters[lvl] += 1;
                        counters[lvl + 1..].iter_mut().for_each(|c| *c = 0);
                        label(Some(&style), counters[lvl])
                    } else if body.contains("<a:buChar") || body_placeholder {
                        counters[lvl..].iter_mut().for_each(|c| *c = 0);
                        "- ".to_string()
                    } else {
                        return p[0].to_string();
                    };
                    format!("<a:p>{}{marker}{body}</a:p>", "  ".repeat(lvl))
                })
                .into_owned()
        })
        .into_owned()
}

/// Word: each paragraph carries its level and its list identifier.
pub(in crate::docs) fn mark_ooxml_lists(
    xml: &str,
    ordered: &std::collections::HashSet<String>,
    numbering: &NumberStyles,
) -> String {
    let para = re(r"(?s)<w:p\b[^>]*>(.*?)</w:p>");
    let ilvl = re(r#"<w:ilvl\s+w:val="(\d+)""#);
    let numid = re(r#"<w:numId\s+w:val="(\d+)""#);
    let numpr = re(r"(?s)<w:numPr\b.*?</w:numPr>");
    // One counter per (list identifier, level). Word does not repeat
    // the index in the document: it has the reader compute it.
    let mut seen: std::collections::HashMap<(String, usize), usize> =
        std::collections::HashMap::new();
    let mut started: std::collections::HashSet<String> = std::collections::HashSet::new();
    para.replace_all(xml, |c: &regex::Captures| {
        let body = &c[1];
        let Some(pr) = numpr.find(body) else {
            return c[0].to_string();
        };
        let pr = pr.as_str();
        let lvl = ilvl
            .captures(pr)
            .and_then(|m| m[1].parse::<usize>().ok())
            .unwrap_or(0);
        let Some(id) = numid.captures(pr).map(|m| m[1].to_string()) else {
            return c[0].to_string();
        };
        // `w:numId="0"` means « this list is cancelled on this
        // paragraph »: it is ordinary text, not an item.
        if id == "0" {
            return c[0].to_string();
        }
        // Never shallower than the depth its style shows (`with_style_numbering`).
        let indent = re(r#"<w:tkShown\s+w:val="(\d+)""#)
            .captures(pr)
            .and_then(|m| m[1].parse::<usize>().ok())
            .map_or(lvl, |d| d.max(lvl));
        let key = list_key(&id, lvl);
        let marker = if ordered.contains(&key) {
            // The counter belongs to the ABSTRACT list that several `numId`s
            // share; a `startOverride` resets it to the start on the first
            // use of ITS `numId` (Word, ECMA-376 §17.9.27).
            let style = numbering.get(&key);
            let counter = style
                .and_then(|s| s.list.clone())
                .unwrap_or_else(|| id.clone());
            let first_use = started.insert(key.clone());
            let n = seen
                .entry((counter, lvl))
                .or_insert(style.map_or(0, |s| s.start.saturating_sub(1)));
            if first_use && style.is_some_and(|s| s.restarts) {
                *n = style.map_or(0, |s| s.start.saturating_sub(1));
            }
            *n += 1;
            label(style, *n)
        } else {
            "- ".to_string()
        };
        format!("<w:p>{}{marker}{body}</w:p>", "  ".repeat(indent.min(6)))
    })
    .into_owned()
}

/// Word: a list can come from the paragraph's STYLE, not the paragraph.
///
/// The built-in styles `List Number` / `List Bullet` (and any custom style that
/// numbers) carry their `w:numPr` in `word/styles.xml`; the paragraph only
/// cites `<w:pStyle w:val="ListNumber"/>`. Read on the paragraph alone, a
/// « 1. 2. 3. » rendered three lines without numbers. The style's `w:numPr` (followed
/// through `w:basedOn`) is copied into the paragraph that carries none; one
/// that carries one keeps its own, and only receives the list identifier
/// when it gives only its level.
pub(in crate::docs) fn with_style_numbering(doc_xml: &str, styles_xml: &str) -> String {
    let numid = re(r#"<w:numId\s+w:val="(\d+)""#);
    let ilvl = re(r#"<w:ilvl\s+w:val="(\d+)""#);
    let mut own: std::collections::HashMap<
        String,
        (Option<String>, Option<String>, Option<String>),
    > = std::collections::HashMap::new();
    // Word's built-in `List Bullet 2`, `List Number 3`… nest by their NUMBER:
    // their own list sits at level 0 and only its indent says it is deeper, so
    // `List Bullet 2` read as a top-level item (2026-10-04, python-docx). The
    // name is stored in English whatever the interface language.
    let mut nested: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let builtin = re(r#"<w:name\s+w:val="List (?:Bullet|Number) ([2-9])""#);
    for c in re(r#"(?s)<w:style\b[^>]*\bw:styleId="([^"]*)"[^>]*>(.*?)</w:style>"#)
        .captures_iter(styles_xml)
    {
        let body = &c[2];
        if let Some(n) = builtin
            .captures(body)
            .and_then(|m| m[1].parse::<usize>().ok())
        {
            nested.insert(c[1].to_string(), (n - 1).to_string());
        }
        let pr = re(r"(?s)<w:numPr\b.*?</w:numPr>")
            .find(body)
            .map(|m| m.as_str())
            .unwrap_or("");
        let based = re(r#"<w:basedOn\s+w:val="([^"]*)""#)
            .captures(body)
            .map(|m| m[1].to_string());
        let id = numid.captures(pr).map(|m| m[1].to_string());
        let lvl = ilvl.captures(pr).map(|m| m[1].to_string());
        own.insert(c[1].to_string(), (id, lvl, based));
    }
    if own.values().all(|(id, _, _)| id.is_none()) {
        return doc_xml.to_string();
    }
    // The first ancestor that names a list; bounded against a cyclic `basedOn`.
    let resolve = |style: &str| -> Option<(String, String)> {
        let mut cur = own.get(style)?;
        for _ in 0..16 {
            if let Some(id) = &cur.0 {
                return Some((id.clone(), cur.1.clone().unwrap_or_else(|| "0".into())));
            }
            cur = own.get(cur.2.as_deref()?)?;
        }
        None
    };
    let pstyle = re(r#"<w:pStyle\s+w:val="([^"]*)""#);
    re(r"(?s)<w:pPr>(.*?)</w:pPr>")
        .replace_all(doc_xml, |c: &regex::Captures| {
            let pr = &c[1];
            let Some((id, lvl)) = pstyle.captures(pr).and_then(|m| resolve(&m[1])) else {
                return c[0].to_string();
            };
            if numid.is_match(pr) {
                return c[0].to_string();
            }
            // The depth the style's NAME shows (`List Bullet 2`), for the indent
            // only: format and count stay those of the level in its own list.
            let shown = pstyle
                .captures(pr)
                .and_then(|m| nested.get(&m[1]))
                .map(|d| format!(r#"<w:tkShown w:val="{d}"/>"#));
            let tag = format!(r#"<w:numId w:val="{id}"/>{}"#, shown.unwrap_or_default());
            if pr.contains("</w:numPr>") {
                return format!(
                    "<w:pPr>{}</w:pPr>",
                    pr.replacen("</w:numPr>", &format!("{tag}</w:numPr>"), 1)
                );
            }
            format!(r#"<w:pPr><w:numPr><w:ilvl w:val="{lvl}"/>{tag}</w:numPr>{pr}</w:pPr>"#)
        })
        .into_owned()
}

/// OpenDocument: the list is an ELEMENT that contains its items, and it
/// nests. A scan rather than an expression: a regular expression cannot
/// tell which list an item belongs to when they are nested.
pub(in crate::docs) fn mark_odf_lists(
    xml: &str,
    ordered: &std::collections::HashSet<String>,
    numbering: &NumberStyles,
) -> String {
    if !xml.contains("<text:list") {
        return xml.to_string();
    }
    let style_re = re(r#"(?s)^<text:list\b[^>]*\btext:style-name="([^"]*)""#);
    let start_re = re(r#"(?s)^<text:list\b[^>]*\btext:start-value="(\d+)""#);
    // `text:start-value` on the ITEM (that is where ODF sets it), and the
    // resumption of an interrupted list (`text:continue-numbering`).
    let item_start = re(r#"(?s)^<text:list-item\b[^>]*\btext:start-value="(\d+)""#);
    let mut last: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut out = String::with_capacity(xml.len());
    // (inherited style, counter) per open list level.
    let mut stack: Vec<(String, usize)> = Vec::new();
    // The item's marker waits for the item's first PARAGRAPH: written right
    // after `<text:list-item>`, it sat before the whitespace LibreOffice puts
    // between elements in a flat `.fodt`, and came out alone on its line
    // (`-` then `Linux`). A heading item gets none — `# Plan` is not a list
    // entry, and the `.docx` of the same document marks it as a heading only
    // (2026-10-03).
    let mut pending: Option<String> = None;
    let mut rest = xml;
    // `</text:list>` does NOT contain `<text:`: searching for the opening form alone
    // made the closings invisible, the stack never came back down and everything
    // ended up indented to the ceiling. Searched on `<`, decided on the name.
    while let Some(at) = rest.find('<') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let Some(close) = tail.find('>') else {
            out.push_str(tail);
            return out;
        };
        let tag = &tail[..=close];
        // **The EXACT name, not a prefix.** `<text:list-style>`,
        // `<text:list-level-style-number>` and `<text:list-item>` all
        // start with `<text:list`: counting them as lists opened a stack
        // that never came back down, and everything came out indented to the ceiling.
        let named = |t: &str, name: &str| {
            t.strip_prefix('<')
                .and_then(|r| r.strip_prefix(name))
                .is_some_and(|r| r.starts_with(['>', ' ', '\t', '\n', '\r', '/']))
        };
        if named(tag, "text:list-item") || named(tag, "text:list-header") {
            out.push_str(tag);
            let depth = stack.len().saturating_sub(1);
            if let Some((style, n)) = stack.last_mut() {
                // A `list-header` is the introductory text of a list: it
                // is not an item and consumes no index.
                if named(tag, "text:list-item") {
                    let key = list_key(style, depth);
                    let marker = if ordered.contains(&key) {
                        if let Some(v) = item_start
                            .captures(tag)
                            .and_then(|c| c[1].parse::<usize>().ok())
                        {
                            *n = v.saturating_sub(1);
                        }
                        *n += 1;
                        last.insert(key.clone(), *n);
                        label(numbering.get(&key), *n)
                    } else {
                        "- ".to_string()
                    };
                    pending = Some(format!("{}{marker}", "  ".repeat(depth.min(6))));
                }
            }
        } else if (named(tag, "text:p") || named(tag, "text:h")) && !tag.ends_with("/>") {
            out.push_str(tag);
            if let Some(m) = pending.take() {
                if named(tag, "text:p") {
                    out.push_str(&m);
                }
            }
        } else if tag == "</text:list>" {
            stack.pop();
            out.push_str(tag);
        } else if named(tag, "text:list") && !tag.ends_with("/>") {
            let style = style_re
                .captures(tag)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().to_string())
                // A nested list does not redeclare its style: it inherits
                // that of the list containing it.
                .or_else(|| stack.last().map(|(s, _)| s.clone()))
                .unwrap_or_default();
            let start = start_re
                .captures(tag)
                .and_then(|c| c.get(1))
                .and_then(|m| m.as_str().parse::<usize>().ok())
                .map(|v| v - 1);
            let key = list_key(&style, stack.len());
            let continues = tag.contains(r#"text:continue-numbering="true""#);
            let start = start
                .or_else(|| continues.then(|| last.get(&key).copied()).flatten())
                .unwrap_or_else(|| numbering.get(&key).map_or(0, |s| s.start.saturating_sub(1)));
            stack.push((style, start));
            out.push_str(tag);
        } else {
            out.push_str(tag);
        }
        rest = &tail[close + 1..];
    }
    out.push_str(rest);
    out
}

/// The `w:numId` identifiers whose level-0 format is a NUMBERING.
///
/// Two hops: `w:num` links the identifier the paragraph cites to a
/// `w:abstractNum`, and it is the latter that carries `w:numFmt`. Anything that is
/// neither `bullet` nor `none` is ordered — decimal, roman, letters.
pub(in crate::docs) fn ooxml_ordered_lists(
    numbering_xml: &str,
) -> std::collections::HashSet<String> {
    ooxml_level_formats(numbering_xml)
        .into_iter()
        .filter(|(_, (fmt, _))| fmt != "bullet" && fmt != "none")
        .map(|(key, _)| key)
        .collect()
}

/// The levels that display NOTHING: `w:numFmt="none"` or an empty `w:lvlText`.
/// LibreOffice thus attaches its heading styles to an outline list without
/// number; read as a bullet, each heading came out `# - Intro` (2026-10-04).
pub(in crate::docs) fn ooxml_silent_lists(
    numbering_xml: &str,
) -> std::collections::HashSet<String> {
    ooxml_level_formats(numbering_xml)
        .into_iter()
        .filter(|(_, (fmt, text))| fmt == "none" || text.as_deref() == Some(""))
        .map(|(key, _)| key)
        .collect()
}

/// The paragraph whose list displays nothing is not a list item:
/// its `w:numPr` is removed (after [`with_style_numbering`], which may have
/// copied it from the style).
pub(in crate::docs) fn without_silent_numbering(
    doc_xml: &str,
    silent: &std::collections::HashSet<String>,
) -> String {
    if silent.is_empty() {
        return doc_xml.to_string();
    }
    let ilvl = re(r#"<w:ilvl\s+w:val="(\d+)""#);
    let numid = re(r#"<w:numId\s+w:val="(\d+)""#);
    re(r"(?s)<w:numPr\b.*?</w:numPr>")
        .replace_all(doc_xml, |c: &regex::Captures| {
            let pr = &c[0];
            let lvl = ilvl
                .captures(pr)
                .and_then(|m| m[1].parse::<usize>().ok())
                .unwrap_or(0);
            match numid.captures(pr) {
                Some(id) if silent.contains(&list_key(&id[1], lvl)) => String::new(),
                _ => pr.to_string(),
            }
        })
        .into_owned()
}

/// Per list level (`list_key` of the `w:numId` identifier): its
/// `w:numFmt` and its `w:lvlText`.
fn ooxml_level_formats(
    numbering_xml: &str,
) -> std::collections::HashMap<String, (String, Option<String>)> {
    ooxml_level_details(numbering_xml)
        .into_iter()
        .map(|(k, d)| (k, (d.0, d.1)))
        .collect()
}

/// Per level: `w:numFmt`, `w:lvlText`, the first number (`w:start` of the
/// level, which a `w:num`'s `w:startOverride` replaces), the `abstractNum`
/// it belongs to and whether that start comes from a `startOverride`. Two hops:
/// `w:num` links the identifier the paragraph cites to a `w:abstractNum`,
/// which carries the levels.
#[allow(clippy::type_complexity)]
pub(in crate::docs) fn ooxml_level_details(
    numbering_xml: &str,
) -> std::collections::HashMap<String, (String, Option<String>, Option<usize>, String, bool)> {
    let attr = |block: &str, name: &str| {
        re(&format!(r#"(?s)<w:{name}\b[^>]*\bw:val="([^"]*)""#))
            .captures(block)
            .map(|m| m[1].to_string())
    };
    type Level = (usize, (String, Option<String>, Option<usize>));
    let abstract_levels: std::collections::HashMap<String, Vec<Level>> =
        re(r"(?s)<w:abstractNum\b[^>]*\bw:abstractNumId=\x22([^\x22]*)\x22.*?</w:abstractNum>")
            .captures_iter(numbering_xml)
            .map(|c| {
                let levels = re(r"(?s)<w:lvl\b[^>]*\bw:ilvl=\x22(\d+)\x22.*?</w:lvl>")
                    .captures_iter(&c[0])
                    .filter_map(|l| {
                        let n = l[1].parse::<usize>().ok()?;
                        let start = attr(&l[0], "start").and_then(|v| v.parse().ok());
                        Some((n, (attr(&l[0], "numFmt")?, attr(&l[0], "lvlText"), start)))
                    })
                    .collect();
                (c[1].to_string(), levels)
            })
            .collect();
    let mut out = std::collections::HashMap::new();
    for c in
        re(r"(?s)<w:num\b[^>]*\bw:numId=\x22([^\x22]*)\x22.*?</w:num>").captures_iter(numbering_xml)
    {
        let Some(abs) = attr(&c[0], "abstractNumId") else {
            continue;
        };
        let Some(levels) = abstract_levels.get(&abs) else {
            continue;
        };
        let overrides: std::collections::HashMap<usize, usize> =
            re(r"(?s)<w:lvlOverride\b[^>]*\bw:ilvl=\x22(\d+)\x22.*?</w:lvlOverride>")
                .captures_iter(&c[0])
                .filter_map(|o| {
                    Some((
                        o[1].parse().ok()?,
                        attr(&o[0], "startOverride")?.parse().ok()?,
                    ))
                })
                .collect();
        for (lvl, (fmt, text, start)) in levels {
            let restart = overrides.get(lvl).copied();
            out.insert(
                list_key(&c[1], *lvl),
                (
                    fmt.clone(),
                    text.clone(),
                    restart.or(*start),
                    abs.clone(),
                    restart.is_some(),
                ),
            );
        }
    }
    out
}

/// The key of a list level. Both schemas declare a format PER
/// LEVEL: a Word or OpenDocument bullet list numbers its
/// sub-levels, so judging the whole style rendered `1.` on bullets. Measured
/// on a `.odt` written by LibreOffice: a bullet list's style declares
/// **one** bullet and **nine** numberings.
pub(in crate::docs) fn list_key(id: &str, level: usize) -> String {
    format!("{id}#{level}")
}

/// OpenDocument list styles whose level 1 is a NUMBERING.
pub(in crate::docs) fn odf_ordered_lists(content_xml: &str) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for c in re(r"(?s)<text:list-style\b[^>]*\bstyle:name=\x22([^\x22]*)\x22.*?</text:list-style>")
        .captures_iter(content_xml)
    {
        let (Some(name), Some(block)) = (c.get(1), c.get(0)) else {
            continue;
        };
        for l in re(r#"(?s)<text:list-level-style-number\b[^>]*\btext:level="(\d+)""#)
            .captures_iter(block.as_str())
        {
            if let Some(n) = l.get(1).and_then(|m| m.as_str().parse::<usize>().ok()) {
                // `text:level` counts from 1, `w:ilvl` from 0.
                out.insert(list_key(name.as_str(), n.saturating_sub(1)));
            }
        }
    }
    out
}
