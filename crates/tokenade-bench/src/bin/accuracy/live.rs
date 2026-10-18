//! **The real judge** — a real model, opt-in.
//!
//! The offline corpus asks "is the expected string still there?".
//! A model asks "does the question have an ANSWER?", which is not
//! the same thing: a fact preserved but drowned passes the first test and fails
//! the second.
//!
//! Opt-in because it costs money and is not reproducible
//! — a result that cannot be replayed cannot be a ratchet.

use super::*;

// ── live LLM accuracy (opt-in) ────────────────────────────────────────────────

#[derive(Serialize, Clone)]
pub(crate) struct LiveRow {
    pub(crate) id: String,
    pub(crate) mechanism: String,
    /// Fraction of trials answered correctly from the raw output (0.0–1.0).
    pub(crate) acc_raw: f64,
    /// Fraction of trials answered correctly from the compacted output.
    pub(crate) acc_compacted: f64,
    pub(crate) trials: usize,
    pub(crate) tokens_before: usize,
    pub(crate) tokens_after: usize,
}

/// Render the live workload table: per-domain token-reduction next to raw and
/// compacted end-task accuracy, plus the headline no-quality-loss verdict.
pub(crate) fn render_live_workload_md(
    rows: &[LiveRow],
    acc_raw: f64,
    acc_cmp: f64,
    delta: f64,
    verdict_ok: bool,
    tolerance_pp: f64,
) -> String {
    let mut by: BTreeMap<&str, Vec<&LiveRow>> = BTreeMap::new();
    for r in rows {
        by.entry(domain_of(&r.id)).or_default().push(r);
    }
    let verdict = if verdict_ok {
        "NO QUALITY LOSS ✓"
    } else {
        "REGRESSION ✗"
    };
    let mut s = String::from("# tokenade accuracy workload (live end-task)\n\n");
    s.push_str(&format!(
        "**Verdict: {verdict}** — raw {acc_raw:.1}% → compacted {acc_cmp:.1}% (delta {delta:+.1}pp, tolerance {tolerance_pp:.1}pp)\n\n"
    ));
    s.push_str(
        "| domain | tasks | token reduction | accuracy raw | accuracy compacted | delta |\n",
    );
    s.push_str(
        "|--------|------:|----------------:|-------------:|-------------------:|------:|\n",
    );
    for (domain, rs) in &by {
        let n = rs.len() as f64;
        let tb: usize = rs.iter().map(|r| r.tokens_before).sum();
        let ta: usize = rs.iter().map(|r| r.tokens_after).sum();
        let red = if tb == 0 {
            0.0
        } else {
            100.0 * (1.0 - ta as f64 / tb as f64)
        };
        let ar = 100.0 * rs.iter().map(|r| r.acc_raw).sum::<f64>() / n;
        let ac = 100.0 * rs.iter().map(|r| r.acc_compacted).sum::<f64>() / n;
        s.push_str(&format!(
            "| {} | {} | {:.1}% | {:.0}% | {:.0}% | {:+.0}pp |\n",
            domain,
            rs.len(),
            red,
            ar,
            ac,
            ac - ar
        ));
    }
    s.push_str(&format!(
        "| **all** | **{}** | — | **{acc_raw:.1}%** | **{acc_cmp:.1}%** | **{delta:+.1}pp** |\n",
        rows.len()
    ));
    s
}

