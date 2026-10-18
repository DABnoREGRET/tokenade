// The tests of `crate::stealth`, moved out of the production file.
//
// Mounted by `include!`: the tokens are spliced INTO `stealth`, so every
// `super::` keeps its meaning. Split by `srcsweep::production_lines`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloudflare_turnstile_detected() {
        let html = r#"<div class="cf-turnstile" data-sitekey="0x4AAA"></div>"#;
        assert_eq!(
            detect_challenge(html),
            Some(ChallengeKind::CloudflareTurnstile)
        );
    }

    #[test]
    fn cloudflare_interstitial_detected() {
        let html = "<html><body><h1>Just a moment...</h1><script src='/cdn-cgi/challenge-platform/x'></script></body></html>";
        assert_eq!(
            detect_challenge(html),
            Some(ChallengeKind::CloudflareInterstitial)
        );
    }

    /// The interstitial's own orchestrate script is proof on its own — even when
    /// the page is padded out enough to pass the widget hollowness gate.
    #[test]
    fn a_verbose_cloudflare_interstitial_is_detected_without_the_title() {
        let filler = "<p>Checking your browser before accessing the site.</p>".repeat(400);
        let html = format!(
            "<html><body>{filler}<script src='/cdn-cgi/challenge-platform/h/g/orchestrate/chl_page/v1?ray=abc'></script></body></html>"
        );
        assert_eq!(
            detect_challenge(&html),
            Some(ChallengeKind::CloudflareInterstitial)
        );
    }

    /// Neither half of that pair may fire alone: the JS-detection beacon rides
    /// on ordinary 200s under `challenge-platform/**/scripts/jsd/`, and
    /// `/orchestrate/` is an ordinary word in a URL.
    #[test]
    fn the_beacon_and_the_word_orchestrate_are_each_harmless_alone() {
        let article = "<p>Some genuinely long article body about orchestration.</p>".repeat(400);
        let beacon = format!(
            "<html><body>{article}<script src='/cdn-cgi/challenge-platform/scripts/jsd/main.js'></script></body></html>"
        );
        assert_eq!(detect_challenge(&beacon), None);

        let docs = format!(
            "<html><body>{article}<a href='/guides/orchestrate/'>Orchestrate</a></body></html>"
        );
        assert_eq!(detect_challenge(&docs), None);
    }

    #[test]
    fn cloudflare_block_page_is_detected() {
        // Cloudflare error 1020 / firewall block. A different template from the
        // managed challenge: no `cf_chl_opt`, no "Just a moment…", no CF-RAY in
        // the body. 8 of 29 real non-200 bodies captured looked like this.
        let block =
            "<!DOCTYPE html><html><head><title>Attention Required! | Cloudflare</title></head>\
                     <body><h1>Sorry, you have been blocked</h1>\
                     <p>You are unable to access example.com</p></body></html>";
        assert_eq!(
            detect_challenge(block),
            Some(ChallengeKind::CloudflareInterstitial)
        );
    }

    #[test]
    fn a_plain_403_is_not_a_bot_wall() {
        // nginx/IIS "403 Forbidden" and a JSON 401 are refusals, not challenges:
        // there is no stealth remediation that helps, so claiming otherwise
        // sends the agent down a useless path.
        assert_eq!(detect_challenge("<html><head><title>403 Forbidden</title></head><body><center><h1>403 Forbidden</h1></center><hr><center>nginx</center></body></html>"), None);
        assert_eq!(
            detect_challenge("{\"code\":\"401\",\"message\":\"Unauthorized\"}"),
            None
        );
    }

    #[test]
    fn cookie_names_listed_in_a_config_are_not_a_wall() {
        // temu.com ships a cookie-consent manifest that enumerates cookie
        // NAMES. A bare `BIGipServer` / `perimeterx.net` substring match served
        // bot-wall remediation for a working 280 KB retail page.
        let cfg = "<html><body><script>window.__CC={\"SESSIONID\":1,\"cf_clearance\":1,\
                   \"BIGipServerCentinel-Prod-Web\":1,\"ftr_ncd\":1};</script>\
                   <article>Real shop content sits underneath all of that.</article></body></html>";
        assert_eq!(
            detect_challenge(cfg),
            None,
            "a cookie manifest is not a wall"
        );

        // The cookie actually being SET still is.
        let real = "<html><body>Set-Cookie: BIGipServerpool=1234.5678.0000; path=/</body></html>";
        assert_eq!(detect_challenge(real), Some(ChallengeKind::F5BigIp));
    }

    #[test]
    fn perimeterx_sdk_on_a_large_page_is_not_a_wall() {
        // wayfair.com (1.03 MB) and walmart.com (398 KB) both load the HUMAN
        // SDK from perimeterx.net on ordinary pages.
        let page = format!(
            "<html><body><script src=\"https://client.perimeterx.net/abc/main.min.js\"></script>\
             <article>{}</article></body></html>",
            "Ordinary retail copy for a product listing page. ".repeat(60)
        );
        assert_eq!(
            detect_challenge(&page),
            None,
            "PX SDK on a real page is not a wall"
        );
        // The challenge cookie / captcha endpoint still is.
        let blocked = "<html><body>Set-Cookie: _pxhd=abc; <div id=\"px-captcha\"></div>\
                       <script src=\"https://captcha.px-cdn.net/x.js\"></script></body></html>";
        assert_eq!(detect_challenge(blocked), Some(ChallengeKind::PerimeterX));
    }

    #[test]
    fn a_captcha_widget_on_a_real_page_is_not_a_wall() {
        // Measured on the CrUX global top 1 000: pornhub's homepage (~450 KB,
        // 7.5 KB of extracted text) carries `g-recaptcha` on its login form,
        // jutarnji.hr and missav carry `recaptcha/api.js`. All were classified
        // as bot walls, which switched the reader off and prepended stealth
        // remediation to real content.
        let article = format!(
            "<html><body><script src=\"https://www.google.com/recaptcha/api.js\"></script>\
             <div class=\"g-recaptcha\"></div><article>{}</article></body></html>",
            "Real editorial copy that this page is actually made of. ".repeat(60)
        );
        assert_eq!(
            detect_challenge(&article),
            None,
            "captcha on a real page is not a wall"
        );

        // The same widget on a page with nothing else on it still is one.
        let wall = "<html><body><div class=\"g-recaptcha\"></div><p>Verify you are human.</p></body></html>";
        assert_eq!(detect_challenge(wall), Some(ChallengeKind::ReCaptcha));
    }

    #[test]
    fn visible_text_ignores_script_and_style_bodies() {
        let html = "<html><head><style>body{color:red}</style>\
                    <script>var x = 'plenty of characters in here to matter';</script></head>\
                    <body><p>Six words of actual visible text.</p></body></html>";
        // Only "Sixwordsofactualvisibletext." is counted.
        assert_eq!(
            visible_text_len(html),
            28,
            "counted markup or script bodies"
        );
    }

    #[test]
    fn javascript_source_is_not_a_challenge_page() {
        // Verbatim shape of the slice that actually triggered this: a chunk of
        // our own shim.js, which sets the challenge VM's document title. The
        // hook replaced the file content with bot-wall remediation.
        let js = r#"      querySelectorAll: function () { return []; },
      cloneNode: function () { return mkElement(tag); },
    });
    return el;
  }

  // ---- document ------------------------------------------------------------
  var html = mkElement('html');
  var document = EventTargetMix({
    nodeType: 9,
    title: 'Just a moment...',
    readyState: 'loading',
  });
