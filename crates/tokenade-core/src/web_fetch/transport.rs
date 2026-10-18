//! **The transport: the pooled client, and `curl` as a last resort.**
//!
//! A reused client saves one TLS handshake per target in a batch; `curl`
//! exists because some certificate chains only get through with it. The
//! fallback SAYS so — a transport warning kept quiet reads like an empty
//! page.

use super::*;
use std::sync::Arc;

/// Is `curl` actually on this machine?
///
/// Probed ONCE and cached. Until this existed, a missing curl was
/// indistinguishable from an unreachable site: `spawn()` failed, `spawn_curl`
/// returned `None`, and every caller read that as "the site did not answer".
/// On a minimal container or a stripped Windows image that meant EVERY web
/// feature died silently, with no diagnostic pointing at the cause.
pub fn curl_available() -> bool {
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let mut cmd = crate::process_util::internal_command("curl");
        cmd.arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        crate::process_util::configure_no_window(&mut cmd);
        let ok = cmd.status().map(|s| s.success()).unwrap_or(false);
        if !ok {
            crate::debug_log::log(
                "web-fetch",
                "curl_missing",
                serde_json::json!({
                    "effect": "the reqwest primary transport handles everything; \
                               only the rare rustls-refused handshake has no fallback",
                }),
            );
        }
        ok
    })
}

/// One-line explanation of a degraded transport, or `None` when all is well.
///
/// Since the migration the PRIMARY transport is the in-process reqwest+rustls
/// client — in-process, HTTP/2, with a cookie store — so a missing curl is no
/// longer a degradation: curl is only a handshake fallback for the rare server
/// rustls refuses. There is no ordinary degraded mode left to warn about.
pub fn transport_warning() -> Option<&'static str> {
    None
}

/// A per-host reqwest client, so a SEQUENCE of requests to one host reuses the
/// connection and the TLS handshake. reqwest pools connections per client, so a
/// cloned handle shares the pool.
///
/// The chain that pays for it is `notion_drain`, which issues up to
/// `NOTION_MAX_CHUNKS` (6) + `NOTION_MAX_COLLECTIONS` (4) = **ten** dependent
/// requests to `api.notion.com` — each cursor comes from the previous response,
/// so they cannot be batched; only a live connection helps.
pub(super) fn pooled_client(url: &str) -> reqwest::Client {
    use std::sync::{Mutex, OnceLock};
    static POOL: OnceLock<Mutex<std::collections::HashMap<String, reqwest::Client>>> =
        OnceLock::new();
    let host = crate::url::scheme_host(url)
        .map(|(s, h)| format!("{s}://{h}"))
        .unwrap_or_else(|| url.to_string());
    let pool = POOL.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    // A poisoned lock must not take the web down: fall back to a fresh client.
    let Ok(mut map) = pool.lock() else {
        return build_client(Some(Arc::new(reqwest::cookie::Jar::default())), false, url);
    };
    // Bounded so a long session cannot accumulate a client per host visited.
    const MAX_POOLED_HOSTS: usize = 16;
    if map.len() >= MAX_POOLED_HOSTS && !map.contains_key(&host) {
        map.clear();
    }
    map.entry(host)
        .or_insert_with(|| {
            build_client(Some(Arc::new(reqwest::cookie::Jar::default())), false, url)
        })
        .clone()
}

/// Fetch `url` reusing the connection to its host across calls.
///
/// For SEQUENCES of dependent requests to one host — the reader follow-ups.
/// Carries a per-host cookie jar (correct WITHIN one reader chain: a Notion
/// drain or a PR file list). Everything else should use [`fetch`], which uses a
/// fresh ephemeral jar and the polite-then-browser retry.
pub fn fetch_pooled(url: &str, opts: &FetchOpts) -> Option<Fetched> {
    let client = pooled_client(url);
    let owned = localized_headers(BROWSER_HEADERS, url);
    let headers: Vec<&str> = owned.iter().map(String::as_str).collect();
    match crate::runtime::block_on(reqwest_request(
        &client,
        url,
        BROWSER_UA,
        &headers,
        opts.post_body,
        opts.max_time,
        opts.cap,
    )) {
        ReqOutcome::Ok(f) => Some(f),
        _ => None,
    }
}

/// Spawn curl, drain bounded, report `(bytes, exit_ok)`. The one place the
/// product creates an HTTP subprocess.
pub(super) fn spawn_curl(args: &[String], cap: usize) -> Option<(Vec<u8>, bool, bool)> {
    let mut curl = crate::process_util::internal_command("curl");
    curl.args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    crate::process_util::configure_no_window(&mut curl);
    let mut child = curl.spawn().ok()?;
    let so = child.stdout.take()?;
    let thr = crate::process_util::drain_capped(so, cap);
    let ok = child.wait().map(|s| s.success()).unwrap_or(false);
    let (bytes, truncated) = thr.join().ok()?;
    Some((bytes, ok, truncated))
}

/// The demoted fallback: a single curl invocation, reached ONLY when reqwest's
/// rustls refused the handshake. Guarded on curl being present — if it is not,
/// there is genuinely nothing more to try and the caller gets `None`.
pub(super) fn curl_fallback(
    url: &str,
    ua: &str,
    opts: &FetchOpts,
    browser: bool,
) -> Option<Fetched> {
    if !curl_available() {
        return None;
    }
    crate::debug_log::log(
        "web-fetch",
        "curl_handshake_fallback",
        serde_json::json!({ "url": url }),
    );
    run_curl(url, ua, opts, browser)
}

