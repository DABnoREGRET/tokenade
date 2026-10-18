//! Compacting the SAME bytes twice must produce the SAME bytes.
//!
//! Why this is a correctness property and not a nicety: every compacted tool
//! result becomes part of the model's prompt prefix. A compactor whose output
//! wobbles between runs — a `HashMap` iteration order that reaches the page, an
//! elapsed-time string, a pid — rewrites that prefix on a re-run of an
//! unchanged command, and the prompt cache misses on a block that never
//! actually changed. The agent pays full price for identical content, and
//! nothing in the savings number ever shows it.
//!
//! Rust reseeds `RandomState` per `HashMap`, so two compactions inside this one
//! test process already draw different hash keys — an in-process double run is
//! enough to surface order-dependent output. (It found `accesslog`'s untied
//! `sort_by_key` this way.)
//!
//! Idea from `tokf verify` (the tokf project).

// `corpus` is mounted by `main.rs` for the whole binary: a single copy
// compiled instead of one per subject.
use crate::corpus;

use std::path::PathBuf;

fn fixture_dirs() -> Vec<PathBuf> {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    ["macos", "windows"].iter().map(|d| base.join(d)).collect()
}

/// Fixtures are named after the command that produced them
/// (`netstat_an_win.txt`, `ps_get_service.txt`). Walk the `_`-separated stem
/// from the left and keep the longest prefix the router actually knows, so
/// `git_listing` routes as `git` rather than falling through unrouted.
fn prog_for_stem(stem: &str) -> Option<String> {
    corpus::program_of(stem)
}

fn fixtures() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for dir in fixture_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x == "txt").unwrap_or(false))
            .collect();
        // Sorted so a failure message names the same fixture on every run.
        paths.sort();
        for p in paths {
            let Ok(body) = std::fs::read_to_string(&p) else {
                continue;
            };
            if body.trim().is_empty() {
                continue;
            }
            let stem = p.file_stem().unwrap().to_string_lossy().to_string();
            out.push((stem, body));
        }
    }
    out
}

#[test]
fn auto_compact_is_deterministic_across_the_fixture_corpus() {
    let fx = fixtures();
    assert!(!fx.is_empty(), "no fixtures found — corpus path moved?");
    let mut wobbled = Vec::new();
    for (stem, body) in &fx {
        let (_, first, _) = tokenade_core::auto::compact(body);
        let (_, second, _) = tokenade_core::auto::compact(body);
        if first != second {
            wobbled.push(stem.clone());
        }
    }
    assert!(
        wobbled.is_empty(),
        "auto::compact returned different bytes for identical input ({} fixture(s)): {:?}",
        wobbled.len(),
        wobbled
    );
}

#[test]
fn per_command_filters_are_deterministic_across_the_fixture_corpus() {
    let fx = fixtures();
    let mut routed = 0usize;
    let mut wobbled = Vec::new();
    for (stem, body) in &fx {
        let Some(prog) = prog_for_stem(stem) else {
            continue;
        };
        routed += 1;
        let argv = vec![prog];
        let first = tokenade_core::filter::filter_command(&argv, body).output;
        let second = tokenade_core::filter::filter_command(&argv, body).output;
        if first != second {
            wobbled.push(stem.clone());
        }
    }
    assert!(
        routed > 20,
        "only {routed} fixtures routed to a known program — the stem→prog mapping broke"
    );
    assert!(
        wobbled.is_empty(),
        "filter_command returned different bytes for identical input ({} fixture(s)): {:?}",
        wobbled.len(),
        wobbled
    );
}

/// Regression guard for the concrete bug this harness found: an access-log
/// summary sorted only by count let tied request patterns come out in
/// `HashMap` order.
///
/// The paths must be non-numeric: `path_template` rewrites digit runs to
/// `<id>`, so `/route/0`…`/route/9` would fold into ONE tuple with count 10
/// and there would be no tie left to mis-order. Word segments survive
/// templating, giving 12 distinct patterns all tied at a single hit — the
/// shape that makes the wobble certain rather than lucky.
#[test]
fn access_log_ties_do_not_ride_hashmap_order() {
    let mut log = String::new();
    for name in [
        "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india",
        "juliet", "kilo", "lima",
    ] {
        log.push_str(&format!(
            "10.0.0.1 - - [29/Jul/2026:12:00:00 +0000] \"GET /route/{name} HTTP/1.1\" 200 512 \"-\" \"curl/8.0\"\n"
        ));
    }
    let argv = vec!["cat".to_string(), "access.log".to_string()];
    let a = tokenade_core::filter::filter_command(&argv, &log).output;
    let b = tokenade_core::filter::filter_command(&argv, &log).output;
    assert_eq!(
        a, b,
        "tied access-log request patterns must have a stable order"
    );
    let direct_a = tokenade_core::accesslog::compact(&log);
    let direct_b = tokenade_core::accesslog::compact(&log);
    assert_eq!(
        direct_a, direct_b,
        "accesslog::compact must be order-stable on tied counts"
    );
}

/// The web path was outside this harness, and it is the one with the most
/// moving parts: a DOM walk, a subtree deduper, a reader registry. Every one of
/// those is a place an unordered map can reach the page.
#[test]
fn html_to_markdown_is_deterministic_across_the_page_corpus() {
    let pages = corpus::pages();
    assert!(!pages.is_empty(), "no pages found — corpus path moved?");
    let mut wobbled = Vec::new();
    for (p, html) in &pages {
        let first = tokenade_core::web::html_to_markdown(html);
        let second = tokenade_core::web::html_to_markdown(html);
        if first != second {
            wobbled.push(p.clone());
        }
    }
    assert!(
        wobbled.is_empty(),
        "html_to_markdown returned different bytes for identical input ({} page(s)): {:?}",
        wobbled.len(),
        wobbled
    );
}

/// A results page is ranked output, and ranking is exactly where a tie broken
/// by map order changes WHICH results survive the cut — not just their order.
#[test]
fn saved_serp_extraction_is_deterministic() {
    let pages = corpus::serp_pages();
    assert!(
        !pages.is_empty(),
        "no serp fixtures found — corpus path moved?"
    );
    let mut wobbled = Vec::new();
    for (p, html) in &pages {
        let render = |h: &str| {
            let (hits, engine) = tokenade_core::search::extract_saved_page(h);
            let outcome = tokenade_core::search::Outcome {
                results: hits,
                used: engine.map(|e| vec![e.name()]).unwrap_or_default(),
                refused: Vec::new(),
            };
            tokenade_core::search::render("saved page", &outcome)
        };
        if render(html) != render(html) {
            wobbled.push(p.clone());
        }
    }
    assert!(
        wobbled.is_empty(),
        "saved-page extraction wobbled ({} page(s)): {:?}",
        wobbled.len(),
        wobbled
    );
}