"#;
        assert!(
            looks_like_source_or_doc(js),
            "js source must be recognised as source"
        );
        assert_eq!(
            detect_challenge(js),
            None,
            "js source classified as a challenge: {js}"
        );

        // A real interstitial still opens with HTML structure, which the
        // early-return short-circuits before any of this runs.
        let real =
            "<!DOCTYPE html><html><head><title>Just a moment...</title></head><body></body></html>";
        assert_eq!(
            detect_challenge(real),
            Some(ChallengeKind::CloudflareInterstitial)
        );
    }

    #[test]
    fn cloudflare_beacon_alone_is_not_a_wall() {
        // Bot Management injects this script into ordinary 200 responses. A
        // small page carrying it is a wall; a full article carrying it is an
        // article behind Cloudflare, and calling it a wall switched off the JS
        // reader on 6 of 63 corpus pages.
        let beacon = "<script src='/cdn-cgi/challenge-platform/h/b/scripts/jsd'></script>";
        let wall = format!("<html><body>{beacon}</body></html>");
        assert_eq!(
            detect_challenge(&wall),
            Some(ChallengeKind::CloudflareInterstitial)
        );

        let article = format!(
            "<html><body>{beacon}<article>{}</article></body></html>",
            "Genuine prose that makes this a real page, not a challenge shell. ".repeat(1200)
        );
        assert!(visible_text_len(&article) > WIDGET_MAX_WALL_VISIBLE);
        assert_eq!(
            detect_challenge(&article),
            None,
            "beacon on a full article is not a wall"
        );
    }

    #[test]
    fn cloudflare_challenge_options_object_is_a_wall_at_any_size() {
        // `cf_chl_opt` is emitted by the interstitial itself — crunchbase's
        // real block carried it. It must not be size-gated.
        let html = format!(
            "<html><body><script>window._cf_chl_opt={{cvId:'3'}};</script>{}</body></html>",
            "padding ".repeat(20_000)
        );
        assert_eq!(
            detect_challenge(&html),
            Some(ChallengeKind::CloudflareInterstitial)
        );
    }

    #[test]
    fn hcaptcha_detected() {
        let html = r#"<script src="https://hcaptcha.com/1/api.js"></script>"#;
        assert_eq!(detect_challenge(html), Some(ChallengeKind::HCaptcha));
    }

    #[test]
    fn datadome_detected() {
        let html = "<iframe src='https://geo.captcha-delivery.com/captcha/'></iframe>";
        assert_eq!(detect_challenge(html), Some(ChallengeKind::Datadome));
    }

    #[test]
    fn benign_html_clean() {
        let html = "<p>Just a moment ago I bought some recaptcha cookies.</p>"; // contains the phrase but not the markers
                                                                                // No challenge platform markers, no real recaptcha script — must not false-positive.
        assert_eq!(detect_challenge(html), None);
    }

    #[test]
    fn akamai_real_challenge_with_cookie_assignment_detected() {
        let html = "HTTP/2 403\nSet-Cookie: _abck=ABC123; Path=/; Domain=.example.com\n<html>Access Denied</html>";
        assert_eq!(detect_challenge(html), Some(ChallengeKind::Akamai));
    }

    #[test]
    fn akamai_real_with_sensor_data_post_body() {
        let html = "<form><input name=\"sensor_data\" value=\"...\"></form><script src=\"//x.akamaihd.net/a.js\"></script>";
        assert_eq!(detect_challenge(html), Some(ChallengeKind::Akamai));
    }

    #[test]
    fn cf_turnstile_in_prose_not_detected() {
        // Same anti-false-positive pattern as Akamai.
        let prose = "We use cf-turnstile for bot protection — see ChallengeKind::CloudflareTurnstile in stealth.rs.";
        assert_eq!(detect_challenge(prose), None);
    }

    #[test]
    fn akamai_changelog_prose_mentioning_abck_and_sensor_data_not_detected() {
        // The CHANGELOG of tokenade itself mentions `_abck` and
        // `sensor_data` as docs. Without structural evidence
        // (`_abck=` cookie OR `sensor_data=` POST field) this must
        // NOT trigger — false-positive observed 2026-05-29.
        let prose = "Akamai Bot Manager detected (`_abck` + sensor_data). Same TLS-fingerprint constraint as Cloudflare Bot Management.";
        assert_eq!(detect_challenge(prose), None);
    }

    /// Akamai's deny page is ALL-UPPERCASE markup, so the case-sensitive
    /// `<title>Access Denied` needle never fired — homedepot.com came back
    /// refused with no challenge label at all. Note `edgesuite` and NOT
    /// `edgesuite.net`: Akamai entity-encodes the dots.
    #[test]
    fn akamai_uppercase_deny_page_is_detected() {
        let wall = "<HTML><HEAD>\n<TITLE>Access Denied</TITLE>\n</HEAD><BODY>\n<H1>Access Denied</H1>\n \nYou don't have permission to access \"http&#58;&#47;&#47;www&#46;example&#46;com&#47;\" on this server.<P>\nReference&#32;&#35;18&#46;978f1002\n<P>https&#58;&#47;&#47;errors&#46;edgesuite&#46;net&#47;18&#46;978f1002</P>\n</BODY>\n</HTML>\n";
        assert_eq!(detect_challenge(wall), Some(ChallengeKind::Akamai));
    }

    /// A bare "Access denied" title names NO vendor. affordablegolf.co.uk
    /// serves a multilingual `data-i18n` deny page with no Akamai marker;
    /// labelling it `akamai-bot-manager` would assert a provider on no
    /// evidence — precisely what `GenericBlock` exists to avoid.
    #[test]
    fn a_bare_deny_title_is_not_attributed_to_akamai() {
        let generic = "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<title>Access denied</title>\n</head>\n<body>\n<h1 data-i18n=\"content-title\">You do not have permission to access this website</h1>\n<p data-i18n=\"access-denied-desc\">Your request was blocked.</p>\n</body></html>";
        assert_ne!(detect_challenge(generic), Some(ChallengeKind::Akamai));
    }

    #[test]
    fn remediation_tells_the_caller_what_to_do() {
        // Was `remediation_contains_three_tiers`, pinning the names of the
        // libraries a stealth fetcher is built from. Those were acted on zero
        // times in eleven advisories; what the caller can always do is hand
        // tokenade a page saved from a browser.
        let r = stealth_remediation(ChallengeKind::CloudflareTurnstile);
        assert!(r.contains("tokenade web saved.html"), "{r}");
        assert!(r.contains("immediate retry"), "{r}");
        assert!(r.contains("real-Chrome stealth fetch"), "{r}");
    }
}
#[cfg(test)]
mod hollow_response_tests {
    use super::*;

