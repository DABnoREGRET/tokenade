//! **What goes out on the wire.**
//!
//! Two dialects — OpenAI and Anthropic — and the same request is not written
//! the same way in both. The headers, the body, the token budget.
//!
//! The budget is computed, not constant: a fixed cap that exceeds what the
//! provider accepts makes the whole request fail, and the failure does not
//! say which of the two numbers was wrong.

use super::*;

/// One request, plus at most ONE "conclusion retry": when the model spent its
/// whole token budget on reasoning and never wrote `content` (the salvaged
/// text is flagged `from_reasoning`), replay the exchange with that partial
/// reasoning and ask for ONLY the final answer. A real answer beats a
/// truncated chain-of-thought; if the retry also fails, keep the salvage.
pub(crate) fn one_shot(p: &Provider, model: &str, messages: &[Msg]) -> Attempt {
    let mut c = match one_shot_raw(p, model, messages) {
        Attempt::Ok(c) => c,
        att => return att,
    };
    // Case 1 — all-reasoning, no answer: ONE conclusion retry.
    if c.from_reasoning {
        let mut follow: Vec<Msg> = messages.to_vec();
        follow.push(Msg::assistant(c.text.clone()));
        follow.push(Msg::user(
            "Your previous reply was cut off mid-reasoning and contains no final \
             answer. Based on the work above, reply now with ONLY the final \
             answer — no reasoning.",
        ));
        return match one_shot_raw(p, model, &follow) {
            Attempt::Ok(mut c2) if !c2.from_reasoning => {
                c2.usage.prompt_tokens += c.usage.prompt_tokens;
                c2.usage.completion_tokens += c.usage.completion_tokens;
                Attempt::Ok(c2)
            }
            // Retry failed or overthought again — keep the salvaged tail.
            _ => Attempt::Ok(c),
        };
    }
    // Case 2 — real answer but CUT OFF by the budget: bounded continuation
    // rounds, stitched together, so the caller never gets a silent half-reply.
    let mut rounds = 0;
    while c.truncated && rounds < 2 {
        let mut follow: Vec<Msg> = messages.to_vec();
        follow.push(Msg::assistant(c.text.clone()));
        follow.push(Msg::user(
            "Your answer was cut off by the length limit. Continue EXACTLY where \
             you stopped — no repetition, no preamble.",
        ));
        match one_shot_raw(p, model, &follow) {
            Attempt::Ok(c2) if !c2.from_reasoning => {
                c.text.push_str(&c2.text);
                c.usage.prompt_tokens += c2.usage.prompt_tokens;
                c.usage.completion_tokens += c2.usage.completion_tokens;
                c.truncated = c2.truncated;
            }
            _ => break, // keep what we have
        }
        rounds += 1;
    }
    Attempt::Ok(c)
}

pub(crate) fn one_shot_raw(p: &Provider, model: &str, messages: &[Msg]) -> Attempt {
    let (url, body, extra_headers) = match p.wire {
        Wire::OpenAi => (
            format!("{}/chat/completions", p.base_url),
            openai_body(model, messages),
            openai_headers(p),
        ),
        Wire::Anthropic => (
            format!("{}/messages", p.base_url),
            anthropic_body(model, messages),
            vec![("anthropic-version".to_string(), "2023-06-01".to_string())],
        ),
        Wire::TheOldLlm => (
            format!("{}/chatgpt", p.base_url),
            openai_body(model, messages), // {model, messages, stream:false}
            vec![
                ("X-Client-Version".to_string(), "3.8.4".to_string()),
                ("X-Request-Token".to_string(), gen_theoldllm_token()),
            ],
        ),
    };

    // Content-Type is set by `http_post_json`; add UA + Accept + the wire's
    // extra headers. Header strings must outlive the borrow below.
    let ua = if p.wire == Wire::TheOldLlm {
        THEOLDLLM_UA
    } else {
        UA
    };
    let mut headers: Vec<(&str, &str)> = vec![("User-Agent", ua), ("Accept", "application/json")];
    for (k, v) in &extra_headers {
        headers.push((k.as_str(), v.as_str()));
    }

    let resp = match net::http_post_json(&url, &headers, &body, REQ_TIMEOUT_SECS) {
        Some(r) => r,
        None => return Attempt::Failed("transport error".into()),
    };
    if resp.status == 429 {
        return Attempt::RateLimited;
    }
    if !resp.ok() {
        return Attempt::Failed(format!("HTTP {}", resp.status));
    }
    // The full body — SSE `data: {…}` lines included — is parsed line-by-line
    // by `parse_completion` exactly as curl's stdout was.
    match parse_completion(p, model, &resp.text()) {
        Some(c) => Attempt::Ok(c),
        None => Attempt::Failed("unparseable response".into()),
    }
}

pub(crate) fn openai_headers(p: &Provider) -> Vec<(String, String)> {
    let mut h = Vec::new();
    if p.id == "env" {
        // Bearer key only sent to the user's own configured endpoint.
        if let Ok(key) = std::env::var("TOKENADE_AGENT_API_KEY") {
            if !key.trim().is_empty() {
                h.push((
                    "Authorization".to_string(),
                    format!("Bearer {}", key.trim()),
                ));
            }
        }
    }
    h
}

/// Generation budget sent with every request. Reasoning models think BEFORE
/// they answer, and the thinking bills against the same budget — too small a
/// value means the model dies mid-thought with an empty `content` (the exact
/// failure the bench surfaced). 16384 is accepted by every live provider
/// (verified) and large enough that a sub-agent never hits it in practice.
pub(crate) fn max_tokens_budget() -> u64 {
    std::env::var("TOKENADE_AGENT_MAX_TOKENS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(16_384)
}

pub(crate) fn openai_body(model: &str, messages: &[Msg]) -> serde_json::Value {
    let msgs: Vec<serde_json::Value> = messages
        .iter()
        .map(|m| serde_json::json!({"role": m.role, "content": m.content}))
        .collect();
    serde_json::json!({
        "model": model,
        "messages": msgs,
        "stream": false,
        "temperature": 0.2,
        "max_tokens": max_tokens_budget(),
    })
}

pub(crate) fn anthropic_body(model: &str, messages: &[Msg]) -> serde_json::Value {
    // Anthropic splits the system message out of the turn list.
    let system: String = messages
        .iter()
        .filter(|m| m.role == "system")
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n\n");
    let turns: Vec<serde_json::Value> = messages
        .iter()
        .filter(|m| m.role != "system")
        .map(|m| serde_json::json!({"role": m.role, "content": m.content}))
        .collect();
    serde_json::json!({
        "model": model,
        "system": system,
        "messages": turns,
        "max_tokens": 4096,
        "stream": false,
    })
}
