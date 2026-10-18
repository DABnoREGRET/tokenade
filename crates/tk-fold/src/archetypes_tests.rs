// The tests of `crate::archetypes`, moved out of the production file.
//
// Mounted by `include!`, not by `mod`: the tokens are spliced INTO
// `archetypes`, so every `super::` keeps the meaning it had. Split by
// `srcsweep::production_lines`.

#[cfg(test)]
mod noise_prefix_matching {
    /// A noise prefix that begins with whitespace must still be able to match.
    ///
    /// `strip_noise` trims each line before comparing, so a prefix starting
    /// with a space or a tab could never match anything. Measured 2026-08-28
    /// across the callers: **twelve such prefixes in seven files**, all inert —
    /// `java_runtime`'s four JVM-noise rules, `python_runtime`'s two
    /// stdlib-frame rules, `ruby_runtime`'s two, `zip`'s `  adding: ` (the bulk
    /// of `zip -r` output), and the bare indent guards in hashcat, npm_audit
    /// and openssl_cli. Seven compactors whose main rule did nothing.
    #[test]
    fn an_indented_prefix_matches_the_raw_line() {
        let trace = "Exception: BOOM\n    at com.acme.App.run(App.java:9)\n    at java.base/java.util.X.y(X.java:1)\n    at java.base/java.util.X.z(X.java:2)\n";
        let out = super::strip_noise(trace, &["    at java.base/"], "stdlib frames");
        assert!(out.contains("BOOM"), "the message must survive:\n{out}");
        assert!(
            out.contains("com.acme.App.run"),
            "app frames must survive:\n{out}"
        );
        assert!(
            !out.contains("java.base"),
            "the noise rule must fire:\n{out}"
        );
        assert!(out.contains("[elided: 2 stdlib frames line(s)]"), "{out}");
    }

