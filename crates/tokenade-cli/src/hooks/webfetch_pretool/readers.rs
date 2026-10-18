//! Site-reader glue: the follow-up fetches a reader needs beyond its
//! single `Plan`, and the reader dispatch itself.

use super::*;

/// Distinct embedded databases queried per page. Each is one extra POST, so
/// this bounds the worst case a dashboard-style page can cost us.
pub(crate) const NOTION_MAX_COLLECTIONS: usize = 4;

/// Follow-up fetches for a Notion page: remaining `loadPageChunk` chunks,
/// then one `queryCollection` per embedded database. Every part is appended
/// after [`AUX_SEP`]; the renderer merges them order-independently, so a
/// failed follow-up degrades the render instead of breaking it.
pub(crate) fn notion_drain(url: &str, first: String) -> String {
    use tokenade_core::site_readers as sr;
    const CAP: usize = 4 * 1024 * 1024;
    let mut payload = first;
    if let Some(id) = sr::notion_page_id_for_url(url) {
        payload = format!("{}{}{payload}", sr::notion_root_part(&id), sr::AUX_SEP);
        let chunk_url = format!("{}loadPageChunk", sr::NOTION_API);
        let mut cursor = sr::notion_next_cursor(&payload);
        let mut n = 1;
        while n < sr::NOTION_MAX_CHUNKS {
            let Some(c) = cursor.take() else { break };
            // Pooled: this is a DEPENDENT chain — each cursor comes from the
            // previous response — so it cannot be batched, and every hop would
            // otherwise pay a fresh process and a fresh TLS handshake.
            let Some(bytes) = reader_followup(
                &chunk_url,
                "12",
                CAP,
                Some(&sr::notion_chunk_body(&id, n, &c)),
            ) else {
                break;
            };
            let part = String::from_utf8_lossy(&bytes).into_owned();
            cursor = sr::notion_next_cursor(&part);
            payload.push_str(sr::AUX_SEP);
            payload.push_str(&part);
            n += 1;
        }
    }
    let query_url = format!("{}queryCollection", sr::NOTION_API);
    for q in sr::notion_collection_queries(&payload)
        .iter()
        .take(NOTION_MAX_COLLECTIONS)
    {
        if let Some(bytes) = reader_followup(&query_url, "12", CAP, Some(q)) {
            payload.push_str(sr::AUX_SEP);
            payload.push_str(&String::from_utf8_lossy(&bytes));
        }
    }
    payload
}

/// What a reader produced, plus what it cost to produce — everything a
/// delivery channel needs and nothing about how that channel delivers.
pub(crate) struct Rendered {
    /// Markdown, already scrubbed and carrying its `> source:` and provenance.
    pub md: String,
    /// Tokens of the payload the reader fetched, for the savings assiette.
    pub source_tokens: usize,
    /// Did the reader fetch an HTML PAGE (old.reddit for new-reddit) rather
    /// than an API? Decides which assiette applies — see `try_site_reader`.
    pub html_page: bool,
}

thread_local! {
    /// The refusal a reader just received, and the URL it had been asked for.
    /// Consumed by [`super::provenance_notes`].
    static READER_REFUSAL: std::cell::RefCell<Option<(String, String)>> =
        const { std::cell::RefCell::new(None) };
}

/// The mark heading a refused reader's note. A constant because two layers
/// read it: the one that writes it, and the one that silences the
/// `thin-result` banner diagnosis when it is present.
pub(crate) const REFUSED_MARKER: &str = "[tokenade: reader refused]";

/// **A reader that is set aside does not vanish without a word.**
///
/// MEASURED 2026-09-11: reddit answers **429** after a few calls. The reader
/// then returns `None`, the generic path fetches the page ITSELF — the 8 kB
/// interstitial — and the delivered page says "recovered 0 characters …
/// isolation likely picked the wrong element". That diagnosis is FALSE: our
/// isolation has nothing to do with it, the site refused us the source. The
/// agent reads "this page has no text" where the real answer is "retry in a
/// minute".
///
/// The note travels through the provenance, which is the only place ALL
/// channels go through (batch, single target, hook). It carries the requested
/// URL: a note left by one target cannot end up on the next one.
fn note_reader_refusal(via: &str, url: &str, status: u16, reached: bool) {
    let what = match (reached, status) {
        (false, _) => "could not be fetched at all".to_string(),
        (true, 429) => {
            "answered HTTP 429 (rate limit; the same URL usually works again after a minute)"
                .to_string()
        }
        (true, 401) | (true, 403) => format!("answered HTTP {status} (access refused)"),
        (true, code) => format!("answered HTTP {code}"),
    };
    let note = format!(
        "{REFUSED_MARKER} the dedicated reader for this URL ({via}) {what}, so what \
         follows came from the generic page fetch instead, not from the reader's source.]\n\n"
    );
    READER_REFUSAL.with(|c| *c.borrow_mut() = Some((url.to_string(), note)));
}

