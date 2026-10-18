//! Frozen SERP corpus — one real results page per engine.
//!
//! These exist because a search extractor's truth lives OUTSIDE the repo and
//! changes without warning. Unit tests on hand-written fixtures pass forever
//! while an engine quietly reshuffles its markup; the failure mode is not a
//! crash but a silent downgrade to whatever links are lying around. That is
//! exactly what shipped: yahoo and mojeek were returning their own navigation
//! bars as search results, and every test was green.
//!
//! Refresh with the `save` helper documented in `experiments/cf-clearance/`
//! whenever an engine is re-verified. A page that stops parsing is a REAL
//! signal, so a failure here means "go look", not "fix the test".

use tokenade_core::search::Engine;
use tokenade_core::serp;

const QUERY: &str = "python asyncio vs threading";

fn fixture(name: &str) -> String {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/serp")
        .join(format!("{name}.html"));
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
}

/// Minimum organic results each engine must yield from its frozen page.
/// Deliberately below what they currently return — this guards against a
/// COLLAPSE (a layout change taking us to zero or to nav links), not against
/// normal variation.
const FLOOR: &[(&str, Engine, usize)] = &[
    ("ecosia", Engine::Ecosia, 5),
    ("bing", Engine::Bing, 5),
    ("yahoo", Engine::Yahoo, 5),
    ("mojeek", Engine::Mojeek, 5),
    ("marginalia", Engine::Marginalia, 10),
];

#[test]
fn every_engine_still_parses_its_frozen_page() {
    for (name, engine, floor) in FLOOR {
        let hits = engine.parse(&fixture(name));
        assert!(
            hits.len() >= *floor,
            "{name}: {} results, expected >= {floor}. The engine's markup probably \
             changed — go look at the page, do not lower the floor.",
            hits.len()
        );
    }
}

/// The defect that shipped: a portal's navigation bar counted as results.
#[test]
fn no_engine_returns_its_own_navigation() {
    for (name, engine, _) in FLOOR {
        let hits = engine.parse(&fixture(name));
        for h in &hits {
            assert!(
                h.url.starts_with("http"),
                "{name}: relative url {:?} is site chrome, not a result",
                h.url
            );
            for own in [
                "ecosia.org",
                "bing.com",
                "yahoo.com",
                "mojeek.com",
                "marginalia.nu",
            ] {
                assert!(
                    !h.url.contains(own),
                    "{name}: returned a link to a search engine ({own}): {:?}",
                    h.url
                );
            }
        }
        // Nav bars are single words like "Images" / "Settings" / "Weather".
        let single_word = hits.iter().filter(|h| !h.title.contains(' ')).count();
        assert!(
            single_word * 2 <= hits.len(),
            "{name}: {single_word}/{} results have one-word titles — that is a nav bar",
            hits.len()
        );
    }
}

#[test]
fn every_engine_answers_the_query_it_was_asked() {
    for (name, engine, _) in FLOOR {
        let hits = engine.parse(&fixture(name));
        assert!(
            serp::plausible_for_query(&hits, QUERY),
            "{name}: nothing in {} results relates to {QUERY:?}: {:?}",
            hits.len(),
            hits.iter().take(3).map(|h| &h.title).collect::<Vec<_>>()
        );
    }
}

#[test]
fn results_carry_something_worth_reading() {
    for (name, engine, _) in FLOOR {
        let hits = engine.parse(&fixture(name));
        let with_snippet = hits.iter().filter(|h| h.snippet.len() > 20).count();
        assert!(
            with_snippet * 2 >= hits.len(),
            "{name}: only {with_snippet}/{} results have a snippet",
            hits.len()
        );
    }
}

/// HTTP 200, the query echoed in the title, a CAPTCHA where the results should
/// be. Nothing upstream can tell — this fixture is the only guard.
#[test]
fn the_real_bing_challenge_page_is_recognised() {
    let html = fixture("bing_challenge");
    let hits = Engine::Bing.parse(&html);
    // The captured page carries exactly ONE distinct marker (`captcha`, six
    // times) and parses to nothing. That PAIR is what condemns it — one marker
    // alone must not, or a genuine results page about captchas would be
    // condemned too.
    assert!(
        serp::challenge_markers(&html) >= 1,
        "the captured CAPTCHA must carry at least one marker"
    );
    assert!(
        hits.is_empty(),
        "a challenge page has no result blocks, got {}",
        hits.len()
    );
    assert!(
        !serp::plausible_for_query(&hits, "Klimagerät Geräuschpegel dB Schlafzimmer"),
        "a challenge page must never pass the relevance gate"
    );
}

/// A genuine results page ABOUT captchas must survive: one marker plus real
/// results is not a challenge, and treating it as one takes a healthy engine
/// out of rotation.
#[test]
fn a_results_page_about_captchas_is_not_a_challenge() {
    let html = format!(
        "<html><body>{}</body></html>",
        (0..6)
            .map(|i| format!(
                "<li class=\"b_algo\"><h2><a href=\"https://ex{i}.test/a\">\
                 How to solve a captcha programmatically</a></h2>\
                 <p>A long discussion of captcha systems and their weaknesses.</p></li>"
            ))
            .collect::<String>()
    );
    assert!(
        !serp::looks_like_challenge(&html),
        "one marker plus real results is a results page, not a wall"
    );
    assert!(!Engine::Bing.parse(&html).is_empty());
}

/// A genuine SERP must NOT trip the challenge detector — the cost of a false
/// positive is an engine wrongly taken out of rotation.
#[test]
fn genuine_pages_are_not_mistaken_for_challenges() {
    for (name, _, _) in FLOOR {
        assert!(
            !serp::looks_like_challenge(&fixture(name)),
            "{name}: a real results page was classified as a bot challenge"
        );
    }
}

/// A page read from disk must get the same extractor the live path uses.
///
/// Before `engine_of_page` existed, the saved-page command fell back to the
/// engine-agnostic anchor walk — the weak parse that returns navigation.
/// MEASURED on these very fixtures: marginalia came back as "Marginalia /
/// About" (2 hits) and mojeek as a user survey (1 hit), from bytes that hold
/// 25 and 10 real results.
#[test]
fn a_saved_page_is_parsed_by_the_engine_that_made_it() {
    for (name, engine, floor) in FLOOR {
        let html = fixture(name);
        assert_eq!(
            tokenade_core::search::engine_of_page(&html),
            Some(*engine),
            "{name}: not recognised as its own engine's page"
        );
        let (hits, detected) = tokenade_core::search::extract_saved_page(&html);
        assert_eq!(detected, Some(*engine), "{name}");
        assert!(
            hits.len() >= *floor,
            "{name}: saved-page extraction got {} results, expected >= {floor}",
            hits.len()
        );
        assert!(
            serp::plausible_for_query(&hits, QUERY),
            "{name}: saved-page extraction is off-topic"
        );
    }
}

/// An unknown engine's page must still yield something rather than nothing.
#[test]
fn an_unknown_results_page_falls_back_to_the_generic_walk() {
    let html = "<html><body>\
        <h2><a href=\"https://a.test/one\">A perfectly good result title here</a></h2>\
        <p>Some description of the first result.</p>\
        <h2><a href=\"https://b.test/two\">Another result title worth reading</a></h2>\
        <p>Some description of the second result.</p></body></html>";
    assert_eq!(tokenade_core::search::engine_of_page(html), None);
    let (hits, detected) = tokenade_core::search::extract_saved_page(html);
    assert!(detected.is_none());
    assert!(!hits.is_empty(), "the generic walk must still find these");
}
