//! Code samples, kept intact through the noise strip.
//!
//! A `<title>` or `<script>` INSIDE a documented code sample must survive the
//! pass that deletes the real elements, so fences are stashed before it and
//! restored after.

use super::*;

/// Code-fence sanctuary (essence, MIT — adapted). Swap every `<pre>…</pre>`
/// block and standalone `<code>…</code>` span for a NUL-delimited placeholder
/// BEFORE any noise/boilerplate stripping runs, so those regex passes cannot
/// tell "a real `<title>`/`<script>` element" from "the same bytes sitting
/// inside a documented code sample". Restored via [`restore_code_blocks`]
/// once the sample is safely past every pass that would otherwise eat it —
/// the noise-tag strip, negative-class boilerplate stripping, and the
/// markdown-side link/heading cleanups, all of which run before the ordinary
/// `pre_code_to_md`/`inline_code_to_md` conversion ever sees the tag.
///
/// `<pre>` is stashed first (with its `<code>` still inside, if any); the
/// second pass only sees leftover standalone `<code>` spans. Returns the
/// substituted HTML plus the stash to hand to [`restore_code_blocks`].
pub(super) fn sanctuary_code_blocks(html: &str) -> (String, Vec<String>) {
    let mut stash: Vec<String> = Vec::new();
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let hoisted = hoist_wrapper_language(html);
    let html: &str = &hoisted;
    let pre = re(r"(?is)<pre\b[^>]*>.*?</pre>");
    let stage1 = stash_matches(html, pre, true, &mut stash, &mut seen);
    let code = re(r"(?is)<code\b[^>]*>.*?</code>");
    let stage2 = stash_matches(&stage1, code, false, &mut stash, &mut seen);
    (stage2, stash)
}

/// **Pygments writes the language on the WRAPPER, not on the block.**
///
/// Sphinx — hence the Django documentation, the Python glossary, a good part
/// of MediaWiki — renders `<div class="highlight-python notranslate"><div
/// class="highlight"><pre>`. The `<pre>` itself carries nothing, and it is
/// what the sanctuary takes away: the language stayed outside. MEASURED
/// 2026-09-10 on the 39 pages: **84 blocks** in that case (django 53,
/// py_glossary 21, wikipedia 6, k8s 4).
///
/// The language is therefore copied onto the `<pre>` BEFORE it is set aside.
/// Two bounds, because a window of text is not a tree:
///  * the wrapper must be less than 300 characters away — beyond that it is
///    no longer a wrapper;
///  * nothing may CLOSE between it and the block (`</`), otherwise it closed
///    earlier and is about something else — that is what keeps the previous
///    block from lending its language to the next one.
///
/// A `<pre>` that already names its language keeps its own.
fn hoist_wrapper_language(html: &str) -> String {
    const WINDOW: usize = 300;
    if !html.contains("<pre") {
        return html.to_string();
    }
    let class = re(r#"(?is)\bclass\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))"#);
    let opener = re(r"(?is)<pre\b[^>]*>");
    let mut out = String::with_capacity(html.len());
    let mut last = 0usize;
    for m in opener.find_iter(html) {
        let tag = m.as_str();
        // Already named, on the block or on its immediate `<code>`.
        let inner_code = re(r"(?is)^\s*<code\b([^>]*)>")
            .captures(&html[m.end()..(m.end() + 200).min(html.len())])
            .and_then(|c| c.get(1).map(|g| g.as_str().to_string()))
            .unwrap_or_default();
        if super::elements::fence_lang_hint(tag).is_some()
            || super::elements::fence_lang_hint(&inner_code).is_some()
        {
            continue;
        }
        let from = m.start().saturating_sub(WINDOW);
        let window = &html[from..m.start()];
        let found: Vec<(usize, String)> = class
            .captures_iter(window)
            .filter_map(|c| {
                let v = (1..=3).find_map(|i| c.get(i))?;
                let l = super::elements::language_of_class(v.as_str())?;
                Some((c.get(0)?.end(), l))
            })
            .collect();
        let Some((end, lang)) = found.last() else {
            continue;
        };
        let (end, lang) = (*end, lang.as_str());
        if window[end..].contains("</") {
            continue;
        }
        out.push_str(&html[last..m.start()]);
        out.push_str("<pre class=\"language-");
        out.push_str(lang);
        out.push('"');
        out.push_str(&tag["<pre".len()..]);
        last = m.end();
    }
    out.push_str(&html[last..]);
    out
}

/// One stashing pass: every match of `pat` in `input` is pushed to `stash`
/// and replaced by `\x00CODE_FENCE_{i}\x00` (surrounded by blank lines for a
/// block-level match, inline otherwise).
///
/// Identical matches share ONE slot. Not an optimisation: sibling de-duplication
/// hashes what a subtree renders, and giving two byte-identical code samples the
/// placeholders `CODE_FENCE_3` and `CODE_FENCE_4` made them read as different
/// content, so a card grid repeating one sample escaped the collapse it used to
/// get when `dom_dedup` ran on the raw bytes. `restore_code_blocks` maps index
/// to text, so reusing an index restores the same block in both places.
pub(super) fn stash_matches(
    input: &str,
    pat: &Regex,
    block: bool,
    stash: &mut Vec<String>,
    seen: &mut std::collections::HashMap<String, usize>,
) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(m) = pat.find(rest) {
        out.push_str(&rest[..m.start()]);
        let idx = match seen.get(m.as_str()) {
            Some(i) => *i,
            None => {
                let i = stash.len();
                stash.push(m.as_str().to_string());
                seen.insert(m.as_str().to_string(), i);
                i
            }
        };
        if block {
            let _ = write!(out, "\n\n\x00CODE_FENCE_{idx}\x00\n\n");
        } else {
            let _ = write!(out, "\x00CODE_FENCE_{idx}\x00");
        }
        rest = &rest[m.end()..];
    }
    out.push_str(rest);
    out
}

