//! tokenade-bench — defensible benchmark of token-saving mechanisms.
//!
//! Replaces the v1 strawman with four design rules:
//!
//! 1. **Realistic baselines.** Navigation/overview compare tokenade to what an agent
//!    actually does without it — an in-process `grep`/`find` (cheap, line-
//!    grained, capped at 5k matches) — NOT "read 25 whole files". Both sides
//!    are computed in-process so no external plugin (rtk, codegraph…) can
//!    intercept either path.
//! 2. **Real noisy fixtures.** Filter tasks consume real captured logs in
//!    `benchmarks/fixtures/` (one `git log --stat` per repo, plus real
//!    `cargo check` and `pytest --collect-only` runs). No synthetic input,
//!    no input counted N times across repos.
//! 3. **Adversarial impact tests.** Three pure-quality measurements that
//!    penalize the mechanism for dropping signal:
//!      - `adv:filter-completeness` — every error/FAILED line preserved.
//!      - `adv:skeleton-fidelity`  — every top-level declaration preserved.
//!      - `adv:index-recall`       — every declaration found by a strict
//!        ground-truth regex is in the index.
//! 4. **Honest aggregation.** No single token-weighted headline. We report
//!    per-task **median** savings + **min** & **mean** quality, plus a
//!    realistic-session weighted mean with **documented** task weights, plus
//!    the adversarial worst case.
//!
//! Results: `benchmarks/results.csv` + `results.json`.

