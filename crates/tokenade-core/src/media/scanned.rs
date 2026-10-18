//! **A scanned PDF page is read, when the machine can read it.**
//!
//! The PDF layout pass marks a page with no text layer `> [OCR REQUIRED —
//! page N]` and advises the reader to run OCR — what tokenade can run itself.
//! With `pdftoppm` and `tesseract` on the PATH (the optional-dependency
//! contract of `ocr_text`), each marked page is rendered at 300 dpi and
//! recognised, up to [`OCR_PAGE_CAP`] pages; the others keep the marker, which
//! stays true.
//!
//! Applied HERE, above the document readers, and not inside them: extraction
//! (`docs`) is a lower layer than the tools it would have to call (2026-10-04,
//! the layer guard caught `docs -> media` at four sites).

use super::*;

/// Pages OCR'd per document, at most: a page is seconds of `tesseract`, and a
/// document is often read under a hook's time limit.
pub const OCR_PAGE_CAP: usize = 20;

/// The layout pass's marker for a scanned page, with its page number.
fn marker() -> &'static regex::Regex {
    tk_base::regexes::re(r"(?m)^> \[OCR REQUIRED — page (\d+)\]\n> This page is a scanned[^\n]*\n?")
}

/// Page numbers the extraction marked as scanned, first [`OCR_PAGE_CAP`] only.
pub(crate) fn scanned_pages(text: &str) -> Vec<usize> {
    marker()
        .captures_iter(text)
        .filter_map(|c| c[1].parse().ok())
        .take(OCR_PAGE_CAP)
        .collect()
}

/// `text` (a PDF's extraction) with each marked page replaced by its OCR.
pub fn ocr_scanned_pdf_pages(text: &str, pdf: &[u8]) -> String {
    let todo = scanned_pages(text);
    if todo.is_empty() || !has_binary("pdftoppm") || !has_binary("tesseract") {
        return text.to_string();
    }
    let dir = crate::disclose::stash_dir().join(format!("ocr-pdf-{}", std::process::id()));
    if std::fs::create_dir_all(&dir).is_err() {
        return text.to_string();
    }
    let file = dir.join("doc.pdf");
    let mut read: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
    if std::fs::write(&file, pdf).is_ok() {
        for n in todo {
            let page = n.to_string();
            let stem = dir.join(format!("p{n}"));
            let (Some(pdf_s), Some(stem_s)) = (file.to_str(), stem.to_str()) else {
                break;
            };
            // Bounded like every optional tool: a hook must not hang on one page.
            let rendered = run_bounded_for(
                "pdftoppm",
                &[
                    "-r",
                    "300",
                    "-f",
                    &page,
                    "-l",
                    &page,
                    "-png",
                    "-singlefile",
                    pdf_s,
                    stem_s,
                ],
                std::time::Duration::from_secs(30),
            )
            .is_some();
            let png = stem.with_extension("png");
            if rendered {
                if let Some(t) = ocr_text(&png) {
                    read.insert(n, t);
                }
            }
            let _ = std::fs::remove_file(&png);
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    marker()
        .replace_all(text, |c: &regex::Captures| {
            let n: usize = c[1].parse().unwrap_or(0);
            match read.get(&n) {
                Some(t) => format!("[page {n}: scanned, text read by OCR — recognition can misread characters]\n{t}\n"),
                None => c[0].to_string(),
            }
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only the pages the layout pass marked, and no more than the cap.
    #[test]
    fn only_marked_pages_are_sent_to_ocr_up_to_the_cap() {
        let mark = |n: usize| {
            format!("> [OCR REQUIRED — page {n}]\n> This page is a scanned/rasterised image with no extractable text layer; run OCR to recover its content.\n")
        };
        let mut text = format!("Real text on page one.\n\n{}\nMore text.\n", mark(2));
        assert_eq!(scanned_pages(&text), vec![2]);
        for n in 4..=40 {
            text.push_str(&mark(n));
        }
        let todo = scanned_pages(&text);
        assert_eq!(todo.len(), OCR_PAGE_CAP);
        assert_eq!(todo[0], 2);
        assert!(!scanned_pages("> [OCR REQUIRED] without a page").contains(&0));
    }
}
