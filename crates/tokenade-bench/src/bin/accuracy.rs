//! tokenade-accuracy — does the *answer* survive compaction?
//!
//! `tokenade-bench` already measures how many tokens each mechanism saves and
//! a per-mechanism *fidelity* heuristic ("every error line preserved"). What it
//! does **not** measure is the thing that actually matters to a downstream
//! agent: **given a realistic question about a tool's output, can that question
//! still be answered from the *compacted* output?**
//!
//! That is the headroom GSM8K/TruthfulQA precedent (accuracy must be *preserved*
//! by compaction — ideally improved by removing distractors) ported to
//! tokenade's domain (shell/tool output instead of math word problems).
//!
//! ## Two modes
//!
//! 1. **Offline answer-survival gate (default, free, CI-runnable).**
//!    For each task we run the *real production compactor* and assert every
//!    ground-truth answer span survives. This is a deterministic, zero-cost
//!    *lower bound* on accuracy: if the answer string is gone, the question is
//!    provably unanswerable from the compacted text; if it survives, the answer
//!    is at least *recoverable*. Mirrors the existing adversarial gates, but
//!    keyed to questions an agent actually asks rather than mechanism internals.
//!
//! 2. **Live LLM accuracy (`TOKENADE_BENCH_LIVE=1`, opt-in, costs API tokens).**
//!    Asks a real model the question twice — once over raw output, once over
//!    compacted — and scores both against the ground truth. Reports
//!    `accuracy_raw`, `accuracy_compacted`, and the delta (the headline headroom
//!    number). Gated behind an env var + `ANTHROPIC_API_KEY`; shells out to
//!    `curl` so the default `cargo build` gains no HTTP dependency.
//!
//! Results: `benchmarks/accuracy-offline.{csv,json}` (+ `accuracy-live.*`).
//! Exit code is non-zero when any answer fails to survive — so the offline gate
//! can guard CI exactly like the adversarial tests.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::Serialize;
use tokenade_core::{filter, json as tokenade_json, lockfile, ndjson, tokenizer};

/// Which real production compactor a task exercises.
enum Comp {
    /// The `filter_output` / `exec_compact` path: route by the argv that
    /// produced the output (`argv[0]` picks the per-command compactor).
    Filter(&'static [&'static str]),
    /// A content-format compactor invoked directly (the `auto_compact` path),
    /// for outputs detected by shape rather than by command name.
    Module(&'static str, fn(&str) -> String),
}

impl Comp {
    fn label(&self) -> String {
        match self {
            Comp::Filter(argv) => format!("filter:{}", argv.first().copied().unwrap_or("?")),
            Comp::Module(name, _) => format!("module:{name}"),
        }
    }
    fn run(&self, raw: &str) -> String {
        match self {
            Comp::Filter(argv) => {
                let owned: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
                filter::filter_command(&owned, raw).output
            }
            Comp::Module(_, f) => f(raw),
        }
    }
}

/// One (raw output, question, ground-truth) triple.
struct Task {
    /// Stable id, `domain:slug`.
    id: &'static str,
    /// The real compactor exercised.
    comp: Comp,
    /// The question an agent would ask about `raw`.
    question: &'static str,
    /// The raw tool output (built inline so the corpus is deterministic and
    /// needs no external fixture files).
    raw: String,
    /// Answer spans that MUST survive compaction. Plain entries match
    /// case-insensitively as substrings; an entry prefixed `re:` is a regex.
    must_survive: &'static [&'static str],
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    if let Some(pat) = needle.strip_prefix("re:") {
        return regex::Regex::new(pat)
            .map(|re| re.is_match(haystack))
            .unwrap_or(false);
    }
    haystack.to_lowercase().contains(&needle.to_lowercase())
}
// `#[path]` because this file is the ROOT of its binary: `mod x;` would
// look for `src/bin/x.rs` there, next to the other benches.
#[path = "accuracy/corpus.rs"]
mod corpus;
#[path = "accuracy/live.rs"]
mod live;
#[path = "accuracy/workload.rs"]
mod workload;
pub(crate) use corpus::*;
pub(crate) use live::*;
pub(crate) use workload::*;

// ── offline run ───────────────────────────────────────────────────────────────

/// Below this savings %, the compactor barely touched the output — a near
/// passthrough. Answer-survival on such a task proves almost nothing (the
/// answer survives because *everything* did), so we flag it instead of letting
/// it silently inflate the retention headline. The honest claim is "the answer
/// survives compaction that actually *happened*", not "survives a no-op".
const MEANINGFUL_SAVINGS_PCT: f64 = 5.0;

