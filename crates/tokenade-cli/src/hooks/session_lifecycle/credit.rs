//! **Measuring what the fold saved OVER TIME.**
//!
//! A fold does not give back its tokens once: it gives them back on every
//! turn that would have re-carried the context. The compounded saving
//! therefore has a horizon and a per-session cap, otherwise a long session
//! yields a figure nothing supports.

use super::*;

pub(crate) fn record_userprompt_estimates(parsed: &serde_json::Value) {
    let session_id = crate::hooks::session_id_of(parsed).unwrap_or("");
    let transcript_path = parsed
        .get("transcript_path")
        .and_then(|s| s.as_str())
        .unwrap_or("");
    if session_id.is_empty() || transcript_path.is_empty() {
        return;
    }
    let Some(home) = tokenade_core::platform::home_dir() else {
        return;
    };
    let est_state = tokenade_core::platform::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".tokenade")
        .join("sessions")
        .join(format!(
            "{}-upest.json",
            tokenade_core::dedup_ref::hash_content(transcript_path.as_bytes())
        ));
    let _p_est = tokenade_core::profile::phase("sessionstart:transcript_estimate");
    let parts = tokenade_core::claudecode_transcript::session_parts_cached(
        std::path::Path::new(transcript_path),
        &est_state,
    );
    let (total_out, total_batch) = (parts.output, parts.batching);
    let safe = sanitize_session_id(session_id);
    let dir = home.join(".tokenade").join("sessions");

    // Credit only the increase in `cumulative` since the last turn, scaled by
    // `factor` (1.0 for already-saved figures like batching; the lean ratio
    // for raw output). The cursor advances ONLY when we actually record, so a
    // sub-rounding delta (factor·Δ < 0.5) accumulates instead of being lost.
    let record_delta =
        |suffix: &str, op: &str, class: &str, cumulative: u64, factor: f64| -> usize {
            let state = dir.join(format!("{safe}-{suffix}"));
            let Some(grown) = cursor_growth(&state, cumulative) else {
                return 0;
            };
            let saved = (grown as f64 * factor).round() as usize;
            if saved == 0 {
                return 0;
            }
            gain_log::record_estimated(
                "hookwrap-userprompt",
                op,
                "",
                class,
                tokenade_core::Savings::new(saved, 0),
            );
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(&state, cumulative.to_string());
            saved
        };

    let out_saved = record_delta(
        "outacct",
        "output_lean",
        "output",
        total_out,
        tokenade_core::coefficients::output_lean_ratio(),
    );
    // The prose+thinking the model DIDN'T emit this turn also never re-enters
    // the live context as cache_read on every later turn — the exact same
    // compounding a folded tool output earns. Bank the output-lean delta into
    // the session's compaction log so `record_compounding_estimate` accrues its
    // `Σδ·(T−turn)` cache-class credit, bounded by the same horizon (real /
    // synthetic reset). Distinct cost from the one-time output-class credit
    // above (output price vs recurring cache_read price) — not double-counting.
    // `out_saved` counts BILLED tokens (the transcript's `output_tokens`); the
    // fold log counts `tokenizer::estimate` units, priced ×1.6 on Claude at
    // read time. Banked raw, every lean token compounded 1.6 times its size.
    if out_saved > 0 {
        let model = session_state::session_model(session_id);
        let per = tokenade_core::tokenizer::claude_tokens_per_estimated(&model);
        note_session_compaction(parsed, (out_saved as f64 / per).round() as usize);
    }
    record_delta("batchacct", "batching", "batching", total_batch, 1.0);
    // The context the native groups' spared calls would have re-read — billed
    // tokens, cache-priced, booked apart as `turns_context` (see
    // `claudecode_transcript::SessionParts`).
    let ctx_state = dir.join(format!("{safe}-ctxacct"));
    if let Some(grown) = cursor_growth(&ctx_state, parts.turns_context) {
        if grown > 0 {
            tokenade_core::gain::record_estimated_io(
                "hookwrap-userprompt",
                "turns_context",
                "native",
                "batching",
                "cache",
                tokenade_core::Savings::new(grown as usize, 0),
            );
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(&ctx_state, parts.turns_context.to_string());
        }
    }
}

// 0.6.4 — per-session turn counter. Bumped once per UserPromptSubmit; it's
// the clock the compounding math uses to know how many turns a compaction
// has survived. Keyed by session_id → a new session starts fresh.

/// 0.6.4 — bank a compaction for this session's COMPOUNDING credit. Each
/// compaction removes `saved` tokens that would otherwise be re-sent as
/// cache_read on every later turn; we record `{turn, delta}` now and resolve
/// `Σ delta·(T−turn)` at finalize. APPEND-ONLY (one JSONL line) rather than a
/// read-modify-write: this fires from the per-Read hook, which runs as a
/// SEPARATE PROCESS per tool call, and batching makes several fire CONCURRENTLY
/// in one turn — an append of <PIPE_BUF bytes is atomic, so concurrent writers
/// can't lose updates or tear a line. Keyed by session_id; no transcript read.
pub(crate) fn note_session_compaction(parsed: &serde_json::Value, saved: usize) {
    if saved == 0 {
        return;
    }
    let session_id = crate::hooks::session_id_of(parsed).unwrap_or("");
    if session_id.is_empty() {
        return;
    }
    note_compaction_for(&sanitize_session_id(session_id), saved);
}

