//! **What the document corpus keeps must not go down.**
//!
//! A two-month-old oracle that nothing consulted was once found and
//! wired in; an oracle wired in but not GUARDED is the same defect one day
//! later. This ratchet reads `tests/fixtures/docs_recall_baseline.json` — the number of needles found
//! per document, as measured when the baseline was set — and
//! refuses that a document keep fewer.
//!
//! **Per document, not on the total.** A total can rise while a file
//! collapses: a net gain once hid two losses. And the
//! DENOMINATOR is written in the baseline, so a needle removed from the
//! manifest cannot disguise itself as progress.
//!
//! Regenerate after a GAIN, never to silence a drop:
//! `cargo test -p tokenade-bench --test docs_recall_ratchet -- --ignored regen`

use std::collections::BTreeMap;
use tokenade_bench::docs_corpus::{canary, measure_all, NOT_A_DOCUMENT};

/// NEXT TO the ratchet, not inside either of the two corpora: the baseline covers both, and
/// filing it in `docs-eval` would suggest it only speaks about that one.
fn baseline_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/docs_recall_baseline.json")
}

/// `{ "ar.pdf": [7, 14], … }` — kept, asked.
fn baseline() -> BTreeMap<String, (usize, usize)> {
    let raw = std::fs::read_to_string(baseline_path()).expect("readable baseline");
    let v: BTreeMap<String, Vec<usize>> = serde_json::from_str(&raw).expect("baseline JSON");
    v.into_iter().map(|(k, n)| (k, (n[0], n[1]))).collect()
}

#[test]
fn the_canary_still_reproduces_the_defect_it_names() {
    assert_eq!(
        canary(false),
        0,
        "the docs_corpus canary no longer reproduces \"a needle across a line \
         break found, a font-table word lost\": the measurement under this \
         ratchet no longer proves anything"
    );
}

#[test]
fn no_document_keeps_fewer_needles_than_it_did() {
    let base = baseline();
    let rows = measure_all(None);
    let mut regressions = Vec::new();
    let mut unseen: Vec<&String> = base.keys().collect();
    for r in &rows {
        if NOT_A_DOCUMENT.contains(&r.format.as_str()) {
            continue;
        }
        unseen.retain(|k| *k != &r.file);
        let Some(&(was_found, was_total)) = base.get(&r.file) else {
            // A new document is not a regression, but it must enter the
            // baseline: otherwise it is measured without ever being guarded.
            regressions.push(format!(
                "{} is not in the baseline ({}/{} today) — regenerate",
                r.file, r.found, r.total
            ));
            continue;
        };
        assert_eq!(
            r.total, was_total,
            "{} asked for {was_total} needles and now asks for {}: the denominator \
             moved, so the two figures do not compare",
            r.file, r.total
        );
        if !r.served.is_empty() {
            // **The other half, and it did not exist.** This ratchet could only
            // refuse a LOSS. A document under revision carries its deleted
            // text and its reviewer's name; serving them is not losing
            // something, it is inventing something, and fabrication ranks
            // above loss. No baseline enters here: what
            // must not be served must NEVER be, there is no
            // floor to defend.
            regressions.push(format!(
                "{} serves what the document does not say: {}",
                r.file,
                r.served.join(" · ")
            ));
        }
        if r.found < was_found {
            regressions.push(format!(
                "{}: {}/{} versus {was_found}/{was_total} — lost: {}",
                r.file,
                r.found,
                r.total,
                r.missing.join(" · ")
            ));
        }
    }
    assert!(
        unseen.is_empty(),
        "the baseline keeps documents the manifest no longer describes: {unseen:?}"
    );
    assert!(
        regressions.is_empty(),
        "the document corpus is losing ground:\n  {}",
        regressions.join("\n  ")
    );
}

/// The floor is not the only thing that matters: a ratchet set on an empty
/// corpus would be green and prove nothing (denominator floor). Counted over
/// BOTH corpora, so unplugging one turns this red.
#[test]
fn the_corpus_is_actually_reached() {
    let rows = measure_all(None);
    let docs = rows
        .iter()
        .filter(|r| !NOT_A_DOCUMENT.contains(&r.format.as_str()))
        .count();
    assert!(
        docs >= 73,
        "only {docs} document(s) measured: one of the two corpora is not reached"
    );
    let needles: usize = rows
        .iter()
        .filter(|r| !NOT_A_DOCUMENT.contains(&r.format.as_str()))
        .map(|r| r.total)
        .sum();
    assert!(needles >= 415, "only {needles} needle(s) asked");
}

