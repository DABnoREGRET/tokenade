//! `git_diff_numstat` — git diff --numstat.
//!
//! Format: `N\tN\tpath/to/file` (additions, deletions, filename); a binary
//! change is `-\t-\tpath`.
//!
//! RECONSTRUCTION-class compactor: it keeps the first `HEAD` rows verbatim and
//! folds the rest into a count — but git emits rows in PATH order, NOT by
//! magnitude, so the single largest change in a big diff sits at a random
//! position and a head-only fold would drop the answer to the most common
//! review question ("what changed the most?"). So, like `strace::fold_trace`
//! always retains the failed syscalls from the folded tail, this always
//! surfaces the `TOP_K` largest changes (and every binary change) from the
//! folded tail under a banner. Nothing notable is dropped silently.
//!
//! And what IS folded says where it lives: the rows past the head and the
//! top-K are still a list of paths, so the elision carries the same directory
//! census `find` has always printed.

use std::fmt::Write as _;

const HEAD: usize = 20;
const TOP_K: usize = 8;

pub fn detect(text: &str) -> bool {
    text.lines().any(|l| {
        let parts: Vec<&str> = l.trim().splitn(3, '\t').collect();
        parts.len() == 3 && (parts[0].parse::<usize>().is_ok() || parts[0] == "-")
    })
}