#[derive(Serialize, Clone)]
struct OfflineRow {
    id: String,
    mechanism: String,
    question: String,
    tokens_before: usize,
    tokens_after: usize,
    savings_pct: f64,
    /// Every required answer span found in the compacted output.
    answer_survived: bool,
    /// The compactor removed a meaningful fraction (≥ `MEANINGFUL_SAVINGS_PCT`).
    /// When false, this task is a near-passthrough and its survival result is
    /// not evidence the compactor preserves answers under real pressure.
    meaningfully_compacted: bool,
    /// Which required spans were dropped (empty when survived).
    missing: Vec<String>,
}

fn run_offline() -> (Vec<OfflineRow>, bool) {
    let tasks = corpus();
    let mut rows = Vec::new();
    let mut all_ok = true;
    for t in &tasks {
        let compacted = t.comp.run(&t.raw);
        let tb = tokenizer::estimate(&t.raw);
        let ta = tokenizer::estimate(&compacted);
        let missing: Vec<String> = t
            .must_survive
            .iter()
            .filter(|m| !contains_ci(&compacted, m))
            .map(|m| m.to_string())
            .collect();
        let survived = missing.is_empty();
        all_ok &= survived;
        let saved = if tb == 0 {
            0.0
        } else {
            100.0 * (1.0 - ta as f64 / tb as f64)
        };
        rows.push(OfflineRow {
            id: t.id.to_string(),
            mechanism: t.comp.label(),
            question: t.question.to_string(),
            tokens_before: tb,
            tokens_after: ta,
            savings_pct: saved,
            answer_survived: survived,
            meaningfully_compacted: saved >= MEANINGFUL_SAVINGS_PCT,
            missing,
        });
    }
    (rows, all_ok)
}

fn median(mut xs: Vec<f64>) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    xs[xs.len() / 2]
}

fn bench_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("benchmarks"))
        .expect("benchmarks dir")
}

