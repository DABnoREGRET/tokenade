//! **A table without its caption, a numbered list that restarts at 1.**
//!
//! Two defects found outside the corpus, on hand-written pages:
//!
//!  * **`<caption>`** is the only place where a page NAMES its table, and nothing
//!    rendered it — the converter starts from the `<tr>`s, and a caption is not
//!    one. On a Newegg product page, **four specification tables followed each
//!    other with no subject**: "Model", "Details", "Features",
//!    "Dimensions & Weight". On Wikipedia, "Rust Foundation".
//!  * **`<ol start="7">` and `<li value="12">`** say where numbering
//!    starts. Renumbering from 1 does not lose a number: it INVENTS some, and
//!    an agent asked for step 7 was reading step 1 — on exactly the kind of
//!    page where a procedure is split into pieces. **No page of the
//!    corpus carries these attributes**: the measurement is that of the fixtures
//!    below.
//!
//! The caption is written by BOTH converters, the tree one and the regex one.

fn md(body: &str) -> String {
    let filler = "Un chapeau de plus de deux cents caracteres pour que le premier etage garde \
                  le document et juge vraiment la suite, sans quoi l'etage relache rendrait \
                  tout et le test ne prouverait rien du tout ici. ";
    tokenade_core::web::html_to_markdown(&format!(
        "<html><body><main><p>{filler}</p>{body}</main></body></html>"
    ))
}

const TABLE: &str = "<table><caption>Ventes trimestrielles par region</caption>\
    <tr><th>Region</th><th>Ventes</th></tr><tr><td>Nord</td><td>12</td></tr></table>";

#[test]
fn a_caption_names_the_table_it_belongs_to() {
    let out = md(TABLE);
    let lines: Vec<&str> = out.lines().collect();
    let at = lines
        .iter()
        .position(|l| *l == "*Ventes trimestrielles par region*")
        .unwrap_or_else(|| panic!("caption lost:\n{out}"));
    // ABOVE the table: markdown has no caption, that is where it goes.
    assert!(
        lines[at..].iter().any(|l| l.starts_with("| Region")),
        "the caption does not open its table:\n{out}"
    );
}

#[test]
fn a_table_without_a_caption_gains_nothing() {
    // The other side of the boundary: no caption, no italic line.
    for body in [
        "<table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table>",
        // An EMPTY caption is not a caption: without this case, the line
        // `**` — two asterisks and nothing in between — passed the test above.
        "<table><caption></caption><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table>",
        "<table><caption>   </caption><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table>",
    ] {
        let out = md(body);
        assert!(!out.contains('*'), "a caption was invented:\n{out}");
        assert!(out.contains("| A | B |"), "{out}");
    }
}

#[test]
fn a_numbered_list_keeps_the_numbers_the_page_gives() {
    let out = md("<ol start=\"7\"><li>septieme</li><li>huitieme</li></ol>");
    let lines: Vec<&str> = out.lines().map(str::trim).collect();
    assert!(
        lines.contains(&"7. septieme"),
        "the list restarts at 1:\n{out}"
    );
    assert!(lines.contains(&"8. huitieme"), "{out}");

    // `<li value>` repositions what follows, not only its own element.
    let out = md("<ol><li>un</li><li value=\"12\">douze</li><li>treize</li></ol>");
    let lines: Vec<&str> = out.lines().map(str::trim).collect();
    assert!(lines.contains(&"1. un"), "{out}");
    assert!(lines.contains(&"12. douze"), "{out}");
    assert!(lines.contains(&"13. treize"), "{out}");

    // Without an attribute, nothing changes.
    let out = md("<ol><li>un</li><li>deux</li></ol>");
    let lines: Vec<&str> = out.lines().map(str::trim).collect();
    assert!(
        lines.contains(&"1. un") && lines.contains(&"2. deux"),
        "{out}"
    );
}
