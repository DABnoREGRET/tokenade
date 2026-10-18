//! Local in-process inference for light-agents via llama.cpp (in-tree
//! `tokenade-llama-sys` bindings) running a small GGUF model (default
//! Qwen3-4B, Q3_K_M — see DEFAULT_MODEL_URL) on CPU.
//!
//! Architecture — a local model SERVER, agents are thin clients:
//!   * `tokenade local-serve` loads the model ONCE, holds it in RAM, and serves
//!     an OpenAI-compatible endpoint over a TCP socket. A pool of N llama
//!     contexts (the model weights are shared read-only) answers up to N
//!     requests concurrently.
//!   * each `tokenade agent --model local` worker is a CLIENT: it POSTs its
//!     prompt to the server and reads back the completion, auto-starting the
//!     server on first use. Workers don't load the model — many run at once
//!     against the single shared server (no N×700 MB, no per-worker reload).
//!
//! Gated behind the `local-llm` cargo feature; without it `--model local`
//! returns a rebuild hint.

// Helpers below are only exercised by the feature-gated engine/server.
#![cfg_attr(not(feature = "local-llm"), allow(dead_code))]

use std::path::PathBuf;

// Qwen3-4B (Q3_K_M): won a 6-model eval of the agentic tool-calling loop —
// 6/6 tasks, same score as its Q4_K_M at ~400 MB smaller (2.0 GB, CPU-friendly).
// (Qwen2.5-3B 5/6, Llama-3.2-3B 4/6, Phi-4-mini 2/6, Gemma-2-2b 0/6, MiniCPM5-1B
// unreliable.) Override with TOKENADE_LOCAL_MODEL / _URL.
const DEFAULT_MODEL_URL: &str =
    "https://huggingface.co/unsloth/Qwen3-4B-GGUF/resolve/main/Qwen3-4B-Q3_K_M.gguf";
const DEFAULT_MODEL_FILE: &str = "qwen3-4b-q3_k_m.gguf";
const MAX_GEN_TOKENS: usize = 512;
const DEFAULT_ADDR: &str = "127.0.0.1:8919";

/// True when this invocation should use the local engine: `--model local`
/// (also `minicpm`) or `TOKENADE_AGENT_LOCAL=1`.
pub fn wants_local(want: &str) -> bool {
    let w = want.trim();
    w == "local"
        || w == "minicpm"
        || std::env::var("TOKENADE_AGENT_LOCAL").ok().as_deref() == Some("1")
}

/// Address the local model server listens on / clients connect to.
fn server_addr() -> String {
    std::env::var("TOKENADE_LOCAL_ADDR").unwrap_or_else(|_| DEFAULT_ADDR.into())
}

/// Whether to enable a hybrid-thinking model's `<think>` reasoning phase.
/// ON by default — reasoning before a tool call materially improves small-model
/// accuracy on agentic tasks (the `<think>` block is stripped from the final
/// answer). Set `TOKENADE_LOCAL_THINK=0` to disable for speed.
pub fn think_enabled() -> bool {
    std::env::var("TOKENADE_LOCAL_THINK").ok().as_deref() != Some("0")
}

/// How many requests the server answers concurrently (one llama context each).
/// Kept small by default so it runs on any machine (each context costs RAM).
fn parallelism() -> usize {
    std::env::var("TOKENADE_LOCAL_PARALLEL")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| *n >= 1)
        .unwrap_or(2)
}

fn models_dir() -> PathBuf {
    tokenade_core::gain::current_ledger_path()
        .parent()
        .map(|d| d.join("models"))
        .unwrap_or_else(|| PathBuf::from(".tokenade/models"))
}

