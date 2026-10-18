//! **The same formula was delivered THREE times.**
//!
//! Wikipedia writes each equation three times: the presentation MathML
//! (hidden, `display: none`, for screen readers), the `<annotation
//! encoding="application/x-tex">` carrying the TeX, and a fallback `<img>`
//! whose `alt` is that same TeX. A browser paints ONE. tokenade delivered all
//! three:
//!
//! ```text
//! a 2 + b 2 = c 2 . {\displaystyle a^{2}+b^{2}=c^{2}.} [image: {\displaystyle a^{2}+b^{2}=c^{2}.}]
//! ```
//!
//! The first is not even the formula: the exponent has VANISHED, it is a
//! sequence of symbols. The second is markup the browser never renders — the
//! source encoding, same family as `<noframes>`. The third repeats the
//! second.
//!
//! MEASURED on `pages/wikipedia_math` (99 formulas): **−6,246 characters**,
//! and each of the 60 changed lines was reread word for word — nothing is
//! removed that is not one of the two copies.

use std::fs;
use std::path::PathBuf;

fn md(body: &str) -> String {
    let filler = "Un chapeau de plus de deux cents caracteres pour que le premier etage garde \
                  le document et juge vraiment la suite, sans quoi l'etage relache rendrait \
                  tout et le test ne prouverait rien du tout ici. ";
    tokenade_core::web::html_to_markdown(&format!(
        "<html><body><main><p>{filler}</p>{body}</main></body></html>"
    ))
}

const TEX: &str = r"{\displaystyle a^{2}+b^{2}=c^{2}.}";

fn wikipedia_math() -> String {
    let p: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "tests",
        "fixtures",
        "pages",
        "wikipedia_math.html",
    ]
    .iter()
    .collect();
    tokenade_core::web::html_to_markdown(&fs::read_to_string(p).expect("readable fixture"))
}

/// The real page, and the question as a reader asks it: how many times is
/// the Pythagorean theorem written?
#[test]
fn the_real_page_delivers_each_formula_once() {
    let out = wikipedia_math();
    let n = out.matches(TEX).count();
    assert_eq!(n, 1, "the formula is delivered {n} times instead of once");
    assert!(
        out.contains(&format!("${TEX}$")),
        "the formula is not rendered as TeX"
    );
    // The flattened presentation MathML, where the exponent vanished.
    assert!(
        !out.contains("a 2 + b 2 = c 2 ."),
        "the flattened presentation is still delivered:\n{}",
        &out[..out.len().min(400)]
    );
}

/// TeX is declared in two ways by the specification. Without `alttext`, the
/// `<annotation>` answers.
#[test]
fn the_tex_is_read_from_the_annotation_when_there_is_no_alttext() {
    let out = md(
        "<p>On a <math><semantics><mrow><mi>a</mi></mrow>\
         <annotation encoding=\"application/x-tex\">a^{2}</annotation></semantics></math> ici.</p>",
    );
    assert!(out.contains("$a^{2}$"), "{out}");
}

/// The bound: `encoding` says what the annotation CONTAINS. An annotation
/// that is not TeX must not be served as such.
#[test]
fn an_annotation_that_is_not_tex_is_not_served_as_tex() {
    let out = md("<p>On a <math><semantics><mrow><mi>x</mi></mrow>\
         <annotation encoding=\"application/mathml-content+xml\">apply-times-x-y</annotation>\
         </semantics></math> ici.</p>");
    assert!(
        !out.contains("apply-times-x-y"),
        "the source encoding leaked into the page:\n{out}"
    );
    assert!(out.contains('x'), "the presentation vanished too:\n{out}");
}

/// A `<math>` without declared TeX keeps its presentation: it is better than
/// nothing, and it is what the page has.
#[test]
fn a_formula_without_tex_keeps_its_presentation() {
    let out = md("<p>On a <math><mi>y</mi><mo>=</mo><mi>z</mi></math> ici.</p>");
    let line = out
        .lines()
        .find(|l| l.contains("On a"))
        .unwrap_or_else(|| panic!("{out}"));
    assert!(line.contains('y') && line.contains('z'), "{line:?}");
}

/// A formula's fallback image IS that formula, and the mark would deliver it
/// a second time. The bound is the siblings: an image whose `alt` repeats no
/// neighbouring formula keeps its mark.
#[test]
fn an_image_that_is_not_a_formula_keeps_its_mark() {
    let out = md("<p>Voir <span><math alttext=\"a^{2}\"><mi>a</mi></math>\
         <img src=\"f.svg\" alt=\"a^{2}\"></span> et \
         <img src=\"g.png\" alt=\"Le triangle rectangle de la demonstration\"> ici.</p>");
    assert_eq!(
        out.matches("a^{2}").count(),
        1,
        "the formula is still doubled:\n{out}"
    );
    assert!(
        out.contains("[image: Le triangle rectangle de la demonstration]"),
        "an image that is not a formula lost its mark:\n{out}"
    );
}
