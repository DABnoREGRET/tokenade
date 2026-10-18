//! `cargobench` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

pub(crate) fn synth_criterion(
    n_benches: usize,
    regression_every: usize,
    improvement_every: usize,
) -> String {
    let mut s = String::with_capacity(n_benches * 800);
    for i in 0..n_benches {
        let (change_low, change_mid, change_high, verdict) =
            if regression_every > 0 && i % regression_every == 0 {
                ("+5.0%", "+5.2%", "+5.5%", "Performance has regressed.")
            } else if improvement_every > 0 && i % improvement_every == 0 {
                ("-9.0%", "-8.7%", "-8.2%", "Performance has improved.")
            } else {
                (
                    "-1.0%",
                    "+0.1%",
                    "+1.0%",
                    "No change in performance detected.",
                )
            };
        s.push_str(&format!(
            "Benchmarking bench_{i}: Warming up for 3.0000 s\n"
        ));
        s.push_str(&format!(
            "Benchmarking bench_{i}: Collecting 100 samples in estimated 5.0000 s (10M iterations)\n"
        ));
        s.push_str(&format!("Benchmarking bench_{i}: Analyzing\n"));
        s.push_str(&format!(
            "bench_{i}      time:   [{}.234 µs {}.345 µs {}.456 µs]\n",
            i, i, i
        ));
        s.push_str(&format!(
            "                        change: [{change_low} {change_mid} {change_high}] (p = 0.01 < 0.05)\n"
        ));
        s.push_str(&format!("                        {verdict}\n"));
        s.push_str("Found 2 outliers among 100 measurements (2.00%)\n");
        s.push_str("  2 (2.00%) high mild\n");
        s.push('\n');
    }
    s
}

pub(crate) fn task_cargobench_real(results: &mut Vec<RunResult>) {
    // 50 criterion benchmarks with regression rate (every 10) and a
    // coprime improvement rate (every 7) so the two sets don't shadow.
    // Regressions: i ∈ {0, 10, 20, 30, 40} = 5.
    // Improvements: i ∈ {7, 14, 21, 28, 35, 42, 49} = 7 (since 7 and
    // 10 are coprime, none of these are also regressions).
    let raw = synth_criterion(50, 10, 7);
    let (out, sav, ms) = measure(&raw, cargobench::compact_with_savings);
    let quality = if out.contains("50 benchmark(s)")
        && out.contains("5 regression(s)")
        && out.contains("7 improvement(s)")
        && out.contains("bench_0")
        && out.contains("bench_49")
        // Verdict tags present in output.
        && out.contains("regressed")
        && out.contains("improved")
        && out.contains("no change")
        // Outlier chatter dropped.
        && !out.contains("outliers among")
        && !out.contains("high mild")
        // Warmup/Collecting chatter dropped.
        && !out.contains("Warming up")
        && !out.contains("Collecting 100 samples")
        && sav.percent() > 75.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:cargobench-compact",
        "cargobench",
        "synth-50bench-criterion",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #81: huge criterion run — 500 benchmarks × ~8 lines each.
// Must compact >85 % in <500 ms.
pub(crate) fn adv_cargobench_huge(results: &mut Vec<RunResult>) {
    let raw = synth_criterion(500, 25, 50);
    let (out, sav, ms) = measure(&raw, cargobench::compact_with_savings);
    let quality = if out.contains("500 benchmark(s)")
        && out.contains("20 regression(s)")
        && sav.percent() > 80.0
        && ms < 500
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:cargobench:huge-500",
        "cargobench",
        "adversarial",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #82: hostile mix of criterion + classic + edge cases:
//   - mixed criterion AND classic bench output in same input
//   - Unicode in benchmark names (中文_bench, café_bench, 🚀_bench)
//   - benchmarks with very long names (truncate gracefully)
//   - "Change within noise threshold" verdict variant
//   - benchmarks with no change line (just `time:` then next bench)
pub(crate) fn adv_cargobench_hostile(results: &mut Vec<RunResult>) {
    let raw = "Some intro prose to ignore\n\
Benchmarking café_bench/case1: Warming up for 3 s\n\
café_bench/case1      time:   [1.0 µs 1.5 µs 2.0 µs]\n\
                        change: [-1.0% +0.0% +1.0%] (p = 0.5 > 0.05)\n\
                        Change within noise threshold.\n\
Benchmarking 中文_bench: Warming up\n\
中文_bench      time:   [2.0 µs 2.5 µs 3.0 µs]\n\
                        change: [+5.0% +5.5% +6.0%] (p = 0.01 < 0.05)\n\
                        Performance has regressed.\n\
Benchmarking 🚀_rocket_bench: Warming up\n\
🚀_rocket_bench      time:   [10.0 ns 10.5 ns 11.0 ns]\n\
\n\
running 3 tests\n\
test foo                ... bench:           1,234 ns/iter (+/- 56)\n\
test bar                ... bench:          12,345 ns/iter (+/- 234)\n\
test very_long_name_that_goes_on_and_on_and_on_for_diagnostic_purposes ... bench:    99 ns/iter (+/- 1)\n\
\n\
test result: ok. 0 passed; 0 failed; 3 measured; 0 filtered out; finished in 1.23s\n";
    let (out, _sav, ms) = measure(raw, cargobench::compact_with_savings);
    // Expected: 3 criterion + 3 classic = 6 benchmarks.
    let quality = if out.contains("6 benchmark(s)")
        // Unicode names preserved.
        && out.contains("café_bench/case1")
        && out.contains("中文_bench")
        && out.contains("🚀_rocket_bench")
        // Verdict variants recognised.
        && out.contains("regressed")
        && out.contains("no change")  // "Change within noise threshold" → NoChange
        // Classic bench names preserved.
        && out.contains(" foo ")
        && out.contains(" bar ")
        // Long name preserved (not truncated by our column-width formatter).
        && out.contains("very_long_name_that_goes_on_and_on")
        // Times present.
        && out.contains("1.5 µs")
        && out.contains("1,234 ns/iter")
        // Final summary preserved.
        && out.contains("test result: ok")
        && ms < 100
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:cargobench:hostile",
        "cargobench",
        "adversarial",
        raw,
        &out,
        ms,
        quality,
    );
}