/// The same recording, for tests: the normal path goes through a real fetch,
/// and the note must be checkable without network.
#[cfg(test)]
pub(crate) fn note_reader_refusal_for_test(via: &str, url: &str, status: u16, reached: bool) {
    note_reader_refusal(via, url, status, reached);
}

/// The note left by a set-aside reader FOR THIS URL, taken out on the way.
pub(crate) fn take_reader_refusal(requested: &str) -> Option<String> {
    READER_REFUSAL.with(|c| {
        let mut slot = c.borrow_mut();
        match slot.as_ref() {
            Some((url, _)) if url == requested => slot.take().map(|(_, note)| note),
            _ => None,
        }
    })
}

/// Fetch + render a `site_readers::Plan`; `None` sends the caller back to the
/// generic pipeline.
///
/// Split out of `try_site_reader` so `tokenade web` can reach the
/// readers too. It could not before — the fetch, the render and the HOOK's
/// delivery were one function, so `tokenade web https://www.reddit.com/r/rust/`
/// returned 37 characters of SPA shell while the hook, on the same URL, served
/// the whole thread through old.reddit. Seventeen readers, one channel.
pub(crate) fn render_site_reader(
    reader_plan: &tokenade_core::site_readers::Plan,
    url: &str,
    started: std::time::Instant,
) -> Option<Rendered> {
    const READER_DRAIN_CAP: usize = 9 * 1024 * 1024;
    // Ask for the provenance too. A reader fetches its own endpoint, not the
    // page URL, so the redirect comparison is against `fetch_url`: an API that
    // lands on a login host is worth saying, `reddit.com` -> `www.reddit.com`
    // is not. The STATUS is unambiguous either way — a 404 from the API means
    // the thread or repo does not exist, and a reader whose error payload
    // happens to render past `READER_VALID_MIN_CHARS` would otherwise serve it
    // as content.
    let fetched = super::fetch::reader_curl_meta(
        &reader_plan.fetch_url,
        "20",
        READER_DRAIN_CAP,
        reader_plan.post_body.as_deref(),
    );
    let ok = fetched.is_ok();
    // The status the failure knows: a 429 with an empty body carries one.
    let refused_with = fetched.as_ref().err().and_then(|e| e.status());
    // A reader drains at its OWN cap, so it has its own answer to give — 9 MB
    // here against 17 for a page fetch. Taking the page's answer, or a blanket
    // `false`, would say "whole" about a body this cap had cut.
    let (bytes, reader_status, reader_effective, reader_truncated) = match fetched {
        Ok(f) => (f.bytes, f.status, f.effective_url, f.truncated),
        Err(_) => (Vec::new(), refused_with.unwrap_or(200), None, false),
    };
    if !ok || bytes.is_empty() {
        note_reader_refusal(
            reader_plan.via,
            url,
            reader_status,
            ok || refused_with.is_some(),
        );
        tokenade_core::debug_log::log(
            "hookwrap-pretool",
            "webfetch_site_reader_fetch_fail",
            serde_json::json!({
                "url": url,
                "via": reader_plan.via,
                "elapsed_ms": started.elapsed().as_millis() as u64,
            }),
        );
        return None;
    }
    // Decode with the document's OWN charset, like the generic fetch path at
    // mod.rs:504. Most readers hit JSON APIs, which RFC 8259 pins to UTF-8 and
    // where this can never matter — but `FeedDocument`, `Article`,
    // `DiscourseTopic`, `IndeedJobs` and `MasterclassArticle` fetch arbitrary
    // site HTML/XML, and we measured 9 % of Japan's top 1 000 serving
    // Shift-JIS or EUC-JP. `from_utf8_lossy` would hand those to the renderer
    // as U+FFFD soup. Falls back to the old behaviour when `decode` declines.
    let mut body = tokenade_core::text_encoding::decode(&bytes)
        .map(|(text, _enc)| text)
        .unwrap_or_else(|| String::from_utf8_lossy(&bytes).into_owned());
    // Notion streams a page in ~100-block chunks (collapsed toggles only
    // arrive in the later ones) and keeps database rows behind a SECOND
    // endpoint. Drain both before rendering, or a roadmap/wiki page — the
    // dominant public Notion shape — serves as a title and nothing else.
    if matches!(
        reader_plan.kind,
        tokenade_core::site_readers::Kind::NotionPage
    ) {
        body = notion_drain(url, body);
    }
    // Two-fetch readers (Mastodon context, Lemmy comments): fetch the aux
    // resource and append it after the separator so the pure renderer can
    // split them. Best-effort — a failed aux fetch just yields a post with
    // no replies, never blocks the reader.
    if let Some(aux) = &reader_plan.aux_url {
        let aux_body = reader_curl_get(aux, "12", READER_DRAIN_CAP)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        body.push_str(tokenade_core::site_readers::AUX_SEP);
        body.push_str(&aux_body);
    }
    let Some(mut md) = tokenade_core::site_readers::render(&reader_plan.kind, &body) else {
        // An error body that does not render is the same refusal: the site
        // answered, but not the source. A 2xx that does not parse is a broken
        // reader, which the log already says and which is not this.
        if reader_status >= 400 {
            note_reader_refusal(reader_plan.via, url, reader_status, true);
        }
        tokenade_core::debug_log::log(
            "hookwrap-pretool",
            "webfetch_site_reader_parse_fail",
            serde_json::json!({
                "url": url,
                "via": reader_plan.via,
                "body_bytes": body.len(),
                "elapsed_ms": started.elapsed().as_millis() as u64,
            }),
        );
        return None;
    };
    // YouTube: append the transcript (best-effort, third-party frontends) to
    // the metadata+description the pure reader produced. Failure keeps the
    // metadata — never blocks the reader.
    if matches!(reader_plan.kind, tokenade_core::site_readers::Kind::YouTube) {
        if let Some(id) = tokenade_core::site_readers::youtube_video_id(&reader_plan.fetch_url) {
            if let Some(tx) = fetch_youtube_transcript(&id) {
                md.push_str(&format!("\n\n## Transcript\n\n{tx}"));
                tokenade_core::debug_log::log(
                    "hookwrap-pretool",
                    "webfetch_youtube_transcript",
                    serde_json::json!({"video_id": id, "transcript_chars": tx.len()}),
                );
            }
        }
    }
    // A PR's thread is only half of it: append the changed-file list, which
    // lives on a different endpoint. Best-effort — a failed fetch keeps the
    // discussion rather than blocking the reader.
    if matches!(
        reader_plan.kind,
        tokenade_core::site_readers::Kind::GithubIssue
    ) {
        if let Some(files_url) = reader_plan
            .fetch_url
            .rsplit_once("/issues/")
            .filter(|_| url.contains("/pull/"))
            .map(|(base, num)| format!("{base}/pulls/{num}/files?per_page=100"))
        {
            if let Some(bytes) = reader_curl_get(&files_url, "12", READER_DRAIN_CAP) {
                if let Some(table) =
                    tokenade_core::site_readers::github_pr_files(&String::from_utf8_lossy(&bytes))
                {
                    md.push_str(&table);
                }
            }
        }
    }
    // SECURITY: same print-site scrub as every other served channel.
    let md = tokenade_core::redact::redact_for_model(&format!(
        "> source: {via}\n\n{md}",
        via = reader_plan.via
    ));
    // Prepend BEFORE `delivered` is estimated. Round 27 added this note after
    // the measurement, so every non-2xx or redirected reader booked a saving
    // for tokens it then went on to deliver. Small per call, systematically
    // wrong, and accounting integrity is not the place to be approximately
    // right. (`provenance_notes` scrubs itself, so it is already safe here.)
    let md = super::attach_provenance(
        &super::provenance_notes(
            reader_status,
            &reader_plan.fetch_url,
            reader_effective.as_deref(),
            reader_truncated,
        ),
        &md,
    );
    Some(Rendered {
        source_tokens: tokenade_core::tokenizer::estimate(&body),
        html_page: tokenade_core::site_readers::fetches_html_page(&reader_plan.kind),
        md,
    })
}

