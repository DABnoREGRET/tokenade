// The tests of `crate::web_fetch`, moved out of the production file.
//
// Mounted by `include!`: the tokens are spliced INTO `web_fetch`, so every
// `super::` keeps its meaning. Split by `srcsweep::production_lines`.

#[cfg(test)]
mod truncation_plumbing {
    /// The curl fallback must not throw away what its drain computed.
    ///
    /// That path is reached only when rustls refuses the handshake, so no
    /// behavioural test on a loopback server can enter it — and a mutation
    /// that restores `let (bytes, _truncated) = …` therefore survives every
    /// one of them. This reads the source instead, which is the only
    /// instrument that covers the branch.
    ///
    /// It scans the production half ONLY: the strings it searches for appear
    /// in this test, and a guard that matches its own assertion passes on
    /// every mutation.
    #[test]
    fn the_curl_drain_does_not_discard_its_truncation_flag() {
        // The FAMILY: `run_curl` moved down into `web_fetch/transport.rs`.
        // And the production half is read through the DECLARER — cutting at
        // the first `#[cfg(test)]` once made 14% of the repository's shipped
        // code invisible to the guards.
        let whole = crate::source_guard::library_module_family("web_fetch");
        let prod: String = crate::srcsweep::production_lines(&whole)
            .into_iter()
            .map(|(_, l)| l)
            .collect::<Vec<_>>()
            .join("\n");
        let prod = prod.as_str();
        assert!(
            prod.len() > 20_000,
            "the production half must be the bulk of the file, was {}",
            prod.len()
        );
        let discarded = format!("let (bytes, {}truncated) =", "_");
        assert!(
            !prod.contains(&discarded),
            "the drain's truncation flag is bound to a discard again"
        );
        assert!(
            prod.contains("f.truncated = truncated;"),
            "run_curl must stamp what spawn_curl reported"
        );
    }
}
#[cfg(test)]
mod tests {

    /// The reason a caller prints is the transport's own text, in the language
    /// of the sentence it lands in.
    ///
    /// Both `tokenade web` paths interpolate this into an English line
    /// ("tokenade web: {url}: {why}. Set TOKENADE_DEBUG_LOG=…"). It used to
    /// render French — "connexion impossible", "délai dépassé", "réponse vide"
    /// — so the one part of the message that says WHAT WENT WRONG read as a
    /// fragment of another program's output. And the inner text must survive:
    /// "could not connect" alone does not separate a closed port from an
    /// unknown host, which is the whole reason `fetch_detailed` exists.
    #[test]
    fn a_fetch_error_says_why_in_the_language_of_its_sentence() {
        let cases = [
            (
                FetchError::Connect("Connection refused (os error 111)".into()),
                "Connection refused",
            ),
            (
                FetchError::Timeout("deadline has elapsed".into()),
                "deadline has elapsed",
            ),
        ];
        for (e, inner) in cases {
            let s = e.to_string();
            assert!(
                s.contains(inner),
                "the transport's own text must survive: {s}"
            );
            assert!(
                s.is_ascii(),
                "an English sentence gets an English reason, got: {s}"
            );
        }
        assert_eq!(FetchError::Empty(0).to_string(), "empty response");
        assert_eq!(
            FetchError::Empty(429).to_string(),
            "empty response (HTTP 429)",
            "an empty refusal must name its status"
        );
    }
    use super::*;

    /// An Akamai wall as the edge actually serves one: the `_abck=` assignment
    /// is the structural evidence `detect_challenge` requires (the bare cookie
    /// NAME appears in prose and must not count).
    const AKAMAI_WALL: &str = "<html><head><title>Access Denied</title></head><body>\
        <script>document.cookie=\"_abck=0~-1~-1\";bazadebezolkohpepadr=1;</script>\
        </body></html>";

