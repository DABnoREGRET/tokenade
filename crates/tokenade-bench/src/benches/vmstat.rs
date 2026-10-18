//! `vmstat` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

pub(crate) fn synth_vmstat(n_samples: usize, peak_at: Option<usize>) -> String {
    let mut s = String::from("procs -----------memory---------- ---swap-- -----io---- -system-- ------cpu-----\n r  b   swpd   free   buff  cache   si   so    bi    bo   in   cs us sy id wa st\n");
    for i in 0..n_samples {
        let is_peak = peak_at == Some(i);
        let (us, sy, id, wa) = if is_peak {
            (75, 20, 5, 0)
        } else {
            (5 + (i % 5) as i64, 2 + (i % 3) as i64, 90, 0)
        };
        let id_actual = 100 - us - sy - wa;
        s.push_str(&format!(
            " 1  0      0 100000  20000 300000    0    0    10    20  100  200 {us} {sy} {id_actual} {wa}  0\n"
        ));
        let _ = id;
    }
    s
}

pub(crate) fn task_vmstat_real(results: &mut Vec<RunResult>) {
    // Realistic `vmstat 1 600` — 10 minutes of 1-second samples.
    let raw = synth_vmstat(600, Some(300));
    let (out, sav, ms) = measure(&raw, vmstat::compact_with_savings);
    let quality = if out.contains("[tokenade:vmstat] 600 sample(s)")
        && out.contains("per-column aggregates")
        // Header preserved.
        && out.contains("procs")
        && out.contains("memory")
        && out.contains("swpd")
        // First and last sample blocks.
        && out.contains("first sample")
        && out.contains("last sample")
        // Peak-CPU sample preserved (row at i=300 has us=75 sy=20).
        && out.contains("peak-CPU sample")
        && sav.percent() > 90.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:vmstat-compact",
        "vmstat",
        "synth-600samples-peak-at-300",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #85: huge vmstat run — 10000 samples (`vmstat 1 10000` =
// ~2.8 hour run). Must compact >99% in <500ms.
pub(crate) fn adv_vmstat_huge(results: &mut Vec<RunResult>) {
    let raw = synth_vmstat(10_000, Some(5_000));
    let (out, sav, ms) = measure(&raw, vmstat::compact_with_savings);
    let quality = if out.contains("10000 sample(s)") && sav.percent() > 99.0 && ms < 500 {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:vmstat:huge-10k",
        "vmstat",
        "adversarial",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #86: hostile content:
//   - vmstat header followed by lines with WRONG column counts (mid-run
//     `vmstat -t` re-emits the header)
//   - extreme values (very large numbers near i64::MAX-bound)
//   - prose interspersed (shouldn't break parsing)
//   - sample with all-zero values (idle system)
pub(crate) fn adv_vmstat_hostile(results: &mut Vec<RunResult>) {
    let raw = "Some intro prose\n\
procs -----------memory---------- ---swap-- -----io---- -system-- ------cpu-----\n\
 r  b   swpd   free   buff  cache   si   so    bi    bo   in   cs us sy id wa st\n\
 1  0      0 123456  78901 234567    0    0    12    34  100  200  5  3 92  0  0\n\
 0  0      0 100    200    300     0    0    10    20  100  200  5  3 92  0  0\n\
Some chatter between samples\n\
 0  0      0 999999999  88888 234567    0    0     0     0   50   80  1  1 98  0  0\n\
 2  1      0  50000  10000 100000    0    0  5000  6000 3000 5000 60 25  5 10  0\n\
 r  b   swpd   free   buff  cache   si   so    bi    bo   in   cs us sy id wa st\n\
 1  0      0  90000  18000 280000    0    0    15    25  120  220  7  4 89  0  0\n\
 0  0      0      0      0      0    0    0     0     0    0    0  0  0  0  0  0\n";
    let (out, _sav, ms) = measure(raw, vmstat::compact_with_savings);
    let quality = if out.contains("[tokenade:vmstat]")
        // 6 valid data rows: 5 normal + 1 all-zero. Prose + 2nd header are
        // skipped silently by parse_vmstat_row.
        && out.contains("6 sample(s)")
        && out.contains("per-column aggregates")
        // Peak-CPU sample (row with us=60 sy=25) detected.
        && out.contains("peak-CPU sample")
        // All-zero sample doesn't crash min/max math.
        && out.contains("first sample")
        && out.contains("last sample")
        && ms < 100
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:vmstat:hostile",
        "vmstat",
        "adversarial",
        raw,
        &out,
        ms,
        quality,
    );
}
