//! EPUB: the reading order, and the titles a chapter file does not carry.
//!
//! A zip of XHTML files is not a book until the spine says which order they
//! go in and the TOC says what each one is called.

use super::*;

mod css;
mod notes;

/// Trailing integer of a name (`ppt/slides/slide10.xml` → 10), for numeric
/// ordering. Names without one sort first, then by name.
pub(super) fn slide_number(name: &str) -> u32 {
    let digits: String = name
        .rsplit('/')
        .next()
        .unwrap_or(name)
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().unwrap_or(0)
}

/// EPUB: extract every XHTML chapter via the same XML→text pipeline.
pub(super) fn extract_epub<R: std::io::Read + std::io::Seek>(zip: &mut ZipArchive<R>) -> String {
    let names = epub_reading_order(zip);
    let book = epub_book_title(zip);
    let toc = epub_toc_titles(zip);
    // A book's table of contents, when the producer stores it INSIDE the reading
    // order. See `is_navigation_page`.
    let nav_titles: std::collections::HashSet<String> = toc.values().map(|t| squeeze(t)).collect();
    let declared_nav = epub_declared_nav(zip);
    // The classes the book's style sheets hide (`css.rs`).
    let stylesheets: Vec<String> = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().map(|f| f.name().to_string()))
        .filter(|n| n.to_ascii_lowercase().ends_with(".css"))
        .collect();
    let mut hidden_classes = std::collections::HashSet::new();
    for sheet in stylesheets {
        hidden_classes.extend(css::hidden_classes(&raw_xml_entry(zip, &sheet)));
    }
    let mut out = String::new();
    let (mut note_count, mut book_notes) = (0usize, Vec::new());
    for name in names {
        if declared_nav.as_deref() == Some(name.as_str()) {
            continue;
        }
        let xml = raw_xml_entry(zip, &name);
        // …and those of its own `<style>`.
        let mut classes = hidden_classes.clone();
        for block in re(r"(?is)<style\b[^>]*>(.*?)</style>").captures_iter(&xml) {
            classes.extend(css::hidden_classes(&block[1]));
        }
        let xml = super::hidden::without_hidden_classes(&xml, &classes);
        let (xml, notes) = notes::lift(&xml, &mut note_count);
        book_notes.extend(notes);
        let text = tables_aware(&xml, Schema::Html);
        if text.trim().is_empty() || is_navigation_page(&text, &nav_titles) {
            continue;
        }
        // A heading line, only when the chapter does not already carry one.
        //
        // The ZIP path used to be that line: `## OEBPS/part1.xhtml` is noise the
        // reader pays for and the retriever cannot match ("the main GUI window"
        // scored against `text/the_main_gui_window.xhtml`). But `<title>` is not
        // the answer either — most EPUBs put the BOOK's name in every chapter's
        // title, which yields `## Mastery` seventy-five times: ambiguous
        // headings retrieval cannot tell apart. The chapter's real heading is
        // its own <h1>, which `mark_headings` has already turned into markdown,
        // so prefer that and fall back only for chapters that have none.
        let has_heading = text.lines().any(|l| l.trim_start().starts_with('#'));
        // The table of contents is the authority: it is where an EPUB states
        // what its chapters are called. Calibre-converted books carry no <h1>
        // at all and title every chapter after the book (`## Mastery` ×75), yet
        // their ncx names all 122 sections properly.
        let named = toc
            .get(&name)
            .cloned()
            .or_else(|| epub_chapter_title(&xml).filter(|t| Some(t) != book.as_ref()));
        match named {
            // **The title is often ALREADY the first line of the chapter.**
            //
            // A producer that does not use `<hN>` — LibreOffice exports its
            // headings as styled paragraphs — still writes the title at the head
            // of the chapter. Adding it on top served it TWICE;
            // promoting the line that is already there says the same thing once.
            Some(t) if !has_heading && first_line_is(&text, &t) => {
                let rest = text.split_once('\n').map(|x| x.1).unwrap_or_default();
                let _ = write!(out, "\n## {t}\n{}\n", rest.trim_start_matches('\n'));
            }
            Some(t) if !has_heading => {
                let _ = write!(out, "\n## {t}\n{text}\n");
            }
            _ => {
                let _ = write!(out, "\n{text}\n");
            }
        }
    }
    if !book_notes.is_empty() {
        let lines: Vec<String> = book_notes
            .iter()
            .map(|(n, t)| format!("[^{n}]: {t}"))
            .collect();
        let _ = write!(
            out,
            "\n## {}\n{}\n",
            super::notes_pages::NOTE_SECTIONS[0],
            lines.join("\n")
        );
    }
    collapse_whitespace(&out)
}

