//! End-to-end tests for rank 4, against a real socket on both sides.
//!
//! Every test here stands up a fake provider on loopback and talks to the proxy
//! the way an agent would. Nothing is mocked at the HTTP layer, on purpose: the
//! defects this rung can ship — a relayed `Content-Length` that no longer
//! matches the body, a response held back until it is complete, a rewritten
//! body sent when it should not have been — are all invisible to a unit test
//! that hands `plan_outgoing` a struct and inspects what comes back.
//!
//! Two properties are asserted without a single `sleep`, because a test whose
//! verdict depends on a duration is a test that will one day be deleted for
//! flaking:
//!
//! * *fail-open* — the provider records the bytes it received and the test
//!   compares buffers;
//! * *streaming* — the provider REFUSES to send its second event until the
//!   client confirms it has seen the first. A proxy that buffered would wait
//!   for an event that is waiting for it, and the deadlock is detected on the
//!   condvar's timeout path rather than by choosing a sleep long enough.

use super::*;
use crate::llm_proxy::config::Deployment;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// What a fake provider was asked, kept so a test can compare bytes.
#[derive(Default)]
struct Seen {
    bodies: Mutex<Vec<Vec<u8>>>,
    headers: Mutex<Vec<Vec<(String, String)>>>,
    /// The request TARGET as the provider received it. Recorded because the
    /// proxy composes `upstream + target`, and nothing was checking that the
    /// composition lands on a real route.
    targets: Mutex<Vec<String>>,
    methods: Mutex<Vec<String>>,
}

impl Seen {
    fn body(&self, i: usize) -> Vec<u8> {
        self.bodies
            .lock()
            .unwrap()
            .get(i)
            .cloned()
            .unwrap_or_default()
    }
    fn count(&self) -> usize {
        self.bodies.lock().unwrap().len()
    }
    fn target(&self, i: usize) -> String {
        self.targets
            .lock()
            .unwrap()
            .get(i)
            .cloned()
            .unwrap_or_default()
    }
    fn header(&self, i: usize, name: &str) -> Option<String> {
        self.headers.lock().unwrap().get(i).and_then(|hs| {
            hs.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.clone())
        })
    }
}

/// Stand up a provider that answers every request with `respond`.
///
/// One thread per connection: the HTTP client upstream of us pools connections
/// and holds them open, so a single-threaded provider would accept one and
/// never reach the next.
fn provider<F>(respond: F) -> (String, Arc<Seen>)
where
    F: Fn(&mut TcpStream) + Send + Sync + 'static,
{
    let l = TcpListener::bind("127.0.0.1:0").expect("bind provider");
    let addr = l.local_addr().unwrap();
    let seen = Arc::new(Seen::default());
    let out = Arc::clone(&seen);
    let respond = Arc::new(respond);
    std::thread::spawn(move || {
        for stream in l.incoming() {
            let Ok(mut stream) = stream else { continue };
            let seen = Arc::clone(&seen);
            let respond = Arc::clone(&respond);
            std::thread::spawn(move || {
                let Ok(peer) = stream.try_clone() else { return };
                let mut reader = BufReader::new(peer);
                while let Ok(req) = http::read_request(&mut reader) {
                    seen.bodies.lock().unwrap().push(req.body.clone());
                    seen.headers.lock().unwrap().push(req.headers.clone());
                    seen.targets.lock().unwrap().push(req.target.clone());
                    seen.methods.lock().unwrap().push(req.method.clone());
                    respond(&mut stream);
                }
            });
        }
    });
    (format!("http://{addr}"), out)
}

/// A provider that answers with one buffered JSON body.
fn json_provider(body: &'static str) -> (String, Arc<Seen>) {
    provider(move |s| {
        let _ = write!(
            s,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = s.flush();
    })
}

/// Records every turn, so a test can assert what was counted.
struct Collect(Arc<Mutex<Vec<TurnRecord>>>);
impl Accounting for Collect {
    fn record(&self, t: &TurnRecord) {
        self.0.lock().unwrap().push(t.clone());
    }
}

/// Start a proxy in front of `cfg.upstream`; returns its address and its books.
fn start(cfg: Config, fold: Folder) -> (String, Arc<Mutex<Vec<TurnRecord>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind proxy");
    let addr = listener.local_addr().unwrap().to_string();
    let books = Arc::new(Mutex::new(Vec::new()));
    let proxy = Arc::new(Proxy::new(
        Config {
            listen: addr.clone(),
            ..cfg
        },
        fold,
        Box::new(Collect(Arc::clone(&books))),
    ));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let p = Arc::clone(&proxy);
            std::thread::spawn(move || p.handle_for_test(stream));
        }
    });
    (addr, books)
}

/// A fold that halves anything long enough, so a test can predict the result.
fn halving_fold() -> Folder {
    Arc::new(|s: &str, _h: Option<&str>| (s.len() > 40).then(|| s[..s.len() / 2].to_string()))
}

/// A fold that never folds.
fn no_fold() -> Folder {
    Arc::new(|_: &str, _: Option<&str>| None)
}

fn cfg_for(upstream: &str) -> Config {
    Config {
        upstream: upstream.to_string(),
        ..Config::default()
    }
}

/// A history whose only tool result is `n` foldable bytes.
fn history(n: usize) -> String {
    format!(
        r#"{{"messages":[{{"role":"tool","content":"{}"}}]}}"#,
        "y".repeat(n)
    )
}

