//! Free, no-signup LLM client — ported from OmniRoute
//! (`open-sse/executors/*` + `src/shared/constants/providers/noauth.ts`).
//!
//! OmniRoute is a TS/Next service; we keep its *logic* (a fallback chain
//! over free endpoints, two-tier retry — intra-URL 429 retry then
//! provider fallback — and a per-provider circuit breaker) but drop its
//! runtime: every request here is a blocking POST through the shared
//! in-process HTTP client ([`tokenade_core::net`], reqwest+rustls, honouring
//! the corporate proxy). Non-streaming: a light-agent only needs the final
//! message + token usage, not deltas.
//!
//! Providers marked `impl: Http` are wired end-to-end. The
//! reverse-engineered handshake providers from OmniRoute (DuckDuckGo VQD
//! challenge, MiMoCode JWT-bootstrap, Chipotle STOMP websocket) are
//! registered in [`PROVIDERS`] for completeness but flagged `Registered`
//! — they need a stateful handshake a single buffered POST can't express, so
//! they are skipped by the fallback loop until ported. This keeps the
//! chain honest: it only tries endpoints it can actually reach.

use super::pool;
use tokenade_core::net;

/// A chat turn sent to / received from a model.
#[derive(Clone, Debug)]
pub struct Msg {
    pub role: String, // "system" | "user" | "assistant"
    pub content: String,
}

impl Msg {
    pub fn system(c: impl Into<String>) -> Msg {
        Msg {
            role: "system".into(),
            content: c.into(),
        }
    }
    pub fn user(c: impl Into<String>) -> Msg {
        Msg {
            role: "user".into(),
            content: c.into(),
        }
    }
    pub fn assistant(c: impl Into<String>) -> Msg {
        Msg {
            role: "assistant".into(),
            content: c.into(),
        }
    }
}

/// Token accounting reported by the upstream (0 when the endpoint omits
/// usage — the runner then estimates from text length).
#[derive(Clone, Copy, Debug, Default)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

#[derive(Clone, Debug)]
pub struct Completion {
    pub text: String,
    pub usage: Usage,
    pub provider: String,
    pub model: String,
    /// True when `text` came from the `reasoning`/`reasoning_content` fallback
    /// (the model burned its whole budget thinking and never wrote `content`).
    /// [`one_shot`] then issues ONE conclusion retry to get a real answer.
    pub from_reasoning: bool,
    /// True when the upstream reported `finish_reason: "length"` — the answer
    /// was CUT OFF by the generation budget. [`one_shot`] then issues bounded
    /// continuation rounds so the caller never sees a silently-truncated reply.
    pub truncated: bool,
}

/// Wire dialect of an endpoint.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Wire {
    /// POST {base}/chat/completions, OpenAI schema, JSON body response.
    OpenAi,
    /// POST {base}/messages, Anthropic schema.
    Anthropic,
    /// theoldllm: POST {base}/chatgpt with `stream:false`, a DJB2
    /// `X-Request-Token`, and an SSE-style body of `data: {delta.content}`
    /// lines (even in non-stream mode). Verified live 2026-07.
    TheOldLlm,
}

/// Whether we can actually talk to this provider today.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ImplState {
    Http,       // single buffered POST works
    Registered, // needs a stateful handshake — skipped for now
}

#[derive(Clone, Copy, Debug)]
struct Provider {
    id: &'static str,
    base_url: &'static str,
    wire: Wire,
    state: ImplState,
    /// Free model ids, best first. All no-signup.
    models: &'static [&'static str],
}

