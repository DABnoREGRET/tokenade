//! **The HOSTILE inputs whose family has no module of its own.**
//!
//! An unreadable PDF, a decompression bomb in a log, a Cloudflare
//! challenge, injected script, a notebook stuffed with base64. They used to live
//! scattered across `main.rs`; they are here until each one joins
//! the family whose compactor it tests.
//!
//! What is measured is NOT compaction: it is that nothing panics, that
//! nothing executes, and that the output says what it could not read.

use super::super::*;

// Adversarial #20: a non-PDF blob with a `.pdf` extension. Must NOT panic.
// The contract: extractor returns within 1 s, output is anything (empty or
// best-effort), and the process is alive — the `catch_unwind` guard is what
// we're really testing.
pub(crate) fn adv_pdf_garbage_input(results: &mut Vec<RunResult>) {
    let bytes = br#"{"this": "is not actually a PDF", "id": 42}"#;
    let t = Instant::now();
    let (text, _sav) = docs::extract_with_savings(bytes, docs::Format::Pdf);
    let ms = t.elapsed().as_millis();
    let quality = if ms < 1000 { 1.0 } else { 0.0 };
    record(
        results,
        "adv:pdf:garbage-input",
        "pdf-extract",
        "adversarial",
        "junk-pretending-to-be-pdf",
        &text,
        ms,
        quality,
    );
}

// ---- Iter #9: curl -v + generic logs ------------------------------------

// Adversarial #17: a 5000-line TLS handshake bomb must compress > 95 % AND
// keep both request line + response status line + body byte-for-byte.
pub(crate) fn adv_logs_tls_bomb(results: &mut Vec<RunResult>) {
    let mut t = String::from("* Connected to a.b (1.1.1.1) port 443\n");
    for i in 0..5000 {
        t.push_str(&format!(
            "* TLSv1.3 (IN), TLS handshake, Encrypted Extensions ({i}):\n"
        ));
    }
    t.push_str("> POST /v1/orders HTTP/2\n> Host: a.b\n> content-type: application/json\n>\n{\"item\":\"keep_me_intact\"}\n< HTTP/2 201\n< location: /v1/orders/42\n{\"id\":42,\"status\":\"created\"}\n");
    let st = Instant::now();
    let (out, sav) = tokenade_logs::compact_curl_verbose_with_savings(&t);
    let ms = st.elapsed().as_millis();
    let quality = if out.contains("POST /v1/orders HTTP/2")
        && out.contains("HTTP/2 201")
        && out.contains("keep_me_intact")
        && out.contains("\"id\":42")
        && sav.percent() > 95.0
        && ms < 500
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:logs:tls-handshake-bomb",
        "logs",
        "adversarial",
        &t,
        &out,
        ms,
        quality,
    );
}

// Adversarial #18: 10 000 identical "ERROR timeout" lines with rotating
// timestamps (so simple `consecutive identical` dedup fails) must fold to a
// single representative line + `(×10000)`, no message lost.
pub(crate) fn adv_logs_repeated_template(results: &mut Vec<RunResult>) {
    let mut t = String::new();
    for i in 0..10_000 {
        // Identical message, but varying TS (each second) and request-id hex.
        t.push_str(&format!(
            "2024-06-01T12:34:{:02}.123456Z req=abcdef0123456789 ERROR timeout connecting to db at upstream\n",
            i % 60
        ));
    }
    // Add one distinct line at the end to ensure it is not swallowed.
    t.push_str("2024-06-01T12:34:59.999999Z INFO recovery complete\n");
    let st = Instant::now();
    let (out, sav) = tokenade_logs::compact_generic_logs_with_savings(&t);
    let ms = st.elapsed().as_millis();
    let folded = out.contains("(×10000)") || out.contains("(×9999)") || out.contains("(×10001)");
    let distinct_kept = out.contains("recovery complete");
    let quality = if folded && distinct_kept && sav.percent() > 99.0 && ms < 500 {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:logs:repeated-template",
        "logs",
        "adversarial",
        &t,
        &out,
        ms,
        quality,
    );
}

// ---- Iter #8: YAML / kubectl compaction ---------------------------------

// ---- Iter #7: stack-trace compaction ------------------------------------

// ---- Iter #6: stealth, CSV, docs ----------------------------------------

