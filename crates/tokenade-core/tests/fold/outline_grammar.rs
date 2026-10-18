//! **A page's outline counts in grammatical English.**
//!
//! `(1 lines)` and `(1 sections)` went out on the product's busiest route —
//! MEASURED 2026-09-09 on the corpus's 39 pages: **19 times and 13 times**,
//! on 6 pages. An agent reads that string at every folded section.
//!
//! **Judged on the CORPUS, not on a fixture.** Two synthetic fixtures failed
//! to reach the "section without a preview" branch — the one that still wrote
//! its plural by hand — and the mutation putting it back survived them. The
//! corpus does contain it: `latercera` carries one-line sections without a
//! preview. It is the same lesson, applied to a test.

use std::path::PathBuf;

fn pages_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pages")
}

#[test]
fn no_page_outline_says_one_lines_or_one_sections() {
    let mut seen = 0usize;
    let mut faults: Vec<String> = Vec::new();
    let mut singulars = 0usize;
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
        seen += 1;
        let name = p
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let url = format!("https://example.test/{name}");
        // The OUTLINE comes from stashing, not from the distiller: it is
        // `maybe_stash` that summarises a page too big as `md-outline`, and
        // that is exactly what the CLI calls. Querying `distill_page` alone
        // returned ZERO sections — a green test on a string never produced.
        let d = tokenade_core::web_pipeline::distill_page(
            &html,
            &url,
            Some("text/html; charset=utf-8"),
        );
        let md =
            tokenade_core::disclose::maybe_stash(d.md, tokenade_core::disclose::SourceHint::web());
        for bad in ["(1 lines", "(1 sections"] {
            let n = md.matches(bad).count();
            if n > 0 {
                faults.push(format!("{name}: {n}× \"{bad}\""));
            }
        }
        singulars += md.matches("(1 line)").count() + md.matches("(1 section)").count();
    }
    assert!(
        seen >= 30,
        "only {seen} page(s) swept: the corpus is no longer there"
    );
    assert!(
        faults.is_empty(),
        "{} page(s) count in the plural for one:\n  {}",
        faults.len(),
        faults.join("\n  ")
    );
    // **And the branch is indeed REACHED.** Without this floor, a corpus that
    // stopped producing one-line sections would turn the test green without
    // checking anything — a guard that can no longer fire.
    assert!(
        singulars >= 20,
        "only {singulars} singular(s): the corpus no longer reaches the rule"
    );
}
