//! Text an image shows, read by a local `tesseract` (see `ocr_text`).

use super::*;

/// **The text an image shows, read by a LOCAL `tesseract`** — the image
/// counterpart of the whisper transcript. `tokenade read <image>` is promised
/// to agents as reading the file, and a terminal cannot show pixels: without
/// it a screenshot of an error, an invoice or a slide answered "binary
/// content". `None` when tesseract is absent, overruns, or finds no word.
/// `TOKENADE_OCR_LANG` picks the language pack (`eng` by default).
pub fn ocr_text(path: &std::path::Path) -> Option<String> {
    if !has_binary("tesseract") {
        return None;
    }
    let lang: String = std::env::var("TOKENADE_OCR_LANG")
        .unwrap_or_else(|_| "eng".to_string())
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '+' || *c == '_')
        .take(48)
        .collect();
    let lang = if lang.is_empty() {
        "eng".to_string()
    } else {
        lang
    };
    // Small screenshot text is read on an enlarged copy (see
    // `upscaled_for_ocr`), staged next to the other stash files and removed.
    let staged = crate::image_downscale::upscaled_for_ocr(path).and_then(|png| {
        let p = crate::disclose::stash_dir().join(format!("ocr-{}.png", std::process::id()));
        std::fs::write(&p, png).ok().map(|_| p)
    });
    let target = staged.as_deref().unwrap_or(path).to_str()?.to_string();
    let run = |psm: &str| {
        run_bounded_for(
            "tesseract",
            &[&target, "stdout", "-l", &lang, "--psm", psm],
            std::time::Duration::from_secs(30),
        )
        .map(|raw| {
            raw.lines()
                .map(str::trim_end)
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_string()
        })
    };
    // **Automatic layout first, one block when it shredded the page.** The
    // default segmentation reads aligned rows as COLUMNS: 39 lines of
    // `Line N: The quarterly report shows …` came back as 489 lines, `Line`
    // thirty-nine times, then the numbers, then the next word (2026-10-04).
    // A two-column page needs the automatic mode, so it stays the first
    // choice; its sign of failure is lines of barely one or two words.
    let mut text = run("3");
    if text.as_deref().is_some_and(|t| words_per_line(t) < 2.5) {
        if let Some(block) = run("6") {
            if words_per_line(&block) > text.as_deref().map_or(0.0, words_per_line) {
                text = Some(block);
            }
        }
    }
    if let Some(p) = &staged {
        let _ = std::fs::remove_file(p);
    }
    let text = text?;
    text.chars().any(char::is_alphanumeric).then_some(text)
}

/// Mean words per non-empty line.
fn words_per_line(t: &str) -> f32 {
    let lines: Vec<&str> = t.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return 0.0;
    }
    lines
        .iter()
        .map(|l| l.split_whitespace().count())
        .sum::<usize>() as f32
        / lines.len() as f32
}

/// `tokenade read <image>`: `None` when the path is not an image this build
/// decodes; else its label (`image 600x160`) and, when a local OCR read text
/// in it, the answer — the promise the agent rules make for images. Without
/// that answer the caller serves the image as a binary file, under the label.
pub fn image_read(path: &std::path::Path) -> Option<(String, Option<String>)> {
    let ext = path.extension().and_then(|e| e.to_str())?;
    if !crate::image_downscale::is_downscalable_ext(ext) {
        return None;
    }
    let dims = crate::image_downscale::image_dimensions(path)
        .map(|(w, h)| format!("image {w}x{h}"))
        .unwrap_or_else(|| "image".to_string());
    let note = ocr_text_cached(path).map(|text| {
        format!(
            "[tokenade:{dims}] text read by OCR (tesseract) — recognition can misread characters; the image itself is at {}:\n{text}",
            path.display()
        )
    });
    Some((dims, note))
}

/// [`ocr_text`], cached by the image's CONTENT identity and language — a
/// page is seconds of recognition, and an agent re-reads (9 s each time on a
/// 1200x1600 page, measured).
pub fn ocr_text_cached(path: &std::path::Path) -> Option<String> {
    let lang = std::env::var("TOKENADE_OCR_LANG").unwrap_or_else(|_| "eng".to_string());
    let lang: String = lang
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '+' || *c == '_')
        .take(48)
        .collect();
    let key = cheap_media_identity(path)
        .map(|h| crate::disclose::stash_dir().join(format!("ocr-{lang}-{h}.txt")));
    if let Some(k) = &key {
        if let Ok(t) = std::fs::read_to_string(k) {
            return (!t.trim().is_empty()).then_some(t);
        }
    }
    let text = ocr_text(path)?;
    if let Some(k) = &key {
        // Atomic and private: the read above trusts the file for existing, so
        // a write cut short would be served as the page's text from then on —
        // and the stash holds OCR of the user's documents.
        let _ = crate::platform::write_atomic_600(k, text.as_bytes());
    }
    Some(text)
}

#[cfg(test)]
mod ocr_layout_tests {
    use super::words_per_line;

    /// The sign of a page read as columns: a word or two per line.
    #[test]
    fn a_shredded_page_has_few_words_per_line() {
        let shredded = "Line\nLine\nLine\n1:\n2:\n3:\nThe\nThe\nThe";
        // Tesseract separates paragraphs with a blank line; short lines with
        // blanks between them are still a page read in order.
        let read = "Total due here.\n\nInvoice number six.\n\nPaid in full.";
        assert!(
            words_per_line(shredded) < 2.5,
            "{}",
            words_per_line(shredded)
        );
        assert!(words_per_line(read) >= 2.5, "{}", words_per_line(read));
        assert_eq!(words_per_line(""), 0.0);
    }
}
