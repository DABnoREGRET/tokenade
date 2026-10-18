//! Risk #2 — silent proxy loss. `net::resolve_proxy` is consumed by BOTH
//! transport families, and any migration must keep routing through it or every
//! corporate-proxy user loses the web with no error.
//!
//! This lives in its OWN test binary because it mutates `HTTPS_PROXY`
//! process-wide; sharing a process with the reach battery would poison its
//! parallel fetches. Opt-in and never in CI (it makes real requests).
//!
//!     TOKENADE_LIVE_WEB_TESTS=1 cargo test -p tokenade-core --test web_proxy -- --nocapture

use tokenade_core::web_fetch::{fetch, FetchOpts};

fn enabled() -> bool {
    matches!(
        std::env::var("TOKENADE_LIVE_WEB_TESTS").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

/// A set proxy MUST be routed through. With `HTTPS_PROXY` pointing at a dead
/// port, a fetch that succeeds directly must now fail — proof the request went
/// THROUGH the proxy (reqwest's `Proxy::all` from `resolve_proxy`) rather than
/// connecting straight out. curl, the handshake fallback, resolves the same
/// proxy, so it cannot rescue the request either.
#[test]
fn a_set_proxy_is_actually_routed_through_the_client() {
    if !enabled() {
        eprintln!("skipped — set TOKENADE_LIVE_WEB_TESTS=1 to run");
        return;
    }
    // Baseline: the direct fetch works, so a later failure means "routed", not
    // "the site was down".
    let direct = fetch("https://example.com/", &FetchOpts::page("15", 1 << 20));
    assert!(
        direct.is_some(),
        "example.com must be reachable directly for this test to mean anything"
    );

    // Force a dead proxy (port 1: nothing listens) and assert the fetch fails.
    std::env::set_var("HTTPS_PROXY", "http://127.0.0.1:1");
    let via_dead_proxy = fetch("https://example.com/", &FetchOpts::page("8", 1 << 20));
    std::env::remove_var("HTTPS_PROXY");

    assert!(
        via_dead_proxy.is_none(),
        "with a dead proxy set, the fetch MUST fail — a success proves the proxy \
         was bypassed and the request went direct, which is the exact silent \
         proxy-loss regression this guards"
    );
}
