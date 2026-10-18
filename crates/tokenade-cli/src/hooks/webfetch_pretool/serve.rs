//! Serving layer: the selective gate's decision, and the two ways a
//! result reaches the agent (scoped extract, or whole distilled page).

use super::*;

/// Interception policy for the WebFetch hook (#955 v2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WebfetchInterceptMode {
    /// Default — intercept only where native WebFetch measurably fails
    /// (PDF/binary; huge page + anchored prompt). Normal HTML passes
    /// through to the native scoped summarizer.
    Selective,
    /// `TOKENADE_WEBFETCH_INTERCEPT=1|legacy` — always intercept HTML
    /// (pre-v2 behaviour). Kept as kill-switch + A/B bench arm.
    Legacy,
    /// `TOKENADE_WEBFETCH_INTERCEPT=0` — never intercept.
    Off,
}

pub(crate) fn webfetch_intercept_mode() -> WebfetchInterceptMode {
    match std::env::var("TOKENADE_WEBFETCH_INTERCEPT") {
        Ok(v) => match v.trim() {
            "0" | "off" | "never" => WebfetchInterceptMode::Off,
            "1" | "legacy" | "always" => WebfetchInterceptMode::Legacy,
            _ => WebfetchInterceptMode::Selective,
        },
        Err(_) => WebfetchInterceptMode::Selective,
    }
}

/// Minimum distilled-markdown size (tokens) for an HTML intercept in
/// selective mode. `0` = no size gate: every anchored page is intercepted.
///
/// Was `12_000` until 2026-08-01. That number was a PROXY for "the answer may
/// sit beyond the native fetch truncation (~100 KB)" and was never measured in
/// the unit that decides: money. Counting delivered tokens hides that the
/// native path pays **Haiku** to read the whole page and then pays the main
/// model for its summary, where we pay one main-model input for a local
/// extract. Measured in dollars over 181 pages with a paired bootstrap
/// (`tokenade-core/tests/webfetch_gate_bench.rs`), at $5/M main input and a
/// 10-turn carry term, intercepting EVERY anchored page is **-29.1 %
/// [-41.0..-14.6], P(cheaper)=1.000** against never intercepting.
///
/// KNOWN RISK, accepted by the PO 2026-08-01: this is the one setting that is
/// not robust across assumptions. Under the pessimistic floor (assume the
/// native path somehow reads an already-distilled page) it is **+5.2 %
/// [-5.4..+18.2], P(cheaper)=0.178**, where every threshold in 3k..12k wins
/// under all three assumptions. If field cost regresses, restore a gate with
/// `TOKENADE_WEBFETCH_INTERCEPT_MIN_TOKENS=4000` (-17.8 % credible / -12.9 %
/// floor, P=1.000 in both) rather than going back to 12_000.
pub(crate) const WEBFETCH_INTERCEPT_MIN_TOKENS_DEFAULT: usize = 0;

