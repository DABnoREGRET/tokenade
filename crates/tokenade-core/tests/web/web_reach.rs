//! Battery 3 — REACH. Which of the real web can we actually read?
//!
//! This is the battery that prices a TRANSPORT swap. Everything else measures
//! what we do with bytes we already have; this measures whether we get bytes at
//! all. A new HTTP client that does not move the scoreboard here is not worth
//! its build cost, however good it looks on paper.
//!
//!     TOKENADE_LIVE_WEB_TESTS=1 cargo test -p tokenade-core --test web_reach -- --nocapture
//!
//! Opt-in and never in CI: it hits ~20 third-party sites. The URL list is
//! versioned WITH its expected class, so a shift shows up as a diff in review
//! rather than as a number nobody can interpret.
//!
//! Baseline recorded 2026-07-21 with the curl transport, and the wreq
//! (browser-TLS) column measured the same hour.

use tokenade_core::web_fetch::{fetch, FetchOpts};

/// What we expect a target to do TODAY. Not a wish — a record of the last
/// measurement, so a change is legible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// Serves us usable content.
    Readable,
    /// Refuses on an IP/rate signal (403/429). A better TLS fingerprint does
    /// NOT fix these — measured.
    WalledByPolicy,
    /// Refuses with a JS interstitial. TLS fingerprint DOES fix some of these.
    ///
    /// UNMEASURED: no entry in `TARGETS` carries this class today, so the one
    /// failure mode that justifies the TLS/stealth work is the one this corpus
    /// does not exercise. Deliberately left empty rather than filled in — the
    /// rule below (never invent a URL) outranks the wish for a full table, and
    /// a challenge target has to be found by hand and hit at least once before
    /// it can be asserted on.
    #[allow(dead_code)]
    WalledByChallenge,
    /// Answers 200 with a shell; the content needs JS.
    JsShell,
}

struct Target {
    url: &'static str,
    class: Expect,
    /// Per-target usable floor. A global one is wrong: example.com is
    /// legitimately ~170 chars and a 400-char floor called it a regression.
    min_chars: usize,
    /// A string that MUST appear in the distilled markdown for the fetch to
    /// count as usable. Guards against "200 with a wall in the body".
    needle: &'static str,
}

