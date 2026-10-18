//! **A regular fact, said once — on the web route too.**
//!
//! The hard ceiling SAMPLES a page that is too big: one line kept every N, a
//! marker between each, all of the same number. MEASURED 2026-09-10 on a
//! 3,000-row catalogue: **109 markers, all "26 lines", 3,706 of the 11,795
//! delivered characters — 31% of the page for a repeated sentence.**
//!
//! `elision::fold_uniform_markers` answers that and had only ONE caller,
//! `wrap`: a general mechanism wired on a single route.
//!
//! **None of the corpus's 39 pages reaches the hard ceiling** — checked on
//! 2026-09-10, zero elision markers across the set. The rule is therefore
//! exercised only by the page built here, and saying so is better than
//! letting one believe in coverage by the corpus.

fn a_catalogue_too_big_to_serve_whole() -> String {
    let rows: String = (1..=3000)
        .map(|i| {
            format!(
                "<tr><td>{i}</td><td>Produit {i}</td><td>{} en stock</td><td>{:.2} EUR</td></tr>",
                i % 7,
                9.99 + i as f64
            )
        })
        .collect();
    format!(
        "<!doctype html><html><head><title>Catalogue</title></head><body>\
         <h1>Catalogue</h1><p>Le tableau ci-dessous liste les produits.</p>\
         <table><thead><tr><th>Ref</th><th>Nom</th><th>Stock</th><th>Prix</th></tr></thead>\
         <tbody>{rows}</tbody></table></body></html>"
    )
}

fn delivered(html: &str) -> String {
    let d = tokenade_core::web_pipeline::distill_page(
        html,
        "https://example.test/catalogue.html",
        Some("text/html; charset=utf-8"),
    );
    d.md
}

#[test]
fn a_uniform_sample_declares_its_holes_once() {
    let md = delivered(&a_catalogue_too_big_to_serve_whole());

    // The fixture does reach the rule: without a fold, the page is not
    // sampled and this test would pass without checking anything.
    assert!(
        md.contains("lines elided by tokenade"),
        "the page was not sampled at all, the rule is not reached:\n{}",
        &md[..md.len().min(400)]
    );

    let banner = md
        .lines()
        .find(|l| l.contains(" runs of "))
        .unwrap_or_else(|| panic!("no regular banner:\n{}", &md[..md.len().min(600)]));

    // And what the banner replaces is GONE: that was the whole point of the fold.
    let each: usize = banner
        .rsplit_once(" runs of ")
        .and_then(|(_, tail)| tail.trim_end_matches("] …").trim().parse().ok())
        .unwrap_or_else(|| panic!("the banner does not say the hole's size: {banner}"));
    let repeated = tokenade_core::elision::marker(each);
    assert!(
        !md.contains(&repeated),
        "\"{repeated}\" is still repeated under a banner that already said it:\n{banner}"
    );

    // The banner does not claim more than the fold footer removed. Two lines
    // counting the same thing and contradicting each other would be worse
    // than either one alone.
    let claimed = tokenade_core::elision::weight_of(banner).expect("the banner reads back");
    let footer_total: usize = md
        .lines()
        .find_map(|l| {
            l.strip_prefix("[tokenade folded ")
                .and_then(|r| r.split_once(" line(s) total"))
                .and_then(|(n, _)| n.parse().ok())
        })
        .expect("the fold footer");
    assert!(
        claimed <= footer_total,
        "the banner claims {claimed} lines for {footer_total} folded"
    );
}
