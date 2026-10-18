//! `tokenade_json` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

pub(crate) fn task_json(results: &mut Vec<RunResult>, fixtures: &Path, fname: &str, marker: &str) {
    let path = fixtures.join("json").join(fname);
    let Ok(raw) = fs::read_to_string(&path) else {
        return;
    };
    if raw.is_empty() {
        return;
    }
    let t = Instant::now();
    let out = tokenade_json::compact(&raw);
    let ms = t.elapsed().as_millis();
    // Quality: parser-survived (output is non-empty) AND the named signal
    // (e.g. an item name or PR title) is preserved in the compacted output.
    let quality = if !out.is_empty() && (marker.is_empty() || out.contains(marker)) {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:json-compact",
        "json-compact",
        fname,
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #7 (iter #4): hostile JSON keys (control chars, Unicode,
// embedded quotes via escapes) must not crash, must drop noise as usual.
pub(crate) fn adv_json_hostile_keys(results: &mut Vec<RunResult>) {
    let raw = "{\"\\u0000nul\":\"a\",\"emoji-🦀\":\"b\",\"with \\\"quote\\\"\":\"c\",\"node_id\":\"drop\",\"keep\":\"x\"}";
    let t = Instant::now();
    let out = tokenade_json::compact(raw);
    let ms = t.elapsed().as_millis();
    let body_kept = out.contains("\"a\"")
        && out.contains("\"b\"")
        && out.contains("\"c\"")
        && out.contains("\"x\"");
    let noise_dropped = !out.contains("node_id");
    let quality = if body_kept && noise_dropped { 1.0 } else { 0.0 };
    record(
        results,
        "adv:json:hostile-keys",
        "json-compact",
        "adversarial",
        raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #8 (iter #4): a 15 MB real npm registry document must complete
// in reasonable time (< 5s) AND compress substantially without losing the
// package name.
pub(crate) fn adv_json_huge_15mb(results: &mut Vec<RunResult>, fixtures: &Path) {
    let path = fixtures.join("json").join("npm-typescript.json");
    let Ok(raw) = fs::read_to_string(&path) else {
        return;
    };
    if raw.len() < 1_000_000 {
        return; // skip if the fixture is small/missing
    }
    let t = Instant::now();
    let out = tokenade_json::compact(&raw);
    let ms = t.elapsed().as_millis();
    // Quality: package name preserved, completed in < 5_000 ms.
    let name_kept = out.contains("\"typescript\"");
    let fast_enough = ms < 5_000;
    let quality = match (name_kept, fast_enough) {
        (true, true) => 1.0,
        (true, false) => 0.5,
        _ => 0.0,
    };
    record(
        results,
        "adv:json:huge-15mb",
        "json-compact",
        "adversarial",
        &raw,
        &out,
        ms,
        quality,
    );
}
