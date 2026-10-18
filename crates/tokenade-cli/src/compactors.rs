//! Per-format compaction command handlers (`tokenade web|csv|json|yaml|
//! logs|...|snapshot`). Leaf commands extracted from main.rs (refactor).
#![allow(unused_imports)]
mod nudges;
mod web_batch;
use crate::wrap::*;
use crate::*;
pub(crate) use nudges::*;
pub(crate) use web_batch::*;
mod auto_cmd;
mod delivery;
mod media;
mod search_cmd;
mod serve_doc;
mod snapshot;
mod web_cmd;
pub(crate) use auto_cmd::*;
pub(crate) use delivery::*;
// `slice_lines` has a namesake reachable through `use crate::*`. It used to be
// defined HERE, so it won by shadowing; moved into a child, it arrives through
// a second glob and the two become ambiguous. The explicit import settles it,
// and names the one this module has always called.
use delivery::slice_lines;
pub(crate) use media::*;
pub(crate) use search_cmd::*;
pub(crate) use serve_doc::*;
pub(crate) use snapshot::*;
pub(crate) use web_cmd::*;

#[cfg(test)]
mod ordinal_tests {
    use super::ordinal;
    #[test]
    fn ordinals_read_like_english() {
        assert_eq!(ordinal(1), "1st");
        assert_eq!(ordinal(2), "2nd");
        assert_eq!(ordinal(3), "3rd");
        assert_eq!(ordinal(4), "4th");
        // The teens are the whole reason this is not `n % 10`.
        assert_eq!(ordinal(11), "11th");
        assert_eq!(ordinal(12), "12th");
        assert_eq!(ordinal(13), "13th");
        assert_eq!(ordinal(21), "21st");
        assert_eq!(ordinal(112), "112th");
        assert_eq!(ordinal(122), "122nd");
    }
}

#[cfg(test)]
mod search_argument_tests {
    use super::path_shaped_but_missing;

    /// A path that is not there is not a query.
    ///
    /// MEASURED 2026-09-04: `tokenade search /nope/missing.html` answered
    /// `[tokenade:search via bing+duckduckgo] "/nope/missing.html"` with exit
    /// 0 — a typo that reads as a successful read, and a local path (user
    /// name, client name, project name) handed to two search engines.
    #[test]
    fn a_missing_path_is_refused_and_a_real_query_is_not() {
        for q in [
            "/nope/missing.html",
            "./saved.html",
            "../a/b.htm",
            "~/Downloads/results.xhtml",
            // Ends like a page without opening like a path…
            "results.html",
            // …and opens like a path without ending like a page.
            "/nope/saved-results",
            "./downloads/serp",
        ] {
            assert!(path_shaped_but_missing(q), "not caught: {q}");
        }
        // A query may carry a slash, and refusing it to catch a typo trades a
        // working use for a broken one.
        for q in [
            "BurntSushi/ripgrep",
            "rust tokio cancellation safety",
            "-Wall meaning",
            "and/or operator",
            // A sentence that ends like a page is still a sentence.
            "difference between index.html and index.htm",
            "-",
            "",
        ] {
            assert!(!path_shaped_but_missing(q), "a real query was refused: {q}");
        }
        // A file that IS there is a saved page, not a refusal.
        let f = std::env::temp_dir().join("tokenade-search-arg-probe.html");
        std::fs::write(&f, "<html></html>").unwrap();
        assert!(!path_shaped_but_missing(&f.to_string_lossy()));
        let _ = std::fs::remove_file(&f);
    }
}

#[cfg(test)]
mod passage_line_tests {
    use super::annotate_passage_lines;

    const SRC: &str = "alpha line that is long enough\n\
                       \n\
                       beta line that is also long enough\n\
                       gamma line which is long as well\n\
                       \n\
                       delta line here and long enough\n";

