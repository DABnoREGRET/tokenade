//! **Why it did not work**, and whether retrying is worth it.
//!
//! A network error is not a site error, and a 429 is not a 404. The caller at
//! the top of the stack must be able to SAY which: a silent fallback to
//! `None` throws the cause away, and the banner ends up blaming our isolation
//! for the site's refusal.
//!
//! The resettable cookie jar is here for the same reason: a session poisoned
//! by a challenge is recognised by the fact that it always fails, and the
//! only way out is to throw it away.

use super::*;
use std::sync::Arc;
use std::time::Duration;

/// A reqwest attempt's three outcomes. `ConnectFailed` is the ONLY one that
/// falls through to curl: a refused handshake is precisely what curl is for. A
/// timeout or an empty body is not a handshake problem and does not.
pub(crate) enum ReqOutcome {
    Ok(Fetched),
    Empty(FetchError),
    ConnectFailed(FetchError),
}

/// Why a fetch produced nothing.
///
/// Every layer below used to return `Option`, so the reason a page did not
/// arrive was discarded one frame after it was known and the user got the same
/// sentence for a DNS failure, a timeout and a 200 with an empty body. The
/// transport already distinguishes them; this carries the distinction out.
///
/// The payload is REDACTED at construction: a transport error prints the URL it
/// failed on, and URLs carry tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// The connection never established — DNS, refused, TLS handshake.
    Connect(String),
    /// The request was still in flight when the deadline passed.
    Timeout(String),
    /// A response arrived and carried no body. Not a transport failure.
    ///
    /// **The STATUS travels with it.** Three lines above, this module writes
    /// "a non-2xx is NOT an error here: the error page is content the caller
    /// must be told about, which is the whole reason `status` rides along" —
    /// and an EMPTY body threw that status away. MEASURED 2026-09-11: reddit
    /// answers **429 with zero bytes**, the reader came back with "could not
    /// be fetched at all" and the agent read "this page has no text" where
    /// the answer was "retry in a minute". `0` = no known status (the
    /// connection did not complete).
    Empty(u16),
    /// Anything else the transport reported.
    Transport(String),
}

impl FetchError {
    /// The HTTP status this failure knows, when it knows one.
    ///
    /// Only an empty body carries one: a refused connection or a timeout
    /// never saw a response.
    pub fn status(&self) -> Option<u16> {
        match self {
            FetchError::Empty(0) => None,
            FetchError::Empty(code) => Some(*code),
            _ => None,
        }
    }
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // English, like every other line this CLI prints. Both callers
            // interpolate it into an English sentence ("tokenade web: {url}:
            // {why}. Set TOKENADE_DEBUG_LOG=…"), so the French read as a
            // fragment of another program's output. The French docs quote the
            // old wording as measured history; nothing asserts it.
            FetchError::Connect(m) => write!(f, "could not connect ({m})"),
            FetchError::Timeout(m) => write!(f, "timed out ({m})"),
            FetchError::Empty(0) => write!(f, "empty response"),
            FetchError::Empty(code) => write!(f, "empty response (HTTP {code})"),
            FetchError::Transport(m) => write!(f, "{m}"),
        }
    }
}

