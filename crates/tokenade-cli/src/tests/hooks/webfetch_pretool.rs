//! Tests for the `webfetch_pretool` hook.

use super::*;

#[test]
fn a_small_anchored_page_is_intercepted_now_that_there_is_no_size_gate() {
    // PO decision 2026-08-01: the 12k size gate is gone (default 0). A small
    // anchored page used to passthrough on `compact_tokens < min_tokens`; it is
    // now intercepted, because the dollar bench measured that the native path
    // pays Haiku to read the whole page whatever its size. The ONLY passthrough
    // leg left is "no lexical anchor".
    let _g = INSTALL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("TOKENADE_WEBFETCH_INTERCEPT_MIN_TOKENS");
    assert_eq!(super::WEBFETCH_INTERCEPT_MIN_TOKENS_DEFAULT, 0);
    let page = gate_page(300); // would have been far under the old 12k gate
    let toks = tokenade_core::tokenizer::estimate(&page);
    match super::selective_gate_decision(
        "zeta_widget calibration default",
        &page, None,
        toks,
        false,
        false,
    ) {
        super::SelectiveDecision::Intercept { extract } => {
            assert!(
                extract.contains("42ms"),
                "answer must be in excerpt: {extract}"
            );
        }
        d => panic!("small anchored page must intercept, got {d:?}"),
    }
}

#[test]
fn restoring_the_env_threshold_brings_the_size_gate_back() {
    // The size gate is off by default but the code path stays reachable: this
    // is the documented rollback if field cost regresses
    // (`TOKENADE_WEBFETCH_INTERCEPT_MIN_TOKENS=4000`). Asserted so removing the
    // default cannot quietly rot the knob.
    let _g = INSTALL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let page = gate_page(300);
    let toks = tokenade_core::tokenizer::estimate(&page);
    std::env::set_var(
        "TOKENADE_WEBFETCH_INTERCEPT_MIN_TOKENS",
        (toks + 1).to_string(),
    );
    let d = super::selective_gate_decision(
        "zeta_widget calibration default",
        &page, None,
        toks,
        false,
        false,
    );
    std::env::remove_var("TOKENADE_WEBFETCH_INTERCEPT_MIN_TOKENS");
    match d {
        super::SelectiveDecision::Passthrough(why) => {
            assert_eq!(why, "webfetch_passthrough_native_better")
        }
        d => panic!("a restored threshold must passthrough under it, got {d:?}"),
    }
}

#[test]
fn selective_gate_huge_html_anchored_intercepts() {
    // Case 4 (kept): huge page + anchored prompt → scoped excerpt with
    // the answer inline.
    let _g = INSTALL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("TOKENADE_WEBFETCH_INTERCEPT_MIN_TOKENS");
    let page = gate_page(1600);
    let toks = tokenade_core::tokenizer::estimate(&page);
    match super::selective_gate_decision(
        "zeta_widget calibration default",
        &page, None,
        toks,
        false,
        false,
    ) {
        super::SelectiveDecision::Intercept { extract } => {
            assert!(
                extract.contains("42ms"),
                "answer must be in excerpt: {extract}"
            );
        }
        d => panic!("huge+anchored must intercept, got {d:?}"),
    }
}

#[test]
fn selective_gate_huge_html_no_anchor_passthrough() {
    // Big page but the prompt has no lexical foothold → native wins
    // (paraphrase is the small model's game — our measured ceiling).
    let _g = INSTALL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("TOKENADE_WEBFETCH_INTERCEPT_MIN_TOKENS");
    let page = gate_page(1600);
    let toks = tokenade_core::tokenizer::estimate(&page);
    match super::selective_gate_decision(
        "summarise the general vibe please",
        &page, None,
        toks,
        false,
        false,
    ) {
        super::SelectiveDecision::Passthrough(why) => {
            assert_eq!(why, "webfetch_passthrough_no_anchor")
        }
        d => panic!("no-anchor must passthrough, got {d:?}"),
    }
}

#[test]
fn walled_host_intercepts_even_below_the_size_gate() {
    // The size gate encodes "the native model wins the small-page case" —
    // true only where the native fetcher can FETCH. On a host that 403s
    // plain clients it returns an error, so passthrough is a lost turn at
    // any page size. MEASURED on britannica.com: 403 + 66 words to the
    // honest UA, 200 + 1712 words to the browser profile.
    //
    // The default size gate is gone (2026-08-01), so this test RESTORES one via
    // the env knob — otherwise "below the size gate" has no meaning and the
    // walled-vs-reachable contrast the invariant is about disappears.
    let _g = INSTALL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let page = gate_page(80);
    let toks = tokenade_core::tokenizer::estimate(&page);
    std::env::set_var(
        "TOKENADE_WEBFETCH_INTERCEPT_MIN_TOKENS",
        (toks + 1).to_string(),
    );
    let restored = super::webfetch_intercept_min_tokens();
    // Decide everything while the knob is set, then clear it BEFORE asserting so
    // a failure here cannot leak the override into the next test in this process.
    let walled_anchored =
        super::selective_gate_decision("zeta_widget calibration default", &page, None, toks, true, false);
    let walled_no_anchor =
        super::selective_gate_decision("summarise the general vibe", &page, None, toks, true, false);
    let reachable = super::selective_gate_decision(
        "zeta_widget calibration default",
        &page, None,
        toks,
        false,
        false,
    );
    std::env::remove_var("TOKENADE_WEBFETCH_INTERCEPT_MIN_TOKENS");

    assert!(toks < restored, "{toks}");
    // Anchored prompt → scoped excerpt.
    assert!(
        matches!(walled_anchored, super::SelectiveDecision::Intercept { .. }),
        "walled host must not passthrough"
    );
    // No anchor → the whole distilled page, still never the wall.
    assert!(
        matches!(walled_no_anchor, super::SelectiveDecision::InterceptWhole),
        "walled host with no anchor must serve the page whole"
    );
    // Same page, native NOT blocked, threshold restored → the size gate applies.
    assert!(
        matches!(
            reachable,
            super::SelectiveDecision::Passthrough("webfetch_passthrough_native_better")
        ),
        "reachable host keeps the size gate when one is configured"
    );
}