/// The free, no-signup provider table (from OmniRoute's `noauth.ts`).
/// Order = fallback priority. Http providers are tried; Registered ones
/// are documented but skipped until their handshake is ported.
static PROVIDERS: &[Provider] = &[
    // opencode "zen" — public OpenAI-compatible gateway. Model LISTING and
    // the `*-free` model tier need NO auth (verified live 2026-07: a POST to
    // /chat/completions with a `-free` model returns a real completion; the
    // non-free models 401 "Missing API key"). So we only advertise the free
    // tier here. `big-pickle` routes to deepseek-v4-flash server-side.
    Provider {
        id: "opencode-zen",
        base_url: "https://opencode.ai/zen/v1",
        wire: Wire::OpenAi,
        state: ImplState::Http,
        // The five no-auth `-free` tier models (verified live 2026-07).
        // Ordered by measured reliability+latency; mimo last (it overthinks —
        // ~24s on a one-line code task). big-pickle routes to deepseek-v4-flash
        // server-side (separate quota from -flash-free, so a useful 2nd path).
        models: &[
            "deepseek-v4-flash-free",
            "north-mini-code-free",
            "nemotron-3-ultra-free",
            "big-pickle",
            "mimo-v2.5-free",
        ],
    },
    // theoldllm — Vercel proxy, no signup. Wire is ported + verified
    // (Windows-UA gated, DJB2 token, SSE body) BUT kept Registered (skipped):
    // the live endpoint only serves GPT_5_4 (others 400 "Model not
    // supported") and that one 504-times-out at the 25s Vercel function limit
    // on any non-trivial prompt — i.e. every real agent turn. A 25s dead-end
    // in the pool is worse than its absence. Flip to Http if the deployment
    // ever raises its limits.
    Provider {
        id: "theoldllm",
        base_url: "https://theoldllm.vercel.app/api",
        wire: Wire::TheOldLlm,
        state: ImplState::Registered,
        models: &["GPT_5_4"],
    },
    // --- registered from OmniRoute, handshake not yet ported ---
    Provider {
        id: "duckduckgo-web",
        base_url: "https://duckduckgo.com/duckchat/v1",
        wire: Wire::OpenAi,
        state: ImplState::Registered, // needs x-vqd-4 / x-vqd-hash-1 challenge
        models: &["gpt-4o-mini", "claude-3-haiku", "llama-4", "mistral-small"],
    },
    Provider {
        id: "mimocode",
        base_url: "https://api.xiaomimimo.com/api/free-ai/openai",
        wire: Wire::OpenAi,
        state: ImplState::Registered, // needs JWT bootstrap + account rotation
        models: &["mimo-auto"],
    },
    Provider {
        id: "zenmux-free",
        base_url: "https://zenmux.ai/api/anthropic/v1",
        wire: Wire::Anthropic,
        state: ImplState::Registered, // needs ctoken + rotating cookie
        models: &["claude-sonnet", "claude-haiku"],
    },
    Provider {
        id: "chipotle",
        base_url: "wss://amelia.chipotle.com",
        wire: Wire::OpenAi,
        state: ImplState::Registered, // STOMP/SockJS websocket, not HTTP
        models: &["pepper"],
    },
];

const UA: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36";

/// theoldllm allowlists its client's User-Agent and 400s ("Blocked user
/// agent") on anything else — it must be this exact Windows Chrome string.
const THEOLDLLM_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36";

/// Two-tier retry config, mirroring OmniRoute `base.ts` RETRY_CONFIG.
const MAX_INTRA_RETRY: u32 = 2;
const INTRA_DELAY_MS: u64 = 1500;
const REQ_TIMEOUT_SECS: u64 = 90;

/// A soft circuit breaker: after this many consecutive failures a
/// provider is skipped for the rest of the run (OmniRoute opens the
/// circuit; here the "reset" is simply process lifetime).
const CB_TRIP_AFTER: u32 = 2;

/// One attempt's outcome, used to drive fallback vs intra-retry.
enum Attempt {
    Ok(Completion),
    RateLimited,    // 429 — worth an intra-retry
    Failed(String), // anything else — fall through to next provider
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_auto_is_http_only() {
        let c = resolve_chain("auto");
        assert!(!c.is_empty());
        // registered-only providers must never appear
        assert!(c.iter().all(|(p, _)| p.state == ImplState::Http));
    }

    #[test]
    fn chain_prefers_named_provider_first() {
        let c = resolve_chain("opencode-zen");
        assert_eq!(c[0].0.id, "opencode-zen");
    }

    #[test]
    fn token_is_stable_shape() {
        let t = gen_theoldllm_token();
        assert_eq!(t.matches('-').count(), 2);
    }

    pub(super) fn oai() -> Provider {
        Provider {
            id: "t",
            base_url: "x",
            wire: Wire::OpenAi,
            state: ImplState::Http,
            models: &[],
        }
    }
    pub(super) fn anth() -> Provider {
        Provider {
            id: "t",
            base_url: "x",
            wire: Wire::Anthropic,
            state: ImplState::Http,
            models: &[],
        }
    }

