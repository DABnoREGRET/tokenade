//! `ndjson` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

pub(crate) fn synth_ndjson(n_lines: usize, error_every: usize, warn_every: usize) -> String {
    let mut s = String::with_capacity(n_lines * 200);
    for i in 0..n_lines {
        let level = if error_every > 0 && i % error_every == 0 {
            "error"
        } else if warn_every > 0 && i % warn_every == 0 {
            "warn"
        } else {
            "info"
        };
        s.push_str(&format!(
            "{{\"timestamp\":\"2024-01-15T10:{:02}:{:02}.{:03}Z\",\"level\":\"{level}\",\"message\":\"event #{i}\",\"service\":\"api\",\"request_id\":\"req-{i}\",\"user_id\":\"user-{}\",\"duration_ms\":{},\"status\":200,\"trace_id\":\"trace-{:016x}\"}}\n",
            (i / 60) % 60,
            i % 60,
            i % 1000,
            i % 1000,
            i % 500 + 5,
            i,
        ));
    }
    s
}

pub(crate) fn task_ndjson_real(results: &mut Vec<RunResult>) {
    // 2 000-line app log: ~95 % INFO + a handful of errors and warnings.
    let raw = synth_ndjson(2_000, 100, 50);
    let (out, sav, ms) = measure(&raw, ndjson::compact_with_savings);
    let quality = if out.contains("[tokenade:ndjson] 2000 JSON lines")
        && out.contains("20 error")
        && out.contains("20 warning")
        && out.contains("timestamp")
        && out.contains("trace_id")
        && out.contains("event #0")  // first sample
        && sav.percent() > 90.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:ndjson-compact",
        "ndjson",
        "synth-2k-20err-20warn",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #69: 100 000-line CloudWatch-style NDJSON. Must compact >97%
// in under 2 s.
pub(crate) fn adv_ndjson_huge_cloudwatch(results: &mut Vec<RunResult>) {
    let raw = synth_ndjson(100_000, 500, 200);
    let (out, sav, ms) = measure(&raw, ndjson::compact_with_savings);
    let quality = if out.contains("100000 JSON lines")
        && out.contains("200 error")
        && sav.percent() > 97.0
        && ms < 2000
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:ndjson:huge-100k",
        "ndjson",
        "adversarial",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #70: hostile mixed content:
//   - Lines with `{}` in QUOTED string values (must not break brace-balance)
//   - Lines with deeply-nested JSON in error.context
//   - Lines with Unicode in messages (emoji, CJK, RTL)
//   - Numeric severity (syslog style)
//   - A malformed JSON line in the middle (must skip without crashing)
//   - Interspersed blank lines
pub(crate) fn adv_ndjson_hostile(results: &mut Vec<RunResult>) {
    let raw = "{\"level\":\"info\",\"msg\":\"object literal in message: {a: 1, b: \\\"nested {brace}\\\"}\"}\n\
{\"level\":\"error\",\"msg\":\"Failed 🚀\",\"err\":{\"code\":500,\"context\":{\"nested\":{\"deep\":\"value\"}}}}\n\
\n\
{\"level\":\"warning\",\"msg\":\"München déjà-vu 中文 العربية\"}\n\
{\"level\":3,\"msg\":\"syslog-numeric critical\"}\n\
{this is malformed json and should be skipped}\n\
{\"level\":7,\"msg\":\"syslog-numeric debug\"}\n\
\n\
{\"@l\":\"Information\",\"msg\":\"Serilog-style entry\"}\n\
{\"a\":1}\n";
    let (out, sav, ms) = measure(raw, ndjson::compact_with_savings);
    // Expected counts: 2 errors (syslog 3 + level error), 1 warning, 2 info
    // (@l Information + level info), 1 debug (syslog 7), 1 other ({a:1}).
    // Malformed line is skipped, not counted.
    let quality = if out.contains("[tokenade:ndjson] 7 JSON lines")
        && out.contains("2 error")
        && out.contains("1 warning")
        && out.contains("2 info")
        && out.contains("1 debug")
        && out.contains("1 other")
        // Unicode preserved.
        && out.contains("München déjà-vu 中文 العربية")
        && out.contains("🚀")
        // Quoted brace literal preserved as quoted text (not broken).
        && out.contains("object literal in message:")
        && ms < 100
        && sav.percent() >= 0.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:ndjson:hostile",
        "ndjson",
        "adversarial",
        raw,
        &out,
        ms,
        quality,
    );
}
