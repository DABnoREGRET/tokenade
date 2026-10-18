//! **`promo` decided by coin toss, and content lost.**
//!
//! The word was treated as lowercase chrome, back when only the capitalised
//! `Promo` of BBC cards had to survive. Yet it is a COMPONENT name in design
//! systems: `slack.com` builds the whole body of its page out of
//! `c-promo__*`.
//!
//! MEASURED 2026-09-10 on the 39 pages: the word dropped **4,059 bytes** of
//! `slack.com` — eight paragraphs out of nine, the very text of the product —
//! and the "Featured" section of `gov_uk_home`, against 419 bytes of
//! advertising on two other pages. Ten to one.

use std::path::PathBuf;

fn distilled(name: &str) -> String {
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
fn slack_keeps_the_copy_its_design_system_calls_a_promo() {
    let md = distilled("slack.com");
    // Three body paragraphs, taken from three different cards.
    for sentence in [
        "Slackbot remembers what actually happened before",
        "Connect your favorite tools directly to Slackbot",
        "Users have wanted to name their group DMs",
    ] {
        assert!(
            md.contains(sentence),
            "\"{sentence}\" was dropped as chrome:\n{}",
            md.chars().take(400).collect::<String>()
        );
    }
}

/// The other end of the measurement: real adverts are still taken, by the
/// words that, for their part, do not decide by coin toss.
#[test]
fn a_real_advert_block_is_still_dropped() {
    let ad = "<div class=\"sponsor-unit\"><h3>Un titre de réclame</h3>\
              <p>Un texte de réclame assez long pour ne pas être pris pour du décor, \
              avec assez de mots pour franchir tous les planchers de cette maison.</p></div>";
    let body = "<article><h1>Un article</h1><p>Le corps de l'article, assez long pour que la \
                page porte du contenu réel et ne soit pas prise pour une souche, avec plusieurs \
                phrases qui se suivent et disent quelque chose.</p>\
                <p>Un second paragraphe, pour que la mesure de densité ait de la matière.</p>\
                </article>";
    let html = format!("<html><body>{body}{ad}</body></html>");
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
        !md.contains("Un texte de réclame"),
        "an advert declared `sponsor` is delivered:\n{md}"
    );
}
