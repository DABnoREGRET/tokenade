//! **An accordion title vanished with the button carrying it.**
//!
//! A collapsible component puts its title in a `<button>` — an element
//! tokenade drops, because a button is a command and not text — wrapped in a
//! `<div role="heading" aria-level="3">`. ARIA sets the equivalence
//! `role="heading"` + `aria-level` ≡ `<hN>`, exactly like
//! `role="navigation"` ≡ `<nav>`: the page DECLARES, there is no need to
//! interpret.
//!
//! MEASURED 2026-09-10 on the 39 pages: on the IKEA product sheet, **the five
//! titles** ("Materials and care", "Good to know", "Weight & measurements",
//! "Package details") were missing while their panels were delivered — the
//! material and the care instructions arrived without saying what they were
//! about. On franceinfo, **sixteen section titles** ("Grands formats", "Les
//! plus lus", "L'univers radio") were flat paragraphs: the page had no
//! outline.
//!
//! The second half matters as much: an `<li>` containing a real `<hN>` is
//! already promoted to blocks, an `<li>` DECLARING one was not. Without that
//! equality, the four paragraphs of "Good to know" stayed glued in a single
//! bullet.

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
fn a_product_sheet_names_each_of_its_sections() {
    let md = distilled_page("ikea_product");
    let lines: Vec<&str> = md.lines().collect();
    for title in [
        "### Materials and care",
        "### Good to know",
        "### Weight & measurements",
        "### Package details",
    ] {
        assert!(lines.contains(&title), "missing title: {title:?}");
    }
    // The panel stays under its title, and it is UNFOLDED: without promoting
    // the `<li>`, its sentences arrived glued in a single bullet.
    assert!(
        lines.contains(&"Fits with IKEA 365+ rectangular-shaped lids."),
        "the panel stayed glued in one bullet:\n{md:.900}"
    );
}

#[test]
fn a_news_front_page_gets_its_outline() {
    let md = distilled_page("1000__www_franceinfo_fr");
    let lines: Vec<&str> = md.lines().collect();
    for title in ["## Grands formats", "## Les plus lus", "## L'univers radio"] {
        assert!(lines.contains(&title), "section without a title: {title:?}");
    }
    // The level comes from `aria-level`, it is not invented: franceinfo
    // writes its sub-sections at level 3.
    assert!(
        lines.contains(&"### Les derniers replays et podcasts"),
        "the declared level was not followed:\n{md:.400}"
    );
}
