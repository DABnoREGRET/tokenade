//! `vitest_pass` — a vitest run where every test passed.
//!
//! What such a run prints is mostly NOT vitest: the code under test logs while
//! it runs (`[Nest] … WARN [TelemetrySink] …`, hundreds of lines), and the
//! runner adds only its `RUN` header and the tally at the very end. Measured
//! 2026-10-02 on a NestJS monorepo's `pnpm --filter <pkg> run test`: 612 lines,
//! folded to 271 (38 KB) by the generic dedup — still over the delivery
//! threshold, so stashed — and the agent's next call, four times out of four,
//! was `expand-ref <hash> --grep "Test Files|Tests  |FAIL"`. The answer it
//! wanted was the last five lines.
//!
//! When the tally says nothing failed and nothing errored, the application
//! output between the header and the tally is counted, by level, instead of
//! shown. Any failure, any `Errors` line, any `FAIL` / `Unhandled` block and
//! the run keeps its ordinary route — that output is then the evidence.

/// The tally block: from `Test Files` to the end.
fn tally_start(lines: &[&str]) -> Option<usize> {
    let i = lines.iter().rposition(|l| l.trim_start().starts_with("Test Files "))?;
    let tests = lines.get(i + 1)?.trim_start();
    (tests.starts_with("Tests ") && lines[i..].iter().any(|l| l.trim_start().starts_with("Duration "))).then_some(i)
}

fn all_passed(lines: &[&str], tally: usize) -> bool {
    let tally_clean = lines[tally..].iter().all(|l| {
        let t = l.trim_start();
        !t.contains(" failed") && !t.starts_with("Errors ")
    });
    let no_failure_block = lines[..tally].iter().all(|l| {
        let t = l.trim_start();
        !t.starts_with("FAIL ") && !t.starts_with("× ") && !t.contains("Unhandled") && !t.contains("Failed Tests")
    });
    tally_clean && no_failure_block
}

pub fn try_compact(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let tally = tally_start(&lines)?;
    if !all_passed(&lines, tally) {
        return None;
    }
    let run = lines[..tally].iter().position(|l| l.trim_start().starts_with("RUN "))?;
    let middle = &lines[run + 1..tally];
    let printed: Vec<&&str> = middle.iter().filter(|l| !l.trim().is_empty()).collect();
    if printed.len() < 10 {
        return None;
    }
    let level = |name: &str| printed.iter().filter(|l| l.contains(&format!(" {name} "))).count();
    let (errors, warns) = (level("ERROR"), level("WARN"));
    let mut out = String::new();
    for l in &lines[..=run] {
        out.push_str(l);
        out.push('\n');
    }
    out.push_str(&format!(
        "… {} line(s) printed by the code under test while it ran ({errors} at ERROR, {warns} at WARN) — \
         every test passed, so they are folded …\n",
        printed.len()
    ));
    for l in &lines[tally..] {
        out.push_str(l);
        out.push('\n');
    }
    Some(out.trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::try_compact;

    fn run(tally: &str, extra: &str) -> String {
        let mut s = String::from("> @orbit/control-plane@0.0.0 test apps/control-plane\n> vitest run\n\n RUN  v5.0.0 apps/control-plane\n\n");
        for i in 0..200 {
            let lvl = ["LOG", "WARN", "ERROR"][i % 3];
            s.push_str(&format!("[Nest] 10783{i:02}  - 02/10/2026 11:29:56    {lvl} [Svc{i}] message {i}\n"));
        }
        s.push_str(extra);
        s.push_str(tally);
        s
    }

    const PASSING: &str = " Test Files  179 passed | 22 skipped (201)\n      Tests  2281 passed | 191 skipped (2472)\n   Start at  11:29:56\n   Duration  8.49s (import 61%)\n";

    /// The measured case: the tally is the answer, and it stays whole.
    #[test]
    fn a_green_run_keeps_its_header_and_tally_and_counts_the_rest() {
        let out = try_compact(&run(PASSING, "")).expect("a green vitest run");
        assert!(out.contains(" RUN  v5.0.0"));
        assert!(out.contains("Test Files  179 passed | 22 skipped (201)"));
        assert!(out.contains("Tests  2281 passed | 191 skipped (2472)"));
        assert!(out.contains("200 line(s) printed") && out.contains("66 at ERROR"), "{out}");
        assert!(!out.contains("[Svc1]"));
    }

    /// A failure, an unhandled error: the output stays the evidence.
    #[test]
    fn a_failure_or_an_error_keeps_the_ordinary_route() {
        let failed = " Test Files  1 failed | 178 passed (179)\n      Tests  1 failed | 2280 passed (2281)\n   Duration  8.49s\n";
        assert!(try_compact(&run(failed, "")).is_none());
        let errored = format!("{PASSING}     Errors  1 error\n");
        assert!(try_compact(&run(&errored, "")).is_none());
        assert!(try_compact(&run(PASSING, " FAIL  src/a.test.ts > does a thing\n")).is_none());
        assert!(try_compact(&run(PASSING, "⎯⎯⎯ Unhandled Rejection ⎯⎯⎯\n")).is_none());
    }
}
