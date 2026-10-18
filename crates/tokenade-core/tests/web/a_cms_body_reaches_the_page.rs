//! **The article lived in the structured data and did not arrive.**
//!
//! Many CMSs publish a shell: the markup carries the title, the standfirst
//! and the navigation, and the article body is stored in the page's
//! `application/ld+json`, as `hasPart[].text`, in HTML form. The fallback
//! stage that can read that data only opened on a nearly empty body (400
//! characters), and a page with a standfirst goes above that threshold: it
//! therefore never received its article.
//!
//! MEASURED 2026-09-10 on `nhs_leaf` (an NHS "type 2 diabetes" leaflet), via
//! `cargo run -p tokenade-bench --bin page_loss -- nhs_leaf`:
//! **9 paragraphs out of 45 arrived, and 0 list items out of 42.** After:
//! **45 out of 45 and 35 out of 42**, the seven missing being navigation
//! ("Health A to Z"). The page goes from 1,843 to 9,360 characters and it is
//! the ONLY one of the 39 to move: it is also the only one whose structured
//! data carries markup (9,117 characters out of 10,903).
//!
//! The second defect is in the same chain: those values were STRIPPED of
//! their tags, with nothing in their place, so
//! `<li>feeling very tired</li><li>peeing more than usual</li>` arrived as
//! `feeling very tiredpeeing more than usual` — a word fabricated at each
//! boundary, and the list without its shape.

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
fn a_health_page_delivers_its_symptoms_and_its_treatment() {
    let md = distilled_page("nhs_leaf");
    for expected in [
        "The most common symptoms of type 2 diabetes are",
        "Some people have a higher risk of developing type 2 diabetes",
        "Type 2 diabetes is diagnosed with a blood test",
        "Children with type 2 diabetes will be under specialist care",
    ] {
        assert!(md.contains(expected), "lost: {expected:?}\n{md}");
    }
}

#[test]
fn each_item_of_a_mined_list_is_its_own_line() {
    let md = distilled_page("nhs_leaf");
    // The first two symptoms are two neighbouring `<li>` in the JSON value.
    assert!(
        !md.contains("tiredpeeing"),
        "list boundary glued back — a fabricated word:\n{md:.600}"
    );
    let lines: Vec<&str> = md.lines().collect();
    assert!(
        lines.contains(&"- feeling very tired"),
        "the item is not a list line:\n{md:.600}"
    );
    assert!(
        lines.contains(&"- peeing more than usual"),
        "the item is not a list line:\n{md:.600}"
    );
}

#[test]
fn a_page_whose_structured_data_only_repeats_itself_is_untouched() {
    // `bbc_article` carries a 342-character `application/ld+json`, without a
    // tag inside: its `headline` and `description` repeat the markup. The
    // CMS body has its own gate, so nothing is added here.
    let md = distilled_page("bbc_article");
    let head = md.lines().next().unwrap_or_default().to_string();
    assert!(!md.is_empty(), "empty page");
    assert!(
        !md.contains("this page's markup distilled to nothing"),
        "the page fell back to a fallback stage: {head}"
    );
}
