//! **The local server, and the lock that keeps it from existing twice.**
//!
//! A GGUF model loaded twice is twice its memory for a single answer. The
//! file lock is therefore half the work: the losers wait for the winner
//! instead of loading in turn.
//!
//! And the server NEVER starts on its own at import — local inference is a
//! choice, not a side effect.

use super::*;

/// Path of the cross-process lock that serialises server auto-start.
#[cfg(feature = "local-llm")]
pub(crate) fn serve_lock_path() -> PathBuf {
    models_dir()
        .parent()
        .map(|d| d.join("local-serve.lock"))
        .unwrap_or_else(|| PathBuf::from(".tokenade/local-serve.lock"))
}

/// Ensure the local model server is up, auto-starting it (detached) on first
/// use and waiting for it to load the model + become ready.
///
/// SINGLE-FLIGHT, cross-process. `health()`-then-spawn is a check-then-act race,
/// and the callers are separate PROCESSES (one per hook / CLI invocation), so an
/// in-process Mutex would not see them. Unserialised, K concurrent first-uses
/// each spawn their own `local-serve`, and every loser loads the FULL model plus
/// `parallelism()` KV contexts — several GB and N inference threads apiece —
/// before finally dying on the `TcpListener::bind`. That is the one path in this
/// repo that can bring a machine to its knees, which is exactly the failure class
/// we ship against. So: take an exclusive file lock first; the winner spawns and
/// waits, the losers block and then find `health()` already true and return.
#[cfg(feature = "local-llm")]
pub(crate) fn ensure_server(addr: &str) -> Result<(), String> {
    if health(addr) {
        return Ok(()); // fast path: no lock in the common (server-up) case
    }
    let lock_path = serve_lock_path();
    if let Some(d) = lock_path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    // A lock we cannot take must never become a lock we ignore: failing open
    // here would restore the stampede. Fall back to the unlocked path only when
    // the lock FILE itself is unusable (read-only HOME), where a stampede is
    // still better than never starting.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .ok();
    if let Some(f) = &lock {
        use fs2::FileExt;
        let _ = f.lock_exclusive(); // blocks until the winner is done
                                    // Re-check under the lock: whoever held it before us has already
                                    // started the server, so we must NOT spawn a second one.
        if health(addr) {
            let _ = FileExt::unlock(f);
            return Ok(());
        }
    }
    let res = spawn_and_wait(addr);
    if let Some(f) = &lock {
        use fs2::FileExt;
        let _ = FileExt::unlock(f);
    }
    res
}

/// Spawn a detached `local-serve` and poll until it answers health checks.
#[cfg(feature = "local-llm")]
pub(crate) fn spawn_and_wait(addr: &str) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("local-serve")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    tokenade_core::process_util::configure_no_window(&mut cmd);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn().map_err(|e| format!("spawn local-serve: {e}"))?;
    // Model load can take ~10-20s cold; poll readiness.
    for _ in 0..240 {
        if health(addr) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    Err("local model server did not become ready in time".into())
}

// ---- server (feature) ------------------------------------------------------

#[cfg(feature = "local-llm")]
pub(crate) struct Job {
    messages: serde_json::Value,
    tools: serde_json::Value,
    params: engine::GenParams,
    resp: std::sync::mpsc::Sender<Result<(String, usize, usize), String>>,
}

