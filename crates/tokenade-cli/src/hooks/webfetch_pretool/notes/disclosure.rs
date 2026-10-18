//! **What the fold must CONFESS to the agent.**
//!
//! A followed redirect, a non-200 status, a truncated body, a reader that
//! refused: each changes what the agent actually received, and none of them
//! shows in the folded body.
//!
//! Keeping quiet about one of them makes an error page read like an answer.
//! The note is attached to the ADDRESS and removed on read, so that it cannot
//! outlive what it describes.

use super::*;

/// A non-2xx body is an ERROR PAGE, not the page the agent asked for — and the
/// status is known *here* and nowhere downstream, so if it is not said here it
/// is never said at all.
///
/// Measured on 93 real non-200 bodies from the CrUX long tail: the
/// pipeline distilled them into 166–2 149 characters of perfectly ordinary-
/// looking prose with nothing marking them as failures. An agent that fetches a
/// dead URL reads a 404 page and believes it read the article — a wrong answer
/// it has no way to detect. The `refused()` retry above already knows these
/// codes; it just never told anyone.
///
/// Deliberately one line and additive: the body is still served, because a 404
/// page or a wall sometimes carries the only clue about what went wrong.
/// Prefix `text` with the status note when the response was not 2xx.
/// The fetch follows redirects (`curl -L`), so the page the agent receives is
/// not always the page it asked for — and curl reports the landing URL that the
/// hook used to discard.
///
/// Measured on 225 fetches of the CrUX global top 1 000: **26
/// (11.6 %) ended on a different host**. Some are cosmetic (`m.olx.pl` →
/// `www.olx.pl`), but the ones that matter are not: `dzen.ru` lands on
/// `sso.passport.yandex.ru` (an SSO wall) and `a24.app.gree-pf.net` on
/// `/auth/forbidden` — **both with HTTP 200**, so the status note cannot
/// catch them. Domain migrations (`blacktoon413.com` → `blacktoonurl.net`) and
/// TLD moves (`m.vk.com` → `m.vk.ru`) are the same class.
///
/// Reports a host change always, and a path change only when the agent asked
/// for a SPECIFIC document — a homepage redirecting to `/index` or `/fr/` is
/// noise, and suppressing it kept 22 of 48 redirects quiet on the sample.
pub(crate) fn redirect_note(requested: &str, effective: Option<&str>) -> Option<String> {
    let eff = effective?;
    let norm = |u: &str| -> (String, String) {
        let no_scheme = u.split_once("://").map(|(_, r)| r).unwrap_or(u);
        let (host, path) = no_scheme.split_once('/').unwrap_or((no_scheme, ""));
        let host = host.trim_start_matches("www.").to_ascii_lowercase();
        let path = path.split(['?', '#']).next().unwrap_or("");
        let path = path.trim_end_matches('/');
        (host, format!("/{path}"))
    };
    let (rh, rp) = norm(requested);
    let (eh, ep) = norm(eff);
    if rh == eh && (rp == "/" || rp == ep) {
        return None;
    }
    Some(format!(
        "[tokenade: redirected — you requested {requested}, this is {eff}]\n\n"
    ))
}

/// Test-only: production assembles the same note through `Provenance`
/// (the type later replaced the helper call). Kept because the
/// status-note mapping deserves a direct unit test; `cfg(test)` so it stops
/// raising a `dead_code` warning — a spurious one teaches you to ignore the
/// real ones, which is how `detect_any_block` went unwired for so long.
#[cfg(test)]
pub(crate) fn with_status_note(code: u16, text: &str) -> String {
    match http_status_note(code) {
        Some(note) => format!("{note}{text}"),
        None => text.to_string(),
    }
}

/// Both provenance notes, in the order the agent needs them: what came back,
/// then where it came from.
/// The provenance prefix on its own, so callers that assemble their own output
/// (`cmd_web`) get exactly what the hook path gets.
/// May a small response be handed straight to the native fetcher?
///
/// Only when we have nothing to tell the agent about it. Extracted so the
/// decision is testable without a network — the same correction already made
/// to the trailer parser, for the same reason: this is a one-line condition
/// that silently governs whether a third of real failures are ever disclosed.
/// Everything the pipeline knows about a response that the BODY cannot tell the
/// agent: the HTTP status, and whether the fetch silently landed somewhere else.
///
/// **Why this is a type and not a helper call.** Rounds 24-30 added this
/// disclosure and it took seven rounds to actually reach the agent on the
/// default path. Not because any single fix was wrong, but because the string
/// was re-derived from `(http_code, url, effective_url)` at seven sites and had
/// to be taught, one at a time, to every independent decision between it and
/// the output: `redact_for_model`, the savings ledger, a byte gate, a token
/// gate. Each round fixed a real gap and left another.
///
/// Carrying it as ONE value built ONCE fixes the class rather than the
/// instances: a new gate consults `must_reach_agent()`, and `attach()` hands
/// back the token delta so the cost cannot be booked separately from the text
/// (which is exactly what went wrong once).
pub(crate) struct Disclosure {
    prefix: String,
}

