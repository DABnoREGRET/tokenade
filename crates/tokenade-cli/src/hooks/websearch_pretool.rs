//! PreToolUse handler for `WebSearch` — the client-side search gate.
//!
//! Why intercept at all: the native WebSearch tool is the single most
//! expensive tool on web-heavy work, and almost none of that cost is the
//! result TEXT. Measured on a real research run (46 turns, 34 searches):
//! the server-side search loop re-bills the conversation as UNCACHED input
//! on every search (~500k tokens ≈ 38% of the run's cost), adds a
//! per-search fee (~13%), and a sidechain summarizer (~4%) — while the
//! rendered results the model actually reads were ~50k tokens costing
//! ~$0.35 in cache. Compacting the text post-hoc has a hard low ceiling;
//! NOT invoking the native tool is the only lever on the dominant terms.
//!
//! What this does instead: run the search OURSELVES across a ROTATION of
//! key-free engines (Ecosia/Bing/Mojeek/Yahoo), extract the organic hits, and
//! substitute them via the same `PreToolUse deny + permissionDecisionReason`
//! channel the WebFetch gate uses (the model treats the reason as the tool
//! result — validated live on Claude Code 2.1.206).
//!
//! Why a rotation (a lesson from earlier scraping work): from a single IP no engine is
//! reliable alone — engines throttle after a burst of rapid queries or 403 on
//! heavy queries. Rotating across INDEPENDENT indexes (Ecosia=Google,
//! Bing=Microsoft, Mojeek=own crawl) with a PER-ENGINE circuit breaker
//! multiplies the per-IP budget and diversifies coverage; when EVERY engine
//! is down we fail open to the native tool. Default ON — the fallback
//! guarantees we never do worse than native, so the only question is how often
//! we beat it, which we improve engine-by-engine.
//!
//! Transport is the in-process reqwest+rustls client (`tokenade_core::search`
//! → `web_fetch::fetch_search`) with a full browser header set; curl remains
//! only as a handshake fallback. The live fleet is seven engines — Ecosia,
//! Bing, Brave, DuckDuckGo, Mojeek, Yahoo, Marginalia — reached with the full
//! header set (Brave/DDG open on headers, not a spoofed TLS fingerprint). When
//! every engine is saturated the gate fails open to the native tool.
//!
//! Bypass:
//!   * `TOKENADE_NO_WEBSEARCH=1` — kills BOTH websearch features (this gate
//!     and the PostToolUse compactor).
//!   * `TOKENADE_WEBSEARCH_GATE=0` — kills only this gate.
//!   * `TOKENADE_HOOK_DISABLED=1` — all hooks.
use super::*;
use crate::*;

/// Max organic hits served inline. Native renders ~10; we serve the UNION of
/// several engines (each returning ~20), so 15 exposes cross-engine breadth
/// native can't match on coverage-bound research (8→10 measured +recall;
/// 10→15 is the second coverage lever — the merge routinely holds 20+ unique
/// hits and the tail is where the other engines' exclusives live).
pub(crate) const WEBSEARCH_GATE_MAX_HITS: usize = 15;

/// Below this many parsed hits we treat the engine's answer as an anti-bot
/// page / layout drift / genuinely thin query — a FAILURE for that engine
/// (rotate to the next), and native fallback if every engine misses.
pub(crate) const WEBSEARCH_GATE_MIN_HITS: usize = 3;

// The per-engine circuit breaker named in this module's doc is REAL, but it
// lives in `tokenade_core::search` and is driven by `search::CB_MAX_FAILS`.
// Two constants used to sit here — `WEBSEARCH_CB_MAX_FAILS` and
// `WEBSEARCH_CB_TTL_SECS` — spelling the same policy a second time and read by
// nobody. A duplicated knob is worse than none: tuning it changes nothing while
// looking like it should.

