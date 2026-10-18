//! `git log` / `git show` structured compaction.
//!
//! `git log` (with or without `--stat`) on a busy repo emits one block per
//! commit:
//!
//! ```text
//! commit a1b2c3d4e5f6...                              (40-hex SHA at column 0)
//! Merge: f1e2 a3b4                                    (only on merge commits)
//! Author: Foo Bar <foo@example.com>
//! Date:   Mon Jan 15 10:30:00 2024 +0000
//!
//!     Implement feature X
//!
//!     Longer body, possibly many paragraphs, every line indented 4 spaces.
//!
//!  src/main.rs | 25 +++++++++++++++++++++++++              (only with --stat)
//!  src/lib.rs  |  5 +++--
//!  3 files changed, 35 insertions(+), 5 deletions(-)        (summary line)
//!
//! commit f1e2d3c4...
//! …
//! ```
//!
//! On a year of history that's easily 5 000 – 50 000 lines, ≥90 % of which
//! are body text + per-file stat bars the agent doesn't need. What the
//! agent reading the log wants:
//!
//!   - the **commit list** (short hash + author + date + subject line)
//!   - the **stat summary** (files changed, +ins, −del) per commit when
//!     `--stat` was supplied
//!   - merge markers
//!
//! Detection requires ≥2 `commit <hex>` headers at column 0, OR exactly 1
//! such header combined with `Author: ` + `Date:` lines (so a single
//! `git show` invocation routes here too). `git log -p` output, which
//! contains real diff hunks, is intercepted by the earlier `UnifiedDiff`
//! detection branch in `auto.rs` and never reaches this compactor.

use std::collections::HashSet;
use std::fmt::Write;

pub const COMMITS_CAP: usize = 500;
pub const SUBJECT_CHARS_CAP: usize = 100;
/// Per-commit cap on `--stat` filenames retained. The *names* are signal
/// ("which commit touched `auth.rs`?" is a top-tier log question, and the
/// whole point of `--stat`); the `+++---` bars are noise. We keep up to this
/// many names per commit and summarise the rest as `+K more`, so a commit that
/// rewrites 800 files can't blow up the compacted output.
pub const FILES_PER_COMMIT: usize = 6;

pub fn detect(text: &str) -> bool {
    let head: &str = text.get(..text.len().min(32_768)).unwrap_or(text);
    let commit_count = head.lines().filter(|l| is_commit_header(l)).count();
    commit_count >= 2
        || (commit_count == 1
            && (head.contains("\nAuthor: ") || head.starts_with("Author: "))
            && (head.contains("\nDate:") || head.starts_with("Date:")))
}

/// A line is a commit header iff it starts at column 0 with `commit `
/// followed by 7-64 hex chars. This rejects body lines (4-space indented)
/// that happen to mention the word "commit".
fn is_commit_header(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("commit ") else {
        return false;
    };
    let hash = rest.split_whitespace().next().unwrap_or("");
    (7..=64).contains(&hash.len()) && hash.chars().all(|c| c.is_ascii_hexdigit())
}

struct Commit {
    hash: String,
    author: String,
    date: String,
    subject: String,
    files_changed: Option<usize>,
    insertions: Option<usize>,
    deletions: Option<usize>,
    is_merge: bool,
    /// Filenames from per-file `--stat` lines, capped at [`FILES_PER_COMMIT`].
    files: Vec<String>,
    /// Total per-file lines seen (incl. those past the cap) for the `+K more`.
    files_seen: usize,
}

