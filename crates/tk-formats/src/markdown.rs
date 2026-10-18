//! Markdown compaction for READMEs, docs/, blog posts, GitHub wikis.
//!
//! Markdown is universal in agent workflows: every project has a README, a
//! docs/, a CHANGELOG. The signal is the **headings + prose + small code
//! snippets**; the noise is:
//!
//!   - YAML/TOML front-matter blocks (sometimes 50+ lines for static sites),
//!   - Massive code blocks pasted as examples (CI logs, generated SQL, …),
//!   - Reference-style link definitions at the bottom (`[1]: https://… "…"`)
//!     which list every URL referenced inline,
//!   - HTML comments,
//!   - Repeated horizontal rules + table-of-contents auto-generated trees.
//!
//! [`compact`] processes the document line-by-line:
//!   * front-matter (`---` block at the top) stripped
//!   * fenced *or indented* code blocks over [`CODE_BLOCK_CAP`] lines summarised
//!   * HTML comments (`<!-- ... -->`) removed
//!   * link-reference definitions at end-of-doc stripped
//!   * inline images converted to `[image: alt]`
//!   * lists capped at [`LIST_CAP`] visible items
//!   * everything else passed through verbatim

use std::fmt::Write;

// Three topics. The compactor stays here; the OUTLINE, the recognition of line
// shapes and inline HTML moved out.
mod inline_html;
mod outline;
mod shapes;
pub use inline_html::*;
pub use outline::*;
// `pub` and not `pub(crate)` since this module lives in `tk-formats`:
// `skeleton` needs `looks_like_markdown` and `search::render` needs
// `counted`, and both stayed in `tokenade-core`. A glob does not
// propagate what is less visible than itself.
pub use shapes::*;

/// Code blocks larger than this many lines are summarised as
/// `\`\`\`<lang>\n[code block elided: N lines]\n\`\`\``.
///
/// 12 until 2026-10-02: over 94 delivered results carrying the marker, 67
/// (71 %) were followed by a re-fetch of the same source within three calls —
/// the elision bought nothing and cost a round-trip. The 257 blocks it had
/// elided measured median 22 lines, p90 57: 60 keeps nine in ten whole and
/// still folds the captured log or generated listing nobody reads.
pub const CODE_BLOCK_CAP: usize = 60;
/// List runs longer than this many `-`/`*`/`1.` items are folded.
pub const LIST_CAP: usize = 20;

/// HTML block-level tags whose content is decorative in a real-world README
/// (badge clusters, embeds, decorative graphics). When we see an opener for
/// one of these, everything up to the matching closer is dropped wholesale.
const DROPPED_HTML_BLOCKS: &[&str] = &[
    "<picture>",
    "<picture ",
    "<svg ",
    "<svg>",
    "<iframe",
    "<video",
    "<audio",
];

/// HTML block-level tags we *summarise* rather than drop — keep the `<summary>`
/// content of `<details>` so the agent knows what was collapsed.
const SUMMARISED_HTML_BLOCKS: &[(&str, &str)] =
    &[("<details", "</details>"), ("<table", "</table>")];

/// `<table>…</table>` → a markdown table: rows from `<tr>`, cells from
/// `<td>`/`<th>`, inner markup stripped, `|` escaped. `None` when no cell holds
/// text (a layout table of logos), so the caller keeps its marker.
fn html_table_to_markdown(block: &str) -> Option<String> {
    static ROW: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static CELL: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let row = ROW.get_or_init(|| regex::Regex::new(r"(?is)<tr\b[^>]*>(.*?)</tr>").expect("row"));
    let cell =
        CELL.get_or_init(|| regex::Regex::new(r"(?is)<t[dh]\b[^>]*>(.*?)</t[dh]>").expect("cell"));
    let rows: Vec<Vec<String>> = row
        .captures_iter(block)
        .map(|r| {
            cell.captures_iter(&r[1])
                .map(|c| {
                    let text = strip_inline_html(&c[1].replace("<br>", " ").replace("<br/>", " "))
                        .replace("&nbsp;", " ")
                        .replace("&lt;", "<")
                        .replace("&gt;", ">")
                        .replace("&quot;", "\"")
                        .replace("&amp;", "&");
                    text.split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .replace('|', "\\|")
                })
                .collect()
        })
        .filter(|r: &Vec<String>| !r.is_empty())
        .collect();
    if rows.iter().all(|r| r.iter().all(|c| c.is_empty())) {
        return None;
    }
    let width = rows.iter().map(Vec::len).max()?;
    let line = |r: &Vec<String>| {
        let mut cells = r.clone();
        cells.resize(width, String::new());
        format!("| {} |", cells.join(" | "))
    };
    let mut out = vec![line(&rows[0]), format!("|{}", " --- |".repeat(width))];
    out.extend(rows[1..].iter().map(line));
    Some(out.join("\n"))
}

