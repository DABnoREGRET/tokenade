//! **What can be done about a challenge.**
//!
//! Sometimes nothing, and saying so is the right answer. A remediation that
//! fails silently returns an empty page the agent takes for the content.
//!
//! The advice returned therefore names the challenge type and the remaining
//! path, because "the page could not be read" is not actionable and
//! "Cloudflare Turnstile, try the stealth browser" is.

use super::*;

/// What the CALLER can do. Returned as agent-actionable markdown.
///
/// It used to be three numbered tiers naming Patchright, playwright-stealth and
/// a CDP init script — which is how you would BUILD a stealth fetcher, not
/// anything the agent holding a challenge page can do. Measured over our own
/// transcripts: **11 advisories served, and the tiers were acted on zero
/// times** — eight moved on, three retried the same URL, which no line of the
/// message told them would return the same page.
///
/// Three kinds in this very function already return early rather than append
/// the tiers, each with a comment saying they "would be noise", and each says
/// instead what the caller should do next. This is that same judgement applied
/// to the vendor kinds, whose heads already name the real remediation inline.
pub fn stealth_remediation(kind: ChallengeKind) -> String {
    let common = "\n\
                  - An immediate retry returns the same page: the wall is bound \
                  to the client and IP, not just to the request. Clearing it \
                  needs a real-Chrome stealth fetch (TLS/JA3 is checked before \
                  any behaviour is), i.e. a browser you drive.\n\
                  - Without one: save the page from a browser you are signed in \
                  to and hand tokenade the file — `tokenade web saved.html`, or \
                  `curl … | tokenade web -`. An official API, where the site has \
                  one, is the reliable route.";
    let head = match kind {
        ChallengeKind::CloudflareTurnstile => {
            "Cloudflare Turnstile widget detected. The page asked the browser to solve a CAPTCHA.\n\
             - Stock Playwright/Chromium will be detected by Cloudflare's enterprise tier on the \
             pre-check (TLS/JA3 fingerprint), regardless of behaviour."
        }
        ChallengeKind::CloudflareInterstitial => {
            "Cloudflare managed-challenge interstitial detected (\"Just a moment…\"). The agent \
             received the challenge page, not the requested content."
        }
        ChallengeKind::HCaptcha => {
            "hCaptcha widget detected. Page requires user interaction or a solver."
        }
        ChallengeKind::ReCaptcha => {
            "Google reCAPTCHA detected. v3 may pass on a good IP/UA; v2 needs solving."
        }
        ChallengeKind::Datadome => {
            "Datadome bot wall detected. JA3 + TLS + cookie-based; needs real-Chrome JA3 + warmup."
        }
        ChallengeKind::Akamai => {
            "Akamai Bot Manager detected (`_abck` + sensor_data). Same TLS-fingerprint constraint."
        }
        ChallengeKind::PerimeterX => {
            "PerimeterX / HUMAN bot wall detected. JS-challenge based; needs Patchright stealth."
        }
        ChallengeKind::Imperva => {
            "Imperva / Incapsula WAF detected. Often passes with real-Chrome JA3 + warm cookies; \
             watch for `_incap_ses_*` cookie rotation on each request."
        }
        ChallengeKind::F5BigIp => {
            "F5 BIG-IP ASM detected. JS challenge with TS cookies; needs Patchright + a real \
             Chrome user-agent on first request to seed the TSPD cookie correctly."
        }
        ChallengeKind::Sucuri => {
            "Sucuri Web Firewall detected. CDN-fronted blocklist; the agent's IP / ASN is likely \
             the trigger — rotate IP or contact site owner to allow-list."
        }
        ChallengeKind::Reblaze => {
            "Reblaze bot management detected. JS challenge similar to PerimeterX; Patchright + \
             real-Chrome JA3 typically passes after a single warm-up navigation."
        }
        ChallengeKind::Kasada => {
            "Kasada bot management detected (KPSDK markers). Highly aggressive — needs Patchright \
             + real-Chrome JA3 + a fresh warmed-up profile. Fingerprint matching is strict; \
             rotating UA between requests is typically counterproductive."
        }
        ChallengeKind::AwsWaf => {
            "AWS WAF Bot Control challenge detected (`gokuProps` + `token.awswaf.com`). Served as \
             an HTTP 202 whose body is a ~2 KB loader shell — there is no page content to \
             extract. Solving it needs a real browser to run `challenge.js` and mint the \
             `aws-waf-token` cookie."
        }
        // Early return like the two above: there is no vendor widget to route
        // to, so the 3-tier stealth advice would be noise. What the agent needs
        // is that the page it holds is a bot check.
        ChallengeKind::HumanCheck => {
            return format!(
                "[tokenade: challenge={}]\n\
                 A \"prove you are human\" interstitial was served instead of the page. It is \
                 the site's OWN bot check (no CDN vendor identifies it), and it carries none of \
                 the requested content — a distilled result from this page reads like a real \
                 but very thin answer, which is how it goes unnoticed.\n\
                 - If you fetched this yourself, treat the site as unavailable for now rather \
                 than as having returned no results, and try another source.\n\
                 - The check is bound to the requesting IP as much as to the request itself, so \
                 an immediate retry usually returns the same page.",
                kind.as_str()
            );
        }
        // Like GenericBlock, this one states only that
        // a required part did not load, which is as consistent with a strict
        // JS/cookie requirement as with a bot wall. What the agent needs is the
        // plain fact that THIS IS NOT THE PAGE — the failure that cost the most
        // here was 232 chars of it being read as if it were search results.
        ChallengeKind::ClientChallengeShell => {
            return format!(
                "[tokenade: challenge={}]\n\
                 A \"Client Challenge\" interstitial was served instead of the page. Its text \
                 reads like ordinary prose (\"A required part of this site couldn't load…\"), so \
                 it is easy to mistake for a thin or empty result — it is neither. NONE of the \
                 requested content is present.\n\
                 - It names no vendor, so tokenade does not guess one.\n\
                 - Measured on this shell: a plain `curl` with a full browser header set gets \
                 the same interstitial, so retrying the fetch as-is will not clear it.\n\
                 - An official API or data endpoint, where one exists, is the reliable route \
                 (for PyPI search, `https://pypi.org/simple/` and the JSON API are not walled).",
                kind.as_str()
            );
        }
        // GenericBlock is the ONE kind that does not assert a bot wall, so it
        // returns early rather than appending the stealth remediation: the
        // same hollow shell is produced by a JS-only page, and telling the
        // agent to defeat a wall that may not exist sends it down a dead end.
        ChallengeKind::GenericBlock => {
            return format!(
                "[tokenade: challenge={}]\n\
                 The response carried no extractable content: no body text, no semantic \
                 elements. This is EITHER an unnamed anti-bot block OR a page whose text is \
                 rendered client-side. tokenade cannot be certain from the response alone; \
                 what it can say:\n\
                 - {}.\n\
                 - If it renders client-side, the content may arrive by XHR after load, which \
                 no session-less fetch reaches.",
                kind.as_str(),
                crate::web::pipeline::STATUS_QUESTION
            );
        }
    };
    format!("[tokenade: challenge={}]\n{head}{common}", kind.as_str())
}

