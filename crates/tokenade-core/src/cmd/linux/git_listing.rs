//! `git_listing` — shared fold engine for git list commands.
//!
//! Used by: git_ls_files, git_ls_tree, git_log_oneline, git_log_graph,
//!          git_branch_*, git_tag_list, git_rev_list, git_reflog,
//!          git_stash_list, git_worktree, git_diff_name_only,
//!          git_diff_numstat, git_status_short, git_config_list,
//!          git_submodule_status, git_submodule_foreach.
//!
//! Strategy: keep the first HEAD entries verbatim (most-recent /
//! most-important items), then fold the tail into a count announcement.
//! Returns `None` when no folding is needed (total <= head).
//!
//! For `git diff --name-only` and `git ls-files` the tail is also broken
//! down by top-level path prefix so the agent sees what areas changed.

use std::fmt::Write;
/// Fold a plain-line listing.  Returns `None` when total ≤ `head`.
pub fn fold(text: &str, head: usize, label: &str, noun: &str) -> Option<String> {
    fold_with(text, head, label, noun, Tail::Count)
}

/// Like [`fold`], plus a census of the rest by directory.
pub fn fold_paths(text: &str, head: usize, label: &str, noun: &str) -> Option<String> {
    fold_with(text, head, label, noun, Tail::Census)
}

/// Like [`fold_paths`], but a list that fits under the delivery threshold
/// is written WHOLE (grouped): for `grep -l`, the paths are the
/// answer. Measured 2026-10-02: `grep -rlIiE 'tofu|mofu|bofu' …` rendered as 80
/// paths + census, then `expand-ref --grep <folder>` twice.
/// A decision specific to this command, not to the engine: `find` or `dpkg -L`
/// keep their head of 80 (per-command compactor rule).
pub fn list_or_fold_paths(text: &str, head: usize, label: &str, noun: &str) -> Option<String> {
    let paths: Vec<&str> = text.lines().map(str::trim_end).filter(|l| !l.is_empty()).collect();
    if paths.len() > head {
        if let Some(listed) = crate::filelist::listed_whole(&paths) {
            return Some(listed);
        }
    }
    fold_paths(text, head, label, noun)
}

/// What the rest becomes: a count, or a count and a census.
///
/// The only thing that separated `fold` from `fold_paths`. Everything else — the
/// head budget, the banner, the common prefix, the rendering of the kept lines —
/// was written TWICE, and a fix applied to both identical bodies
/// is what showed it: a mutation's anchor was found there twice, so
/// the harness refused to give a verdict.
enum Tail {
    Count,
    Census,
}

/// Write the lines with `strip` leading bytes removed.
///
/// A line SHORTER than the prefix is written whole: it is the
/// root directory itself, which appears in its own listing, and
/// truncating it would return an empty line.
fn push_stripped(out: &mut String, lines: &[&str], strip: usize) {
    crate::filelist::push_grouped(out, lines, strip, &|l| Some(l));
}

