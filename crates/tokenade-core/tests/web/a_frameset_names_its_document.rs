//! **A frames page says where its document lives.**
//!
//! A `<frameset>` carries a title and not a single body line: its content is
//! in another document. The "thin page" diagnostic does not see it, since the
//! title was rendered — and `web` therefore returned one line, keeping quiet
//! about the only useful thing the page declares.
//!
//! Before, what stood in for an answer was the `<noframes>` delivered WITH
//! ITS TAGS: `<body><p>Votre navigateur…<a href="contenu.html">la page de
//! contenu</a>.</p></body>` in the middle of the markdown.

#[test]
fn a_frameset_page_names_its_frames_and_shows_no_markup() {
    let html = "<!doctype html><html><head><title>Vieux site</title></head>\
                <frameset cols=\"20%,80%\"><frame src=\"menu.html\" name=\"menu\">\
                <frame src=\"contenu.html\" name=\"contenu\">\
                <noframes><body><p>Votre navigateur ne gere pas les cadres. \
                Allez a <a href=\"contenu.html\">la page de contenu</a>.</p></body></noframes>\
                </frameset></html>";
    let d = tokenade_core::web_pipeline::distill_page(
        html,
        "https://vieux.test/index.html",
        Some("text/html; charset=utf-8"),
    );
    assert!(
        d.md.contains("the document is in a FRAME"),
        "the page keeps quiet about its frames:\n{}",
        d.md
    );
    assert!(
        d.md.contains("https://vieux.test/contenu.html"),
        "the CONTENT frame is not named:\n{}",
        d.md
    );
    for markup in ["<p>", "<a href", "<body>"] {
        assert!(
            !d.md.contains(markup),
            "\"{markup}\" is delivered as text:\n{}",
            d.md
        );
    }
}

/// The bound: a page that has a BODY does not get that note, even if it
/// carries a frame. Without it, every page with an embedded video would end up
/// announcing that its document is elsewhere.
#[test]
fn a_page_with_a_body_keeps_quiet_about_its_frames() {
    let html = "<!doctype html><html><head><title>Article</title></head><body>\
                <h1>Un article</h1><p>Un paragraphe qui porte le contenu de la page, \
                assez long pour que la page ne soit pas prise pour une souche.</p>\
                <iframe src=\"https://player.test/embed/1\"></iframe></body></html>";
    let d = tokenade_core::web_pipeline::distill_page(
        html,
        "https://presse.test/a",
        Some("text/html; charset=utf-8"),
    );
    assert!(
        !d.md.contains("the document is in a FRAME"),
        "a page with a body announces its document is elsewhere:\n{}",
        d.md
    );
}
