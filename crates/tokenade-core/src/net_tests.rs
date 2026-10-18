// The tests of `crate::net`, moved out of the production file.
//
// Mounted by `include!`: the tokens are spliced INTO `net`, so every
// `super::` keeps its meaning. Split by `srcsweep::production_lines`.

#[cfg(test)]
mod tests {

    use super::*;
    use std::sync::MutexGuard;

    /// The proxy env vars are process-global; serialise every test that reads
    /// or writes them so a sibling can't observe a half-set environment.
    /// The crate-wide lock: a private one serialises nothing against the
    /// tests of other modules that change the environment.
    fn env_lock() -> MutexGuard<'static, ()> {
        crate::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    const VARS: [&str; 8] = [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
        "NO_PROXY",
        "no_proxy",
    ];

    /// Snapshot + clear all proxy vars for the duration of a test, restoring
    /// the ambient values (a CI runner may legitimately set `HTTPS_PROXY`) on
    /// drop so tests are hermetic regardless of the outer environment.
    struct EnvSnapshot(Vec<(&'static str, Option<String>)>);
    impl EnvSnapshot {
        fn clear() -> Self {
            let saved = VARS.iter().map(|k| (*k, std::env::var(k).ok())).collect();
            for k in VARS {
                std::env::remove_var(k);
            }
            EnvSnapshot(saved)
        }
    }
    impl Drop for EnvSnapshot {
        fn drop(&mut self) {
            for (k, v) in &self.0 {
                match v {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
            }
        }
    }

    #[test]
    fn https_proxy_selected_for_https_target() {
        let _g = env_lock();
        let _e = EnvSnapshot::clear();
        std::env::set_var("HTTPS_PROXY", "http://proxy:3128");
        std::env::set_var("HTTP_PROXY", "http://nope:3128");
        assert_eq!(
            resolve_proxy("https://downloads.tokenade.net/x").as_deref(),
            Some("http://proxy:3128")
        );
    }

    #[test]
    fn http_proxy_selected_for_http_target() {
        let _g = env_lock();
        let _e = EnvSnapshot::clear();
        std::env::set_var("HTTP_PROXY", "http://hp:3128");
        std::env::set_var("HTTPS_PROXY", "http://sp:3128");
        assert_eq!(
            resolve_proxy("http://example.com/y").as_deref(),
            Some("http://hp:3128")
        );
    }

    #[test]
    fn all_proxy_is_catch_all() {
        let _g = env_lock();
        let _e = EnvSnapshot::clear();
        std::env::set_var("ALL_PROXY", "http://catchall:1080");
        assert_eq!(
            resolve_proxy("https://x.tokenade.net").as_deref(),
            Some("http://catchall:1080")
        );
    }

    #[test]
    fn no_proxy_exact_dotsuffix_and_star() {
        let _g = env_lock();
        let _e = EnvSnapshot::clear();
        std::env::set_var("HTTPS_PROXY", "http://p:3128");

        // Exact host match bypasses.
        std::env::set_var("NO_PROXY", "downloads.tokenade.net");
        assert!(resolve_proxy("https://downloads.tokenade.net/m").is_none());

        // Leading-dot entry → suffix match on subdomains; unrelated host still proxied.
        std::env::set_var("NO_PROXY", ".tokenade.net");
        assert!(resolve_proxy("https://downloads.tokenade.net/m").is_none());
        assert!(resolve_proxy("https://huggingface.co/m").is_some());

        // `*` bypasses everything.
        std::env::set_var("NO_PROXY", "*");
        assert!(resolve_proxy("https://anything.example/m").is_none());
    }

    #[test]
    fn no_proxy_empty_entries_ignored() {
        let _g = env_lock();
        let _e = EnvSnapshot::clear();
        std::env::set_var("HTTPS_PROXY", "http://p:3128");
        std::env::set_var("NO_PROXY", " , ,");
        assert_eq!(
            resolve_proxy("https://h.co").as_deref(),
            Some("http://p:3128")
        );
    }

    #[test]
    fn no_proxy_takes_precedence_over_all_proxy() {
        let _g = env_lock();
        let _e = EnvSnapshot::clear();
        std::env::set_var("ALL_PROXY", "http://p:3128");
        std::env::set_var("NO_PROXY", "*");
        assert!(resolve_proxy("https://h.co").is_none());
    }

    #[test]
    fn redact_hides_credentials() {
        assert_eq!(
            redact_proxy("http://user:secret@proxy:3128"),
            "http://***@proxy:3128"
        );
        assert_eq!(redact_proxy("http://proxy:3128"), "http://proxy:3128");
        // Malformed input is returned unchanged rather than panicking.
        assert_eq!(redact_proxy("not-a-url"), "not-a-url");
    }
}
#[cfg(test)]
mod credential_redirects {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// A server that answers 302 to `elsewhere`, and records whether the
    /// SECOND request (the followed one) carried the key.
    fn redirector(seen_key: Arc<AtomicBool>) -> (u16, u16) {
        let first = TcpListener::bind("127.0.0.1:0").expect("bind");
        let second = TcpListener::bind("127.0.0.1:0").expect("bind");
        let p1 = first.local_addr().unwrap().port();
        let p2 = second.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut sock, _)) = first.accept() {
                let mut scratch = [0u8; 4096];
                let _ = sock.read(&mut scratch);
                let _ = sock.write_all(
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{p2}/next\r\n\
                         Content-Length: 0\r\n\r\n"
                    )
                    .as_bytes(),
                );
                let _ = sock.flush();
            }
        });
        std::thread::spawn(move || {
            if let Ok((mut sock, _)) = second.accept() {
                let mut scratch = [0u8; 4096];
                let n = sock.read(&mut scratch).unwrap_or(0);
                let req = String::from_utf8_lossy(&scratch[..n]).to_ascii_lowercase();
                if req.contains("sk-live-do-not-leak") {
                    seen_key.store(true, Ordering::SeqCst);
                }
                let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
                let _ = sock.flush();
            }
        });
        (p1, p2)
    }

    /// The header Anthropic, Azure OpenAI and Google use is not one of the
    /// four reqwest strips on a cross-host redirect. Following the 3xx would
    /// hand the user's key to whatever the Location names.
    #[test]
    fn an_api_key_does_not_follow_a_redirect() {
        let seen = Arc::new(AtomicBool::new(false));
        let (p1, _p2) = redirector(seen.clone());
        let resp = http_get(
            &format!("http://127.0.0.1:{p1}/v1"),
            &[("x-api-key", "sk-live-do-not-leak")],
            5,
        )
        .expect("the 3xx itself must come back, not a transport error");
        assert_eq!(
            resp.status, 302,
            "the redirect must be RETURNED, not followed"
        );
        std::thread::sleep(Duration::from_millis(150));
        assert!(
            !seen.load(Ordering::SeqCst),
            "the key reached the redirect target"
        );
    }

    /// …and the credential-free download path still follows, or the updater
    /// and the model fetcher stop working the day a CDN moves.
    #[test]
    fn a_credential_free_request_still_follows() {
        let seen = Arc::new(AtomicBool::new(false));
        let (p1, _p2) = redirector(seen.clone());
        let resp = http_get(&format!("http://127.0.0.1:{p1}/dl"), &[], 5)
            .expect("a plain download must complete");
        assert_eq!(resp.status, 200, "the redirect must be followed");
        assert_eq!(resp.body, b"ok");
    }

    #[test]
    fn every_vendor_spelling_of_a_key_is_recognised() {
        for name in [
            "Authorization",
            "x-api-key",
            "api-key",
            "X-Goog-Api-Key",
            "x-access-token",
            "cookie",
            "client_secret",
        ] {
            assert!(carries_credential(&[(name, "v")]), "{name} not recognised");
        }
        for name in [
            "anthropic-version",
            "content-type",
            "if-none-match",
            "accept",
        ] {
            assert!(
                !carries_credential(&[(name, "v")]),
                "{name} must not disable redirects"
            );
        }
    }
}
#[cfg(test)]
mod streaming_timeout {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// A body that arrives SLOWLY but keeps arriving must complete.
    ///
    /// This is the field failure from 2026-06-10 in miniature: the old client
    /// put the deadline on the whole request, so a download slower than
    /// size/timeout failed however healthy the connection was. Here the total
    /// transfer takes longer than the per-read deadline while no single gap
    /// does — the exact shape that used to fail and must not.
    #[test]
    fn a_slow_but_moving_body_is_not_a_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let chunks = 30usize;
        let gap_ms = 60u64;
        // The fixture's own shape, asserted rather than described: a comment
        // saying "their sum is over the deadline" goes stale the first time
        // someone tunes one of the two numbers.
        assert!(
            gap_ms < 1_000 && (chunks as u64) * gap_ms > 1_000,
            "each gap under the 1 s per-read deadline, their sum over it"
        );
        let chunk = "0123456789".repeat(64); // 640 B each
        let total = chunk.len() * chunks;

