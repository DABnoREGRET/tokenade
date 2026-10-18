//! Line-based unified diff computation.
//!
//! Powers the "Read interceptor returns a diff when the file
//! barely changed" pipeline: when the agent re-reads a file we've
//! seen before but the new content is slightly different, we'd
//! rather feed back a tiny diff than the full new bytes.
//!
//! Algorithm: classic O(N×M) LCS via dynamic programming over the
//! lines, then a backtrack to build the edit sequence. Bounded at
//! [`LCS_MAX_LINES`] lines per side (above that the table allocates
//! megabytes and the diff is rarely a net token-win anyway — we
//! return `None` and let the caller passthrough).
//!
//! Format mirrors GNU `diff -u` minus the file headers (which the
//! caller adds because the framing differs per use site). Hunks have
//! one line of context above and below the changed region:
//!
//! ```text
//! @@ -42,3 +42,3 @@
//!  unchanged before
//! -removed
//! +added
//!  unchanged after
//! ```

use std::fmt::Write;
/// Maximum lines per side that we'll diff. Above this we return None
/// — LCS is O(N×M) so 5000×5000 = 25M comparisons + 25M·4 = 100 MB
/// dp table. Files that large rarely have small diffs anyway.
pub const LCS_MAX_LINES: usize = 5_000;

/// Default context lines around each changed hunk. One line is
/// enough for the agent to locate the hunk; more would inflate the
/// token cost without aiding comprehension.
pub const DEFAULT_CONTEXT: usize = 1;

/// Outcome of a diff attempt. `TooLarge` lets callers skip
/// passthrough decisions when one side is too big.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffOutcome {
    /// One or both inputs exceed [`LCS_MAX_LINES`].
    TooLarge,
    /// No changes — the inputs are identical line-by-line.
    Identical,
    /// At least one change — `text` is the unified diff body
    /// (without the `--- old\n+++ new\n` file headers), plus a count
    /// of added and removed lines for downstream win-ratio checks.
    Diff {
        text: String,
        added: usize,
        removed: usize,
    },
}

/// Compute a unified diff between `prior` and `current`. See
/// [`DiffOutcome`] for the result shape. The default context is
/// `DEFAULT_CONTEXT`; tune via [`unified_with_context`] if you need
/// more.
pub fn unified(prior: &str, current: &str) -> DiffOutcome {
    unified_with_context(prior, current, DEFAULT_CONTEXT)
}

/// Same as [`unified`] but with an explicit context-line count.
pub fn unified_with_context(prior: &str, current: &str, context: usize) -> DiffOutcome {
    let a: Vec<&str> = prior.lines().collect();
    let b: Vec<&str> = current.lines().collect();
    if a.len() > LCS_MAX_LINES || b.len() > LCS_MAX_LINES {
        return DiffOutcome::TooLarge;
    }
    if a == b {
        return DiffOutcome::Identical;
    }
    let ops = lcs_edits(&a, &b);
    let (text, added, removed) = format_unified(&ops, context);
    if added == 0 && removed == 0 {
        // Edge case: all-Same ops (shouldn't happen given a != b
        // check above, but defensive). Treat as identical.
        return DiffOutcome::Identical;
    }
    DiffOutcome::Diff {
        text,
        added,
        removed,
    }
}

/// One element of the edit script. The line-number fields use
/// 1-based indexing matching the GNU `diff` output convention.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Op<'a> {
    Same {
        text: &'a str,
        line_a: usize,
        line_b: usize,
    },
    Del {
        text: &'a str,
        line_a: usize,
    },
    Add {
        text: &'a str,
        line_b: usize,
    },
}

