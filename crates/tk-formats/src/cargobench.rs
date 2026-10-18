//! `cargo bench` / criterion output compaction.
//!
//! Two bench output shapes show up in practice:
//!
//! **criterion** (the modern Rust benchmarking tool) emits 8-12 lines per
//! benchmark:
//!
//! ```text
//! Benchmarking my_function/case_1: Warming up for 3.0000 s
//! Benchmarking my_function/case_1: Collecting 100 samples in estimated 5.0000 s (10M iterations)
//! Benchmarking my_function/case_1: Analyzing
//! my_function/case_1      time:   [1.234 µs 1.345 µs 1.456 µs]
//!                         change: [-2.5% +0.1% +3.2%] (p = 0.95 > 0.05)
//!                         No change in performance detected.
//! Found 3 outliers among 100 measurements (3.00%)
//!   2 (2.00%) high mild
//!   1 (1.00%) high severe
//! ```
//!
//! **classic cargo bench** (deprecated nightly path) emits one line per:
//!
//! ```text
//! running 5 tests
//! test foo  ... bench:           1,234 ns/iter (+/- 56)
//! test bar  ... bench:          12,345 ns/iter (+/- 234)
//!
//! test result: ok. 0 passed; 0 failed; 5 measured; 0 filtered out; finished in 6.31s
//! ```
//!
//! What the agent reading these wants:
//!
//!   - per-benchmark **point-estimate time** + **change-vs-baseline**
//!   - **classifications**: NEW / no-change / improved / regressed
//!   - **final summary** (count + finished in X)
//!
//! Drops: Warming up / Collecting samples / Analyzing chatter, outlier
//! breakdowns (the ` X (Y%) high mild ` lines), the verbose blank-line
//! padding between benchmark blocks.

use std::fmt::Write;

pub const BENCHMARKS_CAP: usize = 500;

#[derive(Default, Debug)]
struct Bench {
    name: String,
    time: String,
    change: Option<String>,
    verdict: Verdict,
}

#[derive(Default, Debug, Copy, Clone, PartialEq, Eq)]
enum Verdict {
    #[default]
    Unknown,
    NoChange,
    Improved,
    Regressed,
    New,
}

impl Verdict {
    fn as_str(self) -> &'static str {
        match self {
            Verdict::Unknown => "unknown",
            Verdict::NoChange => "no change",
            Verdict::Improved => "improved",
            Verdict::Regressed => "regressed",
            Verdict::New => "NEW",
        }
    }
}

pub fn detect(text: &str) -> bool {
    let head: &str = text.get(..text.len().min(8192)).unwrap_or(text);

    // criterion: `Benchmarking <name>:` markers OR `time:   [...]` patterns.
    if head.contains("\nBenchmarking ")
        || head.starts_with("Benchmarking ")
        || head.contains("                        change:")
        || head.contains("Found ") && head.contains("outliers among")
    {
        return true;
    }

    // classic cargo bench: line `running N tests` + lines with `bench:` and `ns/iter`.
    let has_classic_header = head.contains("running ") && head.contains(" tests\n");
    let has_classic_bench = head.contains(" ... bench:") && head.contains(" ns/iter");
    has_classic_bench
        && (has_classic_header
            || head
                .lines()
                .take(80)
                .filter(|l| l.contains("ns/iter"))
                .nth(2)
                .is_some())
}

/// Parse the `name      time:   [a b c]` line (criterion). The middle value
/// `b` is the point estimate.
fn parse_criterion_time_line(line: &str) -> Option<(&str, String)> {
    // Format: `<name>{whitespace}time:   [low mid high]`
    let time_idx = line.find("time:")?;
    let name = line[..time_idx].trim();
    if name.is_empty() {
        return None;
    }
    let after = &line[time_idx + "time:".len()..];
    let bracket_open = after.find('[')?;
    let bracket_close = after.find(']')?;
    let inner = &after[bracket_open + 1..bracket_close];
    let parts: Vec<&str> = inner.split_whitespace().collect();
    // [low_value low_unit mid_value mid_unit high_value high_unit] —
    // criterion mixes "1.234 µs" so each value is value+unit (2 tokens).
    // Easier: take the middle range as `mid_value mid_unit`.
    // Strip the typical 6-token layout.
    let mid = if parts.len() >= 6 {
        format!("{} {}", parts[2], parts[3])
    } else if parts.len() >= 3 {
        // 3 tokens: `value unit value unit value unit` collapsed somehow.
        // Take the middle pair if values share a unit.
        format!("{} {}", parts[1], parts[2])
    } else {
        inner.trim().to_string()
    };
    Some((name, mid))
}