    /// Both renderings of an answer get the same labels: the breadcrumbed one
    /// carries it on the breadcrumb, the plain one opens the block with it.
    #[test]
    fn a_block_is_labelled_with_the_lines_it_came_from() {
        let crumbed = "> doc > section\nbeta line that is also long enough\ngamma line which is long as well\n";
        let out = annotate_passage_lines(crumbed, SRC);
        assert!(
            out.starts_with("> doc > section  [lines 3-4]"),
            "the breadcrumb must carry the range:\n{out}"
        );

        let plain = "beta line that is also long enough\ngamma line which is long as well\n…\ndelta line here and long enough\n";
        let out = annotate_passage_lines(plain, SRC);
        assert!(out.contains("[lines 3-4]"), "{out}");
        assert!(
            out.contains("[line 6] delta line"),
            "the second block too:\n{out}"
        );
        // Nothing of the answer may be lost to the labelling.
        for line in plain.lines() {
            assert!(out.contains(line), "{line:?} disappeared:\n{out}");
        }
    }

    /// A range the block does not carry in full says how much is missing: the
    /// compacted view drops `}` and `#[test]`, and a bare label read as
    /// "these are lines A-B" — code copied from it lost its braces.
    #[test]
    fn a_range_with_dropped_lines_says_how_many() {
        let src: String = (0..3)
            .map(|i| format!("#[test]\nfn case_{i}() {{\n    let repo = a_hand_built_repo(\"repo-{i}\");\n    assert!(repo.ready(), \"case {i}\");\n}}\n\n"))
            .collect();
        let shown: String = (0..3)
            .map(|i| format!("fn case_{i}() {{\n    let repo = a_hand_built_repo(\"repo-{i}\");\n    assert!(repo.ready(), \"case {i}\");\n"))
            .collect();
        let out = annotate_passage_lines(&shown, &src);
        // Lines 2-16 hold 13 non-blank lines; the block carries 9.
        assert!(out.starts_with("[lines 2-16, 4 omitted]\n"), "{out}");
        // Whole, it stays a bare range (`}` is too short to anchor the end).
        let whole: String = src
            .lines()
            .skip(1)
            .take(4)
            .map(|l| format!("{l}\n"))
            .collect();
        assert!(annotate_passage_lines(&whole, &src).starts_with("[lines 2-4]\n"));
    }

    /// A passage the source cannot confirm is left unlabelled. A wrong number
    /// sends the next read to the wrong place and looks authoritative doing it.
    #[test]
    fn an_unverifiable_passage_gets_no_number() {
        let out = annotate_passage_lines("this text is nowhere in the source\n", SRC);
        assert!(!out.contains("[line"), "a guess was printed:\n{out}");
        // Short lines are not evidence either: `}` occurs everywhere.
        let out = annotate_passage_lines("}\n", "}\n}\n}\n");
        assert!(
            !out.contains("[line"),
            "a short line was used as evidence:\n{out}"
        );
    }

    /// A block that OPENS on a heading is still located: the heading is too
    /// short to be evidence, the bullet under it is not.
    ///
    /// Measured 2026-09-04 on one real answer: 14 of 17 blocks hold a
    /// locatable line, and 4 of the 6 that went unlabelled opened on exactly
    /// such a heading (`## 1.1.4`).
    #[test]
    fn a_block_is_anchored_on_its_first_locatable_line() {
        let src =
            "## 1.1.4\nalpha line that is long enough here\nbeta line that is long enough too\n";
        let out = annotate_passage_lines("## 1.1.4\nalpha line that is long enough here\n", src);
        assert!(
            out.contains("[lines 2-2]"),
            "the heading must not sink the block:\n{out}"
        );
    }