    /// …and trimming stays the default for a prefix that does not ask for it:
    /// an indented `bzip2:` is the same noise line as an unindented one.
    #[test]
    fn an_unindented_prefix_still_ignores_leading_space() {
        let text = "keep me\n   bzip2: warning: trailing garbage\nbzip2: warning: again\n";
        let out = super::strip_noise(text, &["bzip2: warning:"], "warnings");
        assert!(out.contains("keep me"));
        assert!(!out.contains("bzip2"), "both spellings must go:\n{out}");
        assert!(out.contains("[elided: 2 warnings line(s)]"), "{out}");
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fence_adjust_extends_head_past_open_fence() {
        let lines = vec![
            "pre", "```rust", "let x=1;", "let y=2;", "```", "p1", "p2", "p3", "p4", "p5",
        ];
        // head=2 lands inside the open fence (1 marker in [..2]).
        let (h, _t) = fence_adjust(&lines, 2, 2);
        assert!(h >= 5, "head must extend past the closing fence, got {h}");
        assert_eq!(lines[..h].iter().filter(|l| is_fence(l)).count() % 2, 0);
    }

    #[test]
    fn fence_adjust_pulls_tail_before_open_fence() {
        // Closing ``` is in the tail window but the open ``` isn't → extend tail.
        let lines = vec![
            "h1", "h2", "h3", "h4", "h5", "```", "code", "more", "```", "end",
        ];
        let (_h, t) = fence_adjust(&lines, 2, 2);
        assert_eq!(
            lines[lines.len() - t..]
                .iter()
                .filter(|l| is_fence(l))
                .count()
                % 2,
            0
        );
    }

    #[test]
    fn fence_adjust_noop_outside_fence() {
        let lines = vec!["a", "b", "c", "d", "e", "f", "g", "h"];
        assert_eq!(fence_adjust(&lines, 2, 2), (2, 2));
    }

    #[test]
    fn line_cap_folds() {
        let mut s = String::new();
        for i in 0..50 {
            let _ = writeln!(s, "line {i}");
        }
        let out = line_cap(&s, 10, "row(s)");
        assert!(out.contains("40 more row(s) elided"));
    }

    #[test]
    fn line_cap_passthrough_short() {
        let s = "a\nb\nc\n";
        assert_eq!(line_cap(s, 10, "row(s)"), "a\nb\nc");
    }

    #[test]
    fn strip_noise_drops_lines() {
        let s = "keep this\nDEBUG: noise\nkeep that\nDEBUG: more\n";
        let out = strip_noise(s, &["DEBUG:"], "debug");
        assert!(out.contains("keep this"));
        assert!(!out.contains("DEBUG:"));
        assert!(out.contains("elided: 2 debug"));
    }

    #[test]
    fn collapse_padding_works() {
        let s = "col1          col2          col3\n";
        let out = collapse_padding(s);
        assert_eq!(out, "col1  col2  col3");
    }

    #[test]
    fn head_tail_keeps_both_ends() {
        let mut s = String::new();
        for i in 0..50 {
            let _ = writeln!(s, "line {i}");
        }
        let out = head_tail(&s, 5, 3, "make", "line(s)").unwrap();
        // head preserved
        assert!(out.contains("line 0"));
        assert!(out.contains("line 4"));
        // TAIL preserved — this is the outcome line a head-slice would drop
        assert!(out.contains("line 47"));
        assert!(out.contains("line 49"));
        // middle folded
        assert!(out.contains("(+42 more)"));
        assert!(!out.contains("line 25"));
    }

    #[test]
    fn head_tail_passthrough_when_short() {
        let s = "a\nb\nc\nd\n";
        assert!(head_tail(s, 5, 3, "x", "line(s)").is_none());
    }

    #[test]
    fn kv_records_folds_paragraphs() {
        let mut s = String::new();
        for i in 0..10 {
            let _ = write!(
                s,
                "Package: pkg{i}\nVersion: 1.{i}.0\nStatus: installed\n\n"
            );
        }
        let out = kv_records(&s, 3, "apt-show", "package(s)").unwrap();
        assert!(out.contains("[tokenade:apt-show] 10 package(s)"));
        assert!(out.contains("Package: pkg0"));
        assert!(out.contains("Package: pkg2"));
        // record 3 folded — never cut mid-record
        assert!(!out.contains("Package: pkg5"));
        // …and the elision NAMES what it folded: seven package names is not
        // the same fact as the number seven.
        assert!(out.contains("(+7 more: pkg3, pkg4"), "{out}");
        assert!(
            out.contains("pkg9"),
            "the last folded record is unnamed: {out}"
        );
    }

    /// A record that does not identify itself keeps the bare count rather
    /// than a line of garbage: the value is taken only from a first line
    /// whose KEY reads like a key, so a timestamp or a URL never splits here
    /// by accident.
    #[test]
    fn records_without_a_name_keep_the_bare_count() {
        let mut s = String::new();
        for i in 0..10 {
            let _ = write!(
                s,
                "[2026-08-27T10:00:0{i}Z] worker started
rss 12{i}M

"
            );
        }
        let out = kv_records(&s, 3, "log-blocks", "record(s)").unwrap();
        assert!(
            out.contains("(+7 more)"),
            "named something it cannot name: {out}"
        );
    }

    /// A numeric first field is a position, not a name.
    ///
    /// The key whitelist alone lets `12: something` through — digits are
    /// alphanumeric — so the first character must be a LETTER. Without this
    /// an indexed listing would be "named" by whatever followed its numbers.
    #[test]
    fn a_numbered_first_line_is_not_a_name() {
        let mut s = String::new();
        for i in 0..10 {
            let _ = write!(s, "{i}: entry\nvalue {i}\n\n");
        }
        let out = kv_records(&s, 3, "num-blocks", "record(s)").unwrap();
        assert!(
            out.contains("(+7 more)"),
            "a position was read as a name: {out}"
        );
    }

    /// More names than the line can carry: the overflow is COUNTED.
    #[test]
    fn the_names_that_do_not_fit_are_counted() {
        let mut s = String::new();
        for i in 0..40 {
            let _ = write!(s, "Package: pkg{i}\nVersion: 1.{i}.0\n\n");
        }
        let out = kv_records(&s, 3, "apt-show", "package(s)").unwrap();
        let elision = out.lines().find(|l| l.contains("more")).unwrap_or_default();
        assert!(
            elision.contains("other(s)"),
            "the unnamed remainder is not counted: {elision}"
        );
    }

    /// The names come from the ELIDED records, never from the ones already
    /// printed above and never from a rescued failure block.
    #[test]
    fn a_rescued_record_is_not_also_named_as_elided() {
        let mut s = String::new();
        for i in 0..10 {
            let body = if i == 8 {
                "Status: error: disk full"
            } else {
                "Status: installed"
            };
            let _ = write!(s, "Package: pkg{i}\nVersion: 1.{i}.0\n{body}\n\n");
        }
        let out = kv_records(&s, 3, "apt-show", "package(s)").unwrap();
        assert!(
            out.contains("look like failures"),
            "no rescue happened: {out}"
        );
        let elision = out.lines().find(|l| l.contains("more")).unwrap_or_default();
        assert!(
            !elision.contains("pkg8"),
            "the rescued record is named as elided too: {elision}"
        );
        assert!(elision.contains("pkg3"), "nothing named at all: {elision}");
    }

    /// A long test run folds on its VERDICTS, not on the words in the names.
    ///
    /// Both halves measured 2026-09-04 on a real `cargo nextest run`:
    /// with 8 884 passes and no failures the fold announced ten lines "kept
    /// below because they look like failures", and a `FAIL [ …]` line matched
    /// no keyword at all, so the one line that mattered was eligible to be
    /// elided while ten passing ones were rescued in its place.
    #[test]
    fn a_test_run_is_folded_on_its_verdicts_not_on_the_test_names() {
        let names = [
            "a_pipeline_that_hid_a_failure_says_so",
            "read_prompt_without_a_value_is_a_clean_error",
            "a_killed_command_says_so",
            "early_pipe_close_does_not_panic",
        ];
        let line = |i: usize, verdict: &str| {
            format!(
                "        {verdict} [   0.0{}s] ({i}/200) tokenade-cli::e2e mod{}::{}_{i}",
                i % 9,
                i % 4,
                names[i % names.len()]
            )
        };
        let green: String = (1..=200).map(|i| line(i, "PASS") + "\n").collect();
        let out = head_tail(&green, 80, 5, "cargo-nextest", "line(s)").unwrap();
        assert!(
            !out.contains("look like failures"),
            "a run where nothing failed reports failures:\n{out}"
        );

        let mut lines: Vec<String> = (1..=200).map(|i| line(i, "PASS")).collect();
        lines[119] = line(120, "FAIL");
        let red = lines.join("\n") + "\n";
        let out = head_tail(&red, 80, 5, "cargo-nextest", "line(s)").unwrap();
        assert!(
            out.contains("look like failures"),
            "the one failure was not rescued:\n{out}"
        );
        assert!(
            out.lines().any(|l| l.starts_with("        FAIL [")),
            "the failing line itself is gone:\n{out}"
        );
    }

    #[test]
    fn looks_like_table_detects_columns() {
        let t = "NAME      STATUS    AGE\npod-a     Running   5m\npod-b     Running   3m\n";
        assert!(looks_like_table(t));
        // a single error line is NOT a table
        assert!(!looks_like_table("Error: connection refused\n"));
    }

    #[test]
    fn looks_like_kv_detects_records() {
        let t = "Name: foo\nVersion: 1.2.3\nArch: amd64\n";
        assert!(looks_like_kv(t));
        assert!(!looks_like_kv("just\nsome\nplain words here\n"));
    }
}
#[cfg(test)]
mod padding_floor_tests {
    /// **A share without a bytes floor refuses a real gain.**
    ///
    /// Fixture: the real `ps aux` capture from the corpus. Fifty lines in
    /// columns, no interior hole, and **1,218 bytes of padding out of
    /// 7,868** — 15.5 %, hence refused by the one-fifth threshold by a point
    /// and a half.
    ///
    /// The test FIRST checks that the fixture really reaches the rule under test:
    /// its share is BELOW the threshold, so only the absolute floor can accept it.
    #[test]
    fn a_large_table_below_the_share_is_still_decorative() {
        let body = std::fs::read_to_string(
            crate::source_guard::fixtures_dir().join("macos/ps_aux_full.txt"),
        )
        .expect("the corpus `ps aux` capture");
        let saved = body.len() - super::collapse_alignment_padding(&body).len();
        assert!(
            saved * 5 < body.len(),
            "the fixture reaches the SHARE threshold, so it does not test the floor"
        );
        assert!(
            saved >= 1_000,
            "the fixture is below the floor: {saved} bytes"
        );
        assert!(
            super::padding_is_decorative(&body),
            "the padding was refused"
        );
    }

