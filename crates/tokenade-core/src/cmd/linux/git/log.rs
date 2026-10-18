//! `git log` — all variants.
//!
//! Variants we recognise:
//!
//! - default — `commit <hash>\nAuthor: …\nDate: …\n    <subject>`
//!   blocks. Parsed to `<short-hash> <subject>` pairs.
//! - `--oneline` / `--pretty=oneline` / `--pretty=format:<fmt>` /
//!   `--format=<fmt>` — one line per commit, already compact.
//! - `--graph` — ASCII tree characters carry topology; we never
//!   collapse, only cap.
//! - `--stat` / `--shortstat` / `--name-only` / `--name-status` —
//!   commit + per-file delta info; both preserved.
//! - `-p` / `--patch` / `--cc` — full diff per commit; the diff
//!   portion is fed back through the `git diff` compactor (still in
//!   `filter.rs` until phase 2 of the refactor lands).
//!
//! All helpers (`has_flag`, `truncate_middle`, …) come from
//! [`crate::filter`] via `pub(crate)` exports.

use crate::compress::generic;
use crate::argv::{flag_value, has_any_flag, has_flag, has_flag_prefix};
use crate::compress::truncate_middle;
use crate::ansi::strip_ansi;
use std::fmt::Write;

/// Top-level dispatcher. Picks the per-variant impl based on argv.
pub fn dispatch(argv: &[String], output: &str) -> String {
    if has_any_flag(argv, &["-p", "--patch", "--cc"]) {
        return with_patch(output);
    }
    if has_flag(argv, "--graph") {
        return graph(output);
    }
    if has_any_flag(argv, &["--stat", "--shortstat", "--name-only", "--name-status"]) {
        return with_files(output);
    }
    if has_flag(argv, "--oneline")
        || flag_value(argv, "--pretty")
            .is_some_and(|v| v == "oneline" || v.starts_with("format:") || v.starts_with("tformat:"))
        || has_flag_prefix(argv, "--format=")
    {
        return oneline(output);
    }
    default(output)
}

/// `git log --oneline` / `--pretty=format:` / `--pretty=oneline`.
/// Output is already cheap (one short line per commit); just cap
/// the length and prepend a count if there are many.
pub fn oneline(output: &str) -> String {
    let s = strip_ansi(output);
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        // L79 — preserve the trimmed raw output instead of returning
        // empty so the gain ledger reports a clean 0-byte savings
        // record rather than an apparent token-loss anomaly.
        return s.trim().to_string();
    }
    // A one-line log is a list where every line is an answer:
    // `git log --oneline --since=2.days` — 163 commits, 16 KB, measured
    // 2026-10-03 — lost its middle 93 at a size that fits.
    crate::compress::whole_or_truncate_middle(&lines.join("\n"), 60, 10)
}

/// `git log --graph` — ASCII tree characters (`* | \ /`) carry the
/// branch topology. Generic dedup would mangle them. Strategy:
/// strip ANSI only, cap length, never collapse.
pub fn graph(output: &str) -> String {
    let s = strip_ansi(output);
    truncate_middle(&s, 80, 20)
}

/// `git log --stat` / `--shortstat` / `--name-only` / `--name-status`
/// — interleaves commit headers with per-file change info. We keep
/// both: parse commit headers as before, but PRESERVE the lines
/// between commits (which carry the file deltas).
pub fn with_files(output: &str) -> String {
    // Same rule as `default`: while it fits, date, author and body stay.
    if tk_base::delivery::fits(output) {
        if let Some(out) = detailed(output) {
            return out;
        }
    }
    let s = strip_ansi(output);
    let mut out = String::new();
    let mut current_hash: Option<String> = None;
    let mut current_subject: Option<String> = None;
    let mut file_lines: Vec<String> = Vec::new();
    let flush = |out: &mut String,
                 hash: &mut Option<String>,
                 subj: &mut Option<String>,
                 files: &mut Vec<String>| {
        if let (Some(h), Some(s)) = (hash.take(), subj.take()) {
            let _ = writeln!(out, "{h} {s}");
            for f in files.iter() {
                out.push_str(f);
                out.push('\n');
            }
            out.push('\n');
        }
        files.clear();
    };
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("commit ") {
            flush(&mut out, &mut current_hash, &mut current_subject, &mut file_lines);
            current_hash = Some(rest.chars().take(8).collect());
        } else if line.starts_with("Author") || line.starts_with("Date") {
            // skip — keep output tight
        } else if current_subject.is_none() && current_hash.is_some() && !line.trim().is_empty() {
            // First non-blank line after Date is the commit subject.
            // Git indents the subject body with 4 spaces; file-stat
            // rows are indented with 1 space. We distinguish to avoid
            // eating a `--stat` row as a subject.
            let leading_spaces = line.chars().take_while(|c| *c == ' ').count();
            if leading_spaces >= 4 || leading_spaces == 0 {
                current_subject = Some(line.trim().to_string());
            } else {
                // Indented with 1-3 spaces → this is a file-stat row,
                // not a subject. Treat as a file line.
                file_lines.push(line.to_string());
            }
        } else if !line.trim().is_empty() && current_hash.is_some() {
            let leading = line.chars().take_while(|c| *c == ' ').count();
            if leading >= 4 {
                // Wrapped commit message body — skip for tightness.
            } else {
                file_lines.push(line.to_string());
            }
        }
    }
    flush(&mut out, &mut current_hash, &mut current_subject, &mut file_lines);
    if out.trim().is_empty() {
        return generic(output);
    }
    // Same posture as `with_patch`: the parser already dropped Author/Date
    // and wrapped bodies. Recutting 80+15 dropped middle `--stat` commits.
    out.trim_end().to_string()
}

