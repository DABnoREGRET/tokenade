//! One markdown converter per HTML element.
//!
//! Everything here answers the same question — how does ONE tag family render
//! as markdown — and nothing here decides WHAT to convert; that stays in the
//! pipeline. `use super::*` reaches the shared `re()` cache and `TAG_ATTRS`.

use super::*;

pub(super) fn headings_to_md(s: &str) -> String {
    let mut out = s.to_string();
    for level in 1..=6 {
        let prefix = "#".repeat(level);
        let r = re(&format!(r"(?is)<h{level}\b[^>]*>(.*?)</h{level}>"));
        out = r
            .replace_all(&out, |c: &regex::Captures| {
                let inner = strip_all_tags(c.get(1).unwrap().as_str())
                    .trim()
                    .to_string();
                format!("\n\n{prefix} {inner}\n\n")
            })
            .into_owned();
    }
    out
}

/// `<dt>`/`<dd>` → heading + body.
///
/// A definition list is how reference documentation is actually built: a
/// glossary term and its meaning, a function signature and its docs, a module
/// name and its summary. docs.python.org/3/glossary is 176 `<dt>` pairs and
/// ZERO headings, so the outline folder — which folds by heading hierarchy —
/// saw one flat section and served `# Glossary (1894 lines)` plus a stash
/// pointer. Every term was behind a second call.
///
/// `<dt>` is promoted because the TAG says it is a term being defined, not
/// because its text scores a certain way. That keeps the decision on the
/// container, where this codebase has repeatedly found the only reliable
/// signal.
pub(super) fn definition_list_to_md(s: &str) -> String {
    let dt = re(r"(?is)<dt\b[^>]*>(.*?)</dt>");
    let out = dt.replace_all(s, |c: &regex::Captures| {
        let inner = c
            .get(1)
            .map(|m| m.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if inner.is_empty() {
            String::new()
        } else {
            format!("\n#### {inner}\n")
        }
    });
    let dd = re(r"(?is)<dd\b[^>]*>(.*?)</dd>");
    dd.replace_all(&out, |c: &regex::Captures| {
        format!("\n{}\n", c.get(1).map(|m| m.as_str()).unwrap_or("").trim())
    })
    .into_owned()
}

pub(super) fn list_to_md(s: &str) -> String {
    // <li>…</li>  →  "- …\n"
    let li = re(r"(?is)<li\b[^>]*>(.*?)</li>");
    let mut out = li
        .replace_all(s, |c: &regex::Captures| {
            let inner = c.get(1).unwrap().as_str();
            format!("- {inner}\n")
        })
        .into_owned();
    // Drop the surrounding <ul>/<ol> tags (the dashes already give markdown semantics).
    for tag in ["ul", "ol"] {
        let r = re(&format!(r"(?is)</?{tag}\b[^>]*>"));
        out = r.replace_all(&out, "").into_owned();
    }
    out
}

/// A `<br>` inside code is a line break, and `strip_all_tags` deletes it with
/// nothing in its place: `<code>make all<br>make install</code>` came out as
/// `make allmake install`, a word the page never wrote — and a `<pre>` written
/// with `<br>` lost every line break. A block keeps the newline; an inline span,
/// which cannot hold one, gets the space a browser shows (as `web_tree` does).
fn breaks_as(s: &str, sep: &str) -> String {
    re(r"(?is)<br\s*/?>").replace_all(s, sep).into_owned()
}

pub(super) fn pre_code_to_md(s: &str) -> String {
    // <pre>…</pre> (with or without <code> inside) → ```lang\n…\n```
    let r = re(r"(?is)<pre\b([^>]*)>(.*?)</pre>");
    r.replace_all(s, |c: &regex::Captures| {
        let pre_attrs = c.get(1).map_or("", |m| m.as_str());
        let inner = c.get(2).unwrap().as_str();
        // essence's fence-language detection (MIT — see module doc): a
        // `class="language-rust"` on `<pre>` itself, or on an IMMEDIATE
        // `<code>` child (the common highlight.js/Prism shape), becomes the
        // fence's info string instead of a bare ` ``` `.
        let lang = fence_lang_hint(pre_attrs)
            .or_else(|| {
                re(r#"(?is)^\s*<code\b([^>]*)>"#)
                    .captures(inner)
                    .and_then(|cc| fence_lang_hint(cc.get(1).map_or("", |m| m.as_str())))
            })
            .unwrap_or_default();
        // essence's gutter/line-number stripping (MIT — see module doc):
        // `strip_all_tags` alone keeps a highlighter's line-number gutter as
        // literal digits glued onto the code, since it drops the WRAPPER tag
        // but not the wrapper's own text content.
        let inner = strip_code_gutters(inner);
        // Decode AFTER stripping tags, like every other text path here: a
        // source `&lt;div&gt;` must reach the agent as `<div>`, and decoding
        // first would hand `strip_all_tags` a tag it would then delete.
        // MEASURED 2026-07-22 on npmjs/package/react: the JSX sample shipped as
        // `&lt;h1&gt;{count}&lt;/h1&gt;` — code an agent cannot copy and run.
        let cleaned = decode_entities(&strip_all_tags(&breaks_as(&inner, "\n")));
        format!("\n\n```{lang}\n{}\n```\n\n", cleaned.trim())
    })
    .into_owned()
}

/// `class="language-rust"` / `lang-python` / `highlight-go` → `Some("rust")`
/// etc. essence's fence-language regex (MIT — see module doc), unchanged:
/// `\b(?:language|lang|highlight)-([a-zA-Z0-9_+-]+)`.
pub(super) fn fence_lang_hint(tag_attrs: &str) -> Option<String> {
    // **The attribute value may NOT be quoted.** HTML allows it and
    // Hugo/Chroma writes it that way: `<code class=language-yaml
    // data-lang=yaml>`. `ATTR_VALUE` only knows the two quoting styles, so the
    // language of **39 blocks** of the Kubernetes documentation was lost.
    let caps = re(r#"(?is)\bclass\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))"#).captures(tag_attrs)?;
    let class = (1..=3)
        .find_map(|i| caps.get(i).map(|m| m.as_str()))?
        .to_string();
    language_of_class(&class)
}

/// The language named by a list of classes, whatever the convention.
///
/// Three families, each carried by a named generator: `language-` and `lang-`
/// (Prism, highlight.js), `highlight-` (Pygments, hence Sphinx and everything
/// built on it) and `brush:` (SyntaxHighlighter, which MDN still writes).
/// MEASURED 2026-09-10 on the 39 pages: the third accounts for 4 blocks on
/// `mdn_doc`, the first two for 127.
pub(super) fn language_of_class(class: &str) -> Option<String> {
    // The prefixes STACK: MediaWiki writes `mw-highlight mw-highlight-lang-rust`,
    // and consuming only the first one rendered ` ```lang-rust `, a language
    // that does not exist. The group is therefore repeated.
    if let Some(c) = re(r"\b(?:(?:language|lang|highlight)-)+([a-zA-Z0-9_+#-]+)").captures(class) {
        let lang = c[1].to_string();
        // **`default` is not a language.** Sphinx writes `highlight-default`
        // when the directive names none: announcing it would be a false
        // CLAIM, worse than a bare fence — 37 blocks in the Django
        // documentation. Same for the synonyms of "nothing".
        if matches!(lang.as_str(), "default" | "none" | "auto" | "nohighlight") {
            return None;
        }
        return Some(lang);
    }
    re(r"(?i)\bbrush\s*:\s*([a-zA-Z0-9_+#-]+)")
        .captures(class)
        .map(|c| c[1].to_string())
}

/// Strip a highlighter's line-number gutter WHOLESALE (tag and text both) —
/// essence's technique (MIT — see module doc), scoped to the wrapper shapes
/// actually seen in the wild (`<span>`/`<td>`/`<div>`; `regex` has no
/// backreferences, so the closing tag can't be tied generically to whichever
/// of the three opened it, see module doc's Rust-specific-craft note).
pub(super) fn strip_code_gutters(s: &str) -> String {
    let mut out = s.to_string();
    for tag in ["span", "td", "div"] {
        let r = re(&format!(
            r#"(?is)<{tag}\b[^>]*\bclass\s*=\s*["'][^"']*(?:gutter|line-number|hljs-ln-numbers|blob-num)[^"']*["'][^>]*>.*?</{tag}>"#
        ));
        out = r.replace_all(&out, "").into_owned();
    }
    out
}

pub(super) fn inline_code_to_md(s: &str) -> String {
    let r = re(r"(?is)<code\b[^>]*>(.*?)</code>");
    r.replace_all(s, |c: &regex::Captures| {
        // Same decode-after-strip rule as the fenced path above: `<code>&amp;</code>`
        // is an ampersand, not the four characters `&amp;`.
        let inner = decode_entities(&strip_all_tags(&breaks_as(c.get(1).unwrap().as_str(), " ")));
        format!("`{}`", inner.trim())
    })
    .into_owned()
}

pub(super) fn links_to_md(s: &str) -> String {
    // <a href="X">Y</a> → [Y](X) — but if the text equals the URL or Y is empty, just keep Y.
    let r = re(&format!(
        r#"(?is)<a\b[^>]*\bhref\s*=\s*{ATTR_VALUE}[^>]*>(.*?)</a>"#
    ));
    r.replace_all(s, |c: &regex::Captures| {
        let href = quoted_value(c, 1).unwrap_or("").trim();
        // A markdown link label cannot contain a newline — the link simply
        // stops parsing at it. News cards nest a heading inside the anchor, and
        // `</h2>` emits a line break, so the label arrived
        // split across lines and 225 links on the corpus were left as the
        // fragment `[## Trump slaps 50% tariffs on Canada…` with no closing
        // bracket and no URL. Collapse the label's whitespace to spaces.
        // Group 3: `ATTR_VALUE` spends groups 1 and 2 on its two quotings.
        let text = strip_all_tags(c.get(3).unwrap().as_str())
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if text.is_empty() {
            String::new()
        } else if text == href || href.starts_with('#') {
            text
        } else {
            md_link(&text, href)
        }
    })
    .into_owned()
}

pub(super) fn blockquote_to_md(s: &str) -> String {
    let r = re(r"(?is)<blockquote\b[^>]*>(.*?)</blockquote>");
    r.replace_all(s, |c: &regex::Captures| {
        let inner = strip_all_tags(c.get(1).unwrap().as_str());
        let quoted: Vec<String> = inner
            .lines()
            .map(|l| format!("> {}", l.trim()))
            .filter(|l| l != "> ")
            .collect();
        format!("\n\n{}\n\n", quoted.join("\n"))
    })
    .into_owned()
}

pub(super) fn paragraph_breaks(s: &str) -> String {
    // <p>X</p>  →  "\n\nX\n\n"
    let r = re(r"(?is)<p\b[^>]*>(.*?)</p>");
    let s = r
        .replace_all(s, |c: &regex::Captures| {
            format!("\n\n{}\n\n", c.get(1).unwrap().as_str())
        })
        .into_owned();
    // <br/>  →  newline
    let s = re(r"(?is)<br\s*/?>").replace_all(&s, "\n").into_owned();
    // Every OTHER block-level element closes with a newline too.
    //
    // Without this, block separation came from whatever whitespace the author
    // happened to leave between the tags — which is to say, from nothing at all
    // on a minified page. `news.ycombinator.com/item?id=1` is one line of HTML,
    // and its comment thread arrived as
    // `…eating lunch on SHR[pg](user?id=pg) [on Oct 9, 2006]…`: the end of one
    // comment welded to the byline of the next, with no way for a reader to
    // tell where either begins. HN reaches the generic strip because its
    // layout tables are nested, and `tables_to_md` deliberately declines those.
    //
    // Only closing tags, and only block-level ones — `</a>`, `</span>`,
    // `</strong>` must keep flowing. `collapse_whitespace` folds the runs this
    // creates, so the cost on a page that was already well-formed is zero.
    re(r"(?is)</(?:div|tr|li|section|article|header|footer|h[1-6]|blockquote|pre|dd|dt|dl|ul|ol|table|td|th|nav|aside|figure|figcaption|main|form|address|details|summary)\s*>")
        .replace_all(&s, "\n${0}")
        .into_owned()
}

pub(super) fn images_to_md_alt(s: &str) -> String {
    // essence's tracking-pixel removal (MIT — see module doc): a 1×1 analytics
    // beacon sometimes carries a plausible-looking `alt`, which would
    // otherwise survive as a `[image: …]` line below. Matched by filename
    // regardless of alt text, since the filename IS the signal.
    let s = re(r#"(?is)<img\b[^>]*\bsrc\s*=\s*["'][^"']*(?:s_1x2\.gif|pixel\.gif|spacer\.gif|blank\.gif|clear\.gif)[^"']*["'][^>]*/?>"#)
        .replace_all(s, "")
        .into_owned();
    let s = s.as_str();
    // Keep only the alt text; images themselves are useless to a text LLM.
    let r = re(&format!(
        r#"(?is)<img\b[^>]*\balt\s*=\s*{ATTR_VALUE}[^>]*/?>"#
    ));
    let s = r
        .replace_all(s, |c: &regex::Captures| {
            let alt = quoted_value(c, 1).unwrap_or("").trim();
            if alt.is_empty() {
                String::new()
            } else {
                // The marker fits on ITS line, as on the tree path: an `alt`
                // can carry line breaks.
                format!("[image: {}]", crate::web_tree::one_line(alt))
            }
        })
        .into_owned();
    // Drop images without alt entirely.
    re(r"(?is)<img\b[^>]*/?>").replace_all(&s, "").into_owned()
}

/// Convert simple (non-nested) `<table>`s to markdown pipe tables — cell
/// structure is answer-bearing data the flat tag-strip used to smear into
/// one run-on line. Nested tables (layout tables, wiki infoboxes) are left
/// untouched and fall through to the generic tag strip: a wrong regex slice
/// there would garble content, and flowing text is the safer degradation.
/// Collapse SOURCE newlines inside elements whose content is inline.
///
/// A newline in HTML is whitespace, not a line break — but the pipeline was
/// carrying it through, so wherever a generator wrapped its markup we emitted a
/// hard break in the middle of a sentence. `man.openbsd.org/ssh` writes
///
/// ```text
/// <p class="Pp"><code class="Nm">ssh</code> &#x2014;
///     <span class="Nd">OpenSSH remote login client</span></p>
/// ```
///
/// and the NAME section arrived as two lines, so the outline previewed it as
/// `` `ssh` — `` and stopped: the one line of a manual page that says what the
/// command IS. mandoc renders every OpenBSD, FreeBSD and illumos manual this
/// way.
///
/// Scoped to the elements that CANNOT contain a block: `<p>`, headings, `<li>`,
/// `<td>`/`<th>`, `<dt>`/`<dd>`. That is what keeps it away from `<pre>` (where
/// newlines are the content) and away from plain-text input (no tags, nothing
/// matches) — a blanket collapse would have flattened every RFC and Gutenberg
/// text this loop just taught the outline to read.
pub(super) fn unwrap_inline_source_newlines(s: &str) -> String {
    // One pattern per tag: the `regex` crate has no backreferences, so
    // `<(p|li|…)>…</\1>` is not expressible.
    const INLINE_CONTENT: [&str; 12] = [
        "p", "h1", "h2", "h3", "h4", "h5", "h6", "li", "td", "th", "dt", "dd",
    ];
    let ws = re(r"\s*\n\s*");
    let mut out = s.to_string();
    for tag in INLINE_CONTENT {
        let re_el = re(&format!(r"(?is)<{tag}\b[^>]*>(.*?)</{tag}>"));
        out = re_el
            .replace_all(&out, |c: &regex::Captures| {
                let inner = c.get(1).map(|m| m.as_str()).unwrap_or("");
                if !inner.contains('\n') {
                    return c[0].to_string();
                }
                let whole = c.get(0).unwrap().as_str();
                whole.replacen(inner, &ws.replace_all(inner, " "), 1)
            })
            .into_owned();
    }
    out
}

/// Render one inline markdown link so the DESTINATION survives a parser.
///
/// Both converters used to interpolate the raw href, which silently produces a
/// link nobody can follow: a space, a `<`, or a `)` inside a URL ends the
/// destination early, so the agent is handed a TRUNCATED address and no sign
/// that it was cut. A `]` in the label ends the label early with the same
/// result. Neither is exotic — query strings carry all of them.
///
/// The destination is never rewritten, only delimited: whenever it holds a
/// character that could end it, it is wrapped in the angle-bracket form
/// CommonMark provides for exactly this (`[t](<dest with space>)`), with any
/// literal `<`/`>` backslash-escaped inside. Percent-encoding instead would
/// change the URL, which is a different bug.
pub(crate) fn md_link(label: &str, href: &str) -> String {
    let label = label.replace('[', "\\[").replace(']', "\\]");
    let needs_wrap = href
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, '<' | '>' | '(' | ')'));
    if !needs_wrap {
        return format!("[{label}]({href})");
    }
    let dest = href.replace('<', "\\<").replace('>', "\\>");
    format!("[{label}](<{dest}>)")
}

/// `align="…"` / `style="text-align:…"` on a `<th>`/`<td>` opening tag, if
/// any — read from the header row only, to build essence's four separator
/// variants (` --- ` / ` :--- ` / ` ---: ` / ` :---: `) instead of flattening
/// every column to the unaligned default. Zero extra tokens (1-2 chars per
/// separator cell) for information the source markup already carried and we
/// were discarding.
pub(crate) fn cell_align_marker(attrs: &str) -> &'static str {
    let a = attrs.to_ascii_lowercase();
    let dir = re(r#"align\s*=\s*["']?\s*(left|right|center)"#)
        .captures(&a)
        .or_else(|| re(r"text-align\s*:\s*(left|right|center)").captures(&a))
        .map(|c| c.get(1).unwrap().as_str().to_string());
    match dir.as_deref() {
        Some("left") => ":---",
        Some("right") => "---:",
        Some("center") => ":---:",
        _ => "---",
    }
}

pub(super) fn tables_to_md(s: &str) -> String {
    let table_re = re(r"(?is)<table\b[^>]*>(.*?)</table>");
    let row_re = re(r"(?is)<tr\b[^>]*>(.*?)</tr>");
    // Group 1 is the cell's own opening-tag attributes (for alignment on the
    // header row); group 2 is the inner content.
    let cell_re = re(r"(?is)<t[hd]\b([^>]*)>(.*?)</t[hd]>");
    table_re
        .replace_all(s, |c: &regex::Captures| {
            let inner = c.get(1).unwrap().as_str();
            if inner.contains("<table") {
                // Nested/layout table — leave the original bytes alone.
                return c.get(0).unwrap().as_str().to_string();
            }
            let mut rows: Vec<Vec<String>> = Vec::new();
            let mut aligns: Vec<&'static str> = Vec::new();
            for r in row_re.captures_iter(inner) {
                let mut cells: Vec<String> = Vec::new();
                let mut row_aligns: Vec<&'static str> = Vec::new();
                for cc in cell_re.captures_iter(r.get(1).unwrap().as_str()) {
                    row_aligns.push(cell_align_marker(cc.get(1).unwrap().as_str()));
                    cells.push(crate::web_tree::escape_pipes(
                        &strip_all_tags(cc.get(2).unwrap().as_str())
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" "),
                    ));
                }
                if cells.iter().any(|c| !c.is_empty()) {
                    // First surviving row is the header — its alignment wins.
                    if rows.is_empty() {
                        aligns = row_aligns;
                    }
                    rows.push(cells);
                }
            }
            if rows.len() < 2 {
                // Not really tabular — let the generic strip handle it.
                return c.get(0).unwrap().as_str().to_string();
            }
            let mut out = String::from("\n\n");
            // The caption, as on the tree path: it is the only place where a
            // page names its table, and markdown has no caption of its own.
            // Written here TOO, because "proven on one consumer, absent from
            // its sibling" is the most frequent mistake in this repository.
            if let Some(cap) = re(r"(?is)<caption\b[^>]*>(.*?)</caption>").captures(inner) {
                let text = decode_entities(&strip_all_tags(cap.get(1).unwrap().as_str()));
                let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
                if !text.is_empty() {
                    out.push_str(&format!("*{text}*\n\n"));
                }
            }
            for (i, row) in rows.iter().enumerate() {
                out.push_str(&format!("| {} |\n", row.join(" | ")));
                if i == 0 {
                    let seps: Vec<&str> = (0..row.len())
                        .map(|col| aligns.get(col).copied().unwrap_or("---"))
                        .collect();
                    out.push_str(&format!("| {} |\n", seps.join(" | ")));
                }
            }
            out.push('\n');
            out
        })
        .into_owned()
}

#[cfg(test)]
mod the_legacy_converter_names_its_tables_too {
    use super::tables_to_md;

    /// "Proven on one consumer, absent from its sibling" is the most frequent
    /// mistake in this repository: the caption is written on BOTH paths, the
    /// tree and this one.
    #[test]
    fn a_caption_reaches_the_regex_path_as_well() {
        let out = tables_to_md(
            "<table><caption>Ventes trimestrielles par region</caption>\
             <tr><th>Region</th><th>Ventes</th></tr><tr><td>Nord</td><td>12</td></tr></table>",
        );
        assert!(
            out.contains("*Ventes trimestrielles par region*"),
            "caption lost: {out}"
        );
        assert!(out.contains("| Region | Ventes |"), "{out}");
    }

    #[test]
    fn a_table_without_a_caption_gains_nothing_here_either() {
        let out = tables_to_md(
            "<table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table>",
        );
        assert!(!out.contains('*'), "a caption was invented: {out}");
    }
}

#[cfg(test)]
mod a_break_inside_code_is_not_deleted {
    use super::{inline_code_to_md, pre_code_to_md};

    /// html-to-markdown-rs splits an inline span on `<br>`; here
    /// `strip_all_tags` deleted it, welding `make allmake install`.
    #[test]
    fn a_break_in_an_inline_span_is_a_space_and_in_a_block_a_newline() {
        assert_eq!(
            inline_code_to_md("<code>make all<br>make install</code>"),
            "`make all make install`"
        );
        let block = pre_code_to_md("<pre><code>first()<br/>second()</code></pre>");
        assert!(block.contains("first()\nsecond()"), "{block}");
    }
}
