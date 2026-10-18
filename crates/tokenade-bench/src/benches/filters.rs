//! **The command compactors, and what they let through.**
//!
//! `cargo`, `pytest`, the router's coverage, and the index benches:
//! recall, unicode, skeleton fidelity. The number that counts is
//! RECALL — an index that answers fast and wrong costs more than no index at all.

use super::super::*;
use super::nav::decl_regex;

pub(crate) fn task_filter_cargo(results: &mut Vec<RunResult>, fixtures: &Path) {
    let log = fs::read_to_string(fixtures.join("cargo-check-ruff.log")).unwrap_or_default();
    if log.len() < 50 {
        return;
    }
    let t = Instant::now();
    let r = filter::filter_command(&["cargo".into(), "check".into()], &log);
    let ms = t.elapsed().as_millis();
    let (mut total, mut kept) = (0usize, 0usize);
    for line in log.lines() {
        let lt = line.trim_start();
        if lt.starts_with("error") || lt.starts_with("warning") {
            total += 1;
            if r.output.contains(lt) {
                kept += 1;
            }
        }
    }
    let quality = if total == 0 {
        1.0
    } else {
        kept as f64 / total as f64
    };
    record(
        results,
        "output:filter-cargo",
        "output-filter",
        "ruff",
        &log,
        &r.output,
        ms,
        quality,
    );
}

pub(crate) fn task_filter_pytest(results: &mut Vec<RunResult>, fixtures: &Path) {
    let log = fs::read_to_string(fixtures.join("pytest-collect.log")).unwrap_or_default();
    if log.len() < 50 {
        return;
    }
    let t = Instant::now();
    let r = filter::filter_command(&["pytest".into()], &log);
    let ms = t.elapsed().as_millis();
    // What an agent actually needs from `pytest --collect-only`:
    //   1. The `collected N items` header (counts incl. collection errors).
    //   2. Every explicit `FAILED` / `ERROR` line (real signal, not noise).
    //   3. At least *some* test IDs preserved (browse sample — agents don't
    //      need all 1500 IDs; a representative sample is sufficient).
    // So quality is binary on those three requirements, NOT
    // "every single <Function> line" which would forbid any truncation.
    let counts_required = log.lines().any(|l| l.contains("collected "));
    let counts_kept = !counts_required || r.output.contains("collected ");
    let failures: Vec<&str> = log
        .lines()
        .filter(|l| l.contains("FAILED") || l.contains("ERROR"))
        .collect();
    let failures_kept = failures.iter().all(|f| r.output.contains(f.trim()));
    let any_marker = log
        .lines()
        .any(|l| l.starts_with("<Module") || l.contains("<Function") || l.starts_with("<Class"));
    let sample_kept = !any_marker
        || r.output.contains("<Module")
        || r.output.contains("<Function")
        || r.output.contains("<Class");
    let quality = if counts_kept && failures_kept && sample_kept {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:filter-pytest",
        "output-filter",
        "click",
        &log,
        &r.output,
        ms,
        quality,
    );
}

// ---- adversarial tests (penalise dropped signal) --------------------------

