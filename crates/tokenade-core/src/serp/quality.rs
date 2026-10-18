//! **Is this a real result?**
//!
//! An engine page carries as many links to the engine itself as it carries
//! results, and an anti-bot challenge page carries zero results while
//! looking like a normal page.
//!
//! Delivering a challenge as an answer is the most expensive failure: the
//! agent reads "verification in progress" as the content of its search and
//! concludes something. Hence `looks_like_challenge`, which refuses rather
//! than return an empty page.

use super::*;

/// Hosts that ARE search engines. A link to one of these from a SERP is the
/// engine's own furniture — its verticals, its preferences page, its sign-in —
/// never a result.
pub(crate) const ENGINE_HOSTS: &[&str] = &[
    "google.com",
    "google.co",
    "bing.com",
    "duckduckgo.com",
    "ecosia.org",
    "mojeek.com",
    "yahoo.com",
    "search.marginalia.nu",
    "brave.com",
    "startpage.com",
    "qwant.com",
    "microsoft.com",
];

/// Is this a link to a RESULT, or to the search engine's own chrome?
///
/// The rule is structural, not a denylist of paths: a search result is an
/// ABSOLUTE url on a host that is not a search engine. It has to be structural,
/// because the denylist version of this function shipped covering Google, Bing
/// and DuckDuckGo only — and by the time four engines were wired, three of them
/// were serving their own navigation bars as results.
///
/// MEASURED on `Klimagerät Geräuschpegel dB Schlafzimmer`, all four engines,
/// same session: yahoo returned "Yahoo / Settings / Finance / Sports / Weather /
/// Sign In", mojeek returned "Images / Substack / About / Mojeek Ads", ecosia
/// put "AI Chat / Images / Videos" above its real hits. Every one of them is
/// either a relative url or an engine-owned host, and none of them survives
/// this rule.
pub(crate) fn is_useful_url(u: &str) -> bool {
    if u.is_empty() || u.starts_with('#') || u.starts_with("javascript:") {
        return false;
    }
    // Relative — necessarily a link back into the SERP. This alone accounts for
    // every mojeek and ecosia chrome link.
    let Some(rest) = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))
    else {
        return false;
    };
    let host = rest
        .split('/')
        .next()
        .unwrap_or("")
        .rsplit('@')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if host.is_empty() {
        return false;
    }
    // Match on a DOMAIN boundary, not a substring: `mybing.com.example.org`
    // must not read as bing, and `sports.yahoo.com` must.
    !ENGINE_HOSTS
        .iter()
        .any(|e| host == *e || host.ends_with(&format!(".{e}")))
}

/// Does this result set actually answer THIS query?
///
/// The guard exists because "we got N links" was the only success test, and a
/// portal's navigation bar is reliably more than N links. An agent was told
/// `✓ SEARCH OK … do NOT retry` over ten Polish weather results for a German
/// question about air-conditioner noise, and went on believing it had searched.
///
/// Deliberately WEAK. A good result need not repeat the query, so this only
/// catches the case where a result set has nothing whatsoever to do with what
/// was asked. It matches on a PREFIX so German compounding
/// (`Klimagerät` → `Klimageräte`) and ordinary plurals still count, and it
/// abstains entirely when the query carries no token long enough to judge —
/// abstaining is correct there, guessing is not.
pub fn plausible_for_query(results: &[Result], query: &str) -> bool {
    /// Shortest query token worth matching on. Below this, tokens are
    /// prepositions and units (`dB`, `the`, `de`) that match everything.
    const MIN_TOKEN: usize = 4;
    /// Prefix compared, so inflections and compounds still match.
    const PREFIX: usize = 6;
    /// Share of results that must relate to the query. One in three is low on
    /// purpose: engines legitimately return tangential hits, and the failure
    /// this catches is TOTAL irrelevance, not imperfect ranking.
    const MIN_RELATED_NUM: usize = 1;
    const MIN_RELATED_DEN: usize = 3;

    if results.is_empty() {
        return false;
    }
    let lower = query.to_lowercase();
    let needles: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.chars().count() >= MIN_TOKEN)
        .map(|t| {
            let end = t
                .char_indices()
                .nth(PREFIX)
                .map(|(i, _)| i)
                .unwrap_or(t.len());
            &t[..end]
        })
        .collect();
    if needles.is_empty() {
        // Nothing to judge with — abstain rather than reject.
        return true;
    }
    let related = results
        .iter()
        .filter(|r| {
            let hay = format!("{} {} {}", r.title, r.snippet, r.url).to_lowercase();
            needles.iter().any(|n| hay.contains(n))
        })
        .count();
    related * MIN_RELATED_DEN >= results.len() * MIN_RELATED_NUM
}

