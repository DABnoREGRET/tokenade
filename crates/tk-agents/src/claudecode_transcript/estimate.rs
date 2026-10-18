//! **How much did this session cost?** — and the cache that avoids asking
//! again.
//!
//! A transcript grows endlessly while a session lives. Re-reading the whole
//! file on every call is the cost this module exists to avoid:
//! the estimate remembers the OFFSET already read and only sweeps the rest, under the
//! [`MAX_SCAN_BYTES_PER_CALL`] ceiling.
//!
//! The ceiling is not a stylistic precaution: a truncated or
//! rewritten file would make every dashboard loop iteration sweep
//! megabytes.

use super::*;

/// 0.6.4 — estimate the tokens saved by BATCHING (parallel tool calls) in a
/// single session transcript. With the zero-MCP pivot tokenade only *nudges*
/// the agent to batch; Claude Code does it natively by emitting several
/// `tool_use` blocks in one assistant message. So we credit it post-hoc:
/// when K≥2 tool calls share one message, the K results all land in one turn
/// instead of K sequential turns. Sequentially, result `i` (size `sᵢ`) would
/// be re-sent as context in each of the `K−i` later turns — so the avoided
/// re-send is `Σ sᵢ·(K−i)` (cache_read tokens). Sizes come from the matching
/// `tool_result` bodies, resolved by `tool_use_id`. Best-effort: anything
/// unparseable contributes 0.
pub fn batching_saved_tokens(transcript_path: &Path) -> u64 {
    session_estimate(transcript_path).1
}

/// One-pass scan returning `(output_tokens, batching_saved)` — the two
/// per-turn 0.6.4 estimate inputs. Combined into a SINGLE read because the
/// UserPromptSubmit hook needs both every turn, and re-reading a multi-MB
/// transcript twice per prompt adds avoidable latency. `output_tokens` = Σ
/// `message.usage.output_tokens` over assistant messages; `batching_saved` =
/// Σ sᵢ·(K−i) over every K≥2 multi-`tool_use` message (model above).
/// Best-effort: an unreadable file or malformed line contributes 0.
/// Offset-cached wrapper: re-scanning the WHOLE transcript on every
/// UserPromptSubmit cost p50=228ms and grew without bound (latency
/// audit). State file holds {off,out,bat}; only the tail since `off` is
/// parsed. A tool_use group torn across the boundary loses its batching
/// credit — negligible for an estimate. Truncated/rotated transcript
/// (len < off) → full rescan from zero.
/// Max transcript bytes parsed in a single `session_estimate_cached` call.
/// Caps the worst case (a full rescan after a cursor reset) so the
/// UserPromptSubmit hook stays well under Claude Code's 30s timeout; the
/// remainder is consumed incrementally on subsequent turns.
pub(crate) const MAX_SCAN_BYTES_PER_CALL: u64 = 16 * 1024 * 1024;

pub fn session_estimate_cached(transcript_path: &Path, state_path: &Path) -> (u64, u64) {
    let p = session_parts_cached(transcript_path, state_path);
    (p.output, p.batching)
}

