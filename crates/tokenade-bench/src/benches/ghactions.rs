//! `ghactions` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

pub(crate) fn synth_ghactions(
    n_groups: usize,
    lines_per_group: usize,
    n_errors: usize,
    n_warnings: usize,
) -> String {
    let mut s = String::with_capacity(n_groups * lines_per_group * 80);
    let mut second = 0usize;
    let mut nano = 0usize;
    for g in 0..n_groups {
        s.push_str(&format!(
            "2024-01-15T10:{:02}:{:02}.{:07}Z ##[group]Run step-{g}\n",
            (second / 60) % 60,
            second % 60,
            nano
        ));
        nano += 1;
        for j in 0..lines_per_group {
            s.push_str(&format!(
                "2024-01-15T10:{:02}:{:02}.{:07}Z routine output line {j} for step-{g} with extra text to fill bytes\n",
                (second / 60) % 60,
                second % 60,
                nano
            ));
            nano += 1;
            if nano > 9_999_999 {
                nano = 0;
                second += 1;
            }
        }
        s.push_str(&format!(
            "2024-01-15T10:{:02}:{:02}.{:07}Z ##[endgroup]\n",
            (second / 60) % 60,
            second % 60,
            nano
        ));
        nano += 1;
    }
    for w in 0..n_warnings {
        s.push_str(&format!(
            "2024-01-15T10:30:00.{:07}Z ##[warning]Warning {w}: deprecated thing\n",
            nano
        ));
        nano += 1;
    }
    for e in 0..n_errors {
        s.push_str(&format!(
            "2024-01-15T10:30:00.{:07}Z ::error::file=src/file_{e}.py,line=42::ImportError {e}\n",
            nano
        ));
        nano += 1;
    }
    if n_errors > 0 {
        s.push_str(&format!(
            "2024-01-15T10:30:00.{:07}Z ##[error]Process completed with exit code 1.\n",
            nano
        ));
    }
    s
}

pub(crate) fn task_ghactions_real(results: &mut Vec<RunResult>) {
    // Realistic CI log: 25 groups × 30 lines/group + 3 errors + 5 warnings.
    let raw = synth_ghactions(25, 30, 3, 5);
    let (out, sav, ms) = measure(&raw, ghactions::compact_with_savings);
    let quality = if out.contains("[tokenade:ghactions] 25 groups, 4 errors, 5 warnings")
        && out.contains("conclusion: Process completed with exit code 1")
        && out.contains("Run step-0")
        && out.contains("Run step-24")
        && out.contains("ImportError 0")
        && out.contains("deprecated thing")
        && !out.contains("2024-01-15T")  // timestamps stripped
        && !out.contains("routine output")  // chatter dropped
        && sav.percent() > 90.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:ghactions-compact",
        "ghactions",
        "synth-25g-30lpg-3e-5w",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #59: a 100 000-line monorepo CI log — must compact >98 % in
// under 2 s, all groups + errors preserved up to cap.
pub(crate) fn adv_ghactions_huge_monorepo(results: &mut Vec<RunResult>) {
    let raw = synth_ghactions(500, 200, 10, 20);
    let (out, sav, ms) = measure(&raw, ghactions::compact_with_savings);
    let quality = if out.contains("500 groups, 11 errors, 20 warnings")
        && out.contains("Process completed with exit code 1")
        && sav.percent() > 98.0
        && ms < 2000
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:ghactions:huge-monorepo-100k",
        "ghactions",
        "adversarial",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #60: hostile input — log with multiple timestamp precisions
// (no-fraction, milliseconds, nanoseconds), ANSI color codes around marker
// directives, UTF-8 group names (emoji + non-ASCII), and a verbose `##[debug]`
// stream that should be dropped wholesale. Must NOT panic, must classify.
pub(crate) fn adv_ghactions_hostile(results: &mut Vec<RunResult>) {
    let raw = "2024-01-15T10:30:00Z ##[group]Setup 🚀\n\
2024-01-15T10:30:00.123Z some text inside\n\
2024-01-15T10:30:00.1234567Z ##[endgroup]\n\
2024-01-15T10:30:01Z \x1b[36m##[group]\x1b[0mRun tests émoji-test\n\
2024-01-15T10:30:01.001Z \x1b[31m##[error]\x1b[0mRed-coloured error in CI\n\
2024-01-15T10:30:01.002Z ##[debug]debug verbose noise that should be dropped\n\
2024-01-15T10:30:01.003Z ##[debug]more debug noise\n\
2024-01-15T10:30:01.004Z ##[endgroup]\n\
2024-01-15T10:30:02.0000001Z ::warning::file=src/中文.py,line=1::unicode file path\n\
2024-01-15T10:30:02.0000002Z ::notice::on ubuntu-latest with éàü\n\
2024-01-15T10:30:02.0000003Z ##[error]Process completed with exit code 2.\n";
    let (out, sav, ms) = measure(raw, ghactions::compact_with_savings);
    let quality = if out.contains("2 groups")
        // Group names with emoji + non-ASCII preserved.
        && out.contains("Setup 🚀")
        && out.contains("émoji-test")
        // Unicode in error annotation preserved.
        && out.contains("中文.py")
        && out.contains("éàü")
        // Conclusion + at least 2 errors detected.
        && out.contains("conclusion: Process completed with exit code 2")
        // Debug noise dropped.
        && !out.contains("debug verbose noise")
        && !out.contains("more debug noise")
        // No timestamps in output.
        && !out.contains("2024-01-15T")
        && ms < 100
        && sav.percent() > 0.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:ghactions:hostile",
        "ghactions",
        "adversarial",
        raw,
        &out,
        ms,
        quality,
    );
}
