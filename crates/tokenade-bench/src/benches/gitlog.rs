//! `gitlog` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

pub(crate) fn synth_gitlog(
    n_commits: usize,
    body_lines_per_commit: usize,
    with_stat: bool,
) -> String {
    let mut s = String::with_capacity(n_commits * 200);
    for i in 0..n_commits {
        s.push_str(&format!(
            "commit {:040x}\nAuthor: Dev {} <dev{}@example.com>\nDate:   Mon Jan 15 10:30:00 2024 +0000\n\n",
            i,
            i % 7,
            i % 7
        ));
        s.push_str(&format!("    Implement feature {i}\n"));
        for b in 0..body_lines_per_commit {
            s.push_str(&format!(
                "    Body line {b} of commit {i}: long descriptive text that fills bytes.\n"
            ));
        }
        s.push('\n');
        if with_stat {
            s.push_str(&format!(
                " src/feature_{i}.rs | 10 ++++++----\n tests/feature_{i}.rs | 5 +++--\n 2 files changed, 9 insertions(+), 6 deletions(-)\n\n"
            ));
        }
    }
    s
}

pub(crate) fn task_gitlog_real(results: &mut Vec<RunResult>) {
    // 100 commits × 10-line bodies + --stat per commit = ~30 lines/commit,
    // typical for `git log --stat -100` on a moderately-active repo.
    let raw = synth_gitlog(100, 10, true);
    let (out, sav, ms) = measure(&raw, gitlog::compact_with_savings);
    let quality = if out.contains("[tokenade:git-log] 100 commits, 7 unique authors")
        && out.contains("Implement feature 0")
        && out.contains("Implement feature 99")
        && out.contains("2024-01-15")
        && out.contains("(2 files, +9 -6)")
        && !out.contains("Body line 0")
        && !out.contains("Body line 9")
        && !out.contains("src/feature_") // per-file bar dropped
        && sav.percent() > 85.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:gitlog-compact",
        "gitlog",
        "synth-100commits-10body-stat",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #65: huge git log — 5 000 commits with --stat + 5-line bodies.
// Must compact >85 % in under 1 s.
pub(crate) fn adv_gitlog_huge(results: &mut Vec<RunResult>) {
    let raw = synth_gitlog(5_000, 5, true);
    let (out, sav, ms) = measure(&raw, gitlog::compact_with_savings);
    let quality = if out.contains("5000 commits") && sav.percent() > 85.0 && ms < 1000 {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:gitlog:huge-5k-commits",
        "gitlog",
        "adversarial",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #66: hostile content — commit bodies contain text that LOOKS
// like git log markers (`commit <hex>`, `Author:`, `Date:`) but is indented
// (body context), Unicode author + Unicode subject, merge commit, commit
// with empty message, very long subject (>100 chars, must be truncated),
// non-default date format (--date=iso), non-default date format (--date=short).
pub(crate) fn adv_gitlog_hostile(results: &mut Vec<RunResult>) {
    let long_subject: String = "x".repeat(150);
    let raw = format!(
        "commit a1b2c3d4e5f6789012345678901234567890abcd\nAuthor: Müller-Lüdenscheid <m@example.com>\nDate:   2024-03-15 10:30:00 +0000\n\n    Fix Unicode handling: café, naïve, 中文\n\n    Body containing fake markers (indented, so they should NOT be parsed):\n    commit deadbeefcafebabe1234567890abcdef01234567\n    Author: Faux Devil <evil@example.com>\n    Date: Bad date format\n    Merge: feedface deadc0de\n\ncommit 0123456789abcdef0123456789abcdef01234567\nMerge: aaaa bbbb\nAuthor: Jane Doe <jane@example.com>\nDate:   Sun Jan 14 09:00:00 2024 +0000\n\n    Merge branch 'feature/y' into main\n\ncommit fedcba9876543210fedcba9876543210fedcba98\nAuthor: x <x@y>\nDate:   2024-02-01\n\n    {long_subject}\n\ncommit aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\nAuthor: No Body <nb@example.com>\nDate:   2024-01-01\n\ncommit bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\nAuthor: Last Author <last@example.com>\nDate:   2024-01-02 12:00:00\n\n    Real commit message\n\n test.rs | 1 +\n 1 file changed, 1 insertion(+)\n"
    );
    let (out, sav, ms) = measure(&raw, gitlog::compact_with_savings);
    let quality = if out.contains("5 commits")
        && out.contains("5 unique authors")
        && out.contains("1 merge")
        // Unicode author + subject preserved.
        && out.contains("[Müller-Lüdenscheid]")
        && out.contains("Fix Unicode handling: café, naïve, 中文")
        // ISO date format normalised.
        && out.contains("2024-03-15")
        // Embedded fake commit MUST NOT be parsed as separate commit.
        && !out.contains("deadbeefcafebabe")
        && !out.contains("Faux Devil")
        // Long subject truncated with ellipsis.
        && out.contains("…")
        && !out.contains(&"x".repeat(120))
        // Merge marker.
        && out.contains("(merge)")
        // Empty-body commit handled.
        && out.contains("(no message)")
        && ms < 100
        && sav.percent() > 0.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:gitlog:hostile",
        "gitlog",
        "adversarial",
        &raw,
        &out,
        ms,
        quality,
    );
}
