//! The safety proof for [`tokenade_core::html_nesting`].
//!
//! The nesting ceiling runs on every page we parse, so the question that
//! matters is not "does it work on a bomb" — the unit tests cover that — but
//! "can it ever touch a page a user actually reads". This asserts it does not:
//! across the whole capture corpus it returns the input BORROWED, so html5ever
//! receives the identical bytes it received before the ceiling existed and the
//! extracted markdown cannot differ.
//!
//! If this ever fails, the ceiling has started rewriting real documents and the
//! fidelity numbers can no longer be trusted — investigate before re-baselining
//! anything.

use std::borrow::Cow;
use tokenade_core::html_nesting::limit_nesting;

#[test]
fn no_real_page_in_the_corpus_is_rewritten() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../experiments/cf-clearance/corpus");
    // The corpus is a capture of real pages and `.gitignore` carries
    // `/experiments/cf-clearance/corpus/`, so it exists on the machine that
    // captured it and NOWHERE else — not in a fresh clone, not in a second
    // worktree, not on CI. Asserting on a fixture that cannot be there made
    // this test fail everywhere except one laptop, which is the same class as
    // an `include_str!` on a gitignored file: a check that can only run
    // where it was written reports on the author, not on the code.
    //
    // ABSENT and TRUNCATED are kept apart on purpose. Absent is a fact about
    // the checkout and skips; a corpus that is present but short is a fact
    // about the corpus and still fails, because that is the case where the
    // proof silently covers less than it claims.
    if !dir.is_dir() {
        eprintln!(
            "skipped — no capture corpus at {}; it is gitignored, so only the \
             machine that captured it can run this proof",
            dir.display()
        );
        return;
    }
    let mut scanned = 0usize;
    let mut rewritten = Vec::new();
    let mut walk = vec![dir];
    while let Some(d) = walk.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk.push(p);
                continue;
            }
            if !matches!(p.extension().and_then(|s| s.to_str()), Some("html" | "htm")) {
                continue;
            }
            let Ok(html) = std::fs::read_to_string(&p) else {
                continue;
            };
            scanned += 1;
            if matches!(limit_nesting(&html), Cow::Owned(_)) {
                rewritten.push(p.display().to_string());
            }
        }
    }
    assert!(scanned > 50, "corpus missing or truncated: {scanned} pages");
    assert!(
        rewritten.is_empty(),
        "the nesting ceiling rewrote {} real page(s), so extraction output is no \
         longer provably unchanged: {:?}",
        rewritten.len(),
        &rewritten[..rewritten.len().min(10)]
    );
}
