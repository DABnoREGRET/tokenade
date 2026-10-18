//! Thin adapter over [`tokenade_core::web_fetch`], plus the one follow-up
//! fetch that is genuinely CLI-side (the YouTube transcript hunt).
//!
//! The transport itself used to live here AND, separately, inside
//! `handle_webfetch_pretool` — two curl invocations with different headers,
//! different redirect bounds and different provenance. Both are now the one in
//! core; these wrappers exist only so the reader call sites keep their names.

use super::*;

pub(crate) use tokenade_core::web_fetch::Fetched;

/// Best-effort GET returning the response body. `None` on any failure/empty.
/// Shared by the site readers and their follow-ups.
pub(crate) fn reader_curl_get(target: &str, max_time: &str, cap: usize) -> Option<Vec<u8>> {
    reader_curl(target, max_time, cap, None)
}

/// As [`reader_curl_get`], but `post_body` switches the request to a JSON
/// `POST` (Notion's `loadPageChunk` has no GET form).
pub(crate) fn reader_curl(
    target: &str,
    max_time: &str,
    cap: usize,
    post_body: Option<&str>,
) -> Option<Vec<u8>> {
    reader_curl_meta(target, max_time, cap, post_body)
        .ok()
        .map(|f| f.bytes)
}

/// As [`reader_curl`], but keeps the provenance curl already knows.
///
/// Reader endpoints are APIs, so this asks for the one-shot browser profile
/// rather than the polite-then-retry order a page fetch wants: an API has no
/// opinion about user agents and the extra attempt is pure latency.
/// A reader's fetch, **with the reason it failed**.
///
/// `fetch` throws the cause away; a reader that was ruled out must be able to
/// say what the site answered it, otherwise its refusal reaches the agent
/// disguised as an empty page.
pub(crate) fn reader_curl_meta(
    target: &str,
    max_time: &str,
    cap: usize,
    post_body: Option<&str>,
) -> Result<Fetched, tokenade_core::web_fetch::FetchError> {
    tokenade_core::web_fetch::fetch_detailed(
        target,
        &tokenade_core::web_fetch::FetchOpts {
            max_time,
            cap,
            post_body,
            polite_first: false,
        },
    )
}

/// A follow-up in a SEQUENCE of dependent requests to one host — Notion's
/// chunk/collection drain, a PR's file list, a thread's context.
///
/// Reuses the connection and the TLS handshake across the chain. The subprocess
/// transport cannot: every call is a new process and a new handshake. MEASURED:
/// three sequential same-host fetches cost 155 ms as three curl processes
/// against 66 ms over one reused connection, and `notion_drain` makes up to ten
/// of them. They are dependent (each cursor comes from the previous response),
/// so batching them into one curl invocation is not available — only a live
/// connection helps.
pub(crate) fn reader_followup(
    target: &str,
    max_time: &str,
    cap: usize,
    post_body: Option<&str>,
) -> Option<Vec<u8>> {
    tokenade_core::web_fetch::fetch_pooled(
        target,
        &tokenade_core::web_fetch::FetchOpts {
            max_time,
            cap,
            post_body,
            polite_first: false,
        },
    )
    .map(|f| f.bytes)
}

/// YouTube transcript, best-effort, via the public Invidious instances (the
/// `timedtext` endpoint is proof-of-origin-token walled — a no-browser HTTP
/// client can't mint it, so the POT work is offloaded to these public
/// frontends, same pattern as the HN/X readers).
///
/// RE-TESTED 2026-07-20 with a real browser capture (a HAR recorded in a browser),
/// because the signed caption URLs are right there in the watch page and the
/// detour looks unnecessary until you measure it:
///   * the page fetched by plain curl DOES carry `captionTracks[].baseUrl`,
///     already signed (`signature`, `expire`, `sparams`) and with NO `pot`;
///   * fetching that URL returns **HTTP 200 with a zero-byte body** — tried
///     with Referer, with `X-YouTube-Client-Name/Version`, with a cookie jar,
///     on the manual and the ASR track, and in xml / json3 / srv3 formats;
///   * the browser's own request for the same track succeeds (46 KB) and
///     differs by exactly `pot=<token>` + `potc=1`, minted by the player's JS.
/// So the wall is real and the detour stays. Don't re-litigate it without a
/// capture showing a `pot`-free 200 with a body.
///
/// (Also measured, unrelated but easy to trip over: the empty response is NOT
/// the tell for the wall — a caption fetch for a language the video lacks also
/// 200s empty. The first capture asked for `hl=fr` only because Chrome was
/// French, which looked like a block and was not.)
///
/// Tries instances in order:
/// caption-list JSON → English track url → VTT → plain transcript. `None`
/// when every instance is down/rate-limited (caller keeps metadata-only).
/// Opt out with `TOKENADE_YOUTUBE_TRANSCRIPT=0`.
pub(crate) fn fetch_youtube_transcript(video_id: &str) -> Option<String> {
    if matches!(
        std::env::var("TOKENADE_YOUTUBE_TRANSCRIPT").as_deref(),
        Ok("0") | Ok("off")
    ) {
        return None;
    }
    const TRANSCRIPT_CAP: usize = 4 * 1024 * 1024;
    // A caption body from either frontend may be VTT or timedtext XML.
    let parse = |body: &[u8]| -> Option<String> {
        let s = String::from_utf8_lossy(body);
        tokenade_core::site_readers::vtt_to_transcript(&s)
            .or_else(|| tokenade_core::site_readers::timedtext_xml_to_transcript(&s))
    };
    // Family 1: Invidious (list JSON → en track url → caption).
    for inst in tokenade_core::site_readers::YOUTUBE_INVIDIOUS_INSTANCES {
        let list_url = tokenade_core::site_readers::invidious_caption_list_url(inst, video_id);
        let Some(list) = reader_curl_get(&list_url, "8", TRANSCRIPT_CAP) else {
            continue;
        };
        let Some(cap_url) =
            tokenade_core::site_readers::pick_en_caption_url(&String::from_utf8_lossy(&list), inst)
        else {
            continue;
        };
        if let Some(vtt) = reader_curl_get(&cap_url, "10", TRANSCRIPT_CAP) {
            if let Some(text) = parse(&vtt) {
                return Some(text);
            }
        }
    }
    // Family 2: Piped (streams JSON → en subtitle url → caption).
    for inst in tokenade_core::site_readers::YOUTUBE_PIPED_INSTANCES {
        let streams_url = tokenade_core::site_readers::piped_streams_url(inst, video_id);
        let Some(streams) = reader_curl_get(&streams_url, "8", TRANSCRIPT_CAP) else {
            continue;
        };
        let Some(cap_url) = tokenade_core::site_readers::pick_en_piped_caption_url(
            &String::from_utf8_lossy(&streams),
        ) else {
            continue;
        };
        if let Some(cap) = reader_curl_get(&cap_url, "10", TRANSCRIPT_CAP) {
            if let Some(text) = parse(&cap) {
                return Some(text);
            }
        }
    }
    None
}