/// [`note_session_compaction`] for a session known by its key — what `wrap`
/// has (`TOKENADE_SESSION`), which never sees the hook's JSON.
pub(crate) fn note_compaction_for(safe: &str, saved: usize) {
    tokenade_core::gain::bank_fold(safe, saved);
}

/// Compounding HORIZON (owner audit 2026-07-06): a folded-away token only
/// keeps "paying" (as an avoided cache_read) while the history segment that
/// contained it is STILL in the live context. Two things end that:
///
///   1. a REAL compaction (auto-compact / `/compact`) — the summariser
///      squashes the segment, so the counterfactual re-send stops. The
///      PreCompact hook observes these exactly → it appends a `reset` line
///      to the session's compaction log (see
///      [`note_session_compaction_reset`]).
///   2. a compaction that WOULD have happened in the counterfactual: had the
///      folded tokens actually been delivered, the context would have grown
///      past the auto-compact threshold EARLIER than the real (smaller)
///      context does. Once `delivered-since-last-reset + Σδ(live)` crosses
///      the model's threshold, we synthesize a reset ourselves.
///
/// A reset FREEZES the credit accrued so far (`base`) and expires the live
/// events: nothing banked before the reset accrues on later turns. Session
/// end needs no special case — credit is retrospective (only turns that
/// actually happened are counted), so it simply stops accruing.
///
/// Session end and "remaining turns" are inherently bounded: the formula
/// `Σδ·(T−turn)` counts only OBSERVED turns, never projected ones.
pub(crate) struct CompoundingFold {
    /// Frozen credit + live accrual at the current turn.
    pub cumulative: u64,
    /// Counterfactual context crossed the threshold → caller should append
    /// a reset marker so later turns stop crediting the expired events.
    pub synthetic_reset: bool,
    /// Σδ of folded content STILL RESIDENT in the (counterfactual) context at
    /// this turn (events since the last real reset, before any synthetic
    /// reset this turn expires them). A single turn's avoided re-send can
    /// never be worth more than this — it's the tokenade-folded slice of the
    /// context, not a whole window.
    pub resident_delta: u64,
}

/// Compounding HORIZON, in turns: a folded token stops paying after this many
/// turns even absent an observed compaction — the context would have been
/// compacted (or the fold aged out of the live window) well before. Bounds the
/// per-event multiplier `min(turn − turn_i, H)` so one early huge fold can't
/// accrue unbounded credit over a long session. Calibratable here.
#[cfg(test)]
pub(crate) const COMPOUNDING_HORIZON_MAX_TURNS: u64 = tokenade_core::gain::HORIZON_TURNS;

/// Session BACKSTOP multiplier: the cumulative compounding credit of a session
/// can never exceed `K ×` what tokenade actually compacted this session
/// (Σ banked δ) — the same bound the overhead re-bill carries
/// (`record_overhead_rebill`): **a token is re-read at most H times, on either
/// side of the ledger**.
///
/// It was `2 ×` banked AND one context window per session, against `12 ×` for
/// the re-bill. MEASURED 2026-10-05 on the 68 sessions of the last 30 days:
/// 4.21 M tokens folded, 24.56 M of re-reads under the per-fold horizon,
/// **1.15 M** credited — 14 sessions hit the cap, while the injected side of
/// the same sessions was charged up to its full 12×. A fold that stays in the
/// context for 12 turns is re-read 12 times, not 2; and a long session re-reads
/// more than one window in total. The per-event horizon and the compaction
/// resets in [`fold_compounding_log`] already bound the total by `H × banked`;
/// this backstop states it for the 377 M case.
#[cfg(test)]
pub(crate) const COMPOUNDING_SESSION_K: u64 = COMPOUNDING_HORIZON_MAX_TURNS;

/// Session-total compounding cap: each banked token re-read at most the
/// proxy's horizon.
pub(crate) fn compounding_session_cap(banked: u64, proxy: RereadProxy) -> u64 {
    banked.saturating_mul(proxy.horizon_calls)
}

