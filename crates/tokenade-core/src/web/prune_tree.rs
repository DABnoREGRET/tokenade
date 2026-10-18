//! **Removing what carries nothing, without removing what carries little.**
//!
//! The reader's most dangerous pruning: every rule here has a twin that cost
//! real content. An empty container goes, a single-child container folds, an
//! address that repeats its label is factored out — but a `<div>` slipped into
//! a `<ul>` took 107 headings with it, and "already said" is almost never
//! already said.

use super::*;

pub(super) fn drop_url_lines_keeping_paths(s: &str) -> String {
    // `- /url: https://example.com/foo`  →  `- /foo`, but ONLY for the page's
    // own origin. Stripping every host turns `https://github.com/pricing` on an
    // MDN page into `/pricing`, which reads as same-site: the agent can no
    // longer tell an outbound link from an internal one. The snapshot carries
    // no page URL, so the origin is inferred — the host that appears most often
    // IS the page, by a wide margin, on any real page.
    let host_re = re(r"(?m)^\s*-\s+/url:\s+https?://([^/\s]+)");
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for c in host_re.captures_iter(s) {
        *counts.entry(c.get(1).unwrap().as_str()).or_insert(0) += 1;
    }
    // Ties keep every host verbatim: with no clear winner there is no "own
    // origin" to elide, and guessing would corrupt half the links.
    let mut ranked: Vec<(&str, usize)> = counts.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    let own = match ranked.as_slice() {
        [(h, _)] => Some(*h),
        [(h, n), (_, m), ..] if n > m => Some(*h),
        _ => None,
    };
    let Some(own) = own else { return s.to_string() };
    let line_re = re(r"(?m)^(\s*-\s+)/url:\s+(https?://[^/\s]+)(/\S*)?\s*$");
    line_re
        .replace_all(s, |c: &regex::Captures| {
            let prefix = c.get(1).map_or("", |m| m.as_str());
            let origin = c.get(2).map_or("", |m| m.as_str());
            if !origin.ends_with(own) || !origin[..origin.len() - own.len()].ends_with("//") {
                return c.get(0).map_or(String::new(), |m| m.as_str().to_string());
            }
            // Own origin: keep the path alone. A bare origin still means "links
            // to the site root" — say `/` rather than leaving a dangling dash.
            format!("{prefix}{}", c.get(3).map_or("/", |m| m.as_str()))
        })
        .into_owned()
}

/// Strip low-signal attributes and role labels from accessibility lines.
///
/// Examples:
///   * `[level=1]` / `[level=2]` — heading-level is usually clear from text.
///   * `[expanded=true]` / `[selected=false]` — interaction state, irrelevant
///     for a static "what's on this page?" read.
///   * `- listitem: foo` → `- foo` (the list-item role is implied by the
///     parent `- list` and the leading dash).
///   * `- paragraph: foo` → `- foo` (paragraph role is the default; the text
///     is what matters).
pub(super) fn strip_attribute_noise(s: &str) -> String {
    // Cached regexes via the `re` helper.
    let attrs = re(
        r"\s*\[(?:level|expanded|selected|checked|pressed|focused|disabled|hidden|orientation|cursor)=[^\]]+\]",
    );
    let s = attrs.replace_all(s, "").into_owned();
    // Valueless interaction state: `[active]` marks the focused element, same
    // family as `[focused]` above. `[cursor=pointer]` (handled by the regex
    // above) is a CSS property, not accessibility information at all — it rides
    // on nearly every interactive line of a real snapshot.
    let flags = re(r"\s*\[active\]");
    let s = flags.replace_all(&s, "").into_owned();
    // Drop `paragraph:` and `listitem:` role prefixes — content is what matters.
    //
    // `[ \t]*`, NOT `\s*`: `\s` matches a NEWLINE, so a trailing `\s*` ran past
    // the end of the line and swallowed the next line's indentation, gluing a
    // wrapper to its first child — `- listitem:` over `- link "A"` and
    // `- text "and"` came out as `- - link "A"` with `- text "and"` nested
    // UNDER the link it used to sit beside. Indentation is the tree here, so
    // that is the one failure this compactor must never have: a node reparented
    // under its own sibling. `(?m)$` anchors per line; `\s` does not care.
    //
    // The role is only dropped from a line that CARRIES its content. A
    // `- listitem:` whose content is on the lines below is a wrapper, and
    // un-naming it merges its children into the list: two items holding
    // `link "A" / text "and"` and `link "B"` read back as three. The doc
    // examples above are all the content-carrying form; the wrapper form is
    // handled by the single-child collapse further down, which only removes it
    // when there is exactly one child to promote.
    let role_strip = re(r"(?m)^(\s*-\s+)(?:paragraph|listitem|text|StaticText|generic):[ \t]*(\S)");
    let s = role_strip.replace_all(&s, "${1}${2}").into_owned();
    // A role line whose content lived on the following lines now ends in a bare
    // bullet. It carries nothing; its children take its place.
    let empty_bullet = re(r"(?m)^\s*-[ \t]*\n");
    empty_bullet.replace_all(&s, "").into_owned()
}

