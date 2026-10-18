//! **A FictionBook has its own vocabulary, and the ODF walker did not know it.**
//!
//! FB2 was rendered through the OpenDocument table-aware stripper, whose
//! block boundaries are `text:p`, `table:table-cell`… — none of FB2's. Every
//! FB2 element therefore ran into its neighbour, and the text carried words
//! the book does not have (measured 2026-10-04 on a hand-built book):
//! `sfAdaLovelaceThe Analytical EngineAn annotation…` for the description,
//! `turns,and every cog` for two verses, `PartCount` / `Mill1` / `Store1000`
//! for a table. No fb2 file was in the documents corpus, so no instrument saw
//! it.
//!
//! This walk knows the format (FictionBook 2.0 schema): `<p>` and `<v>` are
//! lines, a `<stanza>` / `<section>` / `<epigraph>` / `<cite>` is a block, a
//! section `<title>` is a heading at its depth, a table is a markdown table,
//! and author name parts are words of one name.

use super::markup_marks::decode_entities;
use tk_base::regexes::re;

/// Elements whose end closes a LINE.
const LINE_ENDS: &[&str] = &[
    "p",
    "v",
    "subtitle",
    "text-author",
    "date",
    "book-title",
    "author",
];
/// CODES, not text from the book: `prose`, `en` served at the head of the reading.
const CODES: &[&str] = &["genre", "lang", "src-lang"];
/// Elements whose end closes a BLOCK (blank line after).
const BLOCK_ENDS: &[&str] = &[
    "stanza",
    "section",
    "epigraph",
    "cite",
    "poem",
    "annotation",
    "title-info",
    "publish-info",
    "empty-line",
];