/// **The same content, written differently, must read the same.**
///
/// The manifest's needles only cover what someone thought to declare;
/// `docs_corpus::pairs` asks the question without a list, by comparing the words one
/// spelling delivers and another does not. Three divergences REMAIN and are
/// named here rather than hushed up: they are the next questions, not noise.
///
/// The family floor is what stops the ratchet from turning green by
/// no longer having anything to compare.
#[test]
fn two_spellings_of_one_document_read_the_same() {
    let rows = tokenade_bench::docs_corpus::pairs(None);
    let families: std::collections::BTreeSet<&str> = rows.iter().map(|r| r.stem.as_str()).collect();
    let comparable = tokenade_bench::docs_corpus::comparable_families();
    assert!(
        comparable >= 16,
        "only {comparable} comparable famil(y/ies): the corpus is no longer reached"
    );
    let seen: Vec<String> = rows
        .iter()
        .map(|r| format!("{} · {} ABSENT de {}", r.stem, r.word, r.absent.join(" ")))
        .collect();
    assert!(
        rows.len() <= 3,
        "{} divergence(s) instead of 3 at most, over {} famil(y/ies):\n{}",
        rows.len(),
        families.len(),
        seen.join("\n")
    );
}

#[test]
#[ignore = "regenerates the baseline — run after a GAIN, never to silence a drop"]
fn regen_recall_baseline() {
    let rows = measure_all(None);
    let map: BTreeMap<String, [usize; 2]> = rows
        .iter()
        .filter(|r| !NOT_A_DOCUMENT.contains(&r.format.as_str()))
        .map(|r| (r.file.clone(), [r.found, r.total]))
        .collect();
    let out = serde_json::to_string_pretty(&map).expect("serializable");
    std::fs::write(baseline_path(), out + "\n").expect("writable baseline");
    println!("baseline rewritten: {} document(s)", map.len());
}

/// **The path the product calls the cheapest must answer.**
///
/// `truth/questions.json` gives a natural-language question and the exact token
/// that answers it, for each of the ten documents of `docs-audit`. Measured on
/// 2026-09-11: **10/10**, through `rag::answer_over_blob`, the retrieval
/// core behind every "ask a stash". The figure was right the
/// first time; this ratchet exists so that it stays so, because nothing else
/// links a retriever regression to a DOCUMENT.
///
/// The verdict distinguishes three cases, not two: a token absent from the extraction
/// does not blame the retriever for a loss made one floor below.
#[test]
fn every_document_answers_the_question_its_truth_file_asks() {
    let rows = tokenade_bench::docs_corpus::ask_all(None);
    assert!(
        rows.len() >= 10,
        "only {} question(s) asked: the oracle is not reached",
        rows.len()
    );
    let mut lost = Vec::new();
    for r in &rows {
        if !r.in_extract {
            lost.push(format!(
                "{}: {} ABSENT FROM THE EXTRACTION — it is not the retriever",
                r.file, r.token
            ));
        } else if !r.answered {
            lost.push(format!(
                "{}: {} lost by the retriever on \"{}\" ({} chars returned)",
                r.file, r.token, r.question, r.chars
            ));
        }
    }
    assert!(
        lost.is_empty(),
        "documents no longer answer their own question:\n  {}",
        lost.join("\n  ")
    );
    // **"Answered" proves nothing without a denominator.** A retriever that returns
    // the WHOLE document answers every question, and a mutant that
    // replaced the search with "all the text" survived the first
    // version of this ratchet — which is the proof, not a fear.
    //
    // **The threshold is set where it holds in BOTH environments**, and the
    // first version's was not.
    //
    // Set at 10,000, this ratchet passed on the bench and went red in the suite —
    // `pdf_small.pdf`, 11,679 characters, came back whole. The cause is not the
    // runner: it is `TOKENADE_PRESSURE=0.35`, which the session wrapper
    // exports into the shell and which `cargo run` inherits. Under pressure the budget
    // shrinks and this document gets selected (1,521 chars); without it, it
    // fits whole in the budget and comes back as is — which is the right
    // behaviour, 11,679 characters being ~2,900 tokens, under the
    // stashing threshold. The repository already knew this trap:
    // `a_measurement_does_not_inherit_the_operators_pressure`, measured at a
    // factor of 2.5 on another capture.
    //
    // Measured in a CLEAN environment, the cliff is between 11,679 (whole)
    // and 36,825 (6 % returned). The threshold is therefore above the document that
    // rightfully comes back whole, and the three big ones select under both pressures
    // — 2,249, 2,981 and 2,717 characters, identical to the byte.
    const BIG_ENOUGH_TO_SELECT: usize = 20_000;
    let mut dumped = Vec::new();
    let mut selecting = 0usize;
    for r in &rows {
        if r.extract_chars < BIG_ENOUGH_TO_SELECT {
            continue;
        }
        selecting += 1;
        if r.chars * 2 >= r.extract_chars {
            dumped.push(format!(
                "{}: {} chars returned out of {} — that is not a selection",
                r.file, r.chars, r.extract_chars
            ));
        }
    }
    assert!(
        selecting >= 3,
        "only {selecting} document(s) big enough for the selection to \
         show: half of this ratchet measures nothing"
    );
    assert!(
        dumped.is_empty(),
        "the retriever returns the document instead of searching it:\n  {}",
        dumped.join("\n  ")
    );
}
