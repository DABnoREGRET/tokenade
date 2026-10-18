//! `wt` (worktrunk) — its own lines, and the hooks it runs.
//!
//! ```text
//! ◎ Running pre-merge project:test
//!   python3 -m unittest -v
//! test_case_000 (test_calc.T.test_case_000) ... ok
//! …
//! ◎ Merging 1 commit to main @ 1a6b02e (no commit/squash/rebase needed)
//! ✓ Merged to main (1 commit, 1 file, +1)
//! ✗ Cannot remove worktree after merge: feat-b has uncommitted changes
//! ```
//!
//! Worktrunk's own report is a handful of lines, each one a step the caller
//! needs (`✓ Merged`, `✗ Cannot remove`). What makes a `wt merge` long is the
//! project's hooks — a test suite, a linter — whose output it relays whole.
//! `wt` was read as Windows Terminal's spec, a 50-line head and a tail:
//! measured 2026-10-05 on a real `wt merge -y` with a 60-test pre-merge hook,
//! 48 `… ok` lines kept and the `Ran 60 tests … OK` verdict cut, along with
//! `◎ Merging`. Across the fleet `wt` was 1.3 M uncovered tokens in 30 days.
//!
//! Each hook's output now goes through the compactor of the hook's OWN
//! command, exactly as if the agent had run it; worktrunk's lines stay as
//! they are. That compactor lives in the command layer above this one, so it
//! is PASSED in ([`compact_with`]): the command dispatch hands it over, and a
//! caller without it (`cmd::dispatch_named`) keeps each hook's output whole.

/// A line worktrunk writes itself: a status glyph at column 0.
fn is_wt_line(line: &str) -> bool {
    ["◎ ", "✓ ", "✗ ", "▲ ", "↳ ", "○ "].iter().any(|g| line.starts_with(g))
}

pub fn detect(text: &str) -> bool {
    let head: Vec<&str> = text.lines().take(40).collect();
    head.iter().any(|l| is_wt_line(l)) || head.first().is_some_and(|l| l.trim_start().starts_with("Branch  Status"))
}

crate::archetypes::detect_gated_try_compact!();

/// Worktrunk's lines kept, each hook's output left as it is.
pub fn compact(text: &str) -> String {
    compact_with(text, &|_, body| body.to_string())
}

/// Worktrunk's lines kept, each hook's output (`body`) replaced by what
/// `hook(command, body)` makes of it when that is shorter.
pub fn compact_with(text: &str, hook: &dyn Fn(&str, &str) -> String) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        out.push(line.to_string());
        i += 1;
        // `◎ Running <hook> <name>`, then the hook's command, indented.
        let Some(cmd) = line.strip_prefix("◎ Running ").and_then(|_| lines.get(i)).and_then(|l| l.strip_prefix("  ")) else {
            continue;
        };
        out.push(lines[i].to_string());
        i += 1;
        let start = i;
        while i < lines.len() && !is_wt_line(lines[i]) {
            i += 1;
        }
        let body = lines[start..i].join("\n");
        let folded = hook(cmd, &body);
        if folded.len() < body.len() {
            out.extend(folded.lines().map(str::to_string));
        } else {
            out.extend(lines[start..i].iter().map(|l| l.to_string()));
        }
    }
    out.join("\n")
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOOK: &str = include_str!("../../../tests/fixtures/linux/wt_merge_hook.txt");

    /// The way production reaches it: the command dispatch, which passes the
    /// hook compactor in.
    fn through_dispatch(text: &str) -> String {
        let argv: Vec<String> = ["wt", "merge", "-y"].iter().map(|s| s.to_string()).collect();
        crate::filter::filter_command(&argv, text).output
    }
    const SQUASH: &str = include_str!("../../../tests/fixtures/linux/wt_merge_squash.txt");
    const LIST: &str = include_str!("../../../tests/fixtures/linux/wt_list.txt");
    const FAIL: &str = include_str!("../../../tests/fixtures/linux/wt_merge_hook_fail.txt");

    #[test]
    fn a_hook_s_output_goes_through_its_own_command_s_compactor() {
        assert!(detect(HOOK));
        let out = through_dispatch(HOOK);
        // Every line worktrunk wrote is still there.
        for l in HOOK.lines().filter(|l| is_wt_line(l)) {
            assert!(out.lines().any(|o| o == l), "lost: {l}\n{out}");
        }
        assert!(out.contains("  python3 -m unittest -v"), "{out}");
        // The verdict survives; the 400 `… ok` lines do not.
        assert!(out.contains("Ran 400 tests"), "{out}");
        assert!(out.matches("... ok").count() < 10, "{out}");
        assert!(out.len() * 2 < HOOK.len(), "{} vs {}", out.len(), HOOK.len());
    }

    /// The hook failed: its failure is the hook compactor's to keep, and the
    /// `✗` line that ends the hook is worktrunk's — never folded into it.
    #[test]
    fn a_failing_hook_keeps_its_failure_and_worktrunk_s_verdict() {
        let out = through_dispatch(FAIL);
        for kept in ["FAIL: test_case_137", "AssertionError: 138 != 139", "FAILED (failures=1)"] {
            assert!(out.contains(kept), "{kept}: {out}");
        }
        let lines: Vec<&str> = out.lines().collect();
        let verdict = lines.iter().position(|l| l.starts_with("FAILED (failures=1)")).expect("verdict");
        assert_eq!(lines.get(verdict + 1), Some(&"✗ pre-merge command failed: test: exit status: 1"), "{out}");
    }

    /// Every glyph worktrunk opened a line with in the real captures ends a
    /// hook's block: a hook compactor that dropped trailing lines would
    /// otherwise take `✓ Merged` or `✗ … failed` with it.
    #[test]
    fn every_glyph_seen_in_the_captures_is_worktrunk_s() {
        for g in ["◎", "✓", "✗", "▲", "↳", "○"] {
            let seen = [HOOK, SQUASH, LIST, FAIL].iter().flat_map(|t| t.lines()).find(|l| l.starts_with(&format!("{g} ")));
            let line = seen.unwrap_or_else(|| panic!("{g} never seen"));
            assert!(is_wt_line(line), "{line}");
        }
    }

    #[test]
    fn worktrunk_s_own_report_is_kept_whole() {
        for text in [SQUASH, LIST] {
            assert!(detect(text), "{text}");
            assert_eq!(compact(text), text.trim_end_matches('\n'));
        }
    }
}