/// A body with its disclosure attached, plus what that cost. Returning the two
/// together is the point: an earlier version prefixed the text at five sites and measured
/// it at none.
pub(crate) struct Disclosed {
    pub text: String,
    pub added_tokens: usize,
}

impl Disclosure {
    /// Build once, immediately after the fetch, saying whether the body was
    /// cut at the drain cap.
    ///
    /// There used to be a three-argument `new()` just above, delegating here
    /// with `truncated: false`. The commit that introduced the fourth argument
    /// moved its LAST caller, and nobody picked it up: the release pipeline's
    /// dead-code guard reported it at 1.1.10, not the test suite. `truncated`
    /// stays an explicit argument rather than a default — the paths served from
    /// the cache have no fetch to ask, and making them pass `false` would read
    /// as an answer where it is an absence.
    pub(crate) fn new_bounded(
        code: u16,
        requested: &str,
        effective: Option<&str>,
        truncated: bool,
    ) -> Self {
        Self {
            prefix: provenance_notes(code, requested, effective, truncated),
        }
    }

    /// Fold in what only the BODY can tell us: a CDN/origin error page.
    ///
    /// The status note cannot cover this — Vercel's `DEPLOYMENT_NOT_FOUND`,
    /// S3's XML errors and most SPA error shells come back **200**, so without
    /// this the lead marker announces "✓ FETCH OK" over a gateway error page.
    /// Chained onto `new` at the one construction site so the value is still
    /// built once, in one place, with everything known about the response.
    pub(crate) fn with_body(mut self, body: &str) -> Self {
        if let Some(kind) = tokenade_core::edge_error::detect(body) {
            self.prefix.push_str(&tokenade_core::edge_error::note(kind));
        }
        self
    }

    /// The single predicate every gate consults. A gate that passes a response
    /// through when this is true silently discards what we know about it.
    pub(crate) fn must_reach_agent(&self) -> bool {
        !self.prefix.is_empty()
    }

    /// The lead marker for a channel that serves this response.
    ///
    /// Reads the status out of our own prefix, so a channel whose BODY does not
    /// carry the note (the binary path builds its own banners) still cannot
    /// announce a failed fetch as a success.
    pub(crate) fn lead_marker(&self) -> String {
        serve::webfetch_lead_marker(&self.prefix)
    }

    /// Attach to a body and report the cost in the same breath.
    pub(crate) fn attach(&self, body: &str) -> Disclosed {
        if self.prefix.is_empty() {
            return Disclosed {
                text: body.to_string(),
                added_tokens: 0,
            };
        }
        // Measure the prefix that is ACTUALLY emitted, not the one built at
        // fetch time: the empty-body variant is shorter, and the lesson learned
        // was that a note measured somewhere other than where it is written is
        // a note booked wrong.
        let emitted = provenance_for_body(&self.prefix, body);
        Disclosed {
            text: format!("{emitted}{body}"),
            added_tokens: tokenade_core::tokenizer::estimate(&emitted),
        }
    }
}

pub(crate) fn may_pass_through_small_body(body_len: usize, disc: &Disclosure) -> bool {
    body_len < tokenade_core::dedup_ref::MIN_BYTES_TO_DEDUPE && !disc.must_reach_agent()
}

pub(crate) fn provenance_notes(
    code: u16,
    requested: &str,
    effective: Option<&str>,
    truncated: bool,
) -> String {
    let redirect = redirect_note(requested, effective).unwrap_or_default();
    let status = http_status_note(code).unwrap_or_default();
    let cut = body_truncated_note(truncated).unwrap_or_default();
    // What a discarded READER ran into, if there was one for this address.
    // Without this line, a 429 from the reader reaches the agent disguised as
    // "this page has no text".
    let refused = readers::take_reader_refusal(requested).unwrap_or_default();
    // SECURITY: the landing URL is echoed verbatim, and landing URLs carry
    // credentials — the redirect note's own motivating example was
    // `sso.passport.yandex.ru/push?uuid=590f8225-…`, a session identifier.
    // Signed URLs (`?token=`, `?Signature=`, `X-Amz-Credential`) end up here the
    // same way. Every other served channel scrubs before emitting; this note is
    // built after `redact_for_model` runs on the body, so it has to scrub
    // itself.
    tokenade_core::redact::redact_for_model(&format!("{refused}{redirect}{status}{cut}"))
}

