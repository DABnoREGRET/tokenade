//! `misc` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

pub(crate) const EXTS: &[&str] = &[
    "rs", "py", "js", "ts", "tsx", "jsx", "go", "java", "c", "cpp", "h", "hpp",
];

/// Language-aware ground-truth regex. The earlier single-regex version
/// flagged Python code inside Rust string literals (ruff's `ty_ide` test
/// fixtures) as real declarations — falsely lowering recall.
pub(crate) fn strict_for(ext: &str) -> Option<Regex> {
    let pat = match ext {
        "rs" => {
            r"(?m)^(?:pub(?:\([^)]*\))?\s+)?(?:async\s+|unsafe\s+|const\s+|extern(?:\s+\x22[^\x22]*\x22)?\s+)*fn\s+([A-Za-z_][A-Za-z0-9_]*)|(?m)^(?:pub(?:\([^)]*\))?\s+)?(?:struct|enum|trait|type)\s+([A-Za-z_][A-Za-z0-9_]*)"
        }
        "py" => {
            r"(?m)^(?:async\s+)?def\s+([A-Za-z_][A-Za-z0-9_]*)|(?m)^class\s+([A-Za-z_][A-Za-z0-9_]*)"
        }
        "js" | "jsx" | "ts" | "tsx" | "mjs" => {
            r"(?m)^(?:export\s+)?(?:async\s+)?function\s+([A-Za-z_$][A-Za-z0-9_$]*)|(?m)^(?:export\s+)?(?:abstract\s+)?class\s+([A-Za-z_$][A-Za-z0-9_$]*)"
        }
        "go" => {
            r"(?m)^func\s+(?:\([^)]*\)\s*)?([A-Za-z_][A-Za-z0-9_]*)|(?m)^type\s+([A-Za-z_][A-Za-z0-9_]*)"
        }
        _ => return None,
    };
    Some(Regex::new(pat).unwrap())
}