/// Drop `- generic:`, `- group:`, `- none:` and similar wrapper nodes that
/// have no name/text content. These nodes account for ~40 % of a typical
/// Playwright snapshot — they describe DOM structure that the agent doesn't
/// need once we already have the actual content underneath them.
///
/// We only drop a wrapper when it has NO trailing text (`- generic "label":`
/// is kept because the label is meaningful) AND it has a descendant. A
/// stand-alone `- generic:` with no children is also dropped (truly empty).
/// Mark the scaffolding of LAYOUT tables — a `table` whose whole body is one
/// row holding one cell. Nothing is tabular about a 1×1 table; it is a page
/// laid out with `<table>` the way sites were built before CSS grid, and it
/// costs four levels of nesting for zero information. Data tables (several rows
/// or several columns) are untouched.
pub(super) fn layout_table_lines(lines: &[&str]) -> Vec<bool> {
    let indent: Vec<usize> = lines
        .iter()
        .map(|l| l.len() - l.trim_start().len())
        .collect();
    let role = |i: usize| -> &str {
        let t = lines[i].trim_start();
        let Some(body) = t.strip_prefix("- ") else {
            return "";
        };
        &body[..body.find([' ', ':', '[']).unwrap_or(body.len())]
    };
    let kids = |i: usize| -> Vec<usize> {
        let mut out = Vec::new();
        let mut j = i + 1;
        while j < lines.len() && (indent[j] > indent[i] || lines[j].trim().is_empty()) {
            if !lines[j].trim().is_empty() && indent[j] == indent[i] + 2 {
                out.push(j);
            }
            j += 1;
        }
        out
    };
    let mut drop = vec![false; lines.len()];
    for i in 0..lines.len() {
        if role(i) != "table" || a11y_quoted_name(lines[i]).is_some() {
            continue;
        }
        // Look through an intervening rowgroup, which this pass removes anyway.
        let mut body = kids(i);
        if body.len() == 1 && role(body[0]) == "rowgroup" {
            body = kids(body[0]);
        }
        let [row] = body[..] else { continue };
        if role(row) != "row" {
            continue;
        }
        let cells = kids(row);
        let [cell] = cells[..] else { continue };
        if role(cell) != "cell" {
            continue;
        }
        drop[i] = true;
        // The row and the cell may still carry an accessible name that the
        // fold pass did not find fully redundant — keep those, drop the rest.
        drop[row] = a11y_quoted_name(lines[row]).is_none();
        drop[cell] = a11y_quoted_name(lines[cell]).is_none();
    }
    drop
}

