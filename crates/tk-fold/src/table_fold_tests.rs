// The tests of `table_fold`.
//
// Spliced in by `include!` from `table_fold.rs`: the same module, hence the same
// private access and the same `super::` paths, without a single visibility line
// changed. This is the form this repo already uses for 33 other modules.
//
// A `//!` is impossible here: a file spliced in by `include!` is not a
// module, and an INNER documentation comment has nothing to qualify.

#[cfg(test)]
mod restated_revision {
    /// Rows shaped exactly like a captured `flux get kustomizations`
    /// fixture, which was stored COMPLETE (11 530 of 11 530 chars) and so
    /// can be measured honestly.
    fn flux_table(healthy: usize, busy: usize) -> String {
        let mut t = String::from(
            "NAME                        REVISION                                          SUSPENDED   READY   MESSAGE\n",
        );
        // Interleaved: `fold_table` keeps only the first `head` rows, so a
        // block of busy rows at the END would fall past the cut and the
        // "a real message survives" arm would pass for the wrong reason.
        //
        // Written as ONE bounded loop over the total. The first draft advanced
        // the two counters under separate conditions and could satisfy neither
        // — with 62 healthy and 8 busy it spun forever, and three stacked test
        // runs sat on it for 56 minutes. A fixture generator must terminate on
        // its arguments, not on the arguments it was written for.
        let mut h = 0usize;
        let mut b = 0usize;
        for i in 0..(healthy + busy) {
            let busy_turn = b < busy && (i % 8 == 7 || h >= healthy);
            if busy_turn {
                let sha = format!("main@sha1:{:040x}", 900 + b);
                t.push_str(&format!(
                    "ks-b{b:<24} {sha} False       False   reconciliation in progress\n"
                ));
                b += 1;
            } else {
                let sha = format!("main@sha1:{:040x}", h * 7 + 1);
                t.push_str(&format!(
                    "ks-h{h:<24} {sha} False       True    Applied revision: {sha}\n"
                ));
                h += 1;
            }
        }
        debug_assert_eq!(h + b, healthy + busy, "every row must be emitted once");
        t
    }

    /// A message that only repeats the row's own revision is collapsed, and
    /// the substitution is stated.
    #[test]
    fn a_message_that_only_repeats_the_revision_is_collapsed() {
        let t = flux_table(62, 8);
        let out = super::fold_table(&t, 60, "flux", "flux row(s)").expect("folds");

        assert!(
            out.contains("`=REVISION` in MESSAGE on 62 row(s)"),
            "the substitution must be declared, with its count: {out:.300}"
        );
        // The revision itself is still on every row — this replaces a REPEAT,
        // not the fact.
        assert!(
            out.contains("main@sha1:0000000000000000000000000000000000000001"),
            "the REVISION column is untouched: {out:.300}"
        );
        // A message that says something else survives word for word.
        assert!(
            out.contains("reconciliation in progress"),
            "a real message is never rewritten: {out:.300}"
        );
        assert!(out.len() < t.len(), "and it saves: {} vs {}", out.len(), t.len());

        // The header line is a contract this module guards by name.
        let mut lines = out.lines();
        lines.next().unwrap(); // banner
        lines.next().unwrap(); // the substitution note
        assert_eq!(
            lines.next().unwrap(),
            "NAME                        REVISION                                          SUSPENDED   READY   MESSAGE",
            "the header stays byte for byte"
        );
    }

    /// It fires on a REPEAT and on nothing else.
    #[test]
    fn nothing_is_rewritten_unless_the_row_already_said_it() {
        // The message names a DIFFERENT revision: that is real information.
        let line = "ks-a  main@sha1:aaaa  False  True  Applied revision: main@sha1:bbbb";
        let (out, n) = super::fold_restated_revision(line);
        assert_eq!(n, 0, "a different revision is not a repeat");
        assert_eq!(out, line, "and the row comes back untouched");

        // Nothing after the lead-in is not a repeat either.
        let empty = "ks-a  main@sha1:aaaa  False  True  Applied revision: ";
        assert_eq!(super::fold_restated_revision(empty).1, 0);

        // A row with no such message at all.
        let plain = "ks-a  main@sha1:aaaa  False  False  reconciliation in progress";
        assert_eq!(super::fold_restated_revision(plain).1, 0);

        // And the positive case, so the three negatives above mean something.
        let dup = "ks-a  main@sha1:aaaa  False  True  Applied revision: main@sha1:aaaa";
        let (out, n) = super::fold_restated_revision(dup);
        assert_eq!(n, 1);
        assert!(out.ends_with("Applied revision: =REVISION"), "got {out}");
        assert!(out.contains("main@sha1:aaaa"), "the column itself stays: {out}");

        // A table with no repeats gains no note.
        let t = flux_table(0, 70);
        let out = super::fold_table(&t, 60, "flux", "flux row(s)").expect("folds");
        assert!(
            !out.contains("=REVISION"),
            "no repeats, so nothing claimed: {out:.200}"
        );
    }
}

