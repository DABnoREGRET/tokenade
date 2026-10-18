//! `ls -R` — recursive directory listing.
//!
//! Canonical shape:
//! ```text
//! .:
//! Cargo.toml  README.md  src  tests
//!
//! ./src:
//! lib.rs  main.rs  utils.rs
//!
//! ./tests:
//! integration_test.rs  unit_test.rs
//! ```
//!
//! Old compactor: `line_cap(200)` with a broken `detect` that matches *any*
//! non-empty text. A large project can have 1000+ directories with hundreds
//! of files each; cap(200) cuts after the first 200 listing lines.
//!
//! Quality contract:
//!   - Every directory path: always kept
//!   - Files in small directories (≤ INLINE_CAP): listed inline
//!   - Files in large directories: count shown with `[N files]`
//!   - Total directory count + total file count in banner
//!
//! Tested in #789.

use std::fmt::Write;

const INLINE_CAP: usize = 10;

/// One directory block parsed from ls -R output.
struct DirBlock<'a> {
    path: &'a str,
    files: Vec<&'a str>,
}

fn is_dir_header(line: &str) -> bool {
    // Directory header: not indented, ends with ':', non-empty before ':'
    if line.starts_with(' ') || line.starts_with('\t') {
        return false;
    }
    let t = line.trim_end();
    t.ends_with(':') && t.len() > 1
}

fn parse_blocks(text: &str) -> Vec<DirBlock<'_>> {
    let mut blocks: Vec<DirBlock<'_>> = Vec::new();
    let mut current_path: Option<&str> = None;
    let mut current_files: Vec<&str> = Vec::new();

    for line in text.lines() {
        let t = line.trim_end();
        if t.is_empty() {
            if let Some(p) = current_path.take() {
                blocks.push(DirBlock { path: p, files: std::mem::take(&mut current_files) });
            }
            continue;
        }
        if is_dir_header(t) {
            if let Some(p) = current_path.take() {
                blocks.push(DirBlock { path: p, files: std::mem::take(&mut current_files) });
            }
            current_path = Some(&t[..t.len() - 1]);
            continue;
        }
        if current_path.is_some() {
            for f in t.split_whitespace() {
                if !f.is_empty() {
                    current_files.push(f);
                }
            }
        }
    }
    if let Some(p) = current_path {
        blocks.push(DirBlock { path: p, files: current_files });
    }
    blocks
}