/// `git log -p` / `--patch` — each commit followed by a full diff.
/// We keep the commit headers compact (like default) and pass the
/// diff sections through [`super::diff::dispatch`] (the round-13
/// `compact_if_unified` door). That fold already keeps every change;
/// recutting with `truncate_middle(80, 20)` dropped the middle of a
/// multi-hunk `git show` / `git log -p`.
pub fn with_patch(output: &str) -> String {
    let s = strip_ansi(output);
    let mut out = String::new();
    let mut buffer = String::new();
    let mut commit_header: Option<String> = None;
    let flush = |out: &mut String, header: &mut Option<String>, body: &mut String| {
        if let Some(h) = header.take() {
            out.push_str(&h);
            out.push('\n');
            let diff_part = crate::cmd::linux::git::diff::dispatch(&[], body);
            out.push_str(diff_part.trim());
            out.push_str("\n\n");
        }
        body.clear();
    };
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("commit ") {
            flush(&mut out, &mut commit_header, &mut buffer);
            let short = rest.chars().take(8).collect::<String>();
            commit_header = Some(short);
        } else if line.starts_with("Author") || line.starts_with("Date") {
            // Skip — tighter shape.
        } else if commit_header.as_ref().is_some_and(|h| h.len() == 8)
            && !line.trim().starts_with(|c: char| c.is_alphanumeric())
        {
            // Probably the commit subject (indented in default format).
            if buffer.is_empty() && !line.trim().is_empty() {
                if let Some(h) = commit_header.as_mut() {
                    h.push(' ');
                    h.push_str(line.trim());
                }
            } else {
                buffer.push_str(line);
                buffer.push('\n');
            }
        } else {
            buffer.push_str(line);
            buffer.push('\n');
        }
    }
    flush(&mut out, &mut commit_header, &mut buffer);
    if out.trim().is_empty() {
        return generic(output);
    }
    out.trim_end().to_string()
}

/// Default `git log` — verbose format.
///
/// **While it fits, each commit keeps its date, its author and its BODY.**
/// The plain `git log` (not `--oneline`) is asked for exactly what
/// `--oneline` leaves out: measured 2026-10-03 on this repository, `git log
/// -15` — 5 KB — came back as fifteen `hash subject` lines, every date, every
/// author and every body paragraph gone (the body is where a commit says
/// WHY). The author is said once when there is only one; trailers
/// (`Co-Authored-By:`, `Signed-off-by:`) and blank lines go. Past the
/// threshold, the `hash subject` list below.
pub fn default(output: &str) -> String {
    if tk_base::delivery::fits(output) {
        if let Some(out) = detailed(output) {
            return out;
        }
    }
    hash_and_subject(output)
}

/// One commit as parsed from the verbose format.
struct Commit {
    hash: String,
    author: String,
    date: String,
    lines: Vec<String>,
    files: Vec<String>,
}

