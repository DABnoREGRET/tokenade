//! **Three families of INLINE elements cut the sentence around them.**
//!
//! The list of elements rendered within the line named seventeen, and it was
//! a list of names ENCOUNTERED, not the specification's phrasing content.
//! Three families were missing, and the corpus does not contain them — they
//! were measured on hand-written pages:
//!
//!  * **`<math>`** — MathML is inline by default, and a formula arrived at
//!    one line per symbol: "La formule" / "a" / "2" / "b";
//!  * **`<ruby>`/`<rt>`**, the Japanese reading annotation: "Le mot" /
//!    "漢字" / "かんじ" / "se lit ainsi";
//!  * **`<del>`/`<ins>`**, which every changelog writes:
//!    "Version" / "ancienne" / "nouvelle" / "du texte".
//!
//! On the corpus's 39 pages, **no word is lost and 23 headings appear**: a
//! phrasing element that CARRIES block content stays a block, and that bound
//! — now applied to the whole list — gives its heading back to a card whose
//! link wraps an `<hN>`.

fn md(body: &str) -> String {
    let filler = "Un chapeau de plus de deux cents caracteres pour que le premier etage garde \
                  le document et juge vraiment la suite, sans quoi l'etage relache rendrait \
                  tout et le test ne prouverait rien du tout ici. ";
    tokenade_core::web::html_to_markdown(&format!(
        "<html><body><main><p>{filler}</p>{body}</main></body></html>"
    ))
}

#[test]
fn a_formula_an_annotation_and_a_revision_stay_in_their_sentence() {
    for (name, body, expected) in [
        (
            "math",
            "<p>La formule <math><msup><mi>a</mi><mn>2</mn></msup></math> est celebre.</p>",
            "est celebre.",
        ),
        (
            "ruby",
            "<p>Le mot <ruby>漢字<rt>かんji</rt></ruby> se lit ainsi.</p>",
            "se lit ainsi.",
        ),
        (
            "del/ins",
            "<p>Version <del>ancienne</del><ins>nouvelle</ins> du texte.</p>",
            "du texte.",
        ),
        // `<progress>` and `<meter>`: their content is the FALLBACK a browser
        // too old to draw them displays in their place, hence text the page
        // shows. The sentence arrived in five lines.
        (
            "progress/meter",
            "<p>La tache est a <progress value=\"70\" max=\"100\">70 %</progress> et la charge \
             a <meter value=\"3\" max=\"10\">3 sur 10</meter> aujourd hui.</p>",
            "aujourd hui.",
        ),
    ] {
        let out = md(body);
        let line = out
            .lines()
            .find(|l| l.contains(expected))
            .unwrap_or_else(|| panic!("`{name}`: the end of the sentence vanished:\n{out}"));
        assert!(
            line.split_whitespace().count() > 3,
            "`{name}`: the sentence arrived in pieces — the tail is alone on its line \
             ({line:?})\n{out}"
        );
    }
}

#[test]
fn an_element_that_carries_a_block_stays_a_block() {
    // The bound: `<del>` and `<ins>` can wrap a whole paragraph, and
    // `<math display="block">` a detached formula. Pulling that into a line
    // would glue two paragraphs together.
    let out = md("<del><p>Premier paragraphe retiré.</p><p>Second paragraphe retiré.</p></del>");
    assert!(
        !out.contains("Premier paragraphe retiré. Second paragraphe retiré."),
        "two paragraphs were glued:\n{out}"
    );
    assert!(out.contains("Premier paragraphe retiré."), "{out}");
    assert!(out.contains("Second paragraphe retiré."), "{out}");
}