#[cfg(test)]
mod tests {

    /// A file name is not a status, and a status is not a file name.
    ///
    /// The priority hoist reorders a table so failures arrive first. It
    /// decides on words found anywhere in the row, and in a table whose
    /// cells are PATHS those words are ordinary content: a real crate
    /// archive listed with `tar -tzvf` reported `10 problem(s) first` and
    /// promoted `quickjs/test262_errors.txt` over the archive's own tree
    /// order — the one thing a listing is read for. Sibling of the
    /// alignment-read-as-nesting fix in the same function.
    ///
    /// The two fixtures OPPOSE each other on the only field that matters:
    /// in the first the word exists ONLY inside a path, in the second ONLY
    /// inside a status cell. Either half alone would pass with the rule
    /// removed or with the hoist removed.
    #[test]
    fn a_word_inside_a_path_is_not_a_problem_but_a_status_still_is() {
        let mut listing = String::from("PERM  SIZE  PATH\n");
        for i in 0..200 {
            listing.push_str(&format!("-rw-  100  pkg/src/file{i}.rs\n"));
        }
        listing = listing.replace("pkg/src/file137.rs", "pkg/src/test262_errors.txt");
        let out = fold_table_priority(&listing, 50, "tar", "line(s)").expect("folds");
        assert!(
            !out.contains("problem(s) first"),
            "a path cell was counted as a failure:\n{out}"
        );
        let first: Vec<&str> = out.lines().take(5).collect();
        assert!(
            !first.iter().any(|l| l.contains("test262_errors")),
            "the listing was reordered by a word in a file name:\n{}",
            first.join("\n")
        );

        // And the same on Windows separators — `tar-win` is where this was
        // found, and a rule written for `/` alone leaves that half broken.
        let mut win = String::from("PERM  SIZE  PATH\n");
        for i in 0..200 {
            win.push_str(&format!("-rw-  100  pkg\\src\\file{i}.rs\n"));
        }
        win = win.replace("pkg\\src\\file137.rs", "pkg\\src\\test262_errors.txt");
        let out = fold_table_priority(&win, 50, "tar-win", "line(s)").expect("folds");
        assert!(
            !out.contains("problem(s) first"),
            "a backslash path cell was counted as a failure:\n{out}"
        );

        let mut pods = String::from("NAME  READY  STATUS  RESTARTS  AGE\n");
        for i in 0..200 {
            pods.push_str(&format!("api-{i}  1/1  Running  0  5d\n"));
        }
        pods = pods.replace(
            "api-142  1/1  Running  0  5d",
            "api-142  0/1  CrashLoopBackOff  7  5d",
        );
        let out = fold_table_priority(&pods, 50, "kubectl", "pod(s)").expect("folds");
        assert!(
            out.contains("1 problem(s) first"),
            "a real status stopped being hoisted:\n{}",
            out.lines().take(3).collect::<Vec<_>>().join("\n")
        );
        let first: Vec<&str> = out.lines().take(4).collect();
        assert!(
            first.iter().any(|l| l.contains("api-142")),
            "the failing pod was not brought to the top:\n{}",
            first.join("\n")
        );
    }

    /// A promoted problem must arrive with the lines that say what it WAS.
    #[test]
    fn a_problem_row_keeps_its_indented_detail() {
        let mut input = String::from("HEADER\n");
        for i in 0..300 {
            input.push_str(&format!("ok  \tgithub.com/acme/pkg{i}\t0.01s\n"));
        }
        input.push_str("--- FAIL: TestZqx (0.00s)\n");
        input.push_str("    engine_test.go:118: expected 42, got 41\n");
        let out = super::fold_table_priority(&input, 60, "go-test", "row(s)").unwrap();
        assert!(out.contains("--- FAIL: TestZqx"), "the header is promoted:\n{out}");
        assert!(
            out.contains("engine_test.go:118: expected 42, got 41"),
            "and so is the reason — promoting a failure without its detail tells \
             the agent THAT something failed and never why:\n{out}"
        );
    }

