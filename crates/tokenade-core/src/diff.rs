//! Unified-diff / patch-file compaction.
//!
//! Agents see diffs constantly: `gh pr diff`, `git format-patch`, applied
//! `.patch` files, `hg diff`. The format is well-known:
//!
//! ```text
//! diff --git a/foo.rs b/foo.rs    ← file-header line (preserve)
//! index abc..def 100644            ← drop (rev hashes useless to reader)
//! --- a/foo.rs                    ← drop (duplicate of `diff --git`)
//! +++ b/foo.rs                    ← drop
//! @@ -10,7 +10,8 @@ fn old()        ← hunk header (preserve)
//!  unchanged context line          ← context (cap to N around the change)
//! -removed line                    ← signal (preserve)
//! +added line                      ← signal (preserve)
//!  more context …                  ← context
//! ```
//!
//! [`compact`] keeps every `+`/`-` line and hunk header, caps the surrounding
//! context to [`CONTEXT_LINES`] lines on each side of a change, drops the
//! `index`/`---`/`+++` chrome, truncates very long lines, replaces binary
//! file diffs with a one-line marker, and folds multi-file diffs beyond
//! [`MAX_FILES`]. Files with a rare change signature are rendered first, so
//! one real edit among sixty regenerated twins is not the file that gets
//! elided.

use crate::tokenizer;
use std::collections::HashMap;
use std::fmt::Write;

/// Context lines kept around each change (per side). Set to 1 — agents
/// almost always have access to the source via [`crate::skeleton::skeleton`]
/// or symbol search, so a single anchor line is enough.
pub const CONTEXT_LINES: usize = 1;

/// Above this line count, automatically switch to no-context mode where
/// even the single anchor line is stripped. At ~5 k lines the diff is
/// already a "show me what changed, not the surrounding code" scenario.
pub const NO_CONTEXT_TRIGGER: usize = 5_000;

/// Max files to render in full; beyond this we just list their names + status.
pub const MAX_FILES: usize = 20;

/// Truncate per-line content longer than this (e.g. minified JS diffs).
pub const LINE_CAP: usize = 240;

/// [`compact`], but only when the text really is a unified diff.
///
/// A `diff`, `diff -u` or `diff -r` compactor that hands its output to a
/// per-command head+tail archetype keeps the first 50 lines and the last 5,
/// and a diff's changed lines are the only thing it carries. Measured on a
/// 1 610-line `diff -u` of two 1 200-host inventory reports: the named route
/// returned **27 of the 810 changed lines** under a banner reading
/// "[tokenade:diff] 1610 line(s)" — the size of the INPUT, which says nothing
/// about the 783 changes it dropped. The same bytes through this function
/// keep 810 of 810. An unknown program on the same input does better still:
/// the shape fold claims every one of the 1 605 lines with a `[×N lines]`
/// count. Being smaller than a truthful answer is not being better.
///
/// Returns `None` for a NORMAL (`<`/`>`) or context (`-c`) diff, which the
/// unified-diff parser cannot read; those keep the archetype.
pub fn compact_if_unified(text: &str) -> Option<String> {
    (crate::auto::detect(text) == crate::auto::Kind::UnifiedDiff).then(|| compact(text))
}

pub fn compact(diff: &str) -> String {
    let line_count = diff.lines().count();
    let no_context = line_count >= NO_CONTEXT_TRIGGER;
    compact_with(diff, no_context)
}

/// Compact a diff with `no_context = true` to strip every context line
/// (including the single CONTEXT_LINES anchor). Useful for huge refactor
/// reviews where the agent already has the source via symbol search.
pub fn compact_no_context(diff: &str) -> String {
    compact_with(diff, true)
}