/// Parse the `change: [low% mid% high%] (p = N > M)` line (criterion).
fn parse_criterion_change_line(line: &str) -> Option<String> {
    let change_idx = line.find("change:")?;
    let after = &line[change_idx + "change:".len()..];
    let bracket_open = after.find('[')?;
    let bracket_close = after.find(']')?;
    let inner = &after[bracket_open + 1..bracket_close];
    let parts: Vec<&str> = inner.split_whitespace().collect();
    // [-2.5% +0.1% +3.2%] → middle = parts[1]
    if parts.len() >= 3 {
        Some(parts[1].to_string())
    } else if parts.len() == 1 {
        Some(parts[0].to_string())
    } else {
        None
    }
}

/// Parse classic cargo bench line: `test foo  ... bench:    1,234 ns/iter (+/- 56)`.
fn parse_classic_bench_line(line: &str) -> Option<(String, String)> {
    let test_idx = line.find("test ")?;
    let bench_idx = line.find("... bench:")?;
    let name = line[test_idx + 5..bench_idx].trim().to_string();
    if name.is_empty() {
        return None;
    }
    let after = &line[bench_idx + "... bench:".len()..];
    let ns_idx = after.find("ns/iter")?;
    let time_str = after[..ns_idx].trim();
    // Strip commas from large numbers, add unit.
    Some((name, format!("{} ns/iter", time_str)))
}

