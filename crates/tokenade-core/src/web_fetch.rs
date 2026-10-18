//! The ONE HTTP transport the web pipeline uses.
//!
//! Until this module existed there were two: the hook spawned its own curl in
//! `webfetch_pretool/mod.rs`, and everything else (`tokenade web`, the site
//! readers, the YouTube follow-ups) went through `reader_curl_raw`. They were
//! written a year apart and each had what the other lacked — MEASURED, not
//! supposed:
//!
//! |                              | hook  | reader |
//! |------------------------------|-------|--------|
//! | content-type captured        | yes   | no     |
//! | native-UA first, browser retry | yes | no (always browser) |
//! | ephemeral cookie jar         | no    | yes    |
//! | `--max-redirs`               | no    | yes    |
//! | proxy support                | no    | yes    |
//!
//! So the PRIMARY channel — the hook, the one an agent hits without asking —
//! had no proxy support and no redirect bound, and `tokenade web` could not
//! tell HTML from a PDF because it never asked curl for the content type.
//! Neither gap was a decision; both were drift. This module is the union, and
//! there is nowhere else to add a flag.

/// What a fetch actually returned. Every field is provenance the caller needs
/// in order to be honest about what it is serving.
pub struct Fetched {
    pub bytes: Vec<u8>,
    /// Final HTTP status after redirects. 200 when curl emitted no trailer —
    /// "we cannot tell" must not read as a refusal.
    pub status: u16,
    /// `Content-Type` header, raw (`text/html; charset=utf-8`). `None` when the
    /// server sent none or the trailer was unparseable.
    pub content_type: Option<String>,
    /// The URL the fetch ENDED on. `curl -L` follows redirects silently, so
    /// without this the agent cannot tell it asked for one page and got another.
    pub effective_url: Option<String>,
    /// The native UA was refused and the browser profile was not. The agent's
    /// own fetcher would have hit the same wall, so this is a real rescue and
    /// the delivery says so.
    pub native_blocked: bool,
    /// The body hit the drain cap and the rest of the page was never read.
    ///
    /// Carried for the same reason as `effective_url` one field up: without it
    /// the agent cannot tell it asked for a page and got PART of one. Both
    /// drains already computed this — the curl one bound it to `_truncated` and
    /// dropped it, the reqwest one just `break`s — so a 24 MB page came back
    /// looking exactly like a whole one, and a `--grep` for anything past the
    /// cap answered "no match" rather than "never read".
    pub truncated: bool,
}

/// Knobs a caller genuinely varies. Everything else — headers, jar, proxy,
/// redirect bound — is not a per-call decision and is not exposed.
pub struct FetchOpts<'a> {
    /// curl `--max-time`, seconds.
    pub max_time: &'a str,
    /// Hard cap on drained body bytes.
    pub cap: usize,
    /// `Some` switches to a JSON POST (Notion's `loadPageChunk` has no GET).
    pub post_body: Option<&'a str>,
    /// Try the honest `tokenade/x.y` UA first and only fall back to the browser
    /// profile on a refusal. Page fetches want this — it is the polite order and
    /// it is what makes `native_blocked` meaningful. API/reader fetches do not:
    /// they hit endpoints that have no opinion about UAs, and the extra attempt
    /// is pure latency.
    pub polite_first: bool,
}

impl<'a> FetchOpts<'a> {
    /// A page fetch: polite UA first, browser profile on refusal.
    pub fn page(max_time: &'a str, cap: usize) -> Self {
        Self {
            max_time,
            cap,
            post_body: None,
            polite_first: true,
        }
    }
    /// An API/reader fetch: one attempt, browser profile.
    pub fn api(max_time: &'a str, cap: usize) -> Self {
        Self {
            max_time,
            cap,
            post_body: None,
            polite_first: false,
        }
    }
}

use std::sync::Arc;
use std::time::Duration;

mod challenge;
mod errors;
mod identity;
mod transport;
use challenge::*;
pub use errors::*;
pub use identity::*;
pub use transport::*;