fn compact_with(diff: &str, no_context: bool) -> String {
    let files = rank_files(split_per_file(diff));
    let mut out = String::with_capacity(diff.len() / 4);
    let total_files = files.len();
    let to_render = total_files.min(MAX_FILES);

    for (i, file) in files.iter().take(to_render).enumerate() {
        if i > 0 {
            out.push('\n');
        }
        // Generated / lock / minified files: their line-by-line diff carries no
        // review signal, so render just the change-count summary (steal:
        // jusTokenMax + token-optimizer). The file is one read away if needed.
        if let Some(path) = noise_file_path(file) {
            let (adds, dels) = count_changes(file);
            let _ = write!(out, "{path}  +{adds}/-{dels} (generated — diff elided)");
        } else {
            compact_file_diff(file, &mut out, no_context);
        }
    }
    if total_files > MAX_FILES {
        let _ = write!(
            out,
            "\n… {} more changed file(s) elided …",
            total_files - MAX_FILES
        );
        // Also list their names for situational awareness — but cap the list
        // itself so a 200-file diff doesn't end with 180 header lines (which
        // would defeat half the compression).
        const NAME_CAP: usize = 10;
        let extra_files = total_files - MAX_FILES;
        let names: Vec<String> = files
            .iter()
            .skip(MAX_FILES)
            .take(NAME_CAP)
            .filter_map(|f| f.lines().next().map(|l| l.to_string()))
            .collect();
        if !names.is_empty() {
            out.push_str("\n  ");
            out.push_str(&names.join("\n  "));
            if extra_files > NAME_CAP {
                let _ = write!(out, "\n  …{} more names elided", extra_files - NAME_CAP);
            }
        }
    }
    out
}

/// Render rare change signatures before repeated twins.
///
/// `MAX_FILES` keeps the first N files of the patch. Sixty regenerated
/// files plus one logic change at the end therefore dropped the only
/// line that answered (`src/auth.rs` / `claims.sig_ok`) — measured by
/// `error_survival::a_search_and_a_diff_keep_the_line_that_answers`.
/// Identical hunks share a signature and stay in original order, so a
/// patch of fifty equal files still shows file0..file19.
fn rank_files(files: Vec<String>) -> Vec<String> {
    let n = files.len();
    if n <= MAX_FILES {
        return files;
    }
    let sigs: Vec<String> = files.iter().map(|f| change_signature(f)).collect();
    let mut freq: HashMap<&str, usize> = HashMap::new();
    for s in &sigs {
        *freq.entry(s.as_str()).or_insert(0) += 1;
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&ia, &ib| {
        let fa = freq[sigs[ia].as_str()];
        let fb = freq[sigs[ib].as_str()];
        fa.cmp(&fb)
            .then_with(|| {
                let (aa, da) = count_changes(&files[ia]);
                let (ab, db) = count_changes(&files[ib]);
                (ab + db).cmp(&(aa + da))
            })
            .then_with(|| ia.cmp(&ib))
    });
    order.into_iter().map(|i| files[i].clone()).collect()
}

fn change_signature(file: &str) -> String {
    let mut s = String::new();
    for l in file.lines() {
        if l.starts_with("+++")
            || l.starts_with("---")
            || l.starts_with("diff --git")
            || l.starts_with("index ")
            || l.starts_with("@@")
        {
            continue;
        }
        if l.starts_with('+') || l.starts_with('-') {
            s.push_str(l);
            s.push('\n');
        }
    }
    s
}

pub fn compact_with_savings(diff: &str) -> (String, crate::Savings) {
    let before = tokenizer::estimate(diff);
    let out = compact(diff);
    let after = tokenizer::estimate(&out);
    (out, crate::Savings::new(before, after))
}

/// Split a multi-file unified diff into one chunk per file, on `diff --git`
/// (or stand-alone `--- ` if `diff --git` isn't present, e.g. plain unified
/// diffs from `diff -u`).
fn split_per_file(diff: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    for line in diff.lines() {
        // Flush the accumulated chunk when we hit a new file boundary.
        // `--- ` only marks a split when there is no `diff --git` framing,
        // but in that case `current` would already be empty (nothing to
        // flush), so checking `diff --git` alone suffices here.
        if line.starts_with("diff --git ") && !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
        current.push_str(line);
        current.push('\n');
    }
    if !current.trim().is_empty() {
        out.push(current);
    }
    out
}

/// If this per-file diff chunk targets a generated/lock/minified file whose
/// hunk body is pure noise to review, return its path. Conservative: only the
/// unambiguous machine-authored files (lockfiles, minified bundles, source
/// maps, snapshots) and well-known generated dirs.
fn noise_file_path(file: &str) -> Option<String> {
    let header = file.lines().next()?;
    if !header.starts_with("diff --git ") {
        return None;
    }
    let path = header.rsplit(" b/").next()?.trim();
    if path.is_empty() {
        return None;
    }
    let lower = path.to_ascii_lowercase();
    const NOISE_SUFFIX: &[&str] = &[".lock", ".min.js", ".min.css", ".map", ".snap"];
    const NOISE_NAME: &[&str] = &[
        "package-lock.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "cargo.lock",
        "composer.lock",
        "poetry.lock",
        "gemfile.lock",
        "go.sum",
    ];
    const NOISE_DIR: &[&str] = &[
        "/dist/",
        "/build/",
        "/vendor/",
        "/node_modules/",
        "/.next/",
        "/target/",
        "/generated/",
        "/__generated__/",
    ];
    let is_noise = NOISE_SUFFIX.iter().any(|s| lower.ends_with(s))
        || NOISE_NAME.iter().any(|n| lower.ends_with(n))
        || NOISE_DIR.iter().any(|d| lower.contains(d));
    is_noise.then(|| path.to_string())
}