pub(crate) fn adv_filter_completeness(results: &mut Vec<RunResult>) {
    // Synthetic but pathological — errors hidden in long noise, single
    // FAILED in a sea of passes, mixed git-status sections. The filter must
    // NEVER drop these required tokens.
    let cases: Vec<(&str, Vec<String>, String, Vec<&str>)> = vec![
        (
            "cargo:mid-log-error",
            vec!["cargo".into(), "build".into()],
            format!(
                "{}error[E0277]: trait bound failed\n  --> src/foo.rs:1:1\n{}",
                (0..200)
                    .map(|i| format!("   Compiling crate_{i}\n"))
                    .collect::<String>(),
                (0..200)
                    .map(|i| format!("   Compiling crate_{}\n", i + 200))
                    .collect::<String>()
            ),
            vec!["error[E0277]"],
        ),
        (
            "cargo-test:single-failure",
            vec!["cargo".into(), "test".into()],
            format!(
                "{}test result: FAILED. 99 passed; 1 failed; 0 ignored\n",
                (0..100)
                    .map(|i| format!("test t{i} ... ok\n"))
                    .collect::<String>()
            ),
            vec!["FAILED"],
        ),
        (
            "git-status:mixed-sections",
            vec!["git".into(), "status".into()],
            "On branch main\n\
             Changes not staged for commit:\n  modified:   src/foo.rs\n  modified:   src/bar.rs\n\
             Untracked files:\n  doc.md\n"
                .to_string(),
            vec!["src/foo.rs", "src/bar.rs", "doc.md"],
        ),
        (
            "pytest:rare-failure",
            vec!["pytest".into()],
            format!(
                "{}\n=== 1 failed, 99 passed in 1.23s ===\n",
                (0..99)
                    .map(|i| format!("test_x.py::test_{i} PASSED\n"))
                    .collect::<String>()
                    + "test_x.py::test_99 FAILED\n"
            ),
            vec!["FAILED"],
        ),
    ];
    for (name, argv, input, required) in cases {
        let r = filter::filter_command(&argv, &input);
        let kept = required.iter().filter(|m| r.output.contains(*m)).count();
        let quality = kept as f64 / required.len() as f64;
        record(
            results,
            &format!("adv:filter:{name}"),
            "output-filter",
            "adversarial",
            &input,
            &r.output,
            0,
            quality,
        );
    }

    // ---- New adversarial test (added by loop iteration): ----------------
    // FALSE-POSITIVE PRECISION. Lines that contain the word "error" but
    // are NOT real cargo errors (`errno: 0`, `no errors`, …) should NOT
    // be kept by the cargo filter. This deliberately stresses an over-
    // eager `starts_with("error")` heuristic.
    let real_err = "error[E0277]: the trait bound `Foo: Bar` is not satisfied";
    let fp_lines = [
        "errno: 0",
        "no errors detected",
        "errored=false in initialisation",
        "0 errors, 0 warnings",
        "error_count = 0;  // counter",
        "errors_total: 0",
        "error rate is acceptable",
    ];
    let mut input = String::new();
    for line in &fp_lines {
        // Two copies of each false-positive at column 0 — pure noise.
        input.push_str(line);
        input.push('\n');
        input.push_str(line);
        input.push('\n');
    }
    input.push_str(real_err);
    input.push('\n');
    for line in &fp_lines {
        input.push_str(line);
        input.push('\n');
    }
    let r = filter::filter_command(&["cargo".into(), "build".into()], &input);
    let required_kept = r.output.contains(real_err);
    // Bloat: how many of the 21 noise lines did the filter keep?
    let fp_kept: usize = fp_lines.iter().map(|fp| r.output.matches(fp).count()).sum();
    let total_fp = fp_lines.len() * 3; // each appears 3× in input
    let fp_rate = if total_fp == 0 {
        0.0
    } else {
        fp_kept as f64 / total_fp as f64
    };
    // Quality: required kept (must be), MINUS half the noise bloat.
    let q = if required_kept {
        (1.0 - 0.5 * fp_rate).clamp(0.0, 1.0)
    } else {
        0.0
    };
    record(
        results,
        "adv:filter:false-positive-precision",
        "output-filter",
        "adversarial",
        &input,
        &r.output,
        0,
        q,
    );
}

