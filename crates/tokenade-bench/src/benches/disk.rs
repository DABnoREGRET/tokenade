//! `disk` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

pub(crate) fn synth_du(n_dirs: usize, files_per_dir: usize) -> String {
    let mut s = String::with_capacity(n_dirs * files_per_dir * 50);
    let sizes = ["4.0K", "12K", "1.5M", "8.0K", "256K", "2.0M", "512K"];
    for d in 0..n_dirs {
        for f in 0..files_per_dir {
            let size = sizes[(d + f) % sizes.len()];
            s.push_str(&format!("{size}\t./dir{d}/sub/file_{f}\n"));
        }
        // Per-directory aggregate.
        s.push_str(&format!("{}M\t./dir{d}\n", (d + 1) * 5));
    }
    // Tree root.
    s.push_str(&format!("{}M\t.\n", n_dirs * 5));
    s
}

pub(crate) fn task_disk_real(results: &mut Vec<RunResult>) {
    // 100 dirs × 10 files each + per-dir aggregates + tree root = 1101 entries.
    let raw = synth_du(100, 10);
    let (out, sav, ms) = measure(&raw, disk::compact_with_savings);
    let quality = if out.contains("[tokenade:disk] du, 1101 entries")
        && out.contains("largest entries:")
        && out.contains(".")
        // Top entries are the per-dir aggregates: 500M, 495M, 490M, ...
        // and the tree root which is largest at 500M.
        && out.contains("500M")
        && sav.percent() > 90.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:disk-compact",
        "disk",
        "synth-100dirs-10files",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #75: 50 000-entry du listing — must compact >98% in <500ms.
pub(crate) fn adv_disk_huge_du(results: &mut Vec<RunResult>) {
    let raw = synth_du(5000, 10);
    let (out, sav, ms) = measure(&raw, disk::compact_with_savings);
    let quality =
        if out.contains("[tokenade:disk] du, 55001 entries") && sav.percent() > 98.0 && ms < 500 {
            1.0
        } else {
            0.0
        };
    record(
        results,
        "adv:disk:huge-du-55k",
        "disk",
        "adversarial",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #76: hostile shape:
//   - mixed size suffixes (K/M/G/T), lowercase + uppercase
//   - Unicode paths
//   - bytes-only entries (no suffix)
//   - a few non-du lines mixed in (prose, df-style with leading `/`)
//   - tree root at `.`
pub(crate) fn adv_disk_hostile(results: &mut Vec<RunResult>) {
    // Real df-style lines have the FILESYSTEM name as the first field,
    // not the size — that's what makes them not du. The compactor's
    // `parse_du_line` rejects them via the `size_str.starts_with('/')`
    // guard. Mixed in with real du lines they should be silently skipped.
    let raw = "Some intro text we want to ignore\n\
4.0K\t./café/Müller.rs\n\
8.0k\t./café/déjà.rs\n\
12K\t./café\n\
1.5M\t./中文/数据.py\n\
256K\t./中文/测试.py\n\
2M\t./中文\n\
4.0k\t./🚀/launch.go\n\
8K\t./🚀/rocket.go\n\
12K\t./🚀\n\
1024\t./bytes-only-file\n\
2.5G\t./large/binary\n\
3.5G\t./large\n\
/dev/sda1       100G   45G   55G  45% /\n\
4G\t.\n\
5.0G\t./alt-tree\n\
Not a du line either\n";
    let (out, _sav, ms) = measure(raw, disk::compact_with_savings);
    let quality = if out.contains("[tokenade:disk] du,")
        && out.contains("café")
        && out.contains("中文")
        && out.contains("🚀")
        && out.contains("5.0G")
        && out.contains("3.5G")
        // df-style `/dev/sda1` line MUST NOT be counted as a du entry.
        && !out.contains("/dev/sda1")
        // Bytes-only entry recognised.
        && out.contains("1024")
        // Tree root `.` (4G) IS picked as the total.
        && out.contains("~4.0G total")
        && ms < 100
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:disk:hostile",
        "disk",
        "adversarial",
        raw,
        &out,
        ms,
        quality,
    );
}