pub fn compact(text: &str) -> String {
    if !detect(text) {
        return text.to_string();
    }
    // L42 — strip ANSI before parsing. `cargo bench` colourises by
    // default, and the criterion harness wraps numeric estimates in
    // colour codes that fool the regex / token boundary matchers.
    let stripped = crate::ansi::strip_ansi(text);
    let text = stripped.as_str();

    let mut benches: Vec<Bench> = Vec::new();
    let mut current: Option<Bench> = None;
    let mut final_summary: Option<String> = None;

    for raw in text.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Classic bench line is self-contained — emit immediately.
        if let Some((name, time)) = parse_classic_bench_line(trimmed) {
            // Flush any in-progress criterion bench first.
            if let Some(c) = current.take() {
                benches.push(c);
            }
            benches.push(Bench {
                name,
                time,
                change: None,
                verdict: Verdict::Unknown,
            });
            continue;
        }

        // Classic final summary line.
        if trimmed.starts_with("test result:") {
            final_summary = Some(trimmed.to_string());
            continue;
        }

        // criterion `Benchmarking <name>: <stage>` — drop chatter.
        if trimmed.starts_with("Benchmarking ") {
            continue;
        }

        // criterion outlier-stats lines — drop.
        if trimmed.starts_with("Found ") && trimmed.contains("outliers among") {
            continue;
        }
        if (trimmed.contains('(') && trimmed.contains("%) high "))
            || (trimmed.contains('(') && trimmed.contains("%) low "))
        {
            continue;
        }

        // criterion verdict lines (last line of a bench block).
        if trimmed.starts_with("No change in performance") {
            if let Some(c) = current.as_mut() {
                c.verdict = Verdict::NoChange;
            }
            continue;
        }
        if trimmed.starts_with("Change within noise threshold") {
            if let Some(c) = current.as_mut() {
                c.verdict = Verdict::NoChange;
            }
            continue;
        }
        if trimmed.starts_with("Performance has improved") {
            if let Some(c) = current.as_mut() {
                c.verdict = Verdict::Improved;
            }
            continue;
        }
        if trimmed.starts_with("Performance has regressed") {
            if let Some(c) = current.as_mut() {
                c.verdict = Verdict::Regressed;
            }
            continue;
        }

        // criterion `<name>  time: [..]` line — start new bench.
        if line.contains("time:") && line.contains('[') {
            if let Some((name, time)) = parse_criterion_time_line(line) {
                if let Some(c) = current.take() {
                    benches.push(c);
                }
                current = Some(Bench {
                    name: name.to_string(),
                    time,
                    change: None,
                    verdict: Verdict::New, // default until we see a change line
                });
                continue;
            }
        }

        // criterion `                  change: [..] (p ...)` line.
        if line.contains("change:") && line.contains('[') {
            if let Some(change) = parse_criterion_change_line(line) {
                if let Some(c) = current.as_mut() {
                    c.change = Some(change);
                    // Default verdict when change line is present but no
                    // verdict-text follows yet.
                    if c.verdict == Verdict::New {
                        c.verdict = Verdict::Unknown;
                    }
                }
                continue;
            }
        }

        // All other lines (blanks, prose) intentionally dropped.
    }
    if let Some(c) = current.take() {
        benches.push(c);
    }

    if benches.is_empty() {
        return text.to_string();
    }

    let total = benches.len();
    // A summary is only a summary of what it actually READ. This compactor
    // rebuilds from the bench lines it recognised and drops everything else,
    // so a file that merely CONTAINS a `cargo bench` capture came back as a
    // report of that capture with the rest of the file gone.
    //
    // Measured 2026-08-26: `crates/tokenade-core/tests/quality_contracts.rs`,
    // 37 916 bytes of Rust, delivered as 416 bytes — because one of its test
    // fixtures is a `r#"running 3 tests …"#` raw string. The source-quoting rule cannot
    // help here: inside a multi-line raw string the fixture's lines start at
    // column zero exactly like real output does.
    //
    // What separates them is COVERAGE. Real `cargo bench` output is bench
    // lines and little else; a source file quoting one is mostly something
    // else. Under a third and this is an island inside another document.
    // Measured 2026-08-26 over the corpus fixtures and the two offenders:
    // real `cargo bench` / criterion output runs **1.4 to 8.0** non-blank
    // lines per benchmark (criterion prints `Benchmarking`, `time:`,
    // `change:` and an outlier note for each). A source file that merely
    // QUOTES such a capture runs **26.6** (`src/cargobench.rs`, this very
    // module) and **333.7** (`tests/quality_contracts.rs`, whose fixture is
    // one `r#"running 3 tests …"#` raw string inside 1 001 lines of Rust).
    // 15 sits above every real reading and below both offenders.
    const LINES_PER_BENCH_CAP: usize = 15;
    let body_lines = text.lines().filter(|l| !l.trim().is_empty()).count();
    if total > 0 && body_lines > 40 && body_lines > total.saturating_mul(LINES_PER_BENCH_CAP) {
        crate::debug_log::log(
            "cargobench",
            "island_in_a_larger_document",
            serde_json::json!({ "benches": total, "body_lines": body_lines }),
        );
        return text.to_string();
    }
    let n_regressed = benches
        .iter()
        .filter(|b| b.verdict == Verdict::Regressed)
        .count();
    let n_improved = benches
        .iter()
        .filter(|b| b.verdict == Verdict::Improved)
        .count();
    let n_new = benches.iter().filter(|b| b.verdict == Verdict::New).count();

    let mut out = String::with_capacity(2048);
    let plural = |n: usize, s: &str| {
        if n > 1 {
            format!("{n} {s}s")
        } else {
            format!("{n} {s}")
        }
    };
    let _ = write!(out, "{total} benchmark(s)");
    if n_regressed > 0 {
        let _ = write!(out, ", {}", plural(n_regressed, "regression"));
    }
    if n_improved > 0 {
        let _ = write!(out, ", {}", plural(n_improved, "improvement"));
    }
    if n_new > 0 {
        let _ = write!(out, ", {n_new} new");
    }
    out.push('\n');
    if let Some(f) = &final_summary {
        let _ = writeln!(out, "{f}");
    }

    // Regressions block (the diagnostic gold — most important to surface).
    if n_regressed > 0 {
        let _ = write!(out, "\nregressions ({n_regressed}):\n");
        for b in benches.iter().filter(|b| b.verdict == Verdict::Regressed) {
            let change = b.change.as_deref().unwrap_or("?");
            let _ = writeln!(out, "  {:<40} {}  change: {change}", b.name, b.time);
        }
    }
    // Improvements block.
    if n_improved > 0 {
        let _ = write!(out, "\nimprovements ({n_improved}):\n");
        for b in benches.iter().filter(|b| b.verdict == Verdict::Improved) {
            let change = b.change.as_deref().unwrap_or("?");
            let _ = writeln!(out, "  {:<40} {}  change: {change}", b.name, b.time);
        }
    }

    // All benchmarks table (compact).
    let _ = write!(out, "\nall benchmarks ({total}):\n");
    for b in benches.iter().take(BENCHMARKS_CAP) {
        let verdict = b.verdict.as_str();
        let change = b.change.as_deref().unwrap_or("");
        let change_part = if change.is_empty() {
            String::new()
        } else {
            format!(" ({change})")
        };
        let _ = writeln!(out, "  {:<40} {}  [{verdict}]{change_part}", b.name, b.time);
    }
    if total > BENCHMARKS_CAP {
        let _ = writeln!(
            out,
            "  … {} more benchmark(s) elided",
            total - BENCHMARKS_CAP
        );
    }

    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CRITERION_SAMPLE: &str = r#"Benchmarking my_function/case_1: Warming up for 3.0000 s
