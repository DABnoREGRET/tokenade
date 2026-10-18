//! `searchout` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

pub(crate) fn synth_grep(n_files: usize, matches_per_file: usize) -> String {
    let mut s = String::with_capacity(n_files * matches_per_file * 50);
    for f in 0..n_files {
        let path = match f % 5 {
            0 => format!("src/module_{f}/mod.rs"),
            1 => format!("tests/integration/test_{f}.rs"),
            2 => format!("benches/bench_{f}.rs"),
            3 => format!("examples/example_{f}.rs"),
            _ => format!("docs/guide_{f}.md"),
        };
        for m in 0..matches_per_file {
            let line = m * 7 + 1;
            s.push_str(&format!(
                "{path}:{line}:    fn handler_{m}() {{ do_thing() }}\n"
            ));
        }
    }
    s
}

pub(crate) fn task_searchout_real(results: &mut Vec<RunResult>) {
    // Realistic `grep -r foo .` shape on a real repo: a small number of
    // hot files with many matches each (most matches concentrate in a few
    // files). 20 files × 100 matches = 2 000 lines.
    let raw = synth_grep(20, 100);
    let (out, sav, ms) = measure(&raw, searchout::compact_with_savings);
    let quality = if out.contains("[tokenade:grep] 2000 match(es) in 20 file(s)")
        // Each file shown with its match count.
        && out.contains("src/module_0/mod.rs (100):")
        && out.contains("tests/integration/test_1.rs (100):")
        // Sample contents preserved (first match line shown).
        && out.contains("handler_0")
        // Per-file cap kicked in → 90 elided per file.
        && out.contains("more match(es) in this file elided")
        && sav.percent() > 80.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:searchout-compact",
        "searchout",
        "synth-20files-100matches",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #77: 50 000-line grep output across 500 files × 100 matches.
// Must compact >90% in <500ms.
pub(crate) fn adv_searchout_huge(results: &mut Vec<RunResult>) {
    let raw = synth_grep(500, 100);
    let (out, sav, ms) = measure(&raw, searchout::compact_with_savings);
    let quality =
        if out.contains("50000 match(es) in 500 file(s)") && sav.percent() > 90.0 && ms < 500 {
            1.0
        } else {
            0.0
        };
    record(
        results,
        "adv:searchout:huge-50k",
        "searchout",
        "adversarial",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #78: hostile content:
//   - Unicode in file paths AND content (café, 中文, 🚀, العربية)
//   - Code content with embedded colons (type annotations, URLs, JSON)
//   - Context lines (rg --context, uses `-` between linenum and content)
//   - Mix of paths with and without `./` prefix
//   - Real grep lines + a few non-grep prose lines (must be skipped)
pub(crate) fn adv_searchout_hostile(results: &mut Vec<RunResult>) {
    let raw = "Some intro prose that should be skipped\n\
src/café/Müller.rs:10:    let name: String = \"München\".to_string();\n\
src/café/Müller.rs:11:    let url: &str = \"http://example.com:8080/path\";\n\
src/café/Müller.rs:12-    // context line above\n\
src/café/Müller.rs:13:    println!(\"déjà-vu\");\n\
src/中文/数据.py:5:    def 計算(): return 42\n\
src/中文/数据.py:6-    # context line\n\
src/中文/数据.py:7:    x = \"中文字符\"\n\
src/中文/数据.py:8:    y: dict = {\"key\": \"value\"}\n\
src/🚀/launch.go:1:    // Rocket emoji\n\
src/🚀/launch.go:2:    var url string = \"https://api.example.com:443/launch\"\n\
src/🚀/launch.go:3:    var icon = \"🚀\"\n\
src/العربية/ملف.js:1:    const name = \"مكتبة\"\n\
src/العربية/ملف.js:2:    const url = \"https://example.com:443/api\"\n\
Some closing prose line\n";
    let (out, _sav, ms) = measure(raw, searchout::compact_with_savings);
    let quality = if out.contains("[tokenade:grep]")
        // All 4 unique files counted.
        && out.contains("4 file(s)")
        // Unicode paths preserved.
        && out.contains("src/café/Müller.rs")
        && out.contains("src/中文/数据.py")
        && out.contains("src/🚀/launch.go")
        && out.contains("src/العربية/ملف.js")
        // Unicode content preserved.
        && out.contains("München")
        && !out.contains("计算") // we test 計算 with traditional form
        && out.contains("計算")
        && out.contains("中文字符")
        && out.contains("🚀")
        && out.contains("مكتبة")
        // Content with colons (URLs, type annotations) intact.
        && out.contains("http://example.com:8080/path")
        && out.contains("https://api.example.com:443/launch")
        // Context lines (`-` separator) included.
        && out.contains("context line above")
        && ms < 100
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:searchout:hostile",
        "searchout",
        "adversarial",
        raw,
        &out,
        ms,
        quality,
    );
}
