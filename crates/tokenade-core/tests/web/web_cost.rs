//! Battery 6 — TOKEN COST (hermetic, THE VETO).
//!
//! Every other battery asks "is it better"; this one asks "at what price",
//! and its verdict overrides theirs — a change that lifts fidelity while
//! inflating tokens is a regression for a product whose whole purpose is
//! saving them (the Stage 3 gate). Median AND p95 matter separately: p95 is
//! where a pathological page hides behind a healthy median.
//!
//!     cargo test -p tokenade-core --test web_cost -- --nocapture
//!
//! Stage 1 has no candidate yet, so every metric below is baseline vs itself
//! (delta 0.0, verdict `ok`) — this battery exists to prove the VETO MARKER
//! itself renders correctly (see `veto_marker_fires_on_a_token_p95_regression`),
//! so Stage 2/3 can trust it the first time a real candidate regresses.

// `ab` is mounted by `main.rs` for the whole binary: a single copy
// compiled instead of one per subject.
use crate::ab;

use tokenade_core::search::{canonical_url, Engine, Outcome};
use tokenade_core::tokenizer;
use tokenade_core::{search, serp, web_pipeline};

#[derive(serde::Deserialize, Clone)]
struct ManifestEntry {
    file: String,
    class: String,
}

fn fixtures_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn manifest() -> Vec<ManifestEntry> {
    let p = fixtures_dir().join("pages/manifest.json");
    let s = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("missing {p:?}: {e}"));
    serde_json::from_str(&s).unwrap_or_else(|e| panic!("manifest.json does not parse: {e}"))
}

fn page_html(file: &str) -> String {
    let p = fixtures_dir().join("pages").join(file);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
}

fn serp_fixture(name: &str) -> String {
    let p = fixtures_dir().join("serp").join(format!("{name}.html"));
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
}

fn distilled_tokens(entry: &ManifestEntry) -> usize {
    let html = page_html(&entry.file);
    let url = format!(
        "https://fixture.test/pages/{}",
        entry.file.trim_end_matches(".html")
    );
    let d = web_pipeline::distill_page(&html, &url, Some("text/html; charset=utf-8"));
    tokenizer::estimate(&d.md)
}

fn median(sorted: &[usize]) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let n = sorted.len();
    if n % 2 == 1 {
        sorted[n / 2] as f64
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) as f64 / 2.0
    }
}

/// Nearest-rank p95 — fine at this corpus size (~40): the point is to catch a
/// pathological OUTLIER, not to be a statistically precise percentile.
fn p95(sorted: &[usize]) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() as f64) * 0.95).ceil() as usize;
    sorted[idx.saturating_sub(1).min(sorted.len() - 1)] as f64
}