    /// A truncated line is matched by its opening, because the compacted view
    /// TRUNCATES rather than rewrites — but only when there is enough of it to
    /// be evidence.
    #[test]
    fn a_truncated_line_is_matched_by_what_survives_of_it() {
        let full = "the quick brown fox jumps over the lazy dog and keeps running east";
        let src = format!("first line long enough to be evidence\n{full}\n");
        let cut = &full[..48];
        let out = annotate_passage_lines(&format!("{cut}…\n"), &src);
        assert!(out.contains("[line 2] the quick"), "{out}");
        // Too short to be evidence: no label rather than the first line that
        // happens to start that way.
        // A REAL prefix of the source line, but a short one: 15 characters,
        // which is not evidence. Without the floor this labels; with it it
        // stays silent.
        let out = annotate_passage_lines("the quick brown…\n", &src);
        assert!(
            !out.contains("[line"),
            "a weak prefix was used as evidence:\n{out}"
        );
    }

    /// A span far wider than the block is not a region, and the block is left
    /// UNLABELLED rather than pinned to the line it opens on.
    ///
    /// This test asserted `[lines 1-1]` for one iteration, which is the
    /// fallback this version removes. Measured 2026-09-04 over three real
    /// answers about this repository's PATCHNOTES.md: 3 of the 12 labelled
    /// blocks were merged ones, and the worst printed `[lines 27-27]` on 21
    /// lines whose content reaches line 895. `[lines A-B]` means "the passage
    /// is here" on every other block; a merged block has no such span, and
    /// naming its first line makes the same notation mean one line in twenty
    /// with nothing to tell them apart.
    #[test]
    fn a_scattered_block_is_not_labelled_at_all() {
        let mut src = String::from("alpha line that is long enough here\n");
        for i in 0..300 {
            src.push_str(&format!("filler line number {i} of the document\n"));
        }
        src.push_str("omega line that is long enough here\n");
        let out = annotate_passage_lines(
            "alpha line that is long enough here\nomega line that is long enough here\n",
            &src,
        );
        assert!(
            !out.contains("[line"),
            "a two-line block spanning 302 lines was given a number:\n{out}"
        );

        // The neighbouring shape must keep its label: a block whose end is
        // simply not locatable is still honestly anchored on its first line.
        let src = "alpha line that is long enough here\nomega line that is long enough here\n";
        let out = annotate_passage_lines(
            "alpha line that is long enough here\nthis line is nowhere in the source at all\n",
            src,
        );
        assert!(
            out.contains("[lines 1-1]"),
            "a contiguous block lost its anchor:\n{out}"
        );
    }

    /// A one-line passage carries its number inline, and two of them in a row
    /// drop the blank + `…` between them: the numbers show the gap.
    #[test]
    fn single_line_passages_are_labelled_inline() {
        let out = annotate_passage_lines(
            "alpha line that is long enough\n\n…\ndelta line here and long enough\n\n…\nbeta line that is also long enough\ngamma line which is long as well\n",
            SRC,
        );
        assert_eq!(
            out,
            "[line 1] alpha line that is long enough\n[line 6] delta line here and long enough\n…\n[lines 3-4]\nbeta line that is also long enough\ngamma line which is long as well\n"
        );
    }

    /// On a slice the labels are the FILE's lines, not the slice's.
    #[test]
    fn a_slice_is_labelled_in_file_coordinates() {
        let sliced = format!("[tokenade: lines 120-125 of 900]\n{SRC}");
        let out = annotate_passage_lines("beta line that is also long enough\n", &sliced);
        assert!(
            out.contains("[line 122] beta"),
            "expected the file's line 122 (slice line 3):\n{out}"
        );
    }
}

#[cfg(test)]
mod line_range_tests {
    use super::{parse_line_spec, slice_lines};

    /// The four shapes the flag accepts, and what it refuses.
    #[test]
    fn a_line_spec_is_read_the_way_it_is_written() {
        assert_eq!(parse_line_spec("120-260"), Ok((120, 260)));
        assert_eq!(parse_line_spec(" 42 "), Ok((42, 42)));
        assert_eq!(parse_line_spec("-80"), Ok((1, 80)));
        assert_eq!(parse_line_spec("120-"), Ok((120, usize::MAX)));
        for bad in ["", "abc", "0", "0-5", "12-x", "80-20"] {
            assert!(
                parse_line_spec(bad).is_err(),
                "{bad:?} must be refused, not guessed at"
            );
        }
    }