    /// The real imdb.com / espn.com response, trimmed. Served as HTTP 202.
    const AWS_WAF_SHELL: &str = r#"<!DOCTYPE html><html lang="en"><head><meta charset="utf-8">
<title></title><script type="text/javascript">
window.awsWafCookieDomainList = ['imdb.com'];
window.gokuProps = {"key":"AQIDAH...","iv":"CgAGUDIJpwAAC2C6","context":"nUxT..."};
</script><script src="https://c07df3ce.token.awswaf.com/challenge.js"></script>
</head><body><div id="challenge-container"></div></body></html>"#;

    #[test]
    fn perimeterx_first_party_snippet_is_a_widget_not_a_verdict() {
        // sams.com.mx (CrUX rank 10 000) ships PerimeterX's FIRST-PARTY
        // snippet: no `px-cdn.net` host, no `_px*` cookie in the HTML, so the
        // response markers never fired and the wall was delivered as content
        // reading "Verify Your Identity / Mantén presionado el botón".
        let wall = r#"<html><head><script>window._pxAppId='PXjJ0cYtn9';window._pxJsClientSrc='/JZKoZFMs/init.js';</script></head><body><h1>Verifica tu identidad</h1><p>Mantén presionado el botón.</p></body></html>"#;
        assert_eq!(detect_challenge(wall), Some(ChallengeKind::PerimeterX));

        // The SAME snippet ships on ordinary retail pages — the file's own
        // history records wayfair and walmart being served bot-wall
        // remediation for working pages. A page with real content must stay
        // clean; that is what the widget corroboration is for.
        let mut page = String::from(
            r#"<html><head><script>window._pxAppId='PXjJ0cYtn9';</script></head><body><h1>Sofa, 3 seater</h1>"#,
        );
        for i in 0..80 {
            page.push_str(&format!(
                "<p>Product detail paragraph {i} describing the upholstery, the frame construction and the delivery window in ordinary retail prose.</p>"
            ));
        }
        page.push_str("</body></html>");
        assert_eq!(
            detect_challenge(&page),
            None,
            "a content page carrying the snippet must not be called a wall"
        );
    }

