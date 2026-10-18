//! **The summary body can come from the caller, and it ARRIVES.**
//!
//! A batch of web pages is assembled then summarised as a single document, and the
//! outline of the whole is cut at the head and the tail: the MIDDLE parts
//! vanish entirely. Measured on four pages, two were
//! missing from the answer. `maybe_stash_with_body` lets the caller — the only one
//! that knows its document is made of N parts — build the preview, while
//! the STASH keeps the full text and the retrieval pointer.
//!
//! Written because the mutant "the caller's body is ignored" SURVIVED:
//! the batch unit tests checked the built body and never that it
//! went through the stashing.

fn big_document() -> String {
    let mut s = String::from("# Un titre\n");
    for i in 0..4_000 {
        s.push_str(&format!(
            "## Section {i}\nune ligne de contenu numéro {i}\n"
        ));
    }
    s
}

#[test]
fn the_body_the_caller_supplies_is_the_body_delivered() {
    let text = big_document();
    let mine = "# ma part à moi\nune ligne que le plan n'aurait jamais écrite".to_string();
    let out = tokenade_core::disclose::maybe_stash_with_body(
        text.clone(),
        tokenade_core::disclose::SourceHint::web(),
        mine.clone(),
    );
    // It really was stashed — otherwise this test proves nothing.
    assert!(
        out.contains("[stashed:"),
        "nothing was stashed:\n{out:.400}"
    );
    assert!(
        out.contains(&mine),
        "the caller's body did not arrive:\n{out:.400}"
    );
    // And it really is IT that replaces the outline, not an addition next to it.
    assert!(
        !out.contains("## Section 2000"),
        "the computed outline is there too:\n{out:.400}"
    );
    assert!(out.contains("mode=md-outline-per-part"), "{out:.200}");
}

/// Under the threshold, nothing is stashed and the text goes out whole: the
/// offered body then has nothing to replace.
#[test]
fn a_small_text_is_served_whole_and_ignores_the_offered_body() {
    let out = tokenade_core::disclose::maybe_stash_with_body(
        "# court\nune ligne\n".to_string(),
        tokenade_core::disclose::SourceHint::web(),
        "JAMAIS".to_string(),
    );
    assert!(!out.contains("JAMAIS"), "{out}");
    assert!(out.contains("une ligne"), "{out}");
}