pub(crate) fn summarize(results: &[RunResult]) {
    let mut by_task: BTreeMap<&str, Vec<&RunResult>> = BTreeMap::new();
    for r in results {
        by_task.entry(r.task.as_str()).or_default().push(r);
    }

    println!("\n══════════ tokenade-bench v2 (defensible) ══════════");
    println!("runs: {}\n", results.len());

    // Token-weighted savings reflect the actual user experience: sum tokens
    // saved across all repos divided by sum tokens before. A median hides
    // the massive savings on big repos when tiny repos contribute 0 %.
    println!("Per-task results (token-weighted savings; min & mean quality):");
    println!(
        "  {:<28} {:>4} {:>10} {:>8} {:>8}",
        "task", "n", "weighted −%", "min q", "mean q"
    );
    let mut per_task: Vec<(String, f64, f64)> = Vec::new();
    for (task, rs) in &by_task {
        let tb: u64 = rs.iter().map(|r| r.tokens_before as u64).sum();
        let ta: u64 = rs.iter().map(|r| r.tokens_after as u64).sum();
        let savings = if tb > 0 {
            (tb.saturating_sub(ta) as f64 / tb as f64) * 100.0
        } else {
            0.0
        };
        let qs: Vec<f64> = rs.iter().map(|r| r.quality).collect();
        let min_q = qs.iter().cloned().fold(1.0, f64::min);
        let mean_q = qs.iter().sum::<f64>() / qs.len() as f64;
        println!(
            "  {:<28} {:>4} {:>9.1}% {:>8.2} {:>8.2}",
            task,
            rs.len(),
            savings,
            min_q,
            mean_q
        );
        per_task.push((task.to_string(), savings, mean_q));
    }

    // Four named session mixes covering real Claude Code workloads. Web
    // tasks (html → markdown, Playwright snapshots) are part of real coding
    // agent sessions — docs lookups, GitHub issue triage, browser-driven
    // testing — and are weighted accordingly. Change weights and re-run to
    // model your own workload.
    let scenarios: &[(&str, &[(&str, f64)])] = &[
        (
            "balanced",
            &[
                ("nav:find-symbol", 0.05),
                ("overview:files-list", 0.04),
                ("review:file-skeleton", 0.10),
                ("output:filter-gitlog", 0.05),
                ("output:filter-cargo", 0.06),
                ("output:filter-pytest", 0.05),
                ("web:html-to-markdown", 0.09),
                ("web:a11y-snapshot", 0.04),
                ("output:json-compact", 0.09),
                ("output:notebook-compact", 0.08),
                ("output:csv-compact", 0.05),
                ("output:stacktrace-compact", 0.06),
                ("output:yaml-compact", 0.07),
                ("output:curl-verbose-compact", 0.06),
                ("output:pdf-extract", 0.05),
                ("output:diff-compact", 0.06),
                ("output:archive-compact", 0.05),
                ("output:terraform-compact", 0.05),
                ("output:tabular-compact", 0.05),
                ("output:env-compact", 0.04),
                ("output:markdown-compact", 0.06),
                ("output:lockfile-compact", 0.05),
                ("output:sql-compact", 0.04),
                ("output:metrics-compact", 0.04),
                ("output:accesslog-compact", 0.05),
                ("output:journal-compact", 0.05),
                ("output:sourcemap-compact", 0.04),
                ("output:openapi-compact", 0.05),
                ("output:graphql-compact", 0.04),
                ("output:strace-compact", 0.04),
                ("output:jvmbuild-compact", 0.05),
                ("output:kubectl-compact", 0.05),
                ("output:ansible-compact", 0.04),
                ("output:ghactions-compact", 0.05),
                ("output:pkginstall-compact", 0.05),
                ("output:k8smanifest-compact", 0.04),
                ("output:gitlog-compact", 0.04),
                ("output:dns-compact", 0.03),
                ("output:ndjson-compact", 0.05),
                ("output:systemd-compact", 0.04),
                ("output:filelist-compact", 0.05),
                ("output:disk-compact", 0.03),
                ("output:searchout-compact", 0.05),
                ("output:netstat-compact", 0.03),
                ("output:cargobench-compact", 0.03),
                ("output:containerlogs-compact", 0.05),
                ("output:vmstat-compact", 0.03),
                ("output:tcpdump-compact", 0.03),
            ],
        ),
        (
            "build-heavy", // lots of compile/test loops
            &[
                ("nav:find-symbol", 0.10),
                ("overview:files-list", 0.05),
                ("review:file-skeleton", 0.15),
                ("output:filter-gitlog", 0.10),
                ("output:filter-cargo", 0.30),
                ("output:filter-pytest", 0.25),
                ("web:html-to-markdown", 0.03),
                ("web:a11y-snapshot", 0.02),
            ],
        ),
        (
            "navigation-heavy", // exploration / code review
            &[
                ("nav:find-symbol", 0.20),
                ("nav:semantic-search", 0.20),
                ("overview:files-list", 0.15),
                ("review:file-skeleton", 0.25),
                ("output:filter-gitlog", 0.05),
                ("output:filter-cargo", 0.05),
                ("output:filter-pytest", 0.05),
                ("web:html-to-markdown", 0.03),
                ("web:a11y-snapshot", 0.02),
            ],
        ),
        (
            "web-heavy", // docs research, issue triage, browser-driven QA
            &[
                ("nav:find-symbol", 0.05),
                ("overview:files-list", 0.05),
                ("review:file-skeleton", 0.10),
                ("output:filter-gitlog", 0.05),
                ("output:filter-cargo", 0.05),
                ("output:filter-pytest", 0.05),
                ("web:html-to-markdown", 0.45),
                ("web:a11y-snapshot", 0.20),
            ],
        ),
    ];
    println!("\nSession-mix scenarios (named weights — pick yours):");
    let mut scenario_results: Vec<(String, f64, f64)> = Vec::new();
    for (name, weights) in scenarios {
        let (mut wsum, mut wq, mut wtot) = (0.0, 0.0, 0.0);
        for (t, w) in *weights {
            if let Some((_, s, q)) = per_task.iter().find(|(n, _, _)| n == t) {
                wsum += s * w;
                wq += q * w;
                wtot += w;
            }
        }
        let s = if wtot > 0.0 { wsum / wtot } else { 0.0 };
        let q = if wtot > 0.0 { wq / wtot } else { 1.0 };
        println!("  {:<18} savings = {:>5.1}%   quality = {:.2}", name, s, q);
        scenario_results.push((name.to_string(), s, q));
    }

    let adv: Vec<f64> = results
        .iter()
        .filter(|r| r.task.starts_with("adv:"))
        .map(|r| r.quality)
        .collect();
    let adv_min = adv.iter().cloned().fold(1.0, f64::min);
    let adv_mean = if adv.is_empty() {
        1.0
    } else {
        adv.iter().sum::<f64>() / adv.len() as f64
    };
    println!(
        "\nAdversarial impact-negative tests: min q = {adv_min:.2}, mean q = {adv_mean:.2} (over {} runs)",
        adv.len()
    );
    println!("  (lower than 1.00 = the mechanism is dropping required signal somewhere)");

    // Mission target: ≥−70% with quality ≥0.99 AND no adversarial signal loss.
    // We check the BALANCED scenario as the headline, but report all three.
    let (_, bal_s, bal_q) = scenario_results
        .iter()
        .find(|(n, _, _)| n == "balanced")
        .cloned()
        .unwrap_or((String::new(), 0.0, 1.0));
    let met = bal_s >= 70.0 && bal_q >= 0.99 && adv_min >= 0.99;
    println!(
        "\nMission target (balanced mix ≥−70% AND quality ≥0.99 AND no adversarial loss): {}",
        if met { "✅ MET" } else { "❌ not yet" }
    );
}