/// Build the edit script from `a` to `b` via LCS DP + backtrack.
fn lcs_edits<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<Op<'a>> {
    let n = a.len();
    let m = b.len();
    // Row-major flat dp table of size (n+1)*(m+1). u32 is plenty —
    // we already bounded inputs to 5000 lines.
    let stride = m + 1;
    let mut dp: Vec<u32> = vec![0; (n + 1) * stride];
    for i in 0..n {
        for j in 0..m {
            let idx = (i + 1) * stride + (j + 1);
            if a[i] == b[j] {
                dp[idx] = dp[i * stride + j] + 1;
            } else {
                let left = dp[(i + 1) * stride + j];
                let up = dp[i * stride + (j + 1)];
                dp[idx] = left.max(up);
            }
        }
    }

    // Backtrack from (n, m) to (0, 0). Same/Del/Add at each step.
    let mut ops: Vec<Op<'a>> = Vec::new();
    let mut i = n;
    let mut j = m;
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && a[i - 1] == b[j - 1] {
            ops.push(Op::Same {
                text: a[i - 1],
                line_a: i,
                line_b: j,
            });
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || dp[i * stride + (j - 1)] >= dp[(i - 1) * stride + j]) {
            ops.push(Op::Add {
                text: b[j - 1],
                line_b: j,
            });
            j -= 1;
        } else {
            ops.push(Op::Del {
                text: a[i - 1],
                line_a: i,
            });
            i -= 1;
        }
    }
    ops.reverse();
    ops
}