/// One curl invocation.
pub(super) fn run_curl(url: &str, ua: &str, opts: &FetchOpts, browser: bool) -> Option<Fetched> {
    let mut args: Vec<String> = vec![
        "--max-time".into(),
        opts.max_time.into(),
        "--max-filesize".into(),
        "16777216".into(),
        "-L".into(),
        // Bound the redirect chain. curl's default is 50, and a site that loops
        // sends us round all fifty before we learn anything. tensorflow.org does
        // exactly that — see the cookie jar below.
        "--max-redirs".into(),
        "10".into(),
        "-sS".into(),
        "--compressed".into(),
        "-A".into(),
        ua.into(),
        "-w".into(),
        format!(
            "{META_SENTINEL}%{{http_code}}{META_SEP}%{{url_effective}}{META_SEP}%{{content_type}}"
        ),
    ];
    if browser {
        for h in localized_headers(BROWSER_HEADERS, url) {
            args.push("-H".into());
            args.push(h);
        }
    }
    // An EPHEMERAL cookie jar, for the duration of this one fetch chain.
    //
    // We send a full Chrome user-agent and then refuse every cookie, which is a
    // combination no real browser produces. Sites that gate on "the client must
    // accept our cookie" answer with a 302 that sets it — and, seeing it come
    // back missing, redirect again, forever.
    // `www.tensorflow.org/api_docs/python/tf/keras/Model`, measured:
    //
    //   full Chrome UA, no jar   → 302, 50 redirects, nothing delivered
    //   full Chrome UA, with jar → 200, 4 redirects, 107 KB
    //   short/absent UA, no jar  → 200, 0 redirects  (they only gate "browsers")
    //
    // Per-invocation and deleted below: this is what a browser does WITHIN one
    // navigation, not a session. Nothing is carried between fetches, so no
    // identity accumulates and no cookie outlives the call that received it.
    let jar = cookie_jar_path();
    if let Some(j) = jar.as_deref().and_then(std::path::Path::to_str) {
        args.push("-c".into());
        args.push(j.to_string());
        args.push("-b".into());
        args.push(j.to_string());
    }
    if let Some(p) = crate::net::resolve_proxy(url) {
        args.push("--proxy".into());
        args.push(p);
    }
    if let Some(body) = opts.post_body {
        args.push("-H".into());
        args.push("Content-Type: application/json".into());
        // `--data-binary` (not `-d`): `-d` strips newlines, and the body is
        // machine-built JSON that must reach the API byte-exact.
        args.push("--data-binary".into());
        args.push(body.to_string());
    }
    args.push(url.to_string());

    let spawned = spawn_curl(&args, opts.cap);
    if let Some(j) = &jar {
        let _ = std::fs::remove_file(j);
    }
    let (raw, ok, truncated) = spawned?;
    if !ok || raw.is_empty() {
        return None;
    }
    let mut f = split_meta_trailer(raw);
    f.truncated = truncated;
    (!f.bytes.is_empty()).then_some(f)
}

/// Split curl's `-w` trailer off a raw response.
///
/// Separate from the fetch so it is testable without a network. Byte search,
/// not `from_utf8`: the body can be a PDF or an image, and requiring the WHOLE
/// response to decode would lose the provenance on every binary fetch.
///
/// A missing or malformed trailer means "we cannot tell" and yields the body
/// unchanged with a 200 — a curl that never emitted one still behaves as before.
pub fn split_meta_trailer(raw: Vec<u8>) -> Fetched {
    let sb = META_SENTINEL.as_bytes();
    let Some(at) = (0..=raw.len().saturating_sub(sb.len()))
        .rev()
        .find(|&i| raw[i..].starts_with(sb))
    else {
        return Fetched {
            bytes: raw,
            status: 200,
            content_type: None,
            effective_url: None,
            native_blocked: false,
            truncated: false,
        };
    };
    let body = raw[..at].to_vec();
    let tail = String::from_utf8_lossy(&raw[at + sb.len()..]).into_owned();
    let mut it = tail.split(META_SEP);
    // Parse the LEADING digits, not the whole field: the trailer has gained a
    // field twice now, and requiring an exact number would have made every
    // status unparseable — silently reverting the fix that added it.
    let status = it
        .next()
        .map(|d| {
            d.trim()
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
        })
        .and_then(|d| d.parse::<u16>().ok())
        .unwrap_or(200);
    let eff = it
        .next()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map(str::to_string);
    let ct = it
        .next()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(str::to_string);
    Fetched {
        bytes: body,
        status,
        content_type: ct,
        effective_url: eff,
        native_blocked: false,
        // `split_meta_trailer` only splits bytes it was handed; whether the
        // drain cut them is the caller's fact, stamped in `run_curl`.
        truncated: false,
    }
}

/// A unique, short-lived path for one fetch's cookie jar. `None` disables the
/// jar entirely (curl then behaves exactly as it did before), so a read-only or
/// missing temp dir degrades instead of failing the fetch.
pub fn cookie_jar_path() -> Option<std::path::PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join("tokenade-jars");
    std::fs::create_dir_all(&dir).ok()?;
    let n = N.fetch_add(1, Ordering::Relaxed);
    Some(dir.join(format!("{}-{n}.txt", std::process::id())))
}
