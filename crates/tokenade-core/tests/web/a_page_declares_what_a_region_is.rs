//! **A region's ARIA role says what it is; there is no need to guess it.**
//!
//! A navigation bar is written `<nav>`, or `<div role="navigation">`: the ARIA
//! specification sets the equivalence. tokenade dropped the first and not the
//! second, and compensated with class words — including `related`, which
//! caught Sphinx's bar (`<div class="related" role="navigation">`) and, in
//! passing, **the front-page headlines of three press sites**.
//!
//! The mapping table carries NO new judgement: each role is paired with the
//! element the exclusion list already drops, and a role with no dropped
//! equivalent (`main`, `article`, `region`) decides nothing.
//!
//! MEASURED 2026-09-10 on the 39 pages. The role drops: Wikipedia's
//! navigation boxes, **16,082 bytes**, not one line of prose among them;
//! Discourse's pagination; a three-entry table of contents on `go_pkg`.
//! Removing `related` gives back: `latercera` +1,778 bytes, `franceinfo`
//! +1,269, `investopedia_h` +1,258 — front-page headlines, all of them — and
//! `archwiki` +18.

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
fn a_front_page_keeps_its_headlines() {
    for (page, headline) in [
        (
            "10000__www_latercera_com",
            "Gobierno juega sus cartas para que megarreforma no pase a comisión mixta",
        ),
        (
            "1000__www_franceinfo_fr",
            "Incendies : près de 40 000 hectares partis en fumée en France",
        ),
        (
            "investopedia_h",
            "The Iran War Isn't the Only Reason Fuel Prices Are Rising",
        ),
    ] {
        let md = distilled_page(page);
        assert!(md.contains(headline), "{page} lost: {headline:?}");
    }
}

#[test]
fn a_region_that_declares_itself_a_navigation_is_one() {
    // Wikipedia marks its end-of-article boxes `role="navigation"`: they go,
    // and the article's prose stays.
    let md = distilled_page("wikipedia_art");
    assert!(
        !md.contains("Rust at Wikipedia's sister projects"),
        "the sister-projects box is served"
    );
    assert!(
        md.contains("From Wikipedia, the free encyclopedia"),
        "the article was amputated"
    );
    // **And a TABLE is data.** Wikipedia wraps its end-of-article boxes in a
    // `<div role="navigation">` around a `<table>`: taking them cost 16,082
    // bytes and thirteen points of structural fidelity on this page, and the
    // ratchet was right — those tables name Rust's compilers, libraries and
    // environments.
    for data in [
        "Package management, build tools",
        "Development environments",
        "Verification, testing, analysis",
    ] {
        assert!(md.contains(data), "a data table was thrown away: {data:?}");
    }

    // Sphinx writes `<div class="related" role="navigation">`: it is that
    // role, and no longer the word `related`, that drops it now.
    let md = distilled_page("py_glossary");
    assert!(!md.contains("next | previous"), "the Sphinx bar is back");
    assert!(md.contains("# Glossary"), "the page was amputated");
}