    #[test]
    fn aws_waf_shell_is_named_not_generic() {
        // Before: no marker existed, every tier came back empty and
        // `tokenade web https://www.imdb.com/` returned a bare newline.
        assert_eq!(detect_challenge(AWS_WAF_SHELL), Some(ChallengeKind::AwsWaf));
    }

    #[test]
    fn hollow_shell_reaches_the_compactor_but_not_the_js_engine() {
        // The sibling-caller invariant. `js_render::render` bails out on
        // whatever `detect_and_remediate` reports, and a hollow shell is the
        // engine's PRIMARY target (quora / airbnb / x.com all distil to
        // nothing before their inline scripts run). Widening the shared
        // detector would have made the engine refuse exactly the pages it
        // wins on, so the structural tier must stay out of this call.
        let shell = "<!DOCTYPE html><html><head><script>var x=1;</script></head>\
                     <body><div id=root></div></body></html>";
        assert_eq!(
            detect_and_remediate(shell).map(|(k, _)| k),
            None,
            "named-only detector must stay quiet so the JS engine still runs"
        );
        assert_eq!(
            detect_and_remediate_any(shell).map(|(k, _)| k),
            Some(ChallengeKind::GenericBlock),
            "the compactor must explain the empty result rather than emit nothing"
        );
    }

