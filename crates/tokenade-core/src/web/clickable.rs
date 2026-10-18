//! **What an agent can actually REACH.**
//!
//! A link that nothing makes clickable is not a link: delivering it costs
//! bytes and offers an impossible action. Accessible names are here for the
//! same reason — it is the name a screen reader announces that tells whether
//! a target is named, not the visible text.

use super::*;

/// The quoted accessible name of an a11y line, if it has one.
/// Drop `[ref=eN]` from nodes that are not a click target.
///
/// Measured on the real snapshot corpus this is 11.9 % of everything left after
/// the rest of the pipeline has run — the largest remaining item by a distance,
/// and the only large one that costs the agent nothing. What it removes is a
/// handle to something no `browser_click` can act on: a paragraph, a table cell,
/// a term, a piece of code, an image.
///
/// Two guards keep it honest: the list of dropped roles is a DENYLIST, so any
/// role not on it keeps its ref; and a node marked `[cursor=…]` keeps its ref
/// whatever its role, because that marker is how a snapshot says "this responds
/// to a click". Runs before [`strip_attribute_noise`], which removes that
/// marker.
pub(super) fn drop_refs_the_agent_cannot_click(
    s: &str,
    clickable: &std::collections::HashSet<String>,
) -> String {
    let ref_re = re(r"\s*\[ref=(e\d+)\]");
    let mut out = String::with_capacity(s.len());
    for line in s.lines() {
        let t = line.trim_start();
        let role = t
            .strip_prefix("- ")
            .unwrap_or(t)
            .split([' ', ':', '['])
            .next()
            .unwrap_or("");
        let marked = ref_re
            .captures(line)
            .is_some_and(|c| clickable.contains(c.get(1).map_or("", |m| m.as_str())));
        if A11Y_NOT_CLICKABLE.contains(&role) && !marked {
            out.push_str(&ref_re.replace_all(line, ""));
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// The refs the snapshot marked with a pointer cursor — how a Playwright
/// snapshot says "this responds to a click" for an element whose role does not.
pub(super) fn refs_marked_clickable(snapshot: &str) -> std::collections::HashSet<String> {
    let both = re(r"\[ref=(e\d+)\]");
    snapshot
        .lines()
        .filter(|l| l.contains("[cursor="))
        .filter_map(|l| both.captures(l))
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
        .collect()
}

/// Roles that are not a click target. A `ref` is a handle for `browser_click`;
/// on a paragraph, a table cell, a piece of code or a plain image it is a handle
/// to nothing, and it costs several tokens on every line of a snapshot.
///
/// This is a DENYLIST on purpose. An unknown or newly-standardised role keeps
/// its ref: the cost of carrying a ref that is never used is a few tokens, the
/// cost of dropping one an agent needed is a page it cannot operate. The
/// generic wrapper roles are deliberately absent — `generic`, `group` and `none`
/// are exactly where a `<div onclick=…>` shows up.
pub(crate) const A11Y_NOT_CLICKABLE: &[&str] = &[
    "paragraph",
    "text",
    "StaticText",
    "code",
    "emphasis",
    "strong",
    "heading",
    "region",
    "main",
    "article",
    "section",
    "banner",
    "contentinfo",
    "complementary",
    "navigation",
    "list",
    "listitem",
    "term",
    "definition",
    "table",
    "row",
    "cell",
    "rowheader",
    "columnheader",
    "rowgroup",
    "caption",
    "figure",
    "img",
    "image",
    "separator",
    "blockquote",
    "note",
    "time",
    "superscript",
    "subscript",
    "deletion",
    "insertion",
    "mark",
    "math",
    "document",
    "definition",
    "figcaption",
    "directory",
    "feed",
    "presentation",
];

/// Byte span of the quoted accessible name in `line`: the opening quote and
/// the closing one, as offsets into `line` itself.
///
/// **The closing quote is the first UNESCAPED one.** Both callers used to take
/// the first `"` after the opening one, so a name containing an escaped quote
/// ended at the escape. Playwright writes those constantly — any page whose
/// heading quotes something does it — and the damage was not a short name but
/// mangled output: `a11y_without_name` cut at the wrong place and glued the
/// remainder of the name onto the role, turning
/// `heading "[Bug]: \"Future exception was never retrieved\" on SIGINT"` into
/// `headingFuture exception was never retrieved" on SIGINT`. Found by asking
/// whether compacting twice changes anything, which it did.
///
/// Scans by char, not by byte: a name is arbitrary page text.
pub(super) fn a11y_name_span(line: &str) -> Option<(usize, usize)> {
    let start = line.find('"')?;
    let mut escaped = false;
    for (i, c) in line.char_indices().skip_while(|(i, _)| *i <= start) {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '"' => return Some((start, i)),
            _ => {}
        }
    }
    None
}

pub(super) fn a11y_quoted_name(line: &str) -> Option<String> {
    let (start, stop) = a11y_name_span(line)?;
    // The name as the page means it: an escaped quote IS a quote.
    let raw = &line[start + 1..stop];
    if !raw.contains('\\') {
        return Some(raw.to_string());
    }
    let mut out = String::with_capacity(raw.len());
    let mut escaped = false;
    for c in raw.chars() {
        if escaped {
            out.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else {
            out.push(c);
        }
    }
    Some(out)
}

/// The text of a plain text leaf (`- Some words`), which is neither a role line
/// nor a `/url:` line nor an attribute-only node.
pub(super) fn a11y_plain_text(trimmed: &str) -> Option<&str> {
    let rest = trimmed.strip_prefix("- ")?;
    if rest.starts_with('/') || rest.contains('[') || rest.ends_with(':') {
        return None;
    }
    // `- listitem: text` and friends are role lines, not bare text.
    if let Some((head, _)) = rest.split_once(':') {
        if !head.is_empty() && !head.contains(' ') {
            return None;
        }
    }
    Some(rest)
}

/// `- row "a b c" [ref=e5]:` → `- row [ref=e5]:`
pub(super) fn a11y_without_name(line: &str) -> String {
    let Some((start, stop)) = a11y_name_span(line) else {
        return line.to_string();
    };
    let mut out = String::with_capacity(line.len());
    out.push_str(line[..start].trim_end());
    out.push_str(&line[stop + 1..]);
    out
}

/// A trailing `:` promises children. Once a node's children have been folded
/// away, the colon is a broken promise — and a snapshot is parsed as YAML by
/// whoever reads it.
pub(super) fn strip_dangling_colons(s: &str) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let indent: Vec<usize> = lines
        .iter()
        .map(|l| l.len() - l.trim_start().len())
        .collect();
    lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            if !line.ends_with(':') {
                return (*line).to_string();
            }
            let has_child = lines[i + 1..]
                .iter()
                .enumerate()
                .find(|(_, l)| !l.trim().is_empty())
                .is_some_and(|(k, _)| indent[i + 1 + k] > indent[i]);
            if has_child {
                (*line).to_string()
            } else {
                line[..line.len() - 1].to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn fold_uniform_runs(s: &str) -> String {
    // Three or more consecutive `link "..."` siblings (same indent) → keep
    // the first three + "… N more links".
    let lines: Vec<&str> = s.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let is_link = line.trim_start().starts_with("- link ");
        if is_link {
            let indent: usize = line.chars().take_while(|c| *c == ' ').count();
            let mut j = i;
            while j < lines.len() {
                let l = lines[j];
                let li = l.chars().take_while(|c| *c == ' ').count();
                if li == indent && l.trim_start().starts_with("- link ") {
                    j += 1;
                } else {
                    break;
                }
            }
            let run = j - i;
            if run >= 5 {
                for l in lines.iter().skip(i).take(3) {
                    out.push((*l).to_string());
                }
                out.push(format!(
                    "{}- … {} more links …",
                    " ".repeat(indent),
                    run - 3
                ));
                i = j;
                continue;
            }
        }
        out.push(line.to_string());
        i += 1;
    }
    out.join("\n")
}