pub(super) fn render(xml: &str) -> String {
    let mut out = String::new();
    let mut line = String::new();
    // Section depth, and whether we are inside a <title> (whose <p>s are one heading).
    let mut depth = 0usize;
    let mut in_title = 0usize;
    let mut title: Vec<String> = Vec::new();
    let mut row: Option<Vec<String>> = None;
    let mut rows: Vec<(Vec<String>, bool)> = Vec::new();
    let mut header_row = false;
    let flush_line = |line: &mut String, out: &mut String| {
        let t = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if !t.is_empty() {
            out.push_str(&t);
            out.push('\n');
        }
        line.clear();
    };
    // The prologue, comments and CDATA are not elements of the book.
    let xml = re(r"(?s)<\?.*?\?>|<!--.*?-->|<!\[CDATA\[.*?\]\]>").replace_all(xml, " ");
    let mut poem = 0usize;
    let blank_before = |out: &mut String| {
        if !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
    };
    // Links and notes: `<a l:href="https://…">`, and the call
    // `<a l:href="#n1" type="note">[1]</a>` of a note stored in the
    // `<body name="notes">` — rendered `[text](url)`, `[^1]` and `[^1]: …`
    // like those of a `.docx` (2026-10-04: `See the site`, `note[1]`,
    // `## 1` at the end of the book).
    let mut link: Option<(usize, String)> = None;
    let mut in_call = false;
    let mut calls: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut notes: Vec<(usize, String)> = Vec::new();
    let mut in_notes_body = false;
    let mut note_open: Option<(usize, usize)> = None;
    let mut codes = 0usize;
    let attr = |attrs: &str, name: &str| {
        re(&format!(r#"(?:^|\s)(?:\w+:)?{name}="([^"]*)""#))
            .captures(attrs)
            .map(|m| m[1].to_string())
    };
    let tok = re(r#"(?s)<(/?)([\w:.-]+)([^>]*?)(/?)>|([^<]+)"#);
    for c in tok.captures_iter(&xml) {
        if let Some(text) = c.get(5) {
            if in_call || codes > 0 {
                continue;
            }
            let text = decode_entities(text.as_str());
            match (&mut row, in_title > 0) {
                (Some(r), _) => {
                    if let Some(cell) = r.last_mut() {
                        cell.push_str(&text);
                    }
                }
                (None, true) => match title.last_mut() {
                    Some(part) => part.push_str(&text),
                    None => title.push(text),
                },
                (None, false) => line.push_str(&text),
            }
            continue;
        }
        let closing = &c[1] == "/";
        let self_closing = &c[4] == "/";
        let name = c[2].rsplit(':').next().unwrap_or("").to_string();
        match (name.as_str(), closing) {
            (n, false) if CODES.contains(&n) && !self_closing => codes += 1,
            (n, true) if CODES.contains(&n) => codes = codes.saturating_sub(1),
            ("body", false) => {
                in_notes_body = matches!(attr(&c[3], "name").as_deref(), Some("notes" | "comments"))
            }
            ("a", false) if !self_closing => {
                let href = attr(&c[3], "href").unwrap_or_default();
                if attr(&c[3], "type").as_deref() == Some("note") {
                    if let Some(id) = href.strip_prefix('#') {
                        let n = calls.len() + 1;
                        let n = *calls.entry(id.to_string()).or_insert(n);
                        line.push_str(&format!("[^{n}]"));
                        in_call = true;
                    }
                } else if super::markup_marks::has_scheme(&href) && row.is_none() && in_title == 0 {
                    link = Some((line.len(), href));
                }
            }
            ("a", true) => {
                in_call = false;
                if let Some((at, url)) = link.take() {
                    let text = line.get(at..).unwrap_or("").trim().to_string();
                    if !text.is_empty() {
                        line.truncate(at);
                        line.push_str(&format!("[{text}]({url})"));
                    }
                }
            }
            // A section called from the notes body becomes a note.
            ("section", false) if in_notes_body && note_open.is_none() => {
                flush_line(&mut line, &mut out);
                if let Some(&n) = attr(&c[3], "id").and_then(|id| calls.get(&id)) {
                    note_open = Some((n, out.len()));
                }
                depth += 1;
            }
            ("section", true) if note_open.is_some() && depth == 1 => {
                depth = 0;
                flush_line(&mut line, &mut out);
                if let Some((n, at)) = note_open.take() {
                    let body: Vec<&str> = out[at..]
                        .lines()
                        .filter(|l| !l.starts_with('#'))
                        .map(str::trim)
                        .filter(|l| !l.is_empty())
                        .collect();
                    notes.push((n, body.join(" ")));
                    out.truncate(at);
                }
            }
            ("section", false) => {
                flush_line(&mut line, &mut out);
                depth += 1;
            }
            ("section", true) => {
                depth = depth.saturating_sub(1);
                flush_line(&mut line, &mut out);
                blank_before(&mut out);
            }
            ("poem", false) => poem += 1,
            ("poem", true) => {
                poem = poem.saturating_sub(1);
                flush_line(&mut line, &mut out);
                blank_before(&mut out);
            }
            ("title", false) => {
                flush_line(&mut line, &mut out);
                blank_before(&mut out);
                in_title += 1;
                title.clear();
            }
            ("title", true) => {
                in_title = in_title.saturating_sub(1);
                let t = title
                    .iter()
                    .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
                    .filter(|p| !p.is_empty())
                    .collect::<Vec<_>>()
                    .join(" — ");
                if !t.is_empty() {
                    // A poem's title sits one level under its section.
                    out.push_str(&format!(
                        "{} {t}\n\n",
                        "#".repeat((depth + 1 + poem.min(1)).min(6))
                    ));
                }
                title.clear();
                continue;
            }
            // Inside a title its paragraphs are parts of ONE heading.
            ("p", false) if in_title > 0 => title.push(String::new()),
            ("tr", false) => row = Some(Vec::new()),
            ("th" | "td", false) => {
                header_row |= &c[2] == "th";
                if let Some(r) = row.as_mut() {
                    r.push(String::new());
                }
            }
            ("tr", true) => {
                if let Some(r) = row.take() {
                    rows.push((r, std::mem::take(&mut header_row)));
                }
            }
            ("table", true) => {
                flush_line(&mut line, &mut out);
                blank_before(&mut out);
                out.push_str(&markdown_table(&std::mem::take(&mut rows)));
                out.push('\n');
            }
            ("image", _) if self_closing || !closing => {
                flush_line(&mut line, &mut out);
                out.push_str(&crate::markdown::image_marker(None));
                out.push('\n');
            }
            ("first-name" | "middle-name" | "last-name" | "nickname", true) => line.push(' '),
            (n, true) if LINE_ENDS.contains(&n) => flush_line(&mut line, &mut out),
            (n, true) if BLOCK_ENDS.contains(&n) => {
                flush_line(&mut line, &mut out);
                if !out.ends_with("\n\n") && !out.is_empty() {
                    out.push('\n');
                }
            }
            ("empty-line", false) if self_closing => {
                flush_line(&mut line, &mut out);
                out.push('\n');
            }
            _ => {}
        }
    }
    flush_line(&mut line, &mut out);
    if !notes.is_empty() {
        notes.sort_by_key(|(n, _)| *n);
        let lines: Vec<String> = notes.iter().map(|(n, t)| format!("[^{n}]: {t}")).collect();
        out.push_str(&format!(
            "\n## {}\n{}\n",
            super::notes_pages::NOTE_SECTIONS[0],
            lines.join("\n")
        ));
    }
    let collapsed = re(r"\n{3,}").replace_all(&out, "\n\n");
    collapsed.trim().to_string() + "\n"
}

fn markdown_table(rows: &[(Vec<String>, bool)]) -> String {
    let cell = |s: &str| {
        s.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .replace('|', "\\|")
    };
    let width = rows.iter().map(|(r, _)| r.len()).max().unwrap_or(0);
    let mut out = String::new();
    for (i, (r, _)) in rows.iter().enumerate() {
        let cells: Vec<String> = (0..width)
            .map(|k| cell(r.get(k).map_or("", String::as_str)))
            .collect();
        out.push_str(&format!("| {} |\n", cells.join(" | ")));
        if i == 0 {
            out.push_str(&format!("|{}\n", " --- |".repeat(width)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every boundary in the two tables keeps two neighbours apart: a line
    /// end on its own line, a block end with a blank line after it.
    #[test]
    fn every_listed_element_ends_its_line_or_block() {
        for name in LINE_ENDS.iter().chain(BLOCK_ENDS) {
            let xml = format!("<FictionBook><body><{name}>alpha</{name}><{name}>beta</{name}></body></FictionBook>");
            let out = render(&xml);
            assert!(
                out.contains("alpha\n") && !out.contains("alphabeta"),
                "{name}: {out:?}"
            );
            if BLOCK_ENDS.contains(name) {
                assert!(out.contains("alpha\n\nbeta"), "{name} is a block: {out:?}");
            }
        }
    }

    /// A link keeps its address, a note call reads `[^1]` and its section of
    /// the notes body becomes `[^1]: …`; genre and language codes are not
    /// prose.
    #[test]
    fn links_and_notes_read_as_in_a_word_document() {
        let xml = r##"<FictionBook xmlns:l="http://www.w3.org/1999/xlink"><description><title-info><genre>prose</genre><book-title>Engine</book-title><lang>en</lang></title-info></description><body><section><title><p>One</p></title><p>See <a l:href="https://example.com/fb">the site</a> and a note<a l:href="#n1" type="note">[1]</a> here, <a l:href="#n1" type="note">[1]</a> again.</p></section></body><body name="notes"><section id="n1"><title><p>1</p></title><p>NOTE body text.</p></section><section id="n9"><p>Uncalled.</p></section></body></FictionBook>"##;
        let out = render(xml);
        assert!(out.starts_with("Engine\n"), "{out}");
        assert!(
            out.contains("See [the site](https://example.com/fb) and a note[^1] here, [^1] again."),
            "{out}"
        );
        assert!(out.contains("## Footnotes\n[^1]: NOTE body text."), "{out}");
        assert!(
            out.contains("Uncalled.") && !out.contains("## 1\n"),
            "{out}"
        );
    }
}