/// **How many API calls re-read a token, when the transcript cannot say.**
///
/// Where the session's timeline is readable (Claude Code, Codex) re-reads are
/// counted call by call ([`session_timeline`]). Elsewhere only the user-turn
/// clock is known; a token was credited (and charged) at most 12 user turns,
/// while a user turn runs several API calls and a context holds hundreds of
/// calls before it compacts. A proxy measured on Claude Code, scaled to the
/// agent's window.
///
/// MEASURED 2026-10-05, 30 days of local Claude Code transcripts:
/// * API calls per user message, interactive sessions: p25 **6.4**, median
///   8.9, p75 **11.8** (`calls_per_turn.py`);
/// * context growth per API call between two compactions: p25 **1 022**,
///   median 1 175, p75 **1 429** billed tokens; 499 calls (median) between
///   two of the 279 compactions, which fired at 667 k (`growth.py`).
///
/// Calls before compaction ≈ threshold ÷ growth per call (median), of which a
/// token that lands mid-cycle is re-read a share: a tool result, 217 calls
/// (median) of the 499. ONE proxy for both sides, at the medians: the credit
/// used to take the low quartiles and the charge the high ones, so the same
/// token re-read in the same context was credited 233 times and charged 652
/// (at a 667 k threshold) — 2.8× apart for one quantity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RereadProxy {
    pub calls_per_turn: f64,
    pub horizon_calls: u64,
}

impl RereadProxy {
    /// The rule before the proxy: one re-read per user turn, 12 turns.
    #[cfg(test)]
    pub(crate) const PER_TURN: RereadProxy = RereadProxy {
        calls_per_turn: 1.0,
        horizon_calls: COMPOUNDING_HORIZON_MAX_TURNS,
    };

    /// At compaction threshold `threshold`, for a fold's credit and for the
    /// charge of injected context alike.
    pub(crate) fn measured(threshold: u64) -> Self {
        RereadProxy {
            calls_per_turn: 8.9,
            horizon_calls: ((threshold * 217) / (1_175 * 499)).max(1),
        }
    }

    /// Re-reads of a token `turns` user turns old.
    pub(crate) fn rereads(&self, turns: u64) -> u64 {
        ((turns as f64 * self.calls_per_turn) as u64).min(self.horizon_calls)
    }

    /// Re-reads in one user turn.
    pub(crate) fn per_turn(&self) -> u64 {
        (self.calls_per_turn.ceil() as u64).max(1)
    }
}

/// Pure fold over the append-only `<sid>-compactions.jsonl` log. Lines are
/// either events `{"turn":t,"delta":δ}` or resets
/// `{"turn":t,"reset":1,"delivered":bytes}`. `turn` = current turn clock,
/// `delivered_bytes` = session's cumulative delivered tool-output bytes
/// (≈4 bytes/token — same convention as [`session_pressure_factor`]),
/// `threshold_tokens` = the model's auto-compact threshold.
pub(crate) fn fold_compounding_log(
    log: &str,
    turn: u64,
    delivered_bytes: u64,
    threshold_tokens: u64,
    proxy: RereadProxy,
) -> CompoundingFold {
    let mut base = 0u64;
    // Live (unreset) events as (turn_banked, δ). Kept individually so the
    // per-event horizon `min(age, H)` can be applied — the old aggregate
    // `turn·Σδ − Σ(δ·turn)` form can't express a per-event age clamp.
    let mut live: Vec<(u64, u64)> = Vec::new();
    // Delivered counter value at the last reset — the counterfactual-growth
    // baseline restarts there (post-compaction the real context is small
    // again, so only NEW growth pushes toward the next compaction).
    let mut delivered_floor = 0u64;
    // Σ δ_i · min(eval − turn_i, H): each fold pays for at most H turns.
    let contrib = |live: &[(u64, u64)], eval: u64| -> u64 {
        live.iter().fold(0u64, |acc, &(ti, d)| {
            acc.saturating_add(d.saturating_mul(proxy.rereads(eval.saturating_sub(ti))))
        })
    };
    for line in log.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let t = v["turn"].as_u64().unwrap_or(0);
        if v["reset"].as_u64() == Some(1) {
            // Freeze accrual of everything banked so far AT the reset turn,
            // then expire it: those segments left the (counterfactual)
            // context, they never pay again.
            base = base.saturating_add(contrib(&live, t));
            live.clear();
            delivered_floor = v["delivered"].as_u64().unwrap_or(delivered_floor);
            continue;
        }
        let d = v["delta"].as_u64().unwrap_or(0);
        live.push((t, d));
    }
    // Folded content still resident (this episode's Σδ), captured BEFORE any
    // synthetic reset this turn expires it — it's what actually occupied the
    // counterfactual context this turn.
    let resident: u64 = live.iter().map(|&(_, d)| d).sum();
    // Counterfactual context size ≈ real delivered content since the last
    // (real or synthetic) compaction + everything tokenade folded away that
    // WOULD have been in it. Prompts/model output are ignored — that
    // under-counts the counterfactual, i.e. delays the synthetic reset,
    // which errs toward crediting (acceptable: the real PreCompact signal
    // catches the Claude Code case exactly; this guard is for manual
    // compaction and agents without a PreCompact hook).
    let counterfactual = (delivered_bytes.saturating_sub(delivered_floor)) / 4 + resident;
    let synthetic_reset = threshold_tokens > 0 && counterfactual > threshold_tokens;
    if synthetic_reset {
        // The counterfactual compaction would have run by now: freeze at the
        // CURRENT turn and expire the live events.
        base = base.saturating_add(contrib(&live, turn));
        live.clear();
    }
    CompoundingFold {
        cumulative: base.saturating_add(contrib(&live, turn)),
        synthetic_reset,
        resident_delta: resident,
    }
}