/// [`session_estimate_cached`] with every part, the context term included.
pub fn session_parts_cached(transcript_path: &Path, state_path: &Path) -> SessionParts {
    let len = std::fs::metadata(transcript_path)
        .map(|m| m.len())
        .unwrap_or(0);
    let st: serde_json::Value = std::fs::read_to_string(state_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(serde_json::json!({}));
    let (mut off, mut out, mut bat, mut ctx) = (
        st["off"].as_u64().unwrap_or(0),
        st["out"].as_u64().unwrap_or(0),
        st["bat"].as_u64().unwrap_or(0),
        st["ctx"].as_u64().unwrap_or(0),
    );
    if off > len {
        (off, out, bat, ctx) = (0, 0, 0, 0);
    }
    if len > off {
        use std::io::{Read as _, Seek as _};
        if let Ok(mut f) = std::fs::File::open(transcript_path) {
            let _ = f.seek(std::io::SeekFrom::Start(off));
            // Bound the work per call: a cold/reset scan (off just reset to 0
            // because the transcript shrank — e.g. a `/resume` or trim) would
            // otherwise re-read the WHOLE file from zero, synchronously, on
            // this UserPromptSubmit hook. On a large session that single scan
            // blows Claude Code's 30s hook timeout and the preamble is
            // discarded (field report 2026-07-14). Cap the bytes parsed per
            // turn; the cursor advances only by what we consumed, so the tail
            // is picked up on the next few prompts. This is an ESTIMATE, so a
            // brief transient undercount while it catches up is acceptable — a
            // hung hook is not.
            let to_read = (len - off).min(MAX_SCAN_BYTES_PER_CALL);
            // Read raw bytes (not read_to_string): a 16 MiB cap can split a
            // multi-byte UTF-8 char and make read_to_string error out, losing
            // the whole window. Newlines are ASCII, so truncating to the last
            // one is always a valid boundary; lossy-decode the complete-lines
            // prefix (fine for an estimate).
            let mut raw = Vec::new();
            if f.take(to_read).read_to_end(&mut raw).is_ok() {
                let end = raw
                    .iter()
                    .rposition(|&b| b == b'\n')
                    .map(|i| i + 1)
                    .unwrap_or(0);
                let text = String::from_utf8_lossy(&raw[..end]);
                let d = estimate_parts(text.lines());
                out = out.saturating_add(d.output);
                bat = bat.saturating_add(d.batching);
                ctx = ctx.saturating_add(d.turns_context);
                // Forward progress even if a single line exceeds the cap (no
                // newline in a full-cap window): skip past it rather than
                // re-reading the same bytes every turn forever.
                off += if end == 0 && raw.len() as u64 == to_read {
                    to_read
                } else {
                    end as u64
                };
            }
        }
    }
    if let Some(p) = state_path.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    let _ = std::fs::write(
        state_path,
        serde_json::json!({"off": off, "out": out, "bat": bat, "ctx": ctx}).to_string(),
    );
    SessionParts {
        output: out,
        batching: bat,
        turns_context: ctx,
    }
}

pub fn session_estimate(transcript_path: &Path) -> (u64, u64) {
    use std::io::BufRead;
    let Ok(file) = std::fs::File::open(transcript_path) else {
        return (0, 0);
    };
    let lines: Vec<String> = std::io::BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .collect();
    estimate_from_lines(lines.iter().map(|s| s.as_str()))
}

pub(crate) fn estimate_from_lines<'a>(lines: impl Iterator<Item = &'a str>) -> (u64, u64) {
    let p = estimate_parts(lines);
    (p.output, p.batching)
}

/// What one scan of a transcript credits.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SessionParts {
    /// Output-lean base (billed output tokens of prose and thinking).
    pub output: u64,
    /// Native batching: results not re-sent (estimate units).
    pub batching: u64,
    /// Native batching: the context each avoided call would have re-read,
    /// read off the batching message's `usage` (BILLED tokens).
    pub turns_context: u64,
}