    /// Where indentation is column ALIGNMENT rather than nesting, the grouping
    /// must not fire: one section would otherwise absorb every line under it
    /// and hand them all its verdict.
    #[test]
    fn alignment_indentation_is_not_nesting() {
        // A disassembly's shape: an unindented section header, then hundreds of
        // indented instruction lines, one of which calls `error@plt`.
        let mut input = String::from("/usr/bin/ls:     file format elf64-x86-64\n");
        input.push_str("0000000000003000 <_init>:\n");
        for i in 0..200 {
            input.push_str(&format!("    {:4x}:\tmov    %rsp,%rbp\n", 0x3000 + i * 4));
        }
        input.push_str("    3320:\tcall   1180 <error@plt>\n");
        let out = super::fold_table_priority(&input, 80, "objdump", "asm line(s)").unwrap();
        // ONE row carries the word, so at most one row can be a problem. The
        // grouping used to make it 201, the whole section. (Since 2026-10-04
        // it is zero: `error@plt` is a symbol NAME, and a name is not a
        // verdict.)
        let banner = out.lines().next().unwrap_or("");
        let announced: usize = banner
            .split(' ')
            .zip(banner.split(' ').skip(1))
            .find(|(_, w)| w.starts_with("problem"))
            .and_then(|(n, _)| n.parse().ok())
            .unwrap_or(0);
        assert!(announced <= 1, "the section must not inherit one line's verdict:\n{banner}");
        // And the fold still opens where the output does, not 200 lines in.
        assert!(
            out.contains("0000000000003000 <_init>:"),
            "the section header is the first thing a reader needs:\n{out}"
        );
        // The cap is the point of folding, and a single swallowing record
        // walked straight past it: the emit loop breaks only once something
        // has been emitted, so a first record of 202 lines is emitted whole.
        // That is the fold returning MORE than `head` on a 202-row input.
        let emitted = out.lines().count();
        assert!(
            emitted <= 84,
            "the fold must respect head=80, emitted {emitted} lines"
        );
    }

    /// The threshold has to sit where a REAL nesting shape still clears it.
    /// `go test -v` on a bad day is two indented lines under every failure, and
    /// that is 40% of the rows: tightening the cut to a quarter would read a
    /// genuinely nested output as column alignment and tear every explanation
    /// away from the failure it explains.
    #[test]
    fn a_verbose_test_run_is_still_read_as_nested() {
        let mut input = String::from("HEADER\n");
        for i in 0..60 {
            input.push_str(&format!("ok  \tgithub.com/acme/pkg{i}\t0.01s\n"));
        }
        for i in 0..20 {
            input.push_str(&format!("--- FAIL: TestZqx{i} (0.00s)\n"));
            input.push_str(&format!("    engine_test.go:{i}: expected 42, got 41\n"));
            input.push_str(&format!("    engine_test.go:{i}: giving up here\n"));
        }
        let out = super::fold_table_priority(&input, 60, "go-test", "row(s)").unwrap();
        assert!(out.contains("20 problem(s) first"), "20 failures:\n{out}");
        // Each promoted failure arrives with its own two lines, adjacent.
        let ls: Vec<&str> = out.lines().collect();
        let i = ls
            .iter()
            .position(|l| l.contains("--- FAIL: TestZqx0 "))
            .expect("the first failure is promoted");
        assert!(
            ls[i + 1].contains("expected 42, got 41") && ls[i + 2].contains("giving up here"),
            "the explanation must stay under the failure it explains:\n{out}"
        );
    }