    #[test]
    fn a_human_check_page_is_not_delivered_as_a_thin_answer() {
        // The corpus' ddg_search.html shape: a bot check whose few links
        // distilled to `[DuckDuckGo](/html/)` — 21 bytes that read as a real,
        // thin result (MEASURED 2026-07-22).
        let html = "<html><body><h1>DuckDuckGo</h1><p>Unfortunately, bots use DuckDuckGo \
            too. Please complete the following challenge to confirm this search was made by a \
            human. Select all squares containing a duck:</p><a href=\"/html/\">DuckDuckGo</a>\
            </body></html>";
        assert_eq!(detect_challenge(html), Some(ChallengeKind::HumanCheck));
        // Second member of the same family, same mechanism: codeberg's
        // interstitial, served on /explore/repos and on source-file paths,
        // reached the agent as 259 bytes of page (MEASURED 2026-07-22).
        let cookie_monster = "<html><body><h1>Cookie monster!</h1><p>This is an automated \
            check to get rid of most bots. If you have JavaScript enabled, it should redirect \
            to the real page soon.</p></body></html>";
        assert_eq!(
            detect_challenge(cookie_monster),
            Some(ChallengeKind::HumanCheck)
        );
        // A long article ABOUT bot checks must not match: the phrase is bounded
        // by length, and that guard is what makes matching prose safe here.
        let article = format!(
            "<html><body><article><h1>How bot checks work</h1><p>{}</p>\
             <p>Some sites ask you to confirm this search was made by a human.</p>\
             </article></body></html>",
            "Detailed prose about anti-bot systems and their history. ".repeat(60)
        );
        assert_eq!(detect_challenge(&article), None);
    }

    #[test]
    fn client_challenge_shell_is_disclosed_not_served_as_content() {
        // The exact shape pypi.org/search served (MEASURED 2026-07-22): a
        // well-formed document — <body>, <p>, plain prose — so the hollowness
        // tier cannot see it and it was delivered as if it were the results.
        let html = "<html><head><title>Client Challenge</title></head><body>\
            <p>A required part of this site couldn\u{2019}t load. This may be due to a browser \
            extension, network issues, or browser settings.</p><script>x()</script></body></html>";
        assert_eq!(
            detect_challenge(html),
            Some(ChallengeKind::ClientChallengeShell)
        );
        // A straight-quote copy of the same shell must match too.
        assert_eq!(
            detect_challenge(&html.replace('\u{2019}', "'")),
            Some(ChallengeKind::ClientChallengeShell)
        );
        // Both markers are required: the title alone is a phrase a real page
        // could discuss.
        let article = "<html><head><title>Client Challenge</title></head><body>\
            <p>This post explains how a client challenge works and why CDNs use one.</p>\
            <script>x()</script></body></html>";
        assert_ne!(
            detect_challenge(article),
            Some(ChallengeKind::ClientChallengeShell)
        );
        // The message must not describe the page as structurally empty — it
        // is not, and that was the whole reason for a separate kind.
        let msg = stealth_remediation(ChallengeKind::ClientChallengeShell);
        assert!(
            !msg.contains("no body text"),
            "inherited the hollow-shell wording: {msg}"
        );
        for vendor in ["Cloudflare", "Datadome", "Akamai", "Imperva", "PerimeterX"] {
            assert!(
                !msg.contains(vendor),
                "must not guess a vendor ({vendor}): {msg}"
            );
        }
    }

    #[test]
    fn generic_block_does_not_name_a_vendor() {
        // Naming Cloudflare on a page with no Cloudflare evidence is a claim
        // the response does not support — the hollow shape is equally
        // consistent with a JS-only page.
        let msg = stealth_remediation(ChallengeKind::GenericBlock);
        for vendor in [
            "Cloudflare",
            "Datadome",
            "Akamai",
            "PerimeterX",
            "Patchright",
        ] {
            assert!(
                !msg.contains(vendor),
                "generic-block remediation must not name {vendor}"
            );
        }
    }

    #[test]
    fn plain_text_is_not_a_blocked_page() {
        // Regression: the three hollowness signals all fire on ordinary
        // prose (no `<body>`, no semantic tags), so without the HTML-document
        // gate every short text fragment was served a bot-wall banner —
        // caught by `web_html_never_panics`' inflation bound.
        for s in [
            "hello world",
            "",
            "error: connection refused",
            "{\"ok\":true}",
            "# Changelog\n\nFixed a bug.",
        ] {
            assert_eq!(structural_block_signal(s), None, "false positive on {s:?}");
        }
    }

