//! **A `<details>` label names a section of the outline.**
//!
//! `<summary>` is what the page shows while the block is folded: the LABEL of
//! what it hides, exactly what a `<dt>` is for its `<dd>`. Rendered as a
//! paragraph, it named nothing — and the OUTLINE of a large page, which is
//! built on headings, carried none of those sections.
//!
//! MEASURED 2026-09-10 on `pages/go_pkg` (25 `<details>`, the `net/http`
//! documentation): the outline went from **185 to 202 sections**, and
//! `#### func Handle` from "41 lines" to "7 lines" — the 33 example lines it
//! swallowed are now their own, named section.

use std::path::PathBuf;

fn page(name: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/pages")
        .join(name);
    std::fs::read_to_string(p).expect("the page")
}

#[test]
fn go_pkg_names_the_examples_its_details_blocks_hide() {
    let html = page("go_pkg.html");
    // The fixture does reach the rule.
    assert!(
        html.matches("<details").count() >= 20,
        "the page no longer carries folded blocks, this test proves nothing"
    );
    let d = tokenade_core::web_pipeline::distill_page(
        &html,
        "https://pkg.go.dev/net/http",
        Some("text/html; charset=utf-8"),
    );
    let named = d.md.matches("#### Example").count();
    assert!(
        named >= 10,
        "only {named} named example(s): the folded blocks no longer name anything"
    );
}