/// Render an edit script as GNU-`diff -u` style hunks (no file
/// headers; caller prepends those). Returns the body plus add/remove
/// line counts so callers can judge win ratios.
fn format_unified(ops: &[Op], context: usize) -> (String, usize, usize) {
    // First pass: locate the change runs and their hunk boundaries
    // (each change attracts `context` lines of Same on each side,
    // and adjacent hunks within 2*context lines coalesce).
    let mut change_idxs: Vec<usize> = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        if !matches!(op, Op::Same { .. }) {
            change_idxs.push(i);
        }
    }
    if change_idxs.is_empty() {
        return (String::new(), 0, 0);
    }

    let mut hunks: Vec<(usize, usize)> = Vec::new(); // (start, end) inclusive over ops
    let mut start = change_idxs[0].saturating_sub(context);
    let mut end = (change_idxs[0] + context).min(ops.len() - 1);
    for &idx in &change_idxs[1..] {
        let new_start = idx.saturating_sub(context);
        if new_start <= end + 1 {
            // Coalesce — extend the current hunk.
            end = (idx + context).min(ops.len() - 1);
        } else {
            hunks.push((start, end));
            start = new_start;
            end = (idx + context).min(ops.len() - 1);
        }
    }
    hunks.push((start, end));

    let mut out = String::new();
    let mut total_added = 0usize;
    let mut total_removed = 0usize;
    for (start, end) in hunks {
        // Compute hunk header line counts.
        let mut a_start = 0usize;
        let mut b_start = 0usize;
        let mut a_count = 0usize;
        let mut b_count = 0usize;
        for op in &ops[start..=end] {
            match op {
                Op::Same { line_a, line_b, .. } => {
                    if a_start == 0 {
                        a_start = *line_a;
                    }
                    if b_start == 0 {
                        b_start = *line_b;
                    }
                    a_count += 1;
                    b_count += 1;
                }
                Op::Del { line_a, .. } => {
                    if a_start == 0 {
                        a_start = *line_a;
                    }
                    a_count += 1;
                }
                Op::Add { line_b, .. } => {
                    if b_start == 0 {
                        b_start = *line_b;
                    }
                    b_count += 1;
                }
            }
        }
        let _ = writeln!(
            out,
            "@@ -{},{} +{},{} @@",
            a_start.max(1),
            a_count,
            b_start.max(1),
            b_count
        );
        for op in &ops[start..=end] {
            match op {
                Op::Same { text, .. } => {
                    out.push(' ');
                    out.push_str(text);
                    out.push('\n');
                }
                Op::Del { text, .. } => {
                    out.push('-');
                    out.push_str(text);
                    out.push('\n');
                    total_removed += 1;
                }
                Op::Add { text, .. } => {
                    out.push('+');
                    out.push_str(text);
                    out.push('\n');
                    total_added += 1;
                }
            }
        }
    }
    (out, total_added, total_removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_inputs_return_identical() {
        assert_eq!(unified("a\nb\nc\n", "a\nb\nc\n"), DiffOutcome::Identical);
        assert_eq!(unified("", ""), DiffOutcome::Identical);
    }

    #[test]
    fn too_large_returns_too_large() {
        let big: String = (0..6_000).map(|i| format!("line{i}\n")).collect();
        assert_eq!(unified(&big, "small\n"), DiffOutcome::TooLarge);
    }

    #[test]
    fn single_line_change_is_a_tight_hunk() {
        let prior = "alpha\nbeta\ngamma\ndelta\n";
        let current = "alpha\nBETA\ngamma\ndelta\n";
        match unified(prior, current) {
            DiffOutcome::Diff {
                text,
                added,
                removed,
            } => {
                assert_eq!(added, 1);
                assert_eq!(removed, 1);
                // 1 line of context above + 1 below.
                assert!(text.contains(" alpha"));
                assert!(text.contains("-beta"));
                assert!(text.contains("+BETA"));
                assert!(text.contains(" gamma"));
                // No `delta` — too far from the change.
                assert!(!text.contains("delta"));
                // Hunk header roughly correct.
                assert!(text.starts_with("@@ -"));
            }
            other => panic!("expected Diff, got {other:?}"),
        }
    }

    #[test]
    fn additions_only() {
        let prior = "a\nb\n";
        let current = "a\nb\nc\nd\n";
        match unified(prior, current) {
            DiffOutcome::Diff { added, removed, .. } => {
                assert_eq!(added, 2);
                assert_eq!(removed, 0);
            }
            other => panic!("expected Diff, got {other:?}"),
        }
    }

    #[test]
    fn deletions_only() {
        let prior = "a\nb\nc\nd\n";
        let current = "a\nb\n";
        match unified(prior, current) {
            DiffOutcome::Diff { added, removed, .. } => {
                assert_eq!(added, 0);
                assert_eq!(removed, 2);
            }
            other => panic!("expected Diff, got {other:?}"),
        }
    }

    #[test]
    fn distant_changes_produce_separate_hunks() {
        // Two changes 10 lines apart — should NOT coalesce at
        // context=1 (gap > 2*context+1).
        let prior: String = (1..=12).map(|i| format!("line{i}\n")).collect();
        let mut current_lines: Vec<String> = (1..=12).map(|i| format!("line{i}")).collect();
        current_lines[1] = "LINE2".to_string(); // change near top
        current_lines[10] = "LINE11".to_string(); // change near bottom
        let current = format!("{}\n", current_lines.join("\n"));
        match unified(&prior, &current) {
            DiffOutcome::Diff {
                text,
                added,
                removed,
            } => {
                assert_eq!(added, 2);
                assert_eq!(removed, 2);
                // Two `@@` headers means two hunks.
                let hunks = text.matches("@@ -").count();
                assert_eq!(hunks, 2, "expected 2 hunks, got:\n{text}");
            }
            other => panic!("expected Diff, got {other:?}"),
        }
    }

    #[test]
    fn adjacent_changes_coalesce() {
        // Changes on consecutive lines coalesce at context=1.
        let prior = "a\nb\nc\nd\ne\n";
        let current = "a\nB\nC\nd\ne\n";
        match unified(prior, current) {
            DiffOutcome::Diff { text, .. } => {
                assert_eq!(text.matches("@@ -").count(), 1, "should be 1 hunk:\n{text}");
            }
            other => panic!("expected Diff, got {other:?}"),
        }
    }

    #[test]
    fn diff_smaller_than_full_content_for_small_change() {
        // Sanity: the unified-diff body should be MUCH smaller than
        // the full new content for a 1-line tweak in a 50-line file.
        let prior: String = (1..=50).map(|i| format!("line{i}\n")).collect();
        let mut current = prior.clone();
        current = current.replace("line25", "LINE25");
        match unified(&prior, &current) {
            DiffOutcome::Diff { text, .. } => {
                assert!(
                    text.len() < current.len() / 5,
                    "diff ({} bytes) should be much smaller than current ({})",
                    text.len(),
                    current.len()
                );
            }
            other => panic!("expected Diff, got {other:?}"),
        }
    }
}