impl FetchError {
    /// Classify a `reqwest` error, keeping only what a user can act on.
    ///
    /// The message comes from the DEEPEST `source()`, not from the error
    /// itself. MEASURED 2026-08-08: `reqwest::Error::to_string()` is the same
    /// wrapper sentence — "error sending request for url (…)" — for a refused
    /// port, an unresolvable host and a blackholed address, so classifying
    /// correctly and then printing the top-level text still hands the user
    /// three identical lines. The chain underneath says "tcp connect error:
    /// Connection refused" and "dns error: …", which is the part worth reading.
    pub(crate) fn from_reqwest(e: &reqwest::Error) -> Self {
        let mut deepest: &(dyn std::error::Error + 'static) = e;
        while let Some(src) = deepest.source() {
            deepest = src;
        }
        let msg = crate::redact::redact(&deepest.to_string());
        // Timeout is tested FIRST, and the order is measured, not stylistic: a
        // deadline that expires during the handshake makes `is_connect()` and
        // `is_timeout()` BOTH true, and connect-first printed "connexion
        // impossible (deadline has elapsed)" for a blackholed address — the
        // phase instead of the fact. Timeout-first says "raise the timeout",
        // which is the thing the user can do.
        if e.is_timeout() {
            FetchError::Timeout(msg)
        } else if e.is_connect() {
            FetchError::Connect(msg)
        } else {
            FetchError::Transport(msg)
        }
    }
}

// Build a reqwest client. Ship STOCK h2 settings (D3) — do not tune toward
// Chrome; a half-Chrome fingerprint is more conspicuous than a consistent
// non-browser one. `http1_only` forces HTTP/1.1 for the SERP engines whose h2
// fingerprint gets them blocked (SearXNG disables h2 on four engines for
// exactly this reason).

/// A cookie jar whose contents can be dropped without dropping the CLIENT.
///
/// The polite→browser retry reuses one `Client` so the second attempt lands on
/// the connection the first one already opened (F3 — no second TLS handshake).
/// The cookies must NOT come along: `__cf_bm` and its Akamai/Datadome
/// equivalents are bound to the User-Agent and header fingerprint that obtained
/// them, so replaying a token issued to `tokenade/x` while presenting the full
/// Chrome profile is a mismatch the edge reads as evasion — turning a retry that
/// used to succeed into a hard block, reported to the caller as "this site
/// blocks tokenade". `reset()` between attempts keeps the pool and drops the
/// identity, which is what a fresh jar per attempt used to do.
#[derive(Default)]
pub(crate) struct ResettableJar {
    inner: std::sync::RwLock<Arc<reqwest::cookie::Jar>>,
}

impl ResettableJar {
    pub(crate) fn reset(&self) {
        if let Ok(mut g) = self.inner.write() {
            *g = Arc::new(reqwest::cookie::Jar::default());
        }
    }

    fn current(&self) -> Option<Arc<reqwest::cookie::Jar>> {
        self.inner.read().ok().map(|g| g.clone())
    }
}

impl reqwest::cookie::CookieStore for ResettableJar {
    fn set_cookies(
        &self,
        headers: &mut dyn Iterator<Item = &reqwest::header::HeaderValue>,
        url: &reqwest::Url,
    ) {
        if let Some(jar) = self.current() {
            jar.set_cookies(headers, url);
        }
    }

    fn cookies(&self, url: &reqwest::Url) -> Option<reqwest::header::HeaderValue> {
        self.current().and_then(|jar| jar.cookies(url))
    }
}

pub(crate) fn build_client<C: reqwest::cookie::CookieStore + 'static>(
    cookies: Option<Arc<C>>,
    http1_only: bool,
    url: &str,
) -> reqwest::Client {
    let mut b = reqwest::Client::builder()
        // Same 10-hop bound as the old curl path (a site that loops must not send
        // us round fifty times before we learn anything — tensorflow.org loops
        // without a cookie jar), PLUS an SSRF guard: never FOLLOW a redirect into
        // the user's own network. A public URL that 30x-es to 169.254.169.254 /
        // localhost / a LAN IP must not turn a web fetch into a reach into the
        // user's infrastructure.
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
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
        }))
        .connect_timeout(Duration::from_secs(10))
        .gzip(true)
        .brotli(true)
        .deflate(true);
    if http1_only {
        b = b.http1_only();
    }
    if let Some(jar) = cookies {
        b = b.cookie_provider(jar);
    }
    // The SAME proxy resolution both transport families share — read once here.
    if let Some(p) = crate::net::resolve_proxy(url) {
        if let Ok(px) = reqwest::Proxy::all(&p) {
            b = b.proxy(px);
        }
    }
    b.build().unwrap_or_else(|_| reqwest::Client::new())
}
