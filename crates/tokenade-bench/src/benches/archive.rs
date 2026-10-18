//! `archive` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

/// 3000-entry `tar -tvf` listing across many directories — realistic shape
/// for a container layer dump or CI release tarball.
pub(crate) fn task_archive_real(results: &mut Vec<RunResult>) {
    let mut raw = String::new();
    for i in 0..3000 {
        let dir = match i % 5 {
            0 => "src",
            1 => "tests",
            2 => "vendor/lib",
            3 => "docs",
            _ => "node_modules/pkg",
        };
        raw.push_str(&format!(
            "-rw-r--r-- user/group {:>6} 2024-01-01 12:34 {dir}/file_{i}.txt\n",
            1000 + i
        ));
    }
    let (out, sav, ms) = measure(&raw, archive::compact_with_savings);
    // Quality: at least one path preserved AND the metadata columns
    // (perm/owner/date) MUST be dropped.
    let path_kept = out.contains("src/") || out.contains("entries grouped");
    let chrome_dropped = !out.contains("2024-01-01") && !out.contains("user/group");
    let quality = if path_kept && chrome_dropped {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:archive-compact",
        "archive",
        "tar-3000-entries",
        &raw,
        &out,
        ms,
        quality,
    );
    let _ = sav;
}

// Adversarial #23: a 50 000-entry tarball must compact in < 500 ms AND
// achieve > 95 % savings via top-dir grouping.
pub(crate) fn adv_archive_huge(results: &mut Vec<RunResult>) {
    let mut raw = String::with_capacity(50_000 * 80);
    for i in 0..50_000 {
        raw.push_str(&format!(
            "-rw-r--r-- user/group {:>6} 2024-01-01 12:34 a/b/c/file_{i}.txt\n",
            1000 + i
        ));
    }
    let (out, sav, ms) = measure(&raw, archive::compact_with_savings);
    let quality = if out.contains("a/") && sav.percent() > 95.0 && ms < 500 {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:archive:huge-50k-entries",
        "archive",
        "adversarial",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #24: filename with spaces, embedded quotes, and unicode. Path
// must survive, no panic, no corruption.
pub(crate) fn adv_archive_pathological_names(results: &mut Vec<RunResult>) {
    let raw = "-rw-r--r-- user/group 1234 2024-01-01 12:34 path with spaces/file.txt\n\
-rw-r--r-- user/group 5678 2024-01-01 12:34 café/résumé.pdf\n\
-rw-r--r-- user/group 9999 2024-01-01 12:34 a\\tab/embedded.bin\n\
-rw-r--r-- user/group   12 2024-01-01 12:34 \"weird\"-quotes/file.log\n";
    let (out, _sav, ms) = measure(raw, archive::compact_with_savings);
    let signal = out.contains("path with spaces")
        && out.contains("café")
        && out.contains("résumé.pdf")
        && out.contains("weird");
    let chrome_dropped = !out.contains("2024-01-01") && !out.contains("user/group");
    let quality = if signal && chrome_dropped { 1.0 } else { 0.0 };
    record(
        results,
        "adv:archive:pathological-names",
        "archive",
        "adversarial",
        raw,
        &out,
        ms,
        quality,
    );
}
