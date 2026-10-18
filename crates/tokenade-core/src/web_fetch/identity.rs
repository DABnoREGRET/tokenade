//! **What this client CLAIMS to be.**
//!
//! A command-line agent that presents itself as such gets a refusal page on a
//! good part of the web. Presenting itself as a browser is not a trick: it is
//! the only way to get what the user sees in their own browser.
//!
//! Headers go as a SET: a modern agent string with five-year-old headers is
//! more recognisable than an honest string. Hence the full table rather than
//! an isolated `User-Agent`, and the locale that follows the machine.

// No `use super::*;`: what this client claims to be depends on no other part
// of the transport — it is a table, and it reads on its own.

/// Trailer markers. One shape, one parser — an earlier change nearly shipped a silent
/// regression by changing a trailer without a test on the parser reading it,
/// and having had two shapes is what made that possible.
pub const META_SENTINEL: &str = "\x1fTOKENADE_META\x1f";
pub const META_SEP: &str = "\x1fTOKENADE_F\x1f";

/// Statuses that mean "this client is not welcome" rather than "no such page".
/// A 404 is an answer; a 403 is a wall worth trying a browser profile against.
pub(crate) fn refused(code: u16) -> bool {
    matches!(code, 401 | 403 | 406 | 429 | 451 | 503)
}

/// Windows, not Linux. Chrome-on-desktop-Linux is a fraction of a percent of
/// real browser traffic, so announcing it is itself a distinguishing signal —
/// the UA that was supposed to make us look ordinary put us in a bucket that is
/// almost entirely bots and developers. Windows 10 x64 Chrome is the single
/// most common desktop client there is, which is exactly the point: the least
/// interesting request in the log.
///
/// The Chrome major (137) matches `Sec-CH-UA` below, and the platform matches
/// `Sec-CH-UA-Platform`. Those three disagreeing is a cheaper tell than any of
/// them being stale, so they move together or not at all — and since 2026-09-10
/// there is a FOURTH member of that set one layer down:
/// `web_impersonate::EMULATED_CHROME_MAJOR`, the Chrome whose ClientHello the
/// optional impersonating rung presents. A test binds them.
pub(crate) const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
(KHTML, like Gecko) Chrome/137.0.0.0 Safari/537.36";

/// What we send when the TLD tells us nothing. Every gTLD (`.com`, `.org`,
/// `.io`, `.dev`, …) lands here, which is the majority of what we fetch.
pub const DEFAULT_ACCEPT_LANGUAGE: &str = "en-US,en;q=0.9";

/// `Accept-Language` for `url`, derived from its host's TLD.
///
/// A US locale asked of a `.it` or `.de` host is a mismatch anti-bot scoring
/// (DataDome and friends) cross-checks against geo-IP and the TLD itself: a
/// visitor reading an Italian newspaper who has never once preferred Italian is
/// not a shape human traffic makes. Deriving the locale from the TLD costs one
/// table lookup and removes the contradiction.
///
/// ONLY ccTLDs whose country has an unambiguous dominant language are listed.
/// Everything else — unknown suffixes, every gTLD, and the ccTLDs sold as
/// generic ones (`.io`, `.co`, `.me`, `.ai`, `.tv`, and `.rs`, which is Serbia
/// on paper and `docs.rs` in our traffic) — keeps
/// [`DEFAULT_ACCEPT_LANGUAGE`] unchanged, so nothing that works today moves.
///
/// Matching on the LAST label alone is deliberate, not a shortcut: the language
/// follows the country, and the country is the last label. `co.uk`, `com.br`
/// and `co.jp` therefore resolve through `uk`, `br` and `jp` with no
/// public-suffix list to carry or keep current.
pub fn accept_language_for(url: &str) -> &'static str {
    let Some((_, host)) = crate::url::scheme_host(url) else {
        return DEFAULT_ACCEPT_LANGUAGE;
    };
    // An IP literal has no TLD; `split` would hand us "1" and find nothing,
    // which is the right answer anyway.
    let tld = match host.rsplit('.').next() {
        Some(t) if !t.is_empty() => t,
        _ => return DEFAULT_ACCEPT_LANGUAGE,
    };
    match tld {
        // Europe, west
        "de" => "de-DE,de;q=0.9,en;q=0.8",
        "at" => "de-AT,de;q=0.9,en;q=0.8",
        "ch" => "de-CH,de;q=0.9,fr-CH;q=0.8,en;q=0.7",
        "fr" => "fr-FR,fr;q=0.9,en;q=0.8",
        "be" => "nl-BE,nl;q=0.9,fr-BE;q=0.8,en;q=0.7",
        "nl" => "nl-NL,nl;q=0.9,en;q=0.8",
        "it" => "it-IT,it;q=0.9,en;q=0.8",
        "es" => "es-ES,es;q=0.9,en;q=0.8",
        "pt" => "pt-PT,pt;q=0.9,en;q=0.8",
        "gr" => "el-GR,el;q=0.9,en;q=0.8",
        // Europe, north
        "se" => "sv-SE,sv;q=0.9,en;q=0.8",
        "no" => "nb-NO,nb;q=0.9,no;q=0.8,en;q=0.7",
        "dk" => "da-DK,da;q=0.9,en;q=0.8",
        "fi" => "fi-FI,fi;q=0.9,sv;q=0.8,en;q=0.7",
        "is" => "is-IS,is;q=0.9,en;q=0.8",
        // Europe, centre and east
        "pl" => "pl-PL,pl;q=0.9,en;q=0.8",
        "cz" => "cs-CZ,cs;q=0.9,en;q=0.8",
        "sk" => "sk-SK,sk;q=0.9,en;q=0.8",
        "hu" => "hu-HU,hu;q=0.9,en;q=0.8",
        "ro" => "ro-RO,ro;q=0.9,en;q=0.8",
        "bg" => "bg-BG,bg;q=0.9,en;q=0.8",
        "hr" => "hr-HR,hr;q=0.9,en;q=0.8",
        "si" => "sl-SI,sl;q=0.9,en;q=0.8",
        "lt" => "lt-LT,lt;q=0.9,en;q=0.8",
        "lv" => "lv-LV,lv;q=0.9,en;q=0.8",
        "ee" => "et-EE,et;q=0.9,en;q=0.8",
        "ru" => "ru-RU,ru;q=0.9,en;q=0.8",
        "ua" => "uk-UA,uk;q=0.9,ru;q=0.8,en;q=0.7",
        "tr" => "tr-TR,tr;q=0.9,en;q=0.8",
        // Asia
        "jp" => "ja-JP,ja;q=0.9,en;q=0.8",
        "kr" => "ko-KR,ko;q=0.9,en;q=0.8",
        "cn" => "zh-CN,zh;q=0.9,en;q=0.8",
        "tw" => "zh-TW,zh;q=0.9,en;q=0.8",
        "hk" => "zh-HK,zh;q=0.9,en;q=0.8",
        "th" => "th-TH,th;q=0.9,en;q=0.8",
        "vn" => "vi-VN,vi;q=0.9,en;q=0.8",
        "id" => "id-ID,id;q=0.9,en;q=0.8",
        "il" => "he-IL,he;q=0.9,en;q=0.8",
        "sa" => "ar-SA,ar;q=0.9,en;q=0.8",
        "ae" => "ar-AE,ar;q=0.9,en;q=0.8",
        "in" => "en-IN,en;q=0.9,hi;q=0.8",
        // Americas
        "br" => "pt-BR,pt;q=0.9,en;q=0.8",
        "mx" => "es-MX,es;q=0.9,en;q=0.8",
        "ar" => "es-AR,es;q=0.9,en;q=0.8",
        "cl" => "es-CL,es;q=0.9,en;q=0.8",
        "ca" => "en-CA,en;q=0.9,fr-CA;q=0.8",
        // English-speaking, but not American
        "uk" => "en-GB,en;q=0.9",
        "ie" => "en-IE,en;q=0.9",
        "au" => "en-AU,en;q=0.9",
        "nz" => "en-NZ,en;q=0.9",
        "za" => "en-ZA,en;q=0.9",
        _ => DEFAULT_ACCEPT_LANGUAGE,
    }
}

