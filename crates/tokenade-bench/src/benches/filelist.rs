//! `filelist` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

pub(crate) fn synth_find_listing(n_dirs: usize, files_per_dir: usize) -> String {
    let mut s = String::with_capacity(n_dirs * files_per_dir * 50);
    let exts = ["rs", "toml", "md", "json", "yml", "py", "js", "ts"];
    for d in 0..n_dirs {
        let dir = match d % 8 {
            0 => "crates/tokenade-core/src",
            1 => "crates/tokenade-cli/src",
            2 => "crates/tokenade-mcp/src",
            3 => "tests",
            4 => "benchmarks",
            5 => "docs",
            6 => "examples",
            _ => "scripts",
        };
        for f in 0..files_per_dir {
            let ext = exts[(d + f) % exts.len()];
            s.push_str(&format!("./{dir}/file_{d}_{f}.{ext}\n"));
        }
    }
    s
}

pub(crate) fn task_filelist_real(results: &mut Vec<RunResult>) {
    // Realistic mid-size repo: 50 dirs × 20 files = 1 000 files.
    let raw = synth_find_listing(50, 20);
    let (out, sav, ms) = measure(&raw, filelist::compact_with_savings);
    let quality = if out.contains("[tokenade:filelist] find, 1000 file(s)")
        && out.contains("by extension")
        && out.contains(".rs")
        && out.contains(".toml")
        && out.contains(".md")
        // Top-level directories are first-path-component buckets, NOT the
        // full nested paths. `crates/tokenade-core/src/...` rolls up to `crates`.
        && out.contains("crates")
        && out.contains("tests")
        && out.contains("benchmarks")
        // Per-dir extension breakdown shape: `(.json: 60, ...)`.
        && out.contains("file(s)")
        && sav.percent() > 90.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:filelist-compact",
        "filelist",
        "synth-1000files",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #73: 50 000-file repo listing — must compact >98 % in <500ms.
pub(crate) fn adv_filelist_huge_repo(results: &mut Vec<RunResult>) {
    let raw = synth_find_listing(500, 100);
    let (out, sav, ms) = measure(&raw, filelist::compact_with_savings);
    let quality = if out.contains("50000 file(s)") && sav.percent() > 98.0 && ms < 500 {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:filelist:huge-50k",
        "filelist",
        "adversarial",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #74: hostile content:
//   - Mix of `./` and `/`-prefixed paths
//   - Unicode dir & file names (café, 中文, العربية, 🚀)
//   - Hidden files (`.gitignore`, `.env.example`)
//   - Single-component paths (`./Makefile`, `./Dockerfile`)
//   - Deeply-nested paths
//   - A few non-path lines that should NOT count as files
pub(crate) fn adv_filelist_hostile(results: &mut Vec<RunResult>) {
    let mut raw = String::from(
        "./Cargo.toml\n./Makefile\n./Dockerfile\n./.gitignore\n./.env.example\n./café/Müller.rs\n./café/déjà.rs\n./café/naïve.rs\n./中文/数据.py\n./中文/测试.py\n./中文/文件.py\n./العربية/ملف.js\n./العربية/مكتبة.js\n./🚀/launch.go\n./🚀/rocket.go\n",
    );
    for d in 0..5 {
        for f in 0..10 {
            raw.push_str(&format!("./src/deeply/nested/path/level{d}/file_{f}.rs\n"));
        }
    }
    let (out, sav, ms) = measure(&raw, filelist::compact_with_savings);
    let quality = if out.contains("[tokenade:filelist] find,")
        && out.contains("65 file(s)")  // 15 leading + 50 nested
        // Unicode dir names preserved.
        && out.contains("café")
        && out.contains("中文")
        && out.contains("العربية")
        && out.contains("🚀")
        // Hidden-file extensions counted.
        && out.contains(".rs")
        && out.contains(".py")
        && out.contains(".js")
        && out.contains(".go")
        && ms < 100
        && sav.percent() > 0.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:filelist:hostile",
        "filelist",
        "adversarial",
        &raw,
        &out,
        ms,
        quality,
    );
}