/// Restore placeholders left by [`sanctuary_code_blocks`], converting each
/// stashed `<pre>`/`<code>` snippet through the normal converters exactly as
/// if the noise/boilerplate passes had never seen it. A no-op (no allocation)
/// when nothing was stashed, which is the common case.
pub(super) fn restore_code_blocks(s: &str, stash: &[String]) -> String {
    if stash.is_empty() || !s.as_bytes().contains(&0) {
        return s.to_string();
    }
    let mut out = s.to_string();
    for (i, raw) in stash.iter().enumerate() {
        let placeholder = format!("\x00CODE_FENCE_{i}\x00");
        if !out.contains(placeholder.as_str()) {
            continue;
        }
        let converted = if raw.len() >= 4 && raw.as_bytes()[..4].eq_ignore_ascii_case(b"<pre") {
            // A fence MUST own its line. The placeholder usually sits mid-
            // paragraph, so trimming the converter's own blank lines glued the
            // closing fence to the prose that followed it — MEASURED 2026-07-22
            // on docs.npmjs.com/cli/v10/commands/npm-install, which emitted
            // ```` ``` `npm install` saves … Examples: ```bash ```` on ONE line.
            // That is malformed markdown, and any fence-aware reader loses
            // parity there: `markdown::outline` then treated the rest of the
            // page as code and silently dropped 5 of its 7 sections from the
            // fold, so the agent could not even ask for what it could not see.
            // `collapse_whitespace` squeezes the blank runs afterwards, so
            // padding here costs nothing.
            let fenced = pre_code_to_md(raw).trim().to_string();
            // …except on a TABLE ROW, where a fence cannot sit: `<td><pre>`
            // came back as `| Ship | [ ] | ` then a fence then ` |` — the row
            // broken in three (2026-10-04, after html-to-markdown-rs `preserve
            // preformatted cell break structure`). There each code line is a
            // span, the lines joined by `<br>` as a cell writes them.
            let lines: Vec<&str> = out.split('\n').collect();
            out = lines
                .iter()
                .enumerate()
                .map(|(i, &line)| {
                    if line.contains(placeholder.as_str()) && line.trim_start().starts_with('|') {
                        line.replace(placeholder.as_str(), &pre_in_cell(&fenced))
                    } else if let Some(col) = list_item_content_column(line)
                        .filter(|_| line.contains(placeholder.as_str()))
                        // …or on a CONTINUATION line of an item (a second
                        // paragraph or `<div>` of it), at that line's indent:
                        // django's `<li><p>…</p><div class=highlight><pre>`.
                        .or_else(|| {
                            let body = line.trim_start();
                            let lead = line.len() - body.len();
                            (lead > 0
                                && body.starts_with(placeholder.as_str())
                                && continues_an_item(&lines[..i], lead))
                            .then_some(lead)
                        })
                    {
                        // …and in a LIST ITEM, at the item's content column: a
                        // fence at column 0 ends the list, and the items after
                        // it start another one (2026-10-04).
                        let hang = " ".repeat(col);
                        let block: Vec<String> =
                            fenced.lines().map(|l| format!("{hang}{l}")).collect();
                        line.replace(placeholder.as_str(), &format!("\n\n{}\n", block.join("\n")))
                    } else {
                        line.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            let converted = format!("\n\n{fenced}\n\n");
            out = out.replace(placeholder.as_str(), converted.as_str());
            continue;
        } else {
            inline_code_to_md(raw)
        };
        out = restore_inline_span(&out, &placeholder, &converted);
    }
    out
}

/// Is a line indented by `lead` the continuation of a list item above it?
/// Walking up, every non-blank line must stay indented at least `lead` until
/// an item whose content column IS `lead` — a stray leading space before a
/// top-level block is not a continuation.
fn continues_an_item(above: &[&str], lead: usize) -> bool {
    for line in above.iter().rev().take(400) {
        if line.trim().is_empty() {
            continue;
        }
        if list_item_content_column(line) == Some(lead) {
            return true;
        }
        if line.len() - line.trim_start().len() < lead {
            return false;
        }
    }
    false
}

/// Where the content of a list-item line starts (`- `, `12. `), or `None`.
fn list_item_content_column(line: &str) -> Option<usize> {
    let body = line.trim_start();
    let lead = line.len() - body.len();
    let digits = body.bytes().take_while(u8::is_ascii_digit).count();
    let rest = &body[digits..];
    let marker = if digits > 0 && (rest.starts_with(". ") || rest.starts_with(") ")) {
        digits + 2
    } else if digits == 0
        && (body.starts_with("- ") || body.starts_with("* ") || body.starts_with("+ "))
    {
        2
    } else {
        return None;
    };
    Some(lead + marker)
}

/// A fenced block rendered for a table cell: one code span per line.
fn pre_in_cell(fenced: &str) -> String {
    fenced
        .lines()
        .filter(|l| !l.trim_start().starts_with("```"))
        .filter(|l| !l.trim().is_empty())
        .map(|l| crate::web_tree::escape_pipes(&format!("`{}`", l.trim_end())))
        .collect::<Vec<_>>()
        .join("<br>")
}

/// Put one restored inline code span back, ESCAPING its pipes (`\|`) when the
/// span lands on a markdown table row.
///
/// A cell's own text is already substituted where the row is built
/// (`web_tree`'s table pass and `web::elements`), but a `<code>` span never
/// reaches that code: `sanctuary_code_blocks` has already swapped it for a
/// placeholder, so the substitution sees no pipe and the raw one is put back
/// afterwards. A cell of `<code>0|1</code>` then emitted `| `0|1` | … |`,
/// a row of four columns under a header of two — the row stops describing the
/// table it is in. Measured on both converter paths, which share this restore.
///
/// Only inline spans, and only on a row: a fenced block cannot sit inside a
/// cell, and substituting inside one would corrupt the code it exists to keep.
fn restore_inline_span(s: &str, placeholder: &str, converted: &str) -> String {
    // The common case allocates exactly as before.
    if !converted.contains('|') {
        return s.replace(placeholder, converted);
    }
    let in_cell = crate::web_tree::escape_pipes(converted);
    s.split('\n')
        .map(|line| {
            if !line.contains(placeholder) {
                line.to_string()
            } else if line.trim_start().starts_with('|') {
                line.replace(placeholder, &in_cell)
            } else {
                line.replace(placeholder, converted)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// A `<code>` span in a table cell escaped the cell's pipe substitution,
// because the sanctuary had already replaced it with a placeholder. See
// `restore_inline_span`.
#[cfg(test)]
mod a_code_span_cannot_add_a_column {
    use super::*;

    /// Columns of a row: ESCAPED pipes do not open one. Counting with a bare
    /// `split('|')` would make this test pass for the wrong reason — it would
    /// count the content's pipe as a separator.
    fn columns(row: &str) -> usize {
        let mut n = 1;
        let mut escaped = false;
        for c in row.trim().trim_matches('|').chars() {
            match c {
                '\\' => escaped = !escaped,
                '|' if !escaped => n += 1,
                _ => escaped = false,
            }
        }
        n
    }

    #[test]
    fn a_pipe_inside_a_cell_span_is_escaped() {
        let out = restore_inline_span(
            "| \u{0}CODE_FENCE_0\u{0} | q |",
            "\u{0}CODE_FENCE_0\u{0}",
            "`x|y`",
        );
        assert_eq!(out, "| `x\\|y` | q |", "got: {out}");
        assert_eq!(columns(&out), 2, "the row must keep its two columns: {out}");
    }

    #[test]
    fn a_pipe_in_prose_is_left_alone() {
        // Outside a row there is no structure to protect, and the span is code.
        let out = restore_inline_span(
            "Run \u{0}CODE_FENCE_0\u{0} in prose.",
            "\u{0}CODE_FENCE_0\u{0}",
            "`a|b`",
        );
        assert_eq!(out, "Run `a|b` in prose.", "got: {out}");
    }

    #[test]
    fn a_span_without_a_pipe_takes_the_untouched_path() {
        for host in [
            "| \u{0}CODE_FENCE_0\u{0} | q |",
            "Run \u{0}CODE_FENCE_0\u{0} now.",
        ] {
            let out = restore_inline_span(host, "\u{0}CODE_FENCE_0\u{0}", "`plain`");
            assert!(
                out.contains("`plain`") && !out.contains("\\|"),
                "got: {out}"
            );
        }
    }

    #[test]
    fn only_the_line_that_is_a_row_is_substituted() {
        // The same span restored twice: one occurrence on a row, one in prose.
        let s = "| \u{0}CODE_FENCE_0\u{0} | q |\nprose \u{0}CODE_FENCE_0\u{0} here";
        let out = restore_inline_span(s, "\u{0}CODE_FENCE_0\u{0}", "`x|y`");
        assert!(out.contains("| `x\\|y` | q |"), "row not escaped: {out}");
        assert!(
            out.contains("prose `x|y` here"),
            "prose must keep the pipe: {out}"
        );
    }

    #[test]
    fn a_fenced_block_in_a_cell_keeps_its_pipes_and_its_row() {
        // A `<pre>` inside a cell used to be restored as a FENCE, which broke
        // the row in three (a fence owns its lines) — this test then only
        // guarded that the code's pipe was not escaped inside that fence.
        // Since 2026-10-04 the block is rendered INTO the cell as code spans,
        // and inside a GFM table cell `\|` is how a literal pipe is written:
        // the code still reads `a|b`, and the row keeps its two columns.
        // The restore runs on CONVERTED markdown, so the fixture has to be
        // markdown: a row whose cell is the placeholder.
        let stash = vec!["<pre><code>a|b</code></pre>".to_string()];
        let out = restore_code_blocks("| \u{0}CODE_FENCE_0\u{0} | q |", &stash);
        assert_eq!(
            out, "| `a\\|b` | q |",
            "the row keeps its columns, the code its pipe"
        );
    }

    #[test]
    fn a_fenced_block_keeps_its_pipes() {
        // `<pre>` never reaches `restore_inline_span`: substituting inside a
        // code block would corrupt the code the sanctuary exists to keep.
        let (html, stash) = sanctuary_code_blocks("<p>x</p><pre><code>a | b</code></pre>");
        let out = restore_code_blocks(&html, &stash);
        assert!(out.contains("a | b"), "fenced pipes must survive: {out}");
        assert!(!out.contains("\\|"), "no escaping in a block: {out}");
    }
}

#[cfg(test)]
mod a_wrapper_lends_its_language_only_while_it_is_open {
    use super::hoist_wrapper_language;

    #[test]
    fn a_wrapper_that_has_closed_lends_nothing() {
        // Pygments wraps EVERY block. An unwrapped block following a wrapped
        // one has the NEIGHBOUR's class within its 300-character window:
        // without the `</` bound, it would announce someone else's language.
        // The corpus does not carry this case; it happens as soon as a page
        // mixes a highlighted block and a raw one.
        let html = "<div class=\"highlight-python\"><pre>a = 1</pre></div><pre>texte brut</pre>";
        let out = hoist_wrapper_language(html);
        assert!(
            out.contains("<pre class=\"language-python\">a = 1"),
            "the open wrapper did not lend its language: {out}"
        );
        assert!(
            out.contains("<pre>texte brut"),
            "a CLOSED wrapper lent its language: {out}"
        );
    }

    #[test]
    fn a_block_that_names_its_own_language_keeps_it() {
        let html = "<div class=\"highlight-python\"><pre><code class=\"language-yaml\">k: v</code></pre></div>";
        let out = hoist_wrapper_language(html);
        assert_eq!(
            out, html,
            "the block's language was overwritten by the wrapper"
        );
    }
}