/// Run the local model server: load the model once, spawn a pool of worker
/// contexts, and serve OpenAI-compatible chat completions over TCP. Blocks.
#[cfg(feature = "local-llm")]
pub fn serve() -> Result<(), String> {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{mpsc, Arc, Mutex};

    let addr = server_addr();
    let model_path = ensure_model()?;
    let n = parallelism();

    // Load the model once; workers share it (LlamaModel is Send+Sync).
    let shared = Arc::new(engine::load(&model_path)?);
    let (tx, rx) = mpsc::channel::<Job>();
    let rx = Arc::new(Mutex::new(rx));
    for _ in 0..n {
        let rx = Arc::clone(&rx);
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || {
            // Each worker owns one context (its own KV cache) for its lifetime.
            let mut ctx = match engine::new_context(&shared) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[local-serve] worker context failed: {e}");
                    return;
                }
            };
            loop {
                let job = match rx.lock().unwrap().recv() {
                    Ok(j) => j,
                    Err(_) => break,
                };
                let r = engine::generate(&shared, &mut ctx, &job.messages, &job.tools, &job.params);
                let _ = job.resp.send(r);
            }
        });
    }

    // The SAME rule the llm-proxy applies to its own listener, asked at the
    // other bind site in this binary.
    //
    // `TOKENADE_LOCAL_ADDR` was taken verbatim. The default is loopback, so
    // nothing failed visibly, but `TOKENADE_LOCAL_ADDR=0.0.0.0:8919` bound an
    // UNAUTHENTICATED inference server — no token, no origin check, and a
    // tool-calling endpoint — to every interface on the machine. The rule that
    // refuses exactly this already existed, tested, in `llm_proxy::config`,
    // with a refusal message explaining why; this listener never asked it.
    //
    // Found by reading headroom's 2026-08-27 `fix(proxy): enforce
    // HEADROOM_PROXY_TOKEN on WebSocket handshakes`, which is the same shape:
    // one rule, two entry points, enforced at one.
    //
    // The escape hatch is deliberate and mirrors the proxy's: serving a model
    // to a LAN is a real thing to want, and it should be something you said on
    // purpose rather than something an example config did to you.
    if !crate::llm_proxy::config::listens_on_loopback(&addr)
        && std::env::var("TOKENADE_LOCAL_ALLOW_REMOTE").as_deref() != Ok("1")
    {
        return Err(format!(
            "refusing to listen on {addr}: that address is reachable from off this \
             machine, and this server answers model requests with no authentication. \
             Set TOKENADE_LOCAL_ALLOW_REMOTE=1 if that is what you meant."
        ));
    }
    let listener = TcpListener::bind(&addr).map_err(|e| format!("bind {addr}: {e}"))?;
    eprintln!(
        "[tokenade local-serve] ready on {addr} · {n} worker context(s) · {DEFAULT_MODEL_FILE}"
    );

    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let tx = tx.clone();
        std::thread::spawn(move || {
            let peer = stream.try_clone();
            let Ok(peer) = peer else { return };
            let mut reader = BufReader::new(peer);
            // Request line
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                return;
            }
            let is_health = line.starts_with("GET /health");
            // Headers → Content-Length
            let mut clen = 0usize;
            loop {
                let mut h = String::new();
                if reader.read_line(&mut h).is_err() || h == "\r\n" || h == "\n" || h.is_empty() {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                    clen = v.trim().parse().unwrap_or(0);
                }
            }
            if is_health {
                let _ = write_http(&mut stream, "ok");
                return;
            }
            let mut body = vec![0u8; clen];
            if reader.read_exact(&mut body).is_err() {
                let _ = write_http(&mut stream, r#"{"error":"bad request body"}"#);
                return;
            }
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
            // Pass messages + tools through as raw JSON so the chat template can
            // render tool schemas, assistant tool_calls and tool-role results.
            let messages = json
                .get("messages")
                .cloned()
                .unwrap_or_else(|| serde_json::Value::Array(vec![]));
            let tools = json
                .get("tools")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let params = engine::GenParams {
                max_tokens: json["max_tokens"].as_u64().unwrap_or(MAX_GEN_TOKENS as u64) as usize,
                grammar: json["grammar"].as_str().map(str::to_owned),
                temperature: json["temperature"].as_f64().map(|t| t as f32),
                think: json["think"].as_bool(),
            };

            let (rtx, rrx) = mpsc::channel();
            if tx
                .send(Job {
                    messages,
                    tools,
                    params,
                    resp: rtx,
                })
                .is_err()
            {
                let _ = write_http(&mut stream, r#"{"error":"server shutting down"}"#);
                return;
            }
            let out = match rrx.recv() {
                Ok(Ok((text, ptok, gtok))) => serde_json::json!({
                    "choices": [{"message": {"role": "assistant", "content": text}}],
                    "usage": {"prompt_tokens": ptok, "completion_tokens": gtok},
                })
                .to_string(),
                Ok(Err(e)) => serde_json::json!({ "error": e }).to_string(),
                Err(_) => r#"{"error":"worker dropped"}"#.to_string(),
            };
            let _ = write_http(&mut stream, &out);
            let _ = stream.flush();
        });
    }
    Ok(())
}

#[cfg(feature = "local-llm")]
pub(crate) fn write_http<W: std::io::Write>(w: &mut W, body: &str) -> std::io::Result<()> {
    write!(
        w,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}
