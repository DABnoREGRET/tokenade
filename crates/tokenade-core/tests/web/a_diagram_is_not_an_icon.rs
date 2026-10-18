//! **A vector diagram used to lose its name, and an icon must not gain its
//! own.**
//!
//! `<svg>` is dropped from the rendering because that is how a page draws
//! its icons. Measured 2026-09-10 on the corpus's 39 pages: **85 `<svg>`
//! carry a non-empty `<title>`, on 14 pages**, and they are all chrome
//! labels — "play", "search", "caret-left", "facebook", "GOV.UK". Rendering
//! their title would add 85 chrome marks. A threshold on the number of words
//! would be a coin toss: "British Broadcasting Corporation" is three,
//! "Schema du pipeline de compaction" four.
//!
//! Two forms of the specification separate the diagram from the icon, and
//! **none of the 85 carries them**: a `<desc>` (the long description, which
//! an icon never writes) and a `<text>` (the graphic DRAWS words, which a
//! labelled diagram does). The corpus therefore does not move by a byte, and
//! the rule is measured on hand-written pages.

fn md(body: &str) -> String {
    let filler = "Un chapeau de plus de deux cents caracteres pour que le premier etage garde \
                  le document et juge vraiment la suite, sans quoi l'etage relache rendrait \
                  tout et le test ne prouverait rien du tout ici. ";
    tokenade_core::web::html_to_markdown(&format!(
        "<html><body><main><p>{filler}</p>{body}</main></body></html>"
    ))
}

#[test]
fn a_described_diagram_is_named() {
    let out = md(
        "<figure><svg viewBox=\"0 0 10 10\"><title>Schema du pipeline de compaction</title>\
         <desc>Trois etapes reliees par des fleches</desc><rect x=\"1\"/></svg>\
         <figcaption>Figure 1</figcaption></figure>",
    );
    assert!(
        out.contains("[image: Schema du pipeline de compaction]"),
        "the diagram lost its name:\n{out}"
    );
}

#[test]
fn a_diagram_that_draws_words_is_named() {
    let out = md(
        "<figure><svg><title>Etapes de la compaction</title><text x=\"1\">entree</text>\
         <text x=\"2\">sortie</text></svg><figcaption>Fig 2</figcaption></figure>",
    );
    assert!(
        out.contains("[image: Etapes de la compaction]"),
        "a graphic that draws words is a diagram:\n{out}"
    );
}

/// The bound. A `<title>` ALONE is what each of the corpus's 85 icons
/// carries, and it must render nothing.
#[test]
fn an_icon_keeps_its_label_to_itself() {
    let out = md(
        "<p>Voir <svg viewBox=\"0 0 4 4\"><title>fermer</title><path d=\"M0 0\"/></svg> ici.</p>",
    );
    assert!(
        !out.contains("fermer"),
        "an icon's label reached the text:\n{out}"
    );
    assert!(
        out.contains("Voir ici."),
        "the sentence was damaged:\n{out}"
    );
}

/// Without `<title>`, the `<desc>` carries the name: that is the order in
/// which a screen reader announces them.
#[test]
fn a_description_names_a_diagram_that_has_no_title() {
    let out = md("<figure><svg><desc>Courbe des octets livres par mois</desc></svg></figure>");
    assert!(
        out.contains("[image: Courbe des octets livres par mois]"),
        "{out}"
    );
}

/// The other end of the same question: the document's FIRST `<title>` is
/// not always the page's. On a page that declares none in its `<head>`, the
/// glyph's name was delivered as the article title.
#[test]
fn an_icon_label_is_not_the_page_title() {
    let out = tokenade_core::web::html_to_markdown(
        "<html><body><main><p>Un chapeau de plus de deux cents caracteres pour que le premier \
         etage garde le document et juge vraiment la suite, sans quoi l'etage relache rendrait \
         tout et le test ne prouverait rien du tout ici.</p>\
         <p>Voir <svg><title>fermer</title></svg> ici.</p></main></body></html>",
    );
    assert!(
        !out.contains("# fermer"),
        "an icon's name became the page title:\n{out}"
    );
}

/// And the bound: a real `<head>` `<title>` stays the title, including when
/// an `<svg>` writes one right after.
#[test]
fn a_head_title_still_names_the_page() {
    let out = tokenade_core::web::html_to_markdown(
        "<html><head><title>Le pipeline de compaction</title></head><body><main>\
         <p>Un chapeau de plus de deux cents caracteres pour que le premier etage garde le \
         document et juge vraiment la suite, sans quoi l'etage relache rendrait tout et le test \
         ne prouverait rien du tout ici.</p>\
         <p>Voir <svg><title>fermer</title></svg> ici.</p></main></body></html>",
    );
    assert!(out.contains("Le pipeline de compaction"), "{out}");
}
