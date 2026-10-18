//! Stage 5 real-data check: the shared reqwest INFRA client (the one licence,
//! updater, model-fetch, judge and the agents now use) actually fetches real
//! endpoints — JSON GET, a bounded GET, and a streamed download. Opt-in and
//! never in CI (it hits the network):
//!
//!     TOKENADE_LIVE_WEB_TESTS=1 cargo test -p tokenade-core --test net_infra_live -- --nocapture

use tokenade_core::net;

fn enabled() -> bool {
    matches!(
        std::env::var("TOKENADE_LIVE_WEB_TESTS").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

#[test]
fn the_infra_client_fetches_real_json() {
    if !enabled() {
        eprintln!("skipped — set TOKENADE_LIVE_WEB_TESTS=1 to run");
        return;
    }
    // The shape the licence/updater/model-fetch calls rely on: a real GET that
    // returns a 2xx and parseable JSON.
    let resp = net::http_get(
        "https://api.github.com/repos/rust-lang/rust",
        &[("User-Agent", "tokenade-test")],
        20,
    )
    .expect("infra GET must reach api.github.com");
    assert!(resp.ok(), "expected 2xx, got {}", resp.status);
    let v: serde_json::Value = resp.json().expect("body must be JSON");
    assert_eq!(v.get("name").and_then(|n| n.as_str()), Some("rust"));

    // The capped variant (updater's manifest/binary path) must stop at the cap.
    let capped = net::http_get_capped("https://example.com/", &[], 15, 512)
        .expect("capped GET must reach example.com");
    assert!(capped.ok());
    assert!(
        capped.body.len() <= 512,
        "cap must bound the body: {}",
        capped.body.len()
    );

    // The streaming variant (large downloads) writes into a sink.
    let mut sink: Vec<u8> = Vec::new();
    let status = net::http_get_stream("https://example.com/", &[], 15, &mut sink)
        .expect("streamed GET must reach example.com");
    assert!((200..300).contains(&status));
    assert!(
        sink.windows(7).any(|w| w == b"Example"),
        "streamed body must carry the page"
    );
}
