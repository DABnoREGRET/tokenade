//! Per-command benchmark — measures the token-saving of every
//! `dispatch_named` route across the curated VM fixture set.
//!
//! Walks `crates/tokenade-core/tests/fixtures/{macos,windows}/**/*.txt`,
//! resolves each fixture stem to a program name (via the same
//! suffix-stripping trick the integration tests use), calls
//! `dispatch_named(prog, body)` and compares token counts before / after.
//!
//! Outputs `benchmarks/per-command-results.{csv,json}` plus a printed
//! human summary on stdout:
//!
//! - Top 20 best per-fixture savings (which compactors actually save).
//! - Bottom 20 (passthroughs / regressions).
//! - Per-OS aggregate (mean / median savings).
//! - Coverage: % of fixtures that route through dispatch_named (vs
//!   None which means caller would fall back to `generic`).
//! - Bytes-per-fixture distribution.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::Serialize;
use tokenade_core::cmd::dispatch_named;

#[derive(Serialize, Clone)]
struct FixtureResult {
    os: String,
    locale: String,
    stem: String,
    routed_via: String,
    bytes_before: usize,
    bytes_after: usize,
    tokens_before: usize,
    tokens_after: usize,
    saved_pct: f32,
}

/// The program of this capture — see [`tokenade_bench::program_of`]. This
/// body cut at the first underscore and did not try the dashed form, so
/// it did not find the same compactors as the test corpus.
fn resolve_prog(stem: &str, _body: &str) -> Option<String> {
    tokenade_bench::program_of(stem)
}

/// One capture, measured. The enumeration comes from `tokenade_bench::captures()`:
/// this file had its own, and it only read `["macos", "windows"]`.
fn measure(c: tokenade_bench::Capture) -> FixtureResult {
    let tokenade_bench::Capture {
        os,
        locale,
        stem,
        body,
        ..
    } = c;
    let prog = resolve_prog(&stem, &body);
    let prog_str = prog.clone().unwrap_or_else(|| "<unrouted>".into());
    let compacted = if let Some(p) = prog {
        dispatch_named(&p, &body).unwrap_or_else(|| body.clone())
    } else if os == "windows" {
        // PS-expression fixtures fall through to the powershell compactor.
        dispatch_named("powershell", &body).unwrap_or_else(|| body.clone())
    } else {
        body.clone()
    };
    let tokens_before = tokenade_core::tokenizer::estimate(&body);
    let tokens_after = tokenade_core::tokenizer::estimate(&compacted);
    FixtureResult {
        os,
        locale,
        stem,
        routed_via: prog_str,
        bytes_before: body.len(),
        bytes_after: compacted.len(),
        tokens_before,
        tokens_after,
        saved_pct: if tokens_before == 0 {
            0.0
        } else {
            100.0 * (1.0 - tokens_after as f32 / tokens_before as f32)
        },
    }
}

fn main() {
    tokenade_bench::reject_unknown_args(&[], &std::env::args().skip(1).collect::<Vec<_>>());
    let manifest = env!("CARGO_MANIFEST_DIR");
    let results: Vec<FixtureResult> = tokenade_bench::captures_on(tokenade_bench::Route::Command)
        .into_iter()
        .map(measure)
        .collect();
    if results.is_empty() {
        eprintln!("no fixtures found — the bench corpus is empty");
        std::process::exit(1);
    }
    // ──────── Aggregate ────────
    let total = results.len();
    let routed = results
        .iter()
        .filter(|r| r.routed_via != "<unrouted>")
        .count();
    let total_tokens_before: usize = results.iter().map(|r| r.tokens_before).sum();
    let total_tokens_after: usize = results.iter().map(|r| r.tokens_after).sum();
    let overall_saved = if total_tokens_before == 0 {
        0.0
    } else {
        100.0 * (1.0 - total_tokens_after as f32 / total_tokens_before as f32)
    };
    // Per-OS aggregate.
    let mut by_os: BTreeMap<&str, Vec<&FixtureResult>> = BTreeMap::new();
    for r in &results {
        by_os.entry(r.os.as_str()).or_default().push(r);
    }
    // Save CSV + JSON.
    let bench_dir = PathBuf::from(manifest)
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("benchmarks"))
        .expect("benchmarks dir");
    fs::create_dir_all(&bench_dir).ok();
    let json = serde_json::to_string_pretty(&results).expect("json");
    fs::write(bench_dir.join("per-command-results.json"), &json).ok();
    let mut csv = String::from(
        "os,locale,stem,routed_via,bytes_before,bytes_after,tokens_before,tokens_after,saved_pct\n",
    );
    for r in &results {
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},{},{:.2}\n",
            r.os,
            r.locale,
            r.stem,
            r.routed_via,
            r.bytes_before,
            r.bytes_after,
            r.tokens_before,
            r.tokens_after,
            r.saved_pct
        ));
    }
    fs::write(bench_dir.join("per-command-results.csv"), &csv).ok();

    // ──────── Print summary ────────
    println!("\n=== per-command bench: {total} fixtures ===");
    println!(
        "  routed via dispatch_named:        {routed}/{total} ({:.1}%)",
        100.0 * routed as f32 / total as f32
    );
    println!("  overall tokens before:            {total_tokens_before}");
    println!("  overall tokens after:             {total_tokens_after}");
    println!("  overall saved:                    {:.1}%", overall_saved);
    for (os, items) in &by_os {
        let tb: usize = items.iter().map(|r| r.tokens_before).sum();
        let ta: usize = items.iter().map(|r| r.tokens_after).sum();
        let saved = if tb == 0 {
            0.0
        } else {
            100.0 * (1.0 - ta as f32 / tb as f32)
        };
        let mut savings: Vec<f32> = items.iter().map(|r| r.saved_pct).collect();
        savings.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = savings[savings.len() / 2];
        println!(
            "  {:8} {:>4} fixtures · {:>8} → {:>8} tok ({:>5.1}% saved · median {:>5.1}%)",
            os,
            items.len(),
            tb,
            ta,
            saved,
            median
        );
    }
    // Top 20 winners.
    let mut sorted_by_save: Vec<&FixtureResult> = results.iter().collect();
    sorted_by_save.sort_by(|a, b| {
        b.saved_pct
            .partial_cmp(&a.saved_pct)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    println!("\nTop 20 best per-fixture savings:");
    for r in sorted_by_save.iter().take(20) {
        println!(
            "  {:>5.1}% saved · {:>6} → {:>6} tok · {:8} {:24} via {}",
            r.saved_pct, r.tokens_before, r.tokens_after, r.os, r.stem, r.routed_via
        );
    }
    // Bottom 20 (least savings or growth).
    println!("\nBottom 20 (no savings / regression):");
    for r in sorted_by_save.iter().rev().take(20) {
        println!(
            "  {:>5.1}% saved · {:>6} → {:>6} tok · {:8} {:24} via {}",
            r.saved_pct, r.tokens_before, r.tokens_after, r.os, r.stem, r.routed_via
        );
    }
    println!(
        "\nResults written to {}/per-command-results.{{csv,json}}",
        bench_dir.display()
    );
}
