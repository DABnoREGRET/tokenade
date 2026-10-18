//! **The site refuses us: step up a notch, or SAY so.**
//!
//! A 429, a Cloudflare challenge, a cookie wall. Each step up (browser
//! headers, warming up the origin, impersonation) costs time and looks a bit
//! more like what we are not — so each one is gated on a reason READ in the
//! response, not on just any failure. And when none gets through, the refusal
//! is carried all the way to the banner: a silent fallback makes OUR
//! isolation take the blame for the site's refusal.

use super::*;

/// A Cloudflare challenge that our headers cannot explain is a TLS-stack
/// signal, not a header one: MEASURED 2026-07-22, replaying `BROWSER_HEADERS`
/// byte-for-byte through curl returns 200 where reqwest+rustls gets the
/// interstitial, on the same connection and UA (npmjs, stackoverflow, quora,
/// udemy — 4 of a 14-domain sweep). rustls' ClientHello is fingerprinted; curl's
/// is not. So re-ask through curl before handing the agent a challenge page.
///
/// curl is the FIRST of two rungs here, and the cheap one: we already ship it,
/// and it claims nothing — its JA4 is its own. The second is
/// [`crate::web_impersonate`], which does claim to be Chrome, at the layer where
/// the claim is checked. It is tried only after curl has failed to clear the
/// wall, and only in a build that carries it, so the ordinary page costs exactly
/// what it did before.
///
/// A site that challenges ALL THREE is left alone — the challenge page is then
/// the honest answer.
pub(super) fn escalate_on_challenge(
    url: &str,
    opts: &FetchOpts,
    f: Result<Fetched, FetchError>,
) -> Result<Fetched, FetchError> {
    let blocked = f.as_ref().is_ok_and(|f| {
        crate::stealth::detect_challenge(&String::from_utf8_lossy(&f.bytes)).is_some()
    });
    if !blocked {
        return f;
    }
    // The curl rung is CONDITIONAL on curl being installed; the rung after it is
    // not. Nesting the second inside the first — which is what a single early
    // return did — made a machine without curl skip both, silently, on exactly
    // the pages the ladder exists for.
    if !curl_available() {
        return impersonate_on_challenge(url, opts, f);
    }
    crate::debug_log::log(
        "web-fetch",
        "curl_challenge_escalation",
        serde_json::json!({ "url": url }),
    );
    // Only accept curl's answer if it is NOT itself a challenge: a second wall
    // is no better than the first, and the original at least carries its status.
    match run_curl(url, BROWSER_UA, opts, true) {
        Some(c)
            if crate::stealth::detect_challenge(&String::from_utf8_lossy(&c.bytes)).is_none() =>
        {
            Ok(Fetched {
                native_blocked: true,
                ..c
            })
        }
        _ => impersonate_on_challenge(url, opts, f),
    }
}

/// The last rung: re-ask with a browser's TLS fingerprint.
///
/// Same acceptance test as the curl rung above — a second wall is no better
/// than the first — and the same fall-through: whatever we already had is
/// returned untouched when this build has no impersonating stack, when the
/// request fails, or when the answer is another challenge. That is what makes
/// the rung unable to LOSE a page: it is only ever consulted about a response
/// that is already a challenge, and its answer is only ever kept when it is not.
pub(super) fn impersonate_on_challenge(
    url: &str,
    opts: &FetchOpts,
    f: Result<Fetched, FetchError>,
) -> Result<Fetched, FetchError> {
    if !crate::web_impersonate::available() {
        return f;
    }
    crate::debug_log::log(
        "web-fetch",
        "impersonate_challenge_escalation",
        serde_json::json!({ "url": url }),
    );
    let secs = opts.max_time.parse::<u64>().unwrap_or(20);
    match crate::web_impersonate::fetch(url, secs, opts.cap) {
        Some(c)
            if crate::stealth::detect_challenge(&String::from_utf8_lossy(&c.bytes)).is_none() =>
        {
            Ok(c)
        }
        _ => f,
    }
}

/// Challenge kinds a homepage GET can plausibly clear, and only those.
///
/// The vendors listed here identify a first-time visitor by the ABSENCE of a
/// cookie their edge sets in a plain `Set-Cookie` header on any ordinary
/// response — `_abck` / `bm_sz` (Akamai), `TS*` + `BIGipServer*` (F5),
/// `visid_incap_*` (Imperva), `rbzid` (Reblaze), `sucuri_cloudproxy_uuid`
/// (Sucuri), plus Datadome and PerimeterX. A browser never meets that wall on
/// an article because it loaded the homepage first and is already carrying the
/// cookie; a deep-link fetch arrives bare. Fetching the origin and retrying is
/// that same sequence, not an evasion: the cookie is handed to us unprompted.
///
/// Everything else is excluded on purpose. Cloudflare is OUT — its clearance is
/// gated on a JS challenge, which we investigated and closed with a negative
/// verdict; a homepage GET cannot produce `cf_clearance` and pretending
/// otherwise would only cost a second request. The CAPTCHA widgets
/// (hCaptcha/reCAPTCHA), Kasada and AWS WAF need a solved token, not a cookie.
/// `GenericBlock`, `ClientChallengeShell` and `HumanCheck` name no vendor, so
/// there is no evidence a cookie is what they are missing — warming up on them
/// would be a guess billed as a request on every hollow page we fetch.
pub(super) fn cookie_gated_challenge(kind: crate::stealth::ChallengeKind) -> bool {
    use crate::stealth::ChallengeKind as K;
    matches!(
        kind,
        K::Akamai | K::F5BigIp | K::Imperva | K::Datadome | K::PerimeterX | K::Reblaze | K::Sucuri
    )
}

/// `scheme://authority/` of `url`, plus whether `url` ALREADY *is* that origin.
///
/// The port is kept (an origin is scheme+host+PORT; dropping it would warm up a
/// different server), the userinfo is not: a warm-up is a fresh navigation, and
/// replaying credentials the caller happened to put in the URL onto an extra
/// request is not something the caller asked for.
pub(super) fn origin_of(url: &str) -> Option<(String, bool)> {
    let (scheme, rest) = url.split_once("://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let authority = authority
        .rsplit_once('@')
        .map(|(_, h)| h)
        .unwrap_or(authority);
    if authority.is_empty() {
        return None;
    }
    let origin = format!(
        "{}://{}/",
        scheme.to_ascii_lowercase(),
        authority.to_ascii_lowercase()
    );
    // `https://host` and `https://host/` are the origin itself. A query or a
    // fragment is not: `/?q=1` is a distinct response the homepage GET would
    // not be fetching twice for nothing.
    Some((origin, tail.is_empty() || tail == "/"))
}

/// The origin to warm up before retrying `url`, or `None` to leave the response
/// alone. Pure, so the decision is testable without a socket.
pub(super) fn warmup_origin(url: &str, body: &[u8]) -> Option<String> {
    let kind = crate::stealth::detect_challenge(&String::from_utf8_lossy(body))?;
    if !cookie_gated_challenge(kind) {
        return None;
    }
    let (origin, is_origin) = origin_of(url)?;
    // Already the homepage: the warm-up would re-fetch the very page that just
    // challenged us, paying a request to learn nothing.
    if is_origin {
        return None;
    }
    // The same SSRF rule the redirect policy enforces. The origin shares the
    // original URL's host so an entry guard upstream has already cleared it —
    // but the warm-up is a new outbound request, and a guard that holds only
    // because of where it is called from is one refactor away from not holding.
    if crate::url::scheme_host(&origin).is_some_and(|(_, h)| crate::url::is_internal_host(&h)) {
        return None;
    }
    Some(origin)
}
