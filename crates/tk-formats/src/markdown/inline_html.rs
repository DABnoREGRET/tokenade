//! **HTML in the middle of markdown.**
//!
//! A markdown document always contains some: decorative tags, whole
//! blocks, images. Some are removed, others summarised, and
//! an image becomes a mark — the distinction is written here, once, instead
//! of being guessed at every call.

use super::*;

/// Is this an HTML element name, rather than a placeholder that happens to
/// wear angle brackets?
///
/// Deliberately a closed list. The alternative — "anything shaped like a tag
/// is a tag" — is what deleted `<hash>`, and markdown's own rule (arbitrary
/// inline HTML is allowed) cannot settle it either, because a document about a
/// CLI carries far more placeholders than raw tags. An element missing from
/// this list survives as literal text, which is the harmless direction.
pub(super) fn is_html_element(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    // `<div class=x>` and `<Foo />` are shaped like tags; `<hash>` is not.
    matches!(
        name.to_ascii_lowercase().as_str(),
        "a" | "abbr"
            | "address"
            | "area"
            | "article"
            | "aside"
            | "audio"
            | "b"
            | "base"
            | "bdi"
            | "bdo"
            | "blockquote"
            | "body"
            | "br"
            | "button"
            | "canvas"
            | "caption"
            | "cite"
            | "code"
            | "col"
            | "colgroup"
            | "data"
            | "datalist"
            | "dd"
            | "del"
            | "details"
            | "dfn"
            | "dialog"
            | "div"
            | "dl"
            | "dt"
            | "em"
            | "embed"
            | "fieldset"
            | "figcaption"
            | "figure"
            | "footer"
            | "form"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "head"
            | "header"
            | "hgroup"
            | "hr"
            | "html"
            | "i"
            | "iframe"
            | "img"
            | "input"
            | "ins"
            | "kbd"
            | "label"
            | "legend"
            | "li"
            | "link"
            | "main"
            | "map"
            | "mark"
            | "menu"
            | "meta"
            | "meter"
            | "nav"
            | "noscript"
            | "object"
            | "ol"
            | "optgroup"
            | "option"
            | "output"
            | "p"
            | "param"
            | "picture"
            | "pre"
            | "progress"
            | "q"
            | "rp"
            | "rt"
            | "ruby"
            | "s"
            | "samp"
            | "script"
            | "section"
            | "select"
            | "slot"
            | "small"
            | "source"
            | "span"
            | "strong"
            | "style"
            | "sub"
            | "summary"
            | "sup"
            | "svg"
            | "table"
            | "tbody"
            | "td"
            | "template"
            | "textarea"
            | "tfoot"
            | "th"
            | "thead"
            | "time"
            | "title"
            | "tr"
            | "track"
            | "u"
            | "ul"
            | "var"
            | "video"
            | "wbr"
    )
}

