//! **A section heading was left standing above nothing.**
//!
//! `prune_link_list_runs` cuts a run of at least six lines that contain
//! nothing but a link: that is a navigation bar, unless the labels are
//! long. The heading that introduced the run, though, stayed — and the reader
//! was promised a section that no re-read can ever find again.
//!
//! MEASURED 2026-09-10 on the 39 pages of `tests/fixtures/pages`: the pruner
//! cuts **22 runs**, only three sit directly under a heading, and two
//! of those headings ended up EMPTY — `## More on GOV.UK` (the ten most-used
//! services of gov.uk) and `### Examples` (the index of examples of
//! `net/http`). The third, `## SHOP BY Speed` on Newegg, keeps a search
//! field under it and is not touched.

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

fn section_is_empty(md: &str, heading: &str) -> bool {
    let mut lines = md.lines().skip_while(|l| l.trim() != heading);
    if lines.next().is_none() {
        return false; // the heading is not there at all
    }
    lines
        .find(|l| !l.trim().is_empty())
        .is_none_or(|l| l.starts_with("##"))
}

#[test]
fn gov_uk_does_not_announce_a_section_it_cannot_show() {
    let md = distilled_page("gov_uk_home");
    assert!(
        !section_is_empty(&md, "## More on GOV.UK"),
        "empty heading delivered:\n{md}"
    );
    // The rest of the page is intact: the cut only touches the orphan heading.
    assert!(
        md.contains("Welcome to GOV.UK"),
        "page truncated:\n{md:.400}"
    );
    assert!(
        md.contains("Check your State Pension forecast"),
        "the popular services are gone:\n{md:.400}"
    );
}

#[test]
fn go_pkg_does_not_announce_an_empty_examples_section() {
    let md = distilled_page("go_pkg");
    assert!(
        !section_is_empty(&md, "### Examples"),
        "empty heading delivered"
    );
    assert!(md.contains("### Constants"), "next section lost");
}