    /// A range past the end is refused; a range that merely LANDS on blank
    /// lines is served.
    ///
    /// Both were defects in the first cut of this flag, found by running the
    /// matrix before shipping it: clamping the start answered a request for
    /// lines 9000-9100 of a 1 154-line file with line 1 154, and judging the
    /// slice by `is_empty()` refused line 42 of that file because line 42 is
    /// blank. Out of range, empty and blank are three different facts.
    #[test]
    fn out_of_range_and_blank_are_not_the_same_answer() {
        let text = "one\n\nthree\nfour\n";
        assert_eq!(
            slice_lines(text, 2, 2),
            Some((String::new(), 2, 2, 4)),
            "line 2 is blank, and blank is a line"
        );
        assert_eq!(slice_lines(text, 9000, 9100), None, "past the end is None");
        assert_eq!(slice_lines("", 1, 5), None, "an empty file has no line 1");
        assert_eq!(
            slice_lines(text, 3, usize::MAX),
            Some(("three\nfour".to_string(), 3, 4, 4)),
            "an open end stops at the last line and SAYS which one"
        );
        assert_eq!(
            slice_lines(text, 1, 2),
            Some(("one\n".to_string(), 1, 2, 4))
        );
    }
}

#[cfg(test)]
mod source_reaches_the_content_path_too {
    //! `tokenade read foo.go` returned the file whole while READING the same
    //! path through the hook stripped its comments. Measured over this tree:
    //! Go keeps 0.800 of its bytes once comments go, C 0.847, Ruby 0.836,
    //! Python 0.902, Rust 0.906.

    fn tmp(name: &str, body: &str) -> std::path::PathBuf {
        // Deliberately OUTSIDE any git work tree: inside one, the editable
        // -source exception applies and raw bytes are the right answer.
        let d = std::env::temp_dir().join(format!("tokenade-srcstrip-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        let p = d.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    fn go_with_comments() -> String {
        (0..200)
            .map(|i| format!("// what handler {i} does, at some length so it counts\nfunc handler{i}() {{ return }}\n"))
            .collect()
    }

    #[test]
    fn a_go_file_outside_a_work_tree_is_stripped() {
        let p = tmp("a.go", &go_with_comments());
        let rc = super::try_source_strip(Some(p.to_str().unwrap()), &go_with_comments(), None);
        assert_eq!(rc, Some(0), "source with 16 % of comments must be stripped");
    }

    #[test]
    fn a_file_that_is_not_source_is_declined() {
        let body = go_with_comments();
        let p = tmp("a.zzz", &body);
        assert_eq!(
            super::try_source_strip(Some(p.to_str().unwrap()), &body, None),
            None,
            "an unknown extension is not our business"
        );
    }

    #[test]
    fn a_small_source_file_is_declined() {
        let body = "// one comment\nfunc a() {}\n";
        let p = tmp("small.go", body);
        assert_eq!(
            super::try_source_strip(Some(p.to_str().unwrap()), body, None),
            None,
            "under 3 000 bytes the banner costs more than the strip saves"
        );
    }

    #[test]
    fn stdin_with_no_path_is_declined() {
        assert_eq!(
            super::try_source_strip(None, &go_with_comments(), None),
            None
        );
    }

    // `TOKENADE_NO_CODE_STRIP` is deliberately NOT covered here: asserting it
    // means mutating the process environment, and this suite runs in parallel
    // — a set/remove pair leaked the switch into whatever else was running and
    // took an unrelated dedup guard down with it. The switch is one
    // `is_truthy_one` call shared with the Read hook, which covers it.
}