/// What to say when the body hit the drain cap.
///
/// Sibling of [`redirect_note`] and [`http_status_note`], and there for the
/// same reason: the agent cannot see the difference between a page that ended
/// and a page we stopped reading. Measured 2026-09-06 on the published binary,
/// a 24 MB page served whole: the answer was 9 920 bytes carrying the top of
/// the page, no notice of any kind, and a marker on the page's last line was
/// simply absent. A `--grep` for it then answers "no match", which reads as
/// "the page does not contain it" — the false negative this product elsewhere
/// calls worse than having no pointer at all.
pub(crate) fn body_truncated_note(truncated: bool) -> Option<String> {
    truncated.then(|| {
        String::from(
            "[tokenade: the page was longer than tokenade reads in one fetch.              What follows is the BEGINNING of it, not the whole page, and the              recovery pointer holds only what was read. A search that finds              nothing here does not mean the page does not say it.]\n",
        )
    })
}

/// The HTTP status tokenade recorded in the note leading `body`, if any.
///
/// Reads back exactly what [`http_status_note`] writes — parser next to writer,
/// so they cannot drift apart. Deriving this from the SERVED TEXT rather than
/// taking a status parameter is deliberate: the served text is what the agent
/// actually reads, every serve path already carries the note, and the
/// session-cache path has no status code left to pass (only the cached body).
/// A lead marker computed from these same bytes therefore cannot contradict the
/// disclosure sitting right under it, which is the failure this exists to stop.
/// Searched in the leading block rather than at byte 0: [`provenance_notes`]
/// emits the REDIRECT note first when both apply, so a 404 reached through a
/// redirect does not start with the status note. Anchoring on position 0 would
/// have read that case as a success — the exact bug this function exists to
/// prevent, one layer down. The window is bounded so a status note quoted inside
/// page content cannot be mistaken for our own.
///
/// Counted in CHARS, and collected rather than sliced: a byte slice at a fixed
/// offset lands mid-character on any page with a multibyte glyph in its first
/// notes, and `str::get` then returns None — which here would mean "no error
/// status", i.e. announcing a 404 as a success because of an accent.
pub(crate) const STATUS_NOTE_SEARCH_WINDOW: usize = 400;

/// Does the head of `body` carry an edge-error note? Same bounded head window
/// as [`leading_status_code`], for the same reason: the redirect note can come
/// first, and a fixed byte offset would land mid-character.
pub(crate) fn leading_edge_error(body: &str) -> bool {
    body.chars()
        .take(STATUS_NOTE_SEARCH_WINDOW)
        .collect::<String>()
        .contains("[tokenade: EDGE ERROR")
}

