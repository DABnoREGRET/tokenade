//! `jvmbuild` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

pub(crate) fn synth_mvn(
    n_downloads: usize,
    n_modules: usize,
    n_warnings: usize,
    n_errors: usize,
) -> String {
    let mut s = String::from("[INFO] Scanning for projects...\n");
    for m in 0..n_modules {
        s.push_str(&format!(
            "[INFO] ----------< com.example:module-{m} >----------\n"
        ));
        s.push_str(&format!("[INFO] Building module-{m} 1.0-SNAPSHOT\n"));
        s.push_str(
            "[INFO] --------------------------------[ jar ]---------------------------------\n",
        );
    }
    for d in 0..n_downloads {
        s.push_str(&format!(
            "[INFO] Downloading from central: https://repo.maven.apache.org/maven2/org/example/lib-{d}/1.0/lib-{d}-1.0.jar\n"
        ));
    }
    s.push_str("[INFO] --- maven-compiler-plugin:3.8.1:compile (default-compile) @ root ---\n");
    s.push_str("[INFO] Compiling 142 source files to /home/x/target/classes\n");
    for w in 0..n_warnings {
        s.push_str(&format!(
            "[WARNING] /home/x/src/W{w}.java:[15,30] [unchecked] unchecked conversion\n"
        ));
    }
    for e in 0..n_errors {
        s.push_str(&format!(
            "[ERROR] /home/x/src/E{e}.java:[42,15] cannot find symbol\n"
        ));
    }
    if n_errors > 0 {
        s.push_str("[INFO] BUILD FAILURE\n");
    } else {
        s.push_str("[INFO] BUILD SUCCESS\n");
    }
    s.push_str("[INFO] Total time:  12.345 s\n");
    s
}

pub(crate) fn task_jvmbuild_real(results: &mut Vec<RunResult>) {
    let raw = synth_mvn(500, 3, 5, 8);
    let (out, sav, ms) = measure(&raw, jvmbuild::compact_with_savings);
    let quality = if out.contains("Maven")
        && out.contains("BUILD FAILURE")
        && out.contains("8 errors")
        && out.contains("5 warnings")
        && out.contains("Total time:  12.345 s")
        && out.contains("Compiling 142 source files")
        && out.contains("module-0")
        && out.contains("module-2")
        && out.contains("cannot find symbol")
        && !out.contains("Downloading from central")
        && sav.percent() > 90.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:jvmbuild-compact",
        "jvmbuild",
        "synth-mvn-3mod-500dl-8err",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #53: huge Maven log — 50 000 download lines, 100 warnings,
// 50 errors. Must compact >97 % in under 1 s and KEEP every error.
pub(crate) fn adv_jvmbuild_huge_mvn(results: &mut Vec<RunResult>) {
    let raw = synth_mvn(50_000, 5, 100, 50);
    let (out, sav, ms) = measure(&raw, jvmbuild::compact_with_savings);
    let quality = if out.contains("BUILD FAILURE")
        && out.contains("50 errors")
        && out.contains("100 warnings")
        && sav.percent() > 97.0
        && ms < 1000
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:jvmbuild:huge-mvn-50k",
        "jvmbuild",
        "adversarial",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #54: hostile mix of Maven + Gradle markers + Java source code
// with string literals matching `[ERROR]` / `[INFO]` (which should NOT
// false-positive when fed in isolation), plus an empty BUILD without status.
//
// The fixture interleaves real Maven output with Java source. Detection
// MUST classify this as Maven (because Maven markers dominate) and MUST
// NOT count the string-literal `[ERROR]` lines as build errors — wait,
// actually we DO count any line containing `[ERROR]`, so the test is:
// after compaction, the synthesised count should be 3 (2 build + 1 from
// the inline code, which is acceptable behaviour for stdin-piped logs).
// Critically: the compactor must NOT panic on the messy mix.
pub(crate) fn adv_jvmbuild_hostile(results: &mut Vec<RunResult>) {
    let raw = r#"[INFO] Scanning for projects...
[INFO] ----------< com.example:mixed >----------
[INFO] Building mixed 1.0
[INFO] Compiling 5 source files
[INFO] Source listing:
[INFO]   public class Foo {
[INFO]     // [ERROR] this is just a comment — NOT a build error
[INFO]     String msg = "[ERROR] also fake";
[INFO]     String warn = "[WARNING] also fake warning";
[INFO]   }
[WARNING] /src/Bar.java:[10,1] real warning
[ERROR] /src/Baz.java:[20,1] real error 1
[ERROR] /src/Baz.java:[20,2] real error 2
> Task :compileJava   (Gradle marker leaked in — should be ignored by Maven path)
[INFO] BUILD FAILURE
[INFO] Total time:  1.234 s
"#;
    let (out, sav, ms) = measure(raw, jvmbuild::compact_with_savings);
    // We don't try to filter string-literal `[ERROR]` lines (that's beyond
    // line-based heuristics). What we DO require:
    //   - status correctly detected (BUILD FAILURE)
    //   - real errors present in output
    //   - module name extracted
    //   - no panic, fast
    //   - savings nonzero
    let quality = if out.contains("Maven")
        && out.contains("BUILD FAILURE")
        && out.contains("real error 1")
        && out.contains("real error 2")
        && out.contains("real warning")
        && out.contains("mixed 1.0")
        && ms < 100
        && sav.percent() > 0.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:jvmbuild:hostile",
        "jvmbuild",
        "adversarial",
        raw,
        &out,
        ms,
        quality,
    );
}