/// `headers` with its `Accept-Language` bound to `url`'s TLD. Rewriting the
/// entry in place rather than appending one keeps the header ORDER fixed —
/// header order is itself fingerprinted, and a locale is not worth a reordering.
pub(crate) fn localized_headers(headers: &[&str], url: &str) -> Vec<String> {
    let lang = accept_language_for(url);
    headers
        .iter()
        .map(|h| {
            if h.starts_with("Accept-Language:") {
                format!("Accept-Language: {lang}")
            } else {
                (*h).to_string()
            }
        })
        .collect()
}

/// The headers a real Chrome sends on a top-level navigation.
///
/// The browser-UA fallback used to claim to BE Chrome while sending nothing
/// else — a malformed request bot filters reject on sight. Completing the set
/// unblocked 17 of 39 refusing domains (MEASURED: britannica, cdc.gov,
/// mayoclinic, quora, indeed, elpais, nih.gov, academia.edu …).
///
/// This is where header work stops. The 22 domains still refusing do so on
/// signals no header can change (TLS fingerprint, IP reputation, a challenge
/// page).
///
/// One of those three is now addressed, and OUTSIDE this list because it is not
/// a header: [`crate::web_impersonate`] presents a browser's ClientHello and
/// HTTP/2 SETTINGS, and is reached only when the answer would otherwise be a
/// challenge page. The other two are left alone — proxy rotation and challenge
/// solving are evasion engineering, not a fetcher improvement, and a site that
/// says no to a coherent browser is answering the question it was asked.
pub const BROWSER_HEADERS: &[&str] = &[
    "Accept: text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
    "Accept-Language: en-US,en;q=0.9",
    "Sec-Fetch-Dest: document",
    "Sec-Fetch-Mode: navigate",
    "Sec-Fetch-Site: none",
    "Sec-Fetch-User: ?1",
    "Upgrade-Insecure-Requests: 1",
    "Sec-CH-UA: \"Chromium\";v=\"137\", \"Not:A-Brand\";v=\"24\"",
    "Sec-CH-UA-Mobile: ?0",
    "Sec-CH-UA-Platform: \"Windows\"",
];

// ── The in-process transport: reqwest + rustls (ring) ────────────────────────
//
// This is the PRIMARY channel. It is in-process, speaks HTTP/2, carries a
// cookie store, and cross-compiles to every release target with no C toolchain.
// curl is kept only as a handshake fallback (see `curl_fallback`): the rare
// server whose TLS rustls refuses but curl's OpenSSL accepts. The `ureq`
// HTTP/1.1 web fallback is GONE — announcing a Chrome UA over HTTP/1.1 was a
// contradiction no browser makes, a more distinctive signal than curl itself.
