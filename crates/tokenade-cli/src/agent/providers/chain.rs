//! **Try, and try ELSEWHERE.**
//!
//! No free provider is reliable on its own: the chain exists so that one
//! provider's failure is not the call's failure. Hedging (`hedge`) goes
//! further — it launches the next one BEFORE the first has failed, because a
//! slow provider costs as much as a dead one.
//!
//! The delay before hedging is the setting that matters: too short and we
//! pay for two calls for nothing; too long and we have already waited.

use super::*;

/// Resolve a caller-requested model name to (provider, concrete-model)
/// candidates, honouring the fallback order. `want` may be:
///   - empty / "auto" / "free" → the whole Http chain, best first
///   - a provider id ("opencode-zen") → that provider's models
///   - a "provider/model" pair → that exact pair, then the rest as fallback
pub(crate) fn resolve_chain(want: &str) -> Vec<(&'static Provider, &'static str)> {
    let want = want.trim();
    let mut chain: Vec<(&Provider, &str)> = Vec::new();
    let http = || PROVIDERS.iter().filter(|p| p.state == ImplState::Http);

    // Env-configured OpenAI-compatible endpoint takes priority when set.
    // This makes the feature usable TODAY with any no-signup base URL —
    // a LOCAL free model (Ollama: `http://localhost:11434/v1`, model
    // `llama3.2`) is genuinely zero-cost and needs no account — while the
    // scraped free endpoints below stay best-effort. Optional bearer key
    // via TOKENADE_AGENT_API_KEY (only sent to THIS endpoint).
    if let Some(ep) = env_provider() {
        chain.push(ep);
    }

    if want.is_empty() || want == "auto" || want == "free" {
        for p in http() {
            for m in p.models {
                chain.push((p, *m));
            }
        }
        return chain;
    }
    // "provider/model" or "provider"
    let (pid, model) = match want.split_once('/') {
        Some((p, m)) => (p, Some(m)),
        None => (want, None),
    };
    if let Some(p) = PROVIDERS
        .iter()
        .find(|p| p.id == pid && p.state == ImplState::Http)
    {
        match model {
            Some(m) => chain.push((p, leak(m))),
            None => {
                for m in p.models {
                    chain.push((p, *m));
                }
            }
        }
    }
    // Always append the rest of the Http chain as fallback (dedup-free is fine).
    for p in http() {
        if p.id == pid {
            continue;
        }
        for m in p.models {
            chain.push((p, *m));
        }
    }
    chain
}

/// Call the free chain until one provider returns a completion.
///
/// Fallback + retry algorithm (from OmniRoute):
///   for each (provider, model) in chain:
///     if provider circuit is open → skip
///     intra-retry the SAME endpoint up to MAX_INTRA_RETRY on 429
///     on any other failure → next provider (bump breaker)
///   → if the whole chain fails, Err with the collected reasons.
pub fn complete(want: &str, messages: &[Msg]) -> Result<Completion, String> {
    // NOTE: `--model local` never reaches here — `runner::run` intercepts it
    // (native function-calling loop, `--features local-llm` builds only).
    let want_t = want.trim();
    if want_t.is_empty() || want_t == "auto" || want_t == "free" {
        // A user-configured endpoint (env) is tried first when present, then
        // the sticky free pool takes over.
        if let Some((p, model)) = env_provider() {
            let mut t = |p: &Provider, m: &str| one_shot(p, m, messages);
            if let Ok(c) = run_chain(&[(p, model)], &mut t, INTRA_DELAY_MS) {
                return Ok(c);
            }
        }
        return complete_free_pool(messages);
    }
    // Explicit `<provider>` / `<provider>/<model>` (env prepended by resolve).
    let chain = resolve_chain(want);
    if chain.is_empty() {
        return Err(format!(
            "no reachable free provider for `{want}` (registered-but-unported ones are skipped)"
        ));
    }
    let mut transport = |p: &Provider, model: &str| one_shot(p, model, messages);
    run_chain(&chain, &mut transport, INTRA_DELAY_MS)
}

/// Every free (no-auth) model across all Http providers, provider order.
/// This is the bench the availability pool sticks-to / rotates through.
pub(crate) fn free_chain() -> Vec<(&'static Provider, &'static str)> {
    let mut v = Vec::new();
    for p in PROVIDERS.iter().filter(|p| p.state == ImplState::Http) {
        for m in p.models {
            v.push((p, *m));
        }
    }
    v
}