pub fn compact(text: &str) -> String {
    if !detect(text) {
        return text.to_string();
    }
    // L42 — strip ANSI. `git log --color=always` is common in CI
    // captures and developer aliases. Without stripping, the SHA
    // and `Author:` line parsers miss colour-wrapped tokens.
    let stripped = crate::ansi::strip_ansi(text);
    let text = stripped.as_str();

    let mut commits: Vec<Commit> = Vec::new();
    let mut unmatched: Vec<String> = Vec::new();
    let mut current: Option<Commit> = None;
    let mut subject_found = false;

    for line in text.lines() {
        if is_commit_header(line) {
            if let Some(c) = current.take() {
                commits.push(c);
            }
            let hash: String = line[7..]
                .split_whitespace()
                .next()
                .unwrap_or("")
                .chars()
                .take(12)
                .collect();
            current = Some(Commit {
                hash,
                author: String::new(),
                date: String::new(),
                subject: String::new(),
                files_changed: None,
                insertions: None,
                deletions: None,
                is_merge: false,
                files: Vec::new(),
                files_seen: 0,
            });
            subject_found = false;
            continue;
        }

        let Some(c) = current.as_mut() else {
            // Before the first `commit` header: another command's output
            // (`git status; git log …`) — keep instead of dropping.
            if !line.trim().is_empty() {
                unmatched.push(line.to_string());
            }
            continue;
        };

        if let Some(rest) = line.strip_prefix("Author: ") {
            let mut author = rest.trim().to_string();
            // Strip email part to keep the row narrow.
            if let Some(idx) = author.find(" <") {
                author.truncate(idx);
            }
            c.author = author;
            continue;
        }
        if let Some(rest) = line.strip_prefix("Date:") {
            c.date = rest.trim().to_string();
            continue;
        }
        if line.starts_with("Merge: ") {
            c.is_merge = true;
            continue;
        }
        if line.starts_with("CommitDate:") || line.starts_with("AuthorDate:") {
            // `git log --pretty=fuller` emits both — we don't separately
            // track them.
            continue;
        }

        // Subject: first non-blank indented (4-space) line in body.
        if !subject_found {
            if let Some(rest) = line.strip_prefix("    ") {
                let s = rest.trim();
                if !s.is_empty() {
                    let mut subject = s.to_string();
                    if subject.chars().count() > SUBJECT_CHARS_CAP {
                        // Char-boundary-safe truncation: find the byte
                        // index of the (cap+1)-th char.
                        let end = subject
                            .char_indices()
                            .nth(SUBJECT_CHARS_CAP)
                            .map(|(i, _)| i)
                            .unwrap_or(subject.len());
                        subject.truncate(end);
                        subject.push('…');
                    }
                    c.subject = subject;
                    subject_found = true;
                    continue;
                }
            }
        }

        // `--stat` summary line: ` N files changed, M insertions(+), K deletions(-)`.
        if line.starts_with(' ') && line.contains("file") && line.contains("changed") {
            let s = line.trim();
            // Parse leading integer from any of the three substrings.
            if let Some(n) = leading_int_before(s, " file") {
                c.files_changed = Some(n);
            }
            if let Some(n) = leading_int_before(s, " insertion") {
                c.insertions = Some(n);
            }
            if let Some(n) = leading_int_before(s, " deletion") {
                c.deletions = Some(n);
            }
            continue;
        }

        // Per-file `--stat` line: ` <path> | <n> <bars>` or ` <path> | Bin …`.
        // Single-space indent (body is 4-space) + a ` | ` whose right side
        // starts with a digit or `Bin`. We keep the *name*, drop the bars.
        if line.starts_with(' ') && !line.starts_with("    ") {
            if let Some(bar) = line.find(" | ") {
                let fname = line[..bar].trim();
                let after = line[bar + 3..].trim_start();
                let is_stat =
                    after.starts_with(|ch: char| ch.is_ascii_digit()) || after.starts_with("Bin");
                if !fname.is_empty() && is_stat {
                    c.files_seen += 1;
                    if c.files.len() < FILES_PER_COMMIT {
                        c.files.push(fname.to_string());
                    }
                    continue;
                }
            }
        }

        // Commit BODY text (4-space indented) and diff/-p content (diff/
        // index/@@/+/-/… shapes) are the compaction target — dropped as
        // before. An UNINDENTED line matching neither is the next command's
        // output (`git log -3; git status`) — keep it.
        let is_diff_shape = line.starts_with("diff --git")
            || line.starts_with("index ")
            || line.starts_with("@@")
            || line.starts_with('+')
            || line.starts_with('-')
            || line.starts_with("new file mode")
            || line.starts_with("deleted file mode")
            || line.starts_with("similarity index")
            || line.starts_with("rename ")
            || line.starts_with("Binary files ")
            || line.starts_with("\\ No newline");
        let is_body = line.starts_with("    ") || line.trim().is_empty();
        if !is_diff_shape && !is_body {
            unmatched.push(line.to_string());
        }
    }

    if let Some(c) = current.take() {
        commits.push(c);
    }

    let total = commits.len();
    let mut authors: HashSet<&str> = HashSet::new();
    for c in &commits {
        if !c.author.is_empty() {
            authors.insert(c.author.as_str());
        }
    }
    let merges = commits.iter().filter(|c| c.is_merge).count();

    let mut out = String::with_capacity(4096);
    let _ = writeln!(
        out,
        "{total} commits, {} author(s), {merges} merge(s)",
        authors.len()
    );
    if total > COMMITS_CAP {
        let _ = writeln!(out, "(cap reached: showing first {COMMITS_CAP} of {total})");
    }

    if !commits.is_empty() {
        out.push_str("\ncommits:\n");
        for c in commits.iter().take(COMMITS_CAP) {
            let short_date = short_date(&c.date);
            let merge_marker = if c.is_merge { " (merge)" } else { "" };
            let stat = match (c.files_changed, c.insertions, c.deletions) {
                (Some(n), i, d) => {
                    let i = i.unwrap_or(0);
                    let d = d.unwrap_or(0);
                    format!(" ({n} file{}, +{i} -{d})", plural(n))
                }
                _ => String::new(),
            };
            let subj = if c.subject.is_empty() {
                "(no message)"
            } else {
                &c.subject
            };
            let files = if c.files.is_empty() {
                String::new()
            } else {
                let extra = c.files_seen.saturating_sub(c.files.len());
                let more = if extra > 0 {
                    format!(" +{extra} more")
                } else {
                    String::new()
                };
                format!("  files: {}{more}", c.files.join(", "))
            };
            let _ = writeln!(
                out,
                "  {} [{}] {}  {}{}{}{}",
                c.hash, c.author, short_date, subj, merge_marker, stat, files
            );
        }
        if total > COMMITS_CAP {
            let _ = writeln!(out, "  … {} more commit(s) elided", total - COMMITS_CAP);
        }
    }

    if !unmatched.is_empty() {
        out.push('\n');
        out.push_str(&crate::auto::compact_remainder(&unmatched.join("\n")));
    }
    out.trim_end().to_string()
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// `leading_int_before(s, key)` — find `key` in `s`, walk backwards to
/// collect the contiguous digit run, parse it.
fn leading_int_before(s: &str, key: &str) -> Option<usize> {
    let idx = s.find(key)?;
    let prefix = &s[..idx];
    let digits: String = prefix
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    digits.parse::<usize>().ok()
}

/// Map a git Date: value to a 10-character `YYYY-MM-DD` string. Supports
/// the default `Mon Jan 15 10:30:00 2024 +0000` format and the
/// `--date=iso` / `--date=short` variants.
pub(crate) fn short_date(date: &str) -> String {
    let trimmed = date.trim();
    // Already ISO-shaped?
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 10 && bytes[0].is_ascii_digit() && bytes[4] == b'-' && bytes[7] == b'-' {
        return trimmed.chars().take(10).collect();
    }
    // Default format: "Mon Jan 15 10:30:00 2024 +0000"
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    if parts.len() >= 5 {
        let month = match parts[1] {
            "Jan" => "01",
            "Feb" => "02",
            "Mar" => "03",
            "Apr" => "04",
            "May" => "05",
            "Jun" => "06",
            "Jul" => "07",
            "Aug" => "08",
            "Sep" => "09",
            "Oct" => "10",
            "Nov" => "11",
            "Dec" => "12",
            _ => return trimmed.to_string(),
        };
        let day = parts[2];
        let year = parts[4];
        return format!("{year}-{month}-{day:0>2}");
    }
    trimmed.to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"commit a1b2c3d4e5f6789012345678901234567890abcd
Author: Foo Bar <foo@example.com>
Date:   Mon Jan 15 10:30:00 2024 +0000

    Implement feature X

    Longer body description.
    Multi-line paragraph.

 src/main.rs | 25 +++++++++++++++++++++++++
 src/lib.rs  |  5 +++--
 tests/x.rs  |  3 +++
 3 files changed, 33 insertions(+), 2 deletions(-)

commit f1e2d3c4b5a69788765432109876543210fedcba
Merge: deadbeef cafebabe
Author: Jane Doe <jane@example.com>
Date:   Sun Jan 14 09:00:00 2024 +0000

    Merge branch 'feature/y' into main

commit 0123456789abcdef0123456789abcdef01234567
Author: Müller-Lüdenscheid <m@example.com>
Date:   Sat Jan 13 12:34:56 2024 +0100

    Fix Unicode handling in tokenizer

 src/tokenizer.rs | 12 +++++++++---
 1 file changed, 9 insertions(+), 3 deletions(-)
"#;

    #[test]
    fn detects_multi_commit_log() {
        assert!(detect(SAMPLE));
    }

    #[test]
    fn detects_single_show() {
        let s = "commit abc1234567890abcdef\nAuthor: Foo Bar <foo@example.com>\nDate: Mon Jan 1 2024\n\n    Subject\n";
        assert!(detect(s));
    }

    #[test]
    fn rejects_random_text() {
        assert!(!detect(
            "commit yourself to the cause! Author of life. Date a friend.\n"
        ));
    }

    #[test]
    fn rejects_short_hex_only() {
        // `commit abc` has a 3-char hash, too short.
        assert!(!detect("commit abc\nAuthor: x\nDate: y\n"));
    }

    #[test]
    fn rejects_body_mentioning_commit() {
        // Body lines are 4-space indented; they should not match.
        assert!(!detect(
            "    commit deadbeef0123 was reverted\n    More body text\n"
        ));
    }

    #[test]
    fn extracts_three_commits() {
        let out = compact(SAMPLE);
        /* L122: banner removed; assertion deleted */
        assert!(out.contains("a1b2c3d4e5f6"));
        assert!(out.contains("f1e2d3c4b5a6"));
        assert!(out.contains("0123456789ab"));
        // Authors with email stripped.
        assert!(out.contains("[Foo Bar]"));
        assert!(out.contains("[Jane Doe]"));
        assert!(out.contains("[Müller-Lüdenscheid]"));
        // Email should NOT appear.
        assert!(!out.contains("foo@example.com"));
        assert!(!out.contains("jane@example.com"));
    }

    #[test]
    fn extracts_subjects_drops_body() {
        let out = compact(SAMPLE);
        assert!(out.contains("Implement feature X"));
        assert!(out.contains("Merge branch 'feature/y' into main"));
        assert!(out.contains("Fix Unicode handling in tokenizer"));
        // Body must NOT leak.
        assert!(!out.contains("Longer body description"));
        assert!(!out.contains("Multi-line paragraph"));
        // Per-file stat bars must NOT leak.
        assert!(!out.contains("+++++++++++++++++++++++++"));
        assert!(!out.contains(" src/main.rs |"));
    }

    #[test]
    fn extracts_stat_summary() {
        let out = compact(SAMPLE);
        assert!(out.contains("(3 files, +33 -2)"), "out: {out}");
        assert!(out.contains("(1 file, +9 -3)"), "out: {out}");
    }

    #[test]
    fn retains_stat_filenames_drops_bars() {
        let out = compact(SAMPLE);
        // The per-file *names* survive (answerability: "which commit touched X").
        assert!(out.contains("src/main.rs"), "out: {out}");
        assert!(out.contains("src/lib.rs"), "out: {out}");
        assert!(out.contains("src/tokenizer.rs"), "out: {out}");
        assert!(out.contains("files: "), "out: {out}");
        // …but the bar art and the raw ` path |` stat-line form do not.
        assert!(!out.contains("+++++++++++++++++++++++++"));
        assert!(!out.contains(" src/main.rs |"));
    }

    #[test]
    fn caps_filenames_per_commit_with_overflow() {
        let mut s = String::from(
            "commit a1b2c3d4e5f6789012345678901234567890abcd\nAuthor: Dev <d@e.com>\nDate:   Mon Jan 15 10:30:00 2024 +0000\n\n    Big refactor\n\n",
        );
        for i in 0..20 {
            let _ = writeln!(s, " src/file_{i:02}.rs | {} +++--", i + 1);
        }
        let _ = writeln!(s, " 20 files changed, 100 insertions(+), 40 deletions(-)");
        let out = compact(&s);
        // First FILES_PER_COMMIT names shown, the rest summarised.
        assert!(out.contains("src/file_00.rs"), "out: {out}");
        assert!(
            out.contains(&format!("+{} more", 20 - FILES_PER_COMMIT)),
            "out: {out}"
        );
        // A name well past the cap must NOT be listed verbatim.
        assert!(!out.contains("src/file_19.rs"), "out: {out}");
    }

    #[test]
    fn marks_merge_commits() {
        let out = compact(SAMPLE);
        assert!(out.contains("(merge)"));
    }

    #[test]
    fn converts_date_to_iso() {
        let out = compact(SAMPLE);
        assert!(out.contains("2024-01-15"));
        assert!(out.contains("2024-01-14"));
        assert!(out.contains("2024-01-13"));
    }

    #[test]
    fn handles_iso_date_format() {
        let s =
            "commit abc1234567890\nAuthor: x <x@y>\nDate:   2024-03-15 10:30:00 +0000\n\n    msg\n";
        let out = compact(s);
        assert!(out.contains("2024-03-15"));
    }

    #[test]
    fn unknown_passes_through() {
        let t = "this is just prose";
        assert_eq!(compact(t), t);
    }

    #[test]
    fn savings_huge_log() {
        let mut s = String::with_capacity(500_000);
        for i in 0..2_000 {
            let _ = write!(
                s,
                "commit {:040x}\nAuthor: Dev {i} <dev{i}@example.com>\nDate:   Mon Jan 15 10:30:00 2024 +0000\n\n    Commit message {i}\n\n    Detailed body line A for commit {i}\n    Detailed body line B for commit {i}\n    Detailed body line C for commit {i}\n\n src/file_{i}.rs | 10 ++++++----\n 1 file changed, 6 insertions(+), 4 deletions(-)\n\n",
                i
            );
        }
        let (out, sav) = compact_with_savings(&s);
        assert!(out.contains("2000 commits"));
        assert!(out.contains("(cap reached") || out.contains("more commit(s) elided"));
        assert!(sav.percent() > 75.0, "got {sav:?}");
    }

    #[test]
    fn handles_empty_subject() {
        // Commit with no body (rare but valid).
        let s = "commit abc1234567890\nAuthor: x <x@y>\nDate: Mon Jan 1 10:00:00 2024 +0000\n\ncommit def4567890abc\nAuthor: y <y@z>\nDate: Mon Jan 1 11:00:00 2024 +0000\n\n    Has a message\n";
        let out = compact(s);
        assert!(out.contains("(no message)"));
        assert!(out.contains("Has a message"));
    }
}
