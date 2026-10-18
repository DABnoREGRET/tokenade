//! **A marker that this process did NOT write is content.**
//!
//! The regular fold may only claim the lines really removed by the passes of
//! THIS process (`fold_count::folded_ceiling`). A page that QUOTES tokenade's
//! marker — documentation showing its output, for example — must lose nothing,
//! and above all not in exchange for a banner announcing 240 lines that nobody
//! removed.
//!
//! **This test lives in its OWN file, hence its own binary, hence a fresh
//! process, and that is the condition of its validity.** The ceiling is a
//! process counter: the neighbouring test
//! (`uniform_holes_are_declared_once.rs`) folds for real and raises it.
//! Together in one file, alphabetical order ran this one AFTER it, on a
//! ceiling already raised — so the fold applied and the test failed. Green
//! under nextest, which gives each test a process; red under
//! `cargo test --test-threads=1`, which is the release runner, and that is how
//! it was found: the 1.1.12 release stopped on it.

fn delivered(html: &str) -> String {
    let d = tokenade_core::web_pipeline::distill_page(
        html,
        "https://example.test/catalogue.html",
        Some("text/html; charset=utf-8"),
    );
    d.md
}

/// **A marker this process did not write is CONTENT.**
///
/// The page below quotes elision markers — a documentation page showing what
/// a tokenade output looks like does exactly the same. It is short enough that
/// none of our passes folds anything, so the truthfulness ceiling is zero: the
/// regular fold cannot remove anything here without claiming 240 lines that
/// nobody removed.
///
/// Without the ceiling, these eight lines vanish from the page and are
/// replaced by a banner that LIES about what tokenade did.
#[test]
fn markers_this_process_never_wrote_are_left_in_the_page() {
    let quoted = tokenade_core::elision::marker(30);
    // The marker ALONE on its line: that is the only form `gap_of_marker`
    // recognises, hence the only one that reaches the rule. First written in
    // the middle of a sentence, this test passed without checking anything —
    // nothing was recognised, so nothing was removed, either way.
    let body: String = (0..8)
        .map(|i| {
            format!("<p>Exemple numero {i} de sortie repliee, cite tel quel :</p><p>{quoted}</p>")
        })
        .collect();
    let md = delivered(&format!(
        "<!doctype html><html><head><title>Doc</title></head><body>\
         <h1>Ce que tokenade écrit</h1>{body}</body></html>"
    ));

    assert!(
        !md.contains(" runs of "),
        "a banner claims lines that this process did not fold:\n{md}"
    );
    assert_eq!(
        md.matches(&quoted).count(),
        8,
        "lines of the PAGE were removed as if they were ours:\n{md}"
    );
}
