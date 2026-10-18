//! **An image marker opened on one line and closed three lines further down.**
//!
//! An `alt` attribute can contain line breaks — the Rust book and the
//! Guardian write them, because their source is markdown or hand-formatted
//! HTML. Copied verbatim, `[image: …]` no longer fit on its
//! line, and the lines in the middle read as prose.
//!
//! MEASURED 2026-09-10 on the 39 pages: **7 markers on 2 pages**, including the five
//! figures of chapter 4 of the Rust book. The fix is byte-NEUTRAL — a
//! line break becomes a space — and it is the rule that code fences already
//! follow: everything line-oriented downstream (the outline of a
//! large page, the run pruners, the truncated preview) saw a construct
//! cut in two.

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
fn no_page_opens_a_marker_it_does_not_close_on_the_same_line() {
    let mut seen = 0usize;
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pages");
    let mut faults: Vec<String> = Vec::new();
    for e in std::fs::read_dir(&dir).expect("the corpus").flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("html") {
            continue;
        }
        let name = p.file_stem().unwrap().to_string_lossy().to_string();
        let md = distilled_page(&name);
        for line in md.lines() {
            if !line.contains("[image") {
                continue;
            }
            seen += 1;
            if line.matches('[').count() > line.matches(']').count() {
                faults.push(format!(
                    "{name}: {}",
                    line.chars().take(70).collect::<String>()
                ));
            }
        }
    }
    // The detector really LOOKED.
    assert!(seen >= 300, "only {seen} line(s) with a marker");
    assert!(
        faults.is_empty(),
        "{} marker(s) do not close on their line:\n  {}",
        faults.len(),
        faults.join("\n  ")
    );
}

#[test]
fn the_description_survives_whole_on_that_one_line() {
    // The fix FOLDS, it does not cut: the whole sentence from the Rust book
    // must be there, in one piece.
    let md = distilled_page("rust_book");
    assert!(
        md.contains(
            "[image: Two tables: the first table contains the representation of s1 on the stack, \
             consisting of its length (5), capacity (5), and a pointer to the first value in the \
             second table."
        ),
        "the description was cut:\n{md:.1200}"
    );
}