    #[test]
    fn parse_openai_basic() {
        let p = oai();
        let body = r#"{"choices":[{"message":{"content":"hi"}}],"usage":{"prompt_tokens":5,"completion_tokens":2}}"#;
        let c = parse_completion(&p, "m", body).unwrap();
        assert_eq!(c.text, "hi");
        assert_eq!(c.usage.prompt_tokens, 5);
        assert_eq!(c.usage.completion_tokens, 2);
    }

    #[test]
    fn parse_openai_reasoning_fallback() {
        // Reasoning models: content null/empty, text lives in reasoning_content.
        let p = oai();
        let body =
            r#"{"choices":[{"message":{"content":null,"reasoning_content":"the answer is 42"}}]}"#;
        let c = parse_completion(&p, "m", body).unwrap();
        assert_eq!(c.text, "the answer is 42");
        // empty-string content also falls through:
        let body2 = r#"{"choices":[{"message":{"content":"   ","reasoning":"fallback"}}]}"#;
        assert_eq!(parse_completion(&p, "m", body2).unwrap().text, "fallback");
    }

    #[test]
    fn parse_flags_truncated_on_length_finish() {
        let p = oai();
        let body =
            r#"{"choices":[{"message":{"content":"partial answer"},"finish_reason":"length"}]}"#;
        let c = parse_completion(&p, "m", body).unwrap();
        assert!(c.truncated, "length → truncated");
        assert!(!c.from_reasoning);
        let body2 = r#"{"choices":[{"message":{"content":"done"},"finish_reason":"stop"}]}"#;
        assert!(!parse_completion(&p, "m", body2).unwrap().truncated);
    }

    #[test]
    fn openai_body_carries_generation_budget() {
        let v = openai_body("m", &[Msg::user("hi")]);
        assert!(v["max_tokens"].as_u64().unwrap() >= 16_384);
    }

    #[test]
    fn reasoning_fallback_serves_tail_not_head() {
        // Long chain-of-thought: the meta head ("The user wants…") must NOT
        // leak as the answer — keep the concluding paragraphs instead.
        let head = "The user wants me to explore the repo and list the files. \
                    Let me think about how to approach this step by step."
            .repeat(8);
        let tail = "In conclusion: the retry logic lives in net.py, function retry().";
        let long = format!("{head}\n\n{middle}\n\n{tail}", middle = "x".repeat(900));
        let out = reasoning_tail(&long);
        assert!(out.contains("In conclusion"), "tail kept: {out:?}");
        assert!(!out.contains("The user wants"), "head dropped: {out:?}");
        // short reasoning passes through whole
        assert_eq!(reasoning_tail("short answer"), "short answer");
    }

    #[test]
    fn parse_anthropic_shape() {
        let p = anth();
        let body = r#"{"content":[{"type":"text","text":"yo"}],"usage":{"input_tokens":7,"output_tokens":3}}"#;
        let c = parse_completion(&p, "m", body).unwrap();
        assert_eq!(c.text, "yo");
        assert_eq!(c.usage.prompt_tokens, 7);
        assert_eq!(c.usage.completion_tokens, 3);
    }

