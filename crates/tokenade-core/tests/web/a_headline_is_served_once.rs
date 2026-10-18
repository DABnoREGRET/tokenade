//! **Each headline of a press front page was delivered twice.**
//!
//! An article card is written: the thumbnail link, whose only description is
//! the article's HEADLINE, a blank line, then that same headline in full. The
//! rule removing a description its neighbourhood already states only looked
//! at the line immediately above and the one immediately below — so, here,
//! the BLANK line.
//!
//! MEASURED 2026-09-10 on the 39 pages: **78 more descriptions over 5 pages,
//! 8,400 bytes**, of which **73 on the `latercera.com` front page alone**,
//! i.e. 14.6% of what it delivered. The link's address stays; it is the
//! repetition that goes.

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
fn a_press_front_page_says_each_headline_once() {
    let md = distilled_page("10000__www_latercera_com");
    let headline = "Funcionaria PDI enfrenta a asaltantes que la atropellaron";
    // The page itself files this article under two sections, so the HEADLINE
    // appears twice — the page says so, not us. What must no longer exist is
    // the headline served a second time as the DESCRIPTION of its own
    // thumbnail.
    assert_eq!(
        md.matches(&format!("image: {headline}")).count(),
        0,
        "the headline is still served as its thumbnail's description"
    );
    // What remains is the HEADLINE, not the description: the reader keeps the
    // sentence, and the thumbnail keeps its address.
    assert!(
        md.contains(&format!("## {headline}")),
        "the headline vanished:\n{md:.800}"
    );
    // **The address, not its SHAPE.** This assertion pinned the link in
    // clear, `[\[image\]](/nacional/noticia/…)`. Since this label is seen by
    // the pointer numbering like all the others, the address is delivered
    // under `⟨N⟩` and listed at the end of the page — the point was that it
    // SURVIVES, not that it stays in full in the middle of the text.
    let url = "/nacional/noticia/funcionaria-pdi-";
    assert!(
        md.contains(url),
        "the thumbnail lost its address:\n{md:.800}"
    );
    // And it is indeed the THUMBNAIL's: the image mark carries the number the
    // pointer list points at that address.
    let n = md
        .lines()
        .find(|l| l.trim_start().starts_with("⟨") && l.contains(url))
        .and_then(|l| l.split_once('⟩').map(|(a, _)| a.trim_start().to_string()))
        .unwrap_or_else(|| panic!("address missing from the pointer list:\n{md:.800}"));
    assert!(
        md.contains(&format!("\\[image\\]{n}⟩")),
        "the image mark no longer carries pointer {n}⟩:\n{md:.800}"
    );
}

#[test]
fn a_description_no_neighbour_repeats_is_kept() {
    // Control on the same page: a description nothing repeats stays whole.
    // Without it, "nothing sticks out any more" would be indistinguishable
    // from "everything was emptied".
    let md = distilled_page("10000__www_latercera_com");
    assert!(
        md.contains("[image: ") || md.contains("[\\[image: "),
        "all descriptions were emptied:\n{md:.800}"
    );
}