/// One reqwest request, driven to completion on the shared runtime by the
/// synchronous wrappers. Reads the metadata natively off the `Response` — no
/// `-w` trailer, no `split_meta_trailer` on this path.
async fn reqwest_request(
    client: &reqwest::Client,
    url: &str,
    ua: &str,
    headers: &[&str],
    post_body: Option<&str>,
    max_time: &str,
    cap: usize,
) -> ReqOutcome {
    let secs = max_time.parse::<u64>().unwrap_or(20);
    let mut rb = match post_body {
        Some(body) => client
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_string()),
        None => client.get(url),
    };
    rb = rb
        .header(reqwest::header::USER_AGENT, ua)
        .timeout(Duration::from_secs(secs));
    for h in headers {
        if let Some((k, v)) = h.split_once(": ") {
            rb = rb.header(k, v);
        }
    }
    let mut resp = match rb.send().await {
        Ok(r) => r,
        // A refused handshake / failed connect is the one thing curl is kept
        // for. Everything else (timeout, protocol error) is a genuine miss.
        Err(e) => {
            let why = FetchError::from_reqwest(&e);
            return if e.is_connect() {
                ReqOutcome::ConnectFailed(why)
            } else {
                ReqOutcome::Empty(why)
            };
        }
    };
    // A non-2xx is NOT an error here: the error page is content the caller must
    // be told about, which is the whole reason `status` rides along.
    let status = resp.status().as_u16();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let final_url = resp.url().as_str().to_string();
    let effective_url = (final_url != url).then_some(final_url);
    // Bounded drain: a chunked response that ignores every size hint must not
    // stream until memory runs out. A read error mid-body keeps what we have —
    // a partial page beats nothing.
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
        return ReqOutcome::Empty(FetchError::Empty(status));
    }
    ReqOutcome::Ok(Fetched {
        bytes,
        status,
        content_type,
        effective_url,
        native_blocked: false,
        truncated,
    })
}

/// The Chrome we present, for the one other module that must agree with it.
///
/// `BROWSER_UA` stays private: it is not a knob, and a second caller free to
/// send its own would be the drift this getter exists to prevent.
pub fn browser_user_agent() -> &'static str {
    BROWSER_UA
}

/// One page-fetch attempt: reqwest first, curl only if rustls refused the
/// handshake. `browser` sends the full Chrome profile; otherwise the honest
/// `tokenade/x` UA and no extra headers.
///
/// The `client` is owned by the caller so the polite→browser retry reuses one
/// connection pool and one cookie jar: UA and headers are per-REQUEST, and the
/// two attempts are one browser navigation, so a second client would only buy a
/// second connect + TLS handshake to the same host.
fn attempt(
    url: &str,
    opts: &FetchOpts,
    browser: bool,
    client: &reqwest::Client,
) -> Result<Fetched, FetchError> {
    let ua = if browser {
        BROWSER_UA.to_string()
    } else {
        format!("tokenade/{} +https://tokenade.dev", crate::VERSION)
    };
    let owned = if browser {
        localized_headers(BROWSER_HEADERS, url)
    } else {
        Vec::new()
    };
    let headers: Vec<&str> = owned.iter().map(String::as_str).collect();
    match crate::runtime::block_on(reqwest_request(
        client,
        url,
        &ua,
        &headers,
        opts.post_body,
        opts.max_time,
        opts.cap,
    )) {
        ReqOutcome::Ok(f) => Ok(f),
        ReqOutcome::Empty(why) => Err(why),
        // curl is kept for exactly this case; if it fails too, the rustls
        // diagnosis is the one worth reporting — curl's own message would name
        // its exit code and hide which host refused what.
        ReqOutcome::ConnectFailed(why) => curl_fallback(url, &ua, opts, browser).ok_or(why),
    }
}

/// Fetch `url`. `None` on transport failure or an empty body — the caller then
/// falls through to whatever it would have done without us. A non-2xx with a
/// body is NOT a failure: the error page is content the caller must be told
/// about, which is why `status` rides along instead of being an `Err`.
pub fn fetch(url: &str, opts: &FetchOpts) -> Option<Fetched> {
    fetch_detailed(url, opts).ok()
}

