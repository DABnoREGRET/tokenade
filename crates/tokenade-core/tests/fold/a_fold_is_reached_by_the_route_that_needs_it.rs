//! **A fold is only worth something if the route that needs it calls it.**
//!
//! These two tests lived IN the fold layer, and they named modules it cannot
//! reach: `filter`, the commands router, and `disclose`, the gate. As long as
//! everything fit in a single crate, nobody saw it — the layer guard only
//! measures the PRODUCTION half of a file, and a test is free to go up. The
//! extraction of `tk-fold` on 2026-09-13 said so in three compile errors.
//!
//! They are not lost for all that: this is where they belong, in a target
//! that sees the whole crate. It is "count the callers of a general
//! mechanism" mirrored: the fold existed, the route did not call it.

/// **The `npm` compactor folds the 128 lines the template knows how to fold.**
///
/// The fold groups those 128 repetitions; the path handling an `npm build`
/// still has to call it. The answer was first no.
#[test]
fn the_npm_compactor_folds_them_too() {
    let body = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/linux/npm_build.txt"),
    )
    .expect("the corpus's npm capture");
    let count = |t: &str| t.matches("can't recognize the exported").count();
    assert_eq!(count(&body), 128);

    let out =
        tokenade_core::filter::filter_command(&["npm".to_string(), "build".to_string()], &body)
            .output;
    assert!(
        count(&out) <= 1,
        "the compactor lets the same sentence through {} times",
        count(&out)
    );
}

/// **A screen of escapes is not a binary, on both sides.**
///
/// `textshape` answers "this is not text" on a buffer made only of ANSI
/// sequences — there is nothing to assert. The gate must answer "this is not
/// a binary" on the same buffer, otherwise a screen clear goes to the stash
/// under an invented extension. The two verdicts live in two layers; the
/// corollary is tested here.
#[test]
fn a_buffer_of_pure_escapes_is_not_binary_either() {
    let v = b"\x1b[2J\x1b[H\x1b[0m".repeat(20);
    assert!(!tokenade_core::textshape::looks_like_text(&v));
    assert_eq!(tokenade_core::disclose::detect_binary(&v), None);
}