mod benches;
use benches::*;
use benches::{adversarial::*, filters::*, nav::*};
use regex::Regex;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tokenade_core::{
    accesslog, ansible, archive, auto, cargobench, containerlogs, csv as tokenade_csv,
    diff as tokenade_diff, disk, dns, docs, env as tokenade_env, filelist, filter, ghactions,
    gitlog, graphql, index, journal, json as tokenade_json, jvmbuild, k8smanifest, kubectl,
    lockfile, logs as tokenade_logs, markdown, metrics, ndjson, netstat, notebook, openapi,
    pkginstall, searchout, sourcemap, sql, stacktrace, stealth, strace, systemd, tabular, tcpdump,
    terraform, tokenizer, vmstat, web, yaml as tokenade_yaml, Savings,
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct RunResult {
    task: String,
    mechanism: String,
    repo: String,
    tokens_before: usize,
    tokens_after: usize,
    savings_pct: f64,
    millis: u128,
    quality: f64,
}

/// Run one compactor over `input` and time it.
///
/// Fifty-eight bench cases wrote these three lines by hand — `Instant::now`,
/// the call, `elapsed().as_millis()`. Fifty-eight chances to time something
/// subtly different from the others, in a harness whose whole job is to
/// compare timings.
fn measure(input: &str, compact: fn(&str) -> (String, Savings)) -> (String, Savings, u128) {
    let st = Instant::now();
    let (out, sav) = compact(input);
    (out, sav, st.elapsed().as_millis())
}

#[allow(clippy::too_many_arguments)]
fn record(
    results: &mut Vec<RunResult>,
    task: &str,
    mech: &str,
    repo: &str,
    before: &str,
    after: &str,
    ms: u128,
    quality: f64,
) {
    let tb = tokenizer::estimate(before);
    let ta = tokenizer::estimate(after);
    results.push(RunResult {
        task: task.into(),
        mechanism: mech.into(),
        repo: repo.into(),
        tokens_before: tb,
        tokens_after: ta,
        savings_pct: Savings::new(tb, ta).percent(),
        millis: ms,
        quality,
    });
}

// ---- realistic in-process baselines ---------------------------------------

// Use the index's exact skip-list so `adv:index-recall` compares like-for-like.
use tokenade_core::index::is_skip_dir_name;

fn walk_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(es) = fs::read_dir(&d) else { continue };
        for e in es.flatten() {
            let p = e.path();
            let n = e.file_name().to_string_lossy().to_string();
            if p.is_dir() {
                if !is_skip_dir_name(&n) && !n.starts_with('.') {
                    stack.push(p);
                }
            } else if let Some(ext) = p.extension().and_then(|x| x.to_str()) {
                if EXTS.contains(&ext) {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// In-process grep — the realistic baseline for "find this symbol". Returns
/// `file:line:matched_line` for every match, capped at 5k results (above which
/// real ripgrep would page anyway).
fn grep_repo(repo: &Path, pat: &Regex) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    for file in walk_files(repo) {
        let Ok(c) = fs::read_to_string(&file) else {
            continue;
        };
        if c.len() > 2_000_000 {
            continue;
        }
        let rel = file
            .strip_prefix(repo)
            .unwrap_or(&file)
            .display()
            .to_string();
        for (i, line) in c.lines().enumerate() {
            if pat.is_match(line) {
                use std::fmt::Write;
                let _ = writeln!(out, "{}:{}:{}", rel, i + 1, line);
                count += 1;
                if count >= 5000 {
                    return out;
                }
            }
        }
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let baseline_check = args.iter().any(|a| a == "--baseline-check");
    let bench_root = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .cloned()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("benchmarks/repos"));
    let fixtures = PathBuf::from("benchmarks/fixtures");
    let mut results = Vec::new();

    let mut repos: Vec<(String, PathBuf)> = Vec::new();
    if let Ok(es) = fs::read_dir(&bench_root) {
        for e in es.flatten() {
            if e.path().is_dir() {
                repos.push((e.file_name().to_string_lossy().to_string(), e.path()));
            }
        }
    }
    // Framework mini-repos under benchmarks/fixtures/frameworks/* exist only
    // to exercise the framework-aware ranking path. Real bench repos have no
    // package.json/manage.py at root, so without these the framework boost
    // would never trigger in the bench numbers.
    let fw_dir = fixtures.join("frameworks");
    if let Ok(es) = fs::read_dir(&fw_dir) {
        for e in es.flatten() {
            if e.path().is_dir() {
                repos.push((format!("fw:{}", e.file_name().to_string_lossy()), e.path()));
            }
        }
    }
    repos.sort();
    eprintln!(
        "repos ({}): {}",
        repos.len(),
        repos
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    for (name, path) in &repos {
        eprintln!("• {name}");
        let Ok(idx) = index::Index::build(path) else {
            continue;
        };
        task_nav_find_symbol(&mut results, name, path, &idx);
        task_nav_semantic(&mut results, name, path, &idx);
        task_overview_files(&mut results, name, path, &idx);
        task_review_skeleton(&mut results, name, path, &idx);
        task_filter_gitlog(&mut results, name, &fixtures);
    }
    task_filter_cargo(&mut results, &fixtures);
    task_filter_pytest(&mut results, &fixtures);

    // Web-content tasks (new in iter #3): real HTML + Playwright snapshot.
    task_web_html(&mut results, &fixtures, "wiki-software-engineering.html");
    task_web_html(&mut results, &fixtures, "wiki-tokenization.html");
    task_web_a11y(&mut results, &fixtures);

    // JSON/API tasks (new in iter #4): real fixtures from `gh` + cargo metadata.
    task_json(
        &mut results,
        &fixtures,
        "cargo-metadata.json",
        "tokenade-core",
    );
    task_json(&mut results, &fixtures, "gh-prs.json", "title");

    // Notebook tasks (new in iter #5): real .ipynb from firecrawl examples.
    task_notebook(&mut results, &fixtures, "firecrawl-big.ipynb");
    task_notebook(&mut results, &fixtures, "firecrawl-medium.ipynb");

    eprintln!("• adversarial tests");
    adv_filter_completeness(&mut results);
    adv_skeleton_fidelity(&mut results, &repos);
    adv_index_recall(&mut results, &repos);
    adv_index_unicode(&mut results);
    adv_web_broken_html(&mut results);
    adv_web_script_injection(&mut results);
    adv_json_hostile_keys(&mut results);
    adv_json_huge_15mb(&mut results, &fixtures);
    adv_notebook_base64_bomb(&mut results);
    adv_web_cloudflare_challenge(&mut results);
    adv_stealth_all_kinds(&mut results);
    adv_docs_docx_roundtrip(&mut results);
    adv_stack_user_frame_preserved(&mut results);
    adv_stack_mixed_language(&mut results);

    // Real iter #6 task: CSV compaction.
    task_csv(&mut results);
    // Real iter #7 task: stack-trace compaction.
    task_stacktrace_python(&mut results);
    // Real iter #8 task: kubectl YAML compaction.
    task_yaml_kubectl(&mut results);
    adv_yaml_managed_fields_bomb(&mut results);
    adv_yaml_pathological_indent(&mut results);
    // Iter #9: curl -v + generic logs.
    task_curl_verbose(&mut results);
    adv_logs_tls_bomb(&mut results);
    adv_logs_repeated_template(&mut results);
    // Iter #10: PDF extraction.
    task_pdf_real(&mut results);
    adv_pdf_huge(&mut results);
    adv_pdf_garbage_input(&mut results);
    // Iter #11: unified-diff / patch.
    task_diff_real(&mut results);
    adv_diff_huge_multi_file(&mut results);
    adv_diff_binary_in_middle(&mut results);
    // Iter #12: archive listings.
    task_archive_real(&mut results);
    adv_archive_huge(&mut results);
    adv_archive_pathological_names(&mut results);
    // Iter #13: terraform plan.
    task_terraform_real(&mut results);
    adv_terraform_refresh_bomb(&mut results);
    adv_terraform_no_changes(&mut results);
    // Iter #14: tabular CLI output.
    task_tabular_kubectl(&mut results);
    adv_tabular_docker_ps_wide(&mut results);
    adv_tabular_unknown_passthrough(&mut results);
    // Iter #15: auto-detect & dispatch.
    task_auto_dispatch(&mut results);
    adv_auto_ambiguous_json_with_trace(&mut results);
    adv_auto_tiny_input(&mut results);
    // Iter #16: env dump + secret redaction.
    task_env_dump(&mut results);
    adv_env_secret_leak(&mut results);
    adv_env_case_insensitive_secret(&mut results);
    // Iter #17: markdown documents.
    task_markdown_real(&mut results);
    adv_markdown_huge_code_block(&mut results);
    adv_markdown_link_reference_dump(&mut results);
    // Iter #18: lockfile flattening.
    task_lockfile_cargo(&mut results);
    task_lockfile_gosum(&mut results);
    adv_lockfile_huge_cargo(&mut results);
    adv_lockfile_unusual_names(&mut results);
    // Iter #19: SQL result tables.
    task_sql_psql(&mut results);
    adv_sql_unicode_and_nulls(&mut results);
    adv_sql_huge_mysql(&mut results);
    // Iter #20: Prometheus metrics.
    task_metrics_real(&mut results);
    adv_metrics_histogram_explosion(&mut results);
    adv_metrics_cardinality_blowup(&mut results);
    // Iter #21: nginx/Apache access logs.
    task_accesslog_real(&mut results);
    adv_accesslog_perf(&mut results);
    adv_accesslog_malformed_mix(&mut results);
    // Iter #22: systemd journal / dmesg.
    task_journal_real(&mut results);
    adv_journal_dmesg_huge(&mut results);
    adv_journal_mixed_garbage(&mut results);
    // Iter #23: JS/TS source maps (.map files).
    task_sourcemap_real(&mut results);
    adv_sourcemap_huge(&mut results);
    adv_sourcemap_truncated(&mut results);
    // Iter #24: OpenAPI / Swagger specs.
    task_openapi_real(&mut results);
    adv_openapi_huge(&mut results);
    adv_openapi_hostile(&mut results);
    // Iter #25: GraphQL SDL schemas.
    task_graphql_real(&mut results);
    adv_graphql_huge(&mut results);
    adv_graphql_hostile(&mut results);
    // Iter #26: strace / ltrace output.
    task_strace_real(&mut results);
    adv_strace_huge(&mut results);
    adv_strace_hostile(&mut results);
    // Iter #27: JVM build-tool output (Maven / Gradle / SBT).
    task_jvmbuild_real(&mut results);
    adv_jvmbuild_huge_mvn(&mut results);
    adv_jvmbuild_hostile(&mut results);
    // Iter #28: kubectl describe block format.
    task_kubectl_real(&mut results);
    adv_kubectl_huge_events(&mut results);
    adv_kubectl_hostile(&mut results);
    // Iter #29: Ansible playbook output.
    task_ansible_real(&mut results);
    adv_ansible_huge_recap(&mut results);
    adv_ansible_hostile(&mut results);
    // Iter #30: GitHub Actions workflow logs.
    task_ghactions_real(&mut results);
    adv_ghactions_huge_monorepo(&mut results);
    adv_ghactions_hostile(&mut results);
    // Iter #31: package-manager install output (pip / npm / yarn / pnpm).
    task_pkginstall_real(&mut results);
    adv_pkginstall_huge_pip(&mut results);
    adv_pkginstall_hostile(&mut results);
    // Iter #32: Kubernetes manifest YAML (multi-doc).
    task_k8smanifest_real(&mut results);
    adv_k8smanifest_huge(&mut results);
    adv_k8smanifest_hostile(&mut results);
    // Iter #33: git log / git show structured compaction.
    task_gitlog_real(&mut results);
    adv_gitlog_huge(&mut results);
    adv_gitlog_hostile(&mut results);
    // Iter #34: DNS lookup output (dig / nslookup / host).
    task_dns_real(&mut results);
    adv_dns_huge_trace(&mut results);
    adv_dns_hostile(&mut results);
    // Iter #35: JSON Lines / NDJSON cloud-log streams.
    task_ndjson_real(&mut results);
    adv_ndjson_huge_cloudwatch(&mut results);
    adv_ndjson_hostile(&mut results);
    // Iter #36: systemctl status block format.
    task_systemd_real(&mut results);
    adv_systemd_huge_journal(&mut results);
    adv_systemd_hostile(&mut results);
    // Iter #37: find / tree / ls -laR directory listings.
    task_filelist_real(&mut results);
    adv_filelist_huge_repo(&mut results);
    adv_filelist_hostile(&mut results);
    // Iter #38: du disk-usage output.
    task_disk_real(&mut results);
    adv_disk_huge_du(&mut results);
    adv_disk_hostile(&mut results);
    // Iter #39: grep / ripgrep / git grep search output.
    task_searchout_real(&mut results);
    adv_searchout_huge(&mut results);
    adv_searchout_hostile(&mut results);
    // Iter #40: netstat / ss / lsof network listings.
    task_netstat_real(&mut results);
    adv_netstat_huge_server(&mut results);
    adv_netstat_hostile(&mut results);
    // Iter #41: cargo bench / criterion output.
    task_cargobench_real(&mut results);
    adv_cargobench_huge(&mut results);
    adv_cargobench_hostile(&mut results);
    // Iter #42: multi-container k8s logs.
    task_containerlogs_real(&mut results);
    adv_containerlogs_huge(&mut results);
    adv_containerlogs_hostile(&mut results);
    // Iter #43: vmstat system monitoring.
    task_vmstat_real(&mut results);
    adv_vmstat_huge(&mut results);
    adv_vmstat_hostile(&mut results);
    // Iter #44: tcpdump packet capture.
    task_tcpdump_real(&mut results);
    adv_tcpdump_huge(&mut results);
    adv_tcpdump_hostile(&mut results);

    let out_dir = PathBuf::from("benchmarks");
    if baseline_check {
        // Compare this run against `benchmarks/results-baseline.json` and
        // fail (non-zero exit) if any scenario regressed by more than 5
        // percentage points or quality dropped below 0.99. Used in CI.
        let exit = run_baseline_check(&results, &out_dir);
        std::process::exit(exit);
    }
    if let Err(e) = write_outputs(&results, &out_dir) {
        eprintln!("write error: {e}");
    } else {
        eprintln!("→ wrote benchmarks/results.csv & results.json");
    }
    summarize(&results);
    run_competitor_bench(&fixtures, &out_dir);
}