// Adversarial #10: a Cloudflare Turnstile challenge page must be recognised
// (returning the kind + remediation), NOT misread as article content.
pub(crate) fn adv_web_cloudflare_challenge(results: &mut Vec<RunResult>) {
    let html = r#"<!doctype html>
<html><head><title>Just a moment...</title>
<script src="/cdn-cgi/challenge-platform/h/g/orchestrate/jsch/v1"></script></head>
<body><div class="cf-turnstile" data-sitekey="0x4AAA"></div>
<noscript>Please enable JavaScript</noscript></body></html>"#;
    let t = Instant::now();
    let md = web::html_to_markdown(html);
    let ms = t.elapsed().as_millis();
    let kind_marker = md.contains("challenge=cloudflare-turnstile");
    // Scored on whether the advice is ACTIONABLE by the caller, not on
    // whether it names the libraries a stealth fetcher is built from.
    let actionable = md.contains("tokenade web saved.html") && md.contains("immediate retry");
    let quality = if kind_marker && actionable { 1.0 } else { 0.0 };
    record(
        results,
        "adv:web:cloudflare-challenge",
        "stealth",
        "adversarial",
        html,
        &md,
        ms,
        quality,
    );
}

// Adversarial #11: every detector must fire on its canonical fingerprint
// AND the fully benign sample must remain clean.
pub(crate) fn adv_stealth_all_kinds(results: &mut Vec<RunResult>) {
    let cases = [
        (
            "turnstile",
            r#"<div class="cf-turnstile" data-sitekey="x"></div>"#,
            true,
        ),
        (
            "interstitial",
            "<h1>Just a moment...</h1><script src='/cdn-cgi/challenge-platform/h/x'></script>",
            true,
        ),
        (
            "hcaptcha",
            r#"<script src="https://hcaptcha.com/1/api.js"></script>"#,
            true,
        ),
        (
            "recaptcha",
            r#"<script src="https://google.com/recaptcha/api.js"></script>"#,
            true,
        ),
        (
            "datadome",
            "<iframe src='https://geo.captcha-delivery.com/captcha/'></iframe>",
            true,
        ),
        (
            "benign",
            "<html><body><p>nothing to see here</p></body></html>",
            false,
        ),
    ];
    let mut hits = 0;
    let mut total = 0;
    for (_name, sample, expect_detected) in &cases {
        total += 1;
        let detected = stealth::detect_challenge(sample).is_some();
        if detected == *expect_detected {
            hits += 1;
        }
    }
    let quality = hits as f64 / total as f64;
    record(
        results,
        "adv:stealth:detector-coverage",
        "stealth",
        "adversarial",
        "",
        "",
        0,
        quality,
    );
}

// Real task: compact a synthetic-but-realistic CSV.
pub(crate) fn task_csv(results: &mut Vec<RunResult>) {
    // 200 rows × 8 columns, 3 columns fully empty, several long cells.
    let mut raw = String::from("id,name,empty_a,description,empty_b,score,empty_c,notes\n");
    for i in 0..200 {
        raw.push_str(&format!(
            "{i},user_{i},,\"{}\",,{:.2},,n/a\n",
            "the quick brown fox jumps over the lazy dog ".repeat(5),
            i as f64 / 7.0
        ));
    }
    let before = tokenizer::estimate(&raw);
    let t = Instant::now();
    let out = tokenade_csv::compact_csv(&raw);
    let ms = t.elapsed().as_millis();
    let after = tokenizer::estimate(&out);
    // Quality: header preserved, ≥1 data row preserved, "more rows elided" marker present.
    let quality = if out.contains("id,name") && out.contains("user_0") && out.contains("more rows")
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:csv-compact",
        "csv-compact",
        "synthetic-200x8",
        &raw,
        &out,
        ms,
        quality,
    );
    let _ = (before, after); // tokens captured in record()
}