/// Free-tier completion driven by the availability [`pool`]: stick to the
/// last-good model (warm provider cache) while it's healthy, rotate
/// round-robin to the next available one on failure, and cool a failed
/// model for an error-weighted, persisted duration. `intra`-retries a 429
/// on the SAME model before rotating.
pub(crate) fn complete_free_pool(messages: &[Msg]) -> Result<Completion, String> {
    let chain = free_chain();
    if chain.is_empty() {
        return Err("no free model available".into());
    }
    let keys: Vec<String> = chain.iter().map(|(p, m)| format!("{}/{m}", p.id)).collect();
    let key_refs: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();

    let mut pool = pool::Pool::load();
    let order = pool.order(&key_refs);
    let mut reasons: Vec<String> = Vec::new();

    // Tail-latency hedge on the head of the pool: free-tier models have a
    // long latency tail (reasoning burns, queueing). If the primary hasn't
    // answered within the hedge delay, fire the SECOND pool entry in parallel
    // and keep whichever returns first. Tokens are free on both sides, so the
    // only cost is politeness — the delay keeps the second request rare
    // (median stays single-request; only the slow tail pays double).
    // `big-pickle` is the same server-side model as `deepseek-v4-flash-free`
    // with a separate quota, which makes the default pairing quality-neutral.
    if order.len() >= 2 {
        if let Some(winner) = hedged_attempt(&chain, &keys, &order[0], &order[1], messages) {
            let (wkey, res) = winner;
            match res {
                Ok(c) => {
                    pool.report_success(&wkey);
                    pool.save();
                    return Ok(c);
                }
                Err((k1, e1, k2, e2)) => {
                    // Both sides failed: record both, then fall through to the
                    // ordinary rotation over the REMAINING models.
                    pool.report_failure(&k1, pool::ErrKind::from_reason(&e1));
                    pool.report_failure(&k2, pool::ErrKind::from_reason(&e2));
                    reasons.push(format!("{k1}: {e1}"));
                    reasons.push(format!("{k2}: {e2}"));
                    let skip: [String; 2] = [k1, k2];
                    for key in order.iter().filter(|k| !skip.contains(k)) {
                        let idx = keys.iter().position(|k| k == key).unwrap();
                        let (p, model) = chain[idx];
                        match one_shot(p, model, messages) {
                            Attempt::Ok(c) => {
                                pool.report_success(key);
                                pool.save();
                                return Ok(c);
                            }
                            Attempt::RateLimited => {
                                pool.report_failure(key, pool::ErrKind::RateLimited);
                                reasons.push(format!("{key}: 429"));
                            }
                            Attempt::Failed(e) => {
                                pool.report_failure(key, pool::ErrKind::from_reason(&e));
                                reasons.push(format!("{key}: {e}"));
                            }
                        }
                    }
                    pool.save();
                    return Err(format!(
                        "all free models failed:\n  {}",
                        reasons.join("\n  ")
                    ));
                }
            }
        }
        // hedging disabled (TOKENADE_AGENT_HEDGE_MS=0) → ordinary loop below.
    }

    for key in &order {
        let idx = keys.iter().position(|k| k == key).unwrap();
        let (p, model) = chain[idx];
        let mut intra = 0u32;
        loop {
            match one_shot(p, model, messages) {
                Attempt::Ok(c) => {
                    pool.report_success(key);
                    pool.save();
                    return Ok(c);
                }
                Attempt::RateLimited if intra < MAX_INTRA_RETRY => {
                    intra += 1;
                    sleep_ms(INTRA_DELAY_MS * intra as u64);
                    continue;
                }
                Attempt::RateLimited => {
                    pool.report_failure(key, pool::ErrKind::RateLimited);
                    reasons.push(format!("{key}: 429 (exhausted intra-retry)"));
                    break;
                }
                Attempt::Failed(e) => {
                    pool.report_failure(key, pool::ErrKind::from_reason(&e));
                    reasons.push(format!("{key}: {e}"));
                    break;
                }
            }
        }
    }
    pool.save();
    Err(format!(
        "all free models failed:\n  {}",
        reasons.join("\n  ")
    ))
}