#[test]
fn selective_gate_min_tokens_env_override() {
    let _g = INSTALL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    std::env::set_var("TOKENADE_WEBFETCH_INTERCEPT_MIN_TOKENS", "100");
    let page = gate_page(300);
    let toks = tokenade_core::tokenizer::estimate(&page);
    let d = super::selective_gate_decision(
        "zeta_widget calibration default",
        &page, None,
        toks,
        false,
        false,
    );
    std::env::remove_var("TOKENADE_WEBFETCH_INTERCEPT_MIN_TOKENS");
    assert!(
        matches!(d, super::SelectiveDecision::Intercept { .. }),
        "lowered threshold must flip to intercept, got {d:?}"
    );
}

#[test]
fn webfetch_intercept_mode_env_parsing() {
    let _g = INSTALL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("TOKENADE_WEBFETCH_INTERCEPT");
    assert_eq!(
        super::webfetch_intercept_mode(),
        super::WebfetchInterceptMode::Selective
    );
    std::env::set_var("TOKENADE_WEBFETCH_INTERCEPT", "1");
    assert_eq!(
        super::webfetch_intercept_mode(),
        super::WebfetchInterceptMode::Legacy
    );
    std::env::set_var("TOKENADE_WEBFETCH_INTERCEPT", "legacy");
    assert_eq!(
        super::webfetch_intercept_mode(),
        super::WebfetchInterceptMode::Legacy
    );
    std::env::set_var("TOKENADE_WEBFETCH_INTERCEPT", "0");
    assert_eq!(
        super::webfetch_intercept_mode(),
        super::WebfetchInterceptMode::Off
    );
    std::env::remove_var("TOKENADE_WEBFETCH_INTERCEPT");
}