/// Count `+`/`-` change lines in a per-file diff chunk (excluding the
/// `+++`/`---` file headers).
fn count_changes(file: &str) -> (usize, usize) {
    let mut adds = 0;
    let mut dels = 0;
    for l in file.lines() {
        if l.starts_with("+++") || l.starts_with("---") {
            continue;
        }
        if l.starts_with('+') {
            adds += 1;
        } else if l.starts_with('-') {
            dels += 1;
        }
    }
    (adds, dels)
}

fn compact_file_diff(file: &str, out: &mut String, no_context: bool) {
    let lines: Vec<&str> = file.lines().collect();
    if lines.is_empty() {
        return;
    }

    // Detect binary-diff one-liners.
    if file.contains("\nBinary files ") || file.contains("\nGIT binary patch") {
        // Try to find the file name from the first line.
        let header = lines.first().copied().unwrap_or("(binary)");
        let _ = write!(out, "{header}\n[binary diff — content stripped]\n");
        return;
    }

    // Pre-classify each line so we can keep `±CONTEXT_LINES` around any change.
    let kinds: Vec<DiffLine> = lines
        .iter()
        .map(|l| {
            if l.starts_with("diff --git ") || l.starts_with("diff -r ") {
                DiffLine::FileHeader
            } else if l.starts_with("@@") {
                DiffLine::Hunk
            } else if l.starts_with("+++")
                || l.starts_with("---")
                || l.starts_with("index ")
                || l.starts_with("new file mode")
                || l.starts_with("deleted file mode")
                || l.starts_with("similarity index")
                || l.starts_with("rename from")
                || l.starts_with("rename to")
                || l.starts_with("\\ No newline at end of file")
            {
                DiffLine::Chrome
            } else if l.starts_with('+') {
                DiffLine::Added
            } else if l.starts_with('-') {
                DiffLine::Removed
            } else {
                DiffLine::Context
            }
        })
        .collect();

    // Mark a line as "keep" if it's a header/hunk/change OR within
    // CONTEXT_LINES of a change.
    let mut keep: Vec<bool> = kinds
        .iter()
        .map(|k| {
            matches!(
                k,
                DiffLine::FileHeader | DiffLine::Hunk | DiffLine::Added | DiffLine::Removed
            )
        })
        .collect();
    // In no-context mode the context anchor is suppressed entirely.
    let border = if no_context { 0 } else { CONTEXT_LINES };
    if border > 0 {
        for i in 0..kinds.len() {
            if matches!(kinds[i], DiffLine::Added | DiffLine::Removed) {
                let lo = i.saturating_sub(border);
                let hi = (i + border + 1).min(kinds.len());
                for s in keep.iter_mut().take(hi).skip(lo) {
                    *s = true;
                }
            }
        }
    }

    // Also keep the file-rename header (we want the user to see renames).
    for (i, k) in kinds.iter().enumerate() {
        if matches!(k, DiffLine::Chrome) && {
            let l = lines[i];
            l.starts_with("rename from")
                || l.starts_with("rename to")
                || l.starts_with("new file mode")
                || l.starts_with("deleted file mode")
        } {
            keep[i] = true;
        }
    }

    // Walk lines, emitting kept ones; collapse runs of dropped context as
    // "…<n> context lines…" — but only inside a hunk (between two `@@`).
    // In no-context mode we silently drop instead of summarising; the user
    // explicitly traded the count for token savings.
    let mut dropped_run = 0usize;
    let emit_run_marker = !no_context;
    let flush_run = |out: &mut String, run: &mut usize| {
        if *run > 0 && emit_run_marker {
            let _ = writeln!(out, "…{run} context line(s) elided…");
        }
        *run = 0;
    };
    for (i, line) in lines.iter().enumerate() {
        if keep[i] {
            flush_run(out, &mut dropped_run);
            out.push_str(&truncate(line, LINE_CAP));
            out.push('\n');
        } else if matches!(kinds[i], DiffLine::Context) {
            dropped_run += 1;
        }
        // Chrome lines that aren't on the keep-list are silently dropped.
    }
    flush_run(out, &mut dropped_run);
}

