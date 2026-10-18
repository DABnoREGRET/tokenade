//! Anti-bot challenge detection + stealth-browser remediation.
//!
//! Some pages are useless to compress because the agent didn't actually
//! receive the content it asked for — it received a Cloudflare Turnstile
//! challenge, an hCaptcha widget, a Datadome interstitial, or an Akamai
//! "Pardon the interruption" wall. Without recognising this an agent will
//! happily summarise the challenge page as if it were the article.
//!
//! [`detect_challenge`] inspects an HTML page or a Playwright accessibility
//! snapshot and returns the [`ChallengeKind`] it recognises (if any).
//!
//! [`stealth_remediation`] returns a concise, agent-actionable 3-tier
//! fallback strategy:
//!
//!   1. **Patchright** with `channel="chrome"` — uses the real Google Chrome
//!      binary so the TLS/JA3 fingerprint matches a real user. Cloudflare's
//!      enterprise tier detects Chromium's TLS regardless of behaviour, so
//!      bundled Chromium *fails* and real Chrome *passes*.
//!   2. **playwright-stealth** init scripts — masks `navigator.webdriver`,
//!      injects `chrome.runtime`, normalises `plugins.length`, etc.
//!   3. **Manual CDP webdriver hide** — last-resort `Page.addInitScript` that
//!      patches the same flags by hand.
//!
//! We don't *launch* browsers from tokenade (it's a single Rust binary), but
//! emitting this strategy as a tool reply lets the agent route to the right
//! mechanism — and lets the next call carry the right context.

mod blocks;
mod detect;
mod remediate;
pub use blocks::*;
pub use detect::*;
pub use remediate::*;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ChallengeKind {
    CloudflareTurnstile,
    CloudflareInterstitial,
    HCaptcha,
    ReCaptcha,
    Datadome,
    Akamai,
    PerimeterX,
    /// Imperva (Incapsula) — CDN WAF, identified by `_incap_ses_*` /
    /// `visid_incap_*` cookies and a "Reference #" block when blocked.
    Imperva,
    /// F5 BIG-IP ASM — front-edge WAF, identified by `TSXX` cookies +
    /// `BIGipServer*` server-load-balancer markers + an inline
    /// `f5avr*` challenge script.
    F5BigIp,
    /// Sucuri Web Firewall — hosted WAF, identified by the
    /// `sucuri_cloudproxy_uuid` cookie and the canonical
    /// `Sucuri WebSite Firewall` HTML banner.
    Sucuri,
    /// Reblaze — Israeli bot-management CDN, identified by `rbzid`
    /// cookies and `__uzm[abcd]` session markers.
    Reblaze,
    /// Kasada — Australian bot-management vendor, identified by
    /// `KPSDK` script markers and `/v1/abc` API endpoints. crawl4ai
    /// flags this as a Tier-1 high-confidence pattern.
    Kasada,
    /// AWS WAF Bot Control / CAPTCHA — fronts imdb.com and espn.com (both
    /// CrUX top-1 000). Serves an HTTP **202** carrying a ~2 KB shell whose
    /// only payload is `window.gokuProps` plus a
    /// `*.token.awswaf.com/challenge.js` loader. Nothing else is on the page,
    /// so every extraction tier came back empty and the reader emitted a bare
    /// newline with rc=0.
    AwsWaf,
    /// No named provider matched, but the response is structurally hollow
    /// (see [`structural_block_signal`]). Deliberately NOT reported as a
    /// specific vendor: a hollow shell is equally consistent with an unnamed
    /// bot wall and with a JS-only page, and naming the wrong vendor is a
    /// claim the evidence does not support.
    GenericBlock,
    /// The "Client Challenge" shell (PyPI's search sits behind one). Unlike
    /// [`GenericBlock`] this page is NOT hollow — it has a `<body>`, a `<p>`
    /// and a paragraph of plain prose — so it needs its own kind: reporting it
    /// as "no body text, no semantic elements" would be false. It names no
    /// vendor either, so the kind describes the SHELL, not a provider.
    ClientChallengeShell,
    /// A first-party "prove you are human" interstitial — the site's own bot
    /// check, not a CDN's, so no vendor cookie or script identifies it. Named
    /// separately from the CAPTCHA vendors because there is no widget to solve
    /// and no provider to report: the evidence is the page SAYING it is a human
    /// check while carrying none of the requested content.
    HumanCheck,
}

impl ChallengeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChallengeKind::CloudflareTurnstile => "cloudflare-turnstile",
            ChallengeKind::CloudflareInterstitial => "cloudflare-interstitial",
            ChallengeKind::HCaptcha => "hcaptcha",
            ChallengeKind::ReCaptcha => "recaptcha",
            ChallengeKind::Datadome => "datadome",
            ChallengeKind::Akamai => "akamai-bot-manager",
            ChallengeKind::PerimeterX => "perimeterx",
            ChallengeKind::Imperva => "imperva-incapsula",
            ChallengeKind::F5BigIp => "f5-big-ip-asm",
            ChallengeKind::Sucuri => "sucuri-firewall",
            ChallengeKind::Reblaze => "reblaze",
            ChallengeKind::Kasada => "kasada",
            ChallengeKind::AwsWaf => "aws-waf",
            ChallengeKind::GenericBlock => "generic-block",
            ChallengeKind::ClientChallengeShell => "client-challenge-shell",
            ChallengeKind::HumanCheck => "human-check",
        }
    }
}

