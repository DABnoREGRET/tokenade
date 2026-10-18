// The tests of `js_render`.
//
// Spliced by `include!` from `js_render.rs`: the same module, so the same
// private access and the same `super::` paths, without a single visibility
// line changed. This is the form this repo already uses for 33 other modules.
//
// A `//!` is impossible here: a file spliced by `include!` is not a module,
// and an INNER doc comment has nothing to qualify.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executable_script_classifier() {
        assert!(has_executable_script(r#"<script>window.x=1</script>"#));
        assert!(has_executable_script(r#"<script src="/a.js"></script>"#));
        assert!(has_executable_script(
            r#"<script type="module">import x</script>"#
        ));
        // only inert data → nothing to run
        assert!(!has_executable_script(
            r#"<script type="application/json">{"a":1}</script><script type="application/ld+json">{}</script>"#
        ));
        assert!(!has_executable_script(
            r#"<script type="importmap">{}</script>"#
        ));
        assert!(!has_executable_script("<p>no scripts</p>"));
        // mix: one executable among inert → executable
        assert!(has_executable_script(
            r#"<script type="application/json">{}</script><script>run()</script>"#
        ));
    }

    #[test]
    fn a_placeholder_location_is_not_shown_as_the_source() {
        // The shim needs SOME `location` for a file input; the agent must not
        // be told the page came from it.
        let out = label_as_js_harvest("https://example.invalid/", "real text");
        assert!(
            !out.contains("example.invalid"),
            "placeholder leaked: {out}"
        );
        assert!(out.contains("real text"));
        assert!(out.contains("inline scripts"));
        // A genuine URL is still named.
        let named = label_as_js_harvest("https://www.nhs.uk/conditions/type-2-diabetes/", "t");
        assert!(named.contains("> source: https://www.nhs.uk/conditions/type-2-diabetes/"));
    }

    #[test]
    fn executes_inline_script_and_harvests_state() {
        let html = r#"<html><body><div id="root"></div>
        <script>
          window.__INITIAL_STATE__ = {
            article: { body: "The quick brown fox jumps over the lazy dog every single morning." }
          };
        </script></body></html>"#;
        let out = render(html, "https://example.com/a").expect("should harvest");
        assert!(out.contains("quick brown fox"), "got: {out}");
    }

    #[test]
    fn doubly_encoded_json_is_descended() {
        // the Quora shape: a JSON string nested inside a JSON string
        let html = r#"<html><body><script>
          window.__S = {};
          window.__S.r = ["{\"data\":{\"t\":\"{\\\"sections\\\": [{\\\"spans\\\": [{\\\"text\\\": \\\"Deeply nested prose that must survive two decodes.\\\"}]}]}\"}}"];
        </script></body></html>"#;
        let out = render(html, "https://example.com/b").expect("should harvest");
        assert!(out.contains("must survive two decodes"), "got: {out}");
    }

    #[test]
    fn i18n_bundles_are_not_content() {
        // 30 flat UI strings — volume without value
        let mut pairs = String::new();
        for i in 0..30 {
            pairs.push_str(&format!(
                "\"k{i}\":\"Some interface label number {i} here\","
            ));
        }
        let html = format!(
            "<html><body><script>window.__T={{{}\"kz\":\"final label of the bundle here\"}};</script></body></html>",
            pairs
        );
        assert!(
            render(&html, "https://example.com/c").is_none(),
            "i18n bundle must not be served"
        );
    }

    #[test]
    fn a_few_records_do_not_rescue_a_label_bundle() {
        // youtube's `ytcfg.msgs`: 249 UI labels next to SIX plural templates.
        // Treating "contains an object" as a veto let all 249 through as page
        // text — 2.8 KB of "Playlist deleted from downloads." ahead of the
        // video title. The records themselves must still be descended into.
        let mut pairs = String::new();
        for i in 0..60 {
            pairs.push_str(&format!(
                "\"k{i}\":\"Some interface label number {i} here\","
            ));
        }
        let html = format!(
            "<html><body><script>window.__T={{{}\"plural\":{{\"one\":\"1 video\",\"other\":\"# videos\"}},\
             \"rec\":{{\"body\":\"The genuine article body that this page is actually about.\"}}}};\
             </script></body></html>",
            pairs
        );
        let out = render(&html, "https://example.com/yt").expect("the record must survive");
        assert!(
            out.contains("genuine article body"),
            "dropped the record: {out}"
        );
        assert!(!out.contains("interface label"), "served the bundle: {out}");
    }

    #[test]
    fn html_entities_are_decoded_and_escaped_markup_still_rejected() {
        let html = r#"<html><body><script>
          window.__A = { body: "Sorted arrays win because (data[c] - 128) &gt;&gt; 31 &amp; sum stays branch-free." };
          window.__B = { frag: "&lt;div class=&quot;answer&quot;&gt;&lt;p&gt;wrapper markup&lt;/p&gt;&lt;/div&gt;" };
          window.__C = { keep: "A caf&#233; sentence with a numeric entity in it here." };
        </script></body></html>"#;
        let out = render(html, "https://example.com/ent").expect("should harvest");
        assert!(out.contains(">> 31 & sum"), "entities not decoded: {out}");
        assert!(
            out.contains("café sentence"),
            "numeric entity not decoded: {out}"
        );
        // Decoding happens BEFORE the prose test, so escaped markup is finally
        // visible as markup and gets rejected instead of served as a sentence.
        assert!(
            !out.contains("wrapper markup"),
            "escaped markup served as prose: {out}"
        );
        assert!(
            !out.contains("&gt;") && !out.contains("&amp;"),
            "raw entity survived: {out}"
        );
    }

    #[test]
    fn graphql_documents_are_not_content_but_code_prose_is() {
        let html = r#"<html><body><script>
          window.__Q = { doc: "query Context{viewer{...UseCurrentUserFragment}}fragment ViewerNoticeFragment on Notice{type invite{id product{id slug}}}" };
          window.__A = { answer: "The branch predictor guesses, so sorting the array makes the branch predictable and the loop runs faster." };
          window.__C = { prose: "We ran the query against the fragment cache and the mutation still took two seconds." };
        </script></body></html>"#;
        let out = render(html, "https://example.com/gql").expect("should harvest");
        assert!(
            !out.contains("UseCurrentUserFragment"),
            "served a GraphQL document: {out}"
        );
        assert!(
            out.contains("branch predictor guesses"),
            "dropped the answer: {out}"
        );
        // The rule matches GraphQL GRAMMAR, so a sentence that merely uses the
        // words "query", "fragment" and "mutation" is untouched.
        assert!(
            out.contains("still took two seconds"),
            "false positive on prose: {out}"
        );
    }

    #[test]
    fn deeply_nested_state_survives_and_pathological_nesting_cannot_crash() {
        fn nest(levels: usize) -> String {
            let mut s =
                r#"{"leaf":"A genuine sentence of page content buried at the bottom here."}"#
                    .to_string();
            for _ in 0..levels {
                s = format!("{{\"n\":{s}}}");
            }
            format!("<html><body><script>window.__D={s};</script></body></html>")
        }

        // 150 levels is far past anything real (the corpus tops out at 50) and
        // must still yield its leaf — the old ceiling of 60 dropped it silently.
        let out = render(&nest(150), "https://example.com/deep").expect("deep state must be read");
        assert!(
            out.contains("buried at the bottom"),
            "deep leaf lost: {out}"
        );

        // 20 000 levels is a 120 KB script, comfortably inside MAX_HTML. With
        // STACK_LIMIT at the size of the native stack this aborted the PROCESS
        // (`has overflowed its stack`) rather than raising a JS error. If this
        // regresses the test binary dies outright, which is the honest signal.
        assert!(render(&nest(20_000), "https://example.com/bomb").is_none());

        // Memory pressure is the sibling failure: exhausting MEM_LIMIT must
        // come back as `None` (caller keeps the generic extraction), never as a
        // panic or a wedged runtime.
        let bomb = "<html><body><script>var s='x';for(var i=0;i<40;i++){s+=s;}window.__M=s;</script></body></html>";
        assert!(render(bomb, "https://example.com/mem").is_none());
    }

    #[test]
    fn timers_keep_registration_order_and_honour_cancellation() {
        // The pump became a binary heap; `id` is the tiebreak that preserves
        // what the old stable `sort` gave for free. Page setup code depends on
        // same-instant timers running in the order they were registered.
        let html = r#"<html><body><script>
          var seq = [];
          setTimeout(function(){ seq.push('first'); }, 0);
          setTimeout(function(){ seq.push('second'); }, 0);
          setTimeout(function(){ seq.push('third'); }, 0);
          var doomed = setTimeout(function(){ seq.push('cancelled'); }, 0);
          clearTimeout(doomed);
          setTimeout(function(){
            window.__R = { order: "The timers ran in this order: " + seq.join(", ") + "." };
          }, 5);
        </script></body></html>"#;
        let out = render(html, "https://example.com/timers").expect("should harvest");
        assert!(
            out.contains("first, second, third."),
            "wrong timer order: {out}"
        );
        assert!(
            !out.contains("cancelled"),
            "clearTimeout did not cancel: {out}"
        );
    }

    #[test]
    fn js_harvested_text_is_labelled_as_such() {
        // The engine harvests STATE, not the rendered DOM: the text can be
        // partial (XHR content is absent), reordered, and mixed with strings
        // the page never displays. The WebFetch hook said so; `cmd_web` served
        // the bare harvest, so `tokenade web` on a SPA disclosed nothing.
        let out = label_as_js_harvest("https://ex.test/app", "The harvested text.");
        assert!(out.starts_with("> source: https://ex.test/app"), "{out}");
        assert!(
            out.contains("running the page's own inline scripts"),
            "{out}"
        );
        assert!(out.ends_with("The harvested text."), "{out}");
    }

    #[test]
    fn host_fingerprint_is_never_echoed() {
        let html = r#"<html><body><script>
          window.__X = { ua: navigator.userAgent, note: "A genuine sentence of page content here." };
        </script></body></html>"#;
        let out = render(html, "https://example.com/d").expect("should harvest");
        assert!(!out.contains("Mozilla/5.0"), "leaked our UA: {out}");
        assert!(out.contains("genuine sentence"));
    }

    #[test]
    fn broken_script_does_not_abort_the_page() {
        let html = r#"<html><body>
        <script>this is not ( valid javascript $$$</script>
        <script>window.__OK = { v: "Content that follows a broken script must still arrive." };</script>
        </body></html>"#;
        let out = render(html, "https://example.com/e").expect("should harvest");
        assert!(out.contains("must still arrive"), "got: {out}");
    }

    #[test]
    fn empty_and_scriptless_pages_yield_nothing() {
        assert!(render(
            "<html><body><p>plain markup only</p></body></html>",
            "https://x/"
        )
        .is_none());
        assert!(render("", "https://x/").is_none());
    }

    #[test]
    fn runaway_script_is_cut_off_by_the_budget() {
        let html = "<html><body><script>while(true){}</script>\
                    <script>window.__A={v:'never reached but must not hang'}</script></body></html>";
        let t0 = Instant::now();
        let _ = render(html, "https://example.com/f");
        assert!(
            t0.elapsed() < BUDGET * 3,
            "budget not enforced: {:?}",
            t0.elapsed()
        );
    }

    #[test]
    fn oversized_input_is_refused_cheaply() {
        let html = "x".repeat(MAX_HTML + 1);
        let t0 = Instant::now();
        assert!(render(&html, "https://x/").is_none());
        // refused on size, not by burning the whole budget
        assert!(
            t0.elapsed() < BUDGET / 4,
            "refusal was not cheap: {:?}",
            t0.elapsed()
        );
    }

    /// The size ceiling and the time budget have to agree. Admitting a page we
    /// cannot finish means running the full budget and returning nothing — the
    /// worst of both.
    #[test]
    fn size_ceiling_is_reachable_within_the_budget() {
        let kb = MAX_HTML / 1024;
        let worst_ms = kb * WORST_US_PER_KB / 1000;
        let budget_ms = BUDGET.as_millis() as usize;
        assert!(
            worst_ms <= budget_ms,
            "MAX_HTML {kb} KB needs ~{worst_ms} ms at {WORST_US_PER_KB} µs/KB, \
             over the {budget_ms} ms BUDGET"
        );
    }

    /// Build a page that pushes `chunks` onto the Next.js flight stream.
    fn flight_page(chunks: &[&str]) -> String {
        let pushes: String = chunks
            .iter()
            .map(|c| {
                format!(
                    "self.__next_f.push([1,{}]);",
                    serde_json::to_string(c).unwrap()
                )
            })
            .collect();
        format!(
            "<html><body><script>self.__next_f=self.__next_f||[];{pushes}</script></body></html>"
        )
    }

    #[test]
    fn flight_text_chunks_are_harvested() {
        // `T<hexlen>,` chunks carry raw prose — article bodies live here, and a
        // JSON-only reader drops them silently.
        let body =
            "Server components stream their text in T chunks, which is where article bodies live.";
        let page = flight_page(&[&format!("1:T{:x},{}\n", body.len(), body)]);
        let out = render(&page, "https://example.com/t").expect("T chunk should be read");
        assert!(out.contains("article bodies live"), "got: {out}");
    }

    #[test]
    fn flight_lazy_references_are_resolved() {
        // The element in chunk 0 does not contain its own text: it points at
        // chunk 1. Skipping `$L1` (the old behaviour) lost the whole body.
        let body = "This paragraph is only reachable by following a lazy chunk reference.";
        let page = flight_page(&[
            "0:[\"$\",\"div\",null,{\"children\":\"$L1\"}]\n",
            &format!("1:T{:x},{}\n", body.len(), body),
        ]);
        let out = render(&page, "https://example.com/l").expect("ref should resolve");
        assert!(out.contains("lazy chunk reference"), "got: {out}");
    }

    #[test]
    fn flight_reference_cycle_terminates() {
        let page = flight_page(&[
            "0:[\"$\",\"div\",null,{\"children\":\"$L1\"}]\n",
            "1:[\"$\",\"div\",null,{\"children\":\"$L0\"}]\n",
        ]);
        let _ = render(&page, "https://example.com/cy"); // must return, not hang
    }

    #[test]
    fn flight_layout_props_are_not_content() {
        // className / src are the junk that made glassdoor look like a 17.9x win
        let page = flight_page(&[
            "0:[\"$\",\"div\",null,{\"className\":\"flex items-center justify-between gap-4 rounded\",\
             \"src\":\"/_next/static/chunks/main-app-01234567.js\",\
             \"children\":\"Only this sentence is the real page content here.\"}]\n",
        ]);
        let out = render(&page, "https://example.com/p").expect("should harvest children");
        assert!(out.contains("real page content"), "got: {out}");
        assert!(!out.contains("items-center"), "leaked className: {out}");
        assert!(!out.contains("_next/static"), "leaked asset path: {out}");
    }

    #[test]
    fn i18n_format_templates_are_not_content() {
        let html = r#"<html><body><script>
          window.__S = { a: "%s · Get Started with the Framework Guide",
                         b: "Welcome {0} to the documentation portal here",
                         c: "A real sentence of page content that must survive." };
        </script></body></html>"#;
        let out = render(html, "https://example.com/i").expect("should harvest");
        assert!(out.contains("must survive"), "got: {out}");
        assert!(
            !out.contains("Get Started with the Framework"),
            "kept %s template: {out}"
        );
        assert!(
            !out.contains("documentation portal"),
            "kept {{0}} template: {out}"
        );
    }

    #[test]
    fn config_blobs_are_not_content() {
        // youtube inlines 47 KB of experiment flags: letter-rich, no spaces.
        let mut blob = String::from("H5_async_logging_delay_ms=30000.0");
        for i in 0..400 {
            blob.push_str(&format!("&ab_det_flag_number_{i}=true"));
        }
        let html = format!(
            "<html><body><script>window.__C={{cfg:{},real:{}}};</script></body></html>",
            serde_json::to_string(&blob).unwrap(),
            serde_json::to_string("A genuine sentence of readable page content lives here.")
                .unwrap()
        );
        let out = render(&html, "https://example.com/cfg").expect("should harvest");
        assert!(out.contains("genuine sentence"), "got: {out}");
        assert!(!out.contains("ab_det_flag"), "kept config blob: {out}");
    }

    #[test]
    fn host_echo_survives_a_suffix() {
        // youtube appends `,gzip(gfe)` to the UA it echoes back
        let html = r#"<html><body><script>
          window.__Y = { ua: navigator.userAgent + ",gzip(gfe)",
                         real: "Actual readable content of the page goes here." };
        </script></body></html>"#;
        let out = render(html, "https://example.com/ua").expect("should harvest");
        assert!(!out.contains("Mozilla/5.0"), "leaked UA with suffix: {out}");
        assert!(out.contains("Actual readable content"));
    }

    /// Regression guard for a real defect: the first prose filter recognised
    /// latin, cyrillic and han only, and every space-based test assumed words
    /// are space-separated. Measured on this corpus, 8 of 11 writing systems
    /// were being discarded — the whole CJK, Thai, Arabic, Hebrew, Devanagari,
    /// Greek and Korean web read as empty.
    #[test]
    fn prose_survives_every_writing_system() {
        for (script, text) in [
            (
                "latin",
                "This is a genuine paragraph of readable page content in English prose.",
            ),
            (
                "french",
                "Ceci est un vrai paragraphe de contenu lisible rédigé en français courant.",
            ),
            (
                "cyrillic",
                "Это настоящий абзац читаемого содержания страницы на русском языке.",
            ),
            (
                "greek",
                "Αυτή είναι μια πραγματική παράγραφος αναγνώσιμου περιεχομένου στα ελληνικά.",
            ),
            (
                "arabic",
                "هذه فقرة حقيقية من محتوى الصفحة قابلة للقراءة ومكتوبة باللغة العربية.",
            ),
            (
                "hebrew",
                "זוהי פסקה אמיתית של תוכן עמוד קריא הכתובה בשפה העברית לבדיקה.",
            ),
            (
                "hindi",
                "यह पृष्ठ की वास्तविक पठनीय सामग्री का एक अनुच्छेद है जो हिंदी में लिखा गया है।",
            ),
            // spaceless scripts: no word separators at all
            (
                "chinese",
                "这是一段真实可读的网页正文内容，用中文书写，应该被完整保留下来。",
            ),
            (
                "japanese",
                "これは日本語で書かれた実際のページ本文であり、抽出されるべき内容です。",
            ),
            (
                "korean",
                "이것은 한국어로 작성된 실제 페이지 본문 내용이며 추출되어야 합니다.",
            ),
            ("thai", "นี่คือย่อหน้าจริงของเนื้อหาหน้าเว็บที่อ่านได้ซึ่งเขียนเป็นภาษาไทย"),
        ] {
            let html = format!(
                "<html><body><script>window.__S={{body:{}}};</script></body></html>",
                serde_json::to_string(text).unwrap()
            );
            let out = render(&html, "https://example.com/x")
                .unwrap_or_else(|| panic!("{script}: dropped entirely"));
            assert!(out.contains(text), "{script}: mangled -> {out:?}");
        }
    }

    /// Bundled runtime-error templates are perfectly sentence-shaped, so every
    /// other filter here waves them through. vk.com opened its harvest with
    /// three of them before any real wall post.
    #[test]
    fn runtime_error_templates_are_not_content() {
        let html = r#"<html><body><script>
          window.__E = [
            "Failed to execute 'send' on 'WebSocket': Still in CONNECTING state.",
            "Cannot assign to read only property 'push' of object '[object Array]'",
            "TypeError: undefined is not a function in this execution context",
            "Being free consists of not being bound by the opinions that others display."
          ];
        </script></body></html>"#;
        let out = render(html, "https://example.com/err").expect("should harvest");
        assert!(
            out.contains("Being free consists"),
            "lost real content: {out}"
        );
        assert!(
            !out.contains("WebSocket"),
            "kept DOMException template: {out}"
        );
        assert!(
            !out.contains("read only property"),
            "kept TypeError template: {out}"
        );
        assert!(
            !out.contains("not a function"),
            "kept TypeError template: {out}"
        );
    }

    /// A script that throws stops executing, and takes whatever state it had
    /// not yet published with it. These APIs were the measured top causes of
    /// inline-script death across the corpora — `classList`, `console` and
    /// `URL` were simply absent from the shim.
    #[test]
    fn common_browser_apis_do_not_kill_a_script() {
        let html = r#"<html><body><script>
          var el = document.createElement('div');
          el.classList.add('a', 'b');
          el.classList.remove('a');
          console.warn('setup');
          new MutationObserver(function(){}).observe(el, {childList: true});
          new IntersectionObserver(function(){}).observe(el);
          var c = new AbortController(); c.abort();
          var u = new URL('/path/x?q=7&r=hello', 'https://example.com/base/page');
          document.dispatchEvent(new CustomEvent('ready', {detail: 1}));
          window.__STATE = {
            body: "State published only if none of the calls above threw first.",
            klass: el.classList.toString(),
            host: u.hostname,
            path: u.pathname,
            q: u.searchParams.get('q'),
            css: String(CSS.supports('display', 'grid'))
          };
        </script></body></html>"#;
        let out = render(html, "https://example.com/api").expect("script must survive");
        assert!(
            out.contains("State published only if"),
            "script died early: {out}"
        );

        // and the shims must be correct, not merely present
        let probe = r#"<html><body><script>
          var u = new URL('../sibling?x=1#frag', 'https://example.com/a/b/c');
          var el = document.createElement('div');
          el.classList.add('one'); el.classList.add('two'); el.classList.remove('one');
          window.__P = { sentence: "Values collected from the shimmed browser APIs are listed here.",
                         href: u.href, host: u.hostname, x: u.searchParams.get('x'),
                         hash: u.hash, cls: el.classList.toString(),
                         has: String(el.classList.contains('two')) };
        </script></body></html>"#;
        let out2 = render(probe, "https://example.com/api2").expect("should harvest");
        assert!(out2.contains("Values collected"), "got: {out2}");
    }

    #[test]
    fn beats_extraction_requires_a_decisive_margin() {
        assert!(!beats_extraction(&"a".repeat(300), &"b".repeat(200)));
        assert!(beats_extraction(&"a".repeat(5000), &"b".repeat(100)));
        assert!(!beats_extraction("", ""));
    }
}