fn detailed(output: &str) -> Option<String> {
    let s = strip_ansi(output);
    let mut commits: Vec<Commit> = Vec::new();
    for line in s.lines() {
        if let Some(h) = line.strip_prefix("commit ") {
            commits.push(Commit { hash: h.chars().take(8).collect(), author: String::new(), date: String::new(), lines: Vec::new(), files: Vec::new() });
            continue;
        }
        let c = commits.last_mut()?;
        if let Some(a) = line.strip_prefix("Author:") {
            c.author = a.split('<').next().unwrap_or(a).trim().to_string();
        } else if let Some(d) = line.strip_prefix("Date:") {
            c.date = crate::gitlog::short_date(d);
        } else if line.starts_with("Merge:") || line.trim().is_empty() {
        } else if !line.starts_with("    ") {
            // `--stat` (` path | 3 +-`) and `--name-status` (`M\tpath`)
            // rows: the message is the 4-space-indented part only.
            c.files.push(line.trim_end().to_string());
        } else {
            let t = line.trim();
            let trailer = ["Co-Authored-By:", "Co-authored-by:", "Signed-off-by:"].iter().any(|p| t.starts_with(p));
            if !trailer {
                c.lines.push(t.to_string());
            }
        }
    }
    if commits.is_empty() {
        return None;
    }
    let one_author = commits.iter().all(|c| c.author == commits[0].author);
    let mut out = String::new();
    if one_author && commits.len() > 1 {
        let _ = writeln!(out, "[author of all {}: {}]", commits.len(), commits[0].author);
    }
    for c in &commits {
        let subject = c.lines.first().map(String::as_str).unwrap_or("");
        if one_author && commits.len() > 1 {
            let _ = writeln!(out, "{} {} {subject}", c.hash, c.date);
        } else {
            let _ = writeln!(out, "{} {} {} {subject}", c.hash, c.date, c.author);
        }
        for l in c.lines.iter().skip(1) {
            let _ = writeln!(out, "    {l}");
        }
        for f in &c.files {
            let _ = writeln!(out, "{f}");
        }
    }
    Some(out.trim_end().to_string())
}