fn fold_with(text: &str, head: usize, label: &str, noun: &str, tail_mode: Tail) -> Option<String> {
    let lines: Vec<&str> = text.lines().map(|l| l.trim_end()).filter(|l| !l.is_empty()).collect();
    let total = lines.len();
    // The read-mode ladder tunes the kept budget (and, in `entropy` mode,
    // *which* lines are kept). `task`/unset is identity, so this is a no-op
    // for the default — output stays byte-for-byte unchanged.
    let head = crate::read_mode::effective_head(head);
    if total <= head {
        // **Short enough to be kept whole is not « nothing to do ».**
        // The two decisions were welded: the common prefix was only stated
        // when the listing was long enough to be TRUNCATED. MEASURED
        // 2026-09-09 by `residue_census --delivered`: `find` on a home
        // folder returned its 2 437 bytes **whole**, thirty lines all
        // under `/Users/macos/Library/`.
        let (header, strip) = crate::filelist::common_prefix_header(&lines)?;
        let mut out = String::with_capacity(text.len());
        out.push_str(&header);
        out.push('\n');
        push_stripped(&mut out, &lines, strip);
        // And only if it pays: the header is not free.
        return (out.len() < text.len()).then_some(out);
    }
    let (kept, tail) = crate::read_mode::partition_indices(&lines, head);
    let tag = crate::read_mode::banner_tag();
    let mut out = format!("[tokenade:{label}{tag}] {total} {noun}\n");
    // **The common prefix, stated once.** A listing is made of paths that
    // share their root: `find /usr/share/doc` repeats it on each of the
    // two hundred and thirty-six lines delivered. The decision is `grep`'s,
    // shared — not a second writing.
    let kept_lines: Vec<&str> = kept.iter().map(|&i| lines[i]).collect();
    let strip = crate::filelist::common_prefix_header(&kept_lines);
    if let Some((header, _)) = &strip {
        out.push_str(header);
        out.push('\n');
    }
    let strip_len = strip.as_ref().map(|(_, n)| *n).unwrap_or(0);
    push_stripped(&mut out, &kept_lines, strip_len);
    match tail_mode {
        Tail::Count => {
            let _ = write!(out, "  ... (+{} more)", total - head);
        }
        Tail::Census => {
            // Summarise the tail by the shallowest directory depth that separates it.
            let tail_paths: Vec<&str> = tail.iter().map(|&i| lines[i]).collect();
            let summary = crate::filelist::path_census(&tail_paths);
            let _ = write!(out, "  ... (+{} more) — {summary}", total - head);
        }
    }
    Some(out)
}

// Required by the auto-dispatch table (every .rs in cmd/linux/ is registered).
// This module is a helper, not a standalone command — always decline.
crate::archetypes::cmd_helper_module!();

#[cfg(test)]
mod common_prefix_tests {
    use super::*;

    /// **A listing repeats its root on every line.**
    ///
    /// MEASURED 2026-09-08 on the corpus: `/usr/share/doc` appears 236 times
    /// in 6 captures, 11 138 bytes, and `find /usr/share/doc` was the biggest
    /// capture still delivered. The mechanism existed — `[paths from …]`
    /// — written privately in the `grep` compactor, while the twenty
    /// listing commands going through here delivered the whole prefix.
    /// **And each directory once, with its files below.**
    ///
    /// The invariant is RECONSTRUCTION: what the grouped rendering allows to
    /// read back must be exactly what it would have written line by line. Nothing
    /// is invented, nothing is missing, the order is the same.
    #[test]
    fn a_grouped_listing_reconstructs_exactly() {
        let mut lines: Vec<String> = vec!["faad2/README".into(), "faad2/AUTHORS".into()];
        // `find` lists the directory itself before its content.
        lines.push("libunistring".into());
        for i in 0..30 {
            lines.push(format!("libunistring/libunistring_{i}.html"));
        }
        lines.push("seul-en-haut".into());
        lines.push("ailleurs/x".into());
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let mut out = String::new();
        crate::filelist::push_grouped(&mut out, &refs, 0, &|l| Some(l));
        assert!(
            out.contains("libunistring/\n"),
            "the grouping did not fire:\n{out}"
        );
        assert!(out.len() < refs.iter().map(|l| l.len() + 1).sum::<usize>());
        // Two files do not pay: `faad2/` stays written twice.
        assert!(out.contains("faad2/README"), "{out}");

        let mut seen: Vec<String> = Vec::new();
        let mut dir = String::new();
        for line in out.lines() {
            if let Some(rest) = line.strip_prefix("  ") {
                seen.push(format!("{dir}/{rest}"));
            } else if let Some(d) = line.strip_suffix('/') {
                dir = d.to_string();
            } else {
                seen.push(line.to_string());
            }
        }
        // The only line that does not read back as is is the entry of the
        // directory itself: the `libunistring/` header SAYS it, and
        // rewriting it above would say it twice.
        let expected: Vec<String> = lines
            .iter()
            .filter(|l| *l != "libunistring")
            .cloned()
            .collect();
        assert_eq!(seen, expected, "the reconstruction does not return the listing");
        assert!(
            !out.contains("\nlibunistring\n"),
            "the directory entry is written twice:\n{out}"
        );
    }