/// Hedge delay before the second request fires. 0 disables hedging.
pub(crate) fn hedge_delay_ms() -> u64 {
    std::env::var("TOKENADE_AGENT_HEDGE_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(1500)
}

/// Race the pool's top two models against the latency tail: launch the
/// primary immediately; if it hasn't answered within [`hedge_delay_ms`], fire
/// the secondary in parallel and keep whichever succeeds FIRST. Both sides are
/// free-tier, so the only cost is politeness — the delay keeps the median
/// single-request; only the slow tail pays a second call. A fast-failing
/// primary (429/error) arms the secondary immediately. 429 counts as a plain
/// failure here — the hedge IS the retry.
///
/// Returns `None` when hedging is disabled (delay = 0) — caller falls back to
/// the sequential rotation. Otherwise `Some((winner_key, Ok(completion)))`, or
/// `Some(("", Err((k1, e1, k2, e2))))` when BOTH sides failed.
#[allow(clippy::type_complexity)]
pub(crate) fn hedged_attempt(
    chain: &[(&'static Provider, &'static str)],
    keys: &[String],
    k1: &str,
    k2: &str,
    messages: &[Msg],
) -> Option<(String, Result<Completion, (String, String, String, String)>)> {
    let delay = hedge_delay_ms();
    if delay == 0 {
        return None;
    }
    let reason_of = |a: Attempt| -> String {
        match a {
            Attempt::Ok(_) => unreachable!("Ok handled before reason_of"),
            Attempt::RateLimited => "429".into(),
            Attempt::Failed(e) => e,
        }
    };
    let i1 = keys.iter().position(|k| k == k1)?;
    let i2 = keys.iter().position(|k| k == k2)?;
    let (p1, m1) = chain[i1];
    let (p2, m2) = chain[i2];

    let (tx, rx) = std::sync::mpsc::channel::<(u8, Attempt)>();
    let spawn_shot = |tag: u8, p: &'static Provider, m: &'static str| {
        let tx = tx.clone();
        let msgs: Vec<Msg> = messages.to_vec();
        std::thread::spawn(move || {
            let _ = tx.send((tag, one_shot(p, m, &msgs)));
        });
    };
    spawn_shot(1, p1, m1);

    // Phase 1: give the primary `delay` ms to answer alone.
    let first = rx.recv_timeout(std::time::Duration::from_millis(delay));
    let mut fail1: Option<String> = None;
    match first {
        Ok((_, Attempt::Ok(c))) => return Some((k1.to_string(), Ok(c))),
        Ok((_, att)) => fail1 = Some(reason_of(att)), // fast fail → hedge now
        Err(_) => {}                                  // still running → hedge
    }
    spawn_shot(2, p2, m2);

    // Phase 2: first Ok from EITHER side wins. Bounded by each POST's own
    // REQ_TIMEOUT_SECS timeout on both threads (+ slack), so this cannot hang.
    let mut fail2: Option<String> = None;
    let deadline = std::time::Duration::from_secs(REQ_TIMEOUT_SECS + 10);
    let started = std::time::Instant::now();
    loop {
        let remaining = deadline.checked_sub(started.elapsed()).unwrap_or_default();
        match rx.recv_timeout(remaining) {
            Ok((tag, Attempt::Ok(c))) => {
                let key = if tag == 1 { k1 } else { k2 };
                return Some((key.to_string(), Ok(c)));
            }
            Ok((tag, att)) => {
                let slot = if tag == 1 { &mut fail1 } else { &mut fail2 };
                *slot = Some(reason_of(att));
                if fail1.is_some() && fail2.is_some() {
                    break;
                }
            }
            Err(_) => break, // both POSTs past their own timeout — give up
        }
    }
    Some((
        String::new(),
        Err((
            k1.to_string(),
            fail1.unwrap_or_else(|| "hedge timeout".into()),
            k2.to_string(),
            fail2.unwrap_or_else(|| "hedge timeout".into()),
        )),
    ))
}

/// The provider-fallback state machine, transport-agnostic:
///   - intra-retry the SAME (provider, model) up to `MAX_INTRA_RETRY` on 429;
///   - any other failure (or exhausted 429) → next chain entry, bump breaker;
///   - once a provider has `CB_TRIP_AFTER` failures its remaining models are
///     skipped (circuit open for the run);
///   - all entries exhausted → Err with the collected reasons.
/// `retry_delay_ms == 0` disables the backoff sleep (tests).
pub(crate) fn run_chain(
    chain: &[(&'static Provider, &'static str)],
    transport: &mut dyn FnMut(&Provider, &str) -> Attempt,
    retry_delay_ms: u64,
) -> Result<Completion, String> {
    use std::collections::HashMap;
    let mut breaker: HashMap<&str, u32> = HashMap::new();
    let mut reasons: Vec<String> = Vec::new();

    for &(p, model) in chain {
        if breaker.get(p.id).copied().unwrap_or(0) >= CB_TRIP_AFTER {
            continue; // circuit open for this run
        }
        let mut intra = 0u32;
        loop {
            match transport(p, model) {
                Attempt::Ok(c) => return Ok(c),
                Attempt::RateLimited if intra < MAX_INTRA_RETRY => {
                    intra += 1;
                    if retry_delay_ms > 0 {
                        sleep_ms(retry_delay_ms * intra as u64); // linear-ish backoff
                    }
                    continue;
                }
                Attempt::RateLimited => {
                    reasons.push(format!("{}/{model}: 429 (exhausted intra-retry)", p.id));
                    *breaker.entry(p.id).or_insert(0) += 1;
                    break;
                }
                Attempt::Failed(e) => {
                    reasons.push(format!("{}/{model}: {e}", p.id));
                    *breaker.entry(p.id).or_insert(0) += 1;
                    break;
                }
            }
        }
    }
    Err(format!(
        "all free providers failed:\n  {}",
        reasons.join("\n  ")
    ))
}
