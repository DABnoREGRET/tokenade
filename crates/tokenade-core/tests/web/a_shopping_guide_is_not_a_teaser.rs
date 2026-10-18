//! **The word `teaser` dropped the shopping guides of a category page.**
//!
//! `teaser` comes from the vocabulary of the PRESS, where it names the summary
//! of an article — that is, content — and design systems use it to name an
//! editorial card. On IKEA, `pub__teaser` carries the two shopping guides of
//! the "dining furniture" page.
//!
//! MEASURED 2026-09-10 on the 39 pages: the word hits **three blocks**. Two
//! are those guides, with their six topics and their descriptions; the
//! third is a recommendations banner ("Ideas based on your recently
//! viewed products"). **3,508 bytes of content against 46 of chrome.** The
//! banner keeps its 46 bytes: that is the price, and it is measured.

use std::path::PathBuf;

fn distilled_page(name: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/pages")
        .join(format!("{name}.html"));
    let html = std::fs::read_to_string(&p).expect("the page");
    tokenade_core::web_pipeline::distill_page(
        &html,
        &format!("https://example.test/{name}"),
        Some("text/html; charset=utf-8"),
    )
    .md
}

#[test]
fn ikea_keeps_the_two_guides_of_its_category_page() {
    let md = distilled_page("ikea_cat");
    for kept in [
        "How would you like to explore?",
        "Helpful how-tos for more enjoyable dining",
        "From clever storage ideas to beautiful table-setting tips",
        "Find the table that fits your space",
    ] {
        assert!(md.contains(kept), "lost: {kept:?}\n{md:.600}");
    }
}

/// The counterweight, and it must REACH the rule that was touched: the first
/// two attempts — "Skip product categories list" and "Customer services"
/// on the same page — go away for other reasons (the short token `nav`, and
/// the `<footer>` element itself), so the mutant "no strong word is
/// chrome" survived them. These three go away BECAUSE OF a word in the strong
/// list, and that word alone.
#[test]
fn the_strong_keywords_that_remain_still_drop_their_chrome() {
    for (page, chrome) in [
        (
            "gov_uk_guide",
            "We use some essential cookies to make this website",
        ),
        ("guardian_home", "Sign up for the First Edition newsletter"),
        ("ikea_cat", "Products Tables & chairs"),
    ] {
        let md = distilled_page(page);
        assert!(
            !md.contains(chrome),
            "{page}: chrome came back — {chrome:?}"
        );
    }
}