/// Append a reset line to the session's compaction log: an observed (real)
/// context compaction — the summariser is about to squash the history that
/// contained our folded segments, so their compounding credit must stop.
/// Called from the PreCompact hook. Append-only for the same cross-process
/// atomicity reason as [`note_session_compaction`]. No-op when the session
/// never banked a compaction (nothing to expire — and a later first event
/// accrues from its own turn anyway).
pub(crate) fn note_session_compaction_reset(session_id: &str) {
    if session_id.is_empty() {
        return;
    }
    let Some(home) = tokenade_core::platform::home_dir() else {
        return;
    };
    let dir = home.join(".tokenade").join("sessions");
    let safe = sanitize_session_id(session_id);
    let log = dir.join(format!("{safe}-compactions.jsonl"));
    if !log.exists() {
        return;
    }
    let read_ctr = |name: &str| session_state::session_counter(&dir, session_id, name);
    let (turn, delivered) = (read_ctr("turn"), read_ctr("delivered"));
    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&log) {
        use std::io::Write;
        let _ = f.write_all(
            format!("{{\"turn\":{turn},\"reset\":1,\"delivered\":{delivered}}}\n").as_bytes(),
        );
    }
}

/// 0.6.4 — credit the cache_read tokens saved by compaction COMPOUNDING.
/// At turn `T`: `Σ saved·(T−turn) = T·Σsaved − Σ(saved·turn)`, bounded by
/// the compounding HORIZON (see [`fold_compounding_log`]: credit stops at
/// each real/probable context compaction). These are cache_read-class
/// tokens (≈0.1× input price — the dashboard weights them, and
/// `price_cache_per_mtok` prices them), recorded raw here. Delta-based vs
/// what we already credited this session. `class=compounding`, estimated.
/// The session's call-by-call timeline, when its transcript can be read:
/// every API call with the context it sent, every compaction, the context at
/// which this session compacts (observed, else window × auto-compact share).
/// `None` for an agent whose transcript records none of this.
fn session_timeline(
    parsed: &serde_json::Value,
    dir: &std::path::Path,
    safe: &str,
    session_id: &str,
) -> Option<tokenade_core::gain::Timeline> {
    let transcript = parsed
        .get("transcript_path")
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty())?;
    let stl = tokenade_core::claudecode_transcript::session_timeline_cached(
        std::path::Path::new(transcript),
        &dir.join(format!("{safe}-timeline.json")),
    );
    if stl.calls.is_empty() {
        return None;
    }
    let model = parsed
        .get("model")
        .and_then(|m| m.as_str())
        .filter(|m| !m.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| session_state::session_model(session_id));
    let mut seen: Vec<u64> = stl
        .compactions
        .iter()
        .map(|&(_, pre)| pre)
        .filter(|&p| p > 0)
        .collect();
    seen.sort_unstable();
    let threshold = match seen.get(seen.len() / 2) {
        Some(&observed) => observed,
        None if stl.compact_at > 0 => stl.compact_at,
        None => tokenade_core::autocompact::compaction_threshold(
            tokenade_core::autocompact::estimator_window_for_model(&model),
            tokenade_core::autocompact::DEFAULT_PCT,
        ),
    };
    Some(
        tokenade_core::gain::Timeline {
            calls: stl.calls,
            compactions: stl.compactions.iter().map(|&(t, _)| t).collect(),
            threshold,
            billed_per_estimated: tokenade_core::tokenizer::claude_tokens_per_estimated(&model),
            rewrites: stl.rewrites,
            rewrite_weight: tokenade_core::gain::rewrite_weight(&model),
        }
        .with_inferred_compactions(),
    )
}

/// The context size at which a session without a readable timeline is
/// assumed to compact: its model's window (the estimator's, which never
/// over-states it) × the auto-compact share.
pub(crate) fn assumed_threshold(parsed: &serde_json::Value, session_id: &str) -> u64 {
    let model = parsed
        .get("model")
        .and_then(|m| m.as_str())
        .filter(|m| !m.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| session_state::session_model(session_id));
    tokenade_core::autocompact::compaction_threshold(
        tokenade_core::autocompact::estimator_window_for_model(&model),
        tokenade_core::autocompact::DEFAULT_PCT,
    )
}