/// A node that carries nothing: a nameless image, or one whose whole body is
/// punctuation.
///
/// The accessibility tree exposes `img` precisely so a screen reader can
/// announce its alternative text; with no name there is nothing to announce
/// and nothing an agent can do with it. `- ·` and `- "|"` are visual
/// separators that reached the tree as text nodes. Counted across the four
/// snapshot fixtures: 77 nameless images and 33 separators.
///
/// Two conditions, both load-bearing:
///
/// - **no name** — `- img "Wikipedia"` is the alt text and stays;
/// - **no `[ref=`** — this module's doctrine is that a ref is what
///   `browser_click` targets and an icon button with no accessible name can
///   ONLY be addressed by its ref, so anything addressable survives whatever
///   it looks like.
///
/// It carried a third — "and it is a leaf" — for the length of one mutation
/// run. Removing that came back green twice: first because an `img` with
/// children is not a shape a browser emits, and then, with a malformed one
/// written on purpose, because the caller RE-PARENTS — a dropped node hands
/// its slot to its children and the subtree moves up a level rather than
/// being stranded. The condition guarded a hazard the surrounding code
/// already prevents.
///
/// Containers are a different question and this function does not answer it:
/// the wrapper list above is where `generic` / `group` / `listitem`-shaped
/// judgements are made.
pub(super) fn is_contentless_node(body: &str, has_name: bool) -> bool {
    if has_name || body.contains("[ref=") {
        return false;
    }
    let role_end = body.find([' ', ':', '[']).unwrap_or(body.len());
    if &body[..role_end] == "img" {
        return true;
    }
    // A separator: nothing but punctuation, optionally quoted.
    let text = body.trim_end_matches(':').trim().trim_matches('"');
    !text.is_empty() && text.chars().all(|c| !c.is_alphanumeric())
}

pub(super) fn drop_empty_wrapper_nodes(s: &str) -> String {
    // `rowgroup` is `<tbody>`: the accessibility tree emits one around every
    // table body and it separates nothing — the rows are in order either way.
    let wrappers = ["generic", "group", "none", "presentation", "rowgroup"];
    let lines: Vec<&str> = s.lines().collect();
    let layout = layout_table_lines(&lines);
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    // (indent in the SOURCE tree, indent the children of that node get in the
    // OUTPUT tree). A dropped node hands its own slot to its children, so the
    // subtree moves up one level instead of being stranded at an indent that
    // now reads as "child of my former sibling".
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        let trim = line.trim_start();
        if trim.is_empty() {
            out.push(String::new());
            continue;
        }
        let indent = line.len() - trim.len();
        while stack.last().is_some_and(|(src, _)| *src >= indent) {
            stack.pop();
        }
        let new_indent = stack.last().map(|(_, child)| *child).unwrap_or(0);
        let mut drop = layout[idx];
        if let Some(body) = trim.strip_prefix("- ") {
            // A line like `- generic:` (no name, optional attributes).
            // Acceptable forms: `- generic:`, `- generic [foo]:`, `- generic[foo]:`
            let role_end = body.find([' ', ':', '[']).unwrap_or(body.len());
            // A quoted name between the role and the trailing colon is
            // meaningful — `- generic "main content":` stays.
            let has_name = body.contains('"');
            drop = drop || (wrappers.contains(&&body[..role_end]) && !has_name);
            drop = drop || is_contentless_node(body, has_name);
        }
        // Children of a dropped node inherit its slot; children of a kept node
        // sit one level under wherever the parent landed.
        stack.push((indent, if drop { new_indent } else { new_indent + 2 }));
        if !drop {
            out.push(format!("{}{trim}", " ".repeat(new_indent)));
        }
    }
    out.join("\n")
}

