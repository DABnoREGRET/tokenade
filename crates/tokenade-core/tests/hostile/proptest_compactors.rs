//! Property-based safety net for every compactor.
//!
//! For every shipping compactor we assert two universal properties:
//!
//! 1. **Never panics.** Whatever input we feed it (random bytes, malformed
//!    UTF-8 boundaries, deeply nested JSON, etc.), the compactor must
//!    return a string. Hand-written tests cover *known* failure modes;
//!    proptest covers the ones we didn't think of.
//!
//! 2. **Never massively inflates.** Output token count must be ≤ input × 2
//!    (a generous bound that lets a one-line "[tokenade: …]" banner be added
//!    to a tiny input without failing). Real workloads aim for <0.5×, but
//!    we don't want random fuzz to reject valid bounded behaviour.
//!
//! When a property fails proptest shrinks the input to a minimal reproducer
//! and prints it — exactly the diagnostic value a hand-written test can't
//! offer.

use proptest::prelude::*;
use tokenade_core::{
    auto, diff, env as tokenade_env, filter, json, markdown, redact, semantic, serp, stacktrace,
    tabular, tokenizer, web, yaml as tokenade_yaml,
};

/// Bounded inflation check: allows up to `factor` × ratio AND a fixed
/// 50-token banner allowance, so a compactor that adds `[tokenade:foo …]` to a
/// 1-token input doesn't fail proptest spuriously.
fn bounded_inflation(input: &str, output: &str, factor: f64) -> bool {
    let tb = tokenizer::estimate(input);
    let ta = tokenizer::estimate(output);
    (ta as f64) <= (tb as f64 * factor) + 50.0
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn filter_never_panics_never_explodes(
        cmd in "[a-z]{1,8}",
        sub in "[a-z]{0,8}",
        out in "[\\x20-\\x7e\n\t]{0,4096}"
    ) {
        let argv = if sub.is_empty() {
            vec![cmd]
        } else {
            vec![cmd, sub]
        };
        let r = filter::filter_command(&argv, &out);
        prop_assert!(bounded_inflation(&out, &r.output, 2.0));
    }

    #[test]
    fn json_never_panics(s in "[\\x20-\\x7e\n\t]{0,2048}") {
        let out = json::compact(&s);
        prop_assert!(bounded_inflation(&s, &out, 2.0));
    }

    #[test]
    fn yaml_never_panics(s in "[\\x20-\\x7e\n\t]{0,2048}") {
        let out = tokenade_yaml::compact(&s);
        prop_assert!(bounded_inflation(&s, &out, 2.0));
    }

    #[test]
    fn markdown_never_panics(s in "[\\x20-\\x7e\n\t]{0,4096}") {
        let out = markdown::compact(&s);
        prop_assert!(bounded_inflation(&s, &out, 2.0));
    }

    #[test]
    fn diff_never_panics(s in "[\\x20-\\x7e\n\t]{0,4096}") {
        let out = diff::compact(&s);
        // 3× ceiling: the `[tokenade:diff no-context]` banner can inflate a
        // tiny input. Real diffs shrink by 70 %+ — proptest is here to
        // catch *panics* and runaway inflation, not enforce best-case savings.
        prop_assert!(bounded_inflation(&s, &out, 2.0));
    }

    #[test]
    fn stacktrace_never_panics(s in "[\\x20-\\x7e\n\t]{0,4096}") {
        let out = stacktrace::compact(&s);
        prop_assert!(bounded_inflation(&s, &out, 2.0));
    }

    #[test]
    fn web_html_never_panics(s in "[\\x20-\\x7e\n\t]{0,4096}") {
        let out = web::html_to_markdown(&s);
        prop_assert!(bounded_inflation(&s, &out, 2.0));
    }

    #[test]
    fn a11y_never_panics(s in "[\\x20-\\x7e\n\t]{0,4096}") {
        let out = web::compact_a11y_tree(&s);
        prop_assert!(bounded_inflation(&s, &out, 2.0));
    }

    #[test]
    fn serp_never_panics(s in "[\\x20-\\x7e\n\t]{0,4096}") {
        let out = serp::compact(&s);
        prop_assert!(bounded_inflation(&s, &out, 2.0));
    }

    #[test]
    fn tabular_never_panics(s in "[\\x20-\\x7e\n\t]{0,4096}") {
        let out = tabular::compact(&s);
        prop_assert!(bounded_inflation(&s, &out, 2.0));
    }

    #[test]
    fn env_never_panics_and_redacts(s in "[A-Z_]{1,16}=[\\x20-\\x7e]{0,128}\n*") {
        let out = tokenade_env::compact(&s);
        prop_assert!(bounded_inflation(&s, &out, 2.0));
        // Env compactor must drop common secret patterns silently.
        for needle in ["TOKEN=", "SECRET=", "PASSWORD=", "API_KEY="] {
            if let Some(idx) = s.find(needle) {
                // If the input contained a secret-shaped assignment, the
                // OUTPUT should not contain the value verbatim — only the
                // key + a redaction marker.
                let value: String = s[idx + needle.len()..]
                    .chars()
                    .take_while(|c| !c.is_whitespace())
                    .collect();
                if value.len() >= 6 {
                    prop_assert!(
                        !out.contains(&value),
                        "env compactor leaked secret value: input={s:?} output={out:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn auto_compact_never_panics(s in "[\\x20-\\x7e\n\t]{0,4096}") {
        let (_, out, _) = auto::compact(&s);
        prop_assert!(bounded_inflation(&s, &out, 2.0));
    }

    #[test]
    fn redact_idempotent(s in "[\\x20-\\x7e\n\t]{0,2048}") {
        let once = redact::redact(&s);
        let twice = redact::redact(&once);
        prop_assert_eq!(once, twice);
    }

    #[test]
    fn semantic_search_never_panics(query in "[a-zA-Z _]{0,64}") {
        // Build a tiny index and verify search doesn't crash on any query.
        let sem = semantic::SemanticIndex::default();
        let _ = sem.search(&query, 10);
        let _ = sem.search(&query, 0);
    }
}
