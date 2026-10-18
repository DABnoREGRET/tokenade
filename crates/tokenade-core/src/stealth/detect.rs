//! **Is this page a CHALLENGE rather than an answer?**
//!
//! Compacting an anti-bot verification page is wasted work twice over: the
//! agent did not receive what it asked for, and it reads the fold as if it
//! had.
//!
//! Detection is made of markers AND a measurement: a widget marker on a rich
//! page is a widget, the same marker on an almost empty page is a WALL.
//! Confusing the two discards real content.

use super::*;

pub fn detect_challenge(text: &str) -> Option<ChallengeKind> {
    let t = text;
    // Bypass when the input is clearly source code / docs / shell
    // output — not a fetched HTML response. Without this, our own
    // `stealth.rs`, CHANGELOG.md, and any grep over them
    // false-positive on every literal we test below (observed: 19
    // events in gain.jsonl, mean -70% inflation, all on Rust/MD
    // files we were editing).
    if looks_like_source_or_doc(t) {
        return None;
    }
    // ---- WIDGET markers -----------------------------------------------------
    // Everything in this block says "this page CAN show a challenge", not "you
    // were blocked": a captcha script or div sits on the login form of any
    // number of perfectly ordinary pages. They are therefore corroborated by
    // `widget_marker_is_a_wall` — a real wall says almost nothing. Response
    // markers (below) need no corroboration and must stay ungated.
    let widget = if t.contains("challenges.cloudflare.com/turnstile")
        || t.contains("class=\"cf-turnstile")
        || t.contains("id=\"cf-turnstile")
        || t.contains("class='cf-turnstile")
        || t.contains("id='cf-turnstile")
    {
        Some(ChallengeKind::CloudflareTurnstile)
    } else if t.contains("hcaptcha.com/1/api.js") || t.contains("h-captcha") {
        Some(ChallengeKind::HCaptcha)
    } else if t.contains("google.com/recaptcha/api.js")
        || t.contains("www.recaptcha.net")
        || t.contains("g-recaptcha")
    {
        Some(ChallengeKind::ReCaptcha)
    } else if t.contains("_pxAppId") || t.contains("_pxHostUrl") || t.contains("_pxJsClientSrc") {
        // PerimeterX's first-party snippet, which is what its press-and-hold
        // wall actually ships — no `px-cdn.net` host and no `_px*` cookie in
        // the HTML, so the response markers below never fired. sams.com.mx
        // (CrUX rank 10 000) was delivered as CONTENT reading "Verify Your
        // Identity / Mantén presionado el botón", which an agent would take
        // for the page.
        //
        // Filed as a WIDGET, not a response marker: the same snippet ships on
        // ordinary pages, so it only counts once `widget_marker_is_a_wall`
        // confirms the page says almost nothing.
        Some(ChallengeKind::PerimeterX)
    } else if t.contains("perimeterx.net") {
        // The HUMAN/PerimeterX SDK host. It was filed with the structural
        // markers above, but loading the SDK is not being blocked BY it —
        // wayfair.com (1.03 MB) and walmart.com (398 KB) both ship it on
        // ordinary pages and were served bot-wall remediation. The cookies and
        // `captcha.px-cdn.net` above are the real response evidence.
        Some(ChallengeKind::PerimeterX)
    } else if t.contains("/cdn-cgi/challenge-platform/") {
        // Cloudflare Bot Management injects this beacon into ordinary 200
        // responses to score the visitor — we measured 6 full articles
        // carrying it on a 63-page corpus.
        Some(ChallengeKind::CloudflareInterstitial)
    } else {
        None
    };

    // Cloudflare "Just a moment…" / managed challenge page. These markers only
    // ever appear on a real challenge response: the title, the mitigation
    // header, the bypass hook, and `cf_chl_opt` (the challenge's own options
    // object, injected by the interstitial and by nothing else).
    //
    // `challenge-platform` + `/orchestrate/` joins them: the interstitial loads
    // its own script from `/cdn-cgi/challenge-platform/h/{b,g}/orchestrate/…`,
    // and NEITHER half proves anything alone. The bare beacon ships on ordinary
    // 200s (it stays a gated widget below), and `/orchestrate/` on its own is an
    // ordinary URL word — a docs page linking `/guides/orchestrate/` must not be
    // flagged. Together they only occur on the real page, so this pair needs no
    // hollowness corroboration: a challenge page padded out with script would
    // otherwise pass the widget gate and be delivered as content.
    if t.contains("Just a moment...")
        || t.contains("cf-mitigated")
        || t.contains("cf-chl-bypass")
        || t.contains("cf_chl_opt")
        || (t.contains("/cdn-cgi/challenge-platform/") && t.contains("/orchestrate/"))
    {
        return Some(ChallengeKind::CloudflareInterstitial);
    }
    // "Client Challenge" shell — the wall PyPI's search sits behind (MEASURED
    // 2026-07-22: `pypi.org/search/?q=…` returned 232 chars of it, delivered as
    // if it WERE the search results, with no disclosure).
    //
    // Tier 3 cannot reach this one and no shape metric will: the page is a
    // well-formed document with a `<body>`, a `<p>` and a paragraph of ordinary
    // prose — hollow in MEANING, not in structure. It also names no vendor
    // (`_incap_ses`, `imperva`, `datadome` — none present), so it cannot be
    // attributed, hence `GenericBlock` rather than a provider we would be
    // guessing at.
    //
    // BOTH markers are required. The title alone is a phrase a page could
    // legitimately discuss; the title plus the failure sentence is the shell.
    // The apostrophe is matched around, not through: the served page uses a
    // curly `’`, and a straight-quote copy of the same shell must still match.
    if t.contains("<title>Client Challenge</title>") && t.contains("required part of this site") {
        return Some(ChallengeKind::ClientChallengeShell);
    }
    // First-party "are you human" interstitial. MEASURED 2026-07-22 on the
    // corpus' `ddg_search.html`: 14 174 bytes of DuckDuckGo's own CAPTCHA
    // ("Unfortunately, bots use DuckDuckGo too… Select all squares containing a
    // duck") distilled to 21 bytes — `[DuckDuckGo](/html/)` — with NO
    // disclosure. The agent reads that as a search that found one link.
    //
    // The SEARCH path already survives this page, and by a hair worth
    // recording: it carries exactly ONE `serp::challenge_markers` hit
    // (`challenge-form`), so `looks_like_challenge` — which wants two — misses
    // it, and it is the marker-plus-zero-hits rule in `fetch_and_validate` that
    // catches it. Only the `web` delivery path was blind, which is what this
    // signature fixes. Checked, not assumed: an earlier draft of this comment
    // claimed the search path counted it as a success.
    //
    // The length guard is what makes the phrase safe to match: an article
    // ABOUT bot checks is long-form, an interstitial is a few hundred chars of
    // text. It bounds the phrase, it does not do the classifying.
    // What unites this family is that the page SAYS, in plain prose, that it is
    // a bot check. Each phrase below is one such self-description, MEASURED on a
    // page that was being delivered as content:
    //   * DuckDuckGo's CAPTCHA (corpus `ddg_search.html`) → 21 bytes,
    //     `[DuckDuckGo](/html/)`, read as a search that found one link;
    //   * codeberg.org's "Cookie monster!" interstitial, served on
    //     `/explore/repos` and on source-file paths → 259 bytes, read as the page
    //     (MEASURED 2026-07-22).
    // The list grows by measurement, not by guessing at vendors: each entry is a
    // sentence observed on a real interstitial.
    const SELF_DESCRIBED_BOT_CHECK: &[&str] = &[
        "confirm this search was made by a human",
        "Please complete the following challenge",
        "automated check to get rid of",
    ];
    if visible_text_len(t) < 2_000 && SELF_DESCRIBED_BOT_CHECK.iter().any(|p| t.contains(p)) {
        return Some(ChallengeKind::HumanCheck);
    }
    // Datadome (CDN-side bot wall) — `dd_cookie` cookie, `datadome-captcha`
    // iframe, or its `geo.captcha-delivery.com` endpoint.
    if t.contains("geo.captcha-delivery.com")
        || t.contains("datadome-captcha")
        || t.contains("ddJsKey")
    {
        return Some(ChallengeKind::Datadome);
    }
    // AWS WAF Bot Control — the 202 interstitial injects exactly two globals
    // before loading `challenge.js`. Both are matched as ASSIGNMENTS
    // (`window.` prefix), for the reason `_abck` and `PerimeterX` were
    // tightened: a bare `gokuProps` / `awswaf` substring matches this file's
    // own comments and any doc naming the vendor.
    if t.contains("window.gokuProps") || t.contains("window.awsWafCookieDomainList") {
        return Some(ChallengeKind::AwsWaf);
    }
    // Akamai Bot Manager — require STRUCTURAL evidence, not just
    // the keyword `_abck` in prose. The `=` after the cookie name is
    // diagnostic: docs / CHANGELOGs write "`_abck`" or "_abck" alone,
    // a real Akamai challenge always uses `_abck=…` (cookie
    // assignment) or `name="sensor_data"` (form field). Bug 2026-05-29:
    // matching `_abck` alone false-positived on our own CHANGELOG.
    if t.contains("_abck=")
        || t.contains("set-cookie: _abck")
        || t.contains("sensor_data=")
        || t.contains("name=\"sensor_data\"")
    {
        return Some(ChallengeKind::Akamai);
    }
    // PerimeterX / HUMAN — STRUCTURAL evidence required.
    // The bare brand name "PerimeterX" used to be a match; that
    // false-positived on our own CHANGELOG.md, stealth.rs source
    // comments, and any tech doc naming the competitor. Real
    // PerimeterX challenges always carry a `_px*` cookie or hit
    // the `captcha.px-cdn.net` / `/px/` endpoints.
    if t.contains("captcha.px-cdn.net")
        || t.contains("set-cookie: _px")
        || t.contains("Cookie: _px")
        || t.contains("name=\"_pxhd\"")
    {
        return Some(ChallengeKind::PerimeterX);
    }
    // Imperva / Incapsula — distinctive `_incap_ses_` / `visid_incap_`
    // cookies, often paired with a "Reference #...." block in the
    // blocked HTML response.
    if t.contains("_incap_ses_")
        || t.contains("visid_incap_")
        || t.contains("Incapsula incident ID")
        || t.contains("imperva.com")
    {
        return Some(ChallengeKind::Imperva);
    }
    // F5 BIG-IP ASM — load-balancer cookies + inline challenge JS. `BIGipServer`
    // needs STRUCTURAL evidence for the same reason `_abck` and `PerimeterX`
    // did: temu.com (280 KB) ships a cookie-consent config that *enumerates*
    // cookie names — `"BIGipServerCentinel-Prod-Web…":1` next to `"cf_clearance":1`
    // — and a bare substring match served bot-wall remediation for a working
    // retail page. A real F5 cookie is an ASSIGNMENT.
    if bigip_cookie_assigned(t) || t.contains("f5avr") || t.contains("TSPD_101") {
        return Some(ChallengeKind::F5BigIp);
    }
    // Sucuri — distinctive cookie + canonical banner string.
    if t.contains("sucuri_cloudproxy_uuid")
        || t.contains("Sucuri WebSite Firewall")
        || t.contains("sucuri-firewall-block")
    {
        return Some(ChallengeKind::Sucuri);
    }
    // Reblaze — `rbzid` cookie + `__uzm[abcd]` session markers.
    if t.contains("rbzid")
        || t.contains("__uzma=")
        || t.contains("__uzmd=")
        || t.contains("reblaze.com")
    {
        return Some(ChallengeKind::Reblaze);
    }
    // Kasada — `KPSDK.scriptStart = KPSDK.now()` is uniquely Kasada
    // (crawl4ai antibot_detector.py:26-67 Tier-1). Also matches the
    // `/v1/abc/` Kasada API endpoint that real challenged pages hit.
    if t.contains("KPSDK.scriptStart") || t.contains("KPSDK.now()") || t.contains("kasada.io") {
        return Some(ChallengeKind::Kasada);
    }
    // Additional Akamai signal — `<title>Pardon Our Interruption…`
    // or `<h1>Pardon Our Interruption</h1>` on the rendered block
    // page. Bare phrase in prose used to false-positive on our own
    // CHANGELOG / stealth.rs documentation; require an HTML wrapper.
    //
    // Matched case-INSENSITIVELY over a bounded head: Akamai serves its deny
    // page as ALL-UPPERCASE markup (`<HTML><HEAD>\n<TITLE>Access
    // Denied</TITLE>`), so the case-sensitive needles never fired and
    // homedepot.com came back refused with NO challenge label at all. The head
    // bound keeps the structural requirement that stops prose false positives
    // (these markers live in `<head>`/the first `<h1>`; a block page is a
    // couple of hundred bytes) and avoids lowercasing a whole response —
    // `detect_challenge` runs on every page we fetch.
    let head_lower = crate::caps::truncate_at_char_boundary(t, 4096).to_ascii_lowercase();
    // `Pardon Our Interruption` is Akamai's own wording. A bare "Access denied"
    // title is NOT: affordablegolf.co.uk serves a multilingual `data-i18n` deny
    // page with no Akamai marker anywhere, and calling that
    // `akamai-bot-manager` would name a vendor on no evidence — the mistake
    // `GenericBlock` exists to avoid. So the deny title counts for Akamai only
    // alongside Akamai's own edge signature, and otherwise falls through to the
    // generic block detection below (which must stay reachable: a Cloudflare
    // 1020 page can carry a deny title too, and its own check is further down).
    // `edgesuite`, not `edgesuite.net`: Akamai entity-encodes the punctuation of
    // its reference URL (`errors&#46;edgesuite&#46;net`), so the dotted form is
    // never present as a literal. Match the vendor token, which survives.
    let akamai_edge = head_lower.contains("edgesuite") || head_lower.contains("akamai");
    if head_lower.contains("<title>pardon our interruption")
        || head_lower.contains("<h1>pardon our interruption")
        || (head_lower.contains("<title>access denied") && akamai_edge)
    {
        return Some(ChallengeKind::Akamai);
    }
    // Additional Cloudflare error-code shell — `<span class="cf-error-code">`
    // appears on Cloudflare 1015 / 1020 (banned) interstitials. Require
    // structural CSS-class context (the bare string `cf-error-code`
    // appears in CHANGELOG, this file's own doc-comments, etc.).
    if t.contains("class=\"cf-error-code")
        || t.contains("class='cf-error-code")
        || t.contains("CF-RAY:")
    {
        return Some(ChallengeKind::CloudflareInterstitial);
    }
    // Cloudflare's BLOCK page (error 1020 / firewall rule), which is a
    // different template from the "Just a moment…" managed challenge and
    // carries none of its markers. Found by capturing non-200 responses for the
    // first time: **8 of 29 real blocked bodies were this page and
    // every previous round had discarded them**, because the corpora only ever
    // kept HTTP 200. Four rounds of removing false positives and nobody had
    // measured a true positive.
    //
    // The title is Cloudflare's own and cannot appear on a served page; source
    // and docs that quote it are already handled by `looks_like_source_or_doc`.
    if t.contains("Attention Required! | Cloudflare") || t.contains("Sorry, you have been blocked")
    {
        return Some(ChallengeKind::CloudflareInterstitial);
    }
    // No response marker fired. A widget on its own only counts if the page has
    // nothing else to say — the expensive visible-text scan runs here, once, and
    // only for the pages that carry a widget at all.
    widget.filter(|_| widget_marker_is_a_wall(t))
}