fn hash_and_subject(output: &str) -> String {
    let s = strip_ansi(output);
    let mut commits = Vec::new();
    let mut hash = String::new();
    for line in s.lines() {
        if let Some(h) = line.strip_prefix("commit ") {
            hash = h.chars().take(8).collect();
        } else if !hash.is_empty()
            && !line.trim().is_empty()
            && !line.starts_with("Author")
            && !line.starts_with("Date")
        {
            commits.push(format!("{} {}", hash, line.trim()));
            hash.clear();
        }
    }
    // Fallback: --oneline / --pretty=format: / --pretty=oneline emit
    // `<hash> <subject>` per line with NO `commit <hash>` headers, so
    // the verbose parser above finds nothing. Returning an empty
    // string here drops the user's output entirely — observed bug
    // 2026-05-29. Pass through with truncate so very long logs stay
    // bounded but short ones survive verbatim.
    if commits.is_empty() {
        let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
        if lines.is_empty() {
            return String::new();
        }
        return truncate_middle(&lines.join("\n"), 40, 10);
    }
    truncate_middle(&commits.join("\n"), 40, 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(slice: &[&str]) -> Vec<String> {
        slice.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn oneline_does_not_swallow_output() {
        let s = "842daee feat: add setup/\n\
                 7de55e4 feat: add forked MCP servers\n\
                 65a0cba refactor: cleanup\n";
        let out = dispatch(&argv(&["git", "log", "--oneline", "-5"]), s);
        assert!(out.contains("842daee"));
        assert!(out.contains("65a0cba"));
    }

    #[test]
    fn oneline_empty_stays_empty_string() {
        // Empty input → empty output (no banner, no panic).
        let out = dispatch(&argv(&["git", "log", "--oneline"]), "");
        assert_eq!(out, "");
    }

    #[test]
    fn pretty_format_routes_to_oneline() {
        let s = "103a1010 2026-05-29 fix\n3493d8f7 2026-05-28 fix\n";
        let out = dispatch(
            &argv(&["git", "log", "--pretty=format:%h %ad %s", "--date=short"]),
            s,
        );
        assert!(out.contains("103a1010"));
    }

    /// A plain `git log` that fits keeps date, author (once) and body.
    #[test]
    fn a_log_that_fits_keeps_date_author_and_body() {
        let _g = tk_base::delivery::threshold_on_this_thread(5_000);
        let s = "commit ec8704af645f1111111111111111111111111111\n\
                 Author: pi-infected <p@x>\n\
                 Date:   Sat Oct 3 04:30:58 2026 +0200\n\
                 \n    nslookup n'invente plus d'en-tête\n\
                 \n    Richesse re-bénie : host_apple 1,00 → 0,86.\n\
                 \n    Co-Authored-By: Claude <noreply@anthropic.com>\n\
                 \n\
                 commit c1b0e5c92222222222222222222222222222222\n\
                 Author: pi-infected <p@x>\n\
                 Date:   Fri Oct 2 23:10:00 2026 +0200\n\
                 \n    top -l1 atteint son compacteur\n";
        assert_eq!(
            default(s),
            "[author of all 2: pi-infected]\n\
             ec8704af 2026-10-03 nslookup n'invente plus d'en-tête\n    \
             Richesse re-bénie : host_apple 1,00 → 0,86.\n\
             c1b0e5c9 2026-10-02 top -l1 atteint son compacteur"
        );
    }

    /// `git log --name-status` that fits: the header of `default`, then the
    /// file rows as git printed them.
    #[test]
    fn a_file_log_that_fits_keeps_date_body_and_files() {
        let _g = tk_base::delivery::threshold_on_this_thread(5_000);
        let s = "commit b217fed9aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n\
                 Author: pi-infected <p@x>\n\
                 Date:   Sat Oct 3 05:10:00 2026 +0200\n\
                 \n    git log garde la date\n\
                 \n    Le corps dit pourquoi.\n\
                 \nM\tPATCHNOTES.md\nM\tcrates/tokenade-core/src/gitlog.rs\n";
        let out = dispatch(&argv(&["git", "log", "-1", "--name-status"]), s);
        assert_eq!(
            out,
            "b217fed9 2026-10-03 pi-infected git log garde la date\n    \
             Le corps dit pourquoi.\nM\tPATCHNOTES.md\nM\tcrates/tokenade-core/src/gitlog.rs"
        );
    }

    /// A one-line log that fits keeps every commit.
    #[test]
    fn a_oneline_log_that_fits_keeps_every_commit() {
        let _g = tk_base::delivery::threshold_on_this_thread(5_000);
        let s: String = (0..163)
            .map(|i| format!("{:08x} commit subject number {i} with words\n", i * 7919 + 13))
            .collect();
        let out = dispatch(&argv(&["git", "log", "--oneline"]), &s);
        assert_eq!(out, s.trim_end());
    }

    #[test]
    fn default_format_extracts_hash_subject_pairs() {
        let s = "commit 7de55e4abcdef0123456789\n\
                 Author: Foo <foo@bar>\n\
                 Date:   Mon Jan 1 00:00:00 2024\n\
                 \n\
                 feat: add forked MCP servers\n\
                 \n\
                 commit 65a0cbaabcdef0123456789\n\
                 Author: Foo <foo@bar>\n\
                 Date:   Mon Jan 1 00:00:00 2024\n\
                 \n\
                 refactor: cleanup\n";
        let out = dispatch(&argv(&["git", "log"]), s);
        assert!(out.contains("7de55e4a"));
        assert!(out.contains("refactor: cleanup"));
    }

    #[test]
    fn graph_preserves_topology_chars() {
        let s = "* commit 1\n|\n* commit 2\n|\\\n| * commit 3\n";
        let out = dispatch(&argv(&["git", "log", "--graph"]), s);
        assert!(out.contains("|\\"), "topology chars must survive: {out}");
    }

    #[test]
    fn with_patch_keeps_every_change_past_the_old_truncate() {
        let mut s = String::from(
            "commit 7de55e4abcdef0123456789\n\
             Author: Foo <foo@bar>\n\
             Date:   Mon Jan 1 00:00:00 2024\n\
             \n\
                 feat: big\n\
             \n\
             diff --git a/foo b/foo\n--- a/foo\n+++ b/foo\n",
        );
        for i in 0..250 {
            s.push_str(&format!("@@ -{i} +{i} @@\n-old{i}\n+new{i}\n"));
        }
        let out = dispatch(&argv(&["git", "log", "-p"]), &s);
        assert!(out.contains("+new0"), "{out}");
        assert!(out.contains("+new249"), "middle/tail hunk dropped:\n{out}");
        assert!(out.contains("-old124"), "middle hunk dropped:\n{out}");
        assert!(out.contains("7de55e4a"));
    }

    #[test]
    fn with_files_keeps_middle_commits_past_the_old_truncate() {
        let mut s = String::new();
        for i in 0..120 {
            s.push_str(&format!(
                "commit {i:040x}\n\
                 Author: Foo <foo@bar>\n\
                 Date:   Mon Jan 1 00:00:00 2024\n\
                 \n\
                     feat {i}\n\
                 \n\
                  src/f{i}.rs | 1 +\n\
                  1 file changed, 1 insertion(+)\n\n"
            ));
        }
        let out = dispatch(&argv(&["git", "log", "--stat"]), &s);
        assert!(out.contains("feat 0"), "{out}");
        assert!(out.contains("feat 119"), "tail commit dropped:\n{out}");
        assert!(out.contains("feat 60"), "middle commit dropped:\n{out}");
        assert!(out.contains("src/f60.rs"), "middle --stat row dropped:\n{out}");
    }
}