/// Battery 6. Establishes the baseline token-cost numbers Stage 2/3 will
/// diff a candidate against.
#[test]
fn battery6_token_cost() {
    let entries = manifest();
    let mut per_page: Vec<(String, usize)> = entries
        .iter()
        .map(|e| (e.class.clone(), distilled_tokens(e)))
        .collect();
    per_page.sort_by_key(|(_, t)| *t);
    let all_tokens: Vec<usize> = per_page.iter().map(|(_, t)| *t).collect();

    let agg_median = median(&all_tokens);
    let agg_p95 = p95(&all_tokens);
    println!(
        "\nBattery 6: {} pages, median={agg_median:.0} tok, p95={agg_p95:.0} tok\n",
        entries.len()
    );

    let mut metrics = vec![
        ab::Metric::new(
            "distilled page tokens, median",
            agg_median,
            agg_median,
            ab::Unit::Tokens,
            false,
        ),
        ab::Metric::new(
            "distilled page tokens, p95",
            agg_p95,
            agg_p95,
            ab::Unit::Tokens,
            false,
        )
        .vetoed(),
    ];

    let mut classes: Vec<&str> = entries.iter().map(|e| e.class.as_str()).collect();
    classes.sort_unstable();
    classes.dedup();
    for c in &classes {
        let mut xs: Vec<usize> = per_page
            .iter()
            .filter(|(cl, _)| cl == c)
            .map(|(_, t)| *t)
            .collect();
        xs.sort_unstable();
        let med = median(&xs);
        let pp95 = p95(&xs);
        metrics.push(ab::Metric::new(
            format!("tokens, median ({c})"),
            med,
            med,
            ab::Unit::Tokens,
            false,
        ));
        metrics.push(ab::Metric::new(
            format!("tokens, p95 ({c})"),
            pp95,
            pp95,
            ab::Unit::Tokens,
            false,
        ));
    }

    // Tokens for a 10-result search render, on the one query with frozen
    // per-engine SERP bytes.
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
    let mut seen = std::collections::HashSet::new();
    let mut top10 = Vec::new();
    for hits in &per_engine {
        for r in hits {
            if top10.len() >= 10 {
                break;
            }
            if seen.insert(canonical_url(&r.url)) {
                top10.push(r.clone());
            }
        }
    }
    let outcome = Outcome {
        results: top10,
        used: vec!["ecosia", "bing", "yahoo", "mojeek", "marginalia"],
        refused: vec![],
    };
    let render_tokens =
        tokenizer::estimate(&search::render("python asyncio vs threading", &outcome)) as f64;
    metrics.push(ab::Metric::new(
        "tokens, 10-result search render",
        render_tokens,
        render_tokens,
        ab::Unit::Tokens,
        false,
    ));

    // `--answer` (3 distilled pages) vs the naive path (3 raw reads) — three
    // pages spanning different classes so the comparison isn't one page's
    // idiosyncrasy.
    let sample = ["bbc_article.html", "mdn_doc.html", "ikea_product.html"];
    let mut answer_tokens = 0usize;
    let mut naive_tokens = 0usize;
    for file in sample {
        let html = page_html(file);
        naive_tokens += tokenizer::estimate(&html);
        let url = format!(
            "https://fixture.test/pages/{}",
            file.trim_end_matches(".html")
        );
        let d = web_pipeline::distill_page(&html, &url, Some("text/html; charset=utf-8"));
        answer_tokens += tokenizer::estimate(&d.md);
    }
    metrics.push(ab::Metric::new(
        "tokens, 3 pages (--answer, distilled)",
        answer_tokens as f64,
        answer_tokens as f64,
        ab::Unit::Tokens,
        false,
    ));
    metrics.push(ab::Metric::new(
        "tokens, 3 pages (naive, raw)",
        naive_tokens as f64,
        naive_tokens as f64,
        ab::Unit::Tokens,
        false,
    ));

    // Cost per unit of fidelity (Δtokens/Δfidelity). Meaningless until a real
    // candidate exists (Stage 2/3) — recorded here as 0/0 so the row and its
    // column shape exist and later stages only need to fill in `candidate`.
    metrics.push(ab::Metric::new(
        "Δtokens / Δfidelity",
        0.0,
        0.0,
        ab::Unit::Ratio,
        false,
    ));

    ab::print_table(&metrics);
}

/// The veto marker itself, proven on a SYNTHETIC regression — not booked as a
/// real number, just machinery: a token p95 regression must render
/// `REGRESSION  <-- veto`, and nothing else in this battery may.
#[test]
fn veto_marker_fires_on_a_token_p95_regression() {
    let regressed = ab::Metric::new(
        "distilled page tokens, p95",
        4120.0,
        5890.0,
        ab::Unit::Tokens,
        false,
    )
    .vetoed();
    assert_eq!(
        regressed.verdict(),
        ab::Verdict::Regression,
        "a p95 rise of +43% must verdict as a regression"
    );
    assert!(regressed.veto, "the p95 metric must carry the veto flag");

    let improved = ab::Metric::new(
        "distilled page tokens, p95",
        4120.0,
        3000.0,
        ab::Unit::Tokens,
        false,
    )
    .vetoed();
    assert_eq!(
        improved.verdict(),
        ab::Verdict::Better,
        "a p95 drop must verdict as BETTER even when vetoed"
    );

    // Prints the row so a human reading `--nocapture` sees the exact shape
    // `REGRESSION  <-- veto` renders in, without booking it as a real result.
    ab::print_table(&[regressed, improved]);
}