pub(crate) fn leading_status_code(body: &str) -> Option<u16> {
    let head: String = body.chars().take(STATUS_NOTE_SEARCH_WINDOW).collect();
    let at = head.find("[tokenade: HTTP ")?;
    let rest = &head[at + "[tokenade: HTTP ".len()..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

pub(crate) fn http_status_note(code: u16) -> Option<String> {
    if (200..300).contains(&code) {
        return None;
    }
    let what = match code {
        404 | 410 => "the page does not exist",
        401 | 403 => "access was refused",
        402 => "payment is required (paywall)",
        429 => "the client was rate-limited",
        451 => "the content is legally blocked",
        400..=499 => "the request was rejected",
        500..=599 => "the server failed",
        300..=399 => "the request was redirected and not followed",
        _ => "the server did not return the page",
    };
    Some(format!(
        "[tokenade: HTTP {code} — {what}. {NOTE_BODY_FOLLOWS}]\n\n"
    ))
}

/// The status note's trailing clause. It is written for the common case — an
/// error page that distils to something — and `attach_provenance` swaps it when
/// nothing follows. Round 16 found yelp.com returning a 403 whose whole output
/// was the banner: 106 bytes announcing an error page that was not there. A
/// disclosure that describes content it did not observe is the same defect as
/// claiming an emptiness we never checked, pointed the other way.
pub(crate) const NOTE_BODY_FOLLOWS: &str =
    "What follows is the ERROR page, not the requested content.";
pub(crate) const NOTE_BODY_EMPTY: &str = "The response body was empty.";

/// Prepend a provenance prefix to a body without letting the prefix promise
/// something the body does not deliver. Every site that assembles provenance +
/// body goes through here, so the two clauses cannot drift apart.
pub(crate) fn attach_provenance(prefix: &str, body: &str) -> String {
    answer_the_status_question(format!("{}{body}", provenance_for_body(prefix, body)))
}

/// Replace the note's one sentence about the HTTP status with the ANSWER.
///
/// `web::pipeline::STATUS_QUESTION` is emitted by core, which does not hold a
/// status. This layer does: every serve path assembles here, and
/// [`leading_status_code`] reads the code back out of the very bytes the agent
/// will read — the same "parser next to writer" rule the lead marker follows,
/// so the two cannot contradict each other.
///
/// Absent a status note the answer is still ours to give, not the reader's to
/// go and find: tokenade emits one for every non-2xx answer, so its absence is
/// the discrimination, and saying so beats sending the agent after a value it
/// has no way to obtain.
pub(crate) fn answer_the_status_question(served: String) -> String {
    let served = defer_to_a_reader_refusal(served);
    if !served.contains(tokenade_core::web::pipeline::STATUS_QUESTION) {
        return served;
    }
    // **A reader refusal already answers the question.** Without this arm, the
    // page carries "the reader ran into a 429" at the top and three lines
    // below "no status note precedes this, so nothing here is a status
    // refusal" — the page contradicts itself, because the status of the
    // generic FETCH (200, the interstitial) is not the one that decided.
    if served.contains(readers::REFUSED_MARKER) {
        return served.replace(
            tokenade_core::web::pipeline::STATUS_QUESTION,
            "The dedicated reader for this URL was refused, stated above — that refusal is \
             what emptied this page, not a status on the fetch that replaced it",
        );
    }
    let answer = match leading_status_code(&served) {
        Some(code) if !(200..300).contains(&code) => format!(
            "The server answered HTTP {code}, stated above — a status-level refusal, not a \
             page whose text arrives later"
        ),
        // Deliberately not "the server answered 2xx": this funnel also serves a
        // local file, where no server answered at all. What the absence of our
        // own note DOES establish is that nothing here is a status-level
        // refusal, which is the half the reader was being sent to find.
        _ => "No status note precedes this, and tokenade emits one for every non-2xx answer, \
              so nothing here is a status-level refusal: an empty shell points to text \
              rendered client-side"
            .to_string(),
    };
    served.replace(tokenade_core::web::pipeline::STATUS_QUESTION, &answer)
}

/// **The `thin-result` banner blames OUR isolation for a failure that comes
/// from the site.**
///
/// The core produces this banner without knowing the status, let alone the
/// refusal a dedicated reader ran into one layer up. When the refusal note is
/// at the top, its last explanation — "isolation likely picked the wrong
/// element" — is false and quotable: an agent reads it as a tokenade defect
/// and gives up on the domain. It is replaced by the true cause, at the exact
/// place where the core wrote it, through the constant the core exposes
/// rather than a copied string.
pub(crate) fn defer_to_a_reader_refusal(served: String) -> String {
    if !served.contains(readers::REFUSED_MARKER) {
        return served;
    }
    served.replace(
        tokenade_core::web::pipeline::ISOLATION_GUESS,
        "the dedicated reader for this URL was refused, stated above, so this is the page the \
         site serves to a session-less fetch and not the reader's source",
    )
}

/// The prefix as it will actually be emitted in front of `body`. Split out so
/// callers can measure exactly what they emit.
pub(crate) fn provenance_for_body<'a>(prefix: &'a str, body: &str) -> std::borrow::Cow<'a, str> {
    if !prefix.is_empty() && body.trim().is_empty() {
        return std::borrow::Cow::Owned(prefix.replace(NOTE_BODY_FOLLOWS, NOTE_BODY_EMPTY));
    }
    std::borrow::Cow::Borrowed(prefix)
}

/// True when `web::distill_text` would have sent this body down the JSON
/// outline path.
///
/// Deliberately mirrors that function's decision verbatim (content-type OR a
/// `{`/`[` first char, AND it actually parses) rather than re-deciding: the
/// recovery pointer must be attached exactly when records were dropped, and
/// never on an HTML body that merely starts with a brace.
/// **Not currently called.** It was written as the mirror described above and
/// the path that would use it took another shape; the doc says what it is FOR,
/// which read as a description of live behaviour. Left in place because the
/// decision it encodes is still the right one if that pointer is ever attached
/// here — but nobody should read the paragraphs above as a statement about what
/// the product does today.
#[allow(dead_code)]
pub(crate) fn body_took_the_json_outline(body: &str, ct: Option<&str>) -> bool {
    let ct_json = ct
        .map(|c| {
            let mime = c
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            mime == "application/json" || mime.ends_with("+json")
        })
        .unwrap_or(false);
    (ct_json || body.trim_start().starts_with(['{', '[']))
        && serde_json::from_str::<serde_json::Value>(body).is_ok()
}

/// What the hook needs from a WebFetch call once every reason to leave it to
/// native has been ruled out. Borrows `parsed` so the rest of the pipeline
/// keeps the `&str` it always had.
pub(crate) struct Intercept<'a> {
    pub(crate) url: &'a str,
    /// The agent's natural-language ask ("find X on this page"); empty when
    /// absent, which downgrades to whole-page serving.
    pub(crate) prompt: String,
    pub(crate) session_id: String,
    pub(crate) mode: WebfetchInterceptMode,
}