        let body = chunk.clone();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().expect("accept");
            let mut scratch = [0u8; 1024];
            let _ = sock.read(&mut scratch);
            let _ = sock.write_all(
                format!("HTTP/1.1 200 OK\r\nContent-Length: {total}\r\n\r\n").as_bytes(),
            );
            let _ = sock.flush();
            for _ in 0..chunks {
                // Each gap is well under the per-read deadline; their SUM is
                // well over what a total deadline of the same size would allow.
                std::thread::sleep(Duration::from_millis(gap_ms));
                if sock.write_all(body.as_bytes()).is_err() {
                    return;
                }
                let _ = sock.flush();
            }
        });

        // Per-read deadline of 1 s against ~1.8 s of ACCUMULATED stalls, no
        // single one of which exceeds 60 ms. Two margins, pulling opposite
        // ways: each gap must sit UNDER the deadline (or the fixed code fails)
        // and their sum must sit OVER it (or the two worlds agree and the
        // guard proves nothing — the first version used 6 × 120 ms, whose
        // 720 ms total fitted inside a 1 s TOTAL deadline too, so it passed
        // against the very bug it was written to catch).
        //
        // Only the FIRST of the two races. The sum is a property of the
        // fixture and a loaded machine can only push it further over; the gap
        // is a real sleep measured against a real deadline, and at 300 ms it
        // needed just a 3.3× scheduler overshoot to fail. That is what it did:
        // green alone, red under the full 7 000-test parallel load, which
        // reads as a bug in whatever was being changed at the time. Shrinking
        // the gap to 60 ms and buying the sum back with chunks widens the
        // racing margin to ~17× and makes the test FASTER (1.8 s, was 3 s).
        let resp = http_get_capped(&format!("http://127.0.0.1:{port}/x"), &[], 1, 1 << 20)
            .expect("a slow but moving body must complete");
        assert_eq!(resp.status, 200);
        assert_eq!(
            resp.body.len(),
            total,
            "the body was cut short: {} of {total}",
            resp.body.len()
        );
    }

    /// …and a body that STOPS arriving must still die.
    #[test]
    fn a_stalled_body_still_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().expect("accept");
            let mut scratch = [0u8; 1024];
            let _ = sock.read(&mut scratch);
            let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4096\r\n\r\nabc");
            let _ = sock.flush();
            // …and then nothing, ever.
            std::thread::sleep(Duration::from_secs(30));
        });

        let started = std::time::Instant::now();
        let got = http_get_capped(&format!("http://127.0.0.1:{port}/y"), &[], 1, 1 << 20);
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "a stalled connection was not cut: {:?}",
            started.elapsed()
        );
        // Whether it surfaces as None or a short body, it must not be the full
        // promised length — the point is that it RETURNED.
        assert!(got.as_ref().map(|r| r.body.len()).unwrap_or(0) < 4096);
    }
}
#[cfg(test)]
mod a_buffered_body_is_bounded {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Serve `plain_mb` megabytes of compressible filler as `Content-Encoding:
    /// gzip`. Returns the port and the number of bytes actually put on the wire.
    fn gzip_server(plain_mb: usize) -> (u16, usize) {
        let l = TcpListener::bind("127.0.0.1:0").expect("bind");
        let p = l.local_addr().unwrap().port();
        let mut e = GzEncoder::new(Vec::new(), Compression::best());
        let chunk = vec![b'A'; 1024 * 1024];
        for _ in 0..plain_mb {
            e.write_all(&chunk).expect("deflate");
        }
        let gz = e.finish().expect("finish");
        let wire = gz.len();
        std::thread::spawn(move || {
            if let Ok((mut sock, _)) = l.accept() {
                let mut scratch = [0u8; 4096];
                let _ = sock.read(&mut scratch);
                let hdr = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\
                     Content-Encoding: gzip\r\nContent-Length: {}\r\n\r\n",
                    gz.len()
                );
                let _ = sock.write_all(hdr.as_bytes());
                let _ = sock.write_all(&gz);
                let _ = sock.flush();
            }
        });
        (p, wire)
    }

    #[test]
    fn a_compressed_body_cannot_outgrow_the_cap() {
        // 64 MB of filler is ~65 KB on the wire: the caller cannot see what it
        // is about to allocate, which is the whole point of the bound.
        let (p, wire) = gzip_server(64);
        let r = http_get(&format!("http://127.0.0.1:{p}/x"), &[], 20).expect("response");
        assert!(
            wire < BUFFERED_BODY_CAP / 8,
            "the fixture must be small on the wire to be worth anything, was {wire}"
        );
        assert!(
            r.body.len() <= BUFFERED_BODY_CAP,
            "{} bytes kept against a cap of {}",
            r.body.len(),
            BUFFERED_BODY_CAP
        );
        assert!(r.truncated, "a body cut at the cap must say so");
    }

    #[test]
    fn a_body_that_fits_is_whole_and_says_nothing() {
        let (p, _) = gzip_server(1);
        let r = http_get(&format!("http://127.0.0.1:{p}/x"), &[], 20).expect("response");
        assert_eq!(
            r.body.len(),
            1024 * 1024,
            "a body under the cap must arrive entire"
        );
        assert!(!r.truncated, "nothing was cut, so nothing may be claimed");
        assert!(
            r.body.iter().all(|&b| b == b'A'),
            "the bytes must be the served ones"
        );
    }

    /// Serve `n` bytes of identical filler, uncompressed, small enough to
    /// arrive as ONE chunk.
    fn plain_server(n: usize) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").expect("bind");
        let p = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut sock, _)) = l.accept() {
                let mut scratch = [0u8; 4096];
                let _ = sock.read(&mut scratch);
                let _ = sock.write_all(
                    format!("HTTP/1.1 200 OK\r\nContent-Length: {n}\r\n\r\n").as_bytes(),
                );
                let _ = sock.write_all(&vec![b'B'; n]);
                let _ = sock.flush();
            }
        });
        p
    }

    #[test]
    fn a_cut_inside_the_final_chunk_is_still_a_cut() {
        // The body ends in the same chunk the cap lands in, so no later read
        // happens to notice the loss. Without the check on the partial take
        // this came back short and called itself whole.
        let p = plain_server(8 * 1024);
        let r = http_get_capped(&format!("http://127.0.0.1:{p}/x"), &[], 20, 4 * 1024)
            .expect("response");
        assert_eq!(
            r.body.len(),
            4 * 1024,
            "the cap must hold: {}",
            r.body.len()
        );
        assert!(
            r.truncated,
            "the body lost half its bytes and reported itself whole"
        );
    }

    #[test]
    fn the_caller_supplied_cap_reports_its_own_truncation() {
        let (p, _) = gzip_server(4);
        let small = 64 * 1024;
        let r =
            http_get_capped(&format!("http://127.0.0.1:{p}/x"), &[], 20, small).expect("response");
        assert!(r.body.len() <= small, "kept {} over {small}", r.body.len());
        assert!(r.truncated, "the capped reader must report its cut too");
    }
}
#[cfg(test)]
mod a_stalled_download_dies_before_its_budget {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// `TOKENADE_STREAM_STALL_SECS` is process-global, so these two tests
    /// cannot run at once: the second one's `remove_var` cleared the override
    /// the first was relying on, and the download then ran to the server's own
    /// timeout — 120 s, and a failure that says nothing about the code.
    use crate::platform::ENV_LOCK;

