//! `grep -c` / `--count` — per-file count of matching lines.
//!
//! Real command-specific compactor (was: generic `line_cap(80)` behind a
//! `!text.trim().is_empty()` detector that matched literally anything). The cap
//! silently dropped every file past line 80 — and on a recursive `grep -rc`
//! that is exactly where the files WITH matches tend to live, so truncating
//! destroys the answer.
//!
//! Canonical shapes:
//! ```text
//! grep -c pat file        →  42                  (bare count, single file)
//! grep -rc pat dir/       →  src/main.rs:12      (path:count, one per file)
//!                            src/lib.rs:0
//!                            src/util.rs:3
//! ```
//!
//! Where the tokens go: a recursive `grep -c` prints a line for EVERY file it
//! searched, and on a real tree the overwhelming majority matched nothing
//! (`…:0`). Those zero lines are bulk; the signal is the handful of files that
//! actually have a non-zero count.
//!
//! Strategy: keep every non-zero `path:count` line verbatim (those are the
//! answer — never dropped or capped), and fold the zero-count files into a
//! single announced summary (`· N file(s) with 0 matches omitted`). The banner
//! also carries the true total of files-with-matches and the summed match
//! count — so nothing disappears silently; the reader always sees how many
//! files were searched-but-empty. When there is nothing to fold (no zero lines)
//! or the input is small, it passes through unchanged.
//!
//! Detection & parsing are STRUCTURAL, not label-based: a line is a count line
//! when the text after its FINAL `:` is an integer (or the whole line is a bare
//! integer). grep prints the count as a plain ASCII integer regardless of
//! `LANG`/`LC_*` (no thousands separators), and the paths — which may carry
//! accents or CJK — are preserved byte-for-byte. No English keyword anywhere.

const PASSTHROUGH_LINES: usize = 8;

/// Parse a `grep -c` line into `(path, count)`. A bare-integer line (single
/// file mode) has path `None`. Returns `None` when the line isn't a count line.
fn parse_count(line: &str) -> Option<(Option<&str>, u64)> {
    let t = line.trim();
    if t.is_empty() {
        return None;
    }
    // Bare count (single-file mode).
    if let Ok(n) = t.parse::<u64>() {
        return Some((None, n));
    }
    // `path:count` — split on the LAST colon (Windows `C:` drive letters and
    // URLs may contain earlier colons; the count is always the final field).
    let (path, num) = t.rsplit_once(':')?;
    let n = num.trim().parse::<u64>().ok()?;
    if path.is_empty() {
        return None;
    }
    Some((Some(path), n))
}

