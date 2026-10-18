//! **Two tables that must stay aligned, and that are not in the same layer.**
//!
//! `detect::Agent::all()` enumerates the agents tokenade recognises;
//! `coefficients::agent_web_model_is_measured` says which have a web credit
//! model ESTABLISHED by measurement. An agent added to `detect.rs` must not
//! acquire a model silently — that is the class of defect this repo calls "a
//! hand-enumerated guard lets the next variant through".
//!
//! The test lived in `coefficients_tests.rs`, hence in the fold layer, from
//! which `detect` (product layer) can no longer be named since the
//! extraction of `tk-fold` on 2026-09-13. Its place is here: a guard is
//! exactly the place where two layers look at each other.

/// A canonical slug is measured, or declared unmeasured. Never neither.
#[test]
fn every_canonical_agent_slug_is_measured_or_declared_unmeasured() {
    use tokenade_core::coefficients::agent_web_model_is_measured;

    let unmeasured: Vec<&str> = tokenade_core::detect::Agent::all()
        .iter()
        .map(|a| a.as_str())
        .filter(|s| !agent_web_model_is_measured(s))
        .collect();
    assert_eq!(
        unmeasured,
        vec!["t3-code", "odysseus"],
        "web credit models were classified without evidence, or a new \
         agent changed the set"
    );
    // Non-vacuity: the predicate does answer true for those that ARE classified.
    assert!(agent_web_model_is_measured("claude-code"));
    assert!(agent_web_model_is_measured("copilot-vscode"));
    assert!(!agent_web_model_is_measured("unknown"));
}