/// ADVERSARIAL (loop iteration #2): Unicode identifiers.
///
/// Rust and Python both allow non-ASCII identifiers per UAX #31. A real
/// codebase may use them (`fn 計算`, `def データ`, `class Café`). An ASCII-only
/// regex (`[A-Za-z_]`) silently misses these — a real coverage hole this test
/// surfaces. We materialise a tiny synthetic repo, build the index, and
/// assert every declared Unicode symbol shows up.
pub(crate) fn adv_index_unicode(results: &mut Vec<RunResult>) {
    let tmp = std::env::temp_dir().join(format!(
        "tokenade-adv-unicode-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::create_dir_all(&tmp);

    // Rust file with Unicode fn / struct names.
    let rs = "pub fn 計算(n: i32) -> i32 { n + 1 }\n\
              pub fn Café() {}\n\
              pub struct Données { x: i32 }\n\
              pub trait Größe { fn taille(&self) -> usize; }\n";
    let _ = fs::write(tmp.join("u.rs"), rs);

    // Python file with Unicode def / class names.
    let py = "def データ():\n    pass\n\n\
              async def 非同期処理():\n    pass\n\n\
              class Müller:\n    pass\n";
    let _ = fs::write(tmp.join("u.py"), py);

    let expected = [
        "計算",
        "Café",
        "Données",
        "Größe",
        "データ",
        "非同期処理",
        "Müller",
    ];

    let q = match index::Index::build(&tmp) {
        Ok(idx) => {
            let found: HashSet<&str> = idx.symbols.iter().map(|s| s.name.as_str()).collect();
            let kept = expected.iter().filter(|e| found.contains(*e)).count();
            kept as f64 / expected.len() as f64
        }
        Err(_) => 0.0,
    };
    let summary_before = format!("expected: {}", expected.join(", "));
    let summary_after = format!("recall={q:.2}");
    record(
        results,
        "adv:index:unicode-identifiers",
        "symbol-index",
        "adversarial",
        &summary_before,
        &summary_after,
        0,
        q,
    );

    let _ = fs::remove_dir_all(&tmp);
}

pub(crate) fn adv_skeleton_fidelity(results: &mut Vec<RunResult>, repos: &[(String, PathBuf)]) {
    let re = decl_regex();
    for (name, path) in repos {
        // Largest moderate-size source file in the repo.
        let mut best: Option<(PathBuf, u64)> = None;
        for f in walk_files(path).into_iter().take(2000) {
            if let Ok(md) = fs::metadata(&f) {
                let sz = md.len();
                if sz < 500_000 && best.as_ref().is_none_or(|(_, s)| sz > *s) {
                    best = Some((f, sz));
                }
            }
        }
        let Some((file, _)) = best else { continue };
        let Ok(full) = fs::read_to_string(&file) else {
            continue;
        };
        let sk = tokenade_core::skeleton::skeleton(&full);
        let mut seen: HashSet<String> = HashSet::new();
        let (mut total, mut kept) = (0usize, 0usize);
        for cap in re.captures_iter(&full) {
            let n = cap.get(1).unwrap().as_str();
            if !seen.insert(n.to_string()) {
                continue;
            }
            total += 1;
            if sk.contains(n) {
                kept += 1;
            }
        }
        let quality = if total == 0 {
            1.0
        } else {
            kept as f64 / total as f64
        };
        record(
            results,
            "adv:skeleton-fidelity",
            "context-compression",
            name,
            &full,
            &sk,
            0,
            quality,
        );
    }
}

pub(crate) fn adv_index_recall(results: &mut Vec<RunResult>, repos: &[(String, PathBuf)]) {
    for (name, path) in repos {
        let Ok(idx) = index::Index::build(path) else {
            continue;
        };
        let mut truth: HashSet<String> = HashSet::new();
        for f in walk_files(path).into_iter().take(3000) {
            let ext = f.extension().and_then(|x| x.to_str()).unwrap_or("");
            let Some(strict) = strict_for(ext) else {
                continue;
            };
            let Ok(c) = fs::read_to_string(&f) else {
                continue;
            };
            if c.len() > 2_000_000 {
                continue;
            }
            for cap in strict.captures_iter(&c) {
                if let Some(m) = cap.get(1).or_else(|| cap.get(2)) {
                    truth.insert(m.as_str().to_string());
                }
            }
        }
        let found: HashSet<String> = idx.symbols.iter().map(|s| s.name.clone()).collect();
        let total = truth.len();
        let kept = truth.iter().filter(|n| found.contains(*n)).count();
        let quality = if total == 0 {
            1.0
        } else {
            kept as f64 / total as f64
        };
        // Use stat strings on both sides so the record's savings field is
        // meaningless-but-non-zero rather than fake.
        let before = format!("strict-truth={total}");
        let after = format!("index-recall={kept}/{total}={quality:.2}");
        record(
            results,
            "adv:index-recall",
            "symbol-index",
            name,
            &before,
            &after,
            0,
            quality,
        );
    }
}

// ---- honest aggregation ---------------------------------------------------
