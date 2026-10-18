//! Live engine checks — OPT-IN, never in CI.
//!
//! The frozen corpus in `serp_corpus.rs` proves the extractors still parse the
//! pages we captured. It cannot prove the engines still SERVE those pages: the
//! dominant failure mode for this component is a silent layout change or a new
//! bot wall, and both happen outside the repo, on someone else's schedule.
//!
//! Run when an engine is suspected, or before a release:
//!
//!     TOKENADE_LIVE_SEARCH_TESTS=1 cargo test -p tokenade-core --test search_live -- --nocapture
//!
//! A failure here means "go look at that engine", not "fix the test". If an
//! engine has genuinely died, remove it from `ENGINES` and say so in the
//! rotation doc-comment — do not weaken the assertion.

use tokenade_core::search::{self, Engine};
use tokenade_core::serp;

fn enabled() -> bool {
    matches!(
        std::env::var("TOKENADE_LIVE_SEARCH_TESTS").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

/// Long enough to be unambiguous, common enough that every index has it.
const QUERY: &str = "python asyncio vs threading";

struct Probe {
    engine: Engine,
    bytes: usize,
    hits: usize,
    plausible: bool,
    challenge: bool,
}

fn probe(engine: Engine) -> Probe {
    match tokenade_core::web_fetch::fetch_search(&engine.url(QUERY), engine.cookie()) {
        None => Probe {
            engine,
            bytes: 0,
            hits: 0,
            plausible: false,
            challenge: false,
        },
        Some(body) => {
            let hits = engine.parse(&body);
            Probe {
                engine,
                bytes: body.len(),
                plausible: serp::plausible_for_query(&hits, QUERY),
                challenge: serp::challenge_markers(&body) >= 1 && hits.is_empty(),
                hits: hits.len(),
            }
        }
    }
}

/// At least half the fleet must be able to answer, or `tokenade search` is a
/// promise we cannot keep. Asserted on the FLEET rather than per engine: any
/// single engine may be throttling us at any moment, and a test that fails on
/// that is a test nobody will run twice.
#[test]
fn most_engines_still_answer() {
    if !enabled() {
        eprintln!("skipped — set TOKENADE_LIVE_SEARCH_TESTS=1 to run");
        return;
    }
    let mut healthy = 0;
    for engine in search::ENGINES {
        let p = probe(engine);
        let verdict = if p.bytes == 0 {
            "unreachable"
        } else if p.challenge {
            "bot-challenge"
        } else if p.hits < search::MIN_HITS {
            "thin"
        } else if !p.plausible {
            "OFF-TOPIC"
        } else {
            healthy += 1;
            "ok"
        };
        eprintln!(
            "{:<12} bytes={:>7} hits={:>3} {verdict}",
            p.engine.name(),
            p.bytes,
            p.hits
        );
    }
    let total = search::ENGINES.len();
    assert!(
        healthy * 2 >= total,
        "only {healthy}/{total} engines healthy — the fleet cannot serve a search"
    );
}

/// The defect that shipped, checked against the live web: an engine that
/// answers must answer about the QUERY. Off-topic is worse than thin, because
/// thin rotates away and off-topic gets served.
#[test]
fn no_engine_serves_off_topic_results() {
    if !enabled() {
        eprintln!("skipped — set TOKENADE_LIVE_SEARCH_TESTS=1 to run");
        return;
    }
    for engine in search::ENGINES {
        let p = probe(engine);
        if p.hits >= search::MIN_HITS {
            assert!(
                p.plausible,
                "{}: returned {} results with nothing to do with {QUERY:?} — \
                 that is a navigation bar or a wall, and it would be SERVED",
                p.engine.name(),
                p.hits
            );
        }
    }
}

/// End to end, through the real merge: a live search must produce usable,
/// on-topic, non-engine results.
#[test]
fn a_live_search_produces_usable_results() {
    if !enabled() {
        eprintln!("skipped — set TOKENADE_LIVE_SEARCH_TESTS=1 to run");
        return;
    }
    let mut health = search::Health::new();
    let outcome = search::run(QUERY, &mut health, tokenade_core::platform::now_unix_secs());
    eprintln!(
        "used={:?} refused={:?} results={}",
        outcome.used,
        outcome
            .refused
            .iter()
            .map(|(e, r)| (*e, r.as_str()))
            .collect::<Vec<_>>(),
        outcome.results.len()
    );
    assert!(outcome.is_usable(), "no usable results from any engine");
    assert!(
        serp::plausible_for_query(&outcome.results, QUERY),
        "the MERGED set is off-topic: {:?}",
        outcome
            .results
            .iter()
            .take(3)
            .map(|r| &r.title)
            .collect::<Vec<_>>()
    );
    for r in &outcome.results {
        assert!(
            r.url.starts_with("http"),
            "relative url survived the merge: {r:?}"
        );
    }
    // Canonicalisation must leave no duplicate pages in the served list.
    let mut keys: Vec<String> = outcome
        .results
        .iter()
        .map(|r| search::canonical_url(&r.url))
        .collect();
    let before = keys.len();
    keys.sort();
    keys.dedup();
    assert_eq!(before, keys.len(), "duplicate pages survived the merge");
}