/// Where a `generic`/`group` wrapper has been kept but has EXACTLY one
/// descendant chain (single child whose own children are also single),
/// collapse the chain into the deepest meaningful node. This avoids the
/// 5-level-deep `generic > generic > generic > button` trees that some
/// React frameworks emit.
/// A link whose only child is its own address costs two lines to say one thing.
///
/// Playwright writes the destination as a child attribute:
///
/// ```text
/// - link "Donate" [ref=e32]:
///   - /url: /wiki/Donate
/// ```
///
/// Nothing else hangs off that node, so the second line spends a bullet, an
/// indent and a newline to carry a value that fits on the first. MEASURED: 996
/// of these across the corpus. Only a LONE `/url` child is inlined — a node
/// with siblings under it keeps the nested form, because there the indentation
/// is what says which children belong to it.
pub(super) fn inline_lone_url_children(s: &str) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut skip = false;
    for i in 0..lines.len() {
        if skip {
            skip = false;
            continue;
        }
        let line = lines[i];
        let indent = line.len() - line.trim_start().len();
        let lone_url = lines.get(i + 1).is_some_and(|n| {
            let nt = n.trim_start();
            let ni = n.len() - nt.len();
            ni == indent + 2
                && nt.starts_with("- /url:")
                // Nothing else is nested under this node.
                && lines
                    .get(i + 2)
                    .is_none_or(|a| a.len() - a.trim_start().len() <= indent)
        });
        if lone_url && line.trim_end().ends_with(':') {
            let head = line.trim_end();
            let url = lines[i + 1].trim_start().trim_start_matches("- ");
            out.push(format!("{} {url}", &head[..head.len() - 1]));
            skip = true;
        } else {
            out.push(line.to_string());
        }
    }
    out.join("\n")
}

pub(super) fn collapse_single_child_generics(s: &str) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let mut keep: Vec<bool> = vec![true; lines.len()];
    for i in 0..lines.len() {
        let line = lines[i];
        let trim = line.trim_start();
        let indent = line.len() - trim.len();
        // `listitem` joins `generic`/`group` here: a list item holding exactly
        // one node says nothing the enclosing `- list:` has not already said,
        // and navigation menus are built almost entirely out of them (MEASURED:
        // 302 of them across the corpus, 1.9 % of everything that survives
        // compaction). An item with SEVERAL children keeps its wrapper — there
        // it is the only thing saying where one item ends and the next begins.
        if !trim.starts_with("- generic")
            && !trim.starts_with("- group")
            && !trim.starts_with("- listitem")
        {
            continue;
        }
        // Don't collapse a wrapper that has its own *name* — the agent loses
        // semantic context if we drop `- generic "main content":`.
        if trim.contains('"') {
            continue;
        }
        // Single child = next line is indented deeper AND has only one
        // sibling at the same level. Cheap proxy: next line indent == indent+2
        // AND line after next is at indent ≤ indent (or EOF).
        let next = match lines.get(i + 1) {
            Some(l) => l,
            None => continue,
        };
        let next_trim = next.trim_start();
        let next_indent = next.len() - next_trim.len();
        if next_indent != indent + 2 {
            continue;
        }
        let after_next = lines.get(i + 2);
        let after_next_indent = after_next
            .map(|l| l.len() - l.trim_start().len())
            .unwrap_or(0);
        if after_next.is_some() && after_next_indent > indent {
            continue; // wrapper has multiple descendants
        }
        // Drop the wrapper line — its single child carries everything.
        keep[i] = false;
    }
    lines
        .iter()
        .enumerate()
        .filter(|(i, _)| keep[*i])
        .map(|(_, l)| *l)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Pages embed machine-readable metadata in invisible spans — citation
