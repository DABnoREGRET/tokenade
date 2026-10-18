//! `dpkg_listfiles` — `dpkg -L <pkg>` (files installed by a package).
//!
//! Real command-specific compactor (was: generic `line_cap(100)` behind a
//! WORTHLESS `!text.trim().is_empty()` detector — it kept the FIRST 100 path
//! lines and silently truncated the rest, reporting a meaningless "file(s)"
//! count. For a big package the tail (man pages, locale data, docs) is exactly
//! what got thrown away, with no indication anything was dropped).
//!
//! `dpkg -L` prints a flat, depth-first list of absolute paths — directories
//! and the files inside them, interleaved. The byte sink is many files sharing
//! one parent directory (`/usr/share/doc/<pkg>/…`, `/usr/share/man/…`,
//! locale trees). The directory STRUCTURE — which dirs the package populates —
//! is the useful signal; the exhaustive per-file enumeration is not.
//!
//! Strategy: group consecutive path lines by their parent directory; keep the
//! first [`KEEP_PER_DIR`] entries of each group verbatim and fold the rest with
//! an explicit `… (+N more under <dir>)` counter. A true-total banner reports
//! the path count and how many were folded; nothing is dropped silently. Short
//! listings pass through untouched.
//!
//! Multilingual: detection and grouping key ONLY on the absolute-path structure
//! (leading `/`, the last `/` separator) — never on an English word. dpkg's
//! own non-path notes (e.g. diversion lines) don't start with `/`, so they
//! break a group and are preserved verbatim. Accented / CJK file and directory
//! names (localized doc/man trees) survive byte-for-byte; folded counts include
//! them regardless of locale.

use std::fmt::Write;
/// Files kept per parent directory before folding the remainder.
const KEEP_PER_DIR: usize = 3;

/// The parent directory of an absolute path (text up to, excluding, the last
/// `/`). Top-level entries (`/usr`) group under `/`. Purely structural.
fn parent_dir(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) => "/",
        Some(i) => &path[..i],
        None => "",
    }
}

fn is_path(line: &str) -> bool {
    line.starts_with('/')
}