/// Resolve the GGUF path, downloading it on first use. Overrides:
/// `TOKENADE_LOCAL_MODEL` (explicit path) / `TOKENADE_LOCAL_MODEL_URL` (url).
pub fn ensure_model() -> Result<PathBuf, String> {
    if let Ok(p) = std::env::var("TOKENADE_LOCAL_MODEL") {
        let pb = PathBuf::from(&p);
        return if pb.exists() {
            Ok(pb)
        } else {
            Err(format!("TOKENADE_LOCAL_MODEL not found: {p}"))
        };
    }
    let path = models_dir().join(DEFAULT_MODEL_FILE);
    if path.exists() {
        return Ok(path);
    }
    std::fs::create_dir_all(models_dir()).map_err(|e| e.to_string())?;
    let url =
        std::env::var("TOKENADE_LOCAL_MODEL_URL").unwrap_or_else(|_| DEFAULT_MODEL_URL.into());
    eprintln!("[tokenade local] fetching model {DEFAULT_MODEL_FILE} (~700 MB, one-time)…");
    let tmp = path.with_extension("part");
    // Stream the (large) GGUF straight to disk via the shared client (reqwest
    // follows redirects + gunzips like `curl -fL`), retrying on transport/non-2xx
    // up to the same 3 retries `curl --retry 3` gave (4 attempts total).
    let mut ok = false;
    for _ in 0..4 {
        use std::io::Write;
        let Ok(mut file) = std::fs::File::create(&tmp) else {
            break;
        };
        // Two hours, not one. The budget is TOTAL, and one hour over ~700 MB
        // demands 194 kB/s sustained — anyone slower could never finish, and
        // burned four hours across the retries finding out. Two hours admits
        // ~97 kB/s, and being generous is only safe now that a stall dies in
        // `STREAM_STALL_DEADLINE` instead of holding the whole budget.
        let status = tokenade_core::net::http_get_stream(&url, &[], 7200, &mut file);
        let _ = file.flush();
        drop(file);
        if matches!(status, Some(s) if (200..300).contains(&s)) {
            ok = true;
            break;
        }
    }
    if !ok {
        let _ = std::fs::remove_file(&tmp);
        return Err("local model download failed (set TOKENADE_LOCAL_MODEL to a .gguf)".into());
    }
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(path)
}

// ---- client (no feature): rebuild hint -------------------------------------

#[cfg(not(feature = "local-llm"))]
pub fn serve() -> Result<(), String> {
    let _ = (server_addr, parallelism, ensure_model);
    Err("local engine not built in — rebuild with `--features local-llm`".into())
}

// ---- client (feature): talk to the local server ----------------------------