/// `TOKENADE_WEBSEARCH_GATE`: `0`/`off`/`false` → off; unset / anything
/// else → ON. Default ON (PO decision 2026-07-10): the native-tool fallback
/// makes the gate never-worse-than-native, so it ships on and improves
/// engine-by-engine.
/// How long the gate waits for the engines before handing the turn back to the
/// native WebSearch. Four times the p95 of a served gate (2 141 ms) and well
/// under the 18 008 ms stall that motivated it; equal to one engine's own
/// per-attempt budget, so a single healthy engine still wins the race.
const GATE_BUDGET: std::time::Duration = std::time::Duration::from_secs(8);

/// `run_search` with a deadline. `None` means the budget expired.
///
/// The worker is left running rather than cancelled: it holds no lock the
/// caller needs, and a hook process exits moments later, which ends it. The
/// cost of that is a breaker update this round may not persist — a slow engine
/// stays untried-but-unpenalised, which is the conservative direction.
fn run_search_within(
    query: &str,
    now: u64,
    budget: std::time::Duration,
) -> Option<tokenade_core::search::Outcome> {
    let (tx, rx) = std::sync::mpsc::channel();
    let q = query.to_string();
    std::thread::spawn(move || {
        let _ = tx.send(tokenade_core::search::run_search(&q, now));
    });
    rx.recv_timeout(budget).ok()
}

pub(crate) fn websearch_gate_enabled() -> bool {
    match std::env::var("TOKENADE_WEBSEARCH_GATE") {
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            !(v == "0" || v == "off" || v == "false")
        }
        Err(_) => true,
    }
}

// ── Engine registry ───────────────────────────────────────────────────────

// The engine registry — the seven-engine fleet (Ecosia, Bing, Brave,
// DuckDuckGo, Mojeek, Yahoo, Marginalia), chosen for INDEX DIVERSITY under a
// single IP — plus the circuit breaker, rotation, per-engine budget/lock and
// the SERP fetch all moved to
// `tokenade_core::search` so that `tokenade search` shares them. Only the
// PreToolUse adapter is left in this file.