/// [`fetch`], keeping the reason it failed.
///
/// Same call, same work — the only difference is that the cause survives the
/// return. Callers that put a message in front of a human want this one; the
/// rest keep `fetch` and its `Option`.
pub fn fetch_detailed(url: &str, opts: &FetchOpts) -> Result<Fetched, FetchError> {
    let f = fetch_inner(url, opts);
    // ONE choke point for the TLS-fingerprint escalation: every path above can
    // end on a challenge page, and each would otherwise need its own retry.
    escalate_on_challenge(url, opts, f)
}

fn fetch_inner(url: &str, opts: &FetchOpts) -> Result<Fetched, FetchError> {
    // A fresh, EPHEMERAL cookie jar per fetch chain: this is what a browser does
    // WITHIN one navigation, not a session. Nothing is carried between fetches,
    // so no identity accumulates and no cookie outlives the call that got it.
    let jar = Arc::new(ResettableJar::default());
    let client = build_client(Some(jar.clone()), false, url);
    let first = fetch_chain(url, opts, &jar, &client);

    // ── Cookie warm-up ──────────────────────────────────────────────────────
    // Exactly ONE extra round trip, and only behind a cookie-gated wall (see
    // `cookie_gated_challenge`). The retry calls `attempt` directly rather than
    // re-entering `fetch_inner`, so this cannot recurse or loop however the
    // origin answers.
    let Some(origin) = first
        .as_ref()
        .ok()
        .and_then(|f| warmup_origin(url, &f.bytes))
    else {
        return first;
    };
    crate::debug_log::log(
        "web-fetch",
        "cookie_warmup",
        serde_json::json!({ "url": url, "origin": origin }),
    );
    // The browser profile on BOTH requests, and NO `jar.reset()` between them.
    // The cookies the homepage sets are the whole point of the detour, and an
    // `_abck` issued to `tokenade/x` then replayed under the Chrome profile is
    // precisely the UA/cookie mismatch `ResettableJar` exists to prevent.
    // `post_body` is cleared: the caller's POST belongs to the target URL, and
    // POSTing it at the homepage would be a request nobody asked for.
    let warm = FetchOpts {
        max_time: opts.max_time,
        cap: opts.cap,
        post_body: None,
        polite_first: false,
    };
    if attempt(&origin, &warm, true, &client).is_err() {
        // The origin is unreachable. That says nothing about the target, so the
        // original answer — wall and all — stays exactly as it was.
        return first;
    }
    match attempt(url, opts, true, &client) {
        // Only take the retry if it actually got through. A second wall carries
        // no more information than the first, and `first` at least kept the
        // status and the `native_blocked` verdict the chain worked out.
        Ok(f) if crate::stealth::detect_challenge(&String::from_utf8_lossy(&f.bytes)).is_none() => {
            Ok(f)
        }
        _ => first,
    }
}

/// The polite→browser attempt chain for one URL on one client. Split out of
/// [`fetch_inner`] so the warm-up retry can reuse the client and its jar.
fn fetch_chain(
    url: &str,
    opts: &FetchOpts,
    jar: &ResettableJar,
    client: &reqwest::Client,
) -> Result<Fetched, FetchError> {
    if !opts.polite_first {
        return attempt(url, opts, true, client).map(|mut f| {
            f.native_blocked = false;
            f
        });
    }
    let first = attempt(url, opts, false, client);
    match &first {
        // Answered, and not a wall: keep it. A browser retry here would be a
        // second request for nothing.
        Ok(f) if !refused(f.status) => first,
        other => {
            let first_status = other.as_ref().ok().map(|f| f.status);
            crate::debug_log::log(
                "web-fetch",
                "retry_browser_profile",
                serde_json::json!({ "url": url, "first_status": first_status }),
            );
            // Same connection pool, clean slate: see `ResettableJar`.
            jar.reset();
            match attempt(url, opts, true, client) {
                Ok(mut f) => {
                    f.native_blocked = first_status.is_some_and(refused) && !refused(f.status);
                    Ok(f)
                }
                // The retry failed outright; the first answer, wall or not, is
                // still better than nothing. When there was no first answer
                // either, `first` carries the polite attempt's cause — the one
                // that describes the ordinary path.
                Err(_) => first,
            }
        }
    }
}