    #[test]
    fn parse_rejects_garbage_and_missing_fields() {
        let p = oai();
        assert!(parse_completion(&p, "m", "not json").is_none());
        // no content and no reasoning → None (not a panic)
        assert!(parse_completion(&p, "m", r#"{"choices":[{"message":{}}]}"#).is_none());
    }

    // --- run_chain policy tests (transport injected, no network) ---

    static TA: Provider = Provider {
        id: "a",
        base_url: "",
        wire: Wire::OpenAi,
        state: ImplState::Http,
        models: &["m1", "m2", "m3"],
    };
    static TB: Provider = Provider {
        id: "b",
        base_url: "",
        wire: Wire::OpenAi,
        state: ImplState::Http,
        models: &["m1"],
    };

    fn ok(p: &Provider, m: &str) -> Attempt {
        Attempt::Ok(Completion {
            text: "done".into(),
            usage: Usage::default(),
            provider: p.id.to_string(),
            model: m.to_string(),
            from_reasoning: false,
            truncated: false,
        })
    }

    #[test]
    fn intra_retry_then_success_same_provider() {
        // 429 once, then Ok on the retry of the SAME (provider, model).
        let mut n = 0;
        let mut t = |p: &Provider, m: &str| {
            n += 1;
            if n == 1 {
                Attempt::RateLimited
            } else {
                ok(p, m)
            }
        };
        let r = run_chain(&[(&TA, "m1")], &mut t, 0).unwrap();
        assert_eq!(r.provider, "a");
        assert_eq!(n, 2, "should retry once then succeed");
    }

    #[test]
    fn rate_limited_exhausts_then_falls_to_next_provider() {
        // Provider a always 429; must exhaust MAX_INTRA_RETRY then fall to b.
        let mut a_calls = 0;
        let mut t = |p: &Provider, m: &str| {
            if p.id == "a" {
                a_calls += 1;
                Attempt::RateLimited
            } else {
                ok(p, m)
            }
        };
        let r = run_chain(&[(&TA, "m1"), (&TB, "m1")], &mut t, 0).unwrap();
        assert_eq!(r.provider, "b");
        assert_eq!(
            a_calls,
            1 + MAX_INTRA_RETRY,
            "1 try + MAX_INTRA_RETRY retries"
        );
    }

    #[test]
    fn circuit_breaker_skips_remaining_models_of_failed_provider() {
        // a fails hard on every model; after CB_TRIP_AFTER failures its
        // remaining models are skipped (not called), then b succeeds.
        let mut a_calls = 0;
        let mut t = |p: &Provider, m: &str| {
            if p.id == "a" {
                a_calls += 1;
                Attempt::Failed("boom".into())
            } else {
                ok(p, m)
            }
        };
        let chain = [(&TA, "m1"), (&TA, "m2"), (&TA, "m3"), (&TB, "m1")];
        let r = run_chain(&chain, &mut t, 0).unwrap();
        assert_eq!(r.provider, "b");
        assert_eq!(a_calls, CB_TRIP_AFTER, "m3 skipped once circuit opened");
    }

    #[test]
    fn whole_chain_failure_reports_reasons() {
        let mut t = |_p: &Provider, _m: &str| Attempt::Failed("nope".into());
        let e = run_chain(&[(&TA, "m1")], &mut t, 0).unwrap_err();
        assert!(e.contains("all free providers failed"));
        assert!(e.contains("a/m1"));
        assert!(e.contains("nope"));
    }
}

#[cfg(test)]
mod prompt_tokens_means_the_whole_prompt_on_both_wires {
    //! `Usage::prompt_tokens` is summed by the runner into one figure, so it
    //! has to mean the same thing whichever provider answered. OpenAI's
    //! `prompt_tokens` counts the cached part; Anthropic's `input_tokens`
    //! does not, and reading it alone under-reported every cached turn.

    mod chain;
    mod parse;
    mod wire;
    pub(crate) use chain::*;
    pub(crate) use parse::*;
    pub(crate) use wire::*;

    use super::*;

    fn anthropic(usage: &str) -> Usage {
        let body = format!(r#"{{"content":[{{"type":"text","text":"yo"}}],"usage":{usage}}}"#);
        parse_completion(&tests::anth(), "m", &body).unwrap().usage
    }

    #[test]
    fn a_cached_anthropic_turn_reports_the_whole_prompt() {
        let u = anthropic(
            r#"{"input_tokens":7,"cache_read_input_tokens":9000,"cache_creation_input_tokens":300,"output_tokens":3}"#,
        );
        assert_eq!(u.prompt_tokens, 9307, "7 fresh + 9000 read + 300 written");
        assert_eq!(u.completion_tokens, 3);
    }

    #[test]
    fn an_uncached_anthropic_turn_is_unchanged() {
        let u = anthropic(r#"{"input_tokens":7,"output_tokens":3}"#);
        assert_eq!(u.prompt_tokens, 7);
    }

    #[test]
    fn the_two_wires_agree_on_an_identical_turn() {
        // Same conversation, same 9 307-token prompt, two providers.
        let oai_body = r#"{"choices":[{"message":{"content":"yo"}}],"usage":{"prompt_tokens":9307,"completion_tokens":3}}"#;
        let oai = parse_completion(&tests::oai(), "m", oai_body)
            .unwrap()
            .usage;
        let ant = anthropic(
            r#"{"input_tokens":7,"cache_read_input_tokens":9000,"cache_creation_input_tokens":300,"output_tokens":3}"#,
        );
        assert_eq!(oai.prompt_tokens, ant.prompt_tokens);
    }
}
