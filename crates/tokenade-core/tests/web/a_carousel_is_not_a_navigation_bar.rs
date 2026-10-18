//! **`nav-item` names an ENTRY of a bar, not a bar.**
//!
//! The vocabulary comes from Bootstrap, which half the web copies: `nav` is
//! the bar, `nav-item` one of its entries. On a real bar, it is the
//! CONTAINER that carries the signal and it goes in full — its entries are
//! therefore never put to the question. A `nav-item` class that gets that far
//! has, by construction, no ancestor judged chrome.
//!
//! MEASURED 2026-09-10 on the 39 pages: the `nav` token hits 18 times, a
//! single form is followed by `item` — `c-switcher-nav-item__heading-row` on
//! `slack.com` — and it carries **the six product promises of the front
//! page's carousel**. The seventeen others are real bars and keep their
//! verdict: **a single page moves, by +219 bytes**.

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
fn slack_keeps_what_its_home_page_promises() {
    let md = distilled_page("slack.com");
    for promise in [
        "Summarize a conversation you missed",
        "Get fast answers with Claude",
        "Review code with Github Copilot",
        "Turn on AI note-taking in huddles",
        "Lookup customer data in Agentforce",
        "Update deals just by asking Slackbot",
    ] {
        assert!(md.contains(promise), "lost: {promise:?}\n{md:.600}");
    }
}

#[test]
fn the_mega_menu_of_the_same_page_is_still_dropped() {
    // Without this counterweight, "keep more" would be indistinguishable from
    // "keep everything": the site's drop-down menu lives on the same page.
    let md = distilled_page("slack.com");
    for chrome in [
        "Slack Connect Work with external partners",
        "Enterprise Key Management",
        "Workflow Builder Automate everyday tasks",
    ] {
        assert!(!md.contains(chrome), "the site menu is back: {chrome:?}");
    }
}