Benchmarking my_function/case_1: Collecting 100 samples in estimated 5.0000 s (10M iterations)
Benchmarking my_function/case_1: Analyzing
my_function/case_1      time:   [1.234 µs 1.345 µs 1.456 µs]
                        change: [-2.5% +0.1% +3.2%] (p = 0.95 > 0.05)
                        No change in performance detected.
Found 3 outliers among 100 measurements (3.00%)
  2 (2.00%) high mild
  1 (1.00%) high severe

Benchmarking my_function/case_2: Warming up for 3.0000 s
Benchmarking my_function/case_2: Collecting 100 samples in estimated 5.0000 s (10M iterations)
Benchmarking my_function/case_2: Analyzing
my_function/case_2      time:   [2.345 µs 2.500 µs 2.700 µs]
                        change: [+4.5% +5.2% +6.0%] (p = 0.01 < 0.05)
                        Performance has regressed.

Benchmarking my_function/case_3: Warming up for 3.0000 s
my_function/case_3      time:   [3.456 µs 3.500 µs 3.600 µs]
                        change: [-9.0% -8.7% -8.2%] (p = 0.00 < 0.05)
                        Performance has improved.
"#;

    const CLASSIC_SAMPLE: &str = "running 5 tests
test foo                ... bench:           1,234 ns/iter (+/- 56)
test bar                ... bench:          12,345 ns/iter (+/- 234)
test baz                ... bench:         123,456 ns/iter (+/- 1,234)
test qux                ... bench:       1,234,567 ns/iter (+/- 12,345)
test quux               ... bench:      12,345,678 ns/iter (+/- 123,456)

test result: ok. 0 passed; 0 failed; 5 measured; 0 filtered out; finished in 6.31s
";

    #[test]
    fn detects_criterion() {
        assert!(detect(CRITERION_SAMPLE));
    }

    #[test]
    fn detects_classic() {
        assert!(detect(CLASSIC_SAMPLE));
    }

    #[test]
    fn rejects_random_text() {
        assert!(!detect(
            "Some prose mentioning benchmarks and changes but no real bench shape.\n"
        ));
    }

    #[test]
    fn rejects_code() {
        assert!(!detect(
            "fn bench() {\n    let time = compute_time();\n    println!(\"{time}\");\n}\n"
        ));
    }

    #[test]
    fn criterion_extracts_three_benches() {
        let out = compact(CRITERION_SAMPLE);
        assert!(out.contains("3 benchmark(s)"));
        assert!(out.contains("1 regression"));
        assert!(out.contains("1 improvement"));
        // Each benchmark named.
        assert!(out.contains("my_function/case_1"));
        assert!(out.contains("my_function/case_2"));
        assert!(out.contains("my_function/case_3"));
        // Times preserved.
        assert!(out.contains("1.345 µs"));
        assert!(out.contains("2.500 µs"));
        assert!(out.contains("3.500 µs"));
        // Change values preserved.
        assert!(out.contains("+5.2%"));
        assert!(out.contains("-8.7%"));
        // Verdict tags.
        assert!(out.contains("no change"));
        assert!(out.contains("regressed"));
        assert!(out.contains("improved"));
        // Outlier chatter dropped.
        assert!(!out.contains("outliers among"));
        assert!(!out.contains("high mild"));
        // Warmup/Collecting chatter dropped.
        assert!(!out.contains("Warming up"));
        assert!(!out.contains("Collecting 100 samples"));
        assert!(!out.contains("Analyzing"));
    }

    #[test]
    fn classic_extracts_five_benches() {
        let out = compact(CLASSIC_SAMPLE);
        assert!(out.contains("5 benchmark(s)"));
        // All 5 names.
        assert!(out.contains(" foo "));
        assert!(out.contains(" bar "));
        assert!(out.contains(" baz "));
        assert!(out.contains(" qux "));
        assert!(out.contains(" quux "));
        // Times preserved with `ns/iter`.
        assert!(out.contains("1,234 ns/iter"));
        assert!(out.contains("12,345,678 ns/iter"));
        // Final summary preserved.
        assert!(out.contains("test result: ok"));
    }

    #[test]
    fn unknown_passes_through() {
        let t = "not a bench output";
        assert_eq!(compact(t), t);
    }

    #[test]
    fn handles_regressions_only_block() {
        // 50 benches with 5 regressions.
        let mut s = String::new();
        for i in 0..50 {
            let change = if i < 5 { "+10.0%" } else { "+0.1%" };
            let verdict = if i < 5 {
                "Performance has regressed."
            } else {
                "No change in performance detected."
            };
            let _ = writeln!(s, "Benchmarking bench_{i}: Warming up for 3 s");
            let _ = writeln!(s, "bench_{i}      time:   [{i}.0 µs {i}.5 µs {i}.9 µs]");
            let _ = writeln!(
                s,
                "                        change: [{change} {change} {change}] (p = 0.01 < 0.05)"
            );
            let _ = writeln!(s, "                        {verdict}");
        }
        let out = compact(&s);
        assert!(out.contains("50 benchmark(s)"));
        assert!(out.contains("5 regression"));
        assert!(out.contains("regressions (5):"));
    }

    #[test]
    fn savings_huge_criterion() {
        // 200 benchmarks × 8 lines each = 1600 lines of mostly chatter.
        let mut s = String::new();
        for i in 0..200 {
            let _ = writeln!(s, "Benchmarking bench_{i}: Warming up for 3.0000 s");
            let _ = writeln!(s, "Benchmarking bench_{i}: Collecting 100 samples in estimated 5.0000 s (10M iterations)");
            let _ = writeln!(s, "Benchmarking bench_{i}: Analyzing");
            let _ = writeln!(s, "bench_{i}      time:   [1.0 µs 1.5 µs 2.0 µs]");
            s.push_str("                        change: [-1.0% +0.0% +1.0%] (p = 0.5 > 0.05)\n");
            s.push_str("                        No change in performance detected.\n");
            s.push_str("Found 2 outliers among 100 measurements (2.00%)\n");
            s.push_str("  2 (2.00%) high mild\n");
            s.push('\n');
        }
        let (out, sav) = compact_with_savings(&s);
        assert!(out.contains("200 benchmark(s)"));
        // Synth has ~8 lines/bench input, output is ~1.5 lines/bench
        // (per-bench summary row + small overhead). ~82% is the realistic
        // floor; production criterion runs with more verbose output land
        // higher.
        assert!(sav.percent() > 80.0, "got {sav:?}");
    }

    #[test]
    fn parse_criterion_time_line_works() {
        assert_eq!(
            parse_criterion_time_line(
                "my_function/case_1      time:   [1.234 µs 1.345 µs 1.456 µs]"
            ),
            Some(("my_function/case_1", "1.345 µs".to_string()))
        );
    }

    #[test]
    fn parse_criterion_change_line_works() {
        assert_eq!(
            parse_criterion_change_line(
                "                        change: [-2.5% +0.1% +3.2%] (p = 0.95 > 0.05)"
            ),
            Some("+0.1%".to_string())
        );
    }

    #[test]
    fn parse_classic_bench_line_works() {
        assert_eq!(
            parse_classic_bench_line(
                "test foo                ... bench:           1,234 ns/iter (+/- 56)"
            ),
            Some(("foo".to_string(), "1,234 ns/iter".to_string()))
        );
    }
}