pub fn detect(text: &str) -> bool {
    if text.trim().is_empty() {
        return false;
    }
    // ls -R output always starts with ".:" (current dir) or "/path:" headers
    // followed by a blank line and then filenames
    text.lines().any(|line| {
        let t = line.trim_end();
        is_dir_header(t) && {
            let path = &t[..t.len() - 1];
            path == "." || path.starts_with("./") || path.starts_with('/')
        }
    })
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    let blocks = parse_blocks(text);
    if blocks.is_empty() {
        return crate::redact::redact(text.trim_end());
    }

    let n_dirs = blocks.len();
    let n_total_files: usize = blocks.iter().map(|b| b.files.len()).sum();

    let mut out = String::with_capacity(n_dirs * 40 + n_total_files * 15);
    let _ = write!(
        out,
        "[tokenade:ls-R] {n_dirs} director{} · {n_total_files} file{}\n\n",
        if n_dirs == 1 { "y" } else { "ies" },
        if n_total_files == 1 { "" } else { "s" },
    );

    for block in &blocks {
        let n = block.files.len();
        if n == 0 {
            let _ = writeln!(out, "{}:  (empty)", block.path);
            continue;
        }
        if n <= INLINE_CAP {
            let _ = write!(out, "{}:\n  {}\n", block.path, block.files.join("  "));
        } else {
            // Show first INLINE_CAP files, fold the rest
            let shown = &block.files[..INLINE_CAP];
            let folded = n - INLINE_CAP;
            let _ = write!(
                out,
                "{}:  [{n} files]  {}\n  ... {folded} more\n",
                block.path,
                shown.join("  ")
            );
        }
    }

    crate::redact::redact(out.trim_end())
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_ls_r(dirs: &[(&str, &[&str])]) -> String {
        let mut s = String::new();
        for (dir, files) in dirs {
            let _ = writeln!(s, "{dir}:");
            if !files.is_empty() {
                s.push_str(&files.join("  "));
                s.push('\n');
            }
            s.push('\n');
        }
        s
    }

    fn build_large_ls_r(n_dirs: usize, files_per_dir: usize) -> String {
        let mut s = String::new();
        s.push_str(".\n");
        for _ in 0..5 {
            let file = "placeholder";
            let _ = file;
        }
        // Build: ".:", "./src", "./tests", etc.
        let dir_names: Vec<String> = (0..n_dirs).map(|i| format!("./dir{i:03}")).collect();
        let file_names: Vec<Vec<String>> = (0..n_dirs)
            .map(|i| (0..files_per_dir).map(|j| format!("file{i:03}_{j:03}.rs")).collect())
            .collect();

        s.clear();
        s.push_str(".:\n");
        let top_files: Vec<String> = dir_names.iter().map(|d| d[2..].to_string()).collect();
        s.push_str(&top_files.join("  "));
        s.push_str("\n\n");

        for (i, dir) in dir_names.iter().enumerate() {
            let _ = writeln!(s, "{dir}:");
            s.push_str(&file_names[i].join("  "));
            s.push_str("\n\n");
        }
        s
    }

    // ── detect ────────────────────────────────────────────────────────────

    #[test]
    fn detects_ls_r_output() {
        let input = build_ls_r(&[
            (".", &["src", "tests", "Cargo.toml"]),
            ("./src", &["lib.rs", "main.rs"]),
        ]);
        assert!(detect(&input));
    }

    #[test]
    fn detects_absolute_paths() {
        let input = "/home/user:\nfoo.txt  bar.txt\n\n/home/user/docs:\nreadme.md\n\n";
        assert!(detect(input));
    }

    #[test]
    fn rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("   \n"));
    }

    #[test]
    fn rejects_arbitrary_text() {
        // The old broken detect matched everything non-empty
        assert!(!detect("hello world\nsome text\n"));
        assert!(!detect("git log\nsome commits\n"));
    }

    #[test]
    fn rejects_text_without_dir_header() {
        assert!(!detect("file1.rs\nfile2.rs\nfile3.rs\n"));
    }

    // ── quality: kept ─────────────────────────────────────────────────────

    #[test]
    fn quality_keeps_all_dir_paths() {
        let input = build_ls_r(&[
            (".", &["src", "tests"]),
            ("./src", &["lib.rs", "main.rs"]),
            ("./tests", &["test.rs"]),
        ]);
        let out = compact(&input);
        assert!(out.contains('.'), "root dir missing: {out}");
        assert!(out.contains("./src"), "src dir missing: {out}");
        assert!(out.contains("./tests"), "tests dir missing: {out}");
    }

    #[test]
    fn quality_keeps_files_in_small_dirs() {
        let input = build_ls_r(&[
            (".", &["src"]),
            ("./src", &["lib.rs", "main.rs", "utils.rs"]),
        ]);
        let out = compact(&input);
        assert!(out.contains("lib.rs"), "lib.rs missing: {out}");
        assert!(out.contains("main.rs"), "main.rs missing: {out}");
        assert!(out.contains("utils.rs"), "utils.rs missing: {out}");
    }

    #[test]
    fn quality_banner_shows_totals() {
        let input = build_large_ls_r(5, 3);
        let out = compact(&input);
        assert!(out.contains("director"), "directory count missing: {out}");
        assert!(out.contains("file"), "file count missing: {out}");
    }

    // ── quality: large dirs folded ────────────────────────────────────────

    #[test]
    fn quality_folds_large_directories() {
        let files: Vec<&str> = (0..50).map(|_| "file.rs").collect();
        let input = build_ls_r(&[(".", &["src"]), ("./src", &files)]);
        let out = compact(&input);
        assert!(out.contains("more") || out.contains("[50 files]"),
            "large dir fold missing: {out}");
    }

    #[test]
    fn quality_shows_first_files_in_large_dir() {
        let files: Vec<String> = (0..30).map(|i| format!("file{i:03}.rs")).collect();
        let file_refs: Vec<&str> = files.iter().map(|s| s.as_str()).collect();
        let input = build_ls_r(&[(".", &["src"]), ("./src", &file_refs)]);
        let out = compact(&input);
        assert!(out.contains("file000.rs"), "first file missing: {out}");
    }

    // ── regression: broken detect ─────────────────────────────────────────

    #[test]
    fn detect_no_longer_matches_everything() {
        assert!(!detect("random text\nanother line\n"));
        assert!(!detect("cargo build\nCompiling crate\n"));
    }

    // ── savings ───────────────────────────────────────────────────────────

    #[test]
    fn savings_on_large_run() {
        let input = build_large_ls_r(50, 20);
        let (_, sav) = compact_with_savings(&input);
        let pct = 100 * sav.tokens_before.saturating_sub(sav.tokens_after) / sav.tokens_before.max(1);
        assert!(pct >= 40, "large ls-R only saved {pct}%");
    }

    #[test]
    fn savings_on_many_small_dirs() {
        let input = build_large_ls_r(100, 5);
        let (_, sav) = compact_with_savings(&input);
        assert!(sav.tokens_after < sav.tokens_before,
            "ls-R did not save tokens: before={} after={}",
            sav.tokens_before, sav.tokens_after);
    }
}