/// The savings assiette for a reader's output.
///
/// It depends on WHAT the reader fetched. An HTML-page reader (old.reddit for
/// new-reddit) really did compress a page the agent would have carried, so the
/// per-agent web model applies. An API / raw-file reader compressed nothing the
/// agent would ever have seen — crediting the payload would invent a saving, so
/// it books the flat reliability credit instead. See
/// `coefficients::reader_credit_savings`.
///
/// Shared by both channels: two channels computing this independently is how
/// an assiette drifts.
pub(crate) fn reader_savings(r: &Rendered) -> tokenade_core::Savings {
    tokenade_core::web_credit::credit_on_delivery(
        reader_basis(r),
        tokenade_core::tokenizer::estimate(&r.md),
    )
}

/// What a reader's credit is computed from, to price what is finally delivered.
pub(crate) fn reader_basis(r: &Rendered) -> tokenade_core::web_credit::WebBasis {
    use tokenade_core::web_credit::WebBasis;
    if r.html_page {
        WebBasis::Page {
            raw: r.source_tokens,
        }
    } else {
        WebBasis::Flat(tokenade_core::coefficients::reader_credit_savings(
            tokenade_core::tokenizer::estimate(&r.md),
        ))
    }
}

/// Hook channel: render, book, and serve the WebFetch intercept.
pub(crate) fn try_site_reader(
    reader_plan: &tokenade_core::site_readers::Plan,
    url: &str,
    prompt: &str,
    started: std::time::Instant,
) -> Option<i32> {
    let rendered = render_site_reader(reader_plan, url, started)?;
    let sav = reader_savings(&rendered);
    tokenade_core::debug_log::log(
        "hookwrap-pretool",
        "webfetch_site_reader_served",
        serde_json::json!({
            "url": url,
            "via": reader_plan.via,
            "md_tokens": sav.tokens_after,
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
    Some(serve_legacy_intercept(
        url,
        prompt,
        &rendered.md,
        &sav,
        &None,
        started,
        Some(reader_basis(&rendered)),
        // Reader-grade output: it does not go through the hard cap.
        None,
    ))
}