pub(crate) fn record_compounding_estimate(parsed: &serde_json::Value) {
    let session_id = crate::hooks::session_id_of(parsed).unwrap_or("");
    if session_id.is_empty() {
        return;
    }
    let Some(home) = tokenade_core::platform::home_dir() else {
        return;
    };
    let dir = home.join(".tokenade").join("sessions");
    let safe = sanitize_session_id(session_id);
    let log_path = dir.join(format!("{safe}-compactions.jsonl"));
    // An in-flight session whose log was written under the previous spelling
    // must not forget its recorded compactions: a forgotten reset is exactly
    // what over-credits. Read the old name once, and keep writing the new one.
    let log = match std::fs::read_to_string(&log_path) {
        Ok(l) => l,
        Err(_) => {
            let legacy = session_state::legacy_session_id(session_id)
                .map(|l| dir.join(format!("{l}-compactions.jsonl")))
                .and_then(|p| std::fs::read_to_string(p).ok());
            let Some(l) = legacy else { return };
            l
        }
    };
    // Horizon threshold: the model's auto-compact point. Hook payloads
    // rarely carry a model id — the conservative 200k default applies then
    // (see estimator_window_for_model: savings claims must fail SMALL, the
    // opposite of the installer's fail-large default).
    // The payload first — it is authoritative when present. Then whatever the
    // statusline parked for this session, which is the only surface the host
    // tells. Empty leaves the conservative 200 000-token default in place:
    // unknown model still fails small, a KNOWN one is simply not guessed at.
    // Exact where the session's timeline is readable: re-reads counted call
    // by call up to the real or simulated compaction (`gain::rereads`).
    if let Some(tl) = session_timeline(parsed, &dir, &safe, session_id) {
        credit_exact(&safe, &log, &tl, "hookwrap-userprompt");
        return;
    }
    let threshold = assumed_threshold(parsed, session_id);
    compound_by_proxy(
        session_id,
        threshold,
        RereadProxy::measured(threshold),
        "hookwrap-userprompt",
    );
}

/// The proxy-priced credit for re-reads of this session's folds, one step,
/// on its turn clock (user messages — or tool calls, see [`tool_tick`]).
pub(crate) fn compound_by_proxy(
    session_id: &str,
    threshold: u64,
    proxy: RereadProxy,
    source: &str,
) {
    let Some(home) = tokenade_core::platform::home_dir() else {
        return;
    };
    let dir = home.join(".tokenade").join("sessions");
    let safe = sanitize_session_id(session_id);
    let read_ctr = |name: &str| session_state::session_counter(&dir, session_id, name);
    let turn = read_ctr("turn");
    let log_path = dir.join(format!("{safe}-compactions.jsonl"));
    let log = match std::fs::read_to_string(&log_path) {
        Ok(l) => l,
        Err(_) => {
            let legacy = session_state::legacy_session_id(session_id)
                .map(|l| dir.join(format!("{l}-compactions.jsonl")))
                .and_then(|p| std::fs::read_to_string(p).ok());
            let Some(l) = legacy else { return };
            l
        }
    };
    let fold = fold_compounding_log(&log, turn, read_ctr("delivered"), threshold, proxy);
    if fold.synthetic_reset {
        // Persist the synthetic reset so later turns fold the same horizon
        // (and don't re-detect it against a stale delivered floor).
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&log_path) {
            use std::io::Write;
            let _ = f.write_all(
                format!(
                    "{{\"turn\":{turn},\"reset\":1,\"delivered\":{}}}\n",
                    read_ctr("delivered")
                )
                .as_bytes(),
            );
        }
    }
    // SESSION BACKSTOP: the cumulative compounding of a whole session can
    // never exceed K× what tokenade actually folded this session (Σ banked δ),
    // nor one model context window. Even across several compaction cycles a
    // long session's compounding stays a small multiple of its real
    // compaction — this is what makes a 377M figure impossible.
    let banked: u64 = log
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| v["delta"].as_u64())
        .sum();
    let cumulative = fold.cumulative.min(compounding_session_cap(banked, proxy));
    let recorded = dir.join(format!("{safe}-comprec"));
    let prev: u64 = std::fs::read_to_string(&recorded)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    if cumulative <= prev {
        return;
    }
    // Per-turn ceiling: one prompt credits at most one turn's avoided re-send,
    // and that can never exceed the tokenade-folded content actually RESIDENT
    // this turn (Σδ_live) — NOT a whole context window (the old `.min(threshold)`
    // wrongly assumed the entire context was tokenade folding). Excess is
    // DROPPED, not deferred — `recorded` still advances to `cumulative`.
    let per_turn_cap = fold.resident_delta.max(1).saturating_mul(proxy.per_turn());
    let delta = (cumulative - prev).min(per_turn_cap) as usize;
    gain_log::record_estimated(
        source,
        "compounding",
        tokenade_core::gain::PROXY_LABEL,
        "compounding",
        tokenade_core::Savings::new(delta, 0),
    );
    let _ = std::fs::write(&recorded, cumulative.to_string());
}

/// Where a session's cumulative overhead re-bill is remembered, so the
/// horizon below can be enforced across turns.
pub(crate) fn overhead_rebilled_path(skey: &str) -> std::path::PathBuf {
    tokenade_core::platform::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".tokenade")
        .join("sessions")
        .join(format!("{skey}-overhead-rebilled"))
}