    /// The banner counts problems, and a problem's own explanation is not a
    /// second problem.
    #[test]
    fn the_banner_counts_records_not_their_lines() {
        let mut input = String::from("HEADER\n");
        for i in 0..100 {
            input.push_str(&format!("ok  \tgithub.com/acme/pkg{i}\t0.01s\n"));
        }
        input.push_str("--- FAIL: TestZqx (0.00s)\n");
        input.push_str("    engine_test.go:118: expected 42, got 41\n");
        input.push_str("    engine_test.go:119: and another line about it\n");
        let out = super::fold_table_priority(&input, 60, "go-test", "row(s)").unwrap();
        assert!(
            out.contains("1 problem(s) first"),
            "one failing test with two lines of detail is ONE problem:\n{}",
            out.lines().next().unwrap_or("")
        );
    }

    /// The tables this helper was written for have no indented rows, so their
    /// output must be byte-identical to before the record grouping.
    #[test]
    fn a_flat_table_is_unaffected_by_record_grouping() {
        let mut input = String::from("CONTAINER ID   IMAGE   STATUS\n");
        for i in 0..100 {
            input.push_str(&format!("abc{i:03}   nginx:1.2   Up 3 hours\n"));
        }
        let out = super::fold_table_priority(&input, 20, "docker-ps", "row(s)").unwrap();
        assert!(out.contains("100 row(s)"));
        assert!(out.contains("abc000   nginx:1.2   Up 3 hours"));
        assert!(out.contains("... (+80 more)"), "cap and count unchanged:\n{out}");
    }
    use super::*;

    const PS_HEADER: &str =
        "CONTAINER ID   IMAGE          COMMAND   CREATED       STATUS        PORTS     NAMES";

    fn make_ps_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("{:012x}   nginx:latest   /bin/sh   5 min ago   Up 5 min   80/tcp   web{i}\n", i))
            .collect();
        format!("{PS_HEADER}\n{rows}")
    }

    #[test]
    fn returns_none_when_empty() {
        assert!(fold_table("", 10, "test", "row(s)").is_none());
    }

    #[test]
    fn returns_none_when_header_only() {
        assert!(fold_table(PS_HEADER, 10, "test", "row(s)").is_none());
    }

    #[test]
    fn returns_none_when_short() {
        let t = make_ps_table(5);
        assert!(fold_table(&t, 20, "docker-ps", "container(s)").is_none());
    }

    #[test]
    fn folds_long_table() {
        let t = make_ps_table(40);
        let out = fold_table(&t, 20, "docker-ps", "container(s)").unwrap();
        assert!(out.contains("[tokenade:docker-ps] 40 container(s)"));
        assert!(out.contains("CONTAINER ID"));
        assert!(out.contains("(+20 more)"));
        // row 0 kept, row 20 dropped
        assert!(out.contains("000000000000"));
        assert!(out.contains("000000000013"));
    }

    #[test]
    fn header_always_preserved() {
        let t = make_ps_table(30);
        let out = fold_table(&t, 20, "docker-ps", "container(s)").unwrap();
        assert!(out.starts_with("[tokenade:docker-ps]"));
        let mut lines = out.lines();
        lines.next(); // banner
        assert_eq!(lines.next().unwrap(), PS_HEADER);
    }

    // --- fold_table_priority tests ---

    const K8S_HEADER: &str =
        "NAME       READY   STATUS             RESTARTS   AGE";

    fn make_k8s_pods(n_ok: usize, n_bad: usize) -> String {
        let ok: String = (0..n_ok)
            .map(|i| format!("pod-ok-{i}   1/1   Running   0   5m\n"))
            .collect();
        let bad: String = (0..n_bad)
            .map(|i| format!("pod-bad-{i}   0/1   CrashLoopBackOff   5   5m\n"))
            .collect();
        format!("{K8S_HEADER}\n{ok}{bad}")
    }

    #[test]
    fn priority_returns_none_when_short() {
        let t = make_k8s_pods(5, 1);
        assert!(fold_table_priority(&t, 20, "k8s-pods", "pod(s)").is_none());
    }

    #[test]
    fn priority_sorts_problems_first() {
        // 25 ok pods + 3 crashing: table has 28 rows, head=20
        // Without priority: CrashLoopBackOff pods at bottom, may be cut off.
        // With priority: they are first, always visible.
        let t = make_k8s_pods(25, 3);
        let out = fold_table_priority(&t, 20, "kubectl-pods", "pod(s)").unwrap();
        assert!(out.contains("3 problem(s) first"));
        // All 3 crash pods appear before running pods in the output
        let crash_pos = out.find("CrashLoopBackOff").unwrap();
        let run_pos = out.find("Running").unwrap();
        assert!(crash_pos < run_pos, "crash rows should appear before running rows");
    }

    #[test]
    fn priority_banner_has_problem_count() {
        let t = make_k8s_pods(20, 5);
        let out = fold_table_priority(&t, 15, "kubectl-pods", "pod(s)").unwrap();
        assert!(out.contains("25 pod(s), 5 problem(s) first"));
    }

    #[test]
    fn priority_no_problems_omits_annotation() {
        let ok: String = (0..30)
            .map(|i| format!("pod-{i}   1/1   Running   0   5m\n"))
            .collect();
        let t = format!("{K8S_HEADER}\n{ok}");
        let out = fold_table_priority(&t, 20, "kubectl-pods", "pod(s)").unwrap();
        assert!(!out.contains("problem(s)"));
        assert!(out.contains("30 pod(s)"));
    }
}

