//! **The word `modal` dropped a product's sheet.**
//!
//! At IKEA, the product description and its safety instructions live in a
//! collapsible panel whose class is `pipf-product-details-modal__container` —
//! a component name, not a pop-up window.
//!
//! MEASURED 2026-09-10 on the 39 pages: ten carry a `modal` class, the word
//! drops chrome on NONE of them, and it dropped **953 bytes of content** on
//! `ikea_product`, including "Do not use with sharp metal cutlery". The
//! fidelity aggregate goes up (92.8 → 93.0 and 89.0 → 89.1) and no page goes
//! down.

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
fn ikea_keeps_the_description_and_the_care_notice() {
    let md = distilled_page("ikea_product");
    for sentence in [
        "The container is made of durable plastic",
        "Do not use with sharp metal cutlery.",
        "Do not use if there are visible cracks",
    ] {
        assert!(
            md.contains(sentence),
            "\"{sentence}\" was dropped as chrome"
        );
    }
}

/// The other end: the chrome `modal` targeted is still taken by what NAMES
/// it. Without this assertion, also removing `cookie` and `consent` would pass.
#[test]
fn a_consent_dialog_is_still_dropped() {
    let body = "<article><h1>Un article</h1><p>Le corps de l'article, assez long pour que la \
                page porte du contenu réel et ne soit pas prise pour une souche, avec plusieurs \
                phrases qui se suivent et disent quelque chose d'entier.</p>\
                <p>Un second paragraphe, pour que la mesure de densité ait de la matière.</p>\
                </article>";
    // Each class is taken ONLY by the word it tests: `cookie-banner` would
    // have been dropped by `banner`, and the mutation removing `cookie`
    // survived that first writing (the fixture did not reach the rule).
    // `notice`, `panel` and `box` are in no list.
    for chrome in ["cookie-notice", "consent-panel", "newsletter-box"] {
        let html = format!(
            "<html><body>{body}<div class=\"{chrome}\"><h3>Un titre</h3>\
             <p>Acceptez nos conditions pour continuer, disent ces fenêtres, avec assez de \
             mots pour franchir tous les planchers de cette maison.</p></div></body></html>"
        );
        let md = tokenade_core::web_pipeline::distill_page(
            &html,
            "https://exemple.test/a",
            Some("text/html; charset=utf-8"),
        )
        .md;
        assert!(
            md.contains("Le corps de l'article"),
            "the article vanished:\n{md}"
        );
        assert!(
            !md.contains("Acceptez nos conditions"),
            "`{chrome}` is delivered:\n{md}"
        );
    }
}