pub(crate) fn write_outputs(results: &[RunResult], out_dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(out_dir)?;
    let mut csv =
        String::from("task,mechanism,repo,tokens_before,tokens_after,savings_pct,millis,quality\n");
    for r in results {
        csv.push_str(&format!(
            "{},{},{},{},{},{:.1},{},{:.2}\n",
            r.task,
            r.mechanism,
            r.repo,
            r.tokens_before,
            r.tokens_after,
            r.savings_pct,
            r.millis,
            r.quality
        ));
    }
    fs::write(out_dir.join("results.csv"), csv)?;
    fs::write(
        out_dir.join("results.json"),
        serde_json::to_string_pretty(results)?,
    )?;

    // Also emit a canonical (deterministic) view: same numbers, with the
    // machine-dependent `millis` field stripped. Two machines that ran the
    // benchmark against the same pinned SHAs (see `benchmarks/repos/
    // clone_bench.sh`) should produce byte-identical `results-canonical.
    // json` files: diff them to verify reproducibility.
    let canonical: Vec<serde_json::Value> = results
        .iter()
        .map(|r| {
            serde_json::json!({
                "task": r.task,
                "mechanism": r.mechanism,
                "repo": r.repo,
                "tokens_before": r.tokens_before,
                "tokens_after": r.tokens_after,
                "savings_pct": r.savings_pct,
                "quality": r.quality,
            })
        })
        .collect();
    fs::write(
        out_dir.join("results-canonical.json"),
        serde_json::to_string_pretty(&canonical)?,
    )?;
    Ok(())
}