#[test]
fn webfetch_session_cache_roundtrip() {
    // Case 5: record a URL→stash mapping, look it up, get the content.
    let _g = INSTALL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = std::env::temp_dir().join(format!("tok-webfetch-cache-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    std::env::set_var("TOKENADE_WEBFETCH_CACHE_DIR", &tmp);
    let stash_file = tmp.join("page.md");
    std::fs::write(&stash_file, "cached distilled markdown body").unwrap();
    super::webfetch_cache_record("sess-abc-123", "https://x.example/page", &stash_file);
    let hit = super::webfetch_cache_lookup("sess-abc-123", "https://x.example/page");
    assert_eq!(hit.as_deref(), Some("cached distilled markdown body"));
    // Different URL and different session → miss.
    assert!(super::webfetch_cache_lookup("sess-abc-123", "https://x.example/other").is_none());
    assert!(super::webfetch_cache_lookup("other-sess", "https://x.example/page").is_none());
    std::env::remove_var("TOKENADE_WEBFETCH_CACHE_DIR");
}

#[test]
fn build_webfetch_scoped_doc_reason_contains_excerpt_and_pointer() {
    // PDF big-extract route: answer inline + stashed-text pointer.
    let ext = super::WebfetchExtraction {
        text: "full text".into(),
        format_label: "pdf",
        tokens: 30_000,
        inlined: false,
        stash_path: Some("/tmp/stash/abc.txt".into()),
    };
    let r = super::build_webfetch_scoped_doc_reason(
        "https://x.example/paper.pdf",
        2_000_000,
        &ext,
        "the base model uses 8 attention heads",
        "how many attention heads",
        "[stash block]",
    );
    assert!(r.contains("8 attention heads"), "answer inline: {r}");
    assert!(r.contains("/tmp/stash/abc.txt"));
    assert!(r.contains("do NOT re-fetch"));
}

#[test]
fn webfetch_hint_pdf_directs_to_docs() {
    // #947 lock-in: agent should NOT pivot to WebSearch when we
    // have a PDF stash. Point at `tokenade web <stash>` (the read/web
    // path extracts PDF/DOCX) so the text comes back on the next turn.
    let h = super::pick_webfetch_recovery_hint(Some("pdf"), Some("application/pdf"));
    assert!(h.contains("tokenade web"), "got: {h}");
    assert!(
        !h.contains("xxd"),
        "PDF hint shouldn't suggest hex dumping: {h}"
    );
}

#[test]
fn webfetch_hint_html_directs_to_html_to_markdown() {
    // #947 lock-in: the arxiv-abstract case — text/html that
    // bytewise-detected as binary due to inline SVG/JS noise. The
    // agent should be told `tokenade web` (0.5.6 merge of the three
    // web fetchers, kind=page) handles it.
    let h = super::pick_webfetch_recovery_hint(Some("bin"), Some("text/html"));
    assert!(h.contains("tokenade web"), "got: {h}");
    assert!(
        h.contains("text/html"),
        "should explain WHY (Content-Type): {h}"
    );
}

#[test]
fn try_extract_webfetch_returns_none_on_non_binary_kind() {
    // #949 lock-in: a `bin`/unknown kind that doesn't map to a
    // docs::Format should NOT trigger extraction. Returning None
    // keeps the caller on the legacy generic-hint path.
    let bytes = b"not a real binary";
    let r = super::try_extract_webfetch(bytes, Some("bin"), Some("text/html"));
    assert!(r.is_none(), "unknown kind shouldn't extract: got Some");
}

#[test]
fn try_extract_webfetch_returns_none_on_oversized_input() {
    // #949 lock-in: hard cap at 20 MB. A 21 MB blob with kind=pdf
    // must skip extraction entirely (don't even read the bytes).
    let big = vec![0u8; 21 * 1024 * 1024];
    let r = super::try_extract_webfetch(&big, Some("pdf"), Some("application/pdf"));
    assert!(r.is_none(), "oversized input must skip extraction");
}

#[test]
fn try_extract_webfetch_returns_none_on_unparseable_pdf() {
    // #949 lock-in: garbage bytes claiming to be a PDF → docs::extract
    // returns empty (no panic), try_extract returns None → caller falls
    // back to the generic-hint message rather than emitting a half-baked
    // "extracted 0 tokens" banner.
    let r = super::try_extract_webfetch(b"%PDF-fake", Some("pdf"), Some("application/pdf"));
    assert!(r.is_none(), "unparseable bytes must return None");
}

#[test]
fn extract_stash_path_handles_summary_shapes() {
    // #949 lock-in: the disclose-summary parser must extract the
    // path out of the `[stashed: <path> · …]` marker.
    let s1 = "[stashed: /home/username/.cache/tokenade/stash/abcd1234.pdf · use Read]";
    assert_eq!(
        super::extract_stash_path(s1).as_deref(),
        Some("/home/username/.cache/tokenade/stash/abcd1234.pdf"),
    );
    // #954 lock-in: custom TOKENADE_STASH_DIR paths must work too.
    // Previous impl anchored on the literal "/stash/" substring,
    // which silently returned None when users redirected stash
    // away from the default `~/.cache/tokenade/stash/` layout.
    let s_custom = "[stashed: /var/cache/my-dump/abcd1234.txt · use Read]";
    assert_eq!(
        super::extract_stash_path(s_custom).as_deref(),
        Some("/var/cache/my-dump/abcd1234.txt"),
    );
    // No stash marker → None.
    assert!(super::extract_stash_path("nothing relevant here").is_none());
    // Relative path inside marker → reject (we only emit absolute).
    assert!(super::extract_stash_path("[stashed: relative/path.txt · use Read]").is_none());
}

#[test]
fn build_webfetch_extracted_reason_inlined_shape() {
    // #949 lock-in: inlined extraction must contain the text body
    // BEGIN/END markers + raw stash block, in that order.
    let ext = super::WebfetchExtraction {
        text: "Hello, world.\nThis is the PDF body.".to_string(),
        format_label: "pdf",
        tokens: 12,
        inlined: true,
        stash_path: None,
    };
    let reason = super::build_webfetch_extracted_reason(
        "https://example.com/p.pdf",
        12345,
        &ext,
        "[stashed: /tmp/raw.pdf · use Read]",
    );
    assert!(reason.contains("webfetch-binary-extracted"));
    assert!(reason.contains("12345 bytes of pdf"));
    assert!(reason.contains("This is the PDF body."));
    assert!(reason.contains("BEGIN EXTRACTED TEXT"));
    assert!(reason.contains("[stashed: /tmp/raw.pdf"));
    // Inline path must NOT mention "stashed at" line (that's the
    // path-referenced shape).
    assert!(
        !reason.contains("stashed at:"),
        "inlined shape should not say stashed at:"
    );
}

#[test]
fn build_webfetch_extracted_reason_path_referenced_shape() {
    // #949 lock-in: large extraction stashes the text and only the
    // path is mentioned in the banner (no inline body).
    let ext = super::WebfetchExtraction {
        text: "x".repeat(50_000),
        format_label: "epub",
        tokens: 12_500,
        inlined: false,
        stash_path: Some("/home/x/.cache/tokenade/stash/cafef00d.txt".to_string()),
    };
    let reason = super::build_webfetch_extracted_reason(
        "https://example.com/book.epub",
        1_234_567,
        &ext,
        "[stashed: /tmp/book.epub · use Read]",
    );
    assert!(reason.contains("12500 tokens"));
    assert!(reason.contains("stashed at:"));
    assert!(reason.contains("/home/x/.cache/tokenade/stash/cafef00d.txt"));
    // Body itself must NOT be inlined.
    assert!(!reason.contains("BEGIN EXTRACTED TEXT"));
    // 50k x's must not appear in the reason — proves stash-only.
    assert!(!reason.contains(&"x".repeat(100)));
}

#[test]
fn webfetch_hint_unknown_keeps_generic_fallback() {
    // Regression guard: when neither kind nor CT matches a
    // recognized category, we fall back to the original generic
    // `file / xxd / hexdump` advice rather than silently empty.
    let h = super::pick_webfetch_recovery_hint(None, None);
    assert!(h.contains("file <stash file shown below>"), "got: {h}");
    assert!(h.contains("xxd"), "got: {h}");
}

#[test]
fn non_2xx_responses_are_labelled_before_being_served() {
    use crate::hooks::webfetch_pretool::{http_status_note, with_status_note};
    // Measured on 93 real non-200 bodies from the CrUX long tail: the pipeline
    // distilled 404 and 403 pages into 166-2149 chars of ordinary-looking prose
    // with nothing marking them as failures. The status was known at fetch time
    // and thrown away, so an agent reading a dead URL believed it read the page.
    assert!(
        http_status_note(200).is_none(),
        "a 200 must not be annotated"
    );
    assert!(http_status_note(204).is_none());

    let note = http_status_note(404).expect("404 must be annotated");
    assert!(note.contains("HTTP 404"), "{note}");
    assert!(note.contains("does not exist"), "{note}");
    assert!(note.contains("ERROR page"), "{note}");

    assert!(http_status_note(403)
        .unwrap()
        .contains("access was refused"));
    assert!(http_status_note(429).unwrap().contains("rate-limited"));
    assert!(http_status_note(503).unwrap().contains("server failed"));

    // Additive: the body still gets through, because an error page sometimes
    // carries the only clue about what went wrong.
    let served = with_status_note(404, "Page not found. Try our search instead.");
    assert!(served.starts_with("[tokenade: HTTP 404"), "{served}");
    assert!(served.contains("Try our search instead."), "{served}");
    assert_eq!(with_status_note(200, "real content"), "real content");
}

/// Field report 2026-07-31: a 404 was served as `✓ FETCH OK … this is a
/// SUCCESS, not a failure — do NOT retry`, one line above our own
/// `[tokenade: HTTP 404 — the page does not exist]`. The marker exists so the
/// `<error>` wrapper around a PreToolUse deny is not mistaken for a failed
/// fetch — but when the fetch DID fail, that wrapper is correct and the marker
/// must not argue with it.
#[test]
fn a_served_http_error_is_never_announced_as_a_success() {
    use crate::hooks::webfetch_pretool::serve::webfetch_lead_marker;
    use crate::hooks::webfetch_pretool::{http_status_note, leading_status_code};

    let body_404 = format!("{}Page not found.", http_status_note(404).unwrap());
    let m = webfetch_lead_marker(&body_404);
    assert!(m.contains("FETCH FAILED"), "{m}");
    assert!(m.contains("HTTP 404"), "{m}");
    assert!(!m.contains("SUCCESS"), "{m}");
    assert!(m.contains("Retrying the same URL will not help"), "{m}");

    // 429 / 5xx are exactly the cases where retrying IS right, so the old
    // blanket "do NOT retry" was wrong in both directions.
    for code in [429u16, 503] {
        let body = format!("{}Slow down.", http_status_note(code).unwrap());
        let m = webfetch_lead_marker(&body);
        assert!(m.contains("a later retry may succeed"), "{code}: {m}");
        assert!(!m.contains("SUCCESS"), "{code}: {m}");
    }
    let auth = format!("{}Login.", http_status_note(403).unwrap());
    assert!(
        webfetch_lead_marker(&auth).contains("needs credentials"),
        "{auth}"
    );

    // A 2xx keeps the success marker — including a page that distilled to
    // little, which has honest causes and is NOT known to be an error.
    assert!(webfetch_lead_marker("real content").contains("FETCH OK"));
    let thin = "[tokenade: thin-result] recovered 12 characters from 13 KB of HTML";
    assert!(webfetch_lead_marker(thin).contains("FETCH OK"), "{thin}");

    // The status is read back out of the note the pipeline itself wrote, so
    // writer and parser cannot drift.
    assert_eq!(leading_status_code(&body_404), Some(404));
    assert_eq!(leading_status_code("no note here"), None);
    assert_eq!(leading_status_code("[tokenade: HTTP xx — nope"), None);

    // A 404 reached THROUGH A REDIRECT does not start with the status note —
    // `provenance_notes` puts the redirect first. Anchoring on byte 0 would
    // report that as a success, which is the same bug one layer down.
    let redirected = format!(
        "{}{}Gone.",
        crate::hooks::webfetch_pretool::redirect_note("https://a.test/x", Some("https://b.test/y"))
            .unwrap(),
        http_status_note(404).unwrap()
    );
    assert_eq!(leading_status_code(&redirected), Some(404));
    assert!(
        webfetch_lead_marker(&redirected).contains("FETCH FAILED"),
        "{redirected}"
    );

    // …and a multibyte glyph before the note must not turn the lookup into a
    // None that reads as success (a byte slice at a fixed offset would).
    let accented = format!(
        "[tokenade: redirigé — é{}]\n\n{}Gone.",
        "é".repeat(120),
        http_status_note(410).unwrap()
    );
    assert_eq!(leading_status_code(&accented), Some(410));
}

#[test]
fn silent_redirects_are_disclosed_but_cosmetic_ones_are_not() {
    use crate::hooks::webfetch_pretool::{provenance_notes, redirect_note};
    // Measured on 225 fetches of the CrUX global top 1000: 26 (11.6%) ended on
    // a different host. The ones that matter return HTTP 200, so the status
    // note cannot catch them.
    let sso = redirect_note(
        "https://dzen.ru",
        Some("https://sso.passport.yandex.ru/push?uuid=abc"),
    )
    .expect("an SSO landing must be disclosed");
    assert!(
        sso.contains("dzen.ru") && sso.contains("sso.passport.yandex.ru"),
        "{sso}"
    );

    assert!(
        redirect_note("https://m.vk.com", Some("https://m.vk.ru/")).is_some(),
        "TLD move"
    );
    assert!(redirect_note(
        "https://blacktoon413.com",
        Some("https://blacktoonurl.net/")
    )
    .is_some());

    // A homepage landing on its own localised or index path is noise: 22 of the
    // 48 redirects in the sample were this, and reporting them would drown the
    // 26 that matter.
    assert!(redirect_note("https://m.shein.com", Some("https://m.shein.com/fr/?ref=m")).is_none());
    assert!(redirect_note(
        "https://m.humoruniv.com",
        Some("https://m.humoruniv.com/main.html")
    )
    .is_none());
    assert!(
        redirect_note("https://example.com", Some("https://www.example.com/")).is_none(),
        "www is not a move"
    );
    assert!(
        redirect_note("https://example.com/a", Some("https://example.com/a/")).is_none(),
        "trailing slash"
    );
    assert!(redirect_note("https://example.com", None).is_none());

    // A document that MOVED is worth saying, unlike a homepage that localised.
    assert!(redirect_note(
        "https://example.com/2020/article",
        Some("https://example.com/archive/gone")
    )
    .is_some());

    // Both notes compose, redirect first, and the body still comes through.
    let out = format!(
        "{}{}",
        provenance_notes(404, "https://a.test/x", Some("https://b.test/y"), false),
        "Not found here."
    );
    assert!(out.starts_with("[tokenade: redirected"), "{out}");
    assert!(out.contains("[tokenade: HTTP 404"), "{out}");
    assert!(out.ends_with("Not found here."), "{out}");
    assert_eq!(
        provenance_notes(200, "https://a.test/", Some("https://a.test/"), false),
        ""
    );
}

#[test]
fn provenance_does_not_leak_credentials_from_the_landing_url() {
    use crate::hooks::webfetch_pretool::provenance_notes;
    // The landing URL is server-chosen and routinely carries secrets: the
    // motivating example was `sso.passport.yandex.ru/push?uuid=…`, a session
    // identifier. Signed URLs arrive the same way. The note is built after the
    // body has been scrubbed, so it must scrub itself.
    let note = provenance_notes(
        200,
        "https://api.test/thing",
        Some("https://cdn.test/x?token=sk-live-ABCDEF1234567890abcdef"), false);
    assert!(note.contains("redirected"), "{note}");
    assert!(
        !note.contains("sk-live-ABCDEF1234567890abcdef"),
        "credential echoed into the agent's context: {note}"
    );

    // A clean 200 with no redirect stays completely silent.
    assert_eq!(
        provenance_notes(200, "https://a.test/x", Some("https://a.test/x"), false),
        ""
    );
}

#[test]
fn provenance_is_counted_in_what_the_agent_is_charged() {
    use crate::hooks::webfetch_pretool::provenance_notes;
    // Round 27 prefixed these notes AFTER the savings were computed, so every
    // non-2xx or redirected fetch booked a credit for tokens it then went on to
    // deliver. Small per call, systematically wrong, and the ledger is not the
    // place to be approximately right.
    //
    // This pins the invariant the four serve paths now hold: whatever the note
    // costs is part of what gets measured.
    let note = provenance_notes(404, "https://a.test/x", Some("https://b.test/gone"), false);
    assert!(!note.is_empty(), "a redirected 404 must produce a note");

    let body = "The distilled page body that the agent receives.";
    let delivered = format!("{note}{body}");
    let booked =
        tokenade_core::tokenizer::estimate(body) + tokenade_core::tokenizer::estimate(&note);
    assert_eq!(
        tokenade_core::tokenizer::estimate(&delivered),
        booked,
        "the note's cost must be additive, or the ledger drifts from delivery"
    );
    // A clean fetch stays free: no note, nothing added.
    assert_eq!(
        provenance_notes(200, "https://a.test/x", Some("https://a.test/x"), false),
        ""
    );
}

#[test]
fn a_small_failure_is_not_passed_through_silently() {
    use crate::hooks::webfetch_pretool::{may_pass_through_small_body, Disclosure};
    let small = 400; // a plain nginx 403 body is 134-551 bytes
    let big = 50_000;

    // The size passthrough is a TOKEN decision. It was silently making a
    // CORRECTNESS one: 30 of 93 real non-200 bodies are under the 1500-byte
    // threshold, so a third of failures — and every bot wall small enough to be
    // one — reached the agent with no status note and no anti-bot banner,
    // because both are added downstream of the early return.
    // `new_bounded` and not a three-argument `new`: that shortcut had only
    // these three calls left, all of them TESTS, and the release pipeline's
    // dead-code guard compiles a configuration that `src/tests/` is not part
    // of — so it saw it as dead, rightly, and blocked 1.1.10.
    let d = |code, eff| Disclosure::new_bounded(code, "https://a.test/x", Some(eff), false);
    let clean = d(200, "https://a.test/x");
    let failed = d(403, "https://a.test/x");
    let moved = d(200, "https://sso.other/login");

    assert!(
        may_pass_through_small_body(small, &clean),
        "a small SUCCESSFUL page should still pass through"
    );
    assert!(
        !may_pass_through_small_body(small, &failed),
        "a small 403 must be intercepted so the agent is told"
    );
    assert!(
        !may_pass_through_small_body(small, &moved),
        "a small page reached via a silent host change must be intercepted"
    );
    // Size still governs everything we have nothing to say about.
    assert!(!may_pass_through_small_body(big, &clean));
}

#[test]
fn the_selective_gate_yields_to_a_disclosure() {
    // Round 29 stopped the SMALL-BODY passthrough so a failure would reach the
    // disclosure. It was incomplete: a 551-byte 403 gets past that gate,
    // distils to a handful of tokens, and then falls through THIS one on
    // `compact_tokens < min_tokens`. Two gates, one on bytes and one on tokens,
    // both deciding correctness as a side effect of a token trade.
    let _g = INSTALL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("TOKENADE_WEBFETCH_INTERCEPT_MIN_TOKENS");
    let page = "403 Forbidden\n\nnginx";
    let toks = tokenade_core::tokenizer::estimate(page);

    // Nothing to say and no anchor (empty prompt) → still passthrough. Since
    // 2026-08-01 the reason is the anchor leg, not the size gate.
    match super::selective_gate_decision("", page, None, toks, false, false) {
        super::SelectiveDecision::Passthrough(_) => {}
        d => panic!("a small page with nothing to disclose must passthrough, got {d:?}"),
    }
    // Something to say → serve it, whatever the token count.
    match super::selective_gate_decision("", page, None, toks, false, true) {
        super::SelectiveDecision::InterceptWhole => {}
        d => panic!("a disclosure must not be dropped by the size gate, got {d:?}"),
    }
}

#[test]
fn a_200_that_is_really_a_cdn_error_page_cannot_be_announced_as_success() {
    // crw's finding, ported: the edge answers instead of the origin and does it
    // with `200 OK`, so the status note has nothing to say and the lead marker
    // used to read "✓ FETCH OK" over a gateway error page. The body is the only
    // evidence there is.
    use crate::hooks::webfetch_pretool::Disclosure;
    let vercel = "<!doctype html><html><body><h1>404</h1><p>DEPLOYMENT_NOT_FOUND</p></body></html>";
    let disc = Disclosure::new_bounded(200, "https://a.test/x", Some("https://a.test/x"), false).with_body(vercel);
    assert!(
        disc.must_reach_agent(),
        "a 200 carrying an edge error page still has something to disclose"
    );
    let marker = disc.lead_marker();
    assert!(marker.starts_with("✗ FETCH FAILED"), "got: {marker}");
    assert!(
        marker.contains("200"),
        "must say the status can be 200: {marker}"
    );

    // A real 200 with real content keeps the success marker — the guard must
    // not turn every page into a failure.
    let ok = Disclosure::new_bounded(200, "https://a.test/x", Some("https://a.test/x"), false)
        .with_body("<html><body><p>Real article text.</p></body></html>");
    assert!(!ok.must_reach_agent());
    assert!(!ok.lead_marker().starts_with("✗"));
}

#[test]
fn no_gate_can_drop_a_pending_disclosure() {
    // THE invariant the `Disclosure` type exists to hold. Rounds 24-30 shipped
    // this feature through four independent gates — redaction, the savings
    // ledger, a byte gate, a token gate — teaching each one separately, and it
    // took seven rounds to reach the agent on the default path. This asserts
    // the property directly instead of asserting each gate.
    use crate::hooks::webfetch_pretool::{
        may_pass_through_small_body, selective_gate_decision, Disclosure, SelectiveDecision,
    };
    let _g = INSTALL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("TOKENADE_WEBFETCH_INTERCEPT_MIN_TOKENS");

    let tiny_body = "403 Forbidden\n\nnginx";
    let toks = tokenade_core::tokenizer::estimate(tiny_body);

    for disc in [
        Disclosure::new_bounded(404, "https://a.test/x", Some("https://a.test/x"), false),
        Disclosure::new_bounded(403, "https://a.test/x", Some("https://a.test/x"), false),
        Disclosure::new_bounded(
            200,
            "https://a.test/x",
            Some("https://elsewhere.test/login"), false),
    ] {
        assert!(
            disc.must_reach_agent(),
            "fixture must have something to say"
        );
        // Gate 1: bytes.
        assert!(!may_pass_through_small_body(tiny_body.len(), &disc));
        // Gate 2: tokens.
        assert!(
            !matches!(
                selective_gate_decision("", tiny_body, None, toks, false, disc.must_reach_agent()),
                SelectiveDecision::Passthrough(_)
            ),
            "the token gate dropped a pending disclosure"
        );
        // And the cost travels with the text, never separately.
        let d = disc.attach("body");
        assert!(d.added_tokens > 0 && d.text.ends_with("body") && d.text.len() > 4);
    }

    // A clean response is still free at every gate.
    let clean = Disclosure::new_bounded(200, "https://a.test/x", Some("https://a.test/x"), false);
    assert!(!clean.must_reach_agent());
    assert!(may_pass_through_small_body(tiny_body.len(), &clean));
    assert_eq!(clean.attach("body").added_tokens, 0);
}

/// The note asked the ONE party that cannot answer. Measured over this
/// machine's transcripts: 37 real deliveries carried "Check the HTTP status";
/// in 26 no status line was present anywhere in the message, and in the other
/// 11 it was stated three lines above (404 x9, 403, 429). tokenade made the
/// request, so the answer is its to give.
#[test]
fn the_note_answers_the_status_question_instead_of_asking_it() {
    use crate::hooks::webfetch_pretool::{attach_provenance, provenance_notes};
    use tokenade_core::web::pipeline::STATUS_QUESTION;

    let body = format!("The response carried no extractable content. {STATUS_QUESTION}");

    // Status known: name it, and do not leave the question standing.
    let prefix = provenance_notes(404, "https://a.test/x", Some("https://a.test/x"), false);
    let out = attach_provenance(&prefix, &body);
    assert!(
        !out.contains(STATUS_QUESTION),
        "the question survived next to its own answer: {out}"
    );
    assert!(
        out.contains("HTTP 404, stated above"),
        "the answer must name the status: {out}"
    );

    // No status note: its absence is the answer, not an errand.
    let out2 = attach_provenance("", &body);
    assert!(
        !out2.contains(STATUS_QUESTION),
        "question left standing: {out2}"
    );
    assert!(
        out2.contains("nothing here is a status-level refusal"),
        "the absent status must be read, not delegated: {out2}"
    );
    // …and it must not claim a server answered when this path also serves a
    // local file.
    assert!(
        !out2.contains("the server answered 2xx") && !out2.contains("server did answer"),
        "asserts an exchange it did not observe: {out2}"
    );

    // A body without the clause is untouched, byte for byte.
    let plain = "just a page.";
    assert_eq!(attach_provenance("", plain), plain);

    // A 2xx prefix is not a refusal either.
    let ok = provenance_notes(200, "https://a.test/x", Some("https://a.test/x"), false);
    assert!(
        attach_provenance(&ok, &body).contains("nothing here is a status-level refusal"),
        "a 2xx must read as the client-side case"
    );
}

/// Round 16 — yelp.com answered 403 with a page that distilled to nothing, and
/// the whole output was a banner saying "What follows is the ERROR page". The
/// banner must describe what is actually there, in BOTH directions.
#[test]
fn status_note_does_not_promise_a_body_that_is_not_there() {
    use crate::hooks::webfetch_pretool::{
        attach_provenance, provenance_notes, Disclosure, NOTE_BODY_EMPTY, NOTE_BODY_FOLLOWS,
    };

    let prefix = provenance_notes(403, "https://a.test/x", Some("https://a.test/x"), false);
    assert!(
        prefix.contains(NOTE_BODY_FOLLOWS),
        "default clause: {prefix}"
    );

    for empty in ["", "   ", "\n\n"] {
        let out = attach_provenance(&prefix, empty);
        assert!(
            out.contains(NOTE_BODY_EMPTY),
            "empty body must say so: {out:?}"
        );
        assert!(
            !out.contains(NOTE_BODY_FOLLOWS),
            "must not promise a body: {out:?}"
        );
    }

    let out = attach_provenance(&prefix, "Forbidden.");
    assert!(
        out.contains(NOTE_BODY_FOLLOWS),
        "non-empty body keeps the warning: {out:?}"
    );
    assert!(out.ends_with("Forbidden."));

    // The cost booked is the note actually emitted, not the one built at fetch.
    let disc = Disclosure::new_bounded(403, "https://a.test/x", Some("https://a.test/x"), false);
    let empty = disc.attach("");
    let full = disc.attach("Forbidden.");
    assert!(empty.text.contains(NOTE_BODY_EMPTY));
    assert!(empty.added_tokens > 0 && empty.added_tokens <= full.added_tokens);

    // A 200 has no clause to swap, so an empty body stays untouched.
    assert_eq!(attach_provenance("", ""), "");
}

/// A document fetched from a URL is redacted like a local one.
///
/// The LOCAL path (`compactors::cmd_auto`) redacts the extract at the source
/// and says why: a document can hold secrets. The WebFetch binary path had no
/// redaction anywhere along it — not on the inlined reason, not on the stashed
/// `.txt`, and not on `prompt_scoped_extract_dispatch`, which returns the raw
/// matching lines and cannot redact for itself.
#[test]
fn a_document_fetched_from_a_url_is_redacted_like_a_local_one() {
    // Split so the literal never appears whole in this source file.
    let key = format!("AKIA{}", "IOSFODNN7EXAMPLE");
    let rtf = format!(r"{{\rtf1\ansi runbook step 3: export AWS_ACCESS_KEY_ID={key} then deploy}}");

    let ext = super::try_extract_webfetch(rtf.as_bytes(), Some("rtf"), Some("application/rtf"))
        .expect("rtf extracts");
    assert!(
        !ext.text.contains(&key),
        "the extract handed to the inline reason, the stash and the scoped \
         extract still carries the credential:\n{}",
        ext.text
    );
    assert!(
        ext.text.contains("runbook step 3"),
        "redaction must not eat the document:\n{}",
        ext.text
    );
}

/// …and the branch that answers a QUESTION about the document.
///
/// `prompt_scoped_extract_dispatch` returns the raw matching lines and has no
/// redaction of its own — `compactors::cmd_auto` says so where it redacts for
/// the local path. Asking about anything near a key printed it.
///
/// The third branch, the `.txt` written to disk, was ALREADY safe: the stash
/// layer redacts on write (#475). Measured, not assumed — a guard placed there
/// stayed green with this fix removed, which is what sent us to check.
#[test]
fn asking_a_question_about_a_fetched_document_does_not_print_the_key() {
    let key = format!("AKIA{}", "IOSFODNN7EXAMPLE");
    let mut filler = String::new();
    for i in 0..3000 {
        filler.push_str(&format!(
            "Section {i} describes deployment step {i} for service number {i}. "
        ));
    }
    let rtf = format!(
        r"{{\rtf1\ansi {filler} The aws credentials for the runbook are AWS_ACCESS_KEY_ID={key} and must be rotated. {filler}}}"
    );

    let ext = super::try_extract_webfetch(rtf.as_bytes(), Some("rtf"), Some("application/rtf"))
        .expect("rtf extracts");
    assert!(
        !ext.inlined,
        "the fixture must exceed the inline cap or the scoped path never runs"
    );
    let scoped = tokenade_core::rag::prompt_scoped_extract_dispatch(
        &ext.text,
        "aws credentials",
        tokenade_core::disclose::webfetch_scoped_budget_for(tokenade_core::tokenizer::estimate(
            &ext.text,
        )),
    )
    .expect("an anchored prompt scopes the extract");
    assert!(
        !scoped.contains(&key),
        "the passages returned for a question still carry the credential:\n{scoped}"
    );
}

#[test]
fn a_page_smaller_than_its_own_excuse_is_served_whole() {
    // The scoped delivery carries ~150 tokens of prose the whole-page
    // delivery does not (the "too large for the native fetch" paragraph and
    // the `expand-ref` recovery line). `serve_legacy_intercept` has always
    // asked whether a page is small enough to serve whole; the selective gate,
    // which is the DEFAULT path, never did — so a 122-token page came back as
    // an excerpt behind a banner bigger than the page itself, and reading the
    // rest cost a turn.
    //
    // Both halves are asserted: the small page stops paying for scoping, and a
    // page of ordinary size still scopes exactly as before.
    let _g = INSTALL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("TOKENADE_WEBFETCH_INLINE_TOKENS");
    let prompt = "zeta_widget calibration default";
    let scoped_for = |page: &str| -> String {
        let toks = tokenade_core::tokenizer::estimate(page);
        tokenade_core::rag::prompt_scoped_extract_dispatch(
            page,
            prompt,
            tokenade_core::disclose::webfetch_scoped_budget_for(toks),
        )
        .expect("the anchor is on the page")
    };

    let small = gate_page(6);
    let small_toks = tokenade_core::tokenizer::estimate(&small);
    let small_extract = scoped_for(&small);
    assert!(
        !super::scoping_pays(small_toks, &small_extract),
        "a {small_toks}-token page must be served whole, not scoped to \
         {} tokens behind {} tokens of explanation",
        tokenade_core::tokenizer::estimate(&small_extract),
        super::WEBFETCH_SCOPING_OVERHEAD_TOKENS,
    );

    let ordinary = gate_page(150);
    let ordinary_toks = tokenade_core::tokenizer::estimate(&ordinary);
    let ordinary_extract = scoped_for(&ordinary);
    assert!(
        super::scoping_pays(ordinary_toks, &ordinary_extract),
        "a {ordinary_toks}-token page must still be scoped (extract {} tokens)",
        tokenade_core::tokenizer::estimate(&ordinary_extract),
    );

    // A page too big to serve inline is scoped whatever the arithmetic says:
    // there the alternative is not "serve whole", it is "overflow the context".
    let over = super::webfetch_inline_cap() + 1;
    assert!(
        super::scoping_pays(over, &"x ".repeat(over * 4)),
        "a page over the inline cap must never fall back to serving whole"
    );
}

/// **A reader turned away by the site no longer disappears without a word.**
///
/// Measured 2026-09-11: reddit answers 429 after a few calls, the reader
/// returns `None`, and the delivered page said "recovered 0 characters …
/// isolation likely picked the wrong element" — a false diagnosis, since our
/// isolation had nothing to do with it. The note travels through provenance,
/// the one place every channel goes through.
#[test]
fn a_reader_the_site_refused_says_so_on_the_page_that_replaces_it() {
    use crate::hooks::webfetch_pretool::readers;
    let url = "https://www.reddit.com/r/rust/";
    readers::note_reader_refusal_for_test("reddit .rss", url, 429, true);
    let note = provenance_notes(200, url, Some(url), false);
    assert!(note.contains("reader refused"), "{note:?}");
    assert!(note.contains("429"), "{note:?}");
    assert!(note.contains("rate limit"), "{note:?}");
    // Taken ONCE: it cannot carry over to the next target.
    assert_eq!(provenance_notes(200, url, Some(url), false), "");
}

/// The bound: the note belongs to the address that produced it.
#[test]
fn a_refusal_does_not_travel_to_the_next_target() {
    use crate::hooks::webfetch_pretool::readers;
    readers::note_reader_refusal_for_test("reddit .rss", "https://a.test/x", 429, true);
    assert_eq!(provenance_notes(200, "https://b.test/y", None, false), "");
    assert!(provenance_notes(200, "https://a.test/x", None, false).contains("reader refused"));
}

/// **The banner must no longer blame our isolation for a site's refusal.**
///
/// Two sentences of the `thin-result` banner are false when a reader was
/// refused: its last explanation ("isolation likely picked the wrong
/// element") and the answer to its status question ("no status note precedes
/// this"), since the fetch that REPLACED the reader did answer 200. The page
/// contradicted itself, at the top and three lines below.
#[test]
fn a_refused_reader_silences_the_banners_guess_about_us() {
    use crate::hooks::webfetch_pretool::readers;
    let url = "https://www.reddit.com/r/rust/";
    let banner = format!(
        "[tokenade: thin-result] recovered 0 characters from 8 KB of HTML ({}). Possible \
         causes: {}.\n\n---\n\nquelque chose",
        tokenade_core::web::pipeline::ISOLATION_GUESS,
        tokenade_core::web::pipeline::STATUS_QUESTION,
    );
    readers::note_reader_refusal_for_test("reddit .rss", url, 429, true);
    let served = attach_provenance(&provenance_notes(200, url, Some(url), false), &banner);
    assert!(served.contains(readers::REFUSED_MARKER), "{served}");
    assert!(
        !served.contains(tokenade_core::web::pipeline::ISOLATION_GUESS),
        "the isolation is still blamed:\n{served}"
    );
    assert!(
        !served.contains(tokenade_core::web::pipeline::STATUS_QUESTION),
        "the status question was left unanswered:\n{served}"
    );
    assert_eq!(served.matches("refused, stated above").count(), 2, "{served}");
}

/// The bound: without a refusal, the banner keeps its two sentences — they are
/// right in that case, and that is the whole point of replacing only under a
/// condition.
#[test]
fn a_page_no_reader_claimed_keeps_the_banner_as_written() {
    let banner = format!(
        "[tokenade: thin-result] recovered 0 characters ({}).",
        tokenade_core::web::pipeline::ISOLATION_GUESS
    );
    let served = attach_provenance(&provenance_notes(200, "https://x.test/", None, false), &banner);
    assert!(
        served.contains(tokenade_core::web::pipeline::ISOLATION_GUESS),
        "{served}"
    );
}

/// `--raw` is no longer in the webfetch recover advice (File / recovery_menu
/// had already removed it). `--prompt` / `--grep` / Read remain.
#[test]
fn webfetch_recover_advice_does_not_promote_raw() {
    use crate::hooks::webfetch_pretool::webfetch_recover_advice;
    let a = webfetch_recover_advice(
        "abcdef123456",
        9_000,
        std::path::Path::new("/tmp/stash/abcdef123456.md"),
    );
    assert!(!a.contains("--raw"), "the recover advice must not push the dump: {a}");
    assert!(a.contains("--prompt") && a.contains("--grep"), "the cheap fallbacks are missing: {a}");
    assert!(a.contains("abcdef123456"), "the hash is missing: {a}");
    assert!(a.contains("Read /tmp/stash/abcdef123456.md"), "Read manque : {a}");
}