    #[test]
    fn hollow_page_without_scripts_is_not_flagged() {
        // A script-free stub is a stub, not a wall: both things this tier
        // targets load JS.
        let stub =
            "<!DOCTYPE html><html><head><title>x</title></head><body><div></div></body></html>";
        assert_eq!(structural_block_signal(stub), None);
    }
}
#[cfg(test)]
mod source_guard_tests {
    use super::*;

    #[test]
    fn detects_rust_source() {
        let src = "//! Some doc comment\n//!\nuse crate::tokenizer;\n\npub fn foo() {}\n";
        assert!(looks_like_source_or_doc(src));
    }

    #[test]
    fn detects_markdown() {
        let md = "# Title\n\n## Section\n\n- bullet\n- bullet\n";
        assert!(looks_like_source_or_doc(md));
    }

    #[test]
    fn detects_grep_output() {
        let g = "./src/main.rs:42:    if check\n./src/lib.rs:99:    println!\n";
        assert!(looks_like_source_or_doc(g));
    }

    #[test]
    fn line_numbered_source_is_still_source() {
        // The exact blob that reached production (stash `fc76ad88160a`): a
        // `Read`/`cat -n` excerpt of THIS file, numbers then a tab. Its content
        // was replaced by `[tokenade: challenge=reblaze]` remediation.
        let excerpt = "50\t    /// Reblaze — Israeli bot-management CDN, identified by `rbzid`\n\
                       51\t    /// cookies and `__uzm[abcd]` session markers.\n\
                       52\t    Reblaze,\n";
        assert!(
            looks_like_source_or_doc(excerpt),
            "line-numbered source must be recognised through the number"
        );
        assert!(
            detect_challenge(excerpt).is_none(),
            "an excerpt of our own detector must not be read as a bot wall"
        );
        // `grep -n` without a filename: same shape, colon instead of tab.
        let grepped = "105:    // Cloudflare \"Just a moment...\" page.\n\
                       113:    // hCaptcha widget marker.\n";
        assert!(looks_like_source_or_doc(grepped));
        assert!(detect_challenge(grepped).is_none());
    }

    #[test]
    fn a_leading_number_alone_does_not_make_prose_source() {
        // The strip must not hand out signal for free: without a source marker
        // AFTER the number, nothing fires. Otherwise any numbered list would
        // suppress detection on a real wall.
        assert!(!looks_like_source_or_doc(
            "1: we were unable to verify\n2: please enable cookies\n"
        ));
        // And the digits must be bound to a separator — `2026 was the year`
        // is prose, not line 2026.
        assert!(!looks_like_source_or_doc(
            "2026 # not a heading\n2027 # nor this\n"
        ));
    }

    #[test]
    fn html_passes_through() {
        let html = "<!DOCTYPE html><html><head></head><body></body></html>";
        assert!(!looks_like_source_or_doc(html));
    }

    #[test]
    fn challenge_detector_bypasses_source() {
        // Source mentioning a brand literal must NOT trigger the detector.
        let src = "//! Stealth detection\n//!\nuse crate::tokenizer;\n\npub fn foo() {\n    // challenges.cloudflare.com/turnstile\n}\n";
        assert!(
            detect_challenge(src).is_none(),
            "source code naming the brand should not false-positive"
        );
    }

    #[test]
    fn challenge_detector_still_catches_real_html() {
        let html = "<!DOCTYPE html><html><head>\n<script src=\"https://challenges.cloudflare.com/turnstile/v0/api.js\"></script>\n</head></html>";
        assert!(
            detect_challenge(html).is_some(),
            "real challenge HTML must still be detected"
        );
    }

    #[test]
    fn detects_grep_output_with_absolute_paths() {
        // Regression: production showed 20 false-positive auto:challenge
        // events on `grep -rn` output that uses ABSOLUTE paths. The
        // previous `./` prefix check excluded these.
        let g = "/home/username/projects/tokenade/crates/tokenade-core/src/stealth.rs:82:pub fn detect_challenge\n/home/u/foo/bar.rs:99:    use crate::cloudflare\n";
        assert!(
            looks_like_source_or_doc(g),
            "grep output with absolute paths must be recognised as source"
        );
    }