/// A session's overhead re-bill, bounded the way its credit already is.
///
/// SYMMETRY (owner question 2026-07-04): injected context re-bills as
/// cache_read on every later turn exactly like avoided content COMPOUNDS as
/// credit. Each turn books the session's accumulated injected-overhead once as
/// an io=cache overhead row (server weighted SQL: sign -1 x weight 0.1;
/// client: overhead_cache/10 subtracted).
///
/// THE BOUND, and why it was missing (owner report 2026-08-03: "overhead_rebill
/// looks gigantic, it is the largest negative term for many customers"). The
/// symmetry was only half implemented. The credit side carries a HORIZON — a
/// folded token stops paying after [`COMPOUNDING_HORIZON_MAX_TURNS`] turns,
/// added after a 377 M pathological session. The overhead side carried none, so
/// re-billing the running total every turn made the charge TRIANGULAR in the
/// number of turns while the credit stayed linear. Measured on a real ledger:
/// a session that injected **62 k** tokens in total was charged **11.4 M** —
/// 183x, and enough to dominate every other term.
///
/// Same rule as the credit, stated once: **a token pays at most H times.** So
/// the session's cumulative re-bill can never exceed `H x injected`, and each
/// turn books only what is left of that budget. Linear in turns, like the
/// credit it mirrors, and it converges instead of diverging.
pub(crate) fn record_overhead_rebill(parsed: &serde_json::Value) {
    let Some(sid) = crate::hooks::session_id_of(parsed) else {
        return;
    };
    let safe: String = sid
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(40)
        .collect();
    if safe.is_empty() {
        return;
    }
    // Latency: summing a per-session counter file (written by the same
    // process that books the injection) replaces a full gain_estimated
    // scan per prompt — the same growing-file tax the transcript
    // estimator just shed.
    // Exact where the session's timeline is readable: each injection paid
    // once per later call, up to the REAL compaction (`gain::rereads`).
    if let Some(home) = tokenade_core::platform::home_dir() {
        let dir = home.join(".tokenade").join("sessions");
        if let Some(tl) = session_timeline(parsed, &dir, &safe, sid) {
            charge_exact(&safe, &tl, "hookwrap-userprompt");
            return;
        }
    }
    // An agent whose tool calls tokenade sees is charged per CALL
    // (`record_tool_tick`); a second, per-message step would charge twice.
    if dir_ticked(&safe) {
        return;
    }
    rebill_resident(
        &safe,
        RereadProxy::measured(assumed_threshold(parsed, sid)),
        "hookwrap-userprompt",
    );
}

/// Session `session_id`'s fold log, empty when it folded nothing yet.
pub(crate) fn fold_log_of(session_id: &str) -> String {
    let p = tokenade_core::session::sessions_dir().join(format!(
        "{}-compactions.jsonl",
        sanitize_session_id(session_id)
    ));
    std::fs::read_to_string(p).unwrap_or_default()
}

/// The exact credit for session `safe`'s folds (`fold_log`, its
/// `-compactions.jsonl`), counted on its call timeline `tl`: what it grew by
/// since the last count is booked.
pub(crate) fn credit_exact(
    safe: &str,
    fold_log: &str,
    tl: &tokenade_core::gain::Timeline,
    source: &str,
) {
    let folds: Vec<(u64, u64)> = fold_log
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| Some((v["ts"].as_u64()?, v["delta"].as_u64()?)))
        .collect();
    let cumulative = tokenade_core::gain::fold_rereads(&folds, tl);
    // `-v2`: the count changed on 2026-10-15 (inferred compactions, first
    // send not re-counted). A cursor left by the old count would hold back
    // — or, the other way, re-credit — everything up to its value; a fresh
    // one takes the new count from here on, nothing before it.
    let cursor = tokenade_core::session::sessions_dir().join(format!("{safe}-comprec-exact-v2"));
    if let Some(grown) = cursor_growth(&cursor, cumulative).filter(|&g| g > 0) {
        gain_log::record_estimated(
            source,
            "compounding",
            tokenade_core::gain::EXACT_LABEL,
            "compounding",
            tokenade_core::Savings::new(grown as usize, 0),
        );
        let _ = std::fs::write(&cursor, cumulative.to_string());
    }
}

/// The exact charge for what tokenade injected into session `safe`, counted
/// on its call timeline `tl`.
pub(crate) fn charge_exact(safe: &str, tl: &tokenade_core::gain::Timeline, source: &str) {
    let cumulative = tokenade_core::gain::injected_rereads(&injected_log(safe), tl);
    // `-v2`: see the credit's cursor — same change, same reason.
    let cursor = tokenade_core::session::sessions_dir().join(format!("{safe}-rebill-exact-v2"));
    if let Some(grown) = cursor_growth(&cursor, cumulative).filter(|&g| g > 0) {
        gain_log::record_estimated_io(
            source,
            "overhead_rebill",
            tokenade_core::gain::EXACT_LABEL,
            "overhead",
            "cache",
            tokenade_core::Savings::new(grown as usize, 0),
        );
        let _ = std::fs::write(&cursor, cumulative.to_string());
    }
}

