//! `pdf` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

/// Real firecrawl `example.pdf` (~20 KB). The extracted text should contain
/// at least 30 alphabetic characters — proves end-to-end PDF parsing works.
pub(crate) fn task_pdf_real(results: &mut Vec<RunResult>) {
    let path = PathBuf::from("benchmarks/fixtures/pdf/example.pdf");
    let Ok(bytes) = fs::read(&path) else {
        return;
    };
    let t = Instant::now();
    let (text, _sav) = docs::extract_with_savings(&bytes, docs::Format::Pdf);
    let ms = t.elapsed().as_millis();
    let letters = text.chars().filter(|c| c.is_alphabetic()).count();
    let quality = if letters >= 30 { 1.0 } else { 0.0 };
    // Same bytes/4 heuristic as DOCX for the "naive cat-pipe" baseline.
    let before_pseudo = "x".repeat(bytes.len().div_ceil(4));
    record(
        results,
        "output:pdf-extract",
        "pdf-extract",
        "firecrawl-example.pdf",
        &before_pseudo,
        &text,
        ms,
        quality,
    );
}

// Adversarial #19: the firecrawl `example-long.pdf` (4.8 MB) must extract in
// under 30 s without panicking. Real PDFs trigger countless edge cases in
// font-encoding tables, so the `catch_unwind` guard in `extract_pdf` must hold.
pub(crate) fn adv_pdf_huge(results: &mut Vec<RunResult>) {
    let path = PathBuf::from("benchmarks/fixtures/pdf/example-long.pdf");
    let Ok(bytes) = fs::read(&path) else {
        return;
    };
    let t = Instant::now();
    let (text, _sav) = docs::extract_with_savings(&bytes, docs::Format::Pdf);
    let ms = t.elapsed().as_millis();
    let fast_enough = ms < 30_000;
    let non_trivial = text.len() >= 200;
    let quality = if fast_enough && non_trivial { 1.0 } else { 0.0 };
    let before_pseudo = "x".repeat(bytes.len().div_ceil(4));
    record(
        results,
        "adv:pdf:huge-fixture",
        "pdf-extract",
        "firecrawl-example-long.pdf",
        &before_pseudo,
        &text,
        ms,
        quality,
    );
}