/// Collapse the column-alignment padding of markdown tables.
///
/// `| Tool          | Status  | Notes                |` becomes
/// `| Tool | Status | Notes |`. Cell TEXT is untouched; only the spaces that
/// hold columns straight in a text editor go, and markdown needs none of them.
///
/// Code fences are skipped: a fenced block may contain lines that look like
/// table rows, and its bytes are code.
fn unpad_tables(text: &str) -> String {
    if !text.contains('|') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut in_fence = false;
    for line in text.lines() {
        let t = line.trim_start();
        if crate::fences::fence_marker(t).is_some() {
            in_fence = !in_fence;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if in_fence || !is_table_row(t) {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let indent = &line[..line.len() - t.len()];
        out.push_str(indent);
        out.push_str(&unpad_row(t));
        out.push('\n');
    }
    if !text.ends_with('\n') {
        out.pop();
    }
    out
}

/// A table row: starts and ends with an UNESCAPED pipe, and has at least two
/// of them. `|` alone, or a line merely containing one, is not a row.
fn is_table_row(t: &str) -> bool {
    t.len() >= 3
        && t.starts_with('|')
        && t.ends_with('|')
        && !t.ends_with("\\|")
        && split_cells(t).len() >= 2
}

/// Split on unescaped `|`, dropping the leading and trailing empties.
fn split_cells(t: &str) -> Vec<&str> {
    let inner = &t[1..t.len() - 1];
    let mut cells = Vec::new();
    let mut start = 0usize;
    let b = inner.as_bytes();
    for i in 0..b.len() {
        if b[i] == b'|' && (i == 0 || b[i - 1] != b'\\') {
            cells.push(&inner[start..i]);
            start = i + 1;
        }
    }
    cells.push(&inner[start..]);
    cells
}

fn unpad_row(t: &str) -> String {
    let cells = split_cells(t);
    let mut s = String::with_capacity(t.len());
    s.push('|');
    for c in &cells {
        s.push(' ');
        s.push_str(normalise_cell(c.trim()));
        s.push_str(" |");
    }
    s
}

/// The separator row carries ALIGNMENT in its `:` markers, and any run of
/// three or more dashes is the same rule — so `-----------` becomes `---` and
/// `:--------:` becomes `:---:`, while `:` placement is preserved exactly.
/// Every other cell is returned as-is.
fn normalise_cell(c: &str) -> &str {
    let bytes = c.as_bytes();
    if bytes.len() < 3 || !bytes.iter().all(|&b| b == b'-' || b == b':') {
        return c;
    }
    let left = c.starts_with(':');
    let right = c.ends_with(':') && c.len() > 1;
    match (left, right) {
        (true, true) => ":---:",
        (true, false) => ":---",
        (false, true) => "---:",
        (false, false) => "---",
    }
}

/// Caps on the two lossy cuts of [`compact`].
#[derive(Clone, Copy)]
struct Caps {
    code_block: usize,
    list: usize,
}

impl Caps {
    const NONE: Caps = Caps {
        code_block: usize::MAX,
        list: usize::MAX,
    };
    const DEFAULT: Caps = Caps {
        code_block: CODE_BLOCK_CAP,
        list: LIST_CAP,
    };
}

/// The two cuts — a code block summarised, a list cut short — are made only
/// when the document, rendered WITHOUT them, would not be delivered whole
/// anyway ([`tk_base::delivery::fits`]). Measured 2026-10-02: a 1.3 KB docs
/// index (~330 tokens) lost five of its 25 entries to `LIST_CAP`, under a
/// marker that did not say how many; every other step here is lossless or
/// removes decoration, and runs either way.
pub fn compact(text: &str) -> String {
    let full = compact_capped(text, Caps::NONE);
    if tk_base::delivery::fits(&full) {
        return full;
    }
    compact_capped(text, Caps::DEFAULT)
}

fn compact_capped(text: &str, caps: Caps) -> String {
    // BEFORE the markdown gate, because this transformation is LOSSLESS and so
    // needs no certainty about the format. It also has to be: measured
    // 2026-08-26, 9 of 401 documents that `auto::detect` calls Markdown fail
    // this module's own `looks_like_markdown` — 134 KB routed here and handed
    // straight back by the first line. Two disagreeing definitions of "is this
    // markdown", and the disagreement was a silent passthrough.
    let text = &unpad_tables(text);
    if !looks_like_markdown(text) {
        return text.to_string();
    }
    // Column-alignment padding is the one thing in a markdown document that is
    // pure cost. MEASURED 2026-08-26 over 401 real documents of 4-40 KB (the
    // band the disclose outline never rescues): prose is 70.2% of the bytes,
    // code fences 10.9%, tables 12.2% — and **36.2% of the table bytes are
    // alignment padding**, 441 KB of 1.22 MB. 112 of the 401 documents shrink
    // by 5% or more from removing it alone, the worst by 48-57%: half of
    // `formatters.mdx` is spaces holding columns straight — 9 170 bytes to
    // 4 696.
    //
    // Over the WHOLE corpus rather than that first sample of 401, the average
    // is small and the tail is not: 8 574 documents, 92.8 MB, **2.81% of the
    // bytes** are padding, **1 032 documents (12%) shrink by 5% or more** and
    // 219 by 20% or more. The first sample said 28% of documents gained 5%;
    // it was weighted by one project's translated docs. The 5% floor declines
    // this where it is not worth it, which is the right fit for a change whose
    // value is concentrated in one document in eight.
    //
    // Lossless, which is why it is done and row-dropping is not. A markdown
    // table needs no alignment to render, the cell text is untouched, and the
    // separator keeps its `:` markers — so the rendered table is the same
    // table. Every earlier look at `auto:markdown` ended in "the prose IS the
    // payload", which the 70.2% confirms; this is the part that is not prose.

    let lines: Vec<&str> = text.lines().collect();
    let mut out = String::with_capacity(text.len() / 2);
    let mut i = 0;

    // 1. Strip front-matter (`---` ... `---` at very top).
    if let Some(stop) = front_matter_end(&lines) {
        i = stop + 1;
    }

    let ref_block_start = link_ref_block_start(&lines);

    let mut list_run = 0usize;
    let mut list_indent = 0usize;
    let mut badge_run = 0usize;
    // Document start (and the line after YAML front-matter) is a block
    // boundary, so an indented code run may open on the first content line.
    let mut blank_run = 1usize;
    while i < lines.len() {
        if let Some(stop) = ref_block_start {
            if i >= stop {
                break;
            }
        }
        let line = lines[i];
        let trim = line.trim_start();

        // 2. Fenced code block.
        if let Some((marker, lang, body_end, lines_in_body)) = fenced_code_at(&lines, i) {
            // Short language alias — every fence label costs a few
            // tokens; `javascript` is 2 tokens, `js` is 1. Long-form
            // doc files (READMEs with 40+ code fences) save the
            // bracket-density tier through these aliases.
            let short_lang = canonical_fence_lang(lang);
            if lines_in_body > caps.code_block {
                let _ = write!(
                    out,
                    "{marker}{short_lang}\n[code block elided: {lines_in_body} lines]\n{marker}\n"
                );
            } else {
                // Rewrite just the opening fence to use the short label;
                // copy the body and closing fence verbatim.
                if short_lang == lang {
                    for l in &lines[i..=body_end] {
                        out.push_str(l);
                        out.push('\n');
                    }
                } else {
                    let _ = writeln!(out, "{marker}{short_lang}");
                    for l in &lines[i + 1..=body_end] {
                        out.push_str(l);
                        out.push('\n');
                    }
                }
            }
            i = body_end + 1;
            list_run = 0;
            badge_run = 0;
            blank_run = 1;
            continue;
        }

        // 2b. Indented code (CommonMark: 4 spaces or a tab). RAG `classify`
        // already flags these as `has_code`; the elider only saw fences, so a
        // 50-line indented sample was delivered whole — the same hole `~~~`
        // had before `fence_marker`. Nested list items (`    - foo`) and a
        // list-item continuation (indented, no blank after the bullet) are
        // not code: only open after a blank, and never while a list is open.
        if list_run == 0 && blank_run > 0 {
            if let Some((body_end, lines_in_body)) = indented_code_at(&lines, i) {
                if lines_in_body > caps.code_block {
                    let _ = writeln!(out, "[code block elided: {lines_in_body} lines]");
                } else {
                    for l in &lines[i..=body_end] {
                        out.push_str(l);
                        out.push('\n');
                    }
                }
                i = body_end + 1;
                list_run = 0;
                badge_run = 0;
                blank_run = 1;
                continue;
            }
        }

        // 3. HTML comment block.
        if trim.starts_with("<!--") {
            let mut j = i;
            while j < lines.len() && !lines[j].contains("-->") {
                j += 1;
            }
            i = (j + 1).min(lines.len());
            continue;
        }

        // 4. Decorative HTML block — drop wholesale.
        if DROPPED_HTML_BLOCKS.iter().any(|t| trim.starts_with(t)) {
            // Self-closed on one line (`<iframe … />` or inline `</tag>`):
            // there is no closer on a later line — the forward scan used to
            // run to EOF and silently delete the REST OF THE DOCUMENT.
            let closer = trim.split(['<', '>']).find(|s| !s.is_empty()).map(|s| {
                let name = s.split_whitespace().next().unwrap_or(s);
                format!("</{name}>")
            });
            let self_contained =
                trim.ends_with("/>") || closer.as_deref().is_some_and(|c| trim.contains(c));
            let mut j = i + 1;
            if !self_contained {
                if let Some(c) = closer {
                    while j < lines.len() && !lines[j].contains(&c) {
                        j += 1;
                    }
                    if j == lines.len() {
                        // Unclosed block: drop only the opening line rather
                        // than everything below it.
                        j = i;
                    }
                }
            } else {
                j = i;
            }
            i = (j + 1).min(lines.len());
            continue;
        }

        // 5. Summarised HTML block — keep `<summary>` content from `<details>`,
        //    drop the rest.
        if let Some((open_tag, close_tag)) = SUMMARISED_HTML_BLOCKS
            .iter()
            .find(|(o, _)| trim.starts_with(o))
        {
            // The opening line is searched too: `<details><summary>…</summary>`
            // on ONE line is the common way to write it, and starting at
            // `i + 1` lost the title — measured 2026-10-02 on an internal
            // document, whose `<summary>` title was delivered as
            // `[details: collapsed]`.
            let mut j = i;
            let mut summary_text = String::new();
            while j < lines.len() {
                let t = lines[j].trim();
                let t = t.find("<summary").map_or(t, |at| &t[at..]);
                if t.starts_with("<summary") {
                    // Pull text between <summary…> and </summary>.
                    if let Some(start) = t.find('>') {
                        let rest = &t[start + 1..];
                        let end = rest.find("</summary>").unwrap_or(rest.len());
                        let extracted = strip_inline_html(&rest[..end]);
                        if !extracted.trim().is_empty() {
                            summary_text = extracted.trim().to_string();
                        }
                    }
                }
                // Stops ON the closing line — the opening one included, so a
                // block written on a single line swallows nothing after it.
                if lines[j].contains(close_tag) {
                    break;
                }
                j += 1;
            }
            let tag_name = open_tag.trim_start_matches('<').trim_end_matches(' ');
            // **A table is data.** Measured 2026-10-02 over this machine's
            // markdown: 1 792 raw `<table>` blocks in 783 files, 955 of them
            // carrying 15+ words — a printer driver's Linux/Mac/Windows ×
            // Ethernet/USB/Serial matrix among them — every one delivered as
            // `[table: collapsed]`. Rendered as a markdown table when its cells
            // hold text; a table of logos (no cell text) keeps the marker.
            if *open_tag == "<table" {
                let end = j.min(lines.len().saturating_sub(1));
                if let Some(md) = html_table_to_markdown(&lines[i..=end].join("\n")) {
                    out.push_str(&md);
                    out.push('\n');
                    i = (j + 1).min(lines.len());
                    continue;
                }
            }
            // Say how much: a bare `collapsed` gave a reader no way to judge
            // whether to go and read the block.
            let hidden = j.min(lines.len().saturating_sub(1)).saturating_sub(i) + 1;
            let summary = if summary_text.is_empty() {
                "collapsed".to_string()
            } else {
                summary_text
            };
            let _ = writeln!(out, "[{tag_name}: {summary} — {hidden} line(s) not shown]");
            i = (j + 1).min(lines.len());
            continue;
        }

        // 6. Badge-cluster collapsing. A line is a "badge line" when it's
        //    dominated by `[![...]](...)`, raw `<img>` tags, or shields.io
        //    URLs. Multiple in a row collapse to one marker.
        if is_badge_line(line) {
            badge_run += 1;
            if badge_run == 1 {
                out.push_str("[badges: …]\n");
            }
            i += 1;
            continue;
        } else if !trim.is_empty() {
            badge_run = 0;
        }

        // 7. Strip remaining inline HTML wrappers (<p>, <br>, <sub>, <a>, …)
        //    that aren't structural — keep text inside.
        let processed = transform_inline_images(line);
        let processed = strip_inline_html(&processed);

        // 8. List capping.
        let list_marker_indent = list_marker(&processed);
        if let Some(indent) = list_marker_indent {
            if list_run == 0 || indent != list_indent {
                list_run = 0;
                list_indent = indent;
            }
            list_run += 1;
            if list_run > caps.list {
                if list_run == caps.list + 1 {
                    // Count what the cut hides: the rest of this run, at this
                    // indent, across blank lines — the same run the loop sees.
                    let hidden = lines[i..]
                        .iter()
                        .take_while(|l| {
                            l.trim().is_empty() || list_marker(l).is_some_and(|d| d >= indent)
                        })
                        .filter(|l| list_marker(l) == Some(indent))
                        .count();
                    out.push_str(&" ".repeat(indent));
                    let _ = writeln!(out, "- … list continues ({hidden} more item(s) elided) …");
                }
                i += 1;
                continue;
            }
        } else if !processed.trim().is_empty() {
            list_run = 0;
        }

        // 9. Blank-line collapse: ≥2 blank runs → 1 blank.
        if processed.trim().is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                out.push('\n');
            }
            i += 1;
            continue;
        }
        // An ATX heading is a leaf block: the next indented run is code,
        // even with no blank line (CommonMark). A list continuation is not
        // — `list_run` stays non-zero and the opener above stays shut.
        blank_run = usize::from(is_atx_heading(&processed));

        out.push_str(&processed);
        out.push('\n');
        i += 1;
    }

    if let Some(stop) = ref_block_start {
        let n_refs = lines.len() - stop;
        let _ = write!(
            out,
            "\n[tokenade:markdown] {n_refs} link-reference definition(s) elided\n"
        );
    }

    // Everything above is STRUCTURAL: fences, badges, list runs, link refs. It
    // cannot see the one thing a *generated* markdown document is made of — the
    // same sentence per changelog entry, per ticket, per incident, with one
    // value swapped. On live traffic this path returned 7 %, and no two of
    // those lines are ever byte-equal.
    //
    // Scope, measured rather than assumed: over 15 251 markdown files on a real
    // machine (a Rust monorepo plus an agent's working tree) the fold fires on
    // ONE. This is not a broad win and must not be sold as one — ordinary
    // markdown is prose, and the pay-gate correctly declines it. It is there
    // for the generated document, which is rare per file and enormous per read
    // (the four seen in production averaged ~174 k tokens each).
    let out = crate::template_fold::fold_if_it_pays_outside_code(&out);
    let out = attach_orphan_fence_language(&out);
    out.trim_end().to_string()
}