pub(crate) fn webfetch_intercept_min_tokens() -> usize {
    std::env::var("TOKENADE_WEBFETCH_INTERCEPT_MIN_TOKENS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(WEBFETCH_INTERCEPT_MIN_TOKENS_DEFAULT)
}

/// FLOOR credit (tokens) for a scoped intercept: on a small page the scoping
/// delta is tiny, so we still credit at least one avoided turn (the native
/// path burned 3 turns and still failed in the measured big-needle run). The
/// primary credit is the real scoping delta — full distilled page → scoped
/// excerpt (see `serve_selective_gate`); this constant only floors it so a
/// small page never regresses below the old flat credit. Bounded by the full
/// distilled page (never the raw HTML), so it can't book bytes the native
/// model would never have billed the agent for.
pub(crate) const WEBFETCH_SCOPED_TURN_CREDIT_TOKENS: usize = 800;

/// Tokens of prose the SCOPED delivery carries and the whole-page delivery
/// does not: the "this page is too large for the native fetch / below are the
/// passages" paragraph, the `expand-ref` recovery line, and the two excerpt
/// fences. MEASURED 2026-09-05 on the two `reason` shapes below with their
/// bodies emptied: 820 chars scoped against 209 whole, a delta of ~152
/// tokens. Held at 150 so the constant can only UNDER-state the overhead,
/// which errs towards keeping the scoped path.
///
/// Why it exists: `serve_legacy_intercept` has always asked "is this page
/// small enough that serving it whole beats an extract?" ([`webfetch_inline_cap`]),
/// and the selective gate M-bM-^@M-^T the DEFAULT path M-bM-^@M-^T never asked. Measured on the
/// local transcript corpus, 34 of 450 scoped deliveries distilled to under
/// 500 tokens and the smallest to 122: tokenade withheld a page smaller than
/// the note explaining where the page went, and the agent spent a turn on
/// `expand-ref` to read something that would have fitted inline.
pub(crate) const WEBFETCH_SCOPING_OVERHEAD_TOKENS: usize = 150;

/// True when withholding the page behind a scoped excerpt actually pays: the
/// distilled page must be bigger than the excerpt PLUS the machinery that
/// explains the excerpt away ([`WEBFETCH_SCOPING_OVERHEAD_TOKENS`]). A page
/// too big to serve inline is always scoped, whatever this says M-bM-^@M-^T the
/// alternative there is not "serve whole", it is "overflow the context".
pub(crate) fn scoping_pays(compact_tokens: usize, extract: &str) -> bool {
    if compact_tokens > webfetch_inline_cap() {
        return true;
    }
    compact_tokens > tokenade_core::tokenizer::estimate(extract) + WEBFETCH_SCOPING_OVERHEAD_TOKENS
}

/// The pure decision of the selective HTML gate — split from the
/// serving/IO so unit tests can assert the decision table directly.
#[derive(Debug)]
pub(crate) enum SelectiveDecision {
    /// Native WebFetch wins — do not intercept. Payload = debug-log op
    /// naming WHY ("webfetch_passthrough_native_better" = under the
    /// size threshold; "webfetch_passthrough_no_anchor" = big page but
    /// the prompt has no lexical foothold).
    Passthrough(&'static str),
    /// Huge page + anchored prompt: serve this scoped excerpt inline.
    Intercept { extract: String },
    /// The native fetcher is WALLED on this host (we took a 403 with a plain
    /// client and only got in with the browser profile) and the prompt gave
    /// no lexical anchor to scope by. Passing through would hand the agent
    /// the bot wall; serve the whole distilled page instead.
    InterceptWhole,
}

/// Decision table (PO 2026-08-01): intercept ⇔ the prompt anchors lexically.
/// The size gate is OFF by default — [`webfetch_intercept_min_tokens`] is `0`,
/// so `compact_tokens < min` is never true and the branch below only fires when
/// an operator restores a threshold via the env knob. The anchor requirement is
/// untouched: paraphrasing a page with no lexical foothold is still the small
/// model's game, and that passthrough leg is unchanged.
///
/// `native_blocked` overrides the size gate. That gate encodes "the native
/// model wins the small-page case" — true only when the native fetcher can
/// FETCH the page. On a host that 403s plain clients it returns an error, so
/// every passthrough there is a lost turn no matter how small the page is.
pub(crate) fn selective_gate_decision(
    prompt: &str,
    compact: &str,
    // The page BEFORE the hard cap, when the cap bit: what the question is
    // asked of (see `searched_page`).
    full_md: Option<&str>,
    compact_tokens: usize,
    native_blocked: bool,
    must_disclose: bool,
) -> SelectiveDecision {
    // `must_disclose` = we know something about this response the agent cannot
    // learn from the body: a non-2xx status, a silent landing on another host,
    // or an anti-bot banner. Passing through hands the page to the native
    // fetcher and every one of those facts is lost.
    //
    // Round 29 stopped the SMALL-body passthrough for exactly this reason and
    // was incomplete: a 551-byte 403 got past that gate, distilled to a handful
    // of tokens, and then fell straight through *this* one on
    // `compact_tokens < min_tokens`. Two gates, one on size and one on token
    // count, both making a correctness decision as a side effect.
    if must_disclose {
        return SelectiveDecision::InterceptWhole;
    }
    if !native_blocked && compact_tokens < webfetch_intercept_min_tokens() {
        return SelectiveDecision::Passthrough("webfetch_passthrough_native_better");
    }
    let scoped = if prompt.trim().is_empty() {
        None
    } else {
        // Adaptive budget: sized from the distilled page so a page just
        // over the gate serves an extract in the same size class as a
        // native WebFetch summary, not 3× larger.
        tokenade_core::rag::prompt_scoped_extract_dispatch(
            &searched_page(full_md, compact),
            prompt,
            tokenade_core::disclose::webfetch_scoped_budget_for(compact_tokens),
        )
    };
    match scoped {
        Some(extract) => SelectiveDecision::Intercept { extract },
        None if native_blocked => SelectiveDecision::InterceptWhole,
        None => SelectiveDecision::Passthrough("webfetch_passthrough_no_anchor"),
    }
}

/// Name the hosts whose pages are JS shells we hand to the native fetcher
/// anyway. Fires only on the passthrough leg — an intercepted page was
/// readable by definition — and only when we have real before/after numbers
/// (a session-cache hit has no original HTML size to compare against).
///
/// The event is the reader backlog, ranked by how often users actually hit
/// each site. Only the registrable DOMAIN is recorded — never the subdomain,
/// never the path. A workspace slug (`acme.atlassian.net`) names the user's
/// employer; `atlassian.net` names the site we can't read, which is the whole
/// signal. This is the ONLY hostname-derived value tokenade sends anywhere.
pub(crate) fn record_spa_shell_if_empty(
    url: &str,
    compact: &str,
    sav: Option<&tokenade_core::Savings>,
    compact_tokens: usize,
) {
    let Some(sav) = sav else { return };
    if !tokenade_core::web::looks_like_spa_shell(compact, sav.tokens_before, compact_tokens) {
        return;
    }
    // Domain + one path segment: which SITE could not be read, and which
    // SHAPE of page on it. The domain alone left these rows unactionable.
    let domain = tokenade_core::url::domain_and_section(url);
    let has_reader = tokenade_core::site_readers::plan(url).is_some();
    // `/` not `:` — the ledger encodes nudges as `nudge:<kind>:<event>`, so a
    // colon in the kind would split into a fourth segment server-side.
    tokenade_core::gain::record_nudge(
        &format!("webfetch-spa-shell/{domain}"),
        if has_reader { "covered" } else { "fire" },
        tokenade_core::gain::NudgeMetric::Tokens(compact_tokens),
    );
    tokenade_core::debug_log::log(
        "hookwrap-pretool",
        "webfetch_spa_shell",
        serde_json::json!({
            "domain": domain,
            "html_tokens": sav.tokens_before,
            "md_tokens": compact_tokens,
            "has_reader": has_reader,
        }),
    );
}

/// Success marker prepended to every deny-reason so the <error> wrapper
/// Claude Code adds around a PreToolUse deny doesn't read as a failed
/// fetch + retry (PreToolUse has no "success" output channel).
pub(crate) const WEBFETCH_SUCCESS_MARKER: &str = "✓ FETCH OK — tokenade fetched this page \
(native WebFetch intentionally skipped). The content below IS the page; this is a SUCCESS, \
not a failure — do NOT retry.";

/// The lead marker for a served fetch, decided by what the SERVER answered.
///
/// The success marker above exists because PreToolUse has no success channel:
/// we deny to substitute the body, and Claude Code wraps a deny in `<error>`.
/// But it was prepended UNCONDITIONALLY, so a fetch the server answered with
/// 404 was served as `✓ FETCH OK … this is a SUCCESS, not a failure — do NOT
/// retry`, directly above our own `[tokenade: HTTP 404 — the page does not
/// exist]`. The agent is told, in one message, that nothing is wrong and that
/// the page does not exist. On a real error the `<error>` wrapper is CORRECT and
/// the marker must not talk the agent out of it.
///
/// Anything non-2xx counts, which is the same line [`http_status_note`] already
/// draws: the server did not return the requested page. A 200 that distilled to
/// little is NOT included — thin output has honest causes and we are not certain
/// it is an error, so it keeps the success marker and its own `[thin-result]`
/// explanation.
///
/// The retry advice is status-aware because "do NOT retry" is wrong for half of
/// these: 429 and 5xx are the cases where retrying is exactly right.
pub(crate) fn webfetch_lead_marker(body: &str) -> String {
    let Some(code) = super::leading_status_code(body) else {
        // A CDN/origin error page is frequently served 200, so there is no
        // status to read — the body note is the only evidence, and announcing
        // success over it is exactly the failure this marker exists to stop.
        if super::leading_edge_error(body) {
            return "✗ FETCH FAILED — the edge returned an error page instead of the content. \
                    tokenade did reach the URL and the HTTP status may even be 200, but what \
                    follows is the CDN's or origin's error page, not the page you asked for. \
                    See the EDGE ERROR note below for whether a retry can help."
                .to_string();
        }
        return WEBFETCH_SUCCESS_MARKER.to_string();
    };
    let advice = match code {
        408 | 429 | 500..=599 => "This is transient or server-side — a later retry may succeed.",
        401..=403 => {
            "Retrying as-is will not help: it needs credentials, payment, or another route."
        }
        300..=399 => {
            "The redirect was not followed — fetch the target URL directly if you need it."
        }
        _ => "Retrying the same URL will not help.",
    };
    format!(
        "✗ FETCH FAILED — the server answered HTTP {code}. tokenade did reach the URL, so this \
         is the SERVER's answer and not a tokenade failure, but it IS a real error: what \
         follows is not the content you asked for. {advice}"
    )
}

/// Recover a withheld distilled page without advertising `--raw`.
///
/// `--raw` is NOT here. `disclose::recovery_menu` already stopped promoting
/// it (107/111 dumps nus ; round/2026-09-23). Pasting `{RAW_ADVICE}` on
/// both webfetch recover arms (selective + legacy) sent the model back to
/// the dump. It stays in `--help` and the `--prompt` miss refusal.
pub(crate) fn webfetch_recover_advice(hash: &str, tokens: usize, path: &std::path::Path) -> String {
    format!(
        "Full distilled page ({tokens} tokens) NOT shown to save tokens — ask it \
         what you need: `tokenade expand-ref {hash} --prompt \"q1, q2\"` (several questions \
         at once, each answered under its own heading), `--grep <pattern>` for exact lines \
         (repeatable), or Read {}.",
        path.display(),
    )
}

/// #955 v2 — the selective HTML gate. Returns the hook exit code.
/// `sav` is Some on a fresh fetch (real before/after numbers) and None
/// on a session-cache hit (original HTML tokens unknown).
///
/// Gate: intercept ⇔ distilled tokens ≥ [`webfetch_intercept_min_tokens`]
/// AND [`prompt_scoped_extract`] finds a lexical anchor. Both failing
/// legs passthrough (return 0, NO gain record) — on a normal-size page
/// or an unanchored prompt the native scoped summary is strictly better
/// (MEASURED). The old outline+stash route is gone from this path: it
/// never contained the answer and only ever bought a second turn.
#[allow(clippy::too_many_arguments)]
/// **The question is asked of the PAGE, not of the capped view.** `compact`
/// is what is left after the hard cap (50 000 tokens, headings kept and bodies
/// dropped); a term that lives only in a dropped body was searched for where
/// it had already been removed. `web --prompt` was fixed for exactly this
/// (`feedback_a_retriever_must_read_the_source_not_a_reduced_view`); the
/// WebFetch hook carried `full_md` to its stash and still searched `compact`
/// (2026-10-03). Redacted here, as `compact` was upstream.
fn searched_page<'a>(full_md: Option<&str>, compact: &'a str) -> std::borrow::Cow<'a, str> {
    match full_md {
        Some(page) => std::borrow::Cow::Owned(tokenade_core::redact::redact_for_model(page)),
        None => std::borrow::Cow::Borrowed(compact),
    }
}