/// #291 — is a failed first search attempt worth a second, longer one?
///
/// The retry exists for a FAST failure: a blocked h2 fingerprint fails on the
/// connection, a per-query rate limit answers 429 at once, and both serve fine
/// after a beat. A first attempt that burned its whole budget is neither, and
/// retrying it with a LONGER timeout is what turned one slow engine into
/// 8 + 1.6 + 15 = 24.6 seconds in front of a PreToolUse hook.
///
/// The threshold lives here and nowhere else: it used to decide only the pause
/// while the retry ran unconditionally, so "fast" was asserted in prose and
/// applied to half the branch.
pub(crate) fn retry_is_worth_it(first_attempt: std::time::Duration) -> bool {
    const FAST_FAILURE: std::time::Duration = std::time::Duration::from_secs(3);
    first_attempt < FAST_FAILURE
}

/// Fetch a SEARCH ENGINE results page.
///
/// A separate entry point rather than more flags on [`fetch`], because a SERP
/// fetch differs from a page fetch in ways that are MEASURED, not stylistic,
/// and each difference would be a footgun as a general option:
///
///  * `--fail`. A 429 rate-limit or a 403 anti-bot page must count as an engine
///    FAILURE and trip the breaker. Without it curl exits 0 on a full-size 429
///    page, which then parses to zero hits and is mislabelled "thin" (measured
///    2026-07-10). On an ordinary page fetch the opposite is true: the error
///    page IS the content we must disclose.
///  * A current desktop Chrome UA plus `sec-ch-ua`. Bing cross-checks the two;
///    a stale or mismatched UA downgrades it to a results-less shell (measured:
///    0 `b_algo` blocks).
///  * A per-engine cookie (Yahoo only). Yahoo gates the server-rendered SERP
///    behind a language preference cookie; Bing sends none (a thin static
///    preference cookie measurably biased it toward the entity-collapse page).
///  * Fast-fail → pause → retry. A FAST first failure is a per-query rate limit,
///    not a slow origin: Mojeek 403s back-to-back queries and serves fine after
///    a beat. A slow failure was a timeout and is retried at once on HTTP/1.1.
pub fn fetch_search(url: &str, cookie: Option<&str>) -> Option<String> {
    // Real SERPs run 20-350 KB; past 4 MB it is not a results page.
    const DRAIN_CAP: usize = 4 * 1024 * 1024;
    const SERP_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
(KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
    const SERP_HEADERS: &[&str] = &[
        "Accept: text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
        "Accept-Language: en-US,en;q=0.9",
        "Sec-Fetch-Site: none",
        "Sec-Fetch-Mode: navigate",
        "Sec-Fetch-User: ?1",
        "Sec-Fetch-Dest: document",
        "Upgrade-Insecure-Requests: 1",
        "sec-ch-ua: \"Chromium\";v=\"131\", \"Not_A Brand\";v=\"24\"",
        "sec-ch-ua-mobile: ?0",
        "sec-ch-ua-platform: \"Windows\"",
    ];
    // `--fail` semantics: a 429 rate-limit or a 403 anti-bot page must count as
    // an engine FAILURE and trip the breaker, so a non-2xx yields `None` here.
    // (On an ordinary page fetch the opposite is true — the error page IS the
    // content — which is why that path keeps the body and this one drops it.)
    let attempt = |max_time: &str, http1_only: bool| -> Option<Vec<u8>> {
        // No cookie jar: the SERP profile sends the per-engine `Cookie` header
        // explicitly, and a jar would let the engine pin an identity across
        // queries. A fresh client each attempt keeps it stateless.
        let client = build_client::<ResettableJar>(None, http1_only, url);
        // Localized too: a `google.de` / `qwant.fr` SERP asked in en-US is the
        // same locale/TLD contradiction as on an ordinary page, and the engines
        // are the strictest scorers we talk to.
        let mut headers: Vec<String> = localized_headers(SERP_HEADERS, url);
        if let Some(c) = cookie {
            headers.push(format!("Cookie: {c}"));
        }
        let header_refs: Vec<&str> = headers.iter().map(String::as_str).collect();
        match crate::runtime::block_on(reqwest_request(
            &client,
            url,
            SERP_UA,
            &header_refs,
            None,
            max_time,
            DRAIN_CAP,
        )) {
            // A SERP's non-2xx IS a failure (the `--fail` rule).
            ReqOutcome::Ok(f) if (200..300).contains(&f.status) => Some(f.bytes),
            _ => None,
        }
    };
    // Fast-fail → pause → retry, now also switching h2 → HTTP/1.1 on the retry:
    // some engines block the h2 fingerprint, and a FAST first failure is a
    // per-query rate limit that serves fine after a beat.
    //
    // #291 — the retry fires only on a FAST failure, which is what the sentence
    // above actually describes: a blocked h2 fingerprint fails on the
    // connection, and a per-query rate limit answers 429 at once. A first
    // attempt that burned its whole 8-second budget is neither, and retrying it
    // with a LONGER one turned a slow engine into 8 + 1.6 + 15 = 24.6 seconds
    // in front of a PreToolUse hook, with the agent waiting.
    //
    // Measured on this machine's log: 53 of 54 served searches finish under
    // 2.5 s, and the one that did not took 23 012 ms — the sum above. Of three
    // that ended unusable, one took 18 008 ms. The threshold was already
    // computed here to decide the pause; it now decides the retry too, so
    // "fast" has one definition instead of being asserted in prose and applied
    // to only half the branch.
    let started = std::time::Instant::now();
    let bytes = attempt("8", false).or_else(|| {
        if !retry_is_worth_it(started.elapsed()) {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(1600));
        attempt("15", true)
    })?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

// `normalized_content_type` moved to `text_encoding`: reducing a
// `Content-Type` to its bare type is string reading, and the HTML rendering
// asked for it from another level — it was the only thing the converter
// borrowed from the fetch module.

/// Is this response bytes we must not run a text pipeline over?
pub struct BodyKind {
    /// What the magic bytes say it is (`"pdf"`, `"png"`, `"zip"`, `"bin"`, …).
    pub detected: Option<String>,
    /// Route to the binary/document path rather than to markdown conversion.
    pub binary: bool,
}

/// Decide whether a fetched body is binary, using the server's declared type
/// AND the magic bytes — neither alone is enough:
///
/// * bytes alone: modern HTML (inline SVG, base64 fonts, minified JS) trips a
///   control-character heuristic on the first 4 KB, which used to route real
///   `text/html` responses (arxiv abstracts) into a binary stash;
/// * content-type alone: servers mislabel, and a `.docx` behind
///   `application/octet-stream` is still a document we can extract.
///
/// So: an extractable document wins outright, a text content-type overrides
/// the byte heuristic, and otherwise either signal may condemn the body.
///
/// Shared by the hook and `tokenade web`. It was the hook's alone, which is
/// exactly why `tokenade web` had the arxiv bug and the hook did not.
pub fn classify_body(bytes: &[u8], content_type: Option<&str>) -> BodyKind {
    let ct = crate::text_encoding::normalized_content_type(content_type);
    let ct_is_text = ct.as_deref().is_some_and(|c| {
        c.starts_with("text/")
            || c == "application/xhtml+xml"
            || c == "application/json"
            || c == "application/xml"
            || c == "application/javascript"
    });
    let ct_says_binary = ct.as_deref().is_some_and(|c| {
        c.starts_with("image/")
            || c.starts_with("audio/")
            || c.starts_with("video/")
            || c.starts_with("font/")
            || c == "application/pdf"
            || c == "application/octet-stream"
            || c == "application/zip"
            || c == "application/gzip"
            || c == "application/x-tar"
    });
    let detected = crate::disclose::detect_binary(bytes);
    // An extractable document is worth extracting no matter what the server
    // called it — this is the branch that saves a mislabelled PDF.
    let extractable_doc = detected
        .as_deref()
        .and_then(crate::docs::Format::from_extension)
        .is_some();
    let binary = if extractable_doc {
        true
    } else if ct_is_text && !ct_says_binary {
        false
    } else {
        detected.is_some() || ct_says_binary
    };
    BodyKind { detected, binary }
}

#[cfg(test)]
include!("web_fetch_tests.rs");