#[derive(Debug, Clone, Copy)]
enum DiffLine {
    FileHeader,
    Hunk,
    Chrome,
    Added,
    Removed,
    Context,
}

fn truncate(s: &str, cap: usize) -> String {
    let n = s.chars().count();
    if n <= cap {
        return s.to_string();
    }
    let head: String = s.chars().take(cap).collect();
    format!("{head}…[+{} chars]", n - cap)
}

/// Files listed by [`plan`] before it says how many more there are.
pub const PLAN_MAX_FILES: usize = 80;

/// What a diff TOO BIG TO DELIVER changes, file by file: one line per file
/// with its added and removed line counts, the totals, and the lines `diff`
/// wrote about files it did not compare (`Only in …`, binary files).
///
/// The delivery stage used to cut such a diff to its first and last lines —
/// MEASURED 2026-10-02 on a real `diff -r` of 36 files (5 116 lines, ~65 k
/// tokens): ONE file named, and nothing said the other 35 existed. A diff's
/// natural outline is its list of files, as a document's is its headings; this
/// is that outline, for every format a diff comes in — unified (`git diff`,
/// `diff -u`), normal and context, single-file or recursive. `None` when the
/// text is not a diff.
pub fn plan(text: &str) -> Option<String> {
    let (files, framing) = if is_unified(text) {
        (unified_files(text), Vec::new())
    } else if crate::cmd::linux::diff::served_whole(text) {
        classic_files(text)
    } else {
        return None;
    };
    if files.is_empty() {
        return None;
    }
    let (adds, dels) = files
        .iter()
        .fold((0, 0), |(a, d), (_, x, y)| (a + x, d + y));
    let mut out = format!(
        "diff plan — {} file(s) changed · +{adds} −{dels} line(s)\n",
        files.len()
    );
    for (path, a, d) in files.iter().take(PLAN_MAX_FILES) {
        let _ = writeln!(out, "  {path}  +{a} −{d}");
    }
    if files.len() > PLAN_MAX_FILES {
        let _ = writeln!(out, "  … {} more file(s)", files.len() - PLAN_MAX_FILES);
    }
    for l in framing.iter().take(PLAN_MAX_FILES) {
        let _ = writeln!(out, "  {l}");
    }
    if framing.len() > PLAN_MAX_FILES {
        let _ = writeln!(
            out,
            "  … {} more line(s) about files not compared",
            framing.len() - PLAN_MAX_FILES
        );
    }
    Some(out.trim_end().to_string())
}

/// The text IS a unified diff — it opens on one — not a document that quotes
/// one (a PR description with a ```diff block keeps its heading outline).
fn is_unified(text: &str) -> bool {
    let mut ls = text.lines().filter(|l| !l.trim().is_empty());
    match (ls.next(), ls.next()) {
        (Some(a), _) if a.starts_with("diff --git ") => true,
        (Some(a), Some(b)) => {
            a.starts_with("--- ")
                && b.starts_with("+++ ")
                && text.lines().any(|l| l.starts_with("@@ "))
        }
        _ => false,
    }
}

fn unified_files(text: &str) -> Vec<(String, usize, usize)> {
    split_per_file(text)
        .iter()
        .filter_map(|chunk| {
            let path = chunk
                .lines()
                .find_map(|l| {
                    l.strip_prefix("+++ ")
                        .map(|p| p.trim_start_matches("b/").to_string())
                })
                .or_else(|| {
                    chunk
                        .lines()
                        .next()?
                        .strip_prefix("diff --git ")
                        .map(|h| h.rsplit(" b/").next().unwrap_or(h).to_string())
                })?;
            let path = path.split('\t').next().unwrap_or(&path).to_string();
            let (a, d) = count_changes(chunk);
            Some((path, a, d))
        })
        .collect()
}