fn main() {
    tokenade_bench::reject_unknown_args(&[], &std::env::args().skip(1).collect::<Vec<_>>());
    let live = std::env::var("TOKENADE_BENCH_LIVE").is_ok_and(|v| v == "1");

    let (rows, all_ok) = run_offline();

    // Write artifacts.
    let dir = bench_dir();
    fs::create_dir_all(&dir).ok();
    let json = serde_json::to_string_pretty(&rows).expect("json");
    fs::write(dir.join("accuracy-offline.json"), &json).ok();
    let mut csv = String::from(
        "id,mechanism,tokens_before,tokens_after,savings_pct,answer_survived,meaningfully_compacted,missing\n",
    );
    for r in &rows {
        csv.push_str(&format!(
            "{},{},{},{},{:.1},{},{},{}\n",
            r.id,
            r.mechanism,
            r.tokens_before,
            r.tokens_after,
            r.savings_pct,
            r.answer_survived,
            r.meaningfully_compacted,
            r.missing.join("|"),
        ));
    }
    fs::write(dir.join("accuracy-offline.csv"), &csv).ok();

    // Summary.
    let total = rows.len();
    let survived = rows.iter().filter(|r| r.answer_survived).count();
    let retention = 100.0 * survived as f64 / total.max(1) as f64;
    let med_save = median(rows.iter().map(|r| r.savings_pct).collect());
    let tb: usize = rows.iter().map(|r| r.tokens_before).sum();
    let ta: usize = rows.iter().map(|r| r.tokens_after).sum();
    let overall_save = if tb == 0 {
        0.0
    } else {
        100.0 * (1.0 - ta as f64 / tb as f64)
    };

    // Honesty metric: how many tasks were *actually* compacted, and — the claim
    // that matters — did the answer survive among only those. A passthrough
    // task that "survives" proves nothing, so it must not pad the headline.
    let exercised: Vec<&OfflineRow> = rows.iter().filter(|r| r.meaningfully_compacted).collect();
    let exercised_n = exercised.len();
    let exercised_survived = exercised.iter().filter(|r| r.answer_survived).count();
    let exercised_retention = 100.0 * exercised_survived as f64 / exercised_n.max(1) as f64;

    println!("\n=== tokenade accuracy: answer-survival (offline) ===");
    println!("  tasks:                 {total}");
    println!("  answer retention:      {survived}/{total} ({retention:.1}%)");
    println!(
        "  meaningfully compacted: {exercised_n}/{total} (≥{MEANINGFUL_SAVINGS_PCT:.0}% savings)"
    );
    println!(
        "  retention (exercised): {exercised_survived}/{exercised_n} ({exercised_retention:.1}%)  ← the honest headline"
    );
    println!("  token savings:         {overall_save:.1}% overall · {med_save:.1}% median");
    println!("  tokens:                {tb} → {ta}");

    // Name the passthrough tasks explicitly so the corpus stays honest over time
    // (each one is a candidate to harden with real noise or drop).
    let passthrough: Vec<&OfflineRow> = rows.iter().filter(|r| !r.meaningfully_compacted).collect();
    if !passthrough.is_empty() {
        println!(
            "\n  ⚠ passthrough (<{MEANINGFUL_SAVINGS_PCT:.0}% savings — survival not meaningfully tested):"
        );
        for r in &passthrough {
            println!("    {:24} {:.1}% savings", r.id, r.savings_pct);
        }
    }

    // Per-mechanism breakdown.
    let mut by_mech: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for r in &rows {
        let e = by_mech.entry(r.mechanism.as_str()).or_default();
        e.0 += 1;
        e.1 += r.answer_survived as usize;
    }
    println!("\n  per mechanism (survived/total):");
    for (mech, (n, ok)) in &by_mech {
        println!("    {mech:24} {ok}/{n}");
    }

    if !all_ok {
        println!("\n  FAILURES (answer dropped by compaction):");
        for r in rows.iter().filter(|r| !r.answer_survived) {
            println!("    {:24} missing: {}", r.id, r.missing.join(", "));
        }
    }
    // Headroom-style workload table: token-reduction × answer-retention grouped
    // by domain, written as publishable markdown evidence for the product.
    let workload_md = render_workload_md(&rows);
    tokenade_bench::write_generated_doc(&dir.join("accuracy-workload.md"), &workload_md).ok();
    println!("\n{}", workload_md.trim_end());

    println!(
        "\nResults written to {}/accuracy-offline.{{csv,json}} + accuracy-workload.md",
        dir.display()
    );

    let live_ok = if live {
        run_live(&dir)
    } else {
        println!(
            "\n(live LLM accuracy skipped — set TOKENADE_BENCH_LIVE=1 + ANTHROPIC_API_KEY to run it)"
        );
        true
    };

    // The offline gate guards CI exactly like the adversarial tests.
    if !all_ok {
        eprintln!("\nFAIL: at least one answer did not survive compaction.");
        std::process::exit(1);
    }
    // In strict mode, a live end-task accuracy regression beyond tolerance also
    // fails the build — the "no quality loss" claim becomes an enforced gate.
    if live && !live_ok && std::env::var("TOKENADE_BENCH_STRICT").is_ok_and(|v| v == "1") {
        eprintln!("\nFAIL: live accuracy regressed beyond tolerance (TOKENADE_BENCH_STRICT).");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, savings: f64, survived: bool, exercised: bool) -> OfflineRow {
        OfflineRow {
            id: id.to_string(),
            mechanism: "m".to_string(),
            question: "q".to_string(),
            tokens_before: 100,
            tokens_after: 10,
            savings_pct: savings,
            answer_survived: survived,
            meaningfully_compacted: exercised,
            missing: vec![],
        }
    }

    #[test]
    fn domain_of_splits_on_first_colon() {
        assert_eq!(domain_of("kubectl:crashloop"), "kubectl");
        assert_eq!(domain_of("git:numstat:edge"), "git");
        assert_eq!(domain_of("nodomain"), "nodomain");
    }

    #[test]
    fn workload_rows_group_and_score_by_domain() {
        let rows = vec![
            row("a:one", 90.0, true, true),
            row("a:two", 50.0, false, true),
            // passthrough: not counted toward exercised/retention.
            row("b:one", 1.0, true, false),
        ];
        let wl = workload_rows(&rows);

        let a = wl.iter().find(|r| r.domain == "a").unwrap();
        assert_eq!(a.tasks, 2);
        assert_eq!(a.exercised, 2);
        assert_eq!(a.survived, 1);
        assert!((a.retention_pct - 50.0).abs() < 1e-6);

        let b = wl.iter().find(|r| r.domain == "b").unwrap();
        assert_eq!(b.tasks, 1);
        assert_eq!(b.exercised, 0);
        // Nothing meaningfully compacted → 0/ max(1) → 0%, never an inflated 100%.
        assert_eq!(b.retention_pct, 0.0);
    }

    #[test]
    fn workload_md_has_table_header_and_totals_row() {
        let md = render_workload_md(&[row("a:one", 90.0, true, true)]);
        assert!(md.contains("| domain | tasks | median savings"));
        assert!(md.contains("| **all** |"));
        assert!(md.contains("| a |"));
    }

    #[test]
    fn live_workload_verdict_reflects_tolerance() {
        let rows = vec![LiveRow {
            id: "a:one".to_string(),
            mechanism: "m".to_string(),
            acc_raw: 1.0,
            acc_compacted: 1.0,
            trials: 3,
            tokens_before: 100,
            tokens_after: 20,
        }];
        let pass = render_live_workload_md(&rows, 100.0, 100.0, 0.0, true, 2.0);
        assert!(pass.contains("NO QUALITY LOSS"));
        assert!(pass.contains("token reduction"));
        let fail = render_live_workload_md(&rows, 100.0, 90.0, -10.0, false, 2.0);
        assert!(fail.contains("REGRESSION"));
    }
}
