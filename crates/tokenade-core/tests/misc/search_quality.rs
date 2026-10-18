//! Battery 5 — SEARCH QUALITY (hermetic).
//!
//! The four-arm ranking bake-off (D4) is Stage 4's job — it needs frozen
//! per-engine SERP fixtures for every judged query, and today's frozen corpus
//! (`tests/fixtures/serp/*.html`) only covers ONE query, "python asyncio vs
//! threading", across five engines. This battery builds the SCAFFOLDING Stage
//! 4 will run inside: the judged gold set (`fixtures/judgments.json`), a thin
//! cross-engine merge (dedup via the real, public `canonical_url` — `merge_and_rank`
//! itself is private, so this is a proxy for "agreement + best-rank", not the
//! shipped arm), and the metrics the bake-off will read: precision@5, MRR,
//! domain diversity, duplicate-rate (must be 0 — D5's proof), and stability
//! (identical input twice ⇒ byte-identical output).
//!
//!     cargo test -p tokenade-core --test search_quality -- --nocapture

// `ab` is mounted by `main.rs` for the whole binary: a single compiled
// copy instead of one per subject.
use crate::ab;

use std::collections::{HashMap, HashSet};
use tokenade_core::search::{canonical_url, Engine};
use tokenade_core::serp;

#[derive(serde::Deserialize, Clone)]
struct Judgment {
    query: String,
    #[serde(rename = "type")]
    kind: String,
    correct: Vec<String>,
}

fn fixtures_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn judgments() -> Vec<Judgment> {
    let p = fixtures_dir().join("judgments.json");
    let s = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("missing {p:?}: {e}"));
    serde_json::from_str(&s).unwrap_or_else(|e| panic!("judgments.json does not parse: {e}"))
}

fn serp_fixture(name: &str) -> String {
    let p = fixtures_dir().join("serp").join(format!("{name}.html"));
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
}

/// Every judged query MUST have a schema-valid entry, and the judged set MUST
/// span non-English and CJK queries (the battery's multi-language requirement) — an
/// English-only gold set would hide exactly the encoding/query-shaping bugs
/// this project has already shipped.
#[test]
fn judged_set_is_well_formed_and_multilingual() {
    let js = judgments();
    assert!(
        js.len() >= 10,
        "judged set too small: {} queries, expected ~15",
        js.len()
    );
    for j in &js {
        assert!(!j.query.trim().is_empty(), "empty query");
        assert!(
            !j.correct.is_empty(),
            "{}: no correct URLs recorded",
            j.query
        );
        assert!(
            matches!(
                j.kind.as_str(),
                "navigational" | "informational" | "long-tail"
            ),
            "{}: unknown type {:?}",
            j.query,
            j.kind
        );
    }
    let non_ascii = js.iter().filter(|j| !j.query.is_ascii()).count();
    let cjk = js.iter().any(|j| {
        j.query
            .chars()
            .any(|c| matches!(c as u32, 0x3040..=0x30FF | 0x4E00..=0x9FFF))
    });
    ab::assert_floor(
        non_ascii >= 2,
        "judged set needs >= 2 non-ASCII queries (a CJK one and a non-English one)",
    );
    ab::assert_floor(cjk, "judged set needs at least one CJK query");
}

/// Cross-engine agreement + best-native-rank, deduped via the real
/// `canonical_url` — a PROXY for the shipped `merge_and_rank` (private),
/// built only to exercise the metrics below. Stage 4 replaces this with the
/// bake-off's actual arms.
fn thin_merge(per_engine: &[Vec<serp::Result>]) -> Vec<serp::Result> {
    let mut agreement: HashMap<String, (serp::Result, usize, usize)> = HashMap::new();
    for hits in per_engine {
        for (rank, r) in hits.iter().enumerate() {
            let key = canonical_url(&r.url);
            agreement
                .entry(key)
                .and_modify(|(_, count, best_rank)| {
                    *count += 1;
                    *best_rank = (*best_rank).min(rank);
                })
                .or_insert((r.clone(), 1, rank));
        }
    }
    let mut merged: Vec<(serp::Result, usize, usize)> = agreement.into_values().collect();
    // Ties break on canonical URL — a total order, so re-running on identical
    // input is byte-identical (the stability metric below).
    merged.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then(a.2.cmp(&b.2))
            .then(canonical_url(&a.0.url).cmp(&canonical_url(&b.0.url)))
    });
    merged.into_iter().map(|(r, _, _)| r).collect()
}

fn host_of(url: &str) -> String {
    let c = canonical_url(url);
    c.split("://")
        .nth(1)
        .unwrap_or(&c)
        .split('/')
        .next()
        .unwrap_or("")
        .to_string()
}