    /// And the floor holds the other side: a little decoration stays a little decoration.
    #[test]
    fn a_small_amount_of_padding_is_not_a_table() {
        // Six lines in columns, about a hundred bytes of decoration: below the
        // share AND below the floor.
        let mut doc = String::new();
        for i in 0..6 {
            doc.push_str(&format!("colonne{i}  x{i}\n"));
            doc.push_str(&"remplissage sans colonnes, du texte ordinaire qui pèse.\n".repeat(4));
        }
        let saved = doc.len() - super::collapse_alignment_padding(&doc).len();
        assert!(
            saved < 1_000 && saved * 5 < doc.len(),
            "the fixture is not below both bounds: {saved}"
        );
        assert!(
            !super::padding_is_decorative(&doc),
            "a little decoration was enough"
        );
    }

    /// **A NESTED document is not a table**, and a table with a right-aligned
    /// first column is not a nested document.
    ///
    /// Both carry several indent widths; what separates them is
    /// where the decoration WEIGHS. Measured on both: the plist gives 343 bytes of
    /// indent versus 48 inside, the `ps -eo` table 7 versus 91.
    #[test]
    fn a_nested_document_keeps_its_depth() {
        let plist = std::fs::read_to_string(
            crate::source_guard::fixtures_dir().join("macos/defaults_dock.txt"),
        )
        .expect("the corpus `defaults` capture");
        // The fixture REACHES the rule: without the refusal, it would pass — it
        // clears the absolute floor and has no interior hole.
        let saved = plist.len() - super::collapse_alignment_padding(&plist).len();
        assert!(
            saved >= 300,
            "the fixture does not clear the floor: {saved}"
        );
        assert!(
            !super::padding_is_decorative(&plist),
            "a document with six levels of indent went out flat"
        );

        // And the right-aligned table keeps folding.
        let table = "  PID  PPID USER             COMM\n\
                     \x20   1     0 root             /sbin/launchd\n\
                     \x20  10     1 root             /usr/sbin/syslogd\n\
                     \x20 100     1 daemon           /usr/sbin/cupsd\n\
                     \x201000     1 alice            /usr/bin/foo\n\
                     10000     1 bob              /usr/bin/bar\n";
        assert!(
            !super::indentation_is_a_nesting_ladder(table),
            "a right-aligned first column was taken for nesting"
        );
    }