    /// **The profitability gate, on BOTH sides.**
    ///
    /// Three files suffice in NUMBER, not in bytes: the header costs the
    /// directory name plus two, and each indented line costs two
    /// spaces to save `len(dir) + 1`. The gate is the repo's
    /// (`fold_gate::pays`, ten percent margin), so a directory of
    /// four characters with three files does not return enough. The test holds
    /// both sides: the smallest case that pays, and the one just below.
    ///
    /// Without this case the gate is UNKILLABLE — measured: the mutation that removes it
    /// survived a fixture whose directories are five and twelve
    /// characters long.
    #[test]
    fn a_group_that_does_not_pay_is_refused() {
        let render = |lines: &[&str]| {
            let mut out = String::new();
            crate::filelist::push_grouped(&mut out, lines, 0, &|l| Some(l));
            out
        };
        // The gate is the repo's: ten percent at least. With three
        // files, `abcd/` does not return it — the group would change the shape
        // for almost nothing.
        let short = ["abcd/x.txt", "abcd/y.txt", "abcd/z.txt"];
        let flat = render(&short);
        assert!(!flat.contains("abcd/\n"), "a group that does not pay was written:\n{flat}");
        assert_eq!(flat, "abcd/x.txt\nabcd/y.txt\nabcd/z.txt\n");

        // One more character in the directory and it pays: it is the smallest
        // accepted case, measured, not assumed.
        let long = ["abcde/x.txt", "abcde/y.txt", "abcde/z.txt"];
        let grouped = render(&long);
        assert!(grouped.contains("abcde/\n"), "the smallest case that pays was refused:\n{grouped}");
        assert!(
            crate::fold_gate::pays(
                long.iter().map(|l| l.len() + 1).sum::<usize>(),
                grouped.len(),
                0
            ),
            "the accepted case does not cross the shared gate"
        );
    }