/// Send one request to the proxy and read the whole response back, de-chunking.
fn round_trip(proxy: &str, body: &str, extra: &[(&str, &str)]) -> (u16, String) {
    let mut s = TcpStream::connect(proxy).expect("connect proxy");
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut req = format!(
        "POST /v1/messages HTTP/1.1\r\nHost: proxy\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (k, v) in extra {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("Connection: close\r\n\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes()).unwrap();
    s.flush().unwrap();
    read_response(&mut s)
}

/// The same thing with a BINARY body. The vendor editors' protocol is not
/// UTF-8, and a helper that takes a `&str` cannot post it.
fn round_trip_bytes(proxy: &str, body: &[u8], extra: &[(&str, &str)]) -> (u16, String) {
    let mut s = TcpStream::connect(proxy).expect("connect proxy");
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut req = format!(
        "POST /v1/messages HTTP/1.1\r\nHost: proxy\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (k, v) in extra {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("Connection: close\r\n\r\n");
    s.write_all(req.as_bytes()).unwrap();
    s.write_all(body).unwrap();
    s.flush().unwrap();
    read_response(&mut s)
}

/// A protobuf message carrying a tool result, as an editor speaking Connect
/// sends one: `[field 1 = message][field 2 = the big text]`.
fn proto_body(text: &str) -> Vec<u8> {
    fn varint(out: &mut Vec<u8>, mut v: u64) {
        loop {
            let mut b = (v & 0x7f) as u8;
            v >>= 7;
            if v != 0 {
                b |= 0x80;
            }
            out.push(b);
            if v == 0 {
                return;
            }
        }
    }
    let mut inner = Vec::new();
    varint(&mut inner, (2 << 3) | 2);
    varint(&mut inner, text.len() as u64);
    inner.extend_from_slice(text.as_bytes());
    let mut outer = Vec::new();
    varint(&mut outer, (1 << 3) | 2);
    varint(&mut outer, inner.len() as u64);
    outer.extend_from_slice(&inner);
    outer
}

/// Without a schema, a long string may be the prompt or a file sent as
/// context: it is not folded — only command OUTPUT is folded.
#[test]
fn a_protobuf_prompt_is_not_folded() {
    let (up, seen) = json_provider(r#"{"ok":true}"#);
    let (proxy, _) = start(cfg_for(&up), halving_fold());
    let prose = "Please refactor the parser so that it keeps the comments. ".repeat(60);
    let body = proto_body(&prose);
    round_trip_bytes(
        &proxy,
        &body,
        &[("content-type", "application/connect+proto")],
    );
    assert_eq!(seen.body(0).len(), body.len());
}

/// The vendor editors' protocol crosses rank 4 and COMES OUT FOLDED.
///
/// This is the cell four agents of the table — Cursor, Windsurf, Devin
/// Desktop, Antigravity — carried as `Partial` with the sole proof "an
/// unrecognised body is relayed untouched". The schema was not needed: the
/// wire format carries the field number and its type.
#[test]
fn a_protobuf_body_is_folded_on_the_way_out() {
    let (up, seen) = json_provider(r#"{"ok":true}"#);
    let (proxy, _) = start(cfg_for(&up), halving_fold());
    let text: String = (0..200)
        .map(|i| {
            format!(
                "2026-07-04T10:00:{:02} INFO worker {i} status=ok latency={}ms\n",
                i % 60,
                i % 13
            )
        })
        .collect();
    assert_eq!(
        tokenade_core::auto::detect(&text),
        tokenade_core::auto::Kind::Logs
    );
    let body = proto_body(&text);
    let (status, _) = round_trip_bytes(
        &proxy,
        &body,
        &[("content-type", "application/connect+proto")],
    );
    assert_eq!(status, 200);
    let got = seen.body(0);
    assert!(
        got.len() < body.len(),
        "the provider must receive less than what the agent sent: {} vs {}",
        got.len(),
        body.len()
    );
    // And what it receives is still protobuf: the first byte is the header
    // of field 1, not a text fragment.
    assert_eq!(got[0], (1 << 3) | 2);
}

/// The reverse promise, and it is the one that protects sessions: a protobuf
/// body with nothing foldable reaches the provider BYTE FOR BYTE.
#[test]
fn a_protobuf_body_with_nothing_to_fold_reaches_the_provider_untouched() {
    let (up, seen) = json_provider(r#"{"ok":true}"#);
    let (proxy, _) = start(cfg_for(&up), no_fold());
    let body = proto_body(&"z".repeat(8000));
    let (status, _) = round_trip_bytes(&proxy, &body, &[("content-type", "application/proto")]);
    assert_eq!(status, 200);
    assert_eq!(seen.body(0), body, "no byte may move");
}

/// A binary body that is NOT valid protobuf is not guessed at.
#[test]
fn a_binary_body_that_is_not_protobuf_is_relayed_verbatim() {
    let (up, seen) = json_provider(r#"{"ok":true}"#);
    let (proxy, _) = start(cfg_for(&up), halving_fold());
    let body: Vec<u8> = (0..4000u32).map(|i| (i % 251) as u8).collect();
    let (status, _) = round_trip_bytes(&proxy, &body, &[("content-type", "application/proto")]);
    assert_eq!(status, 200);
    assert_eq!(seen.body(0), body);
}

/// The `content-type` decides, and JSON must NEVER enter through this path:
/// it has its own, and letting it through twice would be the double
/// declaration this project refuses.
#[test]
fn a_json_body_never_takes_the_protobuf_path() {
    let (up, seen) = json_provider(r#"{"ok":true}"#);
    let (proxy, _) = start(cfg_for(&up), halving_fold());
    let body = history(4000);
    let (status, _) = round_trip(
        &proxy,
        &body,
        &[("content-type", "application/connect+json")],
    );
    assert_eq!(status, 200);
    let got = String::from_utf8_lossy(&seen.body(0)).to_string();
    // Folded by the JSON path, once only: exactly half.
    assert!(got.contains("\"role\":\"tool\""), "still JSON: {got:.80}");
    assert!(got.len() < body.len());
}

/// Read one HTTP response, de-chunking if it is framed that way.
fn read_response(s: &mut TcpStream) -> (u16, String) {
    let mut r = BufReader::new(s);
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    let status: u16 = line
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let mut chunked = false;
    let mut len = 0usize;
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
            break;
        }
        let low = h.to_ascii_lowercase();
        if low.starts_with("transfer-encoding:") && low.contains("chunked") {
            chunked = true;
        }
        if let Some(v) = low.strip_prefix("content-length:") {
            len = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = Vec::new();
    if chunked {
        loop {
            let mut size = String::new();
            if r.read_line(&mut size).unwrap_or(0) == 0 {
                break;
            }
            let n = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
            if n == 0 {
                break;
            }
            let mut buf = vec![0u8; n];
            if r.read_exact(&mut buf).is_err() {
                break;
            }
            body.extend_from_slice(&buf);
            let mut crlf = [0u8; 2];
            let _ = r.read_exact(&mut crlf);
        }
    } else {
        let mut buf = vec![0u8; len];
        let _ = r.read_exact(&mut buf);
        body = buf;
    }
    (status, String::from_utf8_lossy(&body).into_owned())
}

#[test]
fn the_substring_scan_finds_a_needle_wherever_it_sits() {
    // It decides whether a request is priced as cached, so a miss silently
    // switches the cache gate to its permissive half. The edges are where a
    // hand-rolled scan goes wrong: at the very end, straddling a false start,
    // and on a haystack shorter than the needle.
    assert!(contains(b"cache_control", b"cache_control"));
    assert!(contains(b"xxcache_control", b"cache_control"));
    assert!(contains(b"cache_contro-cache_control", b"cache_control"));
    assert!(contains(
        b"\"system\":[{\"cache_control\":{}}]",
        b"cache_control"
    ));
    assert!(!contains(b"cache_contro", b"cache_control"));
    assert!(!contains(b"", b"cache_control"));
    assert!(!contains(b"ccccccccccccc", b"cache_control"));
    // A needle at the last possible offset must still be found.
    let mut hay = vec![b'q'; 5000];
    hay.extend_from_slice(b"cache_control");
    assert!(contains(&hay, b"cache_control"));
}

// ---------------------------------------------------------------------------
// Fail-open: the property that outranks every other one here.
// ---------------------------------------------------------------------------

#[test]
fn a_body_we_cannot_parse_reaches_the_provider_byte_for_byte() {
    let (up, seen) = json_provider(r#"{"ok":true}"#);
    let (proxy, _) = start(cfg_for(&up), halving_fold());
    // Not JSON at all. A proxy that had to understand a request in order to
    // relay it would end this session here.
    let raw = "this is not json, and a long enough line that a fold would want it";
    let (status, _) = round_trip(&proxy, raw, &[]);
    assert_eq!(status, 200);
    assert_eq!(
        seen.body(0),
        raw.as_bytes(),
        "an unparseable body must be relayed unchanged"
    );
}

#[test]
fn an_unrecognised_wire_is_relayed_unchanged() {
    let (up, seen) = json_provider(r#"{"ok":true}"#);
    let (proxy, _) = start(cfg_for(&up), halving_fold());
    // Valid JSON, no `messages` and no `input`: recognised as nothing, and
    // A shape we do not recognise is relayed without guessing.
    let raw =
        r#"{"prompt":"a long prompt that a fold would happily cut in half if it were allowed"}"#;
    round_trip(&proxy, raw, &[]);
    assert_eq!(seen.body(0), raw.as_bytes());
}

#[test]
fn a_compactor_that_panics_relays_the_original_and_keeps_the_session() {
    // The reason `panic = "unwind"` is set at the workspace level. Under
    // `panic = "abort"` the catch is inert, and what this test would show is
    // the whole test binary dying — which is what a user's session would do.
    let (up, seen) = json_provider(r#"{"ok":true}"#);
    let exploding: Folder = Arc::new(|_: &str, _: Option<&str>| panic!("compactor blew up"));
    let (proxy, _) = start(cfg_for(&up), exploding);
    let raw = history(500);
    let (status, _) = round_trip(&proxy, &raw, &[]);
    assert_eq!(
        status, 200,
        "a panic in the compactor must not fail the turn"
    );
    assert_eq!(seen.body(0), raw.as_bytes());
}

#[test]
fn an_unreachable_provider_answers_an_error_the_agent_can_parse() {
    // Nothing has been said to the agent yet, so we owe it a well-formed
    // answer rather than a closed socket — an agent that gets EOF mid-request
    // usually reports it as a bug in itself.
    let dead = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = dead.local_addr().unwrap();
    drop(dead);
    let (proxy, _) = start(cfg_for(&format!("http://{addr}")), no_fold());
    let (status, body) = round_trip(&proxy, r#"{"messages":[]}"#, &[]);
    assert_eq!(status, 502);
    let v: serde_json::Value = serde_json::from_str(&body).expect("a JSON error envelope");
    assert_eq!(v["error"]["type"], "tokenade_proxy_error");
    // And it must not say where it was going.
    assert!(!body.contains("127.0.0.1"), "{body}");
}

#[test]
fn the_path_the_agent_asked_for_is_appended_to_the_upstream_exactly_once() {
    // The proxy composes `upstream + target`, so the base URL an agent is
    // pointed at must carry NO path of its own: any prefix there is prepended
    // to every target and then joined to an upstream that already has one,
    // giving `…/v1/v1/responses`. A fake provider that answers 200 to anything
    // hides this completely, which is why the target is now recorded.
    let (up, seen) = json_provider("{}");
    let (proxy, _) = start(
        Config {
            // What an agent's config actually holds: a base URL WITH a path.
            upstream: format!("{up}/v1"),
            ..cfg_for(&up)
        },
        no_fold(),
    );
    let mut s = TcpStream::connect(&proxy).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let body = r#"{"messages":[]}"#;
    write!(
        s,
        "POST /responses HTTP/1.1\r\nHost: p\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    s.flush().unwrap();
    read_response(&mut s);
    assert_eq!(
        seen.target(0),
        "/v1/responses",
        "the upstream's own path plus the agent's path, joined once"
    );
}

// ---------------------------------------------------------------------------
// The wire, then the rewrite.
// ---------------------------------------------------------------------------

#[test]
fn a_tool_result_in_the_history_is_folded_on_the_way_out() {
    let (up, seen) = json_provider(r#"{"usage":{"input_tokens":10,"output_tokens":2}}"#);
    let (proxy, _) = start(cfg_for(&up), halving_fold());
    let raw = history(4000);
    round_trip(&proxy, &raw, &[]);
    let sent = seen.body(0);
    assert!(
        sent.len() < raw.len(),
        "the history went out at {} bytes against {}",
        sent.len(),
        raw.len()
    );
    let v: serde_json::Value = serde_json::from_slice(&sent).unwrap();
    assert_eq!(v["messages"][0]["content"].as_str().unwrap().len(), 2000);
}

#[test]
fn compaction_off_means_the_bytes_are_untouched() {
    let (up, seen) = json_provider("{}");
    let (proxy, _) = start(
        Config {
            compaction: false,
            ..cfg_for(&up)
        },
        halving_fold(),
    );
    let raw = history(4000);
    round_trip(&proxy, &raw, &[]);
    assert_eq!(seen.body(0), raw.as_bytes());
}

#[test]
fn a_higher_rung_on_the_request_stops_us_folding() {
    // One authority per payload. The hook has already folded these results; a second pass
    // re-summarises a summary, and that sequence does not even converge.
    let (up, seen) = json_provider("{}");
    let (proxy, _) = start(cfg_for(&up), halving_fold());
    let raw = history(4000);
    round_trip(&proxy, &raw, &[(authority::CHANNEL_HEADER, "hook")]);
    assert_eq!(
        seen.body(0),
        raw.as_bytes(),
        "a declared hook owns the payload"
    );
}

#[test]
fn the_named_agent_decides_whether_we_may_fold_at_all() {
    // One authority per payload end to end, and what it actually turns on.
    //
    // This asserted until 2026-08-20 that Claude Code's history is relayed
    // byte-for-byte because its hooks own every tool result. Measured, that is
    // false — the same history through the same proxy is 142 395 bytes on
    // claude-code and 62 445 on codex, and the difference is a payload no hook
    // touched. What decides is the MARKER on each block, not the agent name.
    //
    // So the agent that must be relayed untouched is one on a wire we do not
    // parse: there the body is not ours to read, let alone rewrite.
    let (up, seen) = json_provider("{}");
    let (proxy, _) = start(
        Config {
            agent: Some(tokenade_core::detect::Agent::Cursor),
            ..cfg_for(&up)
        },
        halving_fold(),
    );
    let raw = history(4000);
    round_trip(&proxy, &raw, &[]);
    assert_eq!(
        seen.body(0),
        raw.as_bytes(),
        "a proprietary wire is relayed, never rewritten"
    );

    // And an agent whose tool results nobody folds IS folded for — otherwise
    // the matrix is an off switch rather than a decision.
    let (up2, seen2) = json_provider("{}");
    let (proxy2, _) = start(
        Config {
            agent: Some(tokenade_core::detect::Agent::Codex),
            ..cfg_for(&up2)
        },
        halving_fold(),
    );
    round_trip(&proxy2, &raw, &[]);
    assert!(
        seen2.body(0).len() < raw.len(),
        "codex has nothing above it"
    );
}

#[test]
fn the_cache_gate_vetoes_a_rewrite_that_costs_more_than_it_saves() {
    // A tiny saving at the FRONT of a large history that the request has ASKED
    // to be cached: folding it invalidates the whole cached prefix behind it.
    // This is the case that makes a naive proxy lose the user money while
    // reporting a gain.
    let (up, seen) = json_provider("{}");
    let (proxy, _) = start(cfg_for(&up), halving_fold());
    let early = format!(
        r#"{{"system":[{{"type":"text","text":"s","cache_control":{{"type":"ephemeral"}}}}],"max_tokens":1,"messages":[{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"t","content":"{}"}}]}},{{"role":"user","content":"{}"}}]}}"#,
        "z".repeat(100),
        "q".repeat(400_000)
    );
    round_trip(&proxy, &early, &[]);
    assert_eq!(
        seen.body(0),
        early.as_bytes(),
        "50 bytes of saving does not justify re-writing a 400 KB cached prefix"
    );

    // The SAME shape with NO `cache_control` has no prefix to invalidate, and
    // the tokens it removes are billed at full input price on every later turn.
    // Refusing here would leave the rung switched off on every request that
    // does not opt into caching — which is most of them.
    let (up2, seen2) = json_provider("{}");
    let (proxy2, _) = start(cfg_for(&up2), halving_fold());
    let uncached = format!(
        r#"{{"max_tokens":1,"messages":[{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"t","content":"{}"}}]}},{{"role":"user","content":"{}"}}]}}"#,
        "z".repeat(4000),
        "q".repeat(400_000)
    );
    round_trip(&proxy2, &uncached, &[]);
    assert!(
        seen2.body(0).len() < uncached.len(),
        "with no prompt cache in play the fold costs nothing and must be taken"
    );
}

// ---------------------------------------------------------------------------
// Streaming.
// ---------------------------------------------------------------------------

#[test]
fn a_stream_reaches_the_agent_event_by_event_and_is_not_buffered() {
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let buffered = Arc::new(AtomicBool::new(false));
    let g = Arc::clone(&gate);
    let b = Arc::clone(&buffered);
    let (up, _) = provider(move |s| {
        let _ = write!(
            s,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n"
        );
        let ev1 =
            "event: message_start\ndata: {\"message\":{\"usage\":{\"input_tokens\":100}}}\n\n";
        let _ = write!(s, "{:X}\r\n{ev1}\r\n", ev1.len());
        let _ = s.flush();

        let (lock, cv) = &*g;
        let mut got = lock.lock().unwrap();
        while !*got {
            let (next, timeout) = cv.wait_timeout(got, Duration::from_secs(10)).unwrap();
            got = next;
            if timeout.timed_out() {
                // The client never saw event 1 while we held event 2 back, so
                // something between us is holding the whole response.
                b.store(true, Ordering::SeqCst);
                break;
            }
        }
        drop(got);
        let ev2 = "event: message_delta\ndata: {\"usage\":{\"output_tokens\":42}}\n\n";
        let _ = write!(s, "{:X}\r\n{ev2}\r\n0\r\n\r\n", ev2.len());
        let _ = s.flush();
    });

    let (proxy, _) = start(cfg_for(&up), no_fold());
    let mut s = TcpStream::connect(&proxy).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let body = r#"{"stream":true,"messages":[]}"#;
    write!(
        s,
        "POST /v1/messages HTTP/1.1\r\nHost: p\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    s.flush().unwrap();

    // Read only as far as the first event, then release the provider.
    let mut r = BufReader::new(s.try_clone().unwrap());
    let mut first = String::new();
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        first.push_str(&line);
        if line.contains("message_start") {
            break;
        }
    }
    {
        let (lock, cv) = &*gate;
        *lock.lock().unwrap() = true;
        cv.notify_all();
    }
    let mut rest = String::new();
    let _ = r.read_to_string(&mut rest);

    assert!(
        !buffered.load(Ordering::SeqCst),
        "the first event did not reach the agent until the response was complete"
    );
    assert!(first.contains("message_start"));
    assert!(
        rest.contains("message_delta"),
        "the rest of the stream: {rest:?}"
    );
}

#[test]
fn the_bill_is_read_off_the_stream_without_holding_it() {
    let (up, _) = provider(|s| {
        let _ = write!(
            s,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n"
        );
        for ev in [
            "event: message_start\ndata: {\"message\":{\"usage\":{\"input_tokens\":1200,\"cache_read_input_tokens\":900,\"output_tokens\":1}}}\n\n",
            "event: message_delta\ndata: {\"usage\":{\"output_tokens\":250}}\n\n",
        ] {
            let _ = write!(s, "{:X}\r\n{ev}\r\n", ev.len());
            let _ = s.flush();
        }
        let _ = write!(s, "0\r\n\r\n");
        let _ = s.flush();
    });
    let (proxy, books) = start(cfg_for(&up), no_fold());
    let (status, body) = round_trip(&proxy, r#"{"stream":true,"messages":[]}"#, &[]);
    assert_eq!(status, 200);
    assert!(body.contains("message_delta"));
    let u = books.lock().unwrap()[0]
        .usage
        .expect("the provider's own numbers");
    assert_eq!((u.input, u.output, u.cache_read), (1200, 250, 900));
}

#[test]
fn a_buffered_response_is_counted_from_its_body() {
    let (up, _) = json_provider(r#"{"usage":{"input_tokens":7,"output_tokens":9}}"#);
    let (proxy, books) = start(cfg_for(&up), no_fold());
    round_trip(&proxy, r#"{"messages":[]}"#, &[]);
    let u = books.lock().unwrap()[0]
        .usage
        .expect("usage from a non-streamed body");
    assert_eq!((u.input, u.output), (7, 9));
}

// ---------------------------------------------------------------------------
// Secrets, credit, and the connection itself.
// ---------------------------------------------------------------------------

#[test]
fn credentials_reach_the_provider_and_hop_by_hop_headers_do_not() {
    let (up, seen) = json_provider("{}");
    let (proxy, _) = start(cfg_for(&up), no_fold());
    round_trip(
        &proxy,
        r#"{"messages":[]}"#,
        &[
            ("x-api-key", "sk-ant-secret-value"),
            ("anthropic-version", "2023-06-01"),
        ],
    );
    // Relaying the key IS the job: the agent is authenticating, not us.
    assert_eq!(
        seen.header(0, "x-api-key").as_deref(),
        Some("sk-ant-secret-value")
    );
    assert_eq!(
        seen.header(0, "anthropic-version").as_deref(),
        Some("2023-06-01")
    );
    // And the framing of OUR hop must not be forwarded as if it were theirs.
    assert_eq!(seen.header(0, "connection"), None);
}

#[test]
fn the_same_conversation_re_sent_is_credited_once() {
    // One credit per payload, at the level that matters: the proxy sees the whole history
    // every turn, so a naive counter credits turn 1's saving on every turn.
    let (up, _) = json_provider("{}");
    let (proxy, books) = start(cfg_for(&up), halving_fold());
    let raw = history(4000);
    for _ in 0..3 {
        round_trip(&proxy, &raw, &[(authority::SESSION_HEADER, "s1")]);
    }
    let recs = books.lock().unwrap();
    assert_eq!(recs.len(), 3);
    assert!(recs[0].tokens_saved > 0, "the first send is a real saving");
    assert_eq!(
        (recs[1].tokens_saved, recs[2].tokens_saved),
        (0, 0),
        "re-sending the same history is not a new saving"
    );
    assert!(
        recs[1].tokens_already_credited > 0,
        "and the repeat is visible rather than silent"
    );
}

/// The real shape: the history GROWS by a message every call. Keyed on the
/// whole body, every call looked new and the whole history was re-credited
/// each time; per block, the old result is a re-read, only the new one a fold.
#[test]
fn a_growing_conversation_credits_each_block_once() {
    let (up, _) = json_provider("{}");
    let (proxy, books) = start(cfg_for(&up), halving_fold());
    let one = format!(
        r#"{{"messages":[{{"role":"tool","content":"{}"}}]}}"#,
        "y".repeat(4000)
    );
    let two = format!(
        r#"{{"messages":[{{"role":"tool","content":"{}"}},{{"role":"tool","content":"{}"}}]}}"#,
        "y".repeat(4000),
        "z".repeat(8000)
    );
    round_trip(&proxy, &one, &[]);
    round_trip(&proxy, &two, &[]);
    let recs = books.lock().unwrap();
    assert_eq!(
        recs[0].tokens_saved, 500,
        "first call: the first block folded"
    );
    assert_eq!(
        recs[1].tokens_saved, 1000,
        "second call: only the NEW block is a saving"
    );
    assert_eq!(
        recs[1].tokens_already_credited, 500,
        "the old one is a re-read spared"
    );
}

/// Codex asks `GET /models` through the proxy; it went upstream as a POST and
/// came back 405.
#[test]
fn a_get_is_relayed_as_a_get() {
    let (up, seen) = json_provider(r#"{"models":[]}"#);
    let (proxy, _) = start(cfg_for(&up), halving_fold());
    let mut s = TcpStream::connect(&proxy).unwrap();
    s.write_all(b"GET /models?client_version=1 HTTP/1.1\r\nHost: p\r\nConnection: close\r\n\r\n")
        .unwrap();
    let (status, _) = read_response(&mut s);
    assert_eq!(status, 200);
    assert_eq!(seen.methods.lock().unwrap()[0], "GET");
}

#[test]
fn one_connection_serves_several_turns() {
    let (up, seen) = json_provider(r#"{"ok":1}"#);
    let (proxy, _) = start(cfg_for(&up), no_fold());
    let mut s = TcpStream::connect(&proxy).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let body = r#"{"messages":[]}"#;
    for _ in 0..2 {
        write!(
            s,
            "POST /v1/messages HTTP/1.1\r\nHost: p\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        s.flush().unwrap();
        assert_eq!(read_response(&mut s).0, 200);
    }
    assert_eq!(seen.count(), 2, "both turns reached the provider");
}

#[test]
fn the_health_route_says_which_deployment_this_is() {
    // A container needs a healthcheck, and an operator needs to know WHICH of
    // the two deployments answered — the mode must be explicit everywhere
    // it is observable, not only in the startup banner.
    let (up, _) = json_provider("{}");
    let (proxy, _) = start(
        Config {
            deployment: Deployment::Local,
            ..cfg_for(&up)
        },
        no_fold(),
    );
    let mut s = TcpStream::connect(&proxy).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(
        s,
        "GET /__tokenade/health HTTP/1.1\r\nHost: p\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    s.flush().unwrap();
    let (status, body) = read_response(&mut s);
    assert_eq!(status, 200);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["deployment"], "local");
    assert_eq!(v["web_capabilities"], true);
    // And the VERSION, through the reader that uses it.
    //
    // `upgrade` replaces the binary without touching the daemons: a proxy
    // started before the update answers `ok` while serving stale code, and
    // the auto-install concludes "already there" (measured 2026-09-15 on
    // three services whose `/proc/<pid>/exe` returned `… (deleted)`). The
    // announced version is what makes this case VISIBLE — a field no test
    // reads would vanish at the first cleanup of this body.
    assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        crate::llm_proxy::cli::version_in_health_body(&body).as_deref(),
        Some(env!("CARGO_PKG_VERSION")),
        "the supervisor's reader must read back what write_health writes"
    );
}

/// A proxy that DOES NOT STATE its version is not this build.
///
/// This is the half that decides the repair: answering "yes" when in doubt
/// would reproduce exactly the failure the field exists to see, and a proxy
/// older than the field is, by construction, the previous version.
#[test]
fn a_proxy_that_does_not_name_its_version_is_not_this_build() {
    use crate::llm_proxy::cli::serving_this_build;
    assert!(serving_this_build(Some(env!("CARGO_PKG_VERSION"))));
    assert!(!serving_this_build(None));
    assert!(!serving_this_build(Some("0.0.1")));
    // A health body WITHOUT the field reads as `None`, not an empty version.
    assert_eq!(
        crate::llm_proxy::cli::version_in_health_body(
            r#"HTTP/1.1 200 OK

{"ok":true,"deployment":"local"}"#
        ),
        None
    );
}

#[test]
fn a_request_that_is_not_http_is_refused_without_echoing_it() {
    let (up, _) = json_provider("{}");
    let (proxy, _) = start(cfg_for(&up), no_fold());
    let mut s = TcpStream::connect(&proxy).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    // A key in the request line is the reason `write_error` never echoes.
    s.write_all(b"GARBAGE sk-ant-secret\r\n\r\n").unwrap();
    s.flush().unwrap();
    let (status, body) = read_response(&mut s);
    assert_eq!(status, 400);
    assert!(!body.contains("sk-ant-secret"), "{body}");
}

// ---------------------------------------------------------------------------
// Secrets never reach the provider, whatever the block's length.
// ---------------------------------------------------------------------------

/// A short tool result carrying a credential is scrubbed, even though it is far
/// below the size where compacting starts paying.
///
/// This is the whole difference between "secret redaction" existing on this
/// rung and not. Before, the proxy's redaction ran inside the compactor, so it
/// fired only on blocks big enough to be worth folding — a 60-character
/// `AWS_SECRET_ACCESS_KEY=…` sailed straight through to the provider, and
/// nothing in the product said so.
#[test]
fn a_short_tool_result_with_a_secret_is_still_scrubbed() {
    let key = "AKIAIOSFODNN7EXAMPLE";
    let secret = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
    let raw = format!(
        r#"{{"model":"gpt-5","messages":[{{"role":"tool","content":"AWS_ACCESS_KEY_ID={key}\nAWS_SECRET_ACCESS_KEY={secret}"}}]}}"#
    );
    assert!(
        raw.len() < 800,
        "the point of this test is a block BELOW the fold floor"
    );

    let out =
        crate::llm_proxy::compact_outgoing(raw.as_bytes(), |s, h| default_folder()(s, h), None)
            .expect("a body carrying a credential must be rewritten");
    let sent = String::from_utf8(out.0).unwrap();

    assert!(
        !sent.contains(secret),
        "the secret reached the provider:\n{sent}"
    );
    assert!(
        out.1.redacted,
        "the rewrite must declare itself a redaction"
    );
}

/// The cache gate may decline a fold; it may not decline a REDACTION. A
/// short tool result with a token, inside a history the provider caches: the
/// gate weighed the (nil) saving against the cached suffix and relayed the
/// original — the token included.
#[test]
fn a_declined_fold_still_goes_out_scrubbed() {
    let (up, seen) = json_provider("{}");
    let (proxy, _) = start(cfg_for(&up), default_folder());
    let token = format!("ghp_{}", "a1B2c3D4e5F6".repeat(3));
    assert_ne!(
        tokenade_core::redact::redact_for_model(&token),
        token,
        "the fixture must be a credential"
    );
    let raw = format!(
        r#"{{"model":"claude-sonnet-4-5","max_tokens":10,"messages":[{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"t1","content":"GITHUB_TOKEN={token}"}}]}},{{"role":"assistant","content":"ok"}},{{"role":"user","content":[{{"type":"text","text":"{}","cache_control":{{"type":"ephemeral"}}}}]}}]}}"#,
        "q".repeat(60_000)
    );
    round_trip(&proxy, &raw, &[]);
    let sent = String::from_utf8(seen.body(0)).unwrap();
    assert!(!sent.contains(&token), "the token reached the provider");
}

/// And the rewrite survives even when scrubbing makes the body LONGER.
///
/// `compact_outgoing` refuses a swap that grows the request, because a proxy
/// that sends more than it received while claiming to compact is the ledger lie
/// this project keeps finding. Applied to a redaction, that rule drops the safe
/// body and forwards the raw key — the exact trap the openclaw bridge wrote
/// down and this path had unremarked.
#[test]
fn a_redaction_that_grows_the_body_is_not_thrown_away() {
    // A token short enough that `<redacted>` is longer than what it replaces.
    let raw = r#"{"model":"gpt-5","messages":[{"role":"tool","content":"ghp_1234567890abcdefghijklmnopqrstuvwxyzAB"}]}"#;
    let out =
        crate::llm_proxy::compact_outgoing(raw.as_bytes(), |s, h| default_folder()(s, h), None);
    let Some((bytes, rewrite, _)) = out else {
        panic!("a credential-bearing body must be rewritten even if it grows")
    };
    let sent = String::from_utf8(bytes).unwrap();
    assert!(
        !sent.contains("ghp_1234567890abcdefghijklmnopqrstuvwxyzAB"),
        "{sent}"
    );
    assert!(rewrite.redacted);
}

// ---------------------------------------------------------------------------
// One process, every agent, many sessions at once.
// ---------------------------------------------------------------------------

/// A loopback address nobody is listening on yet.
fn free_port() -> String {
    let l = TcpListener::bind("127.0.0.1:0").expect("bind");
    let a = l.local_addr().unwrap().to_string();
    drop(l);
    a
}

/// Wait until something answers on `addr`, or give up.
///
/// Polled rather than slept: the verdict is "did it come up", never "within
/// exactly N ms", so a loaded machine makes this slower and not flakier.
fn wait_until_bound(addr: &str) {
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if TcpStream::connect(addr).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("{addr} never came up");
}

/// Two agents, two providers, ONE process — and each agent reaches its own.
///
/// The operator's requirement: three agents on a machine must not mean three
/// proxies. The trap it protects against is subtler than "too many processes":
/// one listener shared by two agents cannot know which upstream a connection
/// belongs to, so one agent's traffic — and its credentials — would reach the
/// other's provider. The PORT carries that identity.
#[test]
fn one_process_serves_several_agents_each_to_its_own_provider() {
    let (up_a, seen_a) = json_provider(r#"{"id":"a","object":"chat.completion"}"#);
    let (up_b, seen_b) = json_provider(r#"{"id":"b","object":"chat.completion"}"#);

    let port_a = free_port();
    let port_b = free_port();
    let mk = |listen: String, upstream: String| {
        Proxy::new(
            Config {
                listen,
                upstream,
                ..Config::default()
            },
            no_fold(),
            Box::new(NoAccounting),
        )
    };
    let a = mk(port_a.clone(), up_a);
    let b = mk(port_b.clone(), up_b);
    std::thread::spawn(move || {
        let _ = super::serve_all(vec![a, b]);
    });
    wait_until_bound(&port_a);
    wait_until_bound(&port_b);

    round_trip(&port_a, r#"{"model":"m","messages":[]}"#, &[]);
    round_trip(&port_b, r#"{"model":"m","messages":[]}"#, &[]);

    assert_eq!(seen_a.count(), 1, "provider A saw the wrong count");
    assert_eq!(seen_b.count(), 1, "provider B saw the wrong count");
}

/// One agent, several SIMULTANEOUS sessions, all in flight together.
///
/// An agent can hold more than one conversation open, and a proxy that served
/// them one after another would turn parallel sessions into a queue. The
/// provider blocks until every session has arrived, so this can only finish if
/// they were concurrent — a serialising proxy hangs instead of passing slowly.
#[test]
fn several_simultaneous_sessions_of_one_agent_are_served_at_once() {
    const N: usize = 4;
    let gate = Arc::new((Mutex::new(0usize), Condvar::new()));
    let g = Arc::clone(&gate);
    let (upstream, _seen) = provider(move |s: &mut TcpStream| {
        {
            let (lock, cv) = &*g;
            let mut n = lock.lock().unwrap();
            *n += 1;
            if *n >= N {
                cv.notify_all();
            } else {
                let _ = cv
                    .wait_timeout_while(n, Duration::from_secs(20), |n| *n < N)
                    .expect("gate");
            }
        }
        let _ = write!(
            s,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}"
        );
        let _ = s.flush();
    });

    let listen = free_port();
    let proxy = Proxy::new(
        Config {
            listen: listen.clone(),
            upstream,
            ..Config::default()
        },
        no_fold(),
        Box::new(NoAccounting),
    );
    std::thread::spawn(move || {
        let _ = super::serve_all(vec![proxy]);
    });
    wait_until_bound(&listen);

    let sessions: Vec<_> = (0..N)
        .map(|i| {
            let l = listen.clone();
            std::thread::spawn(move || {
                round_trip(
                    &l,
                    &format!(r#"{{"model":"m","session":{i},"messages":[]}}"#),
                    &[],
                )
            })
        })
        .collect();
    for s in sessions {
        let (status, _) = s.join().expect("a session thread panicked");
        assert_eq!(status, 200);
    }
    assert_eq!(
        *gate.0.lock().unwrap(),
        N,
        "not every session reached the provider"
    );
}

/// A block the provider cached RAW stays raw: folding it now would make the
/// provider write the rest of the history to cache again. A block that went
/// out folded keeps going out folded.
#[test]
fn a_block_cached_raw_is_not_folded_later() {
    let (up, seen) = json_provider("{}");
    let (proxy, _) = start(cfg_for(&up), halving_fold());
    let tool = |c: &str| format!(r#"{{"role":"tool","content":"{c}"}}"#);
    let (raw_old, new) = ("x".repeat(4000), "y".repeat(4000));
    // `raw_old` sits before the last assistant message and was never folded.
    let first = format!(
        r#"{{"messages":[{},{{"role":"assistant","content":"ok"}},{}]}}"#,
        tool(&raw_old),
        tool(&new)
    );
    round_trip(&proxy, &first, &[]);
    let sent = String::from_utf8(seen.body(0)).unwrap();
    assert!(sent.contains(&raw_old), "the raw-cached block was folded");
    assert!(!sent.contains(&new), "the new block was not folded");
    // Next call: `new` is history now, and it went out folded.
    let second = format!(
        r#"{{"messages":[{},{{"role":"assistant","content":"ok"}},{},{{"role":"assistant","content":"ok"}},{}]}}"#,
        tool(&raw_old),
        tool(&new),
        tool(&"z".repeat(4000))
    );
    round_trip(&proxy, &second, &[]);
    let sent = String::from_utf8(seen.body(1)).unwrap();
    assert!(
        sent.contains(&raw_old) && !sent.contains(&new),
        "history must go out as it was cached"
    );
}
