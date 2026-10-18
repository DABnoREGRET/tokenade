//! **`[image]` asserts an illustration: the page must carry one.**
//!
//! `<figure>` is the container of content DETACHED from the flow of text, not
//! of an image: HTML puts a list, a quotation, a table, a code listing in it.
//! The mark was placed on every figure whose content did not already carry an
//! `[image: …]`.
//!
//! MEASURED 2026-09-10 on the corpus's 39 pages: **6 false marks on 2
//! pages** — the five figures of `rust_book` are Rust code blocks, the one of
//! `yahoofin_quote` is a list of analyst recommendations. The four figures of
//! `guardian_article` all carry an `<img>` or an `<svg>` and keep theirs.

use std::path::PathBuf;

fn markers_of(name: &str) -> usize {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/pages")
        .join(format!("{name}.html"));
    let html = std::fs::read_to_string(&p).expect("the page");
    // The fixture does reach the rule: without a figure, nothing to decide.
    assert!(
        html.contains("<figure"),
        "{name} no longer carries a figure, this test proves nothing"
    );
    let d = tokenade_core::web_pipeline::distill_page(
        &html,
        &format!("https://example.test/{name}"),
        Some("text/html; charset=utf-8"),
    );
    d.md.lines().filter(|l| l.trim() == "[image]").count()
}

#[test]
fn a_figure_that_holds_no_image_claims_none() {
    let n = markers_of("rust_book");
    assert_eq!(
        n, 0,
        "rust_book asserts {n} illustration(s) the page does not have — its five figures \
         are Rust code blocks"
    );
}

/// **Counting the BARE marks of a whole page no longer measures this rule.**
///
/// `yahoofin_quote` was the second case of this test, at zero bare marks. A
/// later change made **eleven** appear, and none comes from a figure: they are
/// article thumbnails, real images whose description repeated word for word
/// the headline right below, and of which only the mark remains. A page total
/// counts two producers; the question asked here concerns only one.
///
/// The offending figure is named directly: the one whose content is the list
/// of analyst recommendations.
#[test]
fn the_analyst_list_of_a_quote_page_is_not_announced_as_an_illustration() {
    let p =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pages/yahoofin_quote.html");
    let html = std::fs::read_to_string(&p).expect("the page");
    assert!(
        html.contains("<figure"),
        "yahoofin_quote no longer carries a figure, this test proves nothing"
    );
    let md = tokenade_core::web_pipeline::distill_page(
        &html,
        "https://example.test/yahoofin_quote",
        Some("text/html; charset=utf-8"),
    )
    .md;
    let lines: Vec<&str> = md.lines().collect();
    let at = lines
        .iter()
        .position(|l| l.trim() == "- Strong Buy")
        .expect("the recommendations list");
    let before: Vec<&str> = lines[at.saturating_sub(4)..at]
        .iter()
        .map(|l| l.trim())
        .collect();
    assert!(
        !before.contains(&"[image]"),
        "a list is announced as an illustration: {before:?}"
    );
}

/// The other side of the bound: a figure carrying an image WITHOUT a
/// description keeps its mark, otherwise nothing says there is an
/// illustration there. Without this assertion, removing the mark everywhere
/// passes the test above.
#[test]
fn a_figure_that_holds_an_image_keeps_its_mark() {
    let n = markers_of("guardian_article");
    assert!(
        n >= 1,
        "no illustration is announced any more: the mark no longer serves any purpose"
    );
}