/// The session's injection log: `ts n` lines, `ts n p` for what the agent
/// reloads into every call (its rules file).
fn injected_log(safe: &str) -> Vec<(u64, u64, bool)> {
    std::fs::read_to_string(tokenade_core::session::session_injected_log_path(safe))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some((
                it.next()?.parse().ok()?,
                it.next()?.parse().ok()?,
                it.next() == Some("p"),
            ))
        })
        .collect()
}

/// One proxy-priced step of the charge for the injected context still
/// resident in session `safe`.
pub(crate) fn rebill_resident(safe: &str, proxy: RereadProxy, source: &str) {
    let injected = overhead_counter_sum(safe);
    if injected == 0 {
        return;
    }
    let compacted_at = std::fs::read_to_string(crate::hooks::replay::compact_ts_path(safe))
        .ok()
        .and_then(|t| t.trim().parse::<u64>().ok());
    // What the agent reloads after a compaction (its rules file) stays in
    // the next window: the base the compaction sets leaves it out.
    let persistent: u64 = injected_log(safe).iter().filter(|e| e.2).map(|e| e.1).sum();
    let at_compaction = std::fs::read_to_string(overhead_at_compaction_path(safe))
        .ok()
        .and_then(|t| t.trim().parse::<u64>().ok())
        .map(|a| a.saturating_sub(persistent));
    let rebilled_path = overhead_rebilled_path(safe);
    let record = std::fs::read_to_string(&rebilled_path)
        .ok()
        .and_then(|t| parse_rebill_record(&t));
    let step = rebill_step(injected, record, compacted_at, at_compaction, proxy);
    if step.book == 0 {
        return;
    }
    let now = tokenade_core::platform::now_unix_secs();
    if let Some(dir) = rebilled_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(
        &rebilled_path,
        format!("{now}:{}:{}", step.already, step.base),
    );
    gain_log::record_estimated_io(
        source,
        "overhead_rebill",
        tokenade_core::gain::PROXY_LABEL,
        "overhead",
        "cache",
        tokenade_core::Savings::new(step.book as usize, 0),
    );
}

/// Sum of the session's injected-overhead counter.
fn overhead_counter_sum(skey: &str) -> u64 {
    std::fs::read_to_string(tokenade_core::session::session_overhead_counter_path(skey))
        .map(|t| t.lines().filter_map(|l| l.trim().parse::<u64>().ok()).sum())
        .unwrap_or(0)
}

/// The overhead counter at the time of the last compaction: what was injected
/// BEFORE left the window along with the summary.
pub(crate) fn overhead_at_compaction_path(skey: &str) -> std::path::PathBuf {
    tokenade_core::platform::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".tokenade")
        .join("sessions")
        .join(format!("{skey}-ovh-at-compact"))
}

/// PreCompact: freezes the baseline of the next window.
pub(crate) fn note_overhead_at_compaction(skey: &str) {
    let p = overhead_at_compaction_path(skey);
    tokenade_core::platform::ensure_parent_dir(&p);
    let _ = tokenade_core::paths::write_atomic(&p, &overhead_counter_sum(skey).to_string());
}