/// Inspect raw HTML *or* an accessibility-tree snapshot for the fingerprint
/// of a known anti-bot challenge. Patterns are conservative (substring +
/// well-known asset paths) so false positives on legitimate content are rare.
/// Visible-text ceiling under which a *widget* marker is taken as a real wall.
///
/// Widget markers — a reCAPTCHA/hCaptcha/Turnstile script or div, or the
/// Cloudflare Bot-Management beacon — say "this page can show a challenge",
/// not "you were blocked". They sit on the login form of perfectly ordinary
/// pages. Measured on a 178-page sample of the CrUX global top 1 000:
/// pornhub (5 locales, ~450 KB, 7.5 KB of extracted text) carries
/// `g-recaptcha`; jutarnji.hr and missav carry `recaptcha/api.js`; cdiscount,
/// betika and toyhou.se carry the beacon. **Ten of eleven challenge hits on
/// real top sites were widgets on real pages.**
///
/// What separates them is not page size but how much the page actually says.
/// Observed: real walls 57 / 297 / 308 visible chars (betika shell, cdiscount
/// "enable JavaScript", gst.gov.in CAPTCHA); the smallest real page carrying a
/// widget, toyhou.se, has 1 094. 600 sits in that gap.
///
/// Visible text alone is NOT enough, and assuming it was regressed a case
/// already fixed earlier: pixiv (75 KB) is a client-rendered shell, so it
/// has almost no visible text either — which is precisely the shape the JS
/// reader exists to handle. A wall is small in *both* senses, so both bounds
/// have to hold. The measured byte ceiling is kept as the second.
const WIDGET_MAX_WALL_VISIBLE: usize = 600;

/// Measured byte ceiling: real walls in the 63-page corpus were 1.7 KB and
/// 9 KB; the smallest genuine page carrying the same beacon was 74 KB.
const WIDGET_MAX_WALL_BYTES: usize = 32 * 1024;

/// Length of the text a reader would actually see: tags, `<script>` and
/// `<style>` bodies removed. Linear, allocation-free, ASCII-cased.
fn visible_text_len(html: &str) -> usize {
    let b = html.as_bytes();
    let mut i = 0;
    let mut n = 0;
    while i < b.len() {
        if b[i] == b'<' {
            let rest = &b[i + 1..];
            let skip_to: Option<&[u8]> = if starts_ci(rest, b"script") {
                Some(b"</script")
            } else if starts_ci(rest, b"style") {
                Some(b"</style")
            } else {
                None
            };
            if let Some(close) = skip_to {
                i = find_ci(b, close, i + 1).unwrap_or(b.len());
            }
            // skip the tag itself
            while i < b.len() && b[i] != b'>' {
                i += 1;
            }
            i += 1;
            continue;
        }
        // Count CHARACTERS, not bytes. A continuation byte (`10xxxxxx`) is the
        // tail of a character already counted; tallying it too made a Japanese
        // or Russian page read three times longer than it is, and both callers
        // compare this against a ceiling — so a genuine wall served in a
        // non-Latin language sailed over the ceiling and was taken for a real
        // page.
        if !b[i].is_ascii_whitespace() && (b[i] & 0xC0) != 0x80 {
            n += 1;
        }
        i += 1;
    }
    n
}

fn starts_ci(hay: &[u8], needle: &[u8]) -> bool {
    hay.len() >= needle.len() && hay[..needle.len()].eq_ignore_ascii_case(needle)
}

fn find_ci(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    (from..hay.len().saturating_sub(needle.len() - 1)).find(|&i| starts_ci(&hay[i..], needle))
}

/// `BIGipServer<pool>=<value>` — the cookie being SET, not its name quoted in a
/// list. Scans each occurrence for an `=` reached before any quote, whitespace
/// or comma, which is what separates an assignment from a JSON key.
fn bigip_cookie_assigned(t: &str) -> bool {
    let mut from = 0;
    while let Some(i) = t[from..].find("BIGipServer") {
        let at = from + i + "BIGipServer".len();
        for (off, c) in t[at..].char_indices().take(120) {
            match c {
                '=' => return true,
                '"' | '\'' | ' ' | ',' | ';' | '\n' | '<' | '>' => break,
                _ => {
                    let _ = off;
                }
            }
        }
        from = at;
    }
    false
}

/// Does a widget marker have any corroboration that this is really a wall?
/// A real challenge page says almost nothing; a login form with a captcha on it
/// says plenty.
fn widget_marker_is_a_wall(t: &str) -> bool {
    t.len() < WIDGET_MAX_WALL_BYTES && visible_text_len(t) < WIDGET_MAX_WALL_VISIBLE
}

#[cfg(test)]
include!("stealth_tests.rs");