/// Chosen to span the failure modes — all but one: `WalledByChallenge` has no
/// entry, see its own note. Not a top-sites list. Every entry
/// was hit at least once by hand; none is invented — inventing a URL and then
/// concluding from its failure is a mistake this project has made six times.
const TARGETS: &[Target] = &[
    // Plainly readable — regressions here mean the transport broke outright.
    Target {
        url: "https://example.com/",
        class: Expect::Readable,
        min_chars: 100,
        needle: "Example Domain",
    },
    Target {
        url: "https://www.rust-lang.org/",
        class: Expect::Readable,
        min_chars: DEFAULT_MIN,
        needle: "Rust",
    },
    Target {
        url: "https://en.wikipedia.org/wiki/Rust_(programming_language)",
        class: Expect::Readable,
        min_chars: DEFAULT_MIN,
        needle: "Mozilla",
    },
    Target {
        url: "https://doc.rust-lang.org/std/vec/struct.Vec.html",
        class: Expect::Readable,
        min_chars: DEFAULT_MIN,
        needle: "Vec",
    },
    Target {
        url: "https://news.ycombinator.com/",
        class: Expect::Readable,
        min_chars: DEFAULT_MIN,
        needle: "comments",
    },
    Target {
        url: "https://arxiv.org/abs/1706.03762",
        class: Expect::Readable,
        min_chars: DEFAULT_MIN,
        needle: "Attention",
    },
    // Reachable only because the browser-profile retry exists:
    // these answered 403 + ~66 words to the honest UA.
    Target {
        url: "https://www.britannica.com/animal/lion",
        class: Expect::Readable,
        min_chars: DEFAULT_MIN,
        needle: "lion",
    },
    Target {
        url: "https://www.cdc.gov/",
        class: Expect::Readable,
        min_chars: DEFAULT_MIN,
        needle: "Health",
    },
    // Feed / JSON / structured — different pipeline branches.
    Target {
        url: "https://simonwillison.net/atom/everything/",
        class: Expect::Readable,
        min_chars: DEFAULT_MIN,
        needle: "Simon",
    },
    Target {
        url: "https://api.github.com/repos/rust-lang/rust",
        class: Expect::Readable,
        min_chars: DEFAULT_MIN,
        needle: "rust",
    },
    // The byte-gap question: wreq returns ~10x the bytes on these. The needle
    // decides whether that is CONTENT or a fatter shell.
    Target {
        url: "https://x.com/rustlang",
        class: Expect::Readable,
        min_chars: DEFAULT_MIN,
        needle: "",
    },
    // 403 to a bare curl, 200 through OUR transport (polite-first, then the
    // browser profile WITH the cookie jar). This is the entry that killed the
    // wreq case: its one supposed exclusive win is not exclusive.
    Target {
        url: "https://www.zillow.com/homes/",
        class: Expect::Readable,
        min_chars: DEFAULT_MIN,
        needle: "",
    },
    // Walled by policy — IP/rate signals. wreq does NOT move these.
    Target {
        url: "https://www.etsy.com/search?q=mug",
        class: Expect::WalledByPolicy,
        min_chars: DEFAULT_MIN,
        needle: "",
    },
    Target {
        url: "https://www.g2.com/categories/crm",
        class: Expect::WalledByPolicy,
        min_chars: DEFAULT_MIN,
        needle: "",
    },
    Target {
        url: "https://www.yelp.com/search?find_desc=coffee",
        class: Expect::WalledByPolicy,
        min_chars: DEFAULT_MIN,
        needle: "",
    },
    Target {
        url: "https://www.glassdoor.com/Reviews/index.htm",
        class: Expect::WalledByPolicy,
        min_chars: DEFAULT_MIN,
        needle: "",
    },
    // Walled by challenge — the class a TLS fingerprint DOES open.
    // MEASURED reachable with our full BROWSER_HEADERS set (200 + 11 results).
    // It is 202 + a challenge WITHOUT them — this entry guards that header set.
    Target {
        url: "https://html.duckduckgo.com/html/?q=rust",
        class: Expect::Readable,
        min_chars: 1000,
        needle: "rust",
    },
    Target {
        url: "https://www.quora.com/",
        class: Expect::Readable,
        min_chars: 300,
        needle: "",
    },
    // JS shell — 200, but the content is not in the markup.
    // 200, but the RESULTS are not in the markup. The needle is what tells a
    // shell from a results page — bytes alone cannot.
    Target {
        url: "https://www.google.com/search?q=rust",
        class: Expect::JsShell,
        min_chars: DEFAULT_MIN,
        needle: "rust-lang.org",
    },
];

/// Default usable floor. Below this we are holding a wall, a shell or an error
/// page — not content. Overridable per target.
const DEFAULT_MIN: usize = 400;

struct Outcome {
    status: u16,
    bytes: usize,
    md_chars: usize,
    has_needle: bool,
    usable: bool,
}

fn probe(t: &Target) -> Outcome {
    const CAP: usize = 8 * 1024 * 1024;
    match fetch(t.url, &FetchOpts::page("20", CAP)) {
        None => Outcome {
            status: 0,
            bytes: 0,
            md_chars: 0,
            has_needle: false,
            usable: false,
        },
        Some(f) => {
            let body = tokenade_core::text_encoding::decode(&f.bytes)
                .map(|(s, _)| s)
                .unwrap_or_else(|| String::from_utf8_lossy(&f.bytes).into_owned());
            let d =
                tokenade_core::web_pipeline::distill_page(&body, t.url, f.content_type.as_deref());
            let md_chars = d.md.chars().count();
            let has_needle =
                t.needle.is_empty() || d.md.to_lowercase().contains(&t.needle.to_lowercase());
            Outcome {
                status: f.status,
                bytes: f.bytes.len(),
                md_chars,
                has_needle,
                // 2xx, enough content, and the needle if one was named.
                usable: (200..300).contains(&f.status) && md_chars >= t.min_chars && has_needle,
            }
        }
    }
}

