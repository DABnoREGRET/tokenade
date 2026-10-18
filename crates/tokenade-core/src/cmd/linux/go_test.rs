//! `go_test` — go test.
//!
//! Form-aware: keeps this command's specific detector, but folds its
//! output by detected shape — columnar table (surfaces problem rows),
//! key/value records (never cut mid-record), else head+tail (the final
//! outcome line is never decapitated). Replaces the blind head-slice.
const HEAD: usize = 60;

pub fn detect(text: &str) -> bool {
    text.contains("=== RUN ") || text.contains("--- PASS") || text.contains("--- FAIL") || text.contains("PASS") || text.contains("FAIL")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    if let Some(plain) = from_json(text) {
        return verbose(&plain)
            .or_else(|| crate::archetypes::form_aware(&plain, HEAD, 5, "go-test", "go test row(s)"))
            .or(Some(plain));
    }
    if text.contains("=== RUN") {
        if let Some(out) = verbose(text) {
            return Some(out);
        }
    }
    crate::archetypes::form_aware(text, HEAD, 5, "go-test", "go test row(s)")
}

/// **`go test -json` is the `-v` text, cut into events.** Each event's
/// `Output` is a piece of exactly what `go test -v` prints, in order; laid
/// back end to end it reads as `-v` and takes the same fold. Without it the
/// stream went to the shape fold: 300 tests, 1 500 events of
/// `{"Time":"<TS>","Action":"output",…}` templates (2 800 bytes) where `-v`
/// gives one summary line. A line that is not an event (a build error on
/// stderr) is kept where it came.
fn from_json(text: &str) -> Option<String> {
    let mut plain = String::new();
    let mut events = 0usize;
    for line in text.lines() {
        let event = line
            .starts_with('{')
            .then(|| serde_json::from_str::<serde_json::Value>(line).ok())
            .flatten()
            .filter(|v| v.get("Action").is_some());
        match event {
            Some(v) => {
                events += 1;
                if let Some(o) = v.get("Output").and_then(|o| o.as_str()) {
                    plain.push_str(o);
                }
            }
            None => {
                plain.push_str(line);
                plain.push('\n');
            }
        }
    }
    (events > 0).then_some(plain)
}