    #[test]
    fn detects_grep_output_with_relative_paths() {
        // MEASURED 2026-09-07 through `tokenade wrap`: this exact command,
        //   grep -rn <vendor> --include=*.rs crates/ | grep -v '^\s*//'
        // returned seven matches in 657 bytes and was served as 559 bytes of
        // bot-wall remediation carrying NONE of them. `grep -rn <dir>` run
        // from a repository root emits a RELATIVE path, and the rule beside
        // this one only ever recognised `./foo` and `/foo`.
        //
        // Every line here is the relative form. The sibling test above mixes
        // one relative line with one absolute line, so it passed on the
        // strength of the absolute one alone and proved nothing about this.
        let g = "crates/tokenade-core/src/stealth.rs:213:pub fn detect_challenge\n                 crates/tokenade-core/src/stealth.rs:266:    Datadome,\n                 crates/tokenade-bench/src/main.rs:88:    let k = Datadome;\n";
        assert!(
            looks_like_source_or_doc(g),
            "grep output with relative paths must be recognised as source"
        );
        assert!(
            detect_challenge(g).is_none(),
            "an excerpt of our own detector must not be read as a bot wall"
        );

        // Anti-vacuity. A false positive HERE suppresses a real detection, so
        // the rule must not hand out signal to anything that merely holds a
        // colon and a digit.
        assert!(
            !looks_like_source_or_doc("see 2: enable cookies\nthen 3: retry\n"),
            "prose with numbered steps is not a grep locator"
        );
        assert!(
            !looks_like_source_or_doc(
                "we could not verify: 12: your browser\nplease wait: 34: a moment\n"
            ),
            "a locator needs a path, not just a colon and a number"
        );
        // Markup must not reach the path field. The field has to carry a `/`
        // and be followed by `<number>:`, or a clause OTHER than this one
        // refuses and the arm proves nothing: in `<script src="https://a/b">`
        // the FIRST colon is the one in `https:`, so the path field is
        // `<script src="https`, which holds no separator. Measured: with that
        // fixture, dropping the markup check survived.
        assert!(
            !looks_like_source_or_doc("<a href=b/c>:12:x\n<img alt=d/e>:13:y\n"),
            "a markup line must not be read as a locator"
        );
        // Same for a field carrying a space, which a grep locator never does.
        assert!(
            !looks_like_source_or_doc("my file/name:12:x\nmy other/name:13:y\n"),
            "a field with a space is not a grep locator"
        );
        // Each clause below is the ONLY one that can refuse its arm, so a
        // mutation dropping any single clause has somewhere to die.
        // The path field must hold a separator: everything else here passes.
        assert!(
            !looks_like_source_or_doc("verify:12:enable cookies\nretry:34:please wait\n"),
            "a bare word is not a path"
        );
        // The line number must be followed by a second colon.
        assert!(
            !looks_like_source_or_doc("assets/img:2024 was a good year\nassets/vid:2025 also\n"),
            "a path and a year is not a locator"
        );
        // There must BE a line number.
        assert!(
            !looks_like_source_or_doc("a/b::enable cookies\nc/d::please wait\n"),
            "two colons with no number is not a locator"
        );

        // And the real wall is still a wall.
        let html = "<!DOCTYPE html><html><head>\n<script src=\"https://challenges.cloudflare.com/turnstile/v0/api.js\"></script>\n</head></html>";
        assert!(
            detect_challenge(html).is_some(),
            "real challenge HTML must still be detected"
        );
    }

    #[test]
    fn detects_single_header_markdown() {
        // Regression: `cat CHANGELOG.md | head -5` produces text that
        // starts with a single `# ` header + prose mentioning brand
        // names like "cloudflare" in release notes. The previous
        // 2-hit-minimum threshold missed this → false-positive
        // challenge detection → +134t inflation per event.
        let md = "# Changelog\n\nAll notable changes. Following [Keep a Changelog](https://keepachangelog.com/).\n\n## [0.2.0]\n\nAdded cloudflare bypass detector.\n";
        assert!(
            looks_like_source_or_doc(md),
            "markdown starting with a single h1 must still be recognised"
        );
    }

    #[test]
    fn challenge_detector_bypasses_grep_brand_search() {
        // The exact production false-positive: agent runs
        // `grep -rn "challenge\|cloudflare\|akamai" /path/to/src`.
        // Output contains brand names but is grep results, not real
        // challenge HTML.
        let grep_out = "\
/home/u/proj/src/stealth.rs:42:    // detect cloudflare challenge pages
/home/u/proj/src/stealth.rs:85:    if text.contains(\"_abck\") { /* akamai */ }
/home/u/proj/src/stealth.rs:120:    pub fn detect_challenge(text: &str)\n";
        assert!(
            detect_challenge(grep_out).is_none(),
            "grep results mentioning brand names must NOT trigger challenge detection"
        );
    }
}
#[cfg(test)]
mod remediation_is_actionable_by_the_caller {
    use super::*;

