//! **A relative target without its origin cannot be followed.**
//!
//! The `## Link targets` table keeps the URL as the page writes it, and a
//! page writes `/news/2026/7/21/…`. MEASURED 2026-09-09 on the 39 pages of the
//! corpus: **248 relative targets delivered on 15 pages, and the page's origin
//! in NO output**. The table promises a resolution and returns something
//! that cannot be followed.
//!
//! Judged on the CORPUS because it is what carries the real proportions of
//! relative links: a fixture would only say that the function works.

use std::path::PathBuf;

fn pages_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pages")
}

/// The host under which the test distills — any one, as long as it is
/// one: it is the PARAMETER of the rule, not a property of the fixtures.
const ORIGIN: &str = "https://www.exemple-test.com";

#[test]
fn every_page_with_a_relative_reference_names_its_origin() {
    let mut with_relative = 0usize;
    let mut faults: Vec<String> = Vec::new();
    for e in std::fs::read_dir(pages_dir())
        .expect("the pages directory")
        .flatten()
    {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("html") {
            continue;
        }
        let Ok(html) = std::fs::read_to_string(&p) else {
            continue;
        };
        let name = p
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let d = tokenade_core::web_pipeline::distill_page(
            &html,
            &format!("{ORIGIN}/une/page"),
            Some("text/html; charset=utf-8"),
        );
        // **The TWO places where a relative target is delivered.** The
        // reference table only exists from six links up, and two forms —
        // `?tab=x`, `../sibling` — never enter it whatever the
        // count. Counting only the table let the guarantee hold
        // above a threshold nobody had stated: measured,
        // `django_leaf` delivers **171 relative targets without any table**.
        let in_table =
            d.md.lines()
                .filter(|l| l.starts_with('⟨') && l.contains("⟩ /"))
                .count();
        let inline =
            d.md.match_indices("](")
                .filter(|(i, _)| {
                    let a = &d.md[i + 2..];
                    a.starts_with('/')
                        || a.starts_with('?')
                        || a.starts_with("../")
                        || a.starts_with("./")
                })
                .count();
        let relative = in_table + inline;
        if relative == 0 {
            continue;
        }
        with_relative += 1;
        let announced = d.md.contains(&format!(
            "## Link targets — relative entries are under {ORIGIN}"
        )) || d
            .md
            .contains(&format!("relative links below resolve under {ORIGIN}"));
        if !announced {
            faults.push(format!(
                "{name}: {relative} relative target(s), origin withheld"
            ));
        }
    }
    // **And the rule really is REACHED.** Without this floor, a corpus that
    // stopped carrying relative links would turn the test green without checking
    // anything. Measured: 15 of the 39 pages.
    assert!(
        with_relative >= 10,
        "only {with_relative} page(s) carry a relative target: the corpus no longer reaches the rule"
    );
    assert!(
        faults.is_empty(),
        "{} page(s) deliver relative targets without saying under which host to resolve them:\n  {}",
        faults.len(),
        faults.join("\n  ")
    );
}

/// **And `<base href>` wins over the served address.**
///
/// That is the HTML rule: a document that declares a base resolves ALL its
/// relative URLs against it. Without that, the announcement above would name an origin that the
/// page itself contradicts — an invention, worse than the silence it
/// replaces. None of the 39 corpus pages declares one, so the fixture is
/// hand-written: twelve relative links to cross the reference-table
/// threshold, and some prose to cross the extraction floors.
#[test]
fn a_declared_base_is_the_origin_that_gets_announced() {
    let mut html =
        String::from("<html><head><base href=\"https://cdn.exemple-test.com/v2/\"></head><body>");
    for i in 0..12 {
        html.push_str(&format!(
            "<p>un paragraphe de contenu ordinaire numéro {i}, assez long pour \
             franchir les planchers d'extraction de la page. <a href=\"/article/{i}\">lien {i}</a></p>"
        ));
    }
    html.push_str("</body></html>");
    let d = tokenade_core::web_pipeline::distill_page(
        &html,
        &format!("{ORIGIN}/une/page"),
        Some("text/html; charset=utf-8"),
    );
    assert!(
        d.md.lines()
            .any(|l| l.starts_with('⟨') && l.contains("⟩ /")),
        "the fixture produced no relative reference, it does not reach the rule:\n{}",
        d.md
    );
    assert!(
        d.md.contains("relative entries are under https://cdn.exemple-test.com"),
        "the declared base was not announced:\n{}",
        d.md
    );
    assert!(
        !d.md
            .contains(&format!("relative entries are under {ORIGIN}")),
        "the served address was announced although the page declares a base:\n{}",
        d.md
    );
}

/// **An empty page that declares a frame names that frame.**
///
/// The thin-result diagnostic lists three possible causes — client-side
/// rendering, anti-bot blocking, stub page — and kept quiet about the one FACT the
/// page carries. Hand-written fixture: no corpus page distills to
/// nothing, so none reaches this branch.
#[test]
fn a_page_that_recovers_nothing_names_the_frame_it_declares() {
    let nav: String = (0..30)
        .map(|i| format!("<a href=\"/s{i}\">Section {i}</a>"))
        .collect();
    let js = "var x=1;".repeat(400);
    let html = format!(
        "<html><head><title>Portail</title></head><body><nav>{nav}</nav><script>{js}</script>\
         <iframe src=\"https://contenu.exemple-test.com/vrai\"></iframe></body></html>"
    );
    let d = tokenade_core::web_pipeline::distill_page(
        &html,
        &format!("{ORIGIN}/portail"),
        Some("text/html; charset=utf-8"),
    );
    assert!(
        d.md.contains(tokenade_core::web_tree::THIN_RESULT_MARK),
        "the fixture does not return a thin result, it does not reach the rule:\n{}",
        d.md
    );
    assert!(
        d.md.contains("in a FRAME — fetch it directly: https://contenu.exemple-test.com/vrai"),
        "the declared frame is not named:\n{}",
        d.md
    );
}
