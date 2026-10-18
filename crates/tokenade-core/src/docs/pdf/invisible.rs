//! **What the page contains and is NOT seen.**
//!
//! The text layer of an OCR scan is laid under the image in invisible
//! render mode: delivering it doubles the document with an approximate version of
//! itself. Ligatures are the reverse problem — `ﬁ` is a single glyph
//! that must become two letters again, otherwise no search finds the word.
//!
//! Both are rewrites that ASSERT something about the page, so
//! each is bounded to what it can prove.

use super::*;

/// Fallback engine: `pdf-extract`, one page at a time.
///
/// Isolated per page because it PANICS on input it dislikes, and a panic
/// anywhere in a 400-page book loses the WHOLE book: a real strategy book
/// extracted 0 words for us and 195 700 for poppler because ONE page
/// declares an `Indexed` colour space and `make_colorspace` panics on colour
/// spaces it does not know — we extract TEXT, and `PlainTextOutput::fill`
/// ignores colour entirely, so the book was lost to a field nobody reads.
/// `extract_text_from_mem_by_pages` cannot do this: it shares one unwind
/// boundary for the whole document.
///
/// Reading order is repaired only on pages that actually have columns; every
/// other page keeps pdf-extract's own text byte for byte (see
/// `pdf_layout::reordered_pages`).
pub(crate) fn pdf_extract_pages(bytes: &[u8]) -> Vec<String> {
    let mut paged = pdf_pages_isolated(bytes);
    if let Some(reordered) = crate::pdf_layout::reordered_pages(bytes) {
        if reordered.len() == paged.len() {
            for (page, fixed) in paged.iter_mut().zip(reordered) {
                if let Some(fixed) = fixed {
                    if !fixed.trim().is_empty() {
                        *page = fixed;
                    }
                }
            }
        }
    }
    paged
}

