//! A TRUE Chrome fingerprint, for the pages a Chrome *header* cannot open.
//!
//! `web_fetch` ships a Chrome User-Agent over rustls, and that pair is a
//! contradiction the edge reads before a single byte of HTTP is exchanged.
//! MEASURED 2026-09-10 against `tls.peet.ws/api/clean`, our own ClientHello:
//!
//! ```text
//! ja3    771,4866-4865-4867-49196-…-255,51-23-5-0-10-11-16-13-45-43-35,29-23-24,
//! akamai 2:0;4:2097152;5:16384;6:16384|5177345|0|m,s,a,p
//! ```
//!
//! No GREASE in the cipher list, a fixed extension order, and an HTTP/2
//! SETTINGS frame no Chrome has ever sent. Chrome's own, from the same probe
//! through this module, is `1:65536;2:0;4:6291456;6:262144|15663105|0|m,a,s,p`
//! with GREASE in both the cipher list and the extension list. A fingerprinting
//! edge compares those to the UA and gets its answer before the request line.
//!
//! This is the LAST rung of the escalation ladder in [`crate::web_fetch`], not a
//! replacement for it. Bare Chrome emulation is not uniformly better than the
//! ladder: asked directly, it LOSES `quora` and `elpais`, which the ladder's
//! cookie warm-up and browser retry win. So it is reached only when the answer
//! would otherwise BE a challenge page — it can add pages, and by construction
//! cannot take one away.
//!
//! **And on the pages measured so far it adds none.** A/B on 17 domains
//! (2026-09-10, same binary with and against the feature, verdict read off
//! `stealth::detect_challenge` rather than off a grep): the same verdict on all
//! seventeen, byte-identical on sixteen — the seventeenth is `zillow`'s
//! homepage, which never was a challenge and differs from one run to the next. The rung FIRES — `impersonate_challenge_escalation` appears in the
//! debug log once per curl escalation — and the answer it brings back is another
//! challenge. What still refuses is Cloudflare's JS interstitial (stackoverflow,
//! nih.gov, crunchbase), DataDome (g2, idealista) and AWS WAF (similarweb), and
//! not one of those is decided by the ClientHello: clearing them needs a token
//! produced by executing the vendor's VM, which is challenge SOLVING and a
//! different question from looking like a browser.
//!
//! That measurement is why the feature stays off, and why it is written here
//! rather than in a commit message: the next person to reach for TLS
//! impersonation should start from the ceiling it actually has.
//!
//! Off by default. The impersonating stack is BoringSSL, which needs a C++
//! toolchain and cmake at build time — the very thing `web_fetch`'s transport
//! note says the reqwest path was chosen to avoid. Enabling it is a
//! release-pipeline decision (eight targets, musl and windows-gnu included),
//! not a source-level one, so it lives behind the `impersonate` feature and
//! [`available`] answers honestly when it is absent.

use crate::web_fetch::Fetched;

/// Whether this build can present a browser's TLS fingerprint.
///
/// A plain `cfg!` at every call site would read as a compile-time constant and
/// invite `if false` dead-code pruning to hide the whole rung from review. One
/// named function keeps the question askable — and testable — either way.
pub fn available() -> bool {
    cfg!(feature = "impersonate")
}

/// Which Chrome we claim to be, in ONE place.
///
/// `web_fetch::BROWSER_UA` names a Chrome major and `Sec-CH-UA` repeats it;
/// that file's own note says the two "move together or not at all". This is the
/// third member of that set: emulating Chrome 137's ClientHello while the UA
/// says 126 is the same contradiction one layer down, and a cheaper tell than
/// either being stale.
#[cfg(feature = "impersonate")]
const EMULATION: wreq_util::Emulation = wreq_util::Emulation::Chrome137;

/// The Chrome major [`EMULATION`] presents. Kept next to it so a version bump
/// is one edit, and asserted against `web_fetch`'s UA in the tests below.
pub const EMULATED_CHROME_MAJOR: u32 = 137;