    /// Answer with a large `Content-Length`, send a few bytes, then send
    /// nothing ever again — the shape a total deadline alone cannot see.
    fn stalling_server() -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").expect("bind");
        let p = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut sock, _)) = l.accept() {
                let mut scratch = [0u8; 4096];
                let _ = sock.read(&mut scratch);
                let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000000\r\n\r\n");
                let _ = sock.write_all(b"first bytes");
                let _ = sock.flush();
                // Hold the socket open, silent, past any sane deadline.
                std::thread::sleep(Duration::from_secs(120));
            }
        });
        p
    }

    #[test]
    fn a_silent_server_does_not_hold_the_whole_budget() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("TOKENADE_STREAM_STALL_SECS", "1");
        let p = stalling_server();
        let mut sink: Vec<u8> = Vec::new();
        let started = std::time::Instant::now();
        // A GENEROUS total budget: only a per-read deadline can end this early.
        let _ = http_get_stream(&format!("http://127.0.0.1:{p}/big"), &[], 300, &mut sink);
        let waited = started.elapsed();
        std::env::remove_var("TOKENADE_STREAM_STALL_SECS");
        assert!(
            waited < Duration::from_secs(30),
            "a dead link held the download for {waited:?} of a 300 s budget"
        );
    }

    /// The other half, and the reason the deadline is PER-READ: a link that is
    /// slow but moving must finish. A test that only proves "a stall dies
    /// fast" is equally happy if the two deadlines are swapped, which would
    /// kill every slow download at the stall value.
    #[test]
    fn a_slow_but_moving_link_still_finishes() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("TOKENADE_STREAM_STALL_SECS", "2");
        const CHUNKS: usize = 8;
        const CHUNK: &[u8] = b"0123456789";
        let l = TcpListener::bind("127.0.0.1:0").expect("bind");
        let p = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut sock, _)) = l.accept() {
                let mut scratch = [0u8; 4096];
                let _ = sock.read(&mut scratch);
                let _ = sock.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                        CHUNK.len() * CHUNKS
                    )
                    .as_bytes(),
                );
                for _ in 0..CHUNKS {
                    // Well inside the stall deadline, and the whole transfer
                    // takes longer than it.
                    std::thread::sleep(Duration::from_millis(400));
                    if sock.write_all(CHUNK).is_err() {
                        return;
                    }
                    let _ = sock.flush();
                }
            }
        });
        let mut sink: Vec<u8> = Vec::new();
        let status = http_get_stream(&format!("http://127.0.0.1:{p}/slow"), &[], 300, &mut sink);
        std::env::remove_var("TOKENADE_STREAM_STALL_SECS");
        assert_eq!(status, Some(200), "a moving link must complete");
        assert_eq!(
            sink.len(),
            CHUNK.len() * CHUNKS,
            "every byte must arrive, not just the ones before the deadline"
        );
    }

    #[test]
    fn the_override_is_read_and_bounded() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("TOKENADE_STREAM_STALL_SECS");
        assert_eq!(stream_stall_deadline(), STREAM_STALL_DEADLINE, "default");
        std::env::set_var("TOKENADE_STREAM_STALL_SECS", "7");
        assert_eq!(stream_stall_deadline(), Duration::from_secs(7));
        // Zero would mean "no deadline at all", which is the defect, not a
        // setting; nonsense falls back rather than disabling the guard.
        for bad in ["0", "abc", ""] {
            std::env::set_var("TOKENADE_STREAM_STALL_SECS", bad);
            assert_eq!(
                stream_stall_deadline(),
                STREAM_STALL_DEADLINE,
                "{bad:?} must not disable the deadline"
            );
        }
        std::env::remove_var("TOKENADE_STREAM_STALL_SECS");
    }
}