fn precision_at_5(merged: &[serp::Result], correct: &HashSet<String>) -> f64 {
    let top5 = &merged[..merged.len().min(5)];
    if top5.is_empty() {
        return 0.0;
    }
    let hits = top5
        .iter()
        .filter(|r| correct.contains(&canonical_url(&r.url)))
        .count();
    hits as f64 / 5.0
}

fn mrr(merged: &[serp::Result], correct: &HashSet<String>) -> f64 {
    for (i, r) in merged.iter().enumerate() {
        if correct.contains(&canonical_url(&r.url)) {
            return 1.0 / (i as f64 + 1.0);
        }
    }
    0.0
}

/// Battery 5. The only judged query with real frozen per-engine SERP bytes
/// today is "python asyncio vs threading" — the other ~14 entries are the
/// gold-set scaffolding Stage 4's per-query fixture capture will fill in.
#[test]
fn battery5_search_quality() {
    let js = judgments();
    let j = js
        .iter()
        .find(|j| j.query == "python asyncio vs threading")
        .expect("judgments.json must include the one query with frozen SERP fixtures");
    let correct: HashSet<String> = j.correct.iter().map(|u| canonical_url(u)).collect();

    let per_engine: Vec<Vec<serp::Result>> = [
        (Engine::Ecosia, "ecosia"),
        (Engine::Bing, "bing"),
        (Engine::Yahoo, "yahoo"),
        (Engine::Mojeek, "mojeek"),
        (Engine::Marginalia, "marginalia"),
    ]
    .iter()
    .map(|(engine, name)| engine.parse(&serp_fixture(name)))
    .collect();

    let merged = thin_merge(&per_engine);
    ab::assert_floor(
        !merged.is_empty(),
        "the merge of five real engine pages produced nothing",
    );

    // Duplicate-rate after canonicalisation — D5's proof, MUST be 0.
    let mut seen = HashSet::new();
    let mut dupes = 0usize;
    for r in &merged {
        if !seen.insert(canonical_url(&r.url)) {
            dupes += 1;
        }
    }
    let dup_rate = dupes as f64 / merged.len() as f64 * 100.0;
    ab::assert_floor(
        dupes == 0,
        format!("{dupes} duplicate canonical URLs survived the merge"),
    );

    // Stability — identical input twice, byte-identical output.
    let merged_again = thin_merge(&per_engine);
    let a: Vec<&str> = merged.iter().map(|r| r.url.as_str()).collect();
    let b: Vec<&str> = merged_again.iter().map(|r| r.url.as_str()).collect();
    ab::assert_floor(
        a == b,
        "the merge is not stable: identical input produced a different order",
    );

    let p5 = precision_at_5(&merged, &correct);
    let m = mrr(&merged, &correct);
    let top10 = &merged[..merged.len().min(10)];
    let diversity = top10
        .iter()
        .map(|r| host_of(&r.url))
        .collect::<HashSet<_>>()
        .len();

    println!(
        "\nBattery 5 (query: {:?}): {} merged results, precision@5={p5:.2}, MRR={m:.3}, \
         diversity={diversity}/{}, duplicate-rate={dup_rate:.1}%\n",
        j.query,
        merged.len(),
        top10.len(),
    );

    let metrics = [
        ab::Metric::new("precision@5", p5 * 100.0, p5 * 100.0, ab::Unit::Pct, true),
        ab::Metric::new("MRR", m, m, ab::Unit::Ratio, true),
        ab::Metric::new(
            "domain diversity (top10)",
            diversity as f64,
            diversity as f64,
            ab::Unit::Count,
            true,
        ),
        ab::Metric::new("duplicate-rate", dup_rate, dup_rate, ab::Unit::Pct, false),
    ];
    ab::print_table(&metrics);
}

/// Refusal-classification stub (D10). Replaying captured challenge/thin/
/// off-topic bodies against a typed classifier is Stage 4's job (it needs a
/// corpus of each); today the one captured challenge page proves the two
/// signals the classifier combines already work on real bytes.
///
/// NOTE the split, straight from `serp::looks_like_challenge`'s own doc: the
/// captured page carries exactly ONE distinct marker (`captcha`, six times),
/// which alone must NOT trip a two-marker verdict — a genuine results page
/// about CAPTCHAs carries one too. The caller (`search::fetch_and_validate`)
/// types it as a challenge by combining ONE marker with ZERO parsed results,
/// which no real SERP produces. That is what this replays.
#[test]
fn a_captured_challenge_page_is_typed_as_a_challenge() {
    let html = serp_fixture("bing_challenge");
    ab::assert_floor(
        !serp::looks_like_challenge(&html),
        "one marker alone must not trip the two-marker verdict (see the doc on looks_like_challenge)",
    );
    ab::assert_floor(
        serp::challenge_markers(&html) >= 1,
        "the captured challenge page must carry at least one challenge marker",
    );
    ab::assert_floor(
        Engine::Bing.parse(&html).is_empty(),
        "the captured challenge page must parse to zero results",
    );
}