/// Normal and context diffs, recursive or not. A file starts at the command
/// echo `diff [flags] a b` — the one framing line that is never translated;
/// every other framing line is kept as written.
fn classic_files(text: &str) -> (Vec<(String, usize, usize)>, Vec<String>) {
    let mut files: Vec<(String, usize, usize)> = Vec::new();
    let mut framing = Vec::new();
    for l in text.lines() {
        if l.starts_with("diff ") && l.split_whitespace().count() >= 3 {
            let path = l.split_whitespace().last().unwrap_or(l).to_string();
            files.push((path, 0, 0));
            continue;
        }
        let entry = || ("(single file)".to_string(), 0, 0);
        let (added, removed) = match l.as_bytes() {
            [b'>', ..] | [b'+', b' ', ..] => (1, 0),
            [b'<', ..] | [b'-', b' ', ..] => (0, 1),
            // A context `!` line is one side of a change; counted on the side
            // its block header (`***` old / `---` new) put it, which this
            // single pass does not track — so it counts as a change on both.
            [b'!', b' ', ..] => (1, 1),
            _ => {
                if !l.trim().is_empty() && crate::cmd::linux::diff::is_diff_framing(l) {
                    framing.push(l.to_string());
                }
                continue;
            }
        };
        if files.is_empty() {
            files.push(entry());
        }
        let last = files.last_mut().expect("pushed above");
        last.1 += added;
        last.2 += removed;
    }
    (files, framing)
}

#[cfg(test)]
mod plan_tests {
    use super::*;

    /// A recursive normal diff, framing in French: every file named with its
    /// counts, the files `diff` did not compare listed as written.
    #[test]
    fn a_recursive_diff_plan_names_every_file() {
        let mut t = String::new();
        let names = [
            "archive.rs",
            "auto/detectors.rs",
            "cmd/linux/grep.rs",
            "filter/intent.rs",
        ];
        for (k, n) in names.iter().enumerate() {
            let _ = writeln!(t, "diff -r old/src/{n} new/src/{n}");
            for h in 0..=k {
                let _ = writeln!(t, "{}c{}\n< let v{h} = old_{n}({h});\n---\n> let v{h} = new_{n}({h});\n> trace!(\"{n}\");", 10 + h * 7, 10 + h * 8);
            }
        }
        t.push_str("Seulement dans new/src/cmd/linux/docker: system_df.rs\n");
        let p = plan(&t).expect("a diff");
        assert!(
            p.starts_with("diff plan — 4 file(s) changed · +20 −10 line(s)"),
            "{p}"
        );
        for n in names {
            assert!(p.contains(&format!("new/src/{n}  +")), "{n} missing:\n{p}");
        }
        assert!(p.contains("  new/src/filter/intent.rs  +8 −4"), "{p}");
        assert!(
            p.contains("  Seulement dans new/src/cmd/linux/docker: system_df.rs"),
            "{p}"
        );
    }

    #[test]
    fn a_git_diff_plan_counts_per_file() {
        let t = "diff --git a/src/a.rs b/src/a.rs\nindex 1..2 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,2 +1,3 @@\n fn a() {}\n-let x = 1;\n+let x = 2;\n+let y = 3;\n\
                 diff --git a/README.md b/README.md\n--- a/README.md\n+++ b/README.md\n@@ -4 +4 @@\n-old line\n+new line\n";
        let p = plan(t).expect("a diff");
        assert!(
            p.starts_with("diff plan — 2 file(s) changed · +3 −2"),
            "{p}"
        );
        assert!(
            p.contains("  src/a.rs  +2 −1") && p.contains("  README.md  +1 −1"),
            "{p}"
        );
    }