/// Ask the model `question` with `context` prepended. Shells out to `curl`
/// (no HTTP crate dependency); returns the model's text answer, or None on any
/// failure (missing key, missing curl, network/parse error).
pub(crate) fn ask_llm(context: &str, question: &str) -> Option<String> {
    let key = std::env::var("ANTHROPIC_API_KEY").ok()?;
    let model = tokenade_core::judge::judge_model();
    let prompt = format!(
        "You are given the output of a command. Answer the question using ONLY that output, as briefly as possible.\n\n--- OUTPUT ---\n{context}\n--- END OUTPUT ---\n\nQuestion: {question}"
    );
    let body = serde_json::json!({
        "model": model,
        "max_tokens": 256,
        "messages": [{ "role": "user", "content": prompt }],
    });
    let out = std::process::Command::new("curl")
        .args([
            "-sS",
            "https://api.anthropic.com/v1/messages",
            "-H",
            "content-type: application/json",
            "-H",
            "anthropic-version: 2023-06-01",
            "-H",
            &format!("x-api-key: {key}"),
            "-d",
            "@-",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .ok()
        .and_then(|mut child| {
            use std::io::Write;
            child
                .stdin
                .take()?
                .write_all(body.to_string().as_bytes())
                .ok()?;
            child.wait_with_output().ok()
        })?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let text = v
        .get("content")?
        .as_array()?
        .iter()
        .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
        .collect::<Vec<_>>()
        .join("");
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// An answer is "correct" if the model's reply contains every ground-truth span
/// (same predicate the offline gate uses).
pub(crate) fn scored_correct(answer: &str, must: &[&str]) -> bool {
    must.iter().all(|m| contains_ci(answer, m))
}

/// Returns whether the "no quality loss" verdict held (compacted accuracy did
/// not regress past tolerance). Always `true` when the run is inconclusive
/// (no key, no successful calls) so an unrunnable live pass never fails CI on
/// its own — only a *measured* regression does, and only in strict mode.
pub(crate) fn run_live(dir: &std::path::Path) -> bool {
    if std::env::var("ANTHROPIC_API_KEY").is_err() {
        println!("\n(live mode requested but ANTHROPIC_API_KEY is unset — skipping)");
        return true;
    }
    // Real models are stochastic; one trial per side is noisy. Average over
    // `TOKENADE_BENCH_TRIALS` (default 1) for a stable per-task accuracy.
    let trials: usize = std::env::var("TOKENADE_BENCH_TRIALS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&n| n >= 1)
        .unwrap_or(1);
    // How far compacted accuracy may fall below raw before we call it a loss.
    let tolerance_pp: f64 = std::env::var("TOKENADE_BENCH_MAX_REGRESSION_PP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2.0);
    println!("\n=== tokenade accuracy: live LLM (raw vs compacted, {trials} trial(s)) ===");
    let tasks = corpus();
    let mut rows = Vec::new();
    for t in &tasks {
        let compacted = t.comp.run(&t.raw);
        let mut raw_hits = 0usize;
        let mut cmp_hits = 0usize;
        let mut calls_ok = true;
        for _ in 0..trials {
            let (Some(raw_ans), Some(cmp_ans)) =
                (ask_llm(&t.raw, t.question), ask_llm(&compacted, t.question))
            else {
                calls_ok = false;
                break;
            };
            raw_hits += scored_correct(&raw_ans, t.must_survive) as usize;
            cmp_hits += scored_correct(&cmp_ans, t.must_survive) as usize;
        }
        if !calls_ok {
            println!("  {:24} LLM call failed — skipped", t.id);
            continue;
        }
        let acc_raw = raw_hits as f64 / trials as f64;
        let acc_cmp = cmp_hits as f64 / trials as f64;
        println!(
            "  {:24} raw={:.0}%  compacted={:.0}%",
            t.id,
            100.0 * acc_raw,
            100.0 * acc_cmp
        );
        rows.push(LiveRow {
            id: t.id.to_string(),
            mechanism: t.comp.label(),
            acc_raw,
            acc_compacted: acc_cmp,
            trials,
            tokens_before: tokenizer::estimate(&t.raw),
            tokens_after: tokenizer::estimate(&compacted),
        });
    }
    if rows.is_empty() {
        println!("  (no successful live calls)");
        return true;
    }
    let n = rows.len() as f64;
    let acc_raw = 100.0 * rows.iter().map(|r| r.acc_raw).sum::<f64>() / n;
    let acc_cmp = 100.0 * rows.iter().map(|r| r.acc_compacted).sum::<f64>() / n;
    let delta = acc_cmp - acc_raw;
    let verdict_ok = delta >= -tolerance_pp;
    println!("\n  accuracy raw={acc_raw:.1}%  compacted={acc_cmp:.1}%  delta={delta:+.1}pp");
    println!(
        "  verdict: {} (tolerance {tolerance_pp:.1}pp)",
        if verdict_ok {
            "NO QUALITY LOSS ✓"
        } else {
            "REGRESSION ✗"
        }
    );

    // Live workload table by domain — token-reduction beside end-task accuracy.
    let live_md = render_live_workload_md(&rows, acc_raw, acc_cmp, delta, verdict_ok, tolerance_pp);
    tokenade_bench::write_generated_doc(&dir.join("accuracy-live-workload.md"), &live_md).ok();
    println!("\n{}", live_md.trim_end());

    let json = serde_json::to_string_pretty(&rows).expect("json");
    fs::write(dir.join("accuracy-live.json"), &json).ok();
    let mut csv =
        String::from("id,mechanism,acc_raw,acc_compacted,trials,tokens_before,tokens_after\n");
    for r in &rows {
        csv.push_str(&format!(
            "{},{},{:.3},{:.3},{},{},{}\n",
            r.id,
            r.mechanism,
            r.acc_raw,
            r.acc_compacted,
            r.trials,
            r.tokens_before,
            r.tokens_after
        ));
    }
    fs::write(dir.join("accuracy-live.csv"), &csv).ok();
    println!(
        "\nResults written to {}/accuracy-live.{{csv,json}} + accuracy-live-workload.md",
        dir.display()
    );
    verdict_ok
}