pub(super) fn strip_inline_html(line: &str) -> String {
    if !line.contains('<') {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len());
    let mut depth_skip = 0i32;
    // Inside a backtick span nothing is markup — that is what a code span
    // MEANS. `tokenade expand-ref <hash>` came back as `tokenade expand-ref `,
    // so an agent reading a CLI document got a command with its argument
    // silently removed. Same family as the byte-as-char class: a compactor
    // altering content instead of shortening it, with nothing saying so.
    let mut in_code = false;
    // Walk CHARACTERS. The previous cursor stepped one byte at a time and
    // emitted `bytes[i] as char`, which turns every byte of a multibyte
    // character into its own Latin-1 codepoint: one inline `<a>` or `<sub>` in
    // a Korean, Japanese, Greek or accented-French line was enough to hand the
    // agent mojibake for the whole line. Nothing said the text had been
    // damaged, so it read as the document.
    let mut i = 0;
    while i < line.len() {
        let rest = &line[i..];
        if rest.starts_with('`') {
            in_code = !in_code;
            if depth_skip == 0 {
                out.push('`');
            }
            i += 1;
            continue;
        }
        if !in_code && rest.starts_with('<') {
            if let Some(end) = rest.find('>') {
                let tag = &rest[1..end];
                // Both slashes: `</p>` closes, `<br/>` self-closes. Only the
                // leading one was trimmed, so `<br/>` produced the name `br/`.
                // That was invisible while an unrecognised name was DELETED —
                // it fell into the same drop as a placeholder and looked right.
                // Once unrecognised means "text", the missed trim would have
                // let `<br/>` survive as literal markup.
                let tag_name = tag
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .trim_start_matches('/')
                    .trim_end_matches('/');
                // Self-closing or known void elements: drop entirely.
                // The list of void elements lives in `html_nesting`, which
                // is the module that knows what an HTML element is. The one that
                // was written here carried eight out of fourteen.
                let void = crate::html_nesting::VOID;
                let inline_keep_text = [
                    "p", "a", "sub", "sup", "span", "em", "strong", "code", "b", "i", "u", "small",
                    "del", "ins", "mark", "kbd",
                ];
                // `<hash>`, `<pattern>`, `<file>`, `<N>` — the universal
                // CLI-documentation idiom, and not HTML. They matched none of
                // the branches below, fell past every one of them, and were
                // dropped by the `i += end + 1` that follows: deleted for not
                // being recognised. An unrecognised angle-bracket token is
                // TEXT until something says otherwise; a stray literal tag
                // surviving costs a few characters, a deleted placeholder
                // changes what the document says.
                if !is_html_element(tag_name) {
                    if depth_skip == 0 {
                        out.push_str(&rest[..=end]);
                    }
                    i += end + 1;
                    continue;
                }
                if void.contains(&tag_name) {
                    // drop
                } else if inline_keep_text.contains(&tag_name) {
                    // drop the tag itself, keep its inner content
                } else if depth_skip > 0 {
                    if tag.starts_with('/') {
                        depth_skip -= 1;
                    } else {
                        depth_skip += 1;
                    }
                }
                i += end + 1;
                continue;
            }
        }
        let c = rest.chars().next().expect("i is on a char boundary");
        if depth_skip == 0 {
            out.push(c);
        }
        i += c.len_utf8();
    }
    out
}

/// **The mark an agent reads as "there was an image here".**
///
/// Written in ONE place because both halves of the product must speak the
/// same language about the same thing: the web route already set it — three sites
/// each composed it with their own `format!` — and the DOCUMENTS route did not set it
/// at all, although Word, PowerPoint, OpenDocument and an EPUB's XHTML
/// all carry the alt text the author typed.
///
/// Without alt text, the bare mark: it says there was an image
/// and asserts nothing about its content (a mark that ASSERTS must be
/// conditioned on its proof).
pub fn image_marker(alt: Option<&str>) -> String {
    match alt.map(str::trim).filter(|a| !a.is_empty()) {
        Some(a) => format!("[image: {a}]"),
        None => "[image]".to_string(),
    }
}

/// Convert every `![alt](url)` occurrence on the line to `[image: alt]`,
/// without pulling in a regex (cheap string scan).
pub(super) fn transform_inline_images(line: &str) -> String {
    if !line.contains("![") {
        return line.to_string();
    }
    // Character cursor, not a byte cursor: see `strip_inline_html`. Emitting
    // `bytes[i] as char` mangled every non-ASCII character on any line that
    // carried an inline image.
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < line.len() {
        let rest = &line[i..];
        if rest.starts_with("![") {
            // Find the closing `]` for the alt text.
            if let Some(rb) = line[i + 2..].find(']') {
                let alt = &line[i + 2..i + 2 + rb];
                let after_rb = i + 2 + rb + 1;
                // Find the matching `)` for the url.
                if line[after_rb..].starts_with('(') {
                    if let Some(rp) = line[after_rb + 1..].find(')') {
                        let trimmed_alt = alt.trim();
                        if trimmed_alt.is_empty() {
                            // No useful alt — drop the image marker entirely.
                        } else {
                            let _ = write!(out, "[image: {trimmed_alt}]");
                        }
                        i = after_rb + 1 + rp + 1;
                        continue;
                    }
                }
            }
        }
        let c = rest.chars().next().expect("i is on a char boundary");
        out.push(c);
        i += c.len_utf8();
    }
    out
}