pub(crate) fn handle_websearch_pretool(
    parsed: &serde_json::Value,
    started: std::time::Instant,
) -> i32 {
    if hook_disabled_by_env() {
        return 0;
    }
    if tokenade_core::env_compat::is_truthy_one("TOKENADE_NO_WEBSEARCH") {
        return 0;
    }
    if !websearch_gate_enabled() {
        return 0;
    }
    let query = tool_input_str(parsed, "query");
    if query.trim().is_empty() {
        return 0;
    }
    // The search runs HERE and its results ride in a decision's reason: a
    // `WebSearch` deny/ask rule would no longer keep them out. Any such rule
    // ⇒ Claude Code's own WebSearch, under its own rules.
    let cwd = parsed
        .get("cwd")
        .and_then(|c| c.as_str())
        .map(std::path::Path::new);
    if crate::hooks::permission_rules::any_user_rule_for("WebSearch", cwd) {
        return 0;
    }
    // Domain filters change native semantics in ways a plain query string
    // can't faithfully reproduce (multi-domain allow-lists, block-lists) —
    // preserve the caller's contract by not intercepting.
    let has_domain_filter = ["allowed_domains", "blocked_domains"].iter().any(|k| {
        parsed
            .pointer(&format!("/tool_input/{k}"))
            .and_then(|v| v.as_array())
            .is_some_and(|a| !a.is_empty())
    });
    if has_domain_filter {
        tokenade_core::debug_log::log(
            "hookwrap-pretool",
            "websearch_gate_domain_filter_passthrough",
            serde_json::json!({"elapsed_ms": started.elapsed().as_millis() as u64}),
        );
        return 0;
    }

    // The search itself — engines, rotation, breaker, validation, merge and
    // ranking — lives in `tokenade_core::search`, so `tokenade search` and this
    // hook cannot answer the same question differently. There was no CLI
    // equivalent at all before, which meant every hookless agent had NO search.
    //
    // What stays HERE is what only a hook has: the session, and therefore
    // cross-search dedup; and the PreToolUse verdict.
    // D13: this native WebSearch is one single-target web-call; count it toward
    // the batch nudge (shared with the CLI channel).
    crate::compactors::nudge_sequential_web_calls("hook-search", "search", false);
    let now = tokenade_core::platform::now_unix_secs();
    // `run_search` reads a lock-free rotation snapshot, fans out, then persists
    // the touched engines under a cross-process advisory lock (D12) — so N
    // concurrent agents cannot corrupt the breaker file or double-hammer one
    // engine.
    //
    // …under a DEADLINE, because this gate has a free fallback and the search
    // does not. Measured over this machine's hook log: a served gate is 1 145 ms
    // at p50 and 2 141 ms at p95, but a gate that ended `unusable` once took
    // **18 008 ms** (`duckduckgo:unreachable`, `bing:off-topic`) — eighteen
    // seconds of the agent's turn spent to learn that the answer would come
    // from the native WebSearch that was available the whole time.
    //
    // The bound belongs HERE and not in `search`: `tokenade search` has no
    // fallback, so waiting out a slow engine is the right thing there. The cost
    // of the bound is a slow-but-successful search we now decline — the answer
    // still arrives natively, only without this channel's dedup and ranking.
    let outcome = match run_search_within(query, now, GATE_BUDGET) {
        Some(o) => o,
        None => {
            tokenade_core::debug_log::log(
                "hookwrap-pretool",
                "websearch_gate_timed_out",
                serde_json::json!({
                    "budget_ms": GATE_BUDGET.as_millis() as u64,
                    "elapsed_ms": started.elapsed().as_millis() as u64,
                }),
            );
            return 0;
        }
    };

    if !outcome.is_usable() {
        // Native fallback. Say WHY in the log: "the engines refused us" and
        // "the web has nothing" are different facts, and only the first is
        // worth acting on.
        tokenade_core::debug_log::log(
            "hookwrap-pretool",
            "websearch_gate_unusable",
            serde_json::json!({
                "refused": outcome.refused.iter()
                    .map(|(e, r)| format!("{e}:{}", r.as_str()))
                    .collect::<Vec<_>>(),
                "elapsed_ms": started.elapsed().as_millis() as u64,
            }),
        );
        return 0;
    }
    let merged = outcome.results;
    let used: Vec<&str> = outcome.used.clone();

    // Cross-search dedup: one seen-set per session, SHARED with the native
    // post-hook compactor (websearch_post), so a URL served by either path
    // is never re-billed by the other. Already-served URLs are omitted and
    // their slots refilled with deeper unseen hits — a follow-up query spends
    // the same tokens on NEW ground instead of repeats. The merge order is
    // deterministic (engine priority, then rank), so the selection is stable
    // across identical inputs. Dialects without a session_id skip this.
    let session_id = crate::hooks::session_id_of(parsed).unwrap_or("");
    let seen_path = (!session_id.is_empty()).then(|| {
        super::websearch_post::websearch_seen_path(
            &super::websearch_post::websearch_state_dir(),
            session_id,
        )
    });
    let mut seen_state = seen_path
        .as_deref()
        .map(super::websearch_post::load_websearch_seen);
    let (picked, omitted) = select_hits(&merged, seen_state.as_ref());
    let shown = picked.len();
    let mut body = String::new();
    for (i, (h, rep)) in picked.iter().enumerate() {
        let mark = if *rep {
            " (already served earlier this session)"
        } else {
            ""
        };
        body.push_str(&format!("{}. {}{mark}\n   {}\n", i + 1, h.title, h.url));
        if !h.snippet.is_empty() {
            body.push_str(&format!("   {}\n", h.snippet));
        }
    }
    if omitted > 0 {
        body.push_str(&format!(
            "({omitted} result(s) already served by earlier searches omitted — \
             vary the query to surface new ground)\n"
        ));
    }
    // Record what we just served so later searches (gate or native) skip it.
    if let (Some(path), Some(s)) = (seen_path.as_deref(), seen_state.as_mut()) {
        for (h, rep) in &picked {
            if !rep {
                s.urls.insert(super::websearch_post::normalize_url(&h.url));
            }
        }
        super::websearch_post::save_websearch_seen(path, s);
    }
    let engine = used.join("+");
    let reason = format!(
        "✓ SEARCH OK — tokenade ran this web search successfully (this is the result, \
         NOT an error; do NOT retry the same query).\n\
         [tokenade:websearch via {engine}] results for {query:?}:\n\n\
         {body}\n\
         Need page details? WebFetch a URL above. Need different results? Search again \
         with a REFORMULATED query.",
    );
    // SECURITY: same print-site scrub as every other served channel.
    let reason = tokenade_core::redact::redact_for_model(&reason);
    println!(
        "{}",
        serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": reason,
            }
        })
    );
    // Credit against the NATIVE SERP baseline (measured ~3× our compacted
    // union list), not a flat turn credit — the native result set reaches the
    // model whole regardless of how compact ours is, so `before` must be the
    // native size, not `served + constant`. Reflects the measured ~3× less
    // consumption on web search. Clamped ≥0 downstream.
    //
    // Round 38 (PO) — the base is the RESULTS (`body`), never the whole
    // `reason`. The banner ("✓ SEARCH OK…", "Need page details?…") is our own
    // overhead, and multiplying it by 3 made the accounting *inverted*: every
    // word added to the banner booked ~2 more words of "savings", and a search
    // returning ZERO hits still booked ~2× the boilerplate. Our own text is
    // not a saving; it is a cost we choose to pay.
    let body_tokens = tokenade_core::tokenizer::estimate(&body);
    let served_tokens = tokenade_core::tokenizer::estimate(&reason);
    let banner_tokens = served_tokens.saturating_sub(body_tokens);
    let credited = tokenade_core::coefficients::web_search_credit_savings(body_tokens);
    // The banner is DELIVERED, so it belongs on the `after` side: we booked a
    // saving against the results, then spent tokens on top of them. This keeps
    // the row honest without needing a negative saving (the measured ledger
    // cannot represent one — its integrity checker treats `after > before` as
    // forged). Tagged so the overhead stays separable from the compaction.
    let credited = tokenade_core::Savings::new(
        credited.tokens_before,
        credited.tokens_after.saturating_add(banner_tokens),
    );
    gain_log::record_classed(
        "hook",
        "websearch_gate",
        "websearch_gate",
        "banner_overhead",
        credited,
    );
    tokenade_core::gain::record_nudge(
        "websearch_banner",
        "served",
        tokenade_core::gain::NudgeMetric::Tokens(banner_tokens),
    );
    tokenade_core::debug_log::log(
        "hookwrap-pretool",
        "websearch_gate_served",
        serde_json::json!({
            "engines": used,
            "hits": shown,
            "omitted_seen": omitted,
            "merged": merged.len(),
            "served_tokens": served_tokens,
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
    0
}

/// Pick which merged hits to serve, honouring the cross-search seen-set:
/// unseen hits first (up to MAX), then — only when fewer than MIN unseen
/// remain, e.g. the same query re-run verbatim — already-served hits top
/// up to MIN, flagged `true` so the caller marks them. Returns the picks
/// plus how many already-served hits were omitted outright. `None` seen
/// (no session id) serves the plain merge.
#[cfg(test)]
mod gate_deadline {
    use std::time::Duration;

    /// The gate must hand the turn back rather than wait out a stalled engine.
    ///
    /// Measured over this machine's hook log: a SERVED gate is 1 145 ms at p50
    /// and 2 141 ms at p95, and one gate that ended `unusable` took
    /// **18 008 ms** — eighteen seconds spent to learn the answer would come
    /// from the native WebSearch available the whole time.
    ///
    /// Exercised on the deadline itself with a stand-in worker, because the
    /// real one reaches the network: what is under test is that a worker slower
    /// than the budget yields `None` promptly, and a faster one yields its
    /// value.
    fn within<T: Send + 'static>(
        budget: Duration,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Option<T> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(work());
        });
        rx.recv_timeout(budget).ok()
    }

    #[test]
    fn a_stalled_search_does_not_hold_the_turn() {
        let t = std::time::Instant::now();
        let out = within(Duration::from_millis(120), || {
            std::thread::sleep(Duration::from_secs(30));
            "too late"
        });
        assert!(out.is_none(), "the budget must expire");
        assert!(
            t.elapsed() < Duration::from_secs(5),
            "and it must expire PROMPTLY, not after the worker: {:?}",
            t.elapsed()
        );
    }

    #[test]
    fn a_search_inside_the_budget_is_served() {
        assert_eq!(
            within(Duration::from_secs(5), || "in time"),
            Some("in time"),
            "a search that answers in time must not be thrown away"
        );
    }

    /// …and the gate must actually USE it. Removing the deadline from the call
    /// site leaves the two helpers unused — which the compiler notices and no
    /// test did. Asserted on the source, because the alternative is a probe
    /// that reaches the network to prove a negative.
    #[test]
    fn the_gate_calls_the_deadlined_search() {
        let src = include_str!("websearch_pretool.rs");
        let body = src
            .split_once("pub(crate) fn handle_websearch_pretool")
            .expect("the gate")
            .1;
        // Only the production half, or the sweep reads its OWN assertion text
        // as if it were the gate's code — the same trap, one file over. Through
        // `srcsweep`, which is the declarant: cutting at the FIRST
        // `#[cfg(test)]` also throws away every line of production that follows
        // a mid-file test module.
        let body = tokenade_core::srcsweep::production_lines(body)
            .into_iter()
            .map(|(_, l)| l)
            .collect::<Vec<_>>()
            .join("\n");
        let body = body.as_str();
        // A user's WebSearch deny/ask rule is consulted BEFORE the search runs.
        let guard = body
            .find("any_user_rule_for(\"WebSearch\", cwd)")
            .expect("the rule guard");
        let search = body.find("run_search_within(").expect("the search");
        assert!(
            guard < search,
            "the search runs before the user's rules are read"
        );
        assert!(
            body.contains("run_search_within(query, now, GATE_BUDGET)"),
            "the gate must search under the deadline"
        );
        assert!(
            !body.contains("search::run_search(query, now)"),
            "…and not beside it"
        );
    }

    /// The budget is a bound on the AGENT's wait, so it must sit above the
    /// observed p95 of a served gate and below the stall that motivated it.
    #[test]
    fn the_budget_is_between_the_p95_and_the_stall() {
        assert!(
            super::GATE_BUDGET > Duration::from_millis(2_141),
            "p95 of a served gate"
        );
        assert!(
            super::GATE_BUDGET < Duration::from_millis(18_008),
            "the measured stall"
        );
    }
}