pub(crate) fn estimate_parts<'a>(lines: impl Iterator<Item = &'a str>) -> SessionParts {
    use std::collections::HashMap;
    let mut output = 0u64;
    // id → result size (filled as we stream; results follow their tool_use).
    let mut result_size: HashMap<String, u64> = HashMap::new();
    // each assistant message → its ordered tool_use ids. Claude Code writes
    // ONE record per content block, all sharing `message.id`: grouping per
    // record found no batch at all (2 433 batched messages in 200 real
    // sessions, 0 records with two `tool_use`), so the ids are gathered by
    // message, and before the usage dedup — which rightly admits only the
    // first record of a message for its OUTPUT, not for its calls.
    let mut groups: Vec<Vec<String>> = Vec::new();
    let mut group_of: HashMap<String, usize> = HashMap::new();
    // THE SERVER PATH. This total becomes an `output_lean` / `batching` event in
    // `gain_estimated.jsonl`, which earlier versions also reported upstream — so
    // the same repeated-entry inflation that tripled the dashboard was also
    // inflating what every installation reported. Dedup here too, and for the
    // same reason: one API response, one count. Fixed forward only; no
    // retroactive correction of what was already sent.
    let mut dedup = crate::usage_dedup::UsageDedup::new();
    // tool_use id → its group; and each tokenade batching reminder, by the
    // call it was attached to.
    let mut call_group: HashMap<String, usize> = HashMap::new();
    // group → the context its message was sent with.
    let mut group_ctx: HashMap<usize, u64> = HashMap::new();
    let mut nudges: Vec<(String, Reminder)> = Vec::new();
    for line in lines {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match v["type"].as_str() {
            Some("attachment") => {
                let a = &v["attachment"];
                let said = format!(
                    "{}{}",
                    a["stdout"].as_str().unwrap_or(""),
                    a["content"].as_str().unwrap_or("")
                );
                if let (Some(id), Some(family)) = (a["toolUseID"].as_str(), reminder_of(&said)) {
                    nudges.push((id.to_string(), family));
                }
            }
            Some("assistant") => {
                let calls = v["message"]["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|b| b["type"].as_str() == Some("tool_use"));
                let calls: Vec<String> = calls
                    .filter_map(|b| b["id"].as_str().map(str::to_string))
                    .collect();
                if !calls.is_empty() {
                    let g = match v["message"]["id"].as_str() {
                        Some(mid) => *group_of.entry(mid.to_string()).or_insert_with(|| {
                            groups.push(Vec::new());
                            groups.len() - 1
                        }),
                        None => {
                            groups.push(Vec::new());
                            groups.len() - 1
                        }
                    };
                    for id in calls {
                        if !groups[g].contains(&id) {
                            call_group.insert(id.clone(), g);
                            groups[g].push(id);
                        }
                    }
                }
                if let Some(&g) = v["message"]["id"]
                    .as_str()
                    .and_then(|mid| group_of.get(mid))
                {
                    let u = &v["message"]["usage"];
                    let ctx: u64 = [
                        "input_tokens",
                        "cache_read_input_tokens",
                        "cache_creation_input_tokens",
                    ]
                    .iter()
                    .map(|k| u[k].as_u64().unwrap_or(0))
                    .sum();
                    group_ctx.entry(g).or_insert(ctx);
                }
                if !dedup.admit(
                    "session",
                    v["message"]["id"].as_str(),
                    &v["message"]["usage"],
                ) {
                    continue;
                }
                let usage_out = v["message"]["usage"]["output_tokens"].as_u64().unwrap_or(0);
                if let Some(content) = v["message"]["content"].as_array() {
                    // The output-lean nudge only shrinks the model's PROSE +
                    // thinking, never its `tool_use` (function-call) blocks —
                    // those are dictated by the task, not reducible by "keep it
                    // terse". So the output-lean BASE must exclude the tool_use
                    // share of this message's `output_tokens`. `usage` gives no
                    // per-block split, so prorate by content-byte fraction:
                    // tool_use bytes = its `name` + serialized `input` JSON.
                    let (mut tt_bytes, mut tu_bytes) = (0u64, 0u64);
                    for b in content {
                        match b["type"].as_str() {
                            Some("text") => {
                                tt_bytes = tt_bytes.saturating_add(
                                    b["text"].as_str().map_or(0, |s| s.len() as u64),
                                )
                            }
                            Some("thinking") => {
                                tt_bytes = tt_bytes.saturating_add(
                                    b["thinking"].as_str().map_or(0, |s| s.len() as u64),
                                )
                            }
                            Some("tool_use") => {
                                tu_bytes = tu_bytes
                                    .saturating_add(
                                        b["name"].as_str().map_or(0, |s| s.len() as u64),
                                    )
                                    .saturating_add(b["input"].to_string().len() as u64)
                            }
                            _ => {}
                        }
                    }
                    let denom = tt_bytes.saturating_add(tu_bytes);
                    let credited = if denom == 0 {
                        usage_out
                    } else {
                        ((usage_out as u128 * tt_bytes as u128) / denom as u128) as u64
                    };
                    output = output.saturating_add(credited);
                } else {
                    // A usage-only assistant record (no content array) still
                    // carries output tokens and has no tool_use to exclude.
                    output = output.saturating_add(usage_out);
                }
            }
            Some("user") => {
                let Some(content) = v["message"]["content"].as_array() else {
                    continue;
                };
                for b in content {
                    if b["type"].as_str() != Some("tool_result") {
                        continue;
                    }
                    let Some(id) = b["tool_use_id"].as_str() else {
                        continue;
                    };
                    // content is a string or an array of {type:text,text:…}.
                    let text = match &b["content"] {
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Array(parts) => parts
                            .iter()
                            .filter_map(|p| p["text"].as_str())
                            .collect::<Vec<_>>()
                            .join(""),
                        other => other.to_string(),
                    };
                    result_size.insert(id.to_string(), crate::tokenizer::estimate(&text) as u64);
                }
            }
            _ => {}
        }
    }
    // A parallel group is credited by its distance from the LAST tokenade
    // reminder before it — see [`attributable_share`]; none before ⇒ 0.
    let mut reminded: HashMap<usize, Reminder> = HashMap::new();
    for (id, family) in &nudges {
        if let Some(&j) = call_group.get(id) {
            reminded.insert(j, *family);
        }
    }
    let mut weight = vec![0f64; groups.len()];
    let mut last: Option<(usize, Reminder)> = None;
    for (g, w) in weight.iter_mut().enumerate() {
        if let Some((j, family)) = last {
            *w = attributable_share(family, g - j);
        }
        if let Some(&family) = reminded.get(&g) {
            last = Some((g, family));
        }
    }
    let mut batching = 0u64;
    let mut turns_context = 0u64;
    for (g, ids) in groups
        .iter()
        .enumerate()
        .filter(|(g, ids)| ids.len() >= 2 && weight[*g] > 0.0)
    {
        let k = ids.len();
        let mut saved = 0u64;
        for (i, id) in ids.iter().enumerate() {
            // result i is spared re-transmission in the (K-1-i) later turns.
            let later = (k - 1 - i) as u64;
            if later == 0 {
                continue;
            }
            let s = result_size.get(id).copied().unwrap_or(0);
            saved = saved.saturating_add(s.saturating_mul(later));
        }
        batching = batching.saturating_add((saved as f64 * weight[g]).round() as u64);
        // Each of the k−1 calls the group spared would have re-read the whole
        // context (the `turns_context` term, here for native groups).
        let ctx = group_ctx.get(&g).copied().unwrap_or(0);
        turns_context =
            turns_context.saturating_add(((k - 1) as f64 * ctx as f64 * weight[g]).round() as u64);
    }
    SessionParts {
        output,
        batching,
        turns_context,
    }
}

#[derive(Clone, Copy)]
enum Reminder {
    Commands,
    Reads,
    Edits,
}

/// Which tokenade batching reminder `said` is (`hooks::nudges`, whose English
/// wording is fixed), if any.
fn reminder_of(said: &str) -> Option<Reminder> {
    if !said.contains("[tokenade]") {
        None
    } else if said.contains("commands one-per-turn") {
        Some(Reminder::Commands)
    } else if said.contains("files one-per-turn") {
        Some(Reminder::Reads)
    } else if said.contains("Edits one-per-turn") {
        Some(Reminder::Edits)
    } else {
        None
    }
}

/// **The share of a parallel group that tokenade's last reminder caused**, by
/// how many tool-calling turns ago it came: `1 − baseline / observed`, i.e.
/// `1 − 1/RR` where RR is how much likelier batching is at that distance.
///
/// Every parallel tool call used to be credited to tokenade; it is mostly the
/// agent's own habit. MEASURED 2026-10-05 on 30 days of local transcripts
/// (`target/probe/nudge_lag.py`): the rate of parallel turns at distance `d`
/// after a reminder, against the rate at the same distance after a streak
/// position where none fired — the reminder fires on a sequential streak, so
/// "before" would only measure regression to the mean.
///
/// | after a reminder | d = 1 | 2–10 | 11–20 | 21–50 | 51–200 |
/// |---|---|---|---|---|---|
/// | commands (RR) | 0.81 (5.4) | 0.74 (≈3.9) | 0.68 (3.1) | 0.47 (1.9) | 0.22 (≈1.3) |
/// | Reads | 0.80 | 0.47 | 0.54 | 0.31 | 0.36 |
/// | Edits | 0.51 | 0.37 | 0.33 | 0.33 | 0.39 |
///
/// Commands rest on ~1 500 reminders (±0.3–0.7 points per distance), Reads and
/// Edits on 110–170 (±2.5–4.6 points at short distance), hence buckets rather
/// than a turn-by-turn curve. Beyond 200 turns nothing was measured; the
/// 51–200 plateau is kept. A group before any reminder gets nothing: no
/// reminder, no cause.
fn attributable_share(after: Reminder, d: usize) -> f64 {
    let b = match d {
        0 | 1 => 0,
        2..=10 => 1,
        11..=20 => 2,
        21..=50 => 3,
        _ => 4,
    };
    let table = match after {
        Reminder::Commands => [0.81, 0.74, 0.68, 0.47, 0.22],
        Reminder::Reads => [0.80, 0.47, 0.54, 0.31, 0.36],
        Reminder::Edits => [0.51, 0.37, 0.33, 0.33, 0.39],
    };
    table[b]
}

/// Sum of `message.usage.output_tokens` across every assistant message in a
/// single session transcript (one JSONL file = one session). Best-effort:
/// an unreadable file or malformed line contributes 0. Used by the 0.6.4
/// output-lean estimate to know how much output the model has produced so
/// far this session (delta-credited at each UserPromptSubmit).
/// **DECLARED RESERVE.** The "output-lean" estimate now computes its
/// total from `lifetime_models_by_day` (see `digest_all_agents`), so nothing
/// calls this per-session read any more. It stays because it answers
/// a question the aggregate does not ask: what THIS session produced.
#[allow(dead_code)]
pub(crate) fn session_output_tokens(transcript_path: &Path) -> u64 {
    session_estimate(transcript_path).0
}

/// A Claude Code session's timeline: each API call (unix seconds, context it
/// sent, billed tokens) and each compaction (unix seconds, context size that
/// triggered it). What `gain::rereads` counts re-reads on.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionTimeline {
    pub calls: Vec<(u64, u64)>,
    pub compactions: Vec<(u64, u64)>,
    /// The context size at which the agent compacts, when its transcript
    /// states it (Codex: 90 % of `model_context_window`, its own rule in
    /// `auto_compact_token_limit`); 0 when not stated.
    #[serde(default)]
    pub compact_at: u64,
    /// Calls (unix seconds) that wrote most of their context to the prompt
    /// cache instead of reading it — the cache had expired, or its prefix
    /// changed. Their re-reads were billed at the cache-WRITE price.
    #[serde(default)]
    pub rewrites: Vec<u64>,
}