    /// And the case happens on the real capture, the heaviest of the ranking.
    #[test]
    fn the_real_find_capture_groups_its_directories() {
        let body = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/linux/find_usr_share_doc.txt"),
        )
        .expect("the corpus `find` capture");
        let out = crate::filter::filter_command(&["find".to_string()], &body).output;
        assert!(
            out.contains("libunistring/\n  libunistring_"),
            "the grouping did not fire:\n{}",
            &out[..out.len().min(400)]
        );
    }

    #[test]
    fn a_listing_states_its_common_root_once() {
        let text: String = (0..300)
            .map(|i| format!("/usr/share/doc/package{i}/README.md\n"))
            .collect();
        let out = fold_paths(&text, 200, "find", "find path(s)").expect("long enough to fold");
        assert!(out.contains("[paths from /usr/share/doc/]"), "{}", &out[..300.min(out.len())]);
        assert_eq!(
            out.matches("[paths from ").count(),
            1,
            "the root is stated ONCE"
        );
        // The kept lines lost the root, and nothing else.
        assert!(out.contains("\npackage7/README.md\n"), "{}", &out[..400.min(out.len())]);
        assert!(!out.contains("/usr/share/doc/package7"), "the root is still there");
        // And the fold pays: that is the whole point.
        assert!(out.len() < text.len() / 2, "{} octets", out.len());
    }

    /// **Short enough to be kept whole is not « nothing to do ».**
    ///
    /// MEASURED 2026-09-09: `find` on a home folder returned its 2 437
    /// bytes WHOLE — thirty lines all under `/Users/macos/Library/`.
    /// The common prefix was only stated when the listing was long enough to
    /// be TRUNCATED: two welded decisions.
    #[test]
    fn a_listing_kept_whole_still_states_its_root() {
        // The subdirectories ALTERNATE: otherwise the common root goes down
        // one level and the test checks something other than what it announces.
        let text: String = (0..30)
            .map(|i| {
                let sub = if i % 2 == 0 { "Saved State" } else { "Application Support" };
                format!("/Users/macos/Library/{sub}/app{i}/windows.plist\n")
            })
            .collect();
        // Thirty lines, a ceiling of two hundred: nothing is truncated.
        let out = fold_paths(&text, 200, "find", "find path(s)")
            .expect("the common root pays, even without truncation");
        assert!(out.contains("[paths from /Users/macos/Library/]"), "{out}");
        assert!(!out.contains("more)"), "nothing must be announced as elided:\n{out}");
        assert!(out.contains("Application Support/app7/windows.plist"), "{out}");
        assert!(!out.contains("/Users/macos/Library/Application Support/app7"), "the root remained");
        assert!(out.len() < text.len(), "{} → {}", text.len(), out.len());
    }

    /// And a short listing WITHOUT a common root is not rewritten at all.
    #[test]
    fn a_short_listing_without_a_root_is_left_alone() {
        let text: String = (0..30).map(|i| format!("/r{i}/a/b/file{i}.txt\n")).collect();
        assert!(fold_paths(&text, 200, "find", "find path(s)").is_none());
    }

    /// A listing without a common root is not rewritten, and announces nothing.
    #[test]
    fn a_listing_without_a_common_root_is_left_alone() {
        let text: String = (0..300)
            .map(|i| format!("/root{i}/some/where/file{i}.txt\n"))
            .collect();
        let out = fold_paths(&text, 200, "find", "find path(s)").expect("long enough to fold");
        assert!(!out.contains("[paths from "), "{}", &out[..200.min(out.len())]);
        assert!(out.contains("/root7/some/where/file7.txt"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_returns_none_when_short() {
        let text = "a\nb\nc\n";
        assert!(fold(text, 20, "git-ls", "file(s)").is_none());
    }

    #[test]
    fn fold_returns_some_when_long() {
        let text = (0..30).map(|i| format!("item{i}\n")).collect::<String>();
        let out = fold(&text, 20, "git-branch", "branch(es)").unwrap();
        assert!(out.contains("[tokenade:git-branch] 30 branch(es)"));
        assert!(out.contains("item0"));
        assert!(out.contains("item19"));
        assert!(!out.contains("item20"));
        assert!(out.contains("(+10 more)"));
    }

    #[test]
    fn fold_paths_groups_tail_by_dir() {
        let paths = (0..25)
            .map(|i| format!("src/file{i}.rs\n"))
            .chain((0..5).map(|i| format!("tests/test{i}.rs\n")))
            .collect::<String>();
        let out = fold_paths(&paths, 20, "git-diff", "file(s)").unwrap();
        assert!(out.contains("[tokenade:git-diff] 30 file(s)"));
        assert!(out.contains("src/ ×"));
        assert!(out.contains("tests/ ×"));
    }
}

// `path_census` moved to `filelist`: it already calls
// `filelist::common_path_segments`, and `tsc` asks for it from another
// layer. A primitive parked with its first caller forces all the
// following ones to climb up to it.


#[cfg(test)]
mod census_tests {
    use super::*;

    /// MEASURED 2026-08-04 on this repo: `find crates -name "*.rs"` summarised
    /// 1 092 paths as `crates/ ×1092` — the search root, restated, with the
    /// total the reader already had. Giving `find` two roots UNDER `crates/`
    /// produced the same single bucket.
    /// The same question, and the answer that was never extended to it.
    ///
    /// The test below was written for `find crates …`, a RELATIVE root one
    /// level deep, and the deepening loop reached past it in one step. An
    /// ABSOLUTE root is four or five levels deep and the loop quits at the
    /// first level that fails to separate, so the census restated the total.
    /// Measured on ten real trees: ten said `/ ×N`.
    #[test]
    fn a_census_under_an_absolute_root_names_what_actually_differs() {
        let mut owned: Vec<String> = Vec::new();
        for i in 0..40 {
            owned.push(format!("/home/u/.cache/proj/src/a_{i}.rs"));
        }
        for i in 0..7 {
            owned.push(format!("/home/u/.cache/proj/docs/b_{i}.md"));
        }
        let paths: Vec<&str> = owned.iter().map(String::as_str).collect();
        let census = crate::filelist::path_census(&paths);
        assert!(
            census.contains("/home/u/.cache/proj/src/"),
            "the census did not reach past the shared root: {census}"
        );
        assert!(
            census.contains("/home/u/.cache/proj/docs/"),
            "only one side of the split was named: {census}"
        );
        assert!(
            !census.starts_with("/ \u{d7}"),
            "still restating the total: {census}"
        );
    }

    #[test]
    fn a_census_under_one_root_names_what_actually_differs() {
        let mut paths: Vec<String> = Vec::new();
        for i in 0..900 {
            paths.push(format!("crates/tokenade-core/src/m{i}.rs"));
        }
        for i in 0..60 {
            paths.push(format!("crates/tokenade-cli/src/m{i}.rs"));
        }
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let out = crate::filelist::path_census(&refs);
        assert!(
            out.contains("crates/tokenade-core/ \u{d7}900") && out.contains("crates/tokenade-cli/ \u{d7}60"),
            "the census still stops at the shared root: {out}"
        );
        assert!(!out.starts_with("crates/ "), "restated the search root: {out}");
    }

    /// When the top level already separates them, the census must not go
    /// deeper — the shallow grouping is the useful one.
    #[test]
    fn a_census_stays_shallow_when_the_top_level_already_separates() {
        let refs = vec!["src/a.rs", "src/b.rs", "docs/x.md", "tests/t.rs"];
        let out = crate::filelist::path_census(&refs);
        assert!(out.contains("src/ \u{d7}2"), "lost the shallow grouping: {out}");
        assert!(!out.contains("src/a"), "went deeper than it needed: {out}");
    }

    /// A long tail of directories must be counted, never silently dropped.
    #[test]
    fn a_census_counts_the_directories_it_does_not_list() {
        let paths: Vec<String> = (0..30).map(|i| format!("d{i}/f.rs")).collect();
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let out = crate::filelist::path_census(&refs);
        assert!(out.contains("other dir(s)"), "dropped the tail without saying so: {out}");
        let listed: usize = out.matches('\u{d7}').count();
        assert!(listed <= 9, "listed too many buckets to be a summary: {out}");
    }

    /// Through the real entry point, not just the helper: `find` reaches this
    /// via `find::compact` → `fold_paths`, and the census only helps if the
    /// wiring carries it.
    ///
    /// A live check through `tokenade wrap` is NOT reliable here — the fold
    /// store re-serves a previously compacted result for the same template, so
    /// the old summary keeps coming back. Same trap recorded on 2026-08-04.
    #[test]
    fn fold_paths_carries_the_deeper_census() {
        let mut text = String::new();
        for i in 0..900 {
            text.push_str(&format!("crates/tokenade-core/src/m{i}.rs\n"));
        }
        for i in 0..60 {
            text.push_str(&format!("crates/tokenade-cli/src/m{i}.rs\n"));
        }
        let out = fold_paths(&text, 5, "find", "find path(s)").expect("should fold");
        assert!(
            out.contains("crates/tokenade-core/ \u{d7}") && out.contains("crates/tokenade-cli/ \u{d7}"),
            "the wiring still reports the shared root: {}",
            out.lines().next_back().unwrap_or("")
        );
    }
}
