//! **A card without a heading lost its address, and a card without a heading
//! was taken for a menu entry.**
//!
//! Two defects that go together. A press front page writes its cards
//! `<li><article><a href="…"><img><p class="…title">…</p></a></article></li>`:
//! no `<hN>`, so the rendering made a single-link bullet of it, and the
//! link-run pruner cut the whole run because the labels are short. Yet
//! `<article>` is the specification's word for a self-contained composition,
//! and no navigation bar puts one in its entries.
//!
//! Unfolding those items first cost **12,372 bytes of addresses** on the
//! franceinfo front page: the link wrapping the card was only placed on a
//! HEADING, and these cards have none. Failing that, it is placed on the
//! first line that says something.

fn md(body: &str) -> String {
    let filler = "Un chapeau de plus de deux cents caracteres pour que le premier etage garde \
                  le document et juge vraiment la suite, sans quoi l'etage relache rendrait \
                  tout et le test ne prouverait rien du tout ici. ";
    tokenade_core::web::html_to_markdown(&format!(
        "<html><body><main><p>{filler}</p>{body}</main></body></html>"
    ))
}

/// Eight cards in a row: beyond six link lines, the pruner cuts.
fn cards(n: usize) -> String {
    (1..=n)
        .map(|i| {
            format!(
                "<li><article><a href=\"/emission-{i}/\"><img src=\"a.jpg\" alt=\"Vignette {i}\">\
                 <div><p class=\"card-collection__title\">Emission {i}</p></div></a></article></li>"
            )
        })
        .collect::<Vec<_>>()
        .join("")
}

#[test]
fn a_carousel_of_cards_is_not_cut_as_a_link_list() {
    let out = md(&format!("<ul>{}</ul>", cards(8)));
    for i in 1..=8 {
        assert!(
            out.contains(&format!("Emission {i}")),
            "card {i} was cut:\n{out}"
        );
    }
}

#[test]
fn a_card_without_a_heading_keeps_its_address() {
    let out = md(&format!("<ul>{}</ul>", cards(8)));
    // The final rendering replaces a link with a numbered pointer, so the
    // proof is the pointer on the heading line AND the address in the list.
    assert!(
        out.contains("Emission 1⟨") && out.contains("/emission-1/"),
        "the card's address is lost:\n{out}"
    );
    // Never on the image mark: `[\[image: …\]](url)` would leave the heading
    // bare and make the description unreadable.
    assert!(
        !out.contains("[[image") && !out.contains("[\\[image"),
        "the link was placed on the image mark:\n{out}"
    );
}

/// The bound: when the card CARRIES a heading, it is the one that takes the
/// link — the previous behaviour, which must not be lost.
#[test]
fn a_card_with_a_heading_still_links_the_heading() {
    let out = md(
        "<ul><li><article><a href=\"/article-un/\"><img src=\"a.jpg\" alt=\"Vignette\">\
         <h2>Le titre de l'article</h2><p>Le chapeau de l'article, un peu plus long.</p>\
         </a></article></li></ul>",
    );
    let line = out
        .lines()
        .find(|l| l.contains("Le titre de l'article"))
        .unwrap_or_else(|| panic!("{out}"));
    // The link is written in full when the page carries few, and as a
    // numbered pointer when it carries many: both forms say the same thing,
    // and this test is about the heading that CARRIES it.
    assert!(
        line.starts_with("## ") && (line.contains("](/article-un/)") || line.contains('⟨')),
        "the heading did not keep the card's link: {line:?}\n{out}"
    );
}