impl SessionTimeline {
    /// Record one API call: `ctx` sent, of which `written` went to the cache.
    /// The session's first call writes its context by nature; it is no
    /// re-read.
    pub fn push_call(&mut self, ts: u64, ctx: u64, written: u64) {
        if !self.calls.is_empty() && written * 2 > ctx {
            self.rewrites.push(ts);
        }
        self.calls.push((ts, ctx));
    }
}

/// Extend `tl` with the calls and compactions in `lines`. A call is counted
/// once per `message.id` (Claude Code writes one record per content block);
/// `last_id` carries the last id across incremental chunks.
pub fn extend_timeline<'a>(
    tl: &mut SessionTimeline,
    last_id: &mut String,
    lines: impl Iterator<Item = &'a str>,
) {
    for line in lines {
        if ![
            "\"assistant\"",
            "compact_boundary",
            "token_count",
            "compacted\"",
        ]
        .iter()
        .any(|k| line.contains(k))
        {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v["isSidechain"].as_bool() == Some(true) {
            continue;
        }
        let Some(ts) = v["timestamp"]
            .as_str()
            .and_then(super::iso8601_to_epoch_ms)
            .map(|ms| ms / 1000)
        else {
            continue;
        };
        if codex_record(tl, last_id, &v, ts) {
            continue;
        }
        if v["type"] == "system" && v["subtype"] == "compact_boundary" {
            tl.compactions
                .push((ts, v["compactMetadata"]["preTokens"].as_u64().unwrap_or(0)));
            continue;
        }
        if v["type"] != "assistant" {
            continue;
        }
        let Some(mid) = v["message"]["id"].as_str() else {
            continue;
        };
        if mid == last_id.as_str() {
            continue;
        }
        *last_id = mid.to_string();
        let u = &v["message"]["usage"];
        let ctx: u64 = [
            "input_tokens",
            "cache_read_input_tokens",
            "cache_creation_input_tokens",
        ]
        .iter()
        .map(|k| u[k].as_u64().unwrap_or(0))
        .sum();
        tl.push_call(
            ts,
            ctx,
            u["cache_creation_input_tokens"].as_u64().unwrap_or(0),
        );
    }
}

/// A Codex rollout line: `token_count` closes an API call (its
/// `last_token_usage.input_tokens` is the context sent, cached part
/// included; the event repeats with unchanged totals, so a call is one new
/// `total_tokens`), `compacted` is the durable compaction record and
/// `context_compacted` its legacy event (the two can come together: one
/// compaction within a few seconds). `true` when the line was Codex's.
fn codex_record(
    tl: &mut SessionTimeline,
    last_id: &mut String,
    v: &serde_json::Value,
    ts: u64,
) -> bool {
    let p = &v["payload"];
    let compaction =
        v["type"] == "compacted" || (v["type"] == "event_msg" && p["type"] == "context_compacted");
    if compaction {
        if tl.compactions.last().is_none_or(|&(t, _)| ts > t + 5) {
            tl.compactions.push((ts, 0));
        }
        return true;
    }
    if v["type"] != "event_msg" || p["type"] != "token_count" {
        return false;
    }
    let info = &p["info"];
    let total = info["total_token_usage"]["total_tokens"]
        .as_u64()
        .unwrap_or(0);
    let ctx = info["last_token_usage"]["input_tokens"]
        .as_u64()
        .unwrap_or(0);
    let id = format!("codex:{total}");
    if total > 0 && ctx > 0 && id != *last_id {
        *last_id = id;
        tl.calls.push((ts, ctx));
    }
    if let Some(w) = info["model_context_window"].as_u64().filter(|&w| w > 0) {
        tl.compact_at = w * 9 / 10;
    }
    true
}

/// [`extend_timeline`] over the part of `transcript_path` not read yet, the
/// read offset and the timeline kept in `state_path`.
pub fn session_timeline_cached(transcript_path: &Path, state_path: &Path) -> SessionTimeline {
    let len = std::fs::metadata(transcript_path)
        .map(|m| m.len())
        .unwrap_or(0);
    let st: serde_json::Value = std::fs::read_to_string(state_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(serde_json::json!({}));
    let mut off = st["off"].as_u64().unwrap_or(0);
    let mut last_id = st["last"].as_str().unwrap_or("").to_string();
    let mut tl: SessionTimeline = serde_json::from_value(st["tl"].clone()).unwrap_or_default();
    if off > len {
        (off, last_id, tl) = (0, String::new(), SessionTimeline::default());
    }
    if len > off {
        use std::io::{Read as _, Seek as _};
        if let Ok(mut f) = std::fs::File::open(transcript_path) {
            let _ = f.seek(std::io::SeekFrom::Start(off));
            let to_read = (len - off).min(MAX_SCAN_BYTES_PER_CALL);
            let mut raw = Vec::new();
            if f.take(to_read).read_to_end(&mut raw).is_ok() {
                let end = raw
                    .iter()
                    .rposition(|&b| b == b'\n')
                    .map(|i| i + 1)
                    .unwrap_or(0);
                extend_timeline(
                    &mut tl,
                    &mut last_id,
                    String::from_utf8_lossy(&raw[..end]).lines(),
                );
                off += if end == 0 && raw.len() as u64 == to_read {
                    to_read
                } else {
                    end as u64
                };
            }
        }
    }
    if let Some(p) = state_path.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    let _ = std::fs::write(
        state_path,
        serde_json::json!({"off": off, "last": last_id, "tl": tl}).to_string(),
    );
    tl
}