pub fn detect(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return false;
    }
    let count_lines = lines.iter().filter(|l| parse_count(l).is_some()).count();
    // Strong majority must parse as count lines (otherwise it's not grep -c).
    count_lines * 10 >= lines.len() * 9
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    let passthrough = || text.trim_end_matches('\n').to_string();

    let mut nonzero: Vec<&str> = Vec::new();
    let mut zero_files = 0usize;
    let mut total_matches: u64 = 0;
    let mut parsed = 0usize;

    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match parse_count(line) {
            Some((path, n)) => {
                parsed += 1;
                if n == 0 {
                    // A bare `0` (single-file mode) has no name to fold; keep it.
                    match path {
                        Some(_) => zero_files += 1,
                        None => nonzero.push(line.trim()),
                    }
                } else {
                    total_matches += n;
                    nonzero.push(line.trim());
                }
            }
            // A stray non-count line: don't risk mangling — passthrough.
            None => return passthrough(),
        }
    }

    if parsed < PASSTHROUGH_LINES || zero_files == 0 {
        return passthrough();
    }

    let mut out = format!(
        "[tokenade:grep-c] {} file(s) with matches · {total_matches} total match(es) · {zero_files} file(s) with 0 matches omitted\n",
        nonzero.len()
    );
    out.push_str(&nonzero.join("\n"));

    let compacted = out.trim_end_matches('\n').to_string();
    // Never emit a larger result than the input.
    if crate::tokenizer::estimate(&compacted) >= crate::tokenizer::estimate(&passthrough()) {
        return passthrough();
    }
    compacted
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    /// Recursive `grep -rc`: `hits` files with a non-zero count, `misses` files
    /// with `:0`. Realistic ratio is many misses, few hits.
    fn build_grep_c(hits: usize, misses: usize) -> String {
        let mut s = String::new();
        for i in 0..hits {
            let _ = writeln!(s, "src/module/component_{i:03}.rs:{}", 1 + i % 9);
        }
        for i in 0..misses {
            let _ = writeln!(s, "src/module/untouched_file_{i:03}.rs:0");
        }
        s
    }

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("   \n"));
    }

    #[test]
    fn detector_is_structural() {
        assert!(detect("a.rs:5\nb.rs:0\nc.rs:12\n"));
        assert!(detect("42\n")); // bare single-file count
        // Prose / non-count lines rejected (no integer suffix).
        assert!(!detect("hello world\nthis is prose\nno counts here\n"));
    }

    #[test]
    fn try_compact_some_or_none() {
        let s = build_grep_c(5, 20);
        let _ = try_compact(&s);
        let _ = compact(&s);
        let _ = compact_with_savings(&s);
    }

    #[test]
    fn quality_nonzero_kept_verbatim_zeros_folded_with_counter() {
        let input = build_grep_c(12, 200);
        let out = compact(&input);
        assert!(out.contains("[tokenade:grep-c]"), "banner missing: {out}");
        // True total of files-with-matches in the banner.
        assert!(out.contains("12 file(s) with matches"), "match-file total wrong: {out}");
        // Summed match count is announced.
        let expected_total: u64 = (0..12u64).map(|i| 1 + (i % 9)).sum();
        assert!(out.contains(&format!("{expected_total} total match(es)")),
            "summed total wrong: {out}");
        // Zero-count files folded WITH a counter — never silently gone.
        assert!(out.contains("200 file(s) with 0 matches omitted"),
            "zero fold not announced: {out}");
        // Every non-zero file:count survives verbatim.
        for i in 0..12 {
            assert!(out.contains(&format!("src/module/component_{i:03}.rs:{}", 1 + i % 9)),
                "nonzero line {i} dropped/mangled: {out}");
        }
        // The zero files' names are folded away (that's the compaction).
        assert!(!out.contains("untouched_file_000.rs"), "zero file should be folded: {out}");
    }

    #[test]
    fn quality_all_nonzero_passthrough() {
        // No zeros to fold → passthrough (a banner would only inflate).
        let input = build_grep_c(30, 0);
        let out = compact(&input);
        assert!(!out.contains("[tokenade:grep-c]"), "all-nonzero got a banner: {out}");
        assert!(out.contains("component_000.rs:1"), "lost a line: {out}");
    }

    #[test]
    fn bare_single_file_count_passthrough() {
        let out = compact("42\n");
        assert!(out.contains("42"), "bare count lost: {out}");
        assert!(!out.contains("[tokenade:grep-c]"), "bare count got a banner: {out}");
    }

    #[test]
    fn dispatcher_contract_small_input_passthrough() {
        // Mirrors `count_mode_routes_to_grep_count`: nonzero lines must survive.
        let out = compact("a.rs:5\nb.rs:0\nc.rs:12\n");
        assert!(out.contains("a.rs:5"), "nonzero a.rs dropped: {out}");
        assert!(out.contains("c.rs:12"), "nonzero c.rs dropped: {out}");
        // Under the fold threshold → passthrough, so the zero line is kept too.
        assert!(out.contains("b.rs:0"), "zero line lost on small input: {out}");
    }

    #[test]
    fn windows_path_with_colon_parsed_on_last_colon() {
        // Earlier colons (drive letter) must not confuse the count split.
        let mut input = String::new();
        for i in 0..10 {
            let _ = writeln!(input, "C:/proj/src/file_{i:02}.rs:{}", i + 1);
        }
        for i in 0..20 {
            let _ = writeln!(input, "C:/proj/src/empty_{i:02}.rs:0");
        }
        let out = compact(&input);
        assert!(out.contains("C:/proj/src/file_00.rs:1"), "windows path count mangled: {out}");
        assert!(out.contains("20 file(s) with 0 matches omitted"), "zero fold wrong: {out}");
    }

    #[test]
    fn multilingual_accented_and_cjk_paths_preserved() {
        // Non-ASCII paths + plain ASCII counts (grep never localises numbers).
        let mut input = String::new();
        input.push_str("app/modèles/café/élément.rs:7\n");
        input.push_str("アプリ/モデル/ファイル.rs:3\n");
        input.push_str("straße/groß.rs:1\n");
        for i in 0..30 {
            let _ = writeln!(input, "app/modèles/vide_{i:02}.rs:0");
        }
        assert!(detect(&input), "accented/CJK count list not detected structurally");
        let out = compact(&input);
        assert!(out.contains("3 file(s) with matches"), "count wrong under non-ASCII: {out}");
        assert!(out.contains("11 total match(es)"), "summed total wrong: {out}"); // 7+3+1
        // Non-ASCII matched files preserved verbatim.
        assert!(out.contains("app/modèles/café/élément.rs:7"), "accented path lost: {out}");
        assert!(out.contains("アプリ/モデル/ファイル.rs:3"), "CJK path lost: {out}");
        assert!(out.contains("straße/groß.rs:1"), "eszett path lost: {out}");
        assert!(out.contains("30 file(s) with 0 matches omitted"), "zero fold wrong: {out}");
    }

    #[test]
    fn savings_on_large_recursive_grep_c() {
        // 40 hits, 460 misses (typical recursive ratio).
        let (_, sav) = compact_with_savings(&build_grep_c(40, 460));
        assert!(sav.tokens_before > 0);
        let pct = 100 * (sav.tokens_before - sav.tokens_after) / sav.tokens_before;
        assert!(pct >= 70, "large grep -c only saved {pct}%");
    }
}