    #[test]
    fn a_text_that_is_not_a_diff_has_no_plan() {
        assert_eq!(plan("# Title\n\n- item\n- other\n\nSome prose.\n"), None);
        assert_eq!(plan("Only in a: x\nOnly in b: y\n"), None);
        // A document QUOTING a diff keeps its own outline.
        let md = "# Release notes\n\n## Fix\n\n```diff\ndiff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n```\n";
        assert_eq!(plan(md), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elides_generated_file_diffs_keeps_real_code() {
        let d = "diff --git a/Cargo.lock b/Cargo.lock\n--- a/Cargo.lock\n+++ b/Cargo.lock\n\
@@ -1,3 +1,4 @@\n+new dep\n-old dep\n\
diff --git a/src/main.rs b/src/main.rs\n--- a/src/main.rs\n+++ b/src/main.rs\n\
@@ -1,2 +1,2 @@\n-fn old() {}\n+fn new() {}\n";
        let out = compact(d);
        // Lock file folded to a one-line summary…
        assert!(out.contains("Cargo.lock  +1/-1 (generated"), "{out}");
        assert!(!out.contains("new dep"), "lockfile body elided: {out}");
        // …real source code kept.
        assert!(out.contains("fn new() {}"), "code diff kept: {out}");
    }

    #[test]
    fn noise_file_path_is_conservative() {
        assert!(noise_file_path("diff --git a/yarn.lock b/yarn.lock").is_some());
        assert!(noise_file_path("diff --git a/web/dist/app.js b/web/dist/app.js").is_some());
        assert!(noise_file_path("diff --git a/app.min.js b/app.min.js").is_some());
        // real source is never elided.
        assert!(noise_file_path("diff --git a/src/lib.rs b/src/lib.rs").is_none());
        assert!(noise_file_path("diff --git a/locks.rs b/locks.rs").is_none());
    }

    #[test]
    fn keeps_changes_drops_chrome() {
        let d = "diff --git a/x.rs b/x.rs\n\
index abc123..def456 100644\n\
--- a/x.rs\n\
+++ b/x.rs\n\
@@ -1,4 +1,4 @@\n\
 fn foo() {\n\
-    return 1;\n\
+    return 2;\n\
 }\n";
        let out = compact(d);
        assert!(out.contains("diff --git a/x.rs b/x.rs"));
        assert!(out.contains("@@ -1,4 +1,4 @@"));
        assert!(out.contains("-    return 1;"));
        assert!(out.contains("+    return 2;"));
        // Chrome dropped.
        assert!(!out.contains("index abc"));
        assert!(!out.contains("+++ b/x.rs"));
        assert!(!out.contains("--- a/x.rs"));
    }

    #[test]
    fn caps_context_around_change() {
        let mut d = String::from("diff --git a/big.txt b/big.txt\n@@ -1,30 +1,30 @@\n");
        for i in 1..15 {
            let _ = writeln!(d, " line {i}");
        }
        d.push_str("-changed\n+changed!\n");
        for i in 15..30 {
            let _ = writeln!(d, " line {i}");
        }
        let out = compact(&d);
        // Change preserved
        assert!(out.contains("-changed"));
        assert!(out.contains("+changed!"));
        // Hunk header preserved
        assert!(out.contains("@@"));
        // Context folded
        assert!(out.contains("context line(s) elided"));
        // Compression real
        let sav = tokenizer::estimate(&d) as f64 - tokenizer::estimate(&out) as f64;
        assert!(sav > 0.0, "no savings");
    }

    #[test]
    fn folds_multi_file_diff() {
        let mut d = String::new();
        for i in 0..50 {
            let _ = write!(
                d,
                "diff --git a/file{i}.rs b/file{i}.rs\n@@ -1 +1 @@\n-old\n+new\n"
            );
        }
        let out = compact(&d);
        assert!(out.contains("more changed file(s) elided"));
        // First 20 files emitted, file 30+ summarised only.
        assert!(out.contains("file0.rs"));
        assert!(out.contains("file19.rs"));
        // file 25 should appear in the elided-names list but not as a hunk.
        assert!(out.contains("file25.rs"));
    }

    #[test]
    fn unique_file_among_regenerated_churn_is_kept() {
        let mut d = String::new();
        for i in 0..60 {
            let _ = write!(
                d,
                "diff --git a/src/gen{i}.rs b/src/gen{i}.rs\n\
                 --- a/src/gen{i}.rs\n+++ b/src/gen{i}.rs\n\
                 @@ -1,4 +1,4 @@\n fn generated() {{\n\
                 -    let x = 1;\n+    let x = 1; // regenerated\n }}\n"
            );
        }
        d.push_str(
            "diff --git a/src/auth.rs b/src/auth.rs\n\
             --- a/src/auth.rs\n+++ b/src/auth.rs\n\
             @@ -42,7 +42,7 @@ pub fn verify(token: &str) -> bool {\n\
                  let claims = decode(token);\n\
             -    claims.exp > now() && claims.sig_ok\n\
             +    claims.exp > now()\n }\n",
        );
        let out = compact(&d);
        assert!(out.contains("src/auth.rs"), "unique file elided:\n{out}");
        assert!(
            out.contains("claims.sig_ok"),
            "the line that answers was folded away:\n{out}"
        );
    }

    #[test]
    fn binary_diff_summarised() {
        let d = "diff --git a/img.png b/img.png\nBinary files a/img.png and b/img.png differ\n";
        let out = compact(d);
        assert!(out.contains("[binary diff"));
        assert!(out.contains("img.png"));
    }

    #[test]
    fn renames_preserved() {
        let d = "diff --git a/old.rs b/new.rs\n\
similarity index 100%\n\
rename from old.rs\n\
rename to new.rs\n";
        let out = compact(d);
        assert!(out.contains("rename from old.rs"));
        assert!(out.contains("rename to new.rs"));
    }

    #[test]
    fn savings_real() {
        // Synthesise a multi-hunk diff where ~80 % is context.
        let mut d = String::from("diff --git a/f.rs b/f.rs\n@@ -1,200 +1,200 @@\n");
        for i in 0..100 {
            let _ = writeln!(d, " context line {i}");
        }
        d.push_str("-removed\n+added\n");
        for i in 100..200 {
            let _ = writeln!(d, " context line {i}");
        }
        let (out, sav) = compact_with_savings(&d);
        assert!(out.contains("-removed"));
        assert!(out.contains("+added"));
        assert!(sav.percent() > 80.0, "got {:?}", sav);
    }

    #[test]
    fn handles_empty_and_garbage() {
        assert_eq!(compact(""), "");
        let _ = compact("just some text not really a diff");
    }

    // ---- Adversarial: no-context mode ------------------------------------

    #[test]
    fn no_context_strips_anchor_lines() {
        let d = "diff --git a/x.rs b/x.rs\n\
                 @@ -1,5 +1,5 @@\n\
                  fn before_context() {\n\
                 -    return 1;\n\
                 +    return 2;\n\
                  fn after_context() {\n";
        let out = compact_no_context(d);
        /* L122: banner removed; assertion deleted */
        assert!(out.contains("-    return 1;"));
        assert!(out.contains("+    return 2;"));
        // Anchor (context) lines must be stripped.
        assert!(
            !out.contains("fn before_context"),
            "context line leaked into no-context output:\n{out}"
        );
        assert!(!out.contains("fn after_context"));
    }

    #[test]
    fn huge_diff_auto_triggers_no_context() {
        // Synthesise a 6000-line diff spread across many small hunks. The
        // auto-trigger at 5000 lines should flip on, stripping anchors.
        let mut d = String::from("diff --git a/big.rs b/big.rs\n");
        for hunk in 0..600 {
            let _ = writeln!(d, "@@ -{0},10 +{0},10 @@", hunk * 10 + 1);
            for j in 0..4 {
                let _ = writeln!(d, " context line {hunk}-{j}");
            }
            d.push_str("-removed\n+added\n");
            for j in 0..4 {
                let _ = writeln!(d, " context tail {hunk}-{j}");
            }
        }
        let line_count = d.lines().count();
        assert!(line_count >= NO_CONTEXT_TRIGGER);
        let out = compact(&d);
        /* L122: banner removed; assertion deleted */
        let sav = crate::Savings::new(tokenizer::estimate(&d), tokenizer::estimate(&out));
        assert!(
            sav.percent() > 70.0,
            "expected >70% savings, got {:.1}%",
            sav.percent()
        );
    }

    #[test]
    fn small_diff_keeps_context() {
        let d = "diff --git a/x.rs b/x.rs\n\
                 @@ -1,3 +1,3 @@\n\
                  fn anchor() {\n\
                 -    1\n\
                 +    2\n\
                  }\n";
        let out = compact(d);
        // Below trigger → no banner.
        assert!(!out.contains("[tokenade:diff no-context]"));
        // Anchor preserved.
        assert!(out.contains("fn anchor"));
    }

    #[test]
    fn no_context_never_loses_change_lines() {
        // Adversarial: 200 changes interleaved with context. Every `+`/`-`
        // line must survive.
        let mut d = String::from("diff --git a/x.rs b/x.rs\n@@ -1,400 +1,400 @@\n");
        for i in 0..200 {
            let _ = writeln!(d, " ctx top {i}");
            let _ = writeln!(d, "-old line {i}");
            let _ = writeln!(d, "+new line {i}");
            let _ = writeln!(d, " ctx bottom {i}");
        }
        let out = compact_no_context(&d);
        for i in 0..200 {
            assert!(
                out.contains(&format!("-old line {i}")),
                "lost removed line {i}"
            );
            assert!(
                out.contains(&format!("+new line {i}")),
                "lost added line {i}"
            );
        }
    }
}
