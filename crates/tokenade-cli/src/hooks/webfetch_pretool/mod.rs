//! Entry point of the WebFetch PreToolUse path: fetch, classify, and decide
//! how the result reaches the agent. Split into `fetch` (transport),
//! `readers` (site-reader glue), `serve` (gate + delivery), `cache` (session
//! index) and `recovery` (PostToolUse net).

#[path = "notes/disclosure.rs"]
mod disclosure;
pub(crate) use disclosure::*;

mod binary;
mod cache;
pub(crate) mod fetch;
pub(crate) mod readers;
mod recovery;
pub(crate) mod serve;
use super::*;
use crate::*;
pub(crate) use binary::*;
pub(crate) use cache::*;
pub(crate) use fetch::*;
pub(crate) use readers::*;
pub(crate) use recovery::*;
pub(crate) use serve::*;

/// Decide whether this WebFetch is ours. `Err(rc)` means hand it back to
/// Claude Code untouched and exit with `rc` — every guard here is a case where
/// intercepting is not measurably better than native, or not ours to make.
fn intercept_or_passthrough(parsed: &serde_json::Value) -> Result<Intercept<'_>, i32> {
    if hook_disabled_by_env() {
        return Err(0);
    }
    if tokenade_core::env_compat::is_truthy_one("TOKENADE_NO_WEBFETCH") {
        return Err(0);
    }
    // #4 (0.4.4 findings) — FAIL-OPEN when not activated. Mirrors the guards
    // in `handle_read_pretool` / `handle_bash_pretool`: with the MCP recovery
    // tools (`expand_ref`, `search_stash`, `tokenade docs`) gated off, fetching
    // + folding/stashing the page here would hand the agent a compacted view
    // (or a binary stash pointer) with NO way to recover the dropped bytes —
    // strictly worse than nothing, since Claude Code's native WebFetch would
    // otherwise return the full content. Pass the WebFetch through untouched.
    let url = parsed
        .get("tool_input")
        .and_then(|i| i.get("url"))
        .and_then(|u| u.as_str())
        .unwrap_or("");
    if url.is_empty() {
        return Err(0);
    }
    let cwd = parsed
        .get("cwd")
        .and_then(|c| c.as_str())
        .map(std::path::Path::new);
    if crate::hooks::permission_rules::a_user_rule_may_guard_url(url, cwd) {
        return Err(0);
    }
    // #955 (webfetch net-negative fix) — the WebFetch tool_input carries
    // the agent's natural-language `prompt` ("find X on this page"). We use
    // it to serve a prompt-scoped extract INLINE instead of stashing the
    // whole distilled page behind an outline (the old net-negative: the
    // answer wasn't in the served bytes, forcing a second grep-the-stash
    // turn). Empty/absent prompt → falls back to whole-page serving.
    let prompt = parsed
        .get("tool_input")
        .and_then(|i| i.get("prompt"))
        .and_then(|p| p.as_str())
        .unwrap_or("")
        .to_string();
    // Sanity guard on the URL scheme — only http/https. Anything else
    // (file://, data://, etc.) we leave to Claude Code.
    let lc = url.to_lowercase();
    if !(lc.starts_with("http://") || lc.starts_with("https://")) {
        return Err(0);
    }
    // SSRF guard. The native WebFetch runs on Anthropic infra and cannot reach
    // the user's LAN / loopback / cloud-metadata endpoints; WE run on the user's
    // machine and would. Intercepting `http://169.254.169.254/…`,
    // `http://localhost:…` or a private-range IP would turn a public web tool
    // into a reach into the user's own network — a scope native does not have.
    // Leave every internal target to native (never worse than native).
    if tokenade_core::url::is_internal_url(url) {
        return Err(0);
    }
    // #955 v2 — SELECTIVE interception. Native WebFetch runs url+prompt
    // through a server-side small model and returns a scoped ~200-token
    // summary that usually CONTAINS the answer, in one turn. On a normal
    // HTML page we can never beat that — so we only intercept where the
    // native path measurably fails:
    //   * PDF/binary URLs — the native model receives raw FlateDecode
    //     bytes and answers "unable to answer" (MEASURED);
    //   * huge pages — the native fetch truncates (~100 KB) before the
    //     answer ("cutting off at the letter 'e'" on
    //     kernel-parameters.html, MEASURED) while our prompt-scoped
    //     extract still reaches it.
    // `TOKENADE_WEBFETCH_INTERCEPT`: unset → selective gate (default);
    // `1`/`legacy` → legacy always-intercept (kill-switch / bench);
    // `0` → never intercept anything (same effect as TOKENADE_NO_WEBFETCH).
    let mode = webfetch_intercept_mode();
    if matches!(mode, WebfetchInterceptMode::Off) {
        return Err(0);
    }
    let session_id = crate::hooks::session_id_of(parsed)
        .unwrap_or("")
        .to_string();
    Ok(Intercept {
        url,
        prompt,
        session_id,
        mode,
    })
}

