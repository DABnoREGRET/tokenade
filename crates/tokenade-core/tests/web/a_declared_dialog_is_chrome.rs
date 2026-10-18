//! **A window that DECLARES itself a window is chrome.**
//!
//! `role="dialog"` (and the `<dialog>` element) says the region is laid OVER
//! the page: a notification, an invitation to install the app, a consent
//! banner, a login window, a region picker. It is the same doctrine as for
//! landmarks and for headings — the page declares, there is no need to
//! interpret the class name. And it is the reverse of the change that REMOVED
//! the word `modal` from the class-name list: a design-system word is not a
//! signal, an ARIA declaration is one.
//!
//! MEASURED 2026-09-10 on the 39 pages: **28 regions declare themselves
//! windows, and not one carries content**. A single page moves, `franceinfo`,
//! by **−1,744 bytes**, and the 27 removed lines were read one by one.

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
fn a_front_page_does_not_serve_its_popups() {
    let md = distilled_page("1000__www_franceinfo_fr");
    for chrome in [
        "Activez les notifications franceinfo",
        "Participer au live",
        "Vous pouvez changer d’avis à tout moment",
        "Votre fichier est trop lourd",
        "Sans paiement. Sans abonnement.",
    ] {
        assert!(!md.contains(chrome), "window served: {chrome:?}");
    }
}

#[test]
fn the_page_behind_the_popups_is_intact() {
    // The counterweight: the front page keeps its sections, its replays and
    // its long reads. Without it, "less chrome" would be indistinguishable
    // from "less page".
    let md = distilled_page("1000__www_franceinfo_fr");
    for kept in [
        "Affaire Jubillar",
        "tous les grands formats",
        "JT de 20h du Lundi",
        "Incendies : près de 40 000 hectares",
    ] {
        assert!(md.contains(kept), "the page was amputated: {kept:?}");
    }
}

/// **The `<dialog>` ELEMENT counts as the declaration**, and no page of the
/// corpus exercises it — otherwise the rule would be code nothing reaches.
#[test]
fn the_dialog_element_is_a_window_too() {
    let outside = "Un chapeau hors de la fenêtre, assez long pour que le premier étage de \
                   l'extraction garde le document et juge vraiment la région suivante. ";
    let page = |markup: &str| {
        format!(
            "<html><body><main><p>{}</p>{markup}</main></body></html>",
            outside.repeat(4)
        )
    };
    let md = tokenade_core::web::html_to_markdown(&page(
        "<dialog><p>Inscrivez-vous à notre infolettre hebdomadaire dès maintenant</p></dialog>",
    ));
    assert!(
        !md.contains("infolettre hebdomadaire"),
        "window served: {md:.300}"
    );
    // The other side: an ordinary region of the same size stays.
    let md = tokenade_core::web::html_to_markdown(&page(
        "<section><p>Inscrivez-vous à notre infolettre hebdomadaire dès maintenant</p></section>",
    ));
    assert!(
        md.contains("infolettre hebdomadaire"),
        "section dropped: {md:.300}"
    );
}

/// **A mislabelled container remains catchable**, not by a local guard — it
/// was written then removed, mute — but by the CUMULATIVE guard that refuses
/// a removal taking too much of the page.
#[test]
fn a_region_that_spans_the_page_is_a_container_and_not_a_window() {
    // The restraint, written here because no page of the corpus exercises it:
    // a region covering most of what is walked is a mislabelled CONTAINER,
    // not a window — the same guard as for class names. The fixture must
    // REACH the rule: outside the window there must be enough text for the
    // first stage not to restart in relaxed mode — which drops NO chrome and
    // would turn the test green without proving anything — and inside the
    // window enough for it to cover most of the page — the threshold is 40%,
    // the window covers 75 here. Written with `{outside}` twice the mutant
    // SURVIVED: the body outside the window fell under the floor of the
    // relaxed stage, which then returned the whole page whatever happened.
    let outside = "Un chapeau hors de la fenêtre, assez long pour que le premier étage de \
                   l'extraction garde le document et juge vraiment la région suivante. ";
    let inside = "Une phrase de corps qui n'aurait jamais dû être étiquetée fenêtre, répétée \
                  pour que la région couvre l'essentiel de ce que le parcours voit. ";
    let html = format!(
        "<html><body><main><p>{outside}{outside}{outside}{outside}</p>\
         <div role=\"dialog\"><p>{}</p></div></main></body></html>",
        inside.repeat(12)
    );
    let md = tokenade_core::web::html_to_markdown(&html);
    assert!(
        md.contains("Une phrase de corps qui n'aurait jamais"),
        "a container labelled as a window took the page away: {md:.200}"
    );
}