/// Text render mode 3 (`neither fill nor stroke`) and 7 (`add to clip path`)
/// paint nothing: the page does not show this text, and a human reading the
/// document never sees it. We hand PDF text to agents, so anything hidden there
/// is a prompt-injection surface — "ignore previous instructions" in mode-3 text
/// is invisible to the user who forwards the file and fully visible to the model.
/// `pdf-extract` does not implement `Tr` at all, so today that text is extracted
/// as ordinary prose.
///
/// Rather than decode fonts ourselves (CMaps, Type1 encodings — the genuinely
/// hard part `pdf-extract` exists to do), rewrite the content stream so the
/// hidden glyphs are gone before extraction: same operators, same order, same
/// positioning, only the shown STRINGS emptied. Font decoding stays where it
/// belongs.
///
/// Returns `None` when there is nothing to strip — the overwhelmingly common
/// case — and then the original bytes are used verbatim, so a normal PDF is
/// never re-serialised and cannot be broken by this pass.
///
/// Scope is deliberately narrow: mode 3/7 is UNAMBIGUOUS. Tiny-font and
/// white-on-white text are also invisible in practice, but detecting them needs
/// the text matrix and background analysis, where a false positive DELETES REAL
/// TEXT. Serving text that is merely hard to see is a lesser harm than silently
/// dropping a document's actual content.
pub(crate) fn strip_invisible_text(bytes: &[u8]) -> Option<Vec<u8>> {
    use lopdf::content::Content;
    use lopdf::Object;

    let mut doc = lopdf::Document::load_mem(bytes).ok()?;
    let pages: Vec<lopdf::ObjectId> = doc.get_pages().values().copied().collect();
    let mut edited = 0usize;
    for page_id in pages {
        // A page carrying a raster image plus invisible text is a SCANNED PAGE:
        // the image is what a human sees, the hidden text is the OCR layer, and
        // it is the only machine-readable content there is. Stripping it deleted
        // 99.2% of a real book (328 472 -> 2 557 chars, a scanned philosophy
        // book) — caught only by running 40 real PDFs off
        // disk. An injection hides text on a page that renders WITHOUT it; an
        // OCR layer exists precisely because the page renders as a picture.
        //
        // This does mean a page with both an image and an injection is left
        // alone. That is the deliberate direction of the error: losing a
        // document is certain and silent, hidden text is a possible harm the
        // rest of the pipeline still redacts and bounds.
        if page_has_image(&doc, page_id) {
            continue;
        }
        let Ok(content) = doc.get_and_decode_page_content(page_id) else {
            continue;
        };
        let mut ops = content.operations;
        // `Tr` belongs to the graphics state, so `q`/`Q` save and restore it.
        // Tracking that stack is what keeps us from blanking visible text that
        // merely follows a hidden run.
        let mut mode = 0i64;
        let mut stack: Vec<i64> = Vec::new();
        let mut hit = false;
        for op in ops.iter_mut() {
            match op.operator.as_str() {
                "q" => stack.push(mode),
                "Q" => mode = stack.pop().unwrap_or(0),
                "Tr" => {
                    if let Some(n) = op.operands.first().and_then(as_i64) {
                        mode = n;
                    }
                }
                // Text-showing operators. Emptying the STRING rather than
                // removing the operator preserves every side effect that is not
                // the glyphs themselves: `'` still advances a line, `"` still
                // sets word/char spacing. Nothing downstream shifts.
                "Tj" | "'" => {
                    if mode == 3 || mode == 7 {
                        if let Some(last) = op.operands.last_mut() {
                            if matches!(last, Object::String(..)) {
                                *last = Object::string_literal("");
                                hit = true;
                            }
                        }
                    }
                }
                "\"" => {
                    if mode == 3 || mode == 7 {
                        if let Some(last) = op.operands.last_mut() {
                            if matches!(last, Object::String(..)) {
                                *last = Object::string_literal("");
                                hit = true;
                            }
                        }
                    }
                }
                "TJ" if (mode == 3 || mode == 7) => {
                    if let Some(Object::Array(items)) = op.operands.first_mut() {
                        for it in items.iter_mut() {
                            if matches!(it, Object::String(..)) {
                                *it = Object::string_literal("");
                                hit = true;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if !hit {
            continue;
        }
        let Ok(encoded) = Content { operations: ops }.encode() else {
            continue;
        };
        if doc.change_page_content(page_id, encoded).is_ok() {
            edited += 1;
        }
    }
    if edited == 0 {
        return None;
    }
    let mut out = Vec::new();
    doc.save_to(&mut out).ok()?;
    Some(out)
}

/// True when the page draws a raster image. Used to recognise a scanned page,
/// whose invisible text is its OCR layer rather than an injection.
pub(crate) fn page_has_image(doc: &lopdf::Document, page_id: lopdf::ObjectId) -> bool {
    let Ok((dict, inherited)) = doc.get_page_resources(page_id) else {
        return false;
    };
    let mut resources: Vec<&lopdf::Dictionary> = dict.into_iter().collect();
    for id in inherited {
        if let Ok(d) = doc.get_dictionary(id) {
            resources.push(d);
        }
    }
    resources.iter().any(|res| {
        res.get(b"XObject")
            .ok()
            .and_then(|x| match x {
                lopdf::Object::Dictionary(d) => Some(d.clone()),
                lopdf::Object::Reference(id) => doc.get_dictionary(*id).ok().cloned(),
                _ => None,
            })
            .is_some_and(|xobjs| {
                xobjs.iter().any(|(_, v)| {
                    let obj = match v {
                        lopdf::Object::Reference(id) => doc.get_object(*id).ok(),
                        other => Some(other),
                    };
                    obj.and_then(|o| o.as_stream().ok())
                        .and_then(|s| s.dict.get(b"Subtype").ok())
                        .and_then(|s| s.as_name().ok())
                        == Some(b"Image".as_slice())
                })
            })
    })
}

pub(crate) fn as_i64(o: &lopdf::Object) -> Option<i64> {
    match o {
        lopdf::Object::Integer(i) => Some(*i),
        lopdf::Object::Real(f) => Some(*f as i64),
        _ => None,
    }
}

/// #955 (lever #4) — remap typographic-ligature code points that PDF
/// text extraction commonly emits verbatim so downstream keyword
/// matching (and the reader) sees plain ASCII. LaTeX/TeX PDFs — the
/// "Attention is all you need" family — render `fi`/`fl`/`ff` through
/// ligature glyphs that `pdf-extract` surfaces as U+FB0x (or the
/// equivalent private-use slots some fonts use). Left as-is, a search
/// for `efficient` misses `eﬃcient` and a table value abutting a
/// ligature is corrupted. This maps only the well-known ligature slots;
/// all other text (proven ligature-free on the standard fixture) is
/// untouched, so it is a no-op on clean output.
pub(crate) fn remap_ligatures(s: &str) -> String {
    if !s
        .chars()
        .any(|c| matches!(c, '\u{FB00}'..='\u{FB06}') || matches!(c, '\u{F001}' | '\u{F002}'))
    {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\u{FB00}' => out.push_str("ff"),
            '\u{FB01}' | '\u{F001}' => out.push_str("fi"),
            '\u{FB02}' | '\u{F002}' => out.push_str("fl"),
            '\u{FB03}' => out.push_str("ffi"),
            '\u{FB04}' => out.push_str("ffl"),
            '\u{FB05}' => out.push_str("ft"),
            '\u{FB06}' => out.push_str("st"),
            _ => out.push(c),
        }
    }
    out
}