/// PreToolUse handler for `WebFetch`. Substitutes the model-visible
/// result with a `web::html_to_markdown`-compacted version of the
/// page bytes we fetched ourselves.
///
/// Why fetch ourselves: Claude Code's `WebFetch` returns the page as
/// text. PostToolUse hooks can only ADD context (the raw response
/// stays in the model's input), so the only way to SUBSTITUTE the
/// bytes is `PreToolUse` + `permissionDecision: "deny"` + a
/// `permissionDecisionReason` that contains the compacted markdown.
/// The model treats the reason as the tool result.
///
/// Bypass:
///   - `TOKENADE_NO_WEBFETCH=1` skips this hook entirely (per-call).
///   - `TOKENADE_HOOK_DISABLED=1` (or its `STS_` alias) skips all hooks.
///   - Non-200 responses fall through to passthrough so Claude Code's
///     own error path runs.
///   - Tiny responses (<MIN_BYTES_TO_DEDUPE) fall through — the
///     deny-as-Error UI cost would exceed any savings.
pub(crate) fn handle_webfetch_pretool(
    parsed: &serde_json::Value,
    started: std::time::Instant,
) -> i32 {
    let Intercept {
        url,
        prompt,
        session_id,
        mode,
    } = match intercept_or_passthrough(parsed) {
        Ok(req) => req,
        Err(rc) => return rc,
    };
    // Case 5 — session re-fetch cache: if WE already fetched + stashed
    // this URL in this session, decide from the stash (no curl). A cache
    // hit that gates to "intercept" saves the fetch AND the native
    // summarizer pass; a cache hit that gates to "passthrough" just
    // avoids our redundant curl.
    if matches!(mode, WebfetchInterceptMode::Selective) && !session_id.is_empty() {
        if let Some(cached_md) = webfetch_cache_lookup(&session_id, url) {
            return serve_selective_gate(
                url,
                &prompt,
                &cached_md,
                None,
                &session_id,
                true,
                started,
                false,
                false,
                // Comes from the session cache: no distillation in this turn.
                None,
            );
        }
    }

    // Site readers — URLs where the NATIVE fetcher measurably fails (Reddit's
    // JS shell walls non-browser fetchers; crates.io is a pure SPA): fetch
    // the alternate representation (old.reddit HTML / registry API) and serve
    // its rendered markdown, always — for these hosts the reader output is
    // strictly better than what the native path would see. Fail-open: any
    // fetch or parse failure (layout drift, API change) falls through to the
    // generic pipeline on the original URL.
    if let Some(reader_plan) = tokenade_core::site_readers::plan(url) {
        if let Some(rc) = try_site_reader(&reader_plan, url, &prompt, started) {
            return rc;
        }
    }

    // The fetch. Transport lives in `tokenade_core::web_fetch` — one curl
    // invocation for the whole product, so a header, a redirect bound or a
    // proxy setting cannot be present on one channel and absent on the other.
    // It was: this function had its own curl with no proxy support and no
    // `--max-redirs`, both of which `tokenade web` had had for rounds.
    //
    // `page()` asks for the polite order — the honest `tokenade/x.y` UA first,
    // the browser profile only on a refusal. That ordering is what makes
    // `native_blocked` mean something, and it is why a bot wall has to come
    // back as a STATUS rather than an error: curl exits 0 on an HTTP 403, so
    // for a long time the retry that exists precisely for "some CDNs 403 tool
    // UAs" could not fire on a 403 — we distilled the Access-Denied page and
    // served it as the article. MEASURED, same machine, same headers:
    // britannica.com/animal/lion and masterclass.com/articles/… answer 403 +
    // ~66 words to the honest UA and 200 + 1712 / 672 words to the browser
    // profile.
    const CURL_DRAIN_CAP: usize = 17 * 1024 * 1024;
    // D13: this native WebFetch is one single-target web-call; count it toward
    // the batch nudge (shared with the CLI channel).
    crate::compactors::nudge_sequential_web_calls("hook-fetch", "fetch", false);
    let Some(fetched) = tokenade_core::web_fetch::fetch(
        url,
        &tokenade_core::web_fetch::FetchOpts::page("25", CURL_DRAIN_CAP),
    ) else {
        tokenade_core::debug_log::log(
            "hookwrap-pretool",
            "webfetch_passthrough_curl_fail",
            serde_json::json!({
                "url": url,
                "elapsed_ms": started.elapsed().as_millis() as u64,
            }),
        );
        return 0;
    };
    // Did the browser profile get us in where the plain client was refused?
    // Then the NATIVE fetcher — a plain client too, observed taking the same
    // 403 on these hosts — cannot read this page either, and the selective
    // gate must not hand it back.
    let native_blocked = fetched.native_blocked;
    let content_type = fetched.content_type.clone();
    let body_bytes = fetched.bytes;
    // Built ONCE, here, while the response is still in hand. Every gate and
    // every serve path below consults this value instead of re-deriving it.
    let disclosure = Disclosure::new_bounded(
        fetched.status,
        url,
        fetched.effective_url.as_deref(),
        fetched.truncated,
    )
    .with_body(&String::from_utf8_lossy(&body_bytes));
    // L69 — binary short-circuit: server returned a non-text type
    // (image, archive, pdf, …). Skip html_to_markdown entirely —
    // it'd produce garbage on binary bytes — and emit a metadata
    // stash with the actual binary content preserved.
    //
    // #947 — Content-Type override: when the server says it's HTML or
    // plain text, trust that signal over the bytewise 5%-control-char
    // heuristic. Modern HTML (inline SVG, base64 fonts, minified JS)
    // easily trips the 5% threshold on the first 4 KB, which used to
    // route legitimate text/html responses (e.g. arxiv abstract pages)
    // to the binary stash path → agent then had to fall back to
    // WebSearch instead of getting the page content. html_to_markdown
    // handles noisy bytes correctly; let it run.
    // One classifier for the whole product — see `web_fetch::classify_body`.
    // This decision used to live here only, which is how `tokenade web` came to
    // stash arxiv abstract pages as binary while the hook read them fine.
    let ct_normalized =
        tokenade_core::text_encoding::normalized_content_type(content_type.as_deref());
    let body_kind = tokenade_core::web_fetch::classify_body(&body_bytes, content_type.as_deref());
    let detected_kind = body_kind.detected.clone();
    if body_kind.binary {
        return binary::serve(BinaryResponse {
            url,
            prompt: prompt.clone(),
            bytes: &body_bytes,
            content_type: content_type.clone(),
            normalized_type: ct_normalized,
            detected_kind,
            disclosure: &disclosure,
            started,
        });
    }
    // Standard text path: decode with the page's OWN charset. `from_utf8_lossy`
    // is binary-SAFE — which is all the previous comment here claimed — but it
    // is not charset-CORRECT: a Shift-JIS, GBK, EUC-KR or windows-1251 page is
    // not a binary blob, and lossy-decoding it replaces its text with U+FFFD.
    // `cmd_web` already learned this the hard way (see the note in
    // compactors.rs); this path — the one Claude Code actually uses — never did,
    // so every non-UTF-8 page reached BOTH the generic extractor and the JS
    // engine as mojibake. Fail open to the old behaviour if detection declines.
    let body = tokenade_core::text_encoding::decode(&body_bytes)
        .map(|(s, _enc)| s)
        .unwrap_or_else(|| String::from_utf8_lossy(&body_bytes).to_string());
    // The size passthrough is a TOKEN decision, and it was silently making a
    // CORRECTNESS one. Error pages are small: of 93 real non-200 bodies from the
    // CrUX long tail, **30 are under this 1500-byte threshold** — plain nginx
    // 403s are 134-551 bytes. Everything downstream of here is what tells the
    // agent something went wrong: the HTTP-status note, the redirect
    // note AND the anti-bot banner that took five rounds of
    // tuning, which `html_to_markdown` only adds at `distill_text`
    // below. Returning early meant a third of real failures — and every bot
    // wall small enough to be a bot wall — reached the agent as bare text.
    //
    // "Claude Code's own fetch is fine" holds for a small SUCCESSFUL page. It
    // does not hold for a page we already know is a 404, a wall, or a landing
    // somewhere else.
    if may_pass_through_small_body(body.len(), &disclosure) {
        // Too small — the dedup UI cost exceeds savings, and the
        // page is small enough that Claude Code's own fetch is fine.
        return 0;
    }

    // The content pipeline. Everything about WHAT this page says lives in
    // `tokenade_core::web_pipeline` so that `tokenade web` cannot drift from
    // it again — it had, by five capabilities, every one of them silently.
    //
    // What stays HERE is delivery: which savings assiette applies, and whether
    // the output goes through the selective gate or straight to the agent.
    // Those are genuinely this channel's business.
    let d = tokenade_core::web_pipeline::distill_page(&body, url, content_type.as_deref());
    let mut sav = d.savings;
    // Taken BEFORE `d.md` moves out: it is the page before clipping, and it is
    // the one the reserve must hold.
    let full_md = d.full_md;
    let compact = d.md;

    // Reader-grade output — a rendered feed, prepended product facts — has
    // ALREADY beaten what the native fetcher would have seen, so it is served
    // rather than re-judged. The selective gate measures "is this ordinary HTML
    // worth intercepting", and it declined a correctly-rendered 30-entry Atom
    // feed (MEASURED on simonwillison.net) by answering the wrong question. A
    // price is the same category: it sits in metadata the native fetcher drops
    // as chrome, so on a product page we hold a fact it structurally cannot
    // produce.
    if d.source.is_reader_grade() {
        // Prefix BEFORE the credit is computed: an earlier version measured the body and
        // then delivered body+note, booking a saving for tokens it went on to
        // spend.
        let served = disclosure.attach(&compact).text;
        let basis = match d.source {
            // Facts merged into a real extraction: the page WAS compacted, so
            // the scoped web credit applies.
            tokenade_core::web_pipeline::Source::ProductFacts => {
                tokenade_core::web_credit::WebBasis::Excerpt {
                    page: sav.tokens_after,
                    floor: WEBFETCH_SCOPED_TURN_CREDIT_TOKENS,
                }
            }
            // A feed render compacted nothing the agent would otherwise have
            // carried, so it books the flat reliability credit — same rule as
            // `readers::reader_savings`.
            _ => tokenade_core::web_credit::WebBasis::Flat(
                tokenade_core::coefficients::reader_credit_savings(
                    tokenade_core::tokenizer::estimate(&served),
                ),
            ),
        };
        let credited = tokenade_core::web_credit::credit_on_delivery(
            basis,
            tokenade_core::tokenizer::estimate(&served),
        );
        tokenade_core::debug_log::log(
            "hookwrap-pretool",
            "webfetch_reader_grade",
            serde_json::json!({
                "url": url,
                "source": format!("{:?}", d.source),
                "md_tokens": credited.tokens_after,
            }),
        );
        return serve_legacy_intercept(
            url,
            &prompt,
            &served,
            &credited,
            &content_type,
            started,
            Some(basis),
            // Reader-grade: this path comes back from the distiller without
            // going through the hard cap.
            None,
        );
    }
    let pct = sav.percent() as u64;
    // #892 — SECURITY: web pages compacted to markdown frequently
    // include code blocks with sample auth headers ("`Authorization:
    // Bearer eyJ…`" in API docs), `?api_key=…` in tutorial URLs, etc.
    // Apply redact before embedding in the reason — disclose only
    // stashes when the body is large; the inline path was untouched.
    let compact = tokenade_core::redact::redact_for_model(&compact);
    // This path's `sav` was computed far above (distill, then the hard cap,
    // then the JS render), so the prefix cannot simply move ahead of it —
    // routing the note through `distill_text` would strip it. Add its cost to
    // the ledger instead, which is the same correction the other three sites
    // get by ordering: deliver N tokens, book N tokens.
    let disclosed = disclosure.attach(&compact);
    if disclosed.added_tokens > 0 {
        sav = tokenade_core::Savings::new(
            sav.tokens_before,
            sav.tokens_after + disclosed.added_tokens,
        );
    }
    let compact = disclosed.text;
    match mode {
        WebfetchInterceptMode::Off => 0, // handled earlier; unreachable
        WebfetchInterceptMode::Selective => {
            // Decision table (2026-07): HTML → passthrough by default;
            // intercept ONLY the proven-win case (huge page + prompt with
            // a lexical anchor). Everything else is the native model's
            // game and it wins it (MEASURED: ~200-token scoped summary
            // containing the answer, 1 turn).
            serve_selective_gate(
                url,
                &prompt,
                &compact,
                Some(&sav),
                &session_id,
                false,
                started,
                native_blocked,
                disclosure.must_reach_agent(),
                full_md.as_deref(),
            )
        }
        WebfetchInterceptMode::Legacy => {
            // Kill-switch/bench path: the pre-#955v2 always-intercept
            // behaviour (inline-whole ≤8k / scoped / outline+stash),
            // including its pct guard and web-credit accounting.
            if pct < 15 {
                tokenade_core::debug_log::log(
                    "hookwrap-pretool",
                    "webfetch_passthrough_low_gain",
                    serde_json::json!({
                        "url": url,
                        "before": sav.tokens_before,
                        "after": sav.tokens_after,
                        "pct": pct,
                        "elapsed_ms": started.elapsed().as_millis() as u64,
                    }),
                );
                return 0;
            }
            serve_legacy_intercept(
                url,
                &prompt,
                &compact,
                &sav,
                &content_type,
                started,
                None,
                full_md.as_deref(),
            )
        }
    }
}

#[cfg(test)]
mod holistic_tests {
    use super::holistic_intent;

    #[test]
    fn detects_summary_and_transcript_asks_en_fr() {
        for p in [
            "summarize this video",
            "give me a summary of the article",
            "résume cette page",
            "tl;dr of this thread",
            "what is this page about",
            "get the full transcript",
            "transcription complète de la vidéo",
            "walk me through this doc",
            "key points of the discussion",
        ] {
            assert!(holistic_intent(p), "should be holistic: {p:?}");
        }
    }

    #[test]
    fn ignores_specific_fact_lookups() {
        for p in [
            "what is the default keepalive_timeout value",
            "which version added require esm",
            "how do I install express",
            "what BLEU score does the model reach",
        ] {
            assert!(!holistic_intent(p), "should NOT be holistic: {p:?}");
        }
    }
}