/// Native function-calling completion: send raw chat `messages` (which may
/// carry assistant tool_calls and tool-role results) + `tools` schemas, and
/// return the model's raw output text + token counts. The tool-call loop lives
/// in the runner (sandbox execution is per-agent, client-side).
#[cfg(feature = "local-llm")]
pub fn complete_tools(
    messages: &serde_json::Value,
    tools: &serde_json::Value,
    max_tokens: usize,
) -> Result<(String, u64, u64), String> {
    let addr = server_addr();
    ensure_server(&addr)?;
    let req = serde_json::json!({
        "messages": messages,
        "tools": tools,
        "max_tokens": max_tokens,
    });
    let resp = http_post(&addr, &req.to_string())?;
    let v: serde_json::Value =
        serde_json::from_str(&resp).map_err(|e| format!("local server response: {e}"))?;
    if let Some(err) = v["error"].as_str() {
        return Err(format!("local server: {err}"));
    }
    let text = v["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let pin = v["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
    let pout = v["usage"]["completion_tokens"].as_u64().unwrap_or(0);
    Ok((text, pin, pout))
}

#[cfg(not(feature = "local-llm"))]
pub fn complete_tools(
    _messages: &serde_json::Value,
    _tools: &serde_json::Value,
    _max_tokens: usize,
) -> Result<(String, u64, u64), String> {
    Err("local engine not built in — rebuild with `--features local-llm`".into())
}

/// Grammar-constrained completion (scout pipelines): the model's output is
/// forced through a GBNF grammar, so it always parses. With `think`, the
/// caller's grammar must open with a `<think>` block (the chat template engages
/// the model's reasoning phase and the JSON constraint applies after it).
#[cfg(feature = "local-llm")]
pub fn complete_structured(
    messages: &serde_json::Value,
    grammar: &str,
    temperature: f32,
    max_tokens: usize,
    think: bool,
) -> Result<(String, u64, u64), String> {
    let addr = server_addr();
    ensure_server(&addr)?;
    let req = serde_json::json!({
        "messages": messages,
        "max_tokens": max_tokens,
        "grammar": grammar,
        "temperature": temperature,
        "think": think,
    });
    let resp = http_post(&addr, &req.to_string())?;
    let v: serde_json::Value =
        serde_json::from_str(&resp).map_err(|e| format!("local server response: {e}"))?;
    if let Some(err) = v["error"].as_str() {
        return Err(format!("local server: {err}"));
    }
    let text = v["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let pin = v["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
    let pout = v["usage"]["completion_tokens"].as_u64().unwrap_or(0);
    Ok((text, pin, pout))
}

#[cfg(not(feature = "local-llm"))]
pub fn complete_structured(
    _messages: &serde_json::Value,
    _grammar: &str,
    _temperature: f32,
    _max_tokens: usize,
    _think: bool,
) -> Result<(String, u64, u64), String> {
    Err("local engine not built in — rebuild with `--features local-llm`".into())
}

#[cfg(feature = "local-llm")]
fn health(addr: &str) -> bool {
    match tokenade_core::net::http_get(&format!("http://{addr}/health"), &[], 2) {
        Some(r) => r.body.starts_with(b"ok"),
        None => false,
    }
}

#[cfg(feature = "local-llm")]
fn http_post(addr: &str, body: &str) -> Result<String, String> {
    let url = format!("http://{addr}/v1/chat/completions");
    match tokenade_core::net::http_post_bytes(
        &url,
        &[],
        "application/json",
        body.as_bytes().to_vec(),
        180,
    ) {
        Some(r) => Ok(r.text()),
        None => Err("local server request failed".into()),
    }
}

#[cfg(feature = "local-llm")]
mod engine {
    //! Raw FFI glue to our freshly-built llama.cpp ([`tokenade_llama_sys`]).
    //! Backend + model load once; each server worker owns a context (its own KV
    //! cache). The model is immutable after load, so it's shared across worker
    //! threads; contexts are thread-local.
    use std::ffi::{CStr, CString};
    use std::os::raw::c_char;
    use std::path::Path;
    use std::ptr;
    use std::sync::Once;
    use tokenade_llama_sys as ffi;

    mod server;
    pub(crate) use server::*;

    static BACKEND: Once = Once::new();

    /// Per-request generation parameters. `None` fields fall back to the
    /// server-wide defaults (env-driven), keeping the legacy tool-call path
    /// byte-identical.
    pub struct GenParams {
        pub max_tokens: usize,
        /// GBNF grammar constraining the output (scout structured calls).
        pub grammar: Option<String>,
        pub temperature: Option<f32>,
        /// Hybrid-thinking phase on/off; `None` = env default.
        pub think: Option<bool>,
    }

    /// Context window per worker. 4096 was far too small — a sub-agent digesting
    /// a file or log overflowed it. Default to the MODEL'S OWN trained context
    /// (`llama_model_n_ctx_train` — 32k+ on Qwen3, ~130k on MiniCPM5) so digests
    /// get the full window the model supports. `TOKENADE_LOCAL_CTX` overrides
    /// (e.g. to cap KV-cache RAM on a constrained machine).
    fn n_ctx(model_train: u32) -> u32 {
        if let Some(v) = std::env::var("TOKENADE_LOCAL_CTX")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|&v| v >= 512)
        {
            return v;
        }
        // Fall back to a safe floor only if the model didn't report a train ctx.
        if model_train >= 512 {
            model_train
        } else {
            16384
        }
    }

    /// The loaded model, shared read-only across worker threads.
    pub struct Shared {
        model: *mut ffi::llama_model,
    }
    // SAFETY: after loading, `llama_model` is immutable and llama.cpp supports
    // creating contexts from it concurrently; we never mutate it.
    unsafe impl Send for Shared {}
    unsafe impl Sync for Shared {}
    impl Drop for Shared {
        fn drop(&mut self) {
            unsafe { ffi::llama_model_free(self.model) }
        }
    }

    pub fn load(model_path: &Path) -> Result<Shared, String> {
        BACKEND.call_once(|| unsafe { ffi::llama_backend_init() });
        let cpath = CString::new(model_path.to_string_lossy().into_owned())
            .map_err(|_| "model path has interior NUL".to_string())?;
        let model = unsafe {
            let params = ffi::llama_model_default_params();
            ffi::llama_model_load_from_file(cpath.as_ptr(), params)
        };
        if model.is_null() {
            return Err(format!("llama.cpp could not load {}", model_path.display()));
        }
        Ok(Shared { model })
    }

    /// A worker's inference context (owns a KV cache). Not shared between
    /// threads — created and used within one worker.
    pub struct Ctx {
        ctx: *mut ffi::llama_context,
        vocab: *const ffi::llama_vocab,
        n_ctx: u32,
    }
    impl Drop for Ctx {
        fn drop(&mut self) {
            unsafe { ffi::llama_free(self.ctx) }
        }
    }

    pub fn new_context(shared: &Shared) -> Result<Ctx, String> {
        // Pin the compute thread count. `available_parallelism()` is the sane
        // default, but the eval harness sets TOKENADE_LOCAL_THREADS to a FIXED
        // value: llama.cpp's CPU matmul reduction order depends on how work is
        // split across threads, so a stable thread count is a precondition for
        // bit-reproducible logits (and thus a reproducible eval).
        let n_threads = std::env::var("TOKENADE_LOCAL_THREADS")
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok())
            .filter(|n| *n >= 1)
            .unwrap_or_else(|| {
                std::thread::available_parallelism()
                    .map(|n| (n.get() as i32).max(1))
                    .unwrap_or(4)
            });
        // Default to the model's own trained context (Qwen3 40960, MiniCPM5
        // ~130k) so digests get the full window — but the KV cache for that is
        // large, so: (1) store it Q8_0 (~half the F16 RAM) with flash attention,
        // and (2) if allocation still fails (not enough RAM), halve n_ctx and
        // retry down to a floor. Net: "max context that actually fits", never a
        // crash. TOKENADE_LOCAL_CTX pins an exact value.
        let train = (unsafe { ffi::llama_model_n_ctx_train(shared.model) }).max(0) as u32;
        let mut nc = n_ctx(train);
        const FLOOR: u32 = 8192;
        loop {
            let ctx = unsafe {
                let mut p = ffi::llama_context_default_params();
                p.n_ctx = nc;
                p.n_threads = n_threads;
                p.n_threads_batch = n_threads;
                // Q8_0 K-cache halves half the KV RAM and is flash-attn-independent
                // (safe on this CPU build); V stays F16 so we don't require flash
                // attention, which some CPU builds don't support.
                p.type_k = ffi::GGML_TYPE_Q8_0;
                ffi::llama_init_from_model(shared.model, p)
            };
            if !ctx.is_null() {
                let vocab = unsafe { ffi::llama_model_get_vocab(shared.model) };
                return Ok(Ctx {
                    ctx,
                    vocab,
                    n_ctx: nc,
                });
            }
            if nc <= FLOOR {
                return Err(format!(
                    "llama.cpp could not create a context even at n_ctx={nc} \
                     (out of memory?)"
                ));
            }
            nc /= 2; // KV allocation likely failed on RAM — degrade and retry.
        }
    }

    fn tokenize(
        vocab: *const ffi::llama_vocab,
        text: &str,
        add_bos: bool,
    ) -> Result<Vec<ffi::llama_token>, String> {
        let c = CString::new(text).map_err(|_| "prompt has interior NUL".to_string())?;
        let len = c.as_bytes().len() as i32;
        // A negative return is the (negated) required capacity.
        let need = unsafe {
            ffi::llama_tokenize(vocab, c.as_ptr(), len, ptr::null_mut(), 0, add_bos, true)
        };
        let cap = (-need).max(0) as usize;
        let mut toks = vec![0 as ffi::llama_token; cap];
        let got = unsafe {
            ffi::llama_tokenize(
                vocab,
                c.as_ptr(),
                len,
                toks.as_mut_ptr(),
                cap as i32,
                add_bos,
                true,
            )
        };
        if got < 0 {
            return Err("tokenize failed".into());
        }
        toks.truncate(got as usize);
        Ok(toks)
    }

    fn detok(vocab: *const ffi::llama_vocab, token: ffi::llama_token) -> String {
        let mut buf = [0 as c_char; 256];
        let n = unsafe {
            ffi::llama_token_to_piece(vocab, token, buf.as_mut_ptr(), buf.len() as i32, 0, true)
        };
        if n <= 0 {
            return String::new();
        }
        let bytes: Vec<u8> = buf[..n as usize].iter().map(|&b| b as u8).collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// Render the model's OWN embedded Jinja chat template with `minijinja`.
    /// llama.cpp's C `llama_chat_apply_template` only handles a fixed set of
    /// built-in templates and silently mismatches new models (e.g. MiniCPM5's
    /// custom Jinja) — which is exactly what produced garbage. We render the
    /// real template and surface any error (no blind fallback).
    fn render_chat(
        model: *mut ffi::llama_model,
        messages: &serde_json::Value,
        tools: &serde_json::Value,
        think: bool,
    ) -> Result<String, String> {
        let raw = unsafe { ffi::llama_model_chat_template(model, ptr::null()) };
        if raw.is_null() {
            return Err("model GGUF has no embedded chat template".into());
        }
        let tmpl = unsafe { CStr::from_ptr(raw) }
            .to_str()
            .map_err(|_| "chat template is not valid UTF-8".to_string())?
            .to_owned();

        let vocab = unsafe { ffi::llama_model_get_vocab(model) };
        let bos = detok(vocab, unsafe { ffi::llama_vocab_bos(vocab) });
        let eos = detok(vocab, unsafe { ffi::llama_vocab_eos(vocab) });

        let mut env = minijinja::Environment::new();
        // Templates reference optional vars (tools, documents…); treat missing
        // ones as empty rather than erroring.
        env.set_undefined_behavior(minijinja::UndefinedBehavior::Lenient);
        // Real chat templates call Python string methods (startswith/split/…).
        env.set_unknown_method_callback(minijinja_contrib::pycompat::unknown_method_callback);
        // HF templates call `tojson(ensure_ascii=False, indent=…)`; minijinja's
        // built-in tojson rejects those kwargs. Override with one that absorbs
        // them (serde_json already emits UTF-8 literals = ensure_ascii=False).
        env.add_filter(
            "tojson",
            |v: minijinja::Value,
             kwargs: minijinja::value::Kwargs|
             -> Result<minijinja::Value, minijinja::Error> {
                let _ = kwargs;
                let s = serde_json::to_string(&v).map_err(|e| {
                    minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, e.to_string())
                })?;
                Ok(minijinja::Value::from_safe_string(s))
            },
        );
        env.render_str(
            &tmpl,
            minijinja::context! {
                messages => messages,
                // Passing tools engages the model's NATIVE function-calling
                // path (its `{% if tools %}` block renders them with the tokens
                // it was trained on, e.g. MiniCPM5 <tools>/<tool_call>).
                tools => tools,
                bos_token => bos,
                eos_token => eos,
                add_generation_prompt => true,
                // Hybrid-thinking models (MiniCPM5, Qwen3): per-request gate.
                enable_thinking => think,
            },
        )
        .map_err(|e| format!("chat template render failed: {e}"))
    }

    fn build_sampler(
        vocab: *const ffi::llama_vocab,
        params: &GenParams,
    ) -> Result<*mut ffi::llama_sampler, String> {
        unsafe {
            let chain = ffi::llama_sampler_chain_init(ffi::llama_sampler_chain_default_params());
            if let Some(gbnf) = &params.grammar {
                // Grammar first: masks logits down to grammar-legal tokens, the
                // rest of the chain samples among those. NO repetition penalty —
                // penalising mandatory structural tokens ("," '"' etc.) fights
                // the grammar. Pure extraction wants near-greedy (temp 0.2);
                // a grammar with a leading <think> block wants the model's
                // recommended reasoning sampling (top-k/top-p, temp ~0.6).
                let g = CString::new(gbnf.as_str())
                    .map_err(|_| "grammar has interior NUL".to_string())?;
                let root = CString::new("root").unwrap();
                let gs = ffi::llama_sampler_init_grammar(vocab, g.as_ptr(), root.as_ptr());
                if gs.is_null() {
                    ffi::llama_sampler_free(chain);
                    return Err("invalid GBNF grammar".into());
                }
                ffi::llama_sampler_chain_add(chain, gs);
                let t = if params.think == Some(true) {
                    params.temperature.unwrap_or(0.6)
                } else {
                    params.temperature.unwrap_or(0.2)
                };
                if t <= 0.0 {
                    // Greedy: top_k(1) collapses the distribution to the single
                    // highest-logit token, so dist() picks it deterministically
                    // (no RNG draw, no temperature scaling). This is the exact
                    // argmax — the reproducible operating point the eval pins.
                    ffi::llama_sampler_chain_add(chain, ffi::llama_sampler_init_top_k(1));
                } else if params.think == Some(true) {
                    ffi::llama_sampler_chain_add(chain, ffi::llama_sampler_init_top_k(20));
                    ffi::llama_sampler_chain_add(chain, ffi::llama_sampler_init_top_p(0.95, 1));
                    ffi::llama_sampler_chain_add(chain, ffi::llama_sampler_init_temp(t));
                } else {
                    ffi::llama_sampler_chain_add(chain, ffi::llama_sampler_init_temp(t));
                }
                ffi::llama_sampler_chain_add(chain, ffi::llama_sampler_init_dist(0));
                return Ok(chain);
            }
            // Free-form: repetition penalty → top-k → top-p → temperature →
            // dist. Greedy loops on small models; this chain matches MiniCPM5's
            // recommended sampling.
            // Repetition penalty stays even in greedy mode — it breaks the
            // small-model loop without introducing any randomness.
            ffi::llama_sampler_chain_add(
                chain,
                ffi::llama_sampler_init_penalties(64, 1.15, 0.0, 0.0),
            );
            let t = params.temperature.unwrap_or(0.8);
            if t <= 0.0 {
                ffi::llama_sampler_chain_add(chain, ffi::llama_sampler_init_top_k(1));
            } else {
                ffi::llama_sampler_chain_add(chain, ffi::llama_sampler_init_top_k(40));
                ffi::llama_sampler_chain_add(chain, ffi::llama_sampler_init_top_p(0.95, 1));
                ffi::llama_sampler_chain_add(chain, ffi::llama_sampler_init_temp(t));
            }
            ffi::llama_sampler_chain_add(chain, ffi::llama_sampler_init_dist(0));
            Ok(chain)
        }
    }

    /// Generate a completion. Clears the worker context's KV cache first so it
    /// can be reused across requests.
    pub fn generate(
        shared: &Shared,
        ctx: &mut Ctx,
        messages: &serde_json::Value,
        tools: &serde_json::Value,
        params: &GenParams,
    ) -> Result<(String, usize, usize), String> {
        unsafe { ffi::llama_memory_clear(ffi::llama_get_memory(ctx.ctx), true) };
        let max_tokens = params.max_tokens;

        // Render the model's own chat template; it emits BOS, so add_bos=false.
        let think = params.think.unwrap_or_else(super::think_enabled);
        let prompt = render_chat(shared.model, messages, tools, think)?;
        let mut toks = tokenize(ctx.vocab, &prompt, false)?;
        let n_prompt = toks.len();
        if n_prompt == 0 {
            return Err("empty prompt".into());
        }
        if n_prompt + max_tokens >= ctx.n_ctx as usize {
            return Err(format!(
                "local: prompt {n_prompt} tok + {max_tokens} gen exceeds ctx {}",
                ctx.n_ctx
            ));
        }

        // Decode the prompt in n_batch-sized chunks. A single llama_decode with
        // more than n_batch (default 2048) tokens ABORTS the process — which is
        // exactly what crashed the worker on any prompt over ~2k tokens (a log
        // or file digest — the sub-agent's core job), leaving an empty response
        // body the client reported as "EOF while parsing a value". Chunking well
        // under n_batch keeps big prompts safe; positions auto-advance across
        // calls (same as the single-token gen loop below).
        const CHUNK: usize = 512;
        let mut off = 0;
        while off < n_prompt {
            let n = (n_prompt - off).min(CHUNK);
            let batch = unsafe { ffi::llama_batch_get_one(toks[off..].as_mut_ptr(), n as i32) };
            if unsafe { ffi::llama_decode(ctx.ctx, batch) } != 0 {
                return Err("decode(prompt) failed".into());
            }
            off += n;
        }

        let smpl = build_sampler(ctx.vocab, params)?;
        let mut out = String::new();
        let mut n_gen = 0usize;
        while n_gen < max_tokens {
            let tok = unsafe { ffi::llama_sampler_sample(smpl, ctx.ctx, -1) };
            if unsafe { ffi::llama_vocab_is_eog(ctx.vocab, tok) } {
                break;
            }
            out.push_str(&detok(ctx.vocab, tok));
            n_gen += 1;
            let mut one = [tok];
            let b = unsafe { ffi::llama_batch_get_one(one.as_mut_ptr(), 1) };
            if unsafe { ffi::llama_decode(ctx.ctx, b) } != 0 {
                break;
            }
        }
        unsafe { ffi::llama_sampler_free(smpl) };
        Ok((out, n_prompt, n_gen))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wants_local_detects_selectors() {
        assert!(wants_local("local"));
        assert!(wants_local("minicpm"));
        assert!(wants_local("  local  ")); // trimmed
        assert!(!wants_local("auto"));
        assert!(!wants_local("free"));
        assert!(!wants_local("opencode-zen/deepseek-v4-flash-free"));
    }

    #[test]
    fn addr_and_parallelism_have_sane_shape() {
        assert!(server_addr().contains(':'));
        assert!(parallelism() >= 1);
    }

    #[test]
    fn default_addr_is_loopback() {
        // Never bind a public interface by default.
        std::env::remove_var("TOKENADE_LOCAL_ADDR");
        assert!(server_addr().starts_with("127.0.0.1:"));
    }

    #[cfg(not(feature = "local-llm"))]
    #[test]
    fn feature_off_gives_rebuild_hint_not_panic() {
        let e = serve().unwrap_err();
        assert!(
            e.contains("local-llm"),
            "should point to the build feature: {e}"
        );
    }

    // Integration: needs a real GGUF + the local-llm build. Run with:
    //   TOKENADE_LOCAL_MODEL=/path/model.gguf cargo test -p tokenade-cli \
    //     --features local-llm -- --ignored local_
    #[cfg(feature = "local-llm")]
    #[test]
    #[ignore = "needs TOKENADE_LOCAL_MODEL gguf + built llama.cpp"]
    fn local_generate_roundtrip() {
        let model = std::env::var("TOKENADE_LOCAL_MODEL").expect("set TOKENADE_LOCAL_MODEL");
        let shared = engine::load(std::path::Path::new(&model)).expect("load model");
        let mut ctx = engine::new_context(&shared).expect("context");
        let msgs = serde_json::json!([{"role": "user", "content": "Reply with only the word: OK"}]);
        let params = engine::GenParams {
            max_tokens: 16,
            grammar: None,
            temperature: None,
            think: None,
        };
        let (text, n_in, n_out) =
            engine::generate(&shared, &mut ctx, &msgs, &serde_json::Value::Null, &params)
                .expect("generate");
        assert!(n_in > 0, "prompt tokenized");
        assert!(n_out > 0, "generated at least one token");
        assert!(!text.trim().is_empty(), "non-empty output, got {text:?}");
    }

    #[cfg(feature = "local-llm")]
    #[test]
    #[ignore = "needs TOKENADE_LOCAL_MODEL gguf + built llama.cpp"]
    fn local_chat_template_renders_nonempty() {
        // The model's own Jinja template must render (pycompat covers Python
        // string methods) — a render error here is the MiniCPM5-class bug.
        let model = std::env::var("TOKENADE_LOCAL_MODEL").expect("set TOKENADE_LOCAL_MODEL");
        let shared = engine::load(std::path::Path::new(&model)).expect("load model");
        let mut ctx = engine::new_context(&shared).expect("context");
        // Two turns to exercise the message loop in the template.
        let msgs = serde_json::json!([
            {"role": "system", "content": "You are terse."},
            {"role": "user", "content": "Hi"},
        ]);
        let params = engine::GenParams {
            max_tokens: 8,
            grammar: None,
            temperature: None,
            think: None,
        };
        let (text, _, n_out) =
            engine::generate(&shared, &mut ctx, &msgs, &serde_json::Value::Null, &params)
                .expect("generate");
        assert!(n_out > 0 && !text.trim().is_empty());
    }
}