    /// Every vendor kind that appends the shared advice must carry the two
    /// things the caller can act on. Measured over our own transcripts: of
    /// eleven advisories served, the previous text (three tiers naming
    /// Patchright, playwright-stealth and a CDP init script) was acted on
    /// zero times, and three were followed by a retry of the same URL that
    /// no line of the message ruled out.
    #[test]
    fn every_vendor_kind_names_the_path_that_needs_no_browser() {
        for kind in [
            ChallengeKind::CloudflareTurnstile,
            ChallengeKind::CloudflareInterstitial,
            ChallengeKind::HCaptcha,
            ChallengeKind::ReCaptcha,
            ChallengeKind::Datadome,
            ChallengeKind::Akamai,
        ] {
            let md = stealth_remediation(kind);
            assert!(
                md.contains("tokenade web saved.html"),
                "{kind:?} must name the saved-page path:\n{md}"
            );
            assert!(
                md.contains("immediate retry"),
                "{kind:?} must rule out a plain retry:\n{md}"
            );
            assert!(
                md.contains(&format!("challenge={}", kind.as_str())),
                "{kind:?} must still name itself:\n{md}"
            );
        }
    }

    /// The kinds that return EARLY still must not grow the shared advice —
    /// each already says what to do, and the comment on each says the vendor
    /// advice "would be noise" for it.
    #[test]
    fn the_early_returning_kinds_do_not_get_the_shared_advice() {
        for kind in [
            ChallengeKind::HumanCheck,
            ChallengeKind::ClientChallengeShell,
            ChallengeKind::GenericBlock,
        ] {
            let md = stealth_remediation(kind);
            assert!(
                !md.contains("real-Chrome stealth fetch"),
                "{kind:?} must keep its own wording:\n{md}"
            );
            assert!(
                md.contains("challenge="),
                "{kind:?} must still name itself:\n{md}"
            );
        }
    }
}
#[cfg(test)]
mod line_numbered_source_slice_tests {
    use super::*;

    /// A line-numbered slice from the MIDDLE of this file must not read as a
    /// bot wall.
    ///
    /// FIELD REPRODUCTION, 2026-08-05: `awk 'NR>=340&&NR<=372; NR>=400&&NR<=432'
    /// stealth.rs` through the bash hook came back as challenge remediation
    /// instead of the 66 lines of code — and `cat` of the identical bytes did
    /// the same, so it is the CONTENT, not the command. The slice is the
    /// densest run of challenge literals in the file and carries none of the
    /// signals the guard looks for: no doc comment, no `use`, no `fn` — it
    /// opens on `"Please complete the following challenge",` and stays inside a
    /// function body all the way down.
    ///
    /// The guard in THIS tree already answers it (this test passes). What
    /// replaced the output was the SHIPPED 1.1.6 binary the hook runs, which
    /// predates the fix. Kept as the regression fixture: real bytes, taken from
    /// the file that trips itself.
    #[test]
    fn a_numbered_slice_of_this_files_middle_is_not_a_wall() {
        // The FAMILY, and the densest window COMPUTED rather than two
        // hand-written line ranges: the challenge markers moved down into
        // `stealth/detect.rs` on 2026-09-13, and frozen line numbers would
        // have numbered emptiness without anything saying so.
        let fam = tk_base::source_guard::library_module_family("stealth");
        let src: Vec<&str> = fam.lines().collect();
        const WIN: usize = 66;
        let literals = |w: &[&str]| w.iter().filter(|l| l.contains('"')).count();
        let start = (0..src.len().saturating_sub(WIN))
            .max_by_key(|&i| literals(&src[i..i + WIN]))
            .unwrap_or(0);
        let mut slice = String::new();
        for (n, l) in src[start..start + WIN].iter().enumerate() {
            slice.push_str(&format!("{}|{l}\n", start + 1 + n));
        }
        // Without this floor, an empty window would pass for a non-detection.
        assert!(
            // MEASURED on 2026-09-13: 26 lines with a literal in the best
            // window. The floor sits below the measurement, not on it.
            literals(&src[start..start + WIN]) >= 20,
            "the densest window carries only {} line(s) with a literal: the \
             fixture no longer looks like what it claims to reproduce",
            literals(&src[start..start + WIN])
        );
        assert_eq!(
            detect_challenge(&slice),
            None,
            "a numbered slice of our own detector read as a bot wall"
        );
    }
}