/// Run a head-to-head comparison against locally-installed competitor
/// binaries (rtk, token-optimizer-mcp, etc.) on the same fixture inputs.
/// Each competitor is invoked via `<bin> <args>` reading the fixture on
/// stdin; the output is tokenised the same way as STS so the comparison
/// is apples-to-apples. Tools that aren't on PATH are silently skipped.
/// Writes per-tool numbers to `benchmarks/competitors.csv`.
pub(crate) fn run_competitor_bench(fixtures: &Path, out_dir: &Path) {
    use std::io::Write;
    use std::process::{Command, Stdio};

    // (binary name, argv, fixture-path)
    let cases: &[(&str, &[&str], &str)] = &[
        ("rtk", &["proxy", "git", "log"], "anyhow-gitlog.txt"),
        ("rtk", &["proxy", "cargo", "build"], "cargo-build.log"),
        ("rtk", &["proxy", "pytest"], "pytest-collect.log"),
    ];
    // Cross-platform `which` — use the same PATH walker that the MCP
    // availability check uses. Avoids invoking the Unix-only `which`
    // binary so the bench compiles and runs identically on Windows.
    let path_has = tokenade_core::availability::is_available;
    let mut available: std::collections::BTreeMap<String, Vec<(String, f64)>> =
        std::collections::BTreeMap::new();
    for (bin, args, fixture) in cases {
        if !path_has(bin) {
            continue;
        }
        let path = fixtures.join(fixture);
        let Ok(input) = fs::read_to_string(&path) else {
            continue;
        };
        let before = tokenade_core::tokenizer::estimate(&input);
        let proc = Command::new(bin)
            .args(*args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut child) = proc else { continue };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(input.as_bytes());
        }
        let Ok(out) = child.wait_with_output() else {
            continue;
        };
        let after = tokenade_core::tokenizer::estimate(&String::from_utf8_lossy(&out.stdout));
        let savings = if before > 0 {
            ((before.saturating_sub(after)) as f64 / before as f64) * 100.0
        } else {
            0.0
        };
        available
            .entry((*bin).to_string())
            .or_default()
            .push(((*fixture).to_string(), savings));
    }
    if available.is_empty() {
        eprintln!(
            "→ no competitor binaries on PATH (looked for rtk, token-optimizer); skipping head-to-head"
        );
        return;
    }
    let path = out_dir.join("competitors.csv");
    let mut s = String::from("competitor,fixture,savings_pct\n");
    for (bin, rows) in &available {
        for (f, p) in rows {
            s.push_str(&format!("{bin},{f},{p:.1}\n"));
        }
    }
    let _ = fs::write(&path, s);
    eprintln!(
        "→ head-to-head with {} competitor(s) → {}",
        available.len(),
        path.display()
    );
}

/// Compare the current run's scenario savings against
/// `benchmarks/results-baseline.json`. Exit code:
///   0 — no regression, or baseline missing (first run, advisory)
///   1 — at least one scenario regressed >5 pp or quality dropped <0.99
pub(crate) fn run_baseline_check(results: &[RunResult], out_dir: &Path) -> i32 {
    use std::collections::BTreeMap;
    let baseline_path = out_dir.join("results-baseline.json");
    let baseline: Vec<RunResult> = match fs::read_to_string(&baseline_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
    {
        Some(v) => v,
        None => {
            eprintln!(
                "no baseline at {} — writing current as baseline (advisory pass)",
                baseline_path.display()
            );
            let _ = fs::write(
                &baseline_path,
                serde_json::to_string_pretty(results).unwrap(),
            );
            return 0;
        }
    };

    let aggregate = |rs: &[RunResult]| -> BTreeMap<String, (f64, f64)> {
        let mut acc: BTreeMap<String, (u64, u64, f64, usize)> = BTreeMap::new();
        for r in rs {
            let e = acc.entry(r.task.clone()).or_default();
            e.0 += r.tokens_before as u64;
            e.1 += r.tokens_after as u64;
            e.2 += r.quality;
            e.3 += 1;
        }
        acc.into_iter()
            .map(|(k, (tb, ta, q, n))| {
                let s = if tb > 0 {
                    (tb.saturating_sub(ta) as f64 / tb as f64) * 100.0
                } else {
                    0.0
                };
                (k, (s, q / n as f64))
            })
            .collect()
    };

    let cur = aggregate(results);
    let base = aggregate(&baseline);
    let mut regressed = Vec::<String>::new();
    for (task, (cur_s, cur_q)) in &cur {
        if let Some((base_s, base_q)) = base.get(task) {
            if cur_s + 5.0 < *base_s {
                regressed.push(format!(
                    "{task}: savings {:.1}% < baseline {:.1}% (−{:.1} pp)",
                    cur_s,
                    base_s,
                    base_s - cur_s
                ));
            }
            if *cur_q + 0.01 < *base_q && *cur_q < 0.99 {
                regressed.push(format!(
                    "{task}: quality {:.2} < baseline {:.2}",
                    cur_q, base_q
                ));
            }
        }
    }
    if regressed.is_empty() {
        eprintln!(
            "baseline-check: no regression vs {}",
            baseline_path.display()
        );
        0
    } else {
        eprintln!("baseline-check: REGRESSION detected:");
        for r in &regressed {
            eprintln!("  {r}");
        }
        1
    }
}