/// Languages a documentation site prints ABOVE its code samples.
///
/// Not an exhaustive list of programming languages — only the tokens short
/// enough and common enough to appear as a bare paragraph on a docs page. A
/// word that is not here is left alone.
const FENCE_LANGUAGES: &[&str] = &[
    "bash",
    "c",
    "cpp",
    "console",
    "cs",
    "css",
    "diff",
    "dockerfile",
    "go",
    "graphql",
    "html",
    "http",
    "ini",
    "java",
    "javascript",
    "js",
    "json",
    "jsx",
    "kotlin",
    "lua",
    "makefile",
    "md",
    "perl",
    "php",
    "powershell",
    "ps1",
    "python",
    "py",
    "r",
    "ruby",
    "rust",
    "scala",
    "sh",
    "shell",
    "sql",
    "svg",
    "swift",
    "toml",
    "ts",
    "tsx",
    "typescript",
    "xml",
    "yaml",
    "yml",
    "zsh",
];

/// Put a code block's language back on its fence.
///
/// A documentation site renders the language as a visible chip next to the
/// copy button, so converting the page yields a paragraph holding one word —
/// `js` — and then an UNLABELLED fence. The reader gets a stray line that says
/// nothing and a code block whose language it has to guess. MEASURED on the
/// page corpus: 5 of them on one MDN reference page, which is exactly the kind
/// of page an agent reads.
///
/// Deliberately narrow: the paragraph must be a single known language token
/// standing alone between blank lines, and the fence must have no language of
/// its own. A sentence, a two-word line, or an already-labelled fence is left
/// untouched — a `- go` list item or a prose line ending in `sql` is not a
/// chip and absorbing it would delete content.
fn attach_orphan_fence_language(s: &str) -> String {
    if !s.contains("```") {
        return s.to_string();
    }
    let lines: Vec<&str> = s.lines().collect();
    let mut drop: Vec<bool> = vec![false; lines.len()];
    let mut label: Vec<Option<&str>> = vec![None; lines.len()];
    for (i, line) in lines.iter().enumerate() {
        // An unlabelled opening fence, and nothing else on the line.
        if line.trim() != "```" {
            continue;
        }
        let mut j = i;
        while j > 0 && lines[j - 1].trim().is_empty() {
            j -= 1;
        }
        if j == 0 || j == i {
            continue; // no blank line between the word and the fence, or nothing above
        }
        let candidate = lines[j - 1].trim();
        if !FENCE_LANGUAGES.contains(&candidate.to_ascii_lowercase().as_str()) {
            continue;
        }
        // It has to be a paragraph of its own, not the tail of a list or table.
        if j >= 2 && !lines[j - 2].trim().is_empty() {
            continue;
        }
        drop[j - 1] = true;
        label[i] = Some(candidate);
    }
    if !drop.iter().any(|d| *d) {
        return s.to_string();
    }
    let mut out = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        if drop[i] {
            continue;
        }
        match label[i] {
            Some(lang) => out.push(format!("```{lang}")),
            None => out.push((*line).to_string()),
        }
    }
    // Dropping the chip leaves the blank line that was above it next to the one
    // that was below. Collapse to a fixed point, not once: four newlines take
    // two passes, and a single `replace` would leave three behind.
    let mut joined = out.join("\n");
    while joined.contains("\n\n\n") {
        joined = joined.replace("\n\n\n", "\n\n");
    }
    joined
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

/// Render rows as a GitHub-flavoured markdown table (header + separator), the
/// shape agents already parse. Cells are pipe-escaped so a value containing `|`
/// cannot forge a column.
pub fn rows_to_markdown(rows: &[Vec<String>]) -> String {
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    if width == 0 {
        return String::new();
    }
    let cell = |s: &str| s.replace('|', "\\|").replace(['\n', '\r'], " ");
    let mut out = String::new();
    for (i, row) in rows.iter().enumerate() {
        out.push_str("| ");
        for c in 0..width {
            out.push_str(&cell(row.get(c).map(String::as_str).unwrap_or("")));
            out.push_str(" | ");
        }
        out.truncate(out.trim_end().len());
        out.push('\n');
        if i == 0 {
            let _ = writeln!(out, "|{}", " --- |".repeat(width));
        }
    }
    out
}

#[cfg(test)]
include!("markdown_tests.rs");