pub(crate) fn serve_selective_gate(
    url: &str,
    prompt: &str,
    compact: &str,
    sav: Option<&tokenade_core::Savings>,
    session_id: &str,
    // `from_cache`: body came from the session cache, not the network.
    //
    // Round 38 audit flagged that a cache hit still books full scoped credit
    // and asked whether that double-counts. It does not, and the reason is
    // worth writing down: a cache hit still DELIVERS the excerpt to the agent,
    // and without tokenade the agent's native fetch would have re-run and
    // re-paid the full page. The counterfactual we credit against is "what the
    // agent would have spent", not "what we spent" — so the credit is owed on
    // every serve, cached or not. Only a serve that delivers nothing may book
    // nothing (see the passthrough path, which books none).
    from_cache: bool,
    started: std::time::Instant,
    native_blocked: bool,
    must_disclose: bool,
    // Voir `serve_legacy_intercept` : le markdown d'avant le plafond dur,
    // transmis tel quel aux deux redispatchs ci-dessous.
    full_md: Option<&str>,
) -> i32 {
    let compact_tokens = sav
        .map(|s| s.tokens_after)
        .unwrap_or_else(|| tokenade_core::tokenizer::estimate(compact));
    // Serving the WHOLE page needs the real before/after numbers the legacy
    // path books; a session-cache hit has none, so it keeps the old gate.
    let native_blocked = native_blocked && sav.is_some();
    let must_disclose = must_disclose && sav.is_some();
    let extract = match selective_gate_decision(
        prompt,
        compact,
        full_md,
        compact_tokens,
        native_blocked,
        must_disclose,
    ) {
        SelectiveDecision::InterceptWhole => {
            let sav = sav.expect("InterceptWhole implies sav");
            tokenade_core::debug_log::log(
                "hookwrap-pretool",
                "webfetch_intercept_native_blocked",
                serde_json::json!({
                    "url": url,
                    "md_tokens": compact_tokens,
                    "elapsed_ms": started.elapsed().as_millis() as u64,
                }),
            );
            return serve_legacy_intercept(
                url, prompt, compact, sav, &None, started, None, full_md,
            );
        }
        SelectiveDecision::Passthrough(why) => {
            record_spa_shell_if_empty(url, compact, sav, compact_tokens);
            tokenade_core::debug_log::log(
                "hookwrap-pretool",
                why,
                serde_json::json!({
                    "url": url,
                    "md_tokens": compact_tokens,
                    "min_tokens": webfetch_intercept_min_tokens(),
                    "from_cache": from_cache,
                    "elapsed_ms": started.elapsed().as_millis() as u64,
                }),
            );
            return 0;
        }
        SelectiveDecision::Intercept { extract } => extract,
    };
    // The excerpt is only worth its own explanation when the page it withholds
    // is bigger than the explanation. Below that the whole distilled page is
    // both cheaper and complete, and `serve_legacy_intercept` already knows how
    // to serve it M-bM-^@M-^T same delivery the native-blocked leg above takes.
    if let Some(sav) = sav {
        if !scoping_pays(compact_tokens, &extract) {
            tokenade_core::debug_log::log(
                "hookwrap-pretool",
                "webfetch_intercept_whole_scoping_would_not_pay",
                serde_json::json!({
                    "url": url,
                    "md_tokens": compact_tokens,
                    "extract_tokens": tokenade_core::tokenizer::estimate(&extract),
                    "overhead_tokens": WEBFETCH_SCOPING_OVERHEAD_TOKENS,
                    "elapsed_ms": started.elapsed().as_millis() as u64,
                }),
            );
            return serve_legacy_intercept(
                url, prompt, compact, sav, &None, started, None, full_md,
            );
        }
    }
    // The same page, the same question, twice in one session: the excerpt we
    // are about to serve is already in the agent's context. The session cache
    // above already avoids the FETCH; nothing avoided the re-DELIVERY, which
    // is where the tokens are (MEASURED: 24.4 % of what this path delivers is
    // a re-delivery). Keyed on the excerpt, not the page, because that is what
    // the agent actually received — a different prompt yields a different
    // excerpt and is delivered in full, as it must be.
    if let Some(note) =
        tokenade_core::web_replay::fold_if_already_delivered(session_id, url, &extract)
    {
        let decision = serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": note,
            }
        });
        println!("{decision}");
        // Credited like any delivery of this page, on the pointer delivered.
        let before = tokenade_core::tokenizer::estimate(&extract);
        let credited = tokenade_core::web_credit::credit_on_delivery(
            tokenade_core::web_credit::WebBasis::Excerpt {
                page: compact_tokens,
                floor: WEBFETCH_SCOPED_TURN_CREDIT_TOKENS,
            },
            tokenade_core::tokenizer::estimate(&note),
        );
        gain_log::record("hook", "web-replay", url, credited);
        tokenade_core::debug_log::log(
            "hookwrap-pretool",
            "webfetch_replay_folded",
            serde_json::json!({
                "url": url,
                "extract_tokens": before,
                "elapsed_ms": started.elapsed().as_millis() as u64,
            }),
        );
        return 0;
    }
    // Intercept: stash the full distilled page for recovery + session
    // cache, serve the scoped excerpt inline.
    let stashed = tokenade_core::disclose::stash_for_reference(
        compact,
        &tokenade_core::disclose::SourceHint::web(),
    );
    if let Some((_, path)) = &stashed {
        webfetch_cache_record(session_id, url, path);
    }
    let recover = match &stashed {
        Some((hash, path)) => webfetch_recover_advice(hash, compact_tokens, path),
        None => "Full distilled page not stashed — re-fetch via `Bash curl -sL <url>` if the \
                 excerpt below is insufficient."
            .to_string(),
    };
    let cache_note = if from_cache {
        " (served from session cache — no network fetch)"
    } else {
        ""
    };
    // Derived from the body we are about to serve, so the marker and the
    // disclosure under it cannot disagree.
    let marker = webfetch_lead_marker(compact);
    // Say WHY the page was withheld, and only claim what is true of THIS page.
    // "too large for the native fetch" was printed at every size, including on
    // a 122-token page: an agent told the page is huge does not ask for the
    // rest of it. Above the inline cap the claim is the real reason; below it
    // the real reason is that the excerpt is cheaper than the page.
    let why = if compact_tokens > webfetch_inline_cap() {
        format!(
            "this page is too large for the native fetch ({compact_tokens} tokens distilled; \
             native truncates and can miss the answer)"
        )
    } else {
        format!(
            "the distilled page is {compact_tokens} tokens and the passages below answer the \
             prompt for fewer — the whole page is one `expand-ref` away if you need it"
        )
    };
    let reason = format!(
        "{marker}\n\
         [tokenade:webfetch-scoped] {url}{cache_note} — {why}; \
         below are the passage(s) matching your prompt {prompt:?}. If several values appear \
         they are ALL shown — pick the one that answers the prompt; do NOT re-fetch. Need \
         several things? Put them in `prompt` separated by commas — each is answered under \
         its own `### Q` heading, in one fetch.\n\
         {recover}\n\n\
         --- BEGIN PROMPT-SCOPED EXCERPT ---\n\
         {extract}\n\
         --- END PROMPT-SCOPED EXCERPT ---",
    );
    let decision = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    });
    println!("{decision}");
    // Remember what was actually delivered, so an identical re-fetch in this
    // session folds instead of repeating it.
    tokenade_core::web_replay::remember(session_id, url, &extract);
    // Accounting: credit the scoping delta against the NATIVE baseline
    // (~3× the delivered excerpt — the measured web economy), capped at the
    // real distilled-page size so a huge/PDF page can't book runaway credit,
    // and floored at one avoided turn so a small page never regresses. Native
    // truncates/fails on exactly the huge & PDF pages this gate targets while
    // we hand back only the scoped excerpt.
    let credited = tokenade_core::web_credit::credit_on_delivery(
        tokenade_core::web_credit::WebBasis::Excerpt {
            page: compact_tokens,
            floor: WEBFETCH_SCOPED_TURN_CREDIT_TOKENS,
        },
        tokenade_core::tokenizer::estimate(&reason),
    );
    gain_log::record("hook", "webfetch_compact", url, credited);
    tokenade_core::debug_log::log(
        "hookwrap-pretool",
        "webfetch_scoped_intercept",
        serde_json::json!({
            "url": url,
            "md_tokens": compact_tokens,
            "extract_tokens": tokenade_core::tokenizer::estimate(&extract),
            "from_cache": from_cache,
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
    0
}

/// Pre-#955v2 always-intercept behaviour (kill-switch / bench arm):
/// inline-whole ≤ cap, scoped extract, or outline+stash — plus the
/// original web-credit accounting.
/// `precomputed` is `Some` when the CALLER already priced the event (a site
/// reader, a feed, a product page). Those assiettes are deliberate — a reader
/// books a flat reliability credit — and re-running them through
/// `web_credit_savings` DESTROYED them: on Claude Code (`Transparent`, ×0.6) a
/// flat 400 became `0.6·(delivered+400) < delivered`, which the anti-inflation
/// backstop clamped to ZERO. Every reader intercept booked nothing.
/// What the footnote can honestly claim.
///
/// One sentence, in one place, because it is written on TWO branches and a
/// rule wired to only one arm has already been paid for once.
fn completeness_note(the_cap_bit: bool) -> String {
    if the_cap_bit {
        "(Distilled markdown, then SAMPLED: the page passed the hard cap, so this view keeps \
         one line in N. The whole page is in the stash named above — ask it with \
         `tokenade expand-ref <hash> --prompt \"…\"`.)"
            .to_string()
    } else {
        "(Distilled markdown: script/style/nav/footer dropped, ALL text preserved.)".to_string()
    }
}

pub(crate) fn serve_legacy_intercept(
    url: &str,
    prompt: &str,
    compact: &str,
    sav: &tokenade_core::Savings,
    content_type: &Option<String>,
    started: std::time::Instant,
    // What the credit is computed from, when the caller decided it (a site
    // reader, a feed, a product page); `None` = the page itself.
    basis: Option<tokenade_core::web_credit::WebBasis>,
    // The markdown from BEFORE the hard cap, when the cap bit.
    //
    // `None` on the paths that do not go through it — the session cache and
    // the reader-grade outputs — and that is true by CONSTRUCTION: those
    // branches come back from the distiller without going through the capping.
    full_md: Option<&str>,
) -> i32 {
    let pct = sav.percent() as u64;
    let web_hint = match content_type {
        Some(ct) if !ct.is_empty() => {
            tokenade_core::disclose::SourceHint::web_with_content_type(ct.clone())
        }
        _ => tokenade_core::disclose::SourceHint::web(),
    };
    // Holistic intent ("summarize this", "full transcript", "tl;dr") wants
    // the WHOLE content, not passages matching a needle — scoping it to a
    // few sections defeats the ask. Raise the serve-whole cap so a full
    // video transcript / article is delivered intact (still bounded so a
    // 2-hour transcript can't blow the context).
    let inline_cap = if holistic_intent(prompt) {
        webfetch_inline_cap().max(WEBFETCH_HOLISTIC_CAP)
    } else {
        webfetch_inline_cap()
    };
    let compact_tokens = sav.tokens_after;
    let scoped_budget = tokenade_core::disclose::webfetch_scoped_budget_for(compact_tokens);
    let marker = webfetch_lead_marker(compact);
    let reason = if compact_tokens <= inline_cap {
        format!(
            "{marker}\n\
             [tokenade:webfetch-compact] {url} ({before}→{after} tokens, -{pct}%)\n\n\
             {compact}\n\n\
             ({completeness} \
             Raw HTML (rare)? `Bash curl -sL <url>` — direct curl is not hook-intercepted.)",
            before = sav.tokens_before,
            after = sav.tokens_after,
            completeness = completeness_note(full_md.is_some()),
        )
    } else {
        let scoped = if prompt.trim().is_empty() {
            None
        } else {
            tokenade_core::rag::prompt_scoped_extract_dispatch(
                &searched_page(full_md, compact),
                prompt,
                scoped_budget,
            )
        };
        match scoped {
            Some(extract) => {
                let recover = match tokenade_core::disclose::stash_for_reference(compact, &web_hint)
                {
                    Some((hash, path)) => webfetch_recover_advice(&hash, sav.tokens_after, &path),
                    None => format!(
                        "Full distilled page ({after} tokens) not stashed — re-fetch via \
                         `Bash curl -sL <url>` if the excerpt below is insufficient.",
                        after = sav.tokens_after,
                    ),
                };
                format!(
                    "{marker}\n\
                     [tokenade:webfetch-scoped] {url} — the page is large ({after} tokens \
                     distilled); below are the passage(s) matching your prompt {prompt:?}. If \
                     several values appear, they are ALL shown — pick the one that answers the \
                     prompt; do NOT re-fetch. Need several things? Put them in `prompt` \
                     separated by commas — each is answered under its own `### Q` heading.\n\
                     {recover}\n\n\
                     --- BEGIN PROMPT-SCOPED EXCERPT ---\n\
                     {extract}\n\
                     --- END PROMPT-SCOPED EXCERPT ---",
                    after = sav.tokens_after,
                )
            }
            None => {
                // Round 39 — "ALL text preserved" is true of the markdown
                // distiller and FALSE of the JSON one, which keeps the shape
                // and the first record and drops the rest. Claiming
                // completeness over an outline is worse than the loss itself:
                // it tells the agent not to go looking. The JSON path appends
                // its own stash pointer, so use that as the signal.
                let is_outline = compact.contains("[full JSON stashed:");
                let footnote = if is_outline {
                    "(JSON outline: shape + first record inline; the full body is stashed above.)"
                        .to_string()
                } else {
                    // **And the hard cap is the third case of the same
                    // family.** The rule written for the JSON outline holds
                    // identically for the OUTPUT cut: above 50 000 tokens the
                    // view is sampled, one line in N, and claiming "ALL text
                    // preserved" there tells the agent not to go looking for
                    // what is missing.
                    completeness_note(full_md.is_some())
                };
                let full = format!(
                    "{marker}\n\
                     [tokenade:webfetch-compact] {url} ({before}→{after} tokens, -{pct}%)\n\n\
                     {compact}\n\n\
                     {footnote}",
                    before = sav.tokens_before,
                    after = sav.tokens_after,
                );
                // The stash holds the PAGE, not the composed delivery: on a
                // capped page, stashing `full` would stash the sample with a
                // banner around it (same move, third route).
                match full_md {
                    Some(page) => tokenade_core::disclose::maybe_stash_with_body(
                        page.to_string(),
                        web_hint.clone(),
                        full,
                    ),
                    None => tokenade_core::disclose::maybe_stash(full, web_hint.clone()),
                }
            }
        }
    };
    let decision = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    });
    println!("{decision}");
    // On what the agent RECEIVES — the page, an excerpt, or a pointer to it.
    let credited = tokenade_core::web_credit::credit_on_delivery(
        basis.unwrap_or(tokenade_core::web_credit::WebBasis::Page {
            raw: sav.tokens_before,
        }),
        tokenade_core::tokenizer::estimate(&reason),
    );
    gain_log::record("hook", "webfetch_compact", url, credited);
    tokenade_core::debug_log::log(
        "hookwrap-pretool",
        "webfetch_compact_hit",
        serde_json::json!({
            "url": url,
            "before": sav.tokens_before,
            "after": sav.tokens_after,
            "pct": pct,
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
    0
}

/// Shared inline cap: below this many tokens, serve content whole
/// (legacy HTML route A, and the extracted-document inline path);
/// above it, scope with RAG. Set from a cap sweep (2026-07-11, 99-case
/// corpus): serve-whole gives 100% recall at cost = page size, RAG gives
/// ~85% at ~500 tok. Recall/token efficiency stays ≥7 pp/1k up to ~4k then
/// falls, and corpus recall plateaus 4k–6k (89.9%) before an expensive jump
/// to 93.9% at 8k. 4k captures ~90% of the recall at ~half the average
/// token cost of 8k — the token-economy compromise. Recall-max users can
/// raise it via `TOKENADE_WEBFETCH_INLINE_TOKENS`.
pub(crate) const WEBFETCH_INLINE_CAP_DEFAULT: usize = 4_000;

/// Serve-whole cap when the prompt asks for the WHOLE content (summary,
/// full transcript). Higher than the default so a full-length video
/// transcript / long article is delivered intact rather than scoped;
/// still bounded so an extreme transcript can't overflow the context
/// (beyond it we fall back to scoping). Overridable via
/// `TOKENADE_WEBFETCH_HOLISTIC_TOKENS`.
pub(crate) const WEBFETCH_HOLISTIC_CAP: usize = 24_000;

/// True when the prompt wants the whole content digested rather than a
/// specific fact — summary / overview / full transcript / tl;dr, in English
/// or French. These asks have no lexical needle to scope to, so serving the
/// whole (up to [`WEBFETCH_HOLISTIC_CAP`]) beats a passage extract.
pub(crate) fn holistic_intent(prompt: &str) -> bool {
    let p = prompt.to_lowercase();
    const NEEDLES: &[&str] = &[
        "summar",
        "summari", // summarize/summary/summarise
        "résum",
        "resum", // résume/résumé/resume
        "tl;dr",
        "tldr",
        "tl dr",
        "overview",
        "vue d'ensemble",
        "vue d ensemble",
        "full transcript",
        "entire transcript",
        "whole transcript",
        "transcript complet",
        "tout le transcript",
        "transcription complète",
        "the whole ",
        "the entire ",
        "in full",
        "en entier",
        "en intégralité",
        "gist of",
        "key points",
        "points clés",
        "main points",
        "what is this ",
        "what's this ",
        "de quoi parle",
        "que dit",
        "walk me through",
        "explain this ",
    ];
    NEEDLES.iter().any(|n| p.contains(n))
}

pub(crate) fn webfetch_inline_cap() -> usize {
    std::env::var("TOKENADE_WEBFETCH_INLINE_TOKENS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(WEBFETCH_INLINE_CAP_DEFAULT)
}

#[cfg(test)]
mod a_note_promises_only_what_it_has {
    use super::*;

    /// The term lives only in a body the hard cap dropped: asked of the
    /// capped view it has no anchor; asked of the page it is found.
    #[test]
    fn the_question_is_asked_of_the_page_before_the_cap() {
        let filler: String = (0..400)
            .map(|i| {
                format!(
                    "## Section {i}\n\nOrdinary text about topic number {i} and its details.\n\n"
                )
            })
            .collect();
        let page =
            format!("{filler}## Hooks\n\nThe updatedToolOutput field replaces the tool result.\n");
        let capped = format!("{filler}## Hooks\n\n");
        let ask = |full: Option<&str>| {
            selective_gate_decision(
                "what does updatedToolOutput do",
                &capped,
                full,
                60_000,
                false,
                false,
            )
        };
        assert!(matches!(ask(None), SelectiveDecision::Passthrough(_)));
        match ask(Some(&page)) {
            SelectiveDecision::Intercept { extract } => {
                assert!(extract.contains("updatedToolOutput field"), "{extract}")
            }
            _ => panic!("the page carries the answer"),
        }
    }

    /// **Claiming completeness on a sampled view is worse than the loss**:
    /// the sentence tells the agent not to go looking. That is the rule
    /// written for the JSON outline; it holds identically for the hard cap,
    /// which keeps one line in N.
    #[test]
    fn the_note_stops_promising_all_the_text_once_the_cap_has_bitten() {
        let whole = completeness_note(false);
        let capped = completeness_note(true);
        assert!(
            whole.contains("ALL text preserved"),
            "the uncapped page must still say what it keeps: {whole}"
        );
        assert!(
            !capped.contains("ALL text preserved"),
            "the sampled view still promises all the text: {capped}"
        );
        // And it does not merely WITHDRAW the promise: it says where the rest
        // is, otherwise the agent only knows something is missing.
        assert!(
            capped.contains("stash") && capped.contains("expand-ref"),
            "the note does not say where to find the page: {capped}"
        );
    }
}