#[cfg(test)]
mod a_capture_quoted_inside_a_larger_file {
    //! A summary is a summary of what it READ. Quoting a `cargo bench`
    //! capture inside a source file had the rest of that file replaced by a
    //! report of the capture: 37 916 bytes of Rust delivered as 416.

    fn source_quoting_a_capture() -> String {
        // The capture must sit INSIDE the detector's 8 KB head, or the file is
        // declined for the wrong reason and the fixture proves nothing.
        let mut s = String::from("//! A test module.\n\nuse std::fmt;\n\n");
        s.push_str("fn fixture() -> String {\n    String::from(\n        r#\"running 3 tests\ntest bench_a ... bench:       1,234 ns/iter (+/- 56)\ntest bench_b ... bench:       2,345 ns/iter (+/- 78)\ntest bench_c ... bench:       3,456 ns/iter (+/- 90)\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 3 measured; 0 filtered out\n\"#,\n    )\n}\n");
        for i in 0..800 {
            s.push_str(&format!("fn tail_{i}() -> usize {{ {i} }}\n"));
        }
        s
    }

    #[test]
    fn the_surrounding_file_is_not_thrown_away() {
        let src = source_quoting_a_capture();
        let out = super::compact(&src);
        assert_eq!(out, src, "three bench lines do not summarise 800 others");
    }

    #[test]
    fn a_real_capture_still_folds() {
        let mut run = String::from("running 50 tests\n");
        for i in 0..50 {
            run.push_str(&format!(
                "test bench_{i} ... bench:       {i},234 ns/iter (+/- 56)\n"
            ));
        }
        run.push_str(
            "\ntest result: ok. 0 passed; 0 failed; 0 ignored; 50 measured; 0 filtered out\n",
        );
        let out = super::compact(&run);
        assert_ne!(out, run, "real output must still be summarised");
        assert!(out.contains("50 benchmark(s)"), "{out}");
    }
}