    #[test]
    fn a_cookie_gated_wall_asks_for_the_origin() {
        assert_eq!(
            warmup_origin("https://www.example.com/jobs/1234", AKAMAI_WALL.as_bytes()),
            Some("https://www.example.com/".to_string()),
        );
        // The port is part of the origin; the userinfo is not.
        assert_eq!(
            warmup_origin(
                "https://u:p@example.com:8443/a/b?q=1",
                AKAMAI_WALL.as_bytes()
            ),
            Some("https://example.com:8443/".to_string()),
        );
    }

    #[test]
    fn an_ordinary_page_never_warms_up() {
        // The guardrail that keeps this off the hot path: no challenge, no
        // second request, whatever the URL depth.
        let page = "<html><body><h1>Hiring</h1><p>We are hiring engineers in \
            Paris and Berlin. Apply through the form below.</p></body></html>";
        assert_eq!(
            warmup_origin("https://www.example.com/jobs/1234", page.as_bytes()),
            None
        );
    }

    #[test]
    fn a_url_that_is_already_the_origin_is_skipped() {
        // Warming up here would re-fetch the page that just challenged us.
        for u in ["https://example.com", "https://example.com/"] {
            assert_eq!(warmup_origin(u, AKAMAI_WALL.as_bytes()), None, "{u}");
        }
    }

    #[test]
    fn the_warmup_cannot_fire_a_second_time() {
        // `fetch_inner` warms up the ORIGIN and then retries. Even if that
        // retry came back behind the same wall, the origin it would ask for is
        // the origin itself — which `origin_of` reports as already-there. So
        // the sequence has no second step to take, independently of the single
        // non-looping `if` in `fetch_inner`.
        let origin = warmup_origin("https://example.com/deep/page", AKAMAI_WALL.as_bytes())
            .expect("first wall must warm up");
        assert_eq!(warmup_origin(&origin, AKAMAI_WALL.as_bytes()), None);
    }

    #[test]
    fn a_js_gated_wall_is_left_alone() {
        // Cloudflare's clearance needs a solved JS challenge, so a homepage GET
        // buys nothing — closed with a negative verdict, and excluded here so
        // it cannot cost a request per blocked fetch.
        let cf = "<html><head><title>Just a moment...</title></head><body>\
            <div id=\"cf-challenge-running\"></div>\
            <script src=\"/cdn-cgi/challenge-platform/h/b/orchestrate/jsch/v1\"></script>\
            </body></html>";
        assert!(matches!(
            crate::stealth::detect_challenge(cf),
            Some(crate::stealth::ChallengeKind::CloudflareInterstitial)
        ));
        assert_eq!(
            warmup_origin("https://example.com/deep/page", cf.as_bytes()),
            None
        );
    }

    #[test]
    fn the_warmup_respects_the_ssrf_guard() {
        // A new outbound request goes through the same host validation as any
        // other, even though the origin shares the caller's already-cleared host.
        for u in [
            "http://localhost/deep/page",
            "http://127.0.0.1/deep/page",
            "http://169.254.169.254/latest/meta-data",
            "http://box.internal/deep/page",
        ] {
            assert_eq!(warmup_origin(u, AKAMAI_WALL.as_bytes()), None, "{u}");
        }
    }

    #[test]
    fn gtlds_keep_the_default_locale() {
        // The majority of what we fetch. Any drift here is a regression on
        // traffic that works today, not an improvement on the rest.
        for u in [
            "https://example.com/a",
            "https://docs.rs/x",
            "https://a.org/",
            "http://b.net",
            "https://c.io/p?q=1",
            "https://d.dev",
        ] {
            assert_eq!(accept_language_for(u), "en-US,en;q=0.9", "{u}");
        }
    }

    #[test]
    fn cctlds_map_to_their_locale() {
        assert_eq!(
            accept_language_for("https://www.repubblica.it/"),
            "it-IT,it;q=0.9,en;q=0.8"
        );
        assert_eq!(
            accept_language_for("https://www.spiegel.de/x"),
            "de-DE,de;q=0.9,en;q=0.8"
        );
        assert_eq!(
            accept_language_for("https://lemonde.fr"),
            "fr-FR,fr;q=0.9,en;q=0.8"
        );
        assert_eq!(
            accept_language_for("https://asahi.jp/a"),
            "ja-JP,ja;q=0.9,en;q=0.8"
        );
    }