/// Fetch `url` with a browser's TLS and HTTP/2 fingerprint.
///
/// `None` when this build carries no impersonating stack, when the transport
/// fails, or when the body is empty — every one of which leaves the caller
/// holding the answer it already had.
#[cfg(not(feature = "impersonate"))]
pub fn fetch(_url: &str, _max_time_secs: u64, _cap: usize) -> Option<Fetched> {
    None
}

/// See the `cfg(not(...))` twin above.
#[cfg(feature = "impersonate")]
pub fn fetch(url: &str, max_time_secs: u64, cap: usize) -> Option<Fetched> {
    crate::runtime::block_on(imp_request(url, max_time_secs, cap))
}

#[cfg(feature = "impersonate")]
async fn imp_request(url: &str, max_time_secs: u64, cap: usize) -> Option<Fetched> {
    let mut b = wreq::Client::builder()
        .emulation(EMULATION)
        .cookie_store(true)
        .connect_timeout(std::time::Duration::from_secs(10))
        // The SAME bound and the SAME SSRF guard as the reqwest client. A rung
        // added to the ladder must not be the one rung that will follow a 30x
        // into the user's own network.
        .redirect(wreq::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 10 {
                attempt.error("too many redirects")
            } else if attempt
                .url()
                .host_str()
                .is_some_and(crate::url::is_internal_host)
            {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }));
    if let Some(p) = crate::net::resolve_proxy(url) {
        if let Ok(px) = wreq::Proxy::all(&p) {
            b = b.proxy(px);
        }
    }
    let client = b.build().ok()?;
    // No User-Agent and no Sec-CH-UA of ours: the emulation ships Chrome's own
    // header set, in Chrome's own order. Ours would overwrite one of them and
    // leave the rest disagreeing, which is the defect this module exists to
    // remove.
    let mut resp = client
        .get(url)
        .timeout(std::time::Duration::from_secs(max_time_secs))
        .send()
        .await
        .ok()?;
    let status = resp.status().as_u16();
    let content_type = resp
        .headers()
        .get(wreq::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let final_url = resp.url().as_str().to_string();
    let effective_url = (final_url != url).then_some(final_url);
    let mut bytes = Vec::new();
    let mut truncated = false;
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                bytes.extend_from_slice(&chunk);
                if bytes.len() >= cap {
                    truncated = true;
                    break;
                }
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }
    if bytes.is_empty() {
        return None;
    }
    Some(Fetched {
        bytes,
        status,
        content_type,
        effective_url,
        // The ordinary path was refused and this one was not: the agent's own
        // fetcher would have hit the same wall, so the delivery says so.
        native_blocked: true,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three places that name a Chrome version must name the SAME one.
    #[test]
    fn the_emulated_chrome_matches_the_user_agent_we_send() {
        let ua = crate::web_fetch::browser_user_agent();
        let major = format!("Chrome/{EMULATED_CHROME_MAJOR}.");
        assert!(
            ua.contains(&major),
            "the emulation presents Chrome {EMULATED_CHROME_MAJOR} \
             and the UA says {ua:?} — one layer contradicts the other"
        );
        let ch_ua = crate::web_fetch::BROWSER_HEADERS
            .iter()
            .find(|h| h.starts_with("Sec-CH-UA:"))
            .copied()
            .unwrap_or_default();
        assert!(
            ch_ua.contains(&format!("v=\"{EMULATED_CHROME_MAJOR}\"")),
            "Sec-CH-UA is {ch_ua:?}, which does not name Chrome {EMULATED_CHROME_MAJOR}"
        );
    }

    /// Without the feature the rung is absent, and says so rather than
    /// pretending to have tried.
    #[test]
    fn a_build_without_the_stack_returns_nothing_and_admits_it() {
        if !available() {
            assert!(fetch("https://example.com/", 5, 1024).is_none());
        }
    }
}