// Adversarial #12: docx extraction from synthetic in-memory ZIP. The contract
// is *information preservation*: the extracted text must contain every
// occurrence of the payload phrase. Savings here come from bytes→tokens,
// recorded via `extract_with_savings`.
pub(crate) fn adv_docs_docx_roundtrip(results: &mut Vec<RunResult>) {
    use zip_helper::build_docx;
    let payload = "The quick brown fox jumps over the lazy dog. ".repeat(40);
    let bytes = build_docx(&payload);
    let t = Instant::now();
    let (text, _sav) = docs::extract_with_savings(&bytes, docs::Format::Docx);
    let ms = t.elapsed().as_millis();
    let occurrences = text.matches("quick brown fox").count();
    let quality = if occurrences == 40 { 1.0 } else { 0.0 };
    // For the bench row, encode `before` as bytes/4 (same as docs module).
    let before_pseudo = "x".repeat(bytes.len().div_ceil(4));
    record(
        results,
        "adv:docs:docx-roundtrip",
        "docs-extract",
        "adversarial",
        &before_pseudo,
        &text,
        ms,
        quality,
    );
}

// Tiny in-test helper to synthesise a .docx zip.
mod zip_helper {
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;
    pub fn build_docx(payload: &str) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut w = ZipWriter::new(Cursor::new(&mut buf));
            let opts: SimpleFileOptions = Default::default();
            w.start_file("word/document.xml", opts).unwrap();
            let xml = format!(
                r#"<?xml version="1.0"?><w:document xmlns:w="x"><w:body><w:p><w:r><w:t>{payload}</w:t></w:r></w:p></w:body></w:document>"#
            );
            w.write_all(xml.as_bytes()).unwrap();
            w.finish().unwrap();
        }
        buf
    }
}

// ---- Notebook tasks (iter #5) --------------------------------------------