    #[test]
    fn multi_part_suffixes_resolve_through_the_last_label() {
        // The whole point of matching on the last label: no public-suffix list.
        assert_eq!(
            accept_language_for("https://bbc.co.uk/news"),
            "en-GB,en;q=0.9"
        );
        assert_eq!(
            accept_language_for("https://uol.com.br/"),
            "pt-BR,pt;q=0.9,en;q=0.8"
        );
        assert_eq!(
            accept_language_for("https://www.sony.co.jp"),
            "ja-JP,ja;q=0.9,en;q=0.8"
        );
    }

    #[test]
    fn unknown_and_unparseable_fall_back() {
        assert_eq!(accept_language_for("https://host.zzz/x"), "en-US,en;q=0.9");
        assert_eq!(
            accept_language_for("https://localhost:8080/x"),
            "en-US,en;q=0.9"
        );
        // No scheme → `scheme_host` declines → default, never a panic.
        assert_eq!(accept_language_for("not a url"), "en-US,en;q=0.9");
        assert_eq!(accept_language_for(""), "en-US,en;q=0.9");
    }

    #[test]
    fn localized_headers_swap_in_place_and_keep_order() {
        let out = localized_headers(BROWSER_HEADERS, "https://x.de/");
        assert_eq!(out.len(), BROWSER_HEADERS.len());
        assert!(out.contains(&"Accept-Language: de-DE,de;q=0.9,en;q=0.8".to_string()));
        // Order is fingerprinted: the swapped entry stays where it was.
        let pos = |h: &[String]| h.iter().position(|s| s.starts_with("Accept-Language:"));
        let orig: Vec<String> = BROWSER_HEADERS.iter().map(|s| s.to_string()).collect();
        assert_eq!(pos(&out), pos(&orig));
    }

    #[test]
    fn browser_profile_does_not_announce_linux() {
        // Chrome-on-Linux is the bot bucket; the UA, the client hint and the
        // Chrome major must agree or the set is worse than any one of them.
        assert!(!BROWSER_UA.contains("X11") && !BROWSER_UA.contains("Linux"));
        assert!(BROWSER_UA.contains("Windows NT 10.0; Win64; x64"));
        assert!(BROWSER_HEADERS.contains(&"Sec-CH-UA-Platform: \"Windows\""));
        // Read off the ONE place the Chrome major is declared, so a bump is a
        // single edit and this guard cannot go on passing against a literal
        // nobody moved.
        let major = crate::web_impersonate::EMULATED_CHROME_MAJOR;
        assert!(BROWSER_UA.contains(&format!("Chrome/{major}.")));
        assert!(BROWSER_HEADERS
            .iter()
            .any(|h| h.starts_with("Sec-CH-UA:") && h.contains(&format!("\"{major}\""))));
    }

    fn trailer(status: &str, eff: &str, ct: &str) -> Vec<u8> {
        format!("BODY{META_SENTINEL}{status}{META_SEP}{eff}{META_SEP}{ct}").into_bytes()
    }

    #[test]
    fn parses_all_three_fields() {
        let f = split_meta_trailer(trailer("404", "https://x/y", "text/html; charset=utf-8"));
        assert_eq!(f.bytes, b"BODY");
        assert_eq!(f.status, 404);
        assert_eq!(f.effective_url.as_deref(), Some("https://x/y"));
        assert_eq!(f.content_type.as_deref(), Some("text/html; charset=utf-8"));
    }

    #[test]
    fn a_missing_trailer_is_not_a_refusal() {
        let f = split_meta_trailer(b"just a body".to_vec());
        assert_eq!(
            f.status, 200,
            "no evidence of a refusal must not read as one"
        );
        assert_eq!(f.bytes, b"just a body");
        assert!(f.content_type.is_none());
    }