pub fn detect(text: &str) -> bool {
    if text.trim().is_empty() {
        return false;
    }
    // At least two absolute-path lines — rejects prose with no file listing.
    text.lines().filter(|l| is_path(l)).nth(1).is_some()
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.iter().filter(|l| is_path(l)).count();

    let mut body = String::new();
    let mut folded = 0usize;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if !is_path(line) {
            // dpkg note (diversion, etc.) — keep verbatim, breaks any group.
            body.push_str(line);
            body.push('\n');
            i += 1;
            continue;
        }
        let par = parent_dir(line);
        let mut j = i;
        while j < lines.len() && is_path(lines[j]) && parent_dir(lines[j]) == par {
            j += 1;
        }
        let group = j - i;
        if group > KEEP_PER_DIR {
            for k in 0..KEEP_PER_DIR {
                body.push_str(lines[i + k]);
                body.push('\n');
            }
            let more = group - KEEP_PER_DIR;
            let _ = writeln!(body, "  … (+{more} more under {par})");
            folded += more;
        } else {
            for k in 0..group {
                body.push_str(lines[i + k]);
                body.push('\n');
            }
        }
        i = j;
    }

    if folded == 0 {
        return text.trim_end_matches('\n').to_string();
    }
    format!(
        "[tokenade:dpkg -L] {total} path(s) · {folded} folded\n{}",
        body.trim_end_matches('\n')
    )
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `dpkg -L`: a small dir scaffold, then `files` files under each of a
    /// doc and a man directory.
    fn build_listing(files: usize) -> String {
        let mut s = String::from(
            "/.\n/usr\n/usr/bin\n/usr/bin/sample\n/usr/share\n/usr/share/doc\n/usr/share/doc/sample\n",
        );
        for f in 0..files {
            let _ = writeln!(s, "/usr/share/doc/sample/file{f:04}.txt");
        }
        s.push_str("/usr/share/man\n/usr/share/man/man1\n");
        for f in 0..files {
            let _ = writeln!(s, "/usr/share/man/man1/tool{f:04}.1.gz");
        }
        s
    }

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("   \n"));
    }

    #[test]
    fn detector_rejects_prose() {
        assert!(!detect("this is prose with no\nabsolute file paths at all\n"));
    }

    #[test]
    fn try_compact_some_or_none() {
        let s = "sample output line\nsecond line\n";
        let _ = try_compact(s);
        let _ = compact(s);
        let _ = compact_with_savings(s);
    }

    #[test]
    fn detects_file_listing() {
        assert!(detect(&build_listing(10)));
    }

    #[test]
    fn quality_true_total_dirs_kept_files_folded() {
        let out = compact(&build_listing(20));
        // 2 file groups × 20 = 40, plus the 9 scaffold/dir lines = 49 path
        // lines; each big group keeps 3, folds 17.
        assert!(out.contains("49 path(s)"), "true total lost: {out}");
        assert!(out.contains("34 folded"), "fold count wrong: {out}");
        assert!(out.contains("more under /usr/share/doc/sample"), "doc fold marker silent: {out}");
        assert!(out.contains("more under /usr/share/man/man1"), "man fold marker silent: {out}");
        // Directory scaffold + first files of each group survive verbatim.
        assert!(out.contains("/usr/bin/sample"), "named path lost: {out}");
        assert!(out.contains("/usr/share/doc/sample/file0000.txt"), "first doc file lost: {out}");
        assert!(out.contains("/usr/share/man/man1/tool0000.1.gz"), "first man file lost: {out}");
        // Deep entries within a group are folded.
        assert!(!out.contains("file0019.txt"), "deep doc file should be folded: {out}");
        assert!(!out.contains("tool0019.1.gz"), "deep man file should be folded: {out}");
    }

    #[test]
    fn quality_small_listing_passthrough_no_banner() {
        // Each group within KEEP_PER_DIR → nothing folds, no banner.
        let out = compact(&build_listing(2));
        assert!(!out.contains("[tokenade:dpkg -L]"), "small listing got a banner: {out}");
        assert!(out.contains("/usr/share/doc/sample/file0001.txt"), "lost a file: {out}");
        assert!(out.contains("/usr/share/man/man1/tool0001.1.gz"), "lost a file: {out}");
    }

    #[test]
    fn quality_non_path_notes_preserved() {
        // dpkg diversion notes don't start with `/` — kept verbatim, and they
        // break grouping so files on either side aren't merged.
        let input = "/usr/bin/foo\n\
            package diverts others to: /usr/bin/foo.real\n\
            /usr/share/doc/foo/a\n/usr/share/doc/foo/b\n/usr/share/doc/foo/c\n/usr/share/doc/foo/d\n";
        let out = compact(input);
        assert!(out.contains("package diverts others to: /usr/bin/foo.real"), "diversion note lost: {out}");
        assert!(out.contains("5 path(s)"), "true total wrong: {out}");
        assert!(out.contains("1 folded"), "fold count wrong: {out}");
    }

    #[test]
    fn multilingual_accented_and_cjk_paths_preserved() {
        // Localized doc/man trees: accented + CJK file names. Detection and
        // grouping key on `/` only, never on an English word.
        let mut input = String::from("/usr/share/doc/paquet\n");
        // First 3 kept verbatim, including an accented and a CJK name.
        input.push_str("/usr/share/doc/paquet/RELÈVE.fr.gz\n");
        input.push_str("/usr/share/doc/paquet/著作権.ja\n");
        input.push_str("/usr/share/doc/paquet/modèle.txt\n");
        for f in 0..20 {
            let _ = writeln!(input, "/usr/share/doc/paquet/fichier-é{f:02}.txt");
        }
        assert!(detect(&input), "accented/CJK listing not detected via structure");
        let out = compact(&input);
        assert!(out.contains("24 path(s)"), "count wrong under locale: {out}");
        assert!(out.contains("20 folded"), "fold count wrong under locale: {out}");
        // Accented / CJK kept files preserved verbatim.
        assert!(out.contains("/usr/share/doc/paquet/RELÈVE.fr.gz"), "accented file lost: {out}");
        assert!(out.contains("著作権.ja"), "CJK file lost: {out}");
        assert!(out.contains("modèle.txt"), "accented file lost: {out}");
        // Fold marker carries the parent dir; deep accented files folded.
        assert!(out.contains("more under /usr/share/doc/paquet"), "fold marker lost under locale: {out}");
        assert!(!out.contains("fichier-é19.txt"), "deep accented file should fold: {out}");
    }

    #[test]
    fn savings_on_large_listing() {
        let input = build_listing(500);
        let (_, sav) = compact_with_savings(&input);
        assert!(sav.tokens_before > 0);
        let pct = 100 * (sav.tokens_before - sav.tokens_after) / sav.tokens_before;
        assert!(pct >= 70, "large dpkg -L only saved {pct}%");
    }
}