fn select_hits<'a>(
    merged: &'a [tokenade_core::serp::Result],
    seen: Option<&super::websearch_post::WebsearchSeen>,
) -> (Vec<(&'a tokenade_core::serp::Result, bool)>, usize) {
    let mut fresh: Vec<&tokenade_core::serp::Result> = Vec::new();
    let mut repeats: Vec<&tokenade_core::serp::Result> = Vec::new();
    for h in merged {
        match seen {
            Some(s)
                if s.urls
                    .contains(&super::websearch_post::normalize_url(&h.url)) =>
            {
                repeats.push(h)
            }
            _ => fresh.push(h),
        }
    }
    let mut picked: Vec<(&tokenade_core::serp::Result, bool)> = fresh
        .into_iter()
        .take(WEBSEARCH_GATE_MAX_HITS)
        .map(|h| (h, false))
        .collect();
    for h in &repeats {
        if picked.len() >= WEBSEARCH_GATE_MIN_HITS {
            break;
        }
        picked.push((h, true));
    }
    let omitted = repeats.len() - picked.iter().filter(|(_, rep)| *rep).count();
    (picked, omitted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn urlencode_covers_spaces_unicode_and_reserved() {
        assert_eq!(
            tokenade_core::search::urlencode_query("rust ureq post"),
            "rust+ureq+post"
        );
        assert_eq!(tokenade_core::search::urlencode_query("a&b=c"), "a%26b%3Dc");
        assert_eq!(tokenade_core::search::urlencode_query("café"), "caf%C3%A9");
        assert_eq!(tokenade_core::search::urlencode_query("x-y_z.~"), "x-y_z.~");
    }

    #[test]
    fn gate_env_semantics() {
        std::env::remove_var("TOKENADE_WEBSEARCH_GATE");
        assert!(websearch_gate_enabled(), "default is ON");
        std::env::set_var("TOKENADE_WEBSEARCH_GATE", "0");
        assert!(!websearch_gate_enabled());
        std::env::set_var("TOKENADE_WEBSEARCH_GATE", "off");
        assert!(!websearch_gate_enabled());
        std::env::set_var("TOKENADE_WEBSEARCH_GATE", "1");
        assert!(websearch_gate_enabled());
        std::env::remove_var("TOKENADE_WEBSEARCH_GATE");
    }

    /// Rotation, the breaker and dedup now live in `tokenade_core::search`
    /// and are tested there against all five engines. This asserts only the
    /// property THIS file still depends on: an all-open fleet must yield an
    /// empty batch, because that is the hook's native-fallback trigger.
    #[test]
    fn an_all_open_fleet_means_native_fallback() {
        use tokenade_core::search::{rotation_order, EngineHealth, Health, ENGINES};
        let now = 1_000_000u64;
        let mut h = Health::new();
        for e in ENGINES {
            h.insert(
                e.name().into(),
                EngineHealth {
                    fails: 5,
                    open_until: now + 600,
                    last_used: now,
                    ..Default::default()
                },
            );
        }
        assert!(
            rotation_order(&h, now).is_empty(),
            "all open → native fallback"
        );
    }

    fn hit(url: &str) -> tokenade_core::serp::Result {
        tokenade_core::serp::Result {
            title: format!("t {url}"),
            url: url.to_string(),
            snippet: String::new(),
        }
    }

    #[test]
    fn select_hits_no_seen_serves_plain_merge_capped() {
        let merged: Vec<_> = (0..WEBSEARCH_GATE_MAX_HITS + 4)
            .map(|i| hit(&format!("https://a.com/{i}")))
            .collect();
        let (picked, omitted) = select_hits(&merged, None);
        assert_eq!(picked.len(), WEBSEARCH_GATE_MAX_HITS);
        assert_eq!(omitted, 0);
        assert!(picked.iter().all(|(_, rep)| !rep));
    }

    #[test]
    fn select_hits_omits_seen_and_refills_with_deeper_hits() {
        // 12 merged hits, first 4 already served → the 10 slots are filled by
        // hits 5..12 (8 fresh), no marked repeats needed (fresh ≥ MIN).
        let merged: Vec<_> = (0..12)
            .map(|i| hit(&format!("https://a.com/{i}")))
            .collect();
        let mut seen = super::super::websearch_post::WebsearchSeen::default();
        for i in 0..4 {
            seen.urls
                .insert(super::super::websearch_post::normalize_url(&format!(
                    "https://a.com/{i}"
                )));
        }
        let (picked, omitted) = select_hits(&merged, Some(&seen));
        assert_eq!(picked.len(), 8, "only fresh hits served");
        assert!(picked.iter().all(|(_, rep)| !rep));
        assert_eq!(
            picked[0].0.url, "https://a.com/4",
            "seen head skipped, deeper hits promoted"
        );
        assert_eq!(omitted, 4);
    }

    #[test]
    fn select_hits_repeat_heavy_tops_up_to_min_with_marked_repeats() {
        // Same query re-run verbatim: everything seen → serve MIN marked
        // repeats (anchors), omit the rest.
        let merged: Vec<_> = (0..10)
            .map(|i| hit(&format!("https://a.com/{i}")))
            .collect();
        let mut seen = super::super::websearch_post::WebsearchSeen::default();
        for h in &merged {
            seen.urls
                .insert(super::super::websearch_post::normalize_url(&h.url));
        }
        let (picked, omitted) = select_hits(&merged, Some(&seen));
        assert_eq!(picked.len(), WEBSEARCH_GATE_MIN_HITS);
        assert!(picked.iter().all(|(_, rep)| *rep));
        assert_eq!(omitted, 10 - WEBSEARCH_GATE_MIN_HITS);
    }
}