/// `at:already[:base]` — `base` is absent from records written before 1.1.20.
pub(crate) fn parse_rebill_record(t: &str) -> Option<(u64, u64, u64)> {
    let mut it = t.trim().split(':');
    let at = it.next()?.parse().ok()?;
    let already = it.next()?.parse().ok()?;
    let base = it.next().and_then(|b| b.parse().ok()).unwrap_or(0);
    Some((at, already, base))
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RebillStep {
    pub book: u64,
    pub already: u64,
    pub base: u64,
}

/// One re-billing step, pure.
///
/// **A compaction EXPIRES what was injected before it** — the same rule as
/// the credit ([`fold_compounding_log`]: a reset freezes and expires the
/// events). The previous version reset the budget against the TOTAL injected
/// since the start of the session: a long session re-paid `H ×` its whole
/// history on every window. Measured 2026-09-28 on a real ledger: 83 M
/// re-billed for 1.08 M injected (×77 against a ×12 bound); one session with
/// 98 compactions: 34.9 M for 110 928 injected.
///
/// The window only re-bills `injected − base`, `base` = the counter at
/// compaction. Without that reading (compaction before 1.1.20), the baseline
/// is the current total: at most we lose the injections between the
/// compaction and this turn, we never re-bill the history.
pub(crate) fn rebill_step(
    injected: u64,
    record: Option<(u64, u64, u64)>,
    compacted_at: Option<u64>,
    at_compaction: Option<u64>,
    proxy: RereadProxy,
) -> RebillStep {
    let (mut already, mut base) = match record {
        Some((at, already, base)) => match compacted_at {
            // A record older than the compaction belongs to a window that no
            // longer exists.
            Some(c) if c >= at => (0, at_compaction.unwrap_or(injected).min(injected)),
            _ => (already, base),
        },
        None => (
            0,
            compacted_at.and(at_compaction).unwrap_or(0).min(injected),
        ),
    };
    base = base.min(injected);
    let window = injected - base;
    let budget = window.saturating_mul(proxy.horizon_calls);
    let step = (window as f64 * proxy.calls_per_turn).round() as u64;
    let book = budget.saturating_sub(already).min(step.max(1));
    already = already.saturating_add(book);
    RebillStep {
        book,
        already,
        base,
    }
}

#[cfg(test)]
mod rebill_tests {
    use super::*;

    /// **One writer of the fold log.** `book_resume_credit` wrote its own line,
    /// without the time the exact re-read count needs, and every resume trim
    /// went uncredited on Claude Code with all tests green. Every line
    /// goes through `note_compaction_for`; only this file may spell the path.
    #[test]
    fn only_this_file_spells_the_fold_log() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let files = tokenade_core::source_guard::rust_sources(&src);
        assert!(files.len() > 100, "only {} file(s) swept", files.len());
        for (p, code) in &files {
            if p.ends_with("hooks/session_lifecycle/credit.rs") {
                continue;
            }
            for (n, line) in code.lines().enumerate() {
                assert!(
                    !(line.contains("-compactions.jsonl") && !line.trim_start().starts_with("//")),
                    "{}:{} names the fold log — bank through `note_session_compaction` / `note_compaction_for`",
                    p.display(),
                    n + 1
                );
            }
        }
    }

    /// The proxy at a 1 M window (667 k threshold, as observed): a fold is
    /// credited, and injected context charged, 8.9 calls per user turn up to
    /// 246 calls — one count for both sides.
    #[test]
    fn the_proxy_counts_calls_not_user_turns() {
        let proxy = RereadProxy::measured(667_000);
        assert_eq!((proxy.rereads(10), proxy.rereads(100)), (89, 246));
        let f = fold_compounding_log("{\"turn\":1,\"delta\":1000}\n", 11, 0, 667_000, proxy);
        assert_eq!(f.cumulative, 89_000);
        let (mut rec, mut total, mut steps) = (None, 0, 0);
        loop {
            let s = rebill_step(1000, rec, None, None, proxy);
            if s.book == 0 {
                break;
            }
            assert!(s.book <= 8_900);
            total += s.book;
            steps += 1;
            rec = Some((1, s.already, s.base));
        }
        assert_eq!((total, steps), (246_000, 28));
    }

    const H: u64 = COMPOUNDING_HORIZON_MAX_TURNS;

    #[test]
    fn one_window_pays_at_most_h_times() {
        let mut rec = None;
        let mut total = 0;
        for _ in 0..40 {
            let s = rebill_step(1000, rec, None, None, RereadProxy::PER_TURN);
            total += s.book;
            rec = Some((1, s.already, s.base));
        }
        assert_eq!(total, H * 1000);
    }

    #[test]
    fn a_compaction_expires_what_was_injected_before_it() {
        // 12 turns paid on 1000, compaction, 50 injected afterwards.
        let s = rebill_step(
            1050,
            Some((10, H * 1000, 0)),
            Some(20),
            Some(1000),
            RereadProxy::PER_TURN,
        );
        assert_eq!(
            s,
            RebillStep {
                book: 50,
                already: 50,
                base: 1000
            }
        );
    }

    #[test]
    fn an_old_record_without_base_still_parses() {
        assert_eq!(
            parse_rebill_record("1790629763:1331136"),
            Some((1790629763, 1331136, 0))
        );
        assert_eq!(parse_rebill_record("5:7:9"), Some((5, 7, 9)));
    }

    #[test]
    fn a_compaction_without_its_counter_reading_rebills_no_history() {
        let s = rebill_step(
            110_928,
            Some((10, H * 110_928, 0)),
            Some(20),
            None,
            RereadProxy::PER_TURN,
        );
        assert_eq!(s.book, 0, "history must not be re-billed");
    }

    /// The real ledger of 2026-09-28: 98 compactions, 110 928 injected,
    /// 34.9 M re-billed. A long session, injecting on every turn and compacting
    /// regularly, can no longer exceed `H ×` what it injected — the rule "a
    /// token pays at most H times".
    #[test]
    fn a_long_session_with_many_compactions_stays_under_h_times_injected() {
        let (mut injected, mut total) = (0u64, 0u64);
        let (mut rec, mut compacted_at, mut at_compaction) = (None, None, None);
        for turn in 1..=1200u64 {
            injected += 90;
            if turn % 12 == 0 {
                compacted_at = Some(turn * 10 + 5);
                at_compaction = Some(injected);
            }
            let s = rebill_step(
                injected,
                rec,
                compacted_at,
                at_compaction,
                RereadProxy::PER_TURN,
            );
            total += s.book;
            rec = Some((turn * 10 + 9, s.already, s.base));
        }
        assert!(total <= H * injected, "{total} > {}", H * injected);
    }
}