pub(crate) fn task_notebook(results: &mut Vec<RunResult>, fixtures: &Path, fname: &str) {
    let path = fixtures.join("notebook").join(fname);
    let Ok(raw) = fs::read_to_string(&path) else {
        return;
    };
    if raw.len() < 500 {
        return;
    }
    let t = Instant::now();
    let out = notebook::compact_ipynb(&raw);
    let ms = t.elapsed().as_millis();
    // Quality: at least one code cell rendered and no leaked base64 (heuristic:
    // the output should not contain any line longer than 4 KB, since base64
    // payloads are typically long unbroken strings).
    let has_code = out.contains("```");
    let no_base64_leak = out.lines().all(|l| l.len() < 4096);
    let quality = if has_code && no_base64_leak { 1.0 } else { 0.0 };
    record(
        results,
        "output:notebook-compact",
        "notebook-compact",
        fname,
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #9 (iter #5): a synthetic notebook with 5 MB of inline base64
// PNG data must compress to <100 KB AND not leak the payload AND keep the source.
pub(crate) fn adv_notebook_base64_bomb(results: &mut Vec<RunResult>) {
    let bomb: String = "A".repeat(5_000_000);
    let raw = format!(
        r#"{{"cells":[{{"cell_type":"code","source":["plot(x, y)\nplt.show()\n"],"outputs":[{{"output_type":"display_data","data":{{"image/png":"{bomb}","text/plain":"<Figure 1>"}}}}],"metadata":{{}},"id":"abc","execution_count":42}}],"metadata":{{"kernelspec":{{"language":"python"}}}},"nbformat":4,"nbformat_minor":5}}"#
    );
    let t = Instant::now();
    let out = notebook::compact_ipynb(&raw);
    let ms = t.elapsed().as_millis();
    let source_kept = out.contains("plot(x, y)") && out.contains("plt.show()");
    let no_leak = !out.contains(&bomb);
    let small_enough = out.len() < 100_000;
    let quality = if source_kept && no_leak && small_enough {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:notebook:base64-bomb",
        "notebook-compact",
        "adversarial",
        &raw,
        &out,
        ms,
        quality,
    );
}

// ---- JSON/API tasks (iter #4) --------------------------------------------

// ---- web-content tasks (iter #3) -----------------------------------------

pub(crate) fn task_web_html(results: &mut Vec<RunResult>, fixtures: &Path, fname: &str) {
    let path = fixtures.join("web").join(fname);
    let Ok(html) = fs::read_to_string(&path) else {
        return;
    };
    let t = Instant::now();
    let md = web::html_to_markdown(&html);
    let ms = t.elapsed().as_millis();
    // Quality: the FIRST SIGNIFICANT WORD of the page title (e.g.
    // "Tokenization", not "(data" or "the") survives somewhere in the body —
    // and the body is not catastrophically empty. Wikipedia articles often
    // paraphrase their full title, so demanding the exact string here would
    // measure the test's strictness, not the extractor's quality.
    let title_re = Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap();
    let topic = title_re
        .captures(&html)
        .and_then(|c| c.get(1))
        .and_then(|m| {
            m.as_str()
                .split('-')
                .next()
                .unwrap_or("")
                .split_whitespace()
                .find(|w| w.len() >= 5 && w.chars().any(|c| c.is_alphabetic()))
                .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
        })
        .unwrap_or_default();
    let topic_lc = topic.to_lowercase();
    let md_lc = md.to_lowercase();
    let topic_kept = topic.is_empty() || md_lc.contains(&topic_lc);
    let body_kept = md.len() >= 200;
    let quality = if topic_kept && body_kept { 1.0 } else { 0.0 };
    record(
        results,
        "web:html-to-markdown",
        "web-extract",
        fname,
        &html,
        &md,
        ms,
        quality,
    );
}

pub(crate) fn task_web_a11y(results: &mut Vec<RunResult>, fixtures: &Path) {
    let path = fixtures.join("web").join("playwright-snapshot.txt");
    let Ok(snap) = fs::read_to_string(&path) else {
        return;
    };
    let t = Instant::now();
    let out = web::compact_a11y_tree(&snap);
    let ms = t.elapsed().as_millis();
    // Quality: top-level headings + main paragraph text preserved.
    let required = ["How to Use Tokenade", "Installation", "Quick start"];
    let kept = required.iter().filter(|m| out.contains(*m)).count();
    let quality = kept as f64 / required.len() as f64;
    record(
        results,
        "web:a11y-snapshot",
        "web-extract",
        "playwright",
        &snap,
        &out,
        ms,
        quality,
    );
}

// Adversarial #5 (iter #3): malformed HTML must not crash, must preserve some text.
pub(crate) fn adv_web_broken_html(results: &mut Vec<RunResult>) {
    let broken = r#"<html><body><p>start <strong>bold not closed
        <div><a href="http://x">link with <em>no close
        <script>not closed either
        <h1>Heading without close
        <p>another <code>inline never closed
        end of doc"#;
    let t = Instant::now();
    let md = web::html_to_markdown(broken);
    let ms = t.elapsed().as_millis();
    // The extractor must not crash and must keep at least the visible words.
    let required_any = ["Heading", "start", "another"];
    let kept = required_any.iter().any(|m| md.contains(*m));
    let no_script_leak = !md.contains("not closed either");
    let quality = match (kept, no_script_leak) {
        (true, true) => 1.0,
        (true, false) => 0.5,
        _ => 0.0,
    };
    record(
        results,
        "adv:web:broken-html",
        "web-extract",
        "adversarial",
        broken,
        &md,
        ms,
        quality,
    );
}

// Adversarial #6 (iter #3): script content must NEVER leak into the markdown,
// even if it contains text that looks like article body. Classic XSS-style
// payload: a `<script>` whose body would otherwise pass as content.
pub(crate) fn adv_web_script_injection(results: &mut Vec<RunResult>) {
    let html = r#"<html><body>
        <main>
          <h1>Article title</h1>
          <p>Real article body text.</p>
          <script>
            document.write("SECRET_FROM_SCRIPT_DO_NOT_LEAK");
            var x = "STILL_INSIDE_SCRIPT_TAG";
          </script>
          <style>
            .x { content: "STYLE_LEAK_FORBIDDEN"; }
          </style>
          <p>More real text.</p>
        </main>
      </body></html>"#;
    let t = Instant::now();
    let md = web::html_to_markdown(html);
    let ms = t.elapsed().as_millis();
    let body_kept = md.contains("Real article body text.") && md.contains("More real text.");
    let no_leak = !md.contains("SECRET_FROM_SCRIPT_DO_NOT_LEAK")
        && !md.contains("STILL_INSIDE_SCRIPT_TAG")
        && !md.contains("STYLE_LEAK_FORBIDDEN");
    let quality = if body_kept && no_leak {
        1.0
    } else if body_kept {
        0.5
    } else {
        0.0
    };
    record(
        results,
        "adv:web:script-injection",
        "web-extract",
        "adversarial",
        html,
        &md,
        ms,
        quality,
    );
}

// ---- mechanism-only fixture runs ------------------------------------------