/// `additions + deletions` for a numstat row; a binary change (`-`) sorts first
/// (it has no line count but is always worth surfacing, never folding silently).
fn magnitude(line: &str) -> usize {
    let parts: Vec<&str> = line.trim().splitn(3, '\t').collect();
    if parts.len() < 3 {
        return 0;
    }
    if parts[0] == "-" || parts[1] == "-" {
        return usize::MAX;
    }
    parts[0].parse::<usize>().unwrap_or(0) + parts[1].parse::<usize>().unwrap_or(0)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let total = lines.len();
    // Every row while the table fits: 52 rows in 2.4 KB (`git diff --numstat
    // HEAD~20`, measured 2026-10-03) came back as 20 + the top 8.
    if total <= HEAD || tk_base::delivery::fits(text) {
        return None;
    }

    let mut out = String::new();
    let _ = writeln!(out, "[tokenade:git-diff-numstat] {total} file(s) changed");
    for l in &lines[..HEAD] {
        let _ = writeln!(out, "{l}");
    }

    // From the folded tail, always surface the largest changes by magnitude so a
    // big (or binary) change buried past the head window is never dropped.
    let mut tail: Vec<(usize, &str)> = lines[HEAD..].iter().map(|l| (magnitude(l), *l)).collect();
    // Stable sort: ties keep their original (path) order. `Reverse` gives
    // descending magnitude without a hand-written comparator.
    tail.sort_by_key(|t| std::cmp::Reverse(t.0));
    let shown = TOP_K.min(tail.len());
    if shown > 0 {
        let _ = writeln!(
            out,
            "[tokenade:git-diff-numstat] {shown} largest change(s) in folded tail:"
        );
        for (_, l) in &tail[..shown] {
            let _ = writeln!(out, "{l}");
        }
    }
    let remaining = tail.len() - shown;
    if remaining > 0 {
        // WHERE the elided rows are, not just how many. The header of this
        // module says "nothing notable is dropped silently" and means it about
        // MAGNITUDE — the largest changes are surfaced above. The remaining
        // rows are still a list of paths, and a bare count says nothing about
        // which part of the tree moved. `find`'s fold has disclosed exactly
        // this since it was written; this one, next door, never did.
        let rest: Vec<&str> = tail[shown..]
            .iter()
            .filter_map(|(_, l)| l.trim().splitn(3, '\t').nth(2))
            .collect();
        if rest.is_empty() {
            let _ = writeln!(out, "  ... (+{remaining} more)");
        } else {
            let census = crate::filelist::path_census(&rest);
            let _ = writeln!(out, "  ... (+{remaining} more) — {census}");
        }
    }
    Some(out.trim_end().to_string())
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detector_rejects_empty() {
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        assert!(!detect(""));
        assert!(!detect("   \n"));
    }

    #[test]
    fn detector_accepts_numstat() {
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        let s = "10\t5\tsrc/main.rs\n3\t1\tCargo.toml\n";
        assert!(detect(s));
    }

    #[test]
    fn short_numstat_passes_through() {
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        let s = "10\t5\tsrc/main.rs\n3\t1\tCargo.toml\n";
        assert!(try_compact(s).is_none());
    }

    #[test]
    fn long_numstat_folds_and_surfaces_biggest_tail() {
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        let s: String = (0..30).map(|i| format!("{i}\t1\tsrc/file{i}.rs\n")).collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("[tokenade:git-diff-numstat] 30 file(s) changed"));
        // Head entries preserved verbatim.
        assert!(out.contains("src/file0.rs"));
        // The biggest change in the folded tail (file29, magnitude 30) is surfaced…
        assert!(out.contains("largest change(s) in folded tail"));
        assert!(out.contains("src/file29.rs"));
        // …but a small tail entry stays folded.
        assert!(!out.contains("src/file20.rs"));
        assert!(out.contains("(+2 more)"));
    }

    #[test]
    fn biggest_tail_change_is_never_folded_away() {
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        // 40 tiny changes then one huge one at the very end (PATH order, as git
        // emits). The head window is 20, so the huge change is in the folded
        // tail — it MUST still be surfaced.
        let mut s = String::new();
        for i in 0..40 {
            s.push_str(&format!("2\t1\tsrc/f{i}.rs\n"));
        }
        s.push_str("1450\t12\tdb/migrations/0099_backfill_accounts.sql\n");
        let out = try_compact(&s).unwrap();
        assert!(out.contains("db/migrations/0099_backfill_accounts.sql"));
        assert!(out.contains("1450"));
    }

    #[test]
    fn binary_change_in_tail_is_surfaced() {
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        let mut s = String::new();
        for i in 0..30 {
            s.push_str(&format!("1\t1\tsrc/f{i}.rs\n"));
        }
        s.push_str("-\t-\tassets/logo.png\n");
        let out = try_compact(&s).unwrap();
        assert!(out.contains("assets/logo.png"));
    }
    /// The elided rows are PATHS, so the elision says where they live.
    ///
    /// A bare `... (+93 more)` passes the fuzzer's disclosure invariant —
    /// its marker set contains the word "more" — so nothing but a test that
    /// asks for the census can hold this.
    #[test]
    fn the_folded_tail_says_where_the_files_are() {
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        let mut s = String::new();
        for i in 0..60 {
            let _ = writeln!(s, "{i}\t{i}\tcrates/core/src/m{i}.rs");
        }
        for i in 0..40 {
            let _ = writeln!(s, "1\t1\tdocs/page_{i}.md");
        }
        let out = try_compact(&s).expect("should fold");
        let last = out.lines().last().unwrap_or_default();
        assert!(last.contains("more"), "no elision line at all: {out}");
        assert!(
            last.contains("crates/") && last.contains("docs/"),
            "the elision does not say where the files are: {last}"
        );
    }

    /// And it stays a count when there is nothing path-shaped to census.
    #[test]
    fn a_tail_without_paths_still_counts() {
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        let mut s = String::new();
        for i in 0..60 {
            let _ = writeln!(s, "{i}\t{i}\tfile{i}");
        }
        let out = try_compact(&s).expect("should fold");
        assert!(out.contains("more"), "{out}");
    }


    #[test]
    fn a_numstat_that_fits_is_left_whole() {
        let _g = tk_base::delivery::threshold_on_this_thread(5_000);
        let text: String = (0..52).map(|i| format!("{}\t{}\tcrates/core/src/file_{i}.rs\n", i % 9, i % 4)).collect();
        assert!(try_compact(&text).is_none());
    }
}
