//! **What comes back, and what to strip from it.**
//!
//! A response carries the text, the usage, sometimes a tail of reasoning the
//! caller did not ask for. Delivering it costs tokens and muddies the answer;
//! cutting it badly cuts the answer itself.
//!
//! Each provider has its own envelope shape, and one tolerant reading beats
//! five strict readings that each fail in turn.

use super::*;

pub(crate) fn parse_completion(p: &Provider, model: &str, payload: &str) -> Option<Completion> {
    // theoldllm streams `data: {…delta.content…}` lines even with
    // stream:false, so it isn't a single JSON object — parse it separately.
    if p.wire == Wire::TheOldLlm {
        let mut text = String::new();
        for line in payload.lines() {
            let line = line.trim();
            let data = line.strip_prefix("data:").map(str::trim).unwrap_or(line);
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
                let d = &v["choices"][0]["delta"];
                if let Some(chunk) = d["content"].as_str().or_else(|| d["text"].as_str()) {
                    text.push_str(chunk);
                }
            }
        }
        if text.is_empty() {
            return None;
        }
        return Some(Completion {
            text,
            usage: Usage::default(), // endpoint omits usage; runner estimates
            provider: p.id.to_string(),
            model: model.to_string(),
            from_reasoning: false,
            truncated: false,
        });
    }
    let v: serde_json::Value = serde_json::from_str(payload.trim()).ok()?;
    // "length" (OpenAI wire) / "max_tokens" (Anthropic wire) = the generation
    // budget cut the reply short.
    let truncated = v["choices"][0]["finish_reason"].as_str() == Some("length")
        || v["stop_reason"].as_str() == Some("max_tokens");
    let (text, usage, from_reasoning) = match p.wire {
        Wire::OpenAi => {
            let msg = &v["choices"][0]["message"];
            // Reasoning models can return an empty `content` — the model spent
            // its whole token budget thinking and never wrote the answer
            // (finish_reason "length"). Salvage the reasoning TAIL (the part
            // closest to a conclusion) and FLAG it: `one_shot` then issues one
            // conclusion retry to obtain a real answer.
            let (text, from_reasoning) =
                match msg["content"].as_str().filter(|s| !s.trim().is_empty()) {
                    Some(c) => (c.to_string(), false),
                    None => (
                        reasoning_tail(
                            msg["reasoning_content"]
                                .as_str()
                                .or_else(|| msg["reasoning"].as_str())?,
                        ),
                        true,
                    ),
                };
            let usage = Usage {
                prompt_tokens: v["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
                completion_tokens: v["usage"]["completion_tokens"].as_u64().unwrap_or(0),
            };
            (text, usage, from_reasoning)
        }
        Wire::Anthropic => {
            let text = v["content"][0]["text"].as_str()?.to_string();
            // `prompt_tokens` on the OpenAI wire is the WHOLE prompt, cached
            // part included. Anthropic's `input_tokens` is the fresh part
            // only: cache reads and cache writes are billed on their own
            // lines. Reading it alone made the same field mean two different
            // quantities depending on which provider answered, and the
            // runner sums them into one figure.
            //
            // Measured 2026-08-26 over 146 444 real Claude Code usage
            // records: 146 368 of them have `input_tokens` SMALLER than
            // `cache_read_input_tokens` — 12 772 099 against 46 727 481 926
            // in total — which is only possible if the cache is not counted
            // inside it.
            let n = |k: &str| v["usage"][k].as_u64().unwrap_or(0);
            let usage = Usage {
                prompt_tokens: n("input_tokens")
                    .saturating_add(n("cache_read_input_tokens"))
                    .saturating_add(n("cache_creation_input_tokens")),
                completion_tokens: n("output_tokens"),
            };
            (text, usage, false)
        }
        Wire::TheOldLlm => return None, // handled by the early return above
    };
    Some(Completion {
        text,
        usage,
        provider: p.id.to_string(),
        model: model.to_string(),
        from_reasoning,
        truncated,
    })
}

/// Shape a reasoning-only reply into something answer-like: chain-of-thought
/// STARTS with meta ("The user wants me to…") and ENDS with the conclusion,
/// so keep the LAST paragraphs (up to ~800 chars). Short reasoning passes
/// through whole.
pub(crate) fn reasoning_tail(r: &str) -> String {
    const MAX: usize = 800;
    let r = r.trim();
    if r.len() <= MAX {
        return r.to_string();
    }
    let mut kept: Vec<&str> = Vec::new();
    let mut total = 0usize;
    for para in r.rsplit("\n\n") {
        let p = para.trim();
        if p.is_empty() {
            continue;
        }
        if total + p.len() > MAX && !kept.is_empty() {
            break;
        }
        total += p.len();
        kept.push(p);
    }
    kept.reverse();
    kept.join("\n\n")
}

/// Port of theoldllm's `generateRequestToken()` (DJB2 over a fixed seed +
/// the millisecond clock). No signup, no secret — the seed string is a
/// constant the server re-derives. Format: `{ms_b36}-{djb2abs_b36}-{rnd8}`.
pub(crate) fn gen_theoldllm_token() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0) as u64;
    // Exact seed from open-sse/executors/theoldllm.ts generateRequestToken.
    let seed = format!("{now}-oldllm-client-2026-Mozilla/5.0 (Windows");
    // JS DJB2 in 32-bit signed arithmetic (`t = (t<<5) - t + c; t = t & t`).
    let mut t: i32 = 0;
    for c in seed.chars() {
        t = t.wrapping_shl(5).wrapping_sub(t).wrapping_add(c as i32);
    }
    let abs = (t as i64).unsigned_abs(); // Math.abs(t)
                                         // The random 8-char tail only needs to be unique; derive it from the
                                         // clock (no rand crate) rather than pulling a dependency.
    let suffix = format!("{:08x}", (now.wrapping_mul(2_654_435_761)) as u32);
    format!("{}-{}-{}", to_base36(now), to_base36(abs), suffix)
}

pub(crate) fn to_base36(mut n: u64) -> String {
    if n == 0 {
        return "0".into();
    }
    const D: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut s = Vec::new();
    while n > 0 {
        s.push(D[(n % 36) as usize]);
        n /= 36;
    }
    s.reverse();
    String::from_utf8(s).unwrap()
}

pub(crate) fn sleep_ms(ms: u64) {
    std::thread::sleep(std::time::Duration::from_millis(ms));
}

/// Leak a short model string to 'static (chain lives for the whole call;
/// only used for a user-supplied "provider/model" override, so bounded).
pub(crate) fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

pub(crate) fn leak_provider(p: Provider) -> &'static Provider {
    Box::leak(Box::new(p))
}

/// Build a provider from `TOKENADE_AGENT_BASE_URL` (OpenAI-compatible),
/// with model from `TOKENADE_AGENT_MODEL` (default `gpt-4o-mini`). Returns
/// None when the env var is unset.
pub(crate) fn env_provider() -> Option<(&'static Provider, &'static str)> {
    let base = std::env::var("TOKENADE_AGENT_BASE_URL").ok()?;
    let base = base.trim().trim_end_matches('/');
    if base.is_empty() {
        return None;
    }
    let model = std::env::var("TOKENADE_AGENT_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());
    let p = leak_provider(Provider {
        id: "env",
        base_url: leak(base),
        wire: Wire::OpenAi,
        state: ImplState::Http,
        models: &[],
    });
    Some((p, leak(&model)))
}

/// Rough token estimate (chars/4) for endpoints that omit `usage`.
pub fn est_tokens(s: &str) -> u64 {
    (s.chars().count() / 4) as u64
}
