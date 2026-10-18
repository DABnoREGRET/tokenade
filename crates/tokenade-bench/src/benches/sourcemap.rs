//! `sourcemap` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

pub(crate) fn synth_sourcemap(
    n_sources: usize,
    content_size: usize,
    mappings_size: usize,
) -> String {
    let mut s = String::from(r#"{"version":3,"file":"main.min.js","sourceRoot":"","sources":["#);
    for i in 0..n_sources {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!("\"webpack:///./src/file_{i}.js\""));
    }
    s.push_str(r#"],"sourcesContent":["#);
    let per = content_size / n_sources.max(1);
    for i in 0..n_sources {
        if i > 0 {
            s.push(',');
        }
        s.push('"');
        s.push_str(&"x".repeat(per));
        s.push('"');
    }
    s.push_str(r#"],"names":["foo","bar","baz"],"mappings":""#);
    s.push_str(&"A".repeat(mappings_size));
    s.push_str("\"}");
    s
}

/// Realistic source map: 100 sources × 5 KB content each + 200 KB mappings.
pub(crate) fn task_sourcemap_real(results: &mut Vec<RunResult>) {
    let raw = synth_sourcemap(100, 500_000, 200_000);
    let (out, sav, ms) = measure(&raw, sourcemap::compact_with_savings);
    let quality = if out.contains("version=3")
        && out.contains("file=main.min.js")
        && out.contains("sources (100):")
        && !out.contains("xxxxxxxx")
        && sav.percent() > 95.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:sourcemap-compact",
        "sourcemap",
        "synth-100src-700kB",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #45: a 20 MB source map (real bundle scale) must compact >99 %
// in under 5 seconds.
pub(crate) fn adv_sourcemap_huge(results: &mut Vec<RunResult>) {
    let raw = synth_sourcemap(200, 10_000_000, 10_000_000);
    let (out, sav, ms) = measure(&raw, sourcemap::compact_with_savings);
    let quality = if out.contains("sources (200):") && sav.percent() > 99.0 && ms < 5000 {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:sourcemap:huge-20MB",
        "sourcemap",
        "adversarial",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #46: malformed source map (truncated JSON). Must NOT panic, and
// since detection still flags it as a sourcemap shape but parsing fails,
// the compactor must return the original verbatim.
pub(crate) fn adv_sourcemap_truncated(results: &mut Vec<RunResult>) {
    let raw = r#"{"version":3,"file":"x.min.js","sources":["a.js","b.js"],"mappings":"AAAA"#;
    // Note: missing closing quote + brace.
    let (out, _sav, ms) = measure(raw, sourcemap::compact_with_savings);
    // Contract: no panic + output non-empty.
    let quality = if !out.is_empty() && ms < 100 {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:sourcemap:truncated",
        "sourcemap",
        "adversarial",
        raw,
        &out,
        ms,
        quality,
    );
}