/// **`go test -v`: a failing test with WHY it failed; a passing one counted.**
///
/// The generic fold kept the `--- FAIL: TestCase150` row and dropped the line
/// above it — `bar_test.go:153: want 42, got 41`, the only line that says what
/// broke — then spent its head on 60 rows of `=== RUN`/`--- PASS` (measured on
/// a real 300-test package, 609 lines). Go writes a test's own output between
/// its `=== RUN` and its `--- FAIL`/`--- PASS`; that span is buffered per test
/// and printed only for a failure. Package lines (`FAIL pkg`, `ok pkg`, a
/// panic and its goroutine trace, a build error) are kept as written; a fuzz
/// run's progress keeps its last line, which carries the final counts.
fn verbose(text: &str) -> Option<String> {
    let mut buffers: std::collections::HashMap<String, Vec<&str>> = std::collections::HashMap::new();
    let mut current: Option<String> = None;
    let (mut passed, mut failed, mut skipped, mut hidden) = (0usize, 0usize, 0usize, 0usize);
    let mut out: Vec<String> = Vec::new();
    let mut fuzz: Option<(&str, usize)> = None;
    let flush_fuzz = |fuzz: &mut Option<(&str, usize)>, out: &mut Vec<String>| {
        if let Some((last, n)) = fuzz.take() {
            if n > 1 {
                out.push(format!("[{} fuzz progress line(s), the last:]", n));
            }
            out.push(last.to_string());
        }
    };
    for line in text.lines() {
        let t = line.trim_start();
        if let Some(name) = t.strip_prefix("=== RUN").or_else(|| t.strip_prefix("=== CONT")) {
            current = Some(name.trim().to_string());
            continue;
        }
        if t.starts_with("=== PAUSE") || t.starts_with("=== NAME") {
            continue;
        }
        let verdict = ["--- PASS: ", "--- FAIL: ", "--- SKIP: "].iter().find(|v| t.starts_with(**v));
        if let Some(v) = verdict {
            let name = t[v.len()..].split(" (").next().unwrap_or("").trim().to_string();
            let own = buffers.remove(&name).unwrap_or_default();
            match *v {
                "--- PASS: " => {
                    passed += 1;
                    hidden += own.len();
                }
                "--- FAIL: " => {
                    failed += 1;
                    out.extend(own.iter().map(|l| l.to_string()));
                    out.push(line.to_string());
                }
                _ => {
                    skipped += 1;
                    // The reason a test skipped is its last line of output.
                    let why = own.last().map(|l| format!(" — {}", l.trim())).unwrap_or_default();
                    out.push(format!("{line}{why}"));
                }
            }
            if current.as_deref() == Some(name.as_str()) {
                current = None;
            }
            continue;
        }
        if t.starts_with("fuzz: elapsed:") {
            fuzz = Some((line, fuzz.map_or(1, |(_, n)| n + 1)));
            continue;
        }
        match &current {
            Some(name) if line.starts_with("    ") || line.starts_with('\t') => buffers.entry(name.clone()).or_default().push(line),
            _ => {
                flush_fuzz(&mut fuzz, &mut out);
                out.push(line.to_string());
            }
        }
    }
    flush_fuzz(&mut fuzz, &mut out);
    // A test still open when the output ended (a panic, a timeout) is the one
    // that broke: its output is evidence, not noise.
    for (name, lines) in buffers {
        if !lines.is_empty() {
            out.push(format!("[output of {name}, which never reported a result:]"));
            out.extend(lines.iter().map(|l| l.to_string()));
        }
    }
    if passed + failed + skipped == 0 {
        return None;
    }
    let mut head = format!("[tokenade:go-test] {passed} passed, {failed} failed, {skipped} skipped");
    if hidden > 0 {
        head.push_str(&format!(" · {hidden} output line(s) of passing tests not shown"));
    }
    head.push('\n');
    let body = out.join("\n");
    let result = head + &body;
    (result.len() < text.len()).then_some(result)
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_json_stream_folds_like_its_verbose_text() {
        let ev = |action: &str, test: &str, output: &str| {
            let mut v = serde_json::json!({"Time": "2026-10-04T11:35:38.82+02:00", "Action": action, "Package": "ex/b"});
            if !test.is_empty() {
                v["Test"] = test.into();
            }
            if !output.is_empty() {
                v["Output"] = output.into();
            }
            v.to_string() + "\n"
        };
        let mut raw = String::from("# ex/b\nwarning: something on stderr\n");
        for i in 0..40 {
            let t = format!("TestCase{i}");
            raw += &ev("run", &t, "");
            raw += &ev("output", &t, &format!("=== RUN   {t}\n"));
            raw += &ev("output", &t, &format!("--- PASS: {t} (0.00s)\n"));
            raw += &ev("pass", &t, "");
        }
        raw += &ev("output", "TestDiv", "=== RUN   TestDiv\n");
        raw += &ev("output", "TestDiv", "    math_test.go:21: want 2, got 3\n");
        raw += &ev("output", "TestDiv", "--- FAIL: TestDiv (0.00s)\n");
        raw += &ev("fail", "TestDiv", "");
        raw += &ev("output", "", "FAIL\tex/b\t0.012s\n");
        raw += &ev("fail", "", "");
        let out = try_compact(&raw).expect("folded");
        assert!(out.starts_with("[tokenade:go-test] 40 passed, 1 failed, 0 skipped"), "{out}");
        assert!(out.contains("    math_test.go:21: want 2, got 3\n--- FAIL: TestDiv"), "{out}");
        assert!(out.contains("FAIL\tex/b\t0.012s") && out.contains("warning: something on stderr"), "{out}");
        assert!(!out.contains("\"Action\""), "{out}");
    }

    #[test]
    fn detector_rejects_empty() { assert!(!detect("")); }

    #[test]
    fn detector_accepts_specific_format() { assert!(detect("=== RUN  test output")); }

    #[test]
    fn short_output_passes_through() {
        let s: String = std::iter::repeat_n("=== RUN  test output", 3).map(|l| format!("{l}\n")).collect();
        assert!(try_compact(&s).is_none());
    }

    #[test]
    fn long_output_folds_non_decapitating() {
        let s: String = std::iter::repeat_n("=== RUN  test output", HEAD + 20).map(|l| format!("{l}\n")).collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("[tokenade:go-test"));
    }

    /// A real `go test -v ./...` (two packages): a failing subtest with its
    /// message, a skip with its reason, a panic with its trace.
    #[test]
    fn verbose_output_keeps_why_a_test_failed_and_counts_the_rest() {
        let raw = "=== RUN   TestTable\n=== RUN   TestTable/one\n=== RUN   TestTable/two\n    a_test.go:8: in=2 want=3\n=== RUN   TestTable/three\n--- FAIL: TestTable (0.00s)\n    --- PASS: TestTable/one (0.00s)\n    --- FAIL: TestTable/two (0.00s)\n    --- PASS: TestTable/three (0.00s)\n=== RUN   TestSkipped\n    a_test.go:12: needs docker\n--- SKIP: TestSkipped (0.00s)\n=== RUN   TestFine\n    a_test.go:13: fine\n--- PASS: TestFine (0.00s)\nFAIL\nFAIL\texample.com/m/a\t0.001s\n=== RUN   TestPanics\n--- FAIL: TestPanics (0.00s)\npanic: assignment to entry in nil map [recovered, repanicked]\n\ngoroutine 6 [running]:\nexample.com/m/b.TestPanics(0x14ae017dc248?)\n\t/src/b/b_test.go:5 +0x28\nFAIL\texample.com/m/b\t0.004s\nFAIL\n";
        assert_eq!(
            verbose(raw).unwrap(),
            "[tokenade:go-test] 3 passed, 3 failed, 1 skipped · 1 output line(s) of passing tests not shown\n\
             --- FAIL: TestTable (0.00s)\n    a_test.go:8: in=2 want=3\n    --- FAIL: TestTable/two (0.00s)\n\
             --- SKIP: TestSkipped (0.00s) — a_test.go:12: needs docker\nFAIL\nFAIL\texample.com/m/a\t0.001s\n\
             --- FAIL: TestPanics (0.00s)\npanic: assignment to entry in nil map [recovered, repanicked]\n\n\
             goroutine 6 [running]:\nexample.com/m/b.TestPanics(0x14ae017dc248?)\n\t/src/b/b_test.go:5 +0x28\n\
             FAIL\texample.com/m/b\t0.004s\nFAIL"
        );
    }

    #[test]
    fn a_fuzz_run_keeps_its_last_progress_line() {
        let mut raw = String::from("=== RUN   FuzzRev\n");
        for i in 0..40 {
            raw.push_str(&format!("fuzz: elapsed: {}s, execs: {} (1000/sec), new interesting: 7 (total: 8)\n", i * 3, i * 3000));
        }
        raw.push_str("--- PASS: FuzzRev (120.00s)\nPASS\nok  \texample.com/foo\t120.1s\n");
        let out = verbose(&raw).unwrap();
        assert!(out.contains("[40 fuzz progress line(s), the last:]\nfuzz: elapsed: 117s, execs: 117000"), "{out}");
        assert!(!out.contains("elapsed: 3s,"), "{out}");
    }
}