/// managers (`ctx_ver=Z39.88-2004&rft_val_fmt=…`), analytics payloads, tracking
/// blobs. They surface as wrapper nodes carrying a single enormous name that no
/// human ever sees and no agent can act on. Un-naming them lets the wrapper
/// stage collect them, while anything nested underneath survives.
pub(super) fn drop_machine_metadata_names(s: &str) -> String {
    let wrappers = ["generic", "group", "none", "presentation"];
    s.lines()
        .map(|line| {
            let trim = line.trim_start();
            let Some(body) = trim.strip_prefix("- ") else {
                return line.to_string();
            };
            let role_end = body.find([' ', ':', '[']).unwrap_or(body.len());
            if !wrappers.contains(&&body[..role_end]) {
                return line.to_string();
            }
            match a11y_quoted_name(line) {
                Some(n) if is_machine_metadata(&n) => a11y_without_name(line),
                _ => line.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A long unbroken `key=value&key=value` run: a query string or an encoded
/// payload, never prose. The length floor keeps short technical labels
/// (`utm=1`, `id=4`) out of it.
pub(super) fn is_machine_metadata(name: &str) -> bool {
    name.len() >= 60 && !name.contains(' ') && name.contains('=') && name.matches('&').count() >= 2
}

/// An accessible name is COMPUTED from the subtree: a `row` is named by the
/// concatenation of its cells, a `button` by the text node inside it. Both the
/// name and the subtree are then in the snapshot, so every level of nesting
/// repeats the same words — on a table-based page the top row can carry the
/// text of the whole page, three levels deep.
///
/// Where the name is EXACTLY the concatenation of the subtree, one of the two
/// is pure duplication and dropping it loses nothing:
///   * subtree is nothing but text → drop the subtree, keep the name (the name
///     sits on the node an agent can act on);
///   * subtree carries structure (links, cells, refs of its own) → drop the
///     name, keep the subtree.
pub(super) fn fold_redundant_names(s: &str) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let indent: Vec<usize> = lines
        .iter()
        .map(|l| l.len() - l.trim_start().len())
        .collect();
    // Subtree end (exclusive) for each line.
    let mut end: Vec<usize> = vec![0; lines.len()];
    for i in 0..lines.len() {
        let mut j = i + 1;
        while j < lines.len() && (indent[j] > indent[i] || lines[j].trim().is_empty()) {
            j += 1;
        }
        end[i] = j;
    }
    let norm = |t: &str| -> String {
        t.chars()
            .filter(|c| !c.is_whitespace())
            .flat_map(|c| c.to_lowercase())
            .collect()
    };
    // Direct children only: a named child already stands for its own subtree,
    // so walking the whole subtree would count the same words at every level
    // and no name would ever match.
    let children = |i: usize| -> Vec<usize> {
        let mut kids = Vec::new();
        let mut j = i + 1;
        while j < end[i] {
            if lines[j].trim().is_empty() {
                j += 1;
                continue;
            }
            kids.push(j);
            j = end[j].max(j + 1);
        }
        kids
    };
    // Post-order: the text a node contributes to its parent's name.
    let mut text_of: Vec<String> = vec![String::new(); lines.len()];
    for i in (0..lines.len()).rev() {
        let t = lines[i].trim_start();
        if t.is_empty() || t.starts_with("- /") {
            continue;
        }
        if let Some(n) = a11y_quoted_name(lines[i]) {
            text_of[i] = n;
        } else if let Some(rest) = a11y_plain_text(t) {
            text_of[i] = rest.to_string();
        } else {
            text_of[i] = children(i).iter().map(|c| text_of[*c].as_str()).collect();
        }
    }
    let mut drop_line: Vec<bool> = vec![false; lines.len()];
    let mut strip_name: Vec<bool> = vec![false; lines.len()];
    for i in 0..lines.len() {
        let Some(name) = a11y_quoted_name(lines[i]) else {
            continue;
        };
        let kids = children(i);
        if kids.is_empty() {
            continue; // leaf: nothing to be redundant with
        }
        let sub: String = kids.iter().map(|c| text_of[*c].as_str()).collect();
        if sub.is_empty() || norm(&sub) != norm(&name) {
            continue;
        }
        let structural = kids.iter().any(|c| {
            let t = lines[*c].trim_start();
            a11y_plain_text(t).is_none()
        });
        if structural {
            strip_name[i] = true;
        } else {
            for c in kids {
                for d in drop_line.iter_mut().take(end[c]).skip(c) {
                    *d = true;
                }
            }
        }
    }
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        if drop_line[i] {
            continue;
        }
        if strip_name[i] {
            out.push(a11y_without_name(line));
        } else {
            out.push((*line).to_string());
        }
    }
    out.join("\n")
}