/// Detect & format in one call, **named providers only**.
///
/// `js_render::render` gates on this: a named vendor wall has no hidden
/// content to run out of it, so the engine budget is wasted there. It must
/// NOT use the structural variant below — a hollow shell is the engine's
/// primary target (quora, airbnb, x.com are all hollow), and widening this
/// call would make the engine refuse exactly the pages it wins on.
pub fn detect_and_remediate(text: &str) -> Option<(ChallengeKind, String)> {
    let kind = detect_challenge(text)?;
    Some((kind, stealth_remediation(kind)))
}

/// Detect & format including the structural Tier-3 fallback.
///
/// The HTML compactor uses this. Tier 3 existed but had zero callers, so a
/// hollow response from an unnamed provider (AWS WAF's 202 shell on imdb.com
/// and espn.com) produced no banner, every extraction tier came back empty,
/// and `tokenade web` handed the agent a bare newline with rc=0 — a silent
/// nothing, the one outcome the reader must never produce. Safe here and not
/// in `render` because this only ever PREPENDS an explanation: when the
/// engine does materialise content, the body clears
/// `CHALLENGE_BANNER_MAX_BODY_CHARS` and the banner is suppressed anyway.
pub fn detect_and_remediate_any(text: &str) -> Option<(ChallengeKind, String)> {
    let kind = detect_any_block(text)?;
    Some((kind, stealth_remediation(kind)))
}