fn enabled() -> bool {
    matches!(
        std::env::var("TOKENADE_LIVE_WEB_TESTS").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

/// The scoreboard. Prints a table and asserts only the floor — the NUMBER is
/// the deliverable, and it is what a transport swap is judged on.
#[test]
fn reach_scoreboard() {
    if !enabled() {
        eprintln!("skipped — set TOKENADE_LIVE_WEB_TESTS=1 to run");
        return;
    }
    println!(
        "\n{:<62} {:>4} {:>9} {:>8}  verdict",
        "url", "http", "bytes", "md"
    );
    let mut readable_ok = 0usize;
    let mut readable_total = 0usize;
    let mut surprises: Vec<String> = Vec::new();

    for t in TARGETS {
        let o = probe(t);
        let verdict = if o.usable { "USABLE" } else { "no" };
        println!(
            "{:<62} {:>4} {:>9} {:>8}  {verdict}",
            t.url.chars().take(62).collect::<String>(),
            o.status,
            o.bytes,
            o.md_chars
        );
        match t.class {
            Expect::Readable => {
                readable_total += 1;
                if o.usable {
                    readable_ok += 1;
                } else {
                    surprises.push(format!(
                        "REGRESSION {} — expected readable, got http={} md={} needle={}",
                        t.url, o.status, o.md_chars, o.has_needle
                    ));
                }
            }
            // A wall that opens is GOOD NEWS and must be reported loudly: it is
            // exactly what a transport upgrade is bought for, and it should be
            // promoted to Readable in this list rather than left as a surprise.
            Expect::WalledByPolicy | Expect::WalledByChallenge | Expect::JsShell => {
                if o.usable {
                    surprises.push(format!(
                        "IMPROVEMENT {} — expected {:?}, now USABLE ({} md chars). \
                         Promote it to Expect::Readable.",
                        t.url, t.class, o.md_chars
                    ));
                }
            }
        }
    }

    let walled = TARGETS
        .iter()
        .filter(|t| t.class != Expect::Readable)
        .count();
    println!(
        "\nreadable {readable_ok}/{readable_total} · known-walled {walled} · \
         usable-min {DEFAULT_MIN} chars (per-target overrides apply)"
    );
    for s in &surprises {
        println!("  ! {s}");
    }

    // Only the floor is a hard failure. Third-party sites have bad days; the
    // scoreboard is for judgement, this assert is for breakage.
    assert!(
        readable_ok * 2 >= readable_total,
        "only {readable_ok}/{readable_total} known-readable targets came back usable — \
         that is a transport regression, not a bad day"
    );
}

/// The two profiles must stay distinct. `--fail` is right for a SERP (a 429 is
/// an engine failure) and WRONG for a page (the error page IS the content we
/// must disclose). Collapsing them is a plausible "simplification" that would
/// silently break disclosure.
#[test]
fn a_page_fetch_still_returns_the_error_page_body() {
    if !enabled() {
        eprintln!("skipped — set TOKENADE_LIVE_WEB_TESTS=1 to run");
        return;
    }
    // The PROPERTY is "a non-2xx reaches the caller instead of being swallowed",
    // not "this endpoint returns 404". Asserting the exact code made the test
    // fail when httpbin itself answered 503 — which actually demonstrates the
    // property. Test the contract, not the third party's mood.
    let f = fetch(
        "https://httpbin.org/status/404",
        &FetchOpts::page("15", 1 << 20),
    );
    match f {
        Some(f) => assert!(
            !(200..300).contains(&f.status),
            "a non-2xx must survive to the caller, got {}",
            f.status
        ),
        // httpbin is flaky; do not fail the suite on its availability.
        None => eprintln!("httpbin unreachable — skipped"),
    }
}

/// Post-migration (2026-07-21): reqwest+rustls is the PRIMARY transport —
/// in-process, HTTP/2, with a cookie store — so a missing curl is no longer a
/// degradation. curl is only a handshake fallback for the rare server rustls
/// refuses. There is therefore no ordinary degraded mode to warn about, and
/// `transport_warning` must stay `None` whether or not curl is on PATH.
#[test]
fn a_missing_curl_is_no_longer_a_degradation() {
    // curl_available() still answers (the fallback + healthcheck use it), but
    // its absence must not surface a degradation warning: the primary client is
    // fully capable on its own.
    let _ = tokenade_core::web_fetch::curl_available();
    assert!(
        tokenade_core::web_fetch::transport_warning().is_none(),
        "reqwest is primary; a missing curl is not a degraded transport"
    );
}

/// The in-process fallback must really fetch — a fallback that does not work is
/// a longer path to the same silent failure.
#[test]
fn the_in_process_transport_can_fetch() {
    if !enabled() {
        eprintln!("skipped — set TOKENADE_LIVE_WEB_TESTS=1 to run");
        return;
    }
    let f = tokenade_core::web_fetch::fetch_pooled(
        "https://example.com/",
        &FetchOpts::api("15", 1 << 20),
    )
    .expect("the pooled in-process client must fetch");
    assert_eq!(f.status, 200);
    assert!(String::from_utf8_lossy(&f.bytes).contains("Example Domain"));
}

/// Connection reuse is the point of the pooled path: ten dependent same-host
/// requests (notion_drain's worst case) must not each pay a fresh handshake.
#[test]
fn the_pooled_path_reuses_its_connection() {
    if !enabled() {
        eprintln!("skipped — set TOKENADE_LIVE_WEB_TESTS=1 to run");
        return;
    }
    const N: u32 = 5;
    let url = "https://example.com/";
    let opts = FetchOpts::api("15", 1 << 20);

    let t = std::time::Instant::now();
    for _ in 0..N {
        let _ = tokenade_core::web_fetch::fetch_pooled(url, &opts);
    }
    let pooled_ms = t.elapsed().as_millis();

    let t = std::time::Instant::now();
    for _ in 0..N {
        let _ = fetch(url, &FetchOpts::page("15", 1 << 20));
    }
    let spawned_ms = t.elapsed().as_millis();

    println!("  {N} same-host fetches: pooled {pooled_ms}ms · process-per-fetch {spawned_ms}ms");
    // Not asserted as a hard ratio — a shared runner's network noise dwarfs the
    // handshake on a good day. The NUMBER is the deliverable; this only guards
    // against the pooled path being accidentally slower.
    assert!(
        pooled_ms <= spawned_ms * 2,
        "pooled ({pooled_ms}ms) should not be dramatically slower than \
         process-per-fetch ({spawned_ms}ms)"
    );
}

/// **A refusal with an EMPTY body keeps its status.**
///
/// This module writes, three lines before the spot of the defect, "a non-2xx is NOT
/// an error here: the error page is content the caller must be told about,
/// which is the whole reason `status` rides along" — and an empty body threw
/// that status away. MEASURED 2026-09-11: reddit answers **429 with zero bytes**, and the
/// reader went away with "could not be fetched at all" where the useful answer
/// was "retry in a minute".
///
/// A local server with a single response, because no site guarantees this
/// status/body pair on demand.
#[test]
fn an_empty_body_keeps_the_status_that_refused_it() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        if let Ok((mut sock, _)) = listener.accept() {
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf);
            let _ = sock.write_all(
                b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    let err = match tokenade_core::web_fetch::fetch_detailed(
        &format!("http://127.0.0.1:{port}/r/rust/.rss"),
        &tokenade_core::web_fetch::FetchOpts::page("10", 1 << 20),
    ) {
        Err(e) => e,
        Ok(f) => panic!("an empty body is not a success (status {})", f.status),
    };
    let _ = server.join();
    assert_eq!(
        err.status(),
        Some(429),
        "the status of the refusal was thrown away: {err}"
    );
    assert!(err.to_string().contains("429"), "{err}");
}