#[cfg(test)]
mod a_record_is_never_cut_in_half {
    /// The property that made the drift costly: a problem row is
    /// lifted, and it arrives WHOLE, with its indented continuations.
    ///
    /// This test also compared the three per-platform modules with each other.
    /// That half was removed because there are no longer three modules: the
    /// copies in `cmd/macos/` and `cmd/windows/` were only delegating,
    /// and they have been deleted. Equality between platforms is no longer a
    /// property to maintain, it is the shape of the code — and a test comparing a
    /// function with itself would pass in both worlds.
    #[test]
    fn a_record_with_continuation_lines_is_never_cut_in_half() {
        let text = "NAME      READY   STATUS\n                    pod-a     1/1     Running\n                    pod-b     0/1     CrashLoopBackOff\n                    \x20 reason: unmet dependency\n                    \x20 detail: invalid config\n                    pod-c     1/1     Running\n                    pod-d     1/1     Running\n                    pod-e     1/1     Running\n";
        let out = super::fold_table_priority(text, 3, "kubectl", "pod(s)")
            .expect("should fold");
        assert!(out.contains("CrashLoopBackOff"), "lost the problem row: {out}");
        assert!(
            out.contains("reason: unmet dependency") && out.contains("detail: invalid config"),
            "a record was cut at the head boundary: {out}"
        );
    }

    /// `errors=remount-ro` is a mount OPTION; `level=error` is a verdict.
    #[test]
    fn the_key_of_an_option_is_not_a_problem_and_its_value_can_be() {
        let mut t = String::from("TARGET SOURCE FSTYPE OPTIONS\n");
        for i in 0..30 {
            t.push_str(&format!("├─/mnt/d{i} /dev/sd{i} ext4 rw,relatime,errors=remount-ro\n"));
        }
        let out = crate::table_fold::fold_table_priority(&t, 10, "findmnt", "mount(s)").unwrap();
        assert!(!out.contains("problem(s) first"), "{out}");
        t.push_str("app level=error msg=boom\n");
        let out = crate::table_fold::fold_table_priority(&t, 10, "x", "row(s)").unwrap();
        assert!(out.contains("1 problem(s) first"), "{out}");
    }

    /// A problem word inside a NAME is part of the name; a verdict stands
    /// alone or as a `key=value`'s value.
    #[test]
    fn a_name_holding_a_problem_word_is_not_a_problem() {
        let mut input = String::from("NAME                         READY   STATUS\n");
        for i in 0..90 {
            input.push_str(&format!("error-ex-{i}@1.3.4            1/1     Running\n"));
        }
        input.push_str("es-errors@1.3.0 deduped      1/1     Running\n");
        input.push_str("test_errors_are_reported     1/1     Running\n");
        input.push_str("api-7f9c                     0/1     CrashLoopBackOff\n");
        input.push_str("worker-2                     0/1     Error\n");
        input.push_str("ingest-1   1/1   Running   log_level=error\n");
        let out = super::fold_table_priority(&input, 40, "kubectl", "pod(s)").unwrap();
        assert!(out.lines().next().unwrap_or("").contains("3 problem(s) first"), "{out}");
        let body: Vec<&str> = out.lines().skip(2).take(3).collect();
        assert!(body.iter().any(|l| l.contains("CrashLoopBackOff")) && body.iter().any(|l| l.contains("log_level=error")), "{out}");
    }
}