    /// **What is measured is what the fold removes**, indentation included.
    ///
    /// The two counts were written separately and did not say the same
    /// thing: the measurement ignored LEADING indentation, which the rewrite
    /// nevertheless reduces to one space.
    #[test]
    fn the_measure_is_what_the_fold_removes() {
        let mut doc = String::new();
        for i in 0..40 {
            doc.push_str(&format!("        clé{i}  valeur{i}\n"));
        }
        let collapsed = super::collapse_alignment_padding(&doc);
        assert!(
            doc.len() - collapsed.len() > 40 * 6,
            "leading indentation is not counted in what the fold removes"
        );
        assert!(super::padding_is_decorative(&doc));
    }
}
#[cfg(test)]
mod constant_column_tests {
    use super::fold_constant_columns as fold;

    /// **A column identical everywhere is said once, and the note NAMES it.**
    ///
    /// Fixture: the real `top -l1` capture from the corpus, the heaviest family
    /// in the ranking. The test first checks that the case occurs,
    /// then that nothing disappears — every removed column is named with its
    /// value, so every row can be read back.
    #[test]
    fn a_column_that_never_changes_is_stated_once() {
        let body =
            std::fs::read_to_string(crate::source_guard::fixtures_dir().join("macos/top_l1.txt"))
                .expect("the corpus `top` capture");
        // The fold only gets a grip after de-alignment: it is
        // `collapse_padding_best` that chains the two, and measuring it
        // any other way would measure the neighbouring bound.
        let out = super::collapse_padding_best(&body).expect("the table qualifies");
        assert!(
            out.contains("[same on every row: "),
            "the fold did not fire:\n{}",
            &out[..out.len().min(400)]
        );
        let note = out
            .lines()
            .find(|l| l.starts_with("[same on every row: "))
            .expect("the note");
        for named in ["%CPU_ME 0.00000", "VSIZE N/A", "INSTRS 0"] {
            assert!(note.contains(named), "{named} is not named in {note}");
        }
        assert!(out.len() < body.len());
    }