    /// The trailer has gained a field twice. Both times the risk was the same:
    /// a status parser that wants the whole remainder stops parsing anything.
    #[test]
    fn extra_trailer_fields_do_not_break_the_status() {
        let mut raw = trailer("200", "https://x", "text/html");
        raw.extend_from_slice(format!("{META_SEP}future").as_bytes());
        assert_eq!(split_meta_trailer(raw).status, 200);
    }

    #[test]
    fn empty_fields_read_as_absent_not_empty_string() {
        let f = split_meta_trailer(trailer("200", "", ""));
        assert!(f.effective_url.is_none());
        assert!(f.content_type.is_none());
    }

    /// A binary body must not cost us the provenance.
    #[test]
    fn survives_a_body_that_is_not_utf8() {
        let mut raw = vec![0x89, 0x50, 0x4e, 0x47, 0xff, 0xfe];
        raw.extend_from_slice(
            format!("{META_SENTINEL}403{META_SEP}https://x{META_SEP}image/png").as_bytes(),
        );
        let f = split_meta_trailer(raw);
        assert_eq!(f.status, 403);
        assert_eq!(f.content_type.as_deref(), Some("image/png"));
        assert_eq!(f.bytes.len(), 6);
    }

    #[test]
    fn walls_are_retryable_but_a_missing_page_is_not() {
        for c in [401, 403, 406, 429, 451, 503] {
            assert!(refused(c), "{c} is a wall");
        }
        for c in [200, 301, 404, 410, 500] {
            assert!(!refused(c), "{c} is an answer, not a wall");
        }
    }

    /// The three reachability failures a user hits must not print the same
    /// sentence.
    ///
    /// This is the defect in test form. Two earlier versions of the fix
    /// passed a "we classify it now" reading and still emitted one identical
    /// line for all three: first because `fetch` threw the cause away, then
    /// because `reqwest::Error::to_string()` is the same wrapper text for every
    /// transport failure. So the assertion is on the RENDERED string, and that
    /// the three differ — not on the enum variant, which was already right
    /// while the output was still useless.
    #[test]
    fn the_three_transport_failures_read_differently() {
        let rendered = [
            super::FetchError::Connect("Connection refused (os error 111)".into()).to_string(),
            FetchError::Connect("failed to lookup address information".into()).to_string(),
            super::FetchError::Timeout("deadline has elapsed".into()).to_string(),
            FetchError::Empty(0).to_string(),
        ];
        let unique: std::collections::HashSet<&String> = rendered.iter().collect();
        assert_eq!(
            unique.len(),
            rendered.len(),
            "two failures render identically, which is the defect: {rendered:?}"
        );
        // Each must carry the transport's own words, not only our label.
        assert!(
            rendered[0].contains("Connection refused"),
            "{}",
            rendered[0]
        );
        assert!(rendered[2].contains("deadline"), "{}", rendered[2]);
    }
}
#[cfg(test)]
mod a_slow_engine_is_not_a_fast_failure {
    use super::retry_is_worth_it;
    use std::time::Duration;

    /// The case the retry was written for: the engine refused at once.
    #[test]
    fn an_instant_refusal_is_retried() {
        assert!(retry_is_worth_it(Duration::from_millis(120)));
        assert!(retry_is_worth_it(Duration::from_millis(2_900)));
    }

    /// The case that cost 23 012 ms: the first attempt burned its whole
    /// 8-second budget, and a 15-second retry was queued behind it.
    #[test]
    fn a_timed_out_attempt_is_not_retried() {
        assert!(!retry_is_worth_it(Duration::from_secs(8)));
        assert!(!retry_is_worth_it(Duration::from_secs(5)));
    }

    /// The worst case a caller can now be made to wait for one engine, against
    /// the 24.6 s it was. Stated as arithmetic so a change to either timeout
    /// has to face this number.
    #[test]
    fn the_worst_case_is_bounded_by_the_threshold() {
        let first = Duration::from_secs(8);
        assert!(!retry_is_worth_it(first));
        let retried_worst =
            Duration::from_secs(3) + Duration::from_millis(1600) + Duration::from_secs(15);
        assert_eq!(retried_worst.as_millis(), 19_600);
    }
}
