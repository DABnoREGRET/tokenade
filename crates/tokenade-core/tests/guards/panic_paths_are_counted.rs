//! **A panic path in production declares itself and is counted.**
//!
//! `unwrap()` and `expect()` are panics. In a tool that WRAPS the user's
//! commands and speaks in protocol channels, a panic does not cost a message:
//! it costs the agent's turn, and it writes a trace where the reader expected
//! JSON.
//!
//! This ratchet does not forbid them — most bear on an invariant the code
//! has just established, and an `expect` that NAMES that invariant is better
//! than a `Result` propagating an impossibility. It COUNTS them, so that a
//! new panic path is a deliberate act.
//!
//! Count of 2026-09-13: **319**, after handing the most copied idiom over to a
//! declarer — `create_dir_all(p.parent().unwrap())`, written eighteen times,
//! where the `let _ =` already said failure is tolerated while the `unwrap`
//! next to it said the opposite.
//!
//! The first count, made with a `split("#[cfg(test)]")`, announced 306 then
//! 289: it threw away everything after the FIRST test block, so it
//! under-counted by thirty. That is the raison d'être of `srcsweep`, and this
//! ratchet almost came to life with a wrong number for forgetting it.

use std::path::{Path, PathBuf};

/// The ceiling. **It must go down.**
const CEILING: usize = 319;

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .to_path_buf()
}

#[test]
fn the_number_of_panic_paths_in_production_is_counted_and_shrinking() {
    let mut total = 0usize;
    let mut files = 0usize;
    let mut stack = vec![crates_dir()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if name == "target" || name == "tests" || name.starts_with('.') {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            if !p.extension().is_some_and(|x| x == "rs") || name.ends_with("_tests.rs") {
                continue;
            }
            let Ok(s) = std::fs::read_to_string(&p) else {
                continue;
            };
            // The half that SHIPS, through the declarer: a test is allowed to
            // panic, it is even its way of failing.
            let production = tokenade_core::srcsweep::production_source(&s);
            files += 1;
            total += production.matches(concat!(".unw", "rap()")).count()
                + production.matches(concat!(".exp", "ect(")).count();
        }
    }
    assert!(
        files > 500,
        "only {files} production files read — the sweep got lost"
    );
    assert!(
        total <= CEILING,
        "production panic paths have grown: {total}, ceiling {CEILING}. \
         An invariant the code has just established deserves an `expect` that NAMES it; \
         an I/O or a parse deserves a `Result` or a measured fallback."
    );
}
