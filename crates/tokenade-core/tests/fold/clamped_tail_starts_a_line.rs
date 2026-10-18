//! **The tail of a clamped preview starts at a LINE.**
//!
//! When a page is too big, it goes back out as an OUTLINE: one line per
//! section, heading plus preview. That outline itself has a budget, and the
//! tail cut fell at a character count — hence at random. MEASURED 2026-09-10
//! on the corpus's 39 pages: **9 of the 10 clamped pages** resumed mid-word,
//! without the section heading, left on the other side of the cut:
//! `ch service to …")`, `s the expected type for a variable`,
//! `w/index.php?title=…&action=edit&section=43>)`, and on `investopedia_l` a
//! whole preview `(7 lines · "…")` attached to no heading.
//!
//! The reader then attributes that fragment to what PRECEDES the banner.

use std::path::PathBuf;

fn pages_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pages")
}

const CLAMP: &str = "(preview clamped,";

/// What an OUTLINE line can be: a section heading, a pointer entry, or one of
/// our own bracketed lines. A sentence fragment is not one.
fn opens_a_line_of_the_outline(line: &str) -> bool {
    line.starts_with('#') || line.starts_with('⟨') || line.starts_with('[')
}

#[test]
fn no_clamped_page_resumes_in_the_middle_of_a_line() {
    let mut clamped = 0usize;
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
            &format!("https://example.test/{name}"),
            Some("text/html; charset=utf-8"),
        );
        let md =
            tokenade_core::disclose::maybe_stash(d.md, tokenade_core::disclose::SourceHint::web());
        let mut lines = md.lines();
        while let Some(l) = lines.next() {
            if !l.contains(CLAMP) {
                continue;
            }
            clamped += 1;
            let after = lines.next().unwrap_or("");
            if !opens_a_line_of_the_outline(after) {
                faults.push(format!(
                    "{name}: {}",
                    after.chars().take(60).collect::<String>()
                ));
            }
        }
    }
    // **The corpus does reach the rule.** Without this floor, a corpus that
    // stopped clamping would turn this test green without checking anything.
    assert!(
        clamped >= 8,
        "only {clamped} clamped page(s): this test no longer proves anything"
    );
    assert!(
        faults.is_empty(),
        "{} page(s) resume in the middle of a line, without their section heading:\n  {}",
        faults.len(),
        faults.join("\n  ")
    );
}
