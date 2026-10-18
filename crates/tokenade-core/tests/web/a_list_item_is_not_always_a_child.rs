//! **A list with a child that is not an `<li>` lost that whole subtree.**
//!
//! The rendering loop only looked at children named `li`. Real markup puts
//! some elsewhere: each section of the franceinfo front page is written
//! `<ul><li>the main card</li><div
//! class="…secondary-contents"><li>…</li><li>…</li></div></ul>`. The HTML is
//! invalid, a browser shows the five cards, and tokenade delivered only one.
//!
//! MEASURED 2026-09-11 on `pages/1000__www_franceinfo_fr`: **107 article
//! headlines missing** from the delivered page — "Dans le Nord, la pétanque
//! est une affaire aussi sérieuse qu'à Marseille", "En Syrie, le monastère de
//! Saint Siméon porte les stigmates de la guerre", the six questions of the
//! fact-checking section.

fn md(body: &str) -> String {
    let filler = "Un chapeau de plus de deux cents caracteres pour que le premier etage garde \
                  le document et juge vraiment la suite, sans quoi l'etage relache rendrait \
                  tout et le test ne prouverait rien du tout ici. ";
    tokenade_core::web::html_to_markdown(&format!(
        "<html><body><main><p>{filler}</p>{body}</main></body></html>"
    ))
}

#[test]
fn an_item_inside_a_wrapper_is_still_an_item() {
    let out = md("<ul><li>La carte principale de la rubrique</li>\
         <div class=\"secondary-contents\">\
         <li>La deuxieme carte de la rubrique</li>\
         <li>La troisieme carte de la rubrique</li></div></ul>");
    for card in ["principale", "deuxieme", "troisieme"] {
        assert!(out.contains(card), "card {card} is missing:\n{out}");
    }
}

/// A wrapper carrying NO item is not an item wrapper: its text is text, and
/// it was lost in the same way.
#[test]
fn text_a_list_carries_outside_an_item_is_not_lost() {
    let out = md("<ul><li>Le premier element</li><div>Une note posee dans la liste</div></ul>");
    assert!(out.contains("Une note posee dans la liste"), "{out}");
}

/// The bound: a nested list stays nested, and an ordered list's numbering
/// continues through the wrapper.
#[test]
fn a_wrapper_does_not_flatten_a_nested_list_nor_lose_the_count() {
    // The list nested DIRECTLY under the list, without an `<li>` around it:
    // it is the path `list_items_into` decides, and the only one reaching it.
    let nested = md("<ul><li>Premier element</li><ul><li>Le sous element</li></ul></ul>");
    let line = nested
        .lines()
        .find(|l| l.contains("sous element"))
        .unwrap_or_else(|| panic!("{nested}"));
    assert!(
        line.starts_with(' '),
        "the nested list lost its indent: {line:?}"
    );

    let ordered = md("<ol><li>Premier</li><div><li>Deuxieme</li><li>Troisieme</li></div></ol>");
    assert!(ordered.contains("2. Deuxieme"), "{ordered}");
    assert!(ordered.contains("3. Troisieme"), "{ordered}");
}