/// Whitespace collapsed — the only form in which two writings of the same
/// heading compare.
fn squeeze(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// **The navigation document, when the book DECLARES it.**
///
/// EPUB 3 sets `properties="nav"` on the manifest entry; EPUB 2 lets the
/// `<guide>` name `type="toc"`. Both say the same thing: this file is
/// the table of contents, not a chapter. Rendered as the reading order
/// names them — path resolved from the OPF — so that the comparison is on the
/// same string.
fn epub_declared_nav<R: std::io::Read + std::io::Seek>(zip: &mut ZipArchive<R>) -> Option<String> {
    let container = raw_xml_entry(zip, "META-INF/container.xml");
    let opf_path = re(r#"full-path="([^"]+)""#)
        .captures(&container)?
        .get(1)?
        .as_str()
        .to_string();
    let opf = raw_xml_entry(zip, &opf_path);
    let href = re(r#"<item\b[^>]*\bproperties="[^"]*\bnav\b[^"]*"[^>]*>"#)
        .find(&opf)
        .and_then(|m| {
            re(r#"\bhref="([^"]+)""#)
                .captures(m.as_str())
                .map(|c| c[1].to_string())
        })
        .or_else(|| {
            re(r#"<reference\b[^>]*\btype="toc"[^>]*>"#)
                .find(&opf)
                .and_then(|m| {
                    re(r#"\bhref="([^"]+)""#)
                        .captures(m.as_str())
                        .map(|c| c[1].to_string())
                })
        })?;
    // The hrefs are relative to the OPF's directory, as in
    // `epub_reading_order` — it is the same string that must be obtained, otherwise the
    // comparison never says yes.
    let href = href.split(['#', '?']).next().unwrap_or(&href).to_string();
    let base = opf_path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    Some(normalise_zip_path(&if base.is_empty() {
        href
    } else {
        format!("{base}/{href}")
    }))
}

/// **A page that says NOTHING but what the navigation already says.**
///
/// Many books store their table of contents in the reading order,
/// with nothing declaring it: it then comes out as a chapter, and every heading
/// of the book is served twice. The corpus's real book does exactly
/// that — fourteen entries, all repeated three lines below.
///
/// The criterion is not a file NAME (`toc.xhtml`, `contents.xhtml`,
/// `inhalt.xhtml`…) but what the page CONTAINS: if each of its lines is
/// a heading the navigation declares, it is the navigation rendered as a page,
/// and removing it loses nothing — the chapters it names follow.
///
/// Two guards against the short page: at least **two** recognised lines
/// are needed, otherwise a chapter reduced to its title alone would disappear.
fn is_navigation_page(text: &str, nav_titles: &std::collections::HashSet<String>) -> bool {
    if nav_titles.len() < 2 {
        return false;
    }
    let mut seen = 0usize;
    for line in text.lines() {
        // A table of contents is an `<ol>` more often than not, and its items
        // now carry their markdown number (`1. `): stripped like `-`/`#`.
        let line = line.trim_start_matches(['#', '-', '*', '\t', ' ']);
        let digits = line.bytes().take_while(u8::is_ascii_digit).count();
        let line = if digits > 0 && line[digits..].starts_with(". ") {
            &line[digits + 2..]
        } else {
            line
        };
        let bare = squeeze(line);
        if bare.is_empty() {
            continue;
        }
        if !nav_titles.contains(&bare) {
            return false;
        }
        seen += 1;
    }
    seen >= 2
}

/// Does the chapter's first non-empty line already say `title`?
///
/// Compared with whitespace collapsed: the rendering may have folded a line that the
/// navigation writes in one go.
fn first_line_is(text: &str, title: &str) -> bool {
    let squeeze = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    text.lines()
        .find(|l| !l.trim().is_empty())
        .is_some_and(|l| squeeze(l) == squeeze(title))
}

/// Chapter files in READING order, per the OPF spine.
///
/// Sorting the ZIP entries by name — what this did — is not reading order and
/// is not close to it: `part10.xhtml` sorts before `part2.xhtml`, and every one
/// of the EPUBs in our corpus (6/6) has a spine that disagrees with its
/// alphabetical order. A scrambled book still extracts, still passes a
/// word-count check, and is still wrong.
///
/// Falls back to sorted names when the OPF is unreadable — a malformed EPUB
/// should degrade, not vanish.
pub(super) fn epub_reading_order<R: std::io::Read + std::io::Seek>(
    zip: &mut ZipArchive<R>,
) -> Vec<String> {
    let entries: Vec<String> = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().map(|f| f.name().to_string()))
        .collect();
    let is_chapter =
        |n: &String| n.ends_with(".xhtml") || n.ends_with(".html") || n.ends_with(".htm");
    let ordered = (|| {
        let container = raw_xml_entry(zip, "META-INF/container.xml");
        let opf_path = re(r#"full-path="([^"]+)""#)
            .captures(&container)?
            .get(1)?
            .as_str()
            .to_string();
        let opf = raw_xml_entry(zip, &opf_path);
        if opf.is_empty() {
            return None;
        }
        // id → href, from <item> in either attribute order.
        let mut href: std::collections::HashMap<String, String> = Default::default();
        for c in re(r"<item\b[^>]*>").find_iter(&opf) {
            let tag = c.as_str();
            let id = re(r#"\bid="([^"]+)""#).captures(tag)?.get(1)?.as_str();
            let h = re(r#"\bhref="([^"]+)""#).captures(tag)?.get(1)?.as_str();
            href.insert(id.to_string(), h.to_string());
        }
        // hrefs are relative to the OPF's own directory.
        let base = opf_path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        let mut out = Vec::new();
        for c in re(r#"<itemref\b[^>]*\bidref="([^"]+)""#).captures_iter(&opf) {
            let Some(h) = href.get(c.get(1)?.as_str()) else {
                continue;
            };
            let h = h.split(['#', '?']).next().unwrap_or(h);
            let full = if base.is_empty() {
                h.to_string()
            } else {
                format!("{base}/{h}")
            };
            let full = normalise_zip_path(&full);
            if entries.contains(&full) {
                out.push(full);
            }
        }
        (!out.is_empty()).then_some(out)
    })();
    ordered.unwrap_or_else(|| {
        let mut n: Vec<String> = entries.into_iter().filter(is_chapter).collect();
        n.sort();
        n
    })
}

/// Resolve `..`/`.` in a zip-relative path so it can be matched against the
/// archive's entry names.
pub(super) fn normalise_zip_path(p: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

/// Chapter file → its title, from the EPUB's own table of contents (`.ncx`).
///
/// This is the only place many EPUBs record their structure: a Calibre
/// conversion emits `<p class="calibre12">` for everything, no heading tags and
/// no per-chapter `<title>`, while its ncx still names all 122 sections. The
/// first entry pointing at a file wins — deeper entries are sub-sections of it,
/// addressed by anchor, and we segment by file.
pub(super) fn epub_toc_titles<R: std::io::Read + std::io::Seek>(
    zip: &mut ZipArchive<R>,
) -> std::collections::HashMap<String, String> {
    let mut out: std::collections::HashMap<String, String> = Default::default();
    let Some(ncx_path) = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().map(|f| f.name().to_string()))
        .find(|n| n.ends_with(".ncx"))
    else {
        return out;
    };
    let ncx = raw_xml_entry(zip, &ncx_path);
    let base = ncx_path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let point = re(r"(?s)<navPoint\b.*?</navPoint>");
    let label = re(r"(?s)<navLabel\b.*?<text\b[^>]*>(.*?)</text>");
    let src = re(r#"<content\b[^>]*\bsrc="([^"]+)""#);
    for m in point.find_iter(&ncx) {
        // `find_iter` on a non-greedy block yields the OUTER navPoint only, so
        // a nested one is reached through its parent's body — walk both.
        let block = m.as_str();
        let (Some(l), Some(s)) = (label.captures(block), src.captures(block)) else {
            continue;
        };
        let title = decode_entities(&re(r"(?s)<[^>]+>").replace_all(&l[1], ""))
            .trim()
            .to_string();
        if title.is_empty() || title.chars().count() > 120 {
            continue;
        }
        let href = s[1].split(['#', '?']).next().unwrap_or(&s[1]);
        let full = if base.is_empty() {
            href.to_string()
        } else {
            format!("{base}/{href}")
        };
        out.entry(normalise_zip_path(&full)).or_insert(title);
    }
    out
}

/// The book's own title, so a chapter that merely repeats it can be told apart
/// from one that names itself.
pub(super) fn epub_book_title<R: std::io::Read + std::io::Seek>(
    zip: &mut ZipArchive<R>,
) -> Option<String> {
    let container = raw_xml_entry(zip, "META-INF/container.xml");
    let opf_path = re(r#"full-path="([^"]+)""#)
        .captures(&container)?
        .get(1)?
        .as_str()
        .to_string();
    let opf = raw_xml_entry(zip, &opf_path);
    let raw = re(r"(?is)<dc:title\b[^>]*>(.*?)</dc:title>")
        .captures(&opf)?
        .get(1)?
        .as_str();
    let t = decode_entities(&re(r"(?s)<[^>]+>").replace_all(raw, ""))
        .trim()
        .to_string();
    (!t.is_empty()).then_some(t)
}

/// A chapter's own `<title>`.
pub(super) fn epub_chapter_title(xml: &str) -> Option<String> {
    let raw = re(r"(?is)<title\b[^>]*>(.*?)</title>")
        .captures(xml)?
        .get(1)?
        .as_str();
    let t = decode_entities(&re(r"(?s)<[^>]+>").replace_all(raw, ""))
        .trim()
        .to_string();
    (!t.is_empty() && t.chars().count() <= 120).then_some(t)
}

#[cfg(test)]
mod a_book_does_not_repeat_itself {
    use super::super::{extract, Format};

    fn make_book(nav_in_spine: bool, nav_properties: bool, heading: &str) -> Vec<u8> {
        let page = |body: &str| {
            format!(
                r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><head><title>TITRE-DE-TETE</title></head><body>{body}</body></html>"#
            )
        };
        let props = if nav_properties {
            r#" properties="nav""#
        } else {
            ""
        };
        let spine = if nav_in_spine {
            r#"<itemref idref="nav"/><itemref idref="a"/>"#
        } else {
            r#"<itemref idref="a"/>"#
        };
        let opf = format!(
            r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0"><manifest><item href="a.xhtml" id="a" media-type="application/xhtml+xml"/><item href="b.xhtml" id="b" media-type="application/xhtml+xml"/><item href="nav.xhtml" id="nav" media-type="application/xhtml+xml"{props}/><item href="toc.ncx" id="ncx" media-type="application/x-dtbncx+xml"/></manifest><spine toc="ncx">{spine}<itemref idref="b"/></spine></package>"#
        );
        let ncx = r#"<?xml version="1.0"?><ncx xmlns="http://www.daisy.org/z3986/2005/ncx/"><navMap><navPoint><navLabel><text>CHAPITRE-UN</text></navLabel><content src="a.xhtml"/></navPoint><navPoint><navLabel><text>CHAPITRE-DEUX</text></navLabel><content src="b.xhtml"/></navPoint></navMap></ncx>"#;
        let container = r#"<?xml version="1.0"?><container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="content.opf"/></rootfiles></container>"#;
        super::super::tests::make_zip(&[
            ("META-INF/container.xml", container),
            ("content.opf", &opf),
            ("toc.ncx", ncx),
            (
                "nav.xhtml",
                &page(
                    r#"<ol><li><a href="a.xhtml">CHAPITRE-UN</a></li><li><a href="b.xhtml">CHAPITRE-DEUX</a></li></ol>"#,
                ),
            ),
            (
                "a.xhtml",
                &page(&format!(
                    "<{heading}>CHAPITRE-UN</{heading}><p>Corps CORPS-UN.</p>"
                )),
            ),
            (
                "b.xhtml",
                &page(&format!(
                    "<{heading}>CHAPITRE-DEUX</{heading}><p>Corps CORPS-DEUX.</p>"
                )),
            ),
        ])
    }

    /// A class that the book's sheet (or the chapter's `<style>`) hides
    /// does not come out; a class hidden only in print does.
    #[test]
    fn a_class_the_sheet_hides_does_not_come_out() {
        let container = r#"<?xml version="1.0"?><container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="content.opf"/></rootfiles></container>"#;
        let opf = r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0"><manifest><item href="a.xhtml" id="a" media-type="application/xhtml+xml"/><item href="s.css" id="s" media-type="text/css"/></manifest><spine><itemref idref="a"/></spine></package>"#;
        let css = ".gone { display: none } @media print { .noprint { display: none } }";
        let page = r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><head><title>t</title><style>.local{display:none}</style></head><body><h1>Chap</h1><p>Shown.</p><p class="x gone">FEUILLE-CACHE</p><div class="local"><p>STYLE-CACHE</p></div><p class="noprint">Printed elsewhere.</p></body></html>"#;
        let book = super::super::tests::make_zip(&[
            ("META-INF/container.xml", container),
            ("content.opf", opf),
            ("s.css", css),
            ("a.xhtml", page),
        ]);
        let out = extract(&book, Format::Epub);
        assert!(
            out.contains("Shown.") && out.contains("Printed elsewhere."),
            "{out}"
        );
        assert!(
            !out.contains("FEUILLE-CACHE") && !out.contains("STYLE-CACHE"),
            "{out}"
        );
    }

    /// **A chapter's head is not its body, and its title is not to be
    /// served twice.**
    ///
    /// Each chapter of an EPUB is a complete document: its `<title>` was
    /// rendered as text and, with nothing separating it from the body, it arrived WELDED
    /// to the first word. The weld cost more than the word: the chapter no
    /// longer seemed to start with a heading, so the stage that adds one when
    /// there is none added one — and the title came out TWICE.
    /// Both writings are set up: title as `<h1>` and title as `<p>`, the
    /// second being what a LibreOffice export produces.
    #[test]
    fn a_chapter_head_is_not_served_and_its_title_comes_out_once() {
        for heading in ["h1", "p"] {
            let out = extract(&make_book(false, true, heading), Format::Epub);
            assert!(
                !out.contains("TITRE-DE-TETE"),
                "({heading}) the document head is served:\n{out}"
            );
            assert!(
                out.contains("CORPS-UN") && out.contains("CORPS-DEUX"),
                "({heading}) a chapter disappeared:\n{out}"
            );
            for title in ["CHAPITRE-UN", "CHAPITRE-DEUX"] {
                assert_eq!(
                    out.matches(title).count(),
                    1,
                    "({heading}) « {title} » is served several times:\n{out}"
                );
            }
        }
    }

    /// **The table of contents stored INSIDE the reading order.**
    ///
    /// Many books put it there: it then comes out as a chapter and
    /// every heading is served twice. Both cases are set up — the one where the
    /// book DECLARES it (`properties="nav"`) and the one where nothing declares it, where
    /// only its content gives it away: each of its lines is a heading the
    /// navigation announces.
    #[test]
    fn the_table_of_contents_is_not_a_chapter() {
        for declared in [true, false] {
            let out = extract(&make_book(true, declared, "h1"), Format::Epub);
            assert!(
                out.contains("CORPS-UN") && out.contains("CORPS-DEUX"),
                "(declared={declared}) a chapter disappeared:\n{out}"
            );
            for title in ["CHAPITRE-UN", "CHAPITRE-DEUX"] {
                assert_eq!(
                    out.matches(title).count(),
                    1,
                    "(declared={declared}) « {title} » comes out twice:\n{out}"
                );
            }
        }
    }
}