    /// **The cost does not grow faster than the table.**
    ///
    /// `constant.contains(i)` for every cell of every row is
    /// quadratic in columns, and a wide table carries as many
    /// constant columns as columns. MEASURED 2026-09-09 by `hostile_scale` on
    /// the delivery path: **66.7 s for 25,000 columns and 252.9 s for
    /// 50,000** — ×3.8 when the input doubles — versus 4.6 s and 9.2 s with a
    /// mask, i.e. ×2.0.
    ///
    /// The bound is wide on purpose: this test says "not quadratic", not
    /// "fast".
    #[test]
    fn a_wide_table_costs_what_it_weighs() {
        // Eight thousand columns: with the mask it is a few tens of
        // milliseconds; with a per-cell lookup, billions of
        // comparisons. Three thousand were NOT enough — the mutation
        // survived, the fixture did not reach the bound it tested.
        let cols = 8_000;
        let header = (0..cols)
            .map(|i| format!("col{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let row = vec!["v"; cols].join(" ");
        let mut doc = header;
        doc.push('\n');
        for _ in 0..40 {
            doc.push_str(&row);
            doc.push('\n');
        }
        let start = std::time::Instant::now();
        let out = fold(&doc).expect("a table where everything is constant folds");
        let elapsed = start.elapsed();
        // The fixture REACHES the rule: almost every column is
        // removed, so it really is the rewrite path being measured.
        assert!(
            out.contains("[same on every row: col1 v"),
            "the fold did not fire"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "column removal is quadratic: {elapsed:?} for {cols} columns"
        );
    }

    /// **And a REAL header is required**, on both sides.    /// **And a REAL header is required**, on both sides.
    ///
    /// Without this check the note came out on a `mount` listing saying
    /// `on on · type type`: the name and the value were the same cell of a
    /// data row. Two shapes of fake header, one per check.
    #[test]
    fn a_data_row_is_not_a_header() {
        // (a) The first cell is a value, not a column name.
        //
        // The other cells of this fake header DIFFER from the constant
        // values, otherwise check (b) would refuse first and
        // this one would never be reached — measured, the mutation survived.
        let mut numeric = String::from("0 alpha beta gamma delta epsilon\n");
        for _ in 0..5 {
            numeric.push_str("1 X Y Z W V\n");
        }
        assert!(fold(&numeric).is_none(), "a data row served as a header");

        // (b) A column's name would be its own constant value.
        let mut echoed = String::from("name on type squashfs a b\n");
        for i in 0..5 {
            echoed.push_str(&format!("p{i} on type squashfs X Y\n"));
        }
        assert!(fold(&echoed).is_none(), "`on on` passed for a column name");

        // And the ACCEPTED case, otherwise the two refusals prove nothing: same
        // shape, a header that really names.
        let mut real = String::from("name state kind fs a b\n");
        for i in 0..5 {
            real.push_str(&format!("p{i} on type squashfs X{i} Y\n"));
        }
        let out = fold(&real).expect("a header that names is accepted");
        assert!(out.contains("[same on every row: state on"), "{out}");
    }
}
#[cfg(test)]
mod the_end_of_a_line_is_never_positional {
    use super::*;

    /// The fixture must REACH the end trim, hence get past what refuses
    /// the TWO other candidates. An empty interior cell makes
    /// `no_interior_cell_is_empty` say no — the real case of `Get-HotFix`,
    /// one row of which has no `InstalledBy` — and until now that refusal took
    /// the line end down with it.
    fn table_with_an_empty_cell() -> String {
        // The geometry of `Get-HotFix`, to the character: the fourth
        // column is EMPTY on one row, and every row trails spaces.
        let head = "Source        Description      HotFixID      InstalledBy          InstalledOn               ";
        let full = "DESKTOP-T2... Update           KB506613{}     AUTORITE NT\\Systeme  23/10/2025 00:00:00       ";
        let hole = "DESKTOP-T2... Update           KB5015684                          05/05/2023 00:00:00       ";
        let mut t = String::from(head);
        t.push('\n');
        for i in 0..7 {
            t.push_str(&full.replace("{}", &i.to_string()));
            t.push('\n');
        }
        t.push_str(hole);
        t.push('\n');
        t
    }

    #[test]
    fn a_refused_table_still_loses_its_line_ends() {
        let t = table_with_an_empty_cell();
        assert!(
            !no_interior_cell_is_empty(&t),
            "the fixture does not reach the rule: the interior fold would accept it"
        );
        let out = collapse_padding_best(&t).expect("the line end should have been trimmed");
        assert!(out.len() < t.len());
        for l in out.lines() {
            assert_eq!(l, l.trim_end(), "a line end survives: {l:?}");
        }
        // And nothing of the CONTENT moves: the interior decoration is intact.
        assert!(out.contains("Update           KB5066130"), "{out}");
    }

    /// Markdown's hard line break is EXACTLY two spaces, and it is
    /// the only meaning a line end carries. It survives.
    #[test]
    fn exactly_two_trailing_spaces_survive() {
        let md = "une ligne  \nune autre  \nune troisième  \n";
        assert_eq!(trim_line_ends(md), None);
    }

    /// The bound on both sides: three spaces go, two stay.
    #[test]
    fn the_bound_is_tested_on_both_sides() {
        assert_eq!(trim_line_ends("a   \nb").as_deref(), Some("a\nb"));
        assert_eq!(trim_line_ends("a  \nb"), None);
    }

    /// Nothing to trim returns nothing: a candidate returning its input
    /// unchanged would pass for a fold.
    #[test]
    fn a_text_without_line_ends_is_left_alone() {
        assert_eq!(trim_line_ends("alpha\nbravo\ncharlie"), None);
    }
}
#[cfg(test)]
mod indented_header_tests {
    use super::*;

    /// **An INDENTED header does not fabricate an empty column.**
    ///
    /// `ps`, `ioreg` and `dir` indent their header as soon as the first
    /// column is right-aligned. The "no empty cell in the middle" rule
    /// read columns from position 0: the phantom column born of
    /// that indentation was empty on every row while the next one
    /// was full, hence a hole in the middle, hence the fold refused on the WHOLE
    /// table. MEASURED 2026-09-09: `ps -eo` came back whole, 2,718 bytes of which
    /// 29 % were spaces.
    ///
    /// Fixtures built line by line: a `\` continuation in Rust does
    /// NOT eat the source file's indentation, and a first version
    /// carried twenty-five leading spaces on every data row — a
    /// DIFFERENT table, which the rule rightly refused.
    #[test]
    fn a_right_aligned_first_column_is_not_an_empty_cell() {
        let table = [
            "  PID  PPID USER             COMM",
            "    1     0 root             /sbin/launchd",
            "   70     1 root             /usr/libexec/logd",
            "   71     1 root             /usr/libexec/UserEventAgent",
            "  100     1 _windowserver    /System/Library/WindowServer",
            "  210    70 root             /usr/sbin/notifyd",
        ]
        .join("\n")
            + "\n";
        let table = table.as_str();
        assert!(
            padding_is_decorative(table),
            "an indented header was read as an empty column:\n{table}"
        );
        let folded = collapse_alignment_padding(table);
        assert!(
            folded.len() < table.len(),
            "{} → {}",
            table.len(),
            folded.len()
        );
        for v in [
            "PID",
            "PPID",
            "_windowserver",
            "/usr/sbin/notifyd",
            "210",
            "70",
        ] {
            assert!(folded.contains(v), "`{v}` disappeared:\n{folded}");
        }
    }

    /// And a REAL hole in the middle still refuses: the `Get-PSDrive` case,
    /// where `Alias Alias` would no longer say which column the second belongs to.
    #[test]
    fn a_real_interior_hole_still_refuses() {
        let table = [
            "Name           Used (GB)     Free (GB) Provider",
            "Alias                                  Alias",
            "Env                                    Environment",
            "Function                               Function",
            "C                 43.55        421.31 FileSystem",
            "Temp              43.55        421.31 FileSystem",
        ]
        .join("\n")
            + "\n";
        assert!(
            !padding_is_decorative(table.as_str()),
            "an empty cell IN THE MIDDLE was folded:\n{table}"
        );
    }

    /// **The WHOLE document competes with the per-segment reading.**
    ///
    /// The `netstat` compactor's output — 4,299 bytes — qualified no
    /// segment taken alone, while the whole document qualified. It
    /// went out as is, and the NEXT pass recognised it as
    /// `AlignedBlock` and returned it at 2,021: the gain arrived one round too
    /// late, and the fixed point was lost in between.
    #[test]
    fn a_table_cut_by_blank_lines_is_still_a_table() {
        let seg = |n: usize| {
            (0..n)
                .map(|i| format!("tcp4       0      0  10.0.2.15.5361{i}        17.32.214.169.443      ESTABLISHED"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let doc = format!(
            "Proto Recv-Q Send-Q  Local Address          Foreign Address        (state)\n{}\n\n{}\n",
            seg(3),
            seg(3)
        );
        // No segment has five column lines; the document does.
        assert!(collapse_decorative_segments(&doc).is_none());
        assert!(padding_is_decorative(&doc), "the whole document qualifies");
        // So the BETTER of the two readings folds, where the per-segment
        // reading alone returned the table as is.
        let best = collapse_padding_best(&doc).expect("the whole document qualifies");
        assert!(best.len() < doc.len(), "{} → {}", doc.len(), best.len());
        // And nothing disappears.
        for v in [
            "Recv-Q",
            "10.0.2.15.53612",
            "17.32.214.169.443",
            "ESTABLISHED",
        ] {
            assert!(best.contains(v), "`{v}` disappeared:\n{best}");
        }
    }
}

/// `lsusb -v`: Device > Configuration > Interface > Endpoint, told only by
/// 2/4/6/8-space indents — one space per level survives the collapse.
#[test]
fn nesting_depth_survives_the_padding_collapse() {
    let doc = "Device Descriptor:\n  bLength                18\n  Configuration Descriptor:\n    bLength                 9\n    Interface Descriptor:\n      bNumEndpoints           1\n      Endpoint Descriptor:\n        wMaxPacketSize     0x0004  1x 4 bytes\n";
    let out = crate::archetypes::collapse_alignment_padding(doc);
    assert_eq!(
        out,
        "Device Descriptor:\n bLength 18\n Configuration Descriptor:\n  bLength 9\n  Interface Descriptor:\n   bNumEndpoints 1\n   Endpoint Descriptor:\n    wMaxPacketSize 0x0004 1x 4 bytes"
    );
}

/// `pactree -r glibc`: depth drawn with box characters keeps its columns —
/// squeezed, `│   └─b` (a grandchild) read as `│ └─b`, a sibling.
#[test]
fn a_tree_drawing_keeps_its_columns() {
    let doc =
        "glibc\n├─a52dec\n│ ├─mplayer        x\n│ └─vlc-plugin\n│   └─vlc-base      y\n└─zstd";
    let out = crate::archetypes::collapse_alignment_padding(doc);
    assert_eq!(
        out,
        "glibc\n├─a52dec\n│ ├─mplayer x\n│ └─vlc-plugin\n│   └─vlc-base y\n└─zstd"
    );
}

/// `docker images`: `DISK USAGE` and `CONTENT SIZE` are column NAMES — with
/// one space between columns the header no longer says where they end.
#[test]
fn multi_word_column_names_keep_two_spaces() {
    let doc = "IMAGE                    ID             DISK USAGE   CONTENT SIZE   EXTRA\nredis:7.2-alpine         aca82ccee5d9       38.6MB             0B   U\npostgres:18.6-alpine     b07129cc272f        304MB             0B   U\nqdrant/qdrant:v1.19.1    f41023a8821d        198MB             0B\n";
    let out = crate::archetypes::collapse_alignment_padding(doc);
    assert!(out.starts_with("IMAGE  ID  DISK USAGE  CONTENT SIZE  EXTRA\nredis:7.2-alpine  aca82ccee5d9  38.6MB  0B  U\n"), "{out}");
}

/// `df` in French: the header is single-spaced throughout, nine words for six
/// columns — the data columns say which words make one cell.
#[test]
fn a_header_is_cut_where_the_data_columns_are() {
    let doc = "Sys. de fichiers Taille Utilisé Dispo Uti% Monté sur\ndev                 31G       0   31G   0% /dev\nrun                 31G    281M   31G   1% /run\n/dev/nvme0n1p5     1,6T    1,1T  432G  73% /\n";
    let out = crate::archetypes::collapse_alignment_padding(doc);
    assert!(out.starts_with("Sys. de fichiers  Taille  Utilisé  Dispo  Uti%  Monté sur\ndev  31G  0  31G  0%  /dev\n"), "{out}");
    // A header word straddling two columns belongs to the one it covers MORE.
    let straddle = "NAME   LONG WORD QQQQQ  R\naaaa   bbbbbbb   cc  dd\neeee   fffffff   gg  hh\nkkkk   lllllll   mm  nn\n";
    let out = crate::archetypes::collapse_alignment_padding(straddle);
    assert!(
        out.starts_with("NAME  LONG WORD  QQQQQ  R\naaaa  bbbbbbb  cc  dd\n"),
        "{out}"
    );
    let kv = "Architecture :    x86_64 and more words here\nModèle :          97\nFamille :         25\nRévision :        2\n";
    assert!(
        crate::archetypes::collapse_alignment_padding(kv).starts_with("Architecture : x86_64"),
        "a key-value list is not a table"
    );
}