/// How many DISTINCT bot-challenge markers a page carries.
///
/// Split out from the verdict because the right threshold depends on what else
/// the caller knows. Two distinct markers is conclusive on its own; ONE is only
/// suggestive — and a real results page for the query "captcha bypass" carries
/// one too. See [`looks_like_challenge`] and `search::fetch_and_validate`.
pub fn challenge_markers(html: &str) -> usize {
    // Phrased in several languages because the engine answers in the language
    // of the exit IP, not of the query.
    const MARKERS: &[&str] = &[
        "captcha",
        "recaptcha",
        "unusual traffic",
        "are you a robot",
        "verify you are human",
        "résoudre le défi",
        "veuillez résoudre",
        "challenge-form",
        "cf-challenge",
        "checking your browser",
        "enable javascript and cookies to continue",
        // Cloudflare interstitial fingerprints (SearXNG facts). One `__cf_chl_`
        // covers both the jschl and the captcha token forms.
        "__cf_chl_",
        "/cdn-cgi/challenge-platform/",
        "_cf_chl_enter",
        "cf-error-code",
        // Marginalia's soft-block: HTTP 200 with a ~983-byte body and this
        // phrase — a live engine silently returning nothing. Combined with zero
        // parsed results the caller types it as a challenge.
        "barraged by queries from bots",
    ];
    let head: String = html
        .chars()
        .take(200_000)
        .collect::<String>()
        .to_lowercase();
    MARKERS.iter().filter(|m| head.contains(**m)).count()
}

/// Did the engine answer with a bot challenge instead of a SERP?
///
/// Conclusive form: TWO distinct markers. One is not enough on its own — an
/// article about CAPTCHAs, or a genuine results page for a query about them,
/// carries exactly one, and taking an engine out of rotation over that costs
/// more than it saves.
///
/// The captured Bing challenge (HTTP 200, the query echoed in the `<title>`,
/// zero result blocks) carries the single marker `captcha`, six times — so it
/// does NOT trip this function, and it must not: the caller catches it by
/// combining ONE marker with ZERO parsed results, which no real SERP can
/// produce. Do not "fix" this by lowering the threshold; that trade was
/// measured and the split is deliberate.
pub fn looks_like_challenge(html: &str) -> bool {
    challenge_markers(html) >= 2
}

/// Dedup by `host + first path segment` so the same site can't fill the list.
pub(crate) fn dedup_key(url: &str) -> String {
    let no_scheme = url.split("://").nth(1).unwrap_or(url);
    let mut parts = no_scheme.splitn(3, '/');
    let host = parts.next().unwrap_or("");
    let first = parts.next().unwrap_or("");
    format!("{host}/{first}")
}

/// One decoder for the product — `web::decode_entities`. This module used
/// to carry its own table, which is how a German result could arrive decoded on
/// one path and raw on another.
pub(crate) fn decode_entities(s: &str) -> String {
    decode_numeric_entities(&crate::web::decode_entities(s))
}

/// Decode decimal (`&#183;`) and hex (`&#x2019;`) numeric character
/// references — Bing and Yahoo pepper titles/snippets with them (`·`, `é`,
/// `’`). Unknown / malformed refs are left verbatim. Skipped entirely when
/// the string has no `&#` so the common case pays nothing.
pub(crate) fn decode_numeric_entities(s: &str) -> String {
    if !s.contains("&#") {
        return s.to_string();
    }
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"&#(x?)([0-9A-Fa-f]+);").unwrap());
    re.replace_all(s, |c: &regex::Captures| {
        let radix = if &c[1] == "x" { 16 } else { 10 };
        u32::from_str_radix(&c[2], radix)
            .ok()
            .and_then(char::from_u32)
            .map(|ch| ch.to_string())
            .unwrap_or_else(|| c[0].to_string())
    })
    .into_owned()
}
