//! A question the source cannot answer must come back as "nothing matched",
//! never as passages shaped like an answer.
//!
//! This is the failure mode that costs the most: a miss the caller can see is
//! a miss costs one wasted call, while a miss that LOOKS like an answer is
//! taken as one. The gate runs both directions on purpose — a rule that only
//! checked the refusals would be satisfied by refusing everything.

use tokenade_core::rag::prompt_scoped_extract_dispatch;

/// A document with a vocabulary of its own, and enough blank lines, headings
/// and code blocks to make the function words `of` / `an` / `with` genuinely
/// RARE line-wise — which is exactly how they used to clear the anchor test.
fn source() -> String {
    let mut s = String::from("# Ledger service\n\n## Retention\n\n");
    for i in 0..40 {
        s.push_str(&format!(
            "- Batch {i} is trimmed after ninety days of inactivity.\n\n### Batch {i}\n\n```\nbatch_{i}.retain(|row| row.age < 90)\n```\n\n"
        ));
    }
    s.push_str(
        "## Encryption\n\nRows are sealed with a rotating envelope key before they reach disk.\n",
    );
    s
}

/// Questions whose content words appear nowhere in the source. Each carries
/// ordinary function words, which is the whole point: those must not be what
/// decides the question has a foothold.
const UNANSWERABLE: &[&str] = &[
    "what is the airspeed velocity of an unladen swallow",
    "how do I renew the tls certificate on the load balancer",
    "which cinema is showing the film tonight",
    "does it come with a warranty for the motorcycle",
];

/// Questions the source really does answer. Worded with the source's OWN
/// vocabulary — retrieval here is lexical, with no stemming, so `batches`
/// does not reach `batch` and a fixture that asks for one while the document
/// says the other is testing the fixture, not the product.
const ANSWERABLE: &[&str] = &[
    "how long before a batch is trimmed",
    "rows sealed before disk",
    "encryption envelope key",
    "inactivity",
];

#[test]
fn a_question_the_source_cannot_answer_is_refused() {
    let doc = source();
    let mut answered = Vec::new();
    for q in UNANSWERABLE {
        if let Some(got) = prompt_scoped_extract_dispatch(&doc, q, 800) {
            answered.push(format!(
                "{q:?} got {} chars of passages instead of a refusal",
                got.len()
            ));
        }
    }
    assert!(answered.is_empty(), "{}", answered.join("\n"));
}

#[test]
fn a_question_the_source_does_answer_is_still_answered() {
    let doc = source();
    let mut refused = Vec::new();
    for q in ANSWERABLE {
        if prompt_scoped_extract_dispatch(&doc, q, 800).is_none() {
            refused.push(format!("{q:?} was refused by its own source"));
        }
    }
    assert!(refused.is_empty(), "{}", refused.join("\n"));
}
