// The tests of [`crate::index`], moved out of the production file.
//
// Mounted by `include!`, not by `mod`: the tokens are spliced INTO `index`,
// so every `super::` and `super::super::` keeps the meaning it had. A
// rewrite of the paths would have been a semantic change disguised as a
// file move — and it broke at the first `super::super::` encountered.
//
// The split comes from `srcsweep::production_lines`, the declarer of "the
// half of a file that ships": it follows braces AND raw strings. A
// hand-written delimiter saw ONE block in `auto.rs` where there are ten.

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn sym(name: &str, kind: &str, file: &str) -> Symbol {
        Symbol {
            name: name.into(),
            kind: kind.into(),
            file: file.into(),
            line: 1,
            decorators: Vec::new(),
            structural_hash: String::new(),
            self_hash: String::new(),
            end_line: 1,
            bases: Vec::new(),
            owner: String::new(),
        }
    }

    /// `no symbol matching 'X'` names no way out, and it is `query`'s most
    /// common non-answer: 28 of 196 real calls here, against 2% for `skeleton`
    /// — while 247 of 425 code-intel calls ran a `grep` in the same command,
    /// hedging against exactly this and paying for both.
    ///
    /// The misses that happen are wrong-WORD, not mistyped: `cmd_mcp_wrap` is
    /// five edits from `cmd_mcp_proxy` and no edit-distance rule pairs them.
    /// Shared snake/camel tokens do.
    /// A file scope is applied BEFORE the cap, and by one predicate.
    ///
    /// `query` shows at most thirty hits, ranked. `Format` matches 219 symbols
    /// in this repository, so a scope applied to the thirty that survived
    /// would answer "nothing" about a file that has thirteen. Both finders go
    /// through `in_scope` for the same reason: the first version of `--file`
    /// filtered two of `query`'s three renderings and the default listing
    /// answered about the whole repository with the flag sitting there looking
    /// honoured.
    #[test]
    fn a_file_scope_filters_before_the_cap_and_both_finders_honour_it() {
        let idx = Index {
            symbols: vec![
                sym("Format", "type", "crates/core/src/docs.rs"),
                sym("format_size", "fn", "crates/core/src/disk.rs"),
                sym("format_label", "field", "crates/cli/src/recovery.rs"),
            ],
            ..Default::default()
        };
        let cen = std::collections::HashMap::new();

        // Unscoped: every finder sees all three.
        assert_eq!(idx.find_in("format", 30, &[]).1, 3);
        assert_eq!(idx.find_ranked_in("format", 30, &cen, &[]).1, 3);

        // A full relative path, a bare suffix and a directory all name it.
        for want in [
            "crates/core/src/docs.rs",
            "docs.rs",
            "src/docs.rs",
            "crates/core",
            "crates/core/",
        ] {
            let scope = vec![want.to_string()];
            let ranked = idx.find_ranked_in("format", 30, &cen, &scope);
            let plain = idx.find_in("format", 30, &scope);
            assert!(
                ranked.1 >= 1 && plain.1 >= 1,
                "scope {want:?} matched nothing"
            );
            assert!(
                ranked.0.iter().all(|s| s.file.starts_with("crates/core")),
                "scope {want:?} let another crate through: {:?}",
                ranked.0.iter().map(|s| &s.file).collect::<Vec<_>>()
            );
        }

        // A scope that names nothing answers nothing — not everything.
        let none = vec!["no/such/file.rs".to_string()];
        assert_eq!(idx.find_in("format", 30, &none).1, 0);
        assert_eq!(idx.find_ranked_in("format", 30, &cen, &none).1, 0);

        // And an EMPTY entry is a mistake, not a wildcard. `--file ""` reaching
        // the index must narrow to nothing rather than quietly widen to
        // everything: an unscoped answer wearing a scoped flag is the exact
        // failure this filter exists to prevent, and it is the one a mutation
        // reaches by turning `!w.is_empty() &&` into `w.is_empty() ||`.
        // Testing what a predicate must REFUSE, not only what it accepts.
        for empty in [
            vec![String::new()],
            vec!["/".to_string()],
            vec!["./".to_string()],
        ] {
            assert_eq!(
                idx.find_in("format", 30, &empty).1,
                0,
                "an empty scope entry {empty:?} must match nothing, not everything"
            );
            assert_eq!(
                idx.find_ranked_in("format", 30, &cen, &empty).1,
                0,
                "{empty:?}"
            );
        }

        // Applied before the cap: with room for one hit, the scoped file's
        // symbol is the one that survives — not whatever ranked highest
        // across the repository.
        let one = vec!["recovery.rs".to_string()];
        let (hits, total) = idx.find_ranked_in("format", 1, &cen, &one);
        assert_eq!(
            total, 1,
            "the scope must bound the TOTAL, not just the page"
        );
        assert_eq!(hits[0].file, "crates/cli/src/recovery.rs");
    }

    #[test]
    fn a_miss_names_the_symbol_that_was_meant() {
        let idx = Index {
            symbols: vec![
                sym("cmd_mcp_proxy", "fn", "a.rs"),
                sym("uninstall_path_shim", "fn", "b.rs"),
                sym("split_chain_segments", "fn", "c.rs"),
                sym("totally_unrelated", "fn", "d.rs"),
            ],
            ..Default::default()
        };
        for (asked, wanted) in [
            ("cmd_mcp_wrap", "cmd_mcp_proxy"),
            ("uninstall_shim", "uninstall_path_shim"),
            ("split_segments", "split_chain_segments"),
        ] {
            let near = idx.nearest_symbols(asked, 3);
            assert!(
                near.contains(&wanted),
                "`{asked}` should point at `{wanted}`, got {near:?}"
            );
        }
    }

    /// The floor is the other half, and the harder one. A wrong suggestion on
    /// a refusal costs more than no suggestion: a long query
    /// sharing one filler token with an unrelated name must get SILENCE, not a
    /// list. `zzz_no_such_thing_anywhere` shares `no` with `NoHome`.
    #[test]
    fn a_miss_with_nothing_close_suggests_nothing() {
        let idx = Index {
            symbols: vec![
                sym("NoHome", "type", "a.rs"),
                sym("NoBlock", "type", "b.rs"),
                sym("cmd_mcp_proxy", "fn", "c.rs"),
            ],
            ..Default::default()
        };
        assert!(
            idx.nearest_symbols("zzz_no_such_thing_anywhere", 3)
                .is_empty(),
            "one filler token in common is not a suggestion: {:?}",
            idx.nearest_symbols("zzz_no_such_thing_anywhere", 3)
        );
        // And a name that IS the query is never offered back as "closest".
        let idx2 = Index {
            symbols: vec![sym("exact_name", "fn", "a.rs")],
            ..Default::default()
        };
        assert!(
            !idx2
                .nearest_symbols("exact_name", 3)
                .contains(&"exact_name"),
            "a hit is not a near-miss"
        );
    }

    /// A declaration in another LANGUAGE must not make a Rust name ambiguous.
    ///
    /// Call attribution gives up on any name declared in more than one indexed
    /// file — the right trade, since a wrong dependent is worse than a missing
    /// one. But "indexed file" spanned every language, so a `function redact()`
    /// in `npm/install.js` counted as a second home for
    /// `tokenade-core/src/redact.rs` and silently killed attribution for it.
    /// The caller side already refuses to let a `.py` file call a Rust
    /// function; this is the same rule applied to the declaration side.
    ///
    /// Kept deliberately synthetic: swept across 30 real core files the change
    /// moved no count, so a fixture is the only thing that can prove it works.
    #[test]
    fn a_declaration_in_another_language_does_not_blur_a_rust_name() {
        let mut idx = Index {
            root: "/repo".into(),
            ..Default::default()
        };
        idx.symbols.push(sym("scrub", "fn", "src/target.rs"));
        // Same name, different language — not a competing home.
        idx.symbols
            .push(sym("scrub", "function", "tools/helper.js"));
        idx.calls.push(Call {
            caller_name: "run".into(),
            callee_name: "scrub".into(),
            file: "src/caller.rs".into(),
            line: 9,
            qualifier: String::new(),
        });
        // A caller the report cannot attribute must be COUNTED, not dropped:
        // a gap that prints nothing reads as "there is no such caller".
        {
            let mut amb = idx.clone();
            amb.symbols.push(sym("scrub", "fn", "src/rival.rs"));
            let r = amb.impact_of("src/target.rs");
            assert!(
                !r.dependents.iter().any(|d| d == "src/caller.rs"),
                "ambiguous call must not be listed as a dependent"
            );
            assert_eq!(
                r.ambiguous_callers, 1,
                "the dropped caller was not reported: {r:?}"
            );
        }
        let deps = idx.impact_of("src/target.rs").dependents;
        assert!(
            deps.iter().any(|d| d == "src/caller.rs"),
            "the JS declaration blurred the Rust name — dependents: {deps:?}"
        );

        // The guard must not become "ignore every other home": a SECOND Rust
        // declaration is a real ambiguity and must still suppress attribution.
        let mut idx2 = idx.clone();
        idx2.symbols.push(sym("scrub", "fn", "src/other.rs"));
        assert!(
            !idx2
                .impact_of("src/target.rs")
                .dependents
                .iter()
                .any(|d| d == "src/caller.rs"),
            "two Rust homes must stay ambiguous — a wrong dependent is worse \
             than a missing one"
        );
    }

    /// A capped result list that reports only what survived the cap is an
    /// answer that is wrong without looking wrong: `query` is what agents run
    /// INSTEAD of reading files, so 30 hits out of 3000 read as "these are all
    /// of them". The pre-cap count is free — every hit is collected to rank
    /// them — so the only way to lose it is to throw it away.
    #[test]
    fn a_capped_symbol_search_still_reports_how_many_matched() {
        let sym = |name: &str, file: &str| Symbol {
            name: name.into(),
            kind: "fn".into(),
            file: file.into(),
            line: 1,
            end_line: 1,
            decorators: Vec::new(),
            bases: Vec::new(),
            owner: String::new(),
            structural_hash: String::new(),
            self_hash: String::new(),
        };
        let mut idx = Index::default();
        for i in 0..250 {
            idx.symbols.push(sym("detect", &format!("src/m{i}.rs")));
        }
        idx.symbols.push(sym("only_once", "src/solo.rs"));

        let (hits, total) = idx.find_with_total("detect", 30);
        assert_eq!(hits.len(), 30, "the cap still applies");
        assert_eq!(total, 250, "and the caller can tell it was capped");

        let cen = std::collections::HashMap::new();
        let (rhits, rtotal) = idx.find_ranked_with_total("detect", 30, &cen);
        assert_eq!((rhits.len(), rtotal), (30, 250), "same on the ranked path");

        // An uncapped search must report no shortfall, or every answer grows a
        // notice nobody needs.
        let (solo, stotal) = idx.find_with_total("only_once", 30);
        assert_eq!((solo.len(), stotal), (1, 1));
    }

    /// Windows hands back `a\\b\\c.rs` from `Path::display()`, and every reader
    /// in this file splits on `/` — so all files landed in one `.` bucket and
    /// `map` listed a single directory for the whole repository. Not
    /// reproducible on Unix, hence the explicit separator.
    #[test]
    fn stored_paths_speak_forward_slashes_whatever_the_platform() {
        assert_eq!(
            with_forward_slashes("crates\\core\\src\\index.rs".to_string(), '\\'),
            "crates/core/src/index.rs"
        );
        assert_eq!(
            parent_dir(&with_forward_slashes(
                "crates\\core\\src\\index.rs".to_string(),
                '\\'
            )),
            "crates/core/src",
            "the grouping that broke is the one to assert"
        );
        // On a platform whose separator IS `/`, a backslash is an ordinary
        // character in a name and must survive untouched.
        assert_eq!(
            with_forward_slashes("odd\\name.rs".to_string(), '/'),
            "odd\\name.rs"
        );
    }

    #[test]
    fn parent_dir_keeps_full_path_not_first_segment() {
        assert_eq!(parent_dir("crates/core/src/index.rs"), "crates/core/src");
        assert_eq!(parent_dir("src/main.rs"), "src");
        assert_eq!(parent_dir("main.rs"), ".");
        assert_eq!(parent_dir("/abs/path/file.rs"), "/abs/path");
        assert_eq!(parent_dir(""), ".");
        // two files under different deep dirs must NOT collapse together
        assert_ne!(
            parent_dir("crates/a/src/x.rs"),
            parent_dir("crates/b/src/y.rs")
        );
    }

    /// A symlinked directory is not part of the project, and BOTH walkers
    /// have to agree — `build` and `rebuild_incremental` each carry their own
    /// copy of this loop, which is how a fix lands on one and misses the twin.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_directory_is_not_part_of_the_project() {
        let outside = tempdir_lite::Dir::new();
        fs::write(outside.path().join("o.rs"), "pub fn outside_marker() {}\n").unwrap();
        // A symlinked FILE is a different question and stays indexed: it costs
        // one file, not whatever tree the link points at.
        fs::write(
            outside.path().join("linked_file_src.rs"),
            "pub fn linked_file_marker() {}\n",
        )
        .unwrap();

        // `isolated_repo`: `rebuild_incremental` reads the GLOBAL index db,
        // shared with sibling tests and the installed binary — `index db: disk
        // I/O error` in two full runs of 2026-10-03, green alone.
        let (repo, _env) = isolated_repo();
        fs::write(repo.path().join("i.rs"), "pub fn inside_marker() {}\n").unwrap();
        // A REAL subdirectory is still walked — otherwise "skip every
        // directory" would pass this test.
        fs::create_dir(repo.path().join("real_sub")).unwrap();
        fs::write(
            repo.path().join("real_sub/s.rs"),
            "pub fn real_sub_marker() {}\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(outside.path(), repo.path().join("linked_dir")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("linked_file_src.rs"),
            repo.path().join("linked_file.rs"),
        )
        .unwrap();

        let names =
            |idx: &Index| -> Vec<String> { idx.symbols.iter().map(|s| s.name.clone()).collect() };
        let check = |idx: &Index, which: &str| {
            let n = names(idx);
            assert!(
                !n.iter().any(|x| x == "outside_marker"),
                "{which}: indexed a file outside the project through a symlinked directory: {n:?}"
            );
            assert!(
                n.iter().any(|x| x == "inside_marker"),
                "{which}: lost a real file: {n:?}"
            );
            assert!(
                n.iter().any(|x| x == "real_sub_marker"),
                "{which}: stopped walking real subdirectories: {n:?}"
            );
            assert!(
                n.iter().any(|x| x == "linked_file_marker"),
                "{which}: a symlinked FILE is still part of the project: {n:?}"
            );
        };

        let built = Index::build(repo.path()).unwrap();
        check(&built, "build");

        // The incremental walker is a second copy of the same loop. Give it a
        // changed file so it actually re-walks rather than short-circuiting.
        fs::write(
            repo.path().join("i.rs"),
            "pub fn inside_marker() {}\npub fn added_later() {}\n",
        )
        .unwrap();
        let again = Index::rebuild_incremental(&built, repo.path()).unwrap();
        check(&again, "rebuild_incremental");
        assert!(
            names(&again).iter().any(|x| x == "added_later"),
            "the incremental walk did not pick the edit up, so it proved nothing"
        );
    }

    /// The full build and the incremental refresh must select the SAME
    /// files. They are two copies of one loop, and a rule added to one and
    /// not the other makes a query answer differently depending on whether
    /// anything happened to be edited since the index was built.
    #[cfg(unix)]
    #[test]
    fn both_walkers_select_the_same_files() {
        let outside = tempdir_lite::Dir::new();
        let mut oversized = String::from("pub fn oversized_symbol() {}\n");
        while oversized.len() <= super::MAX_INDEX_FILE_BYTES as usize {
            oversized.push_str("// padding padding padding padding padding\n");
        }
        fs::write(outside.path().join("huge.rs"), &oversized).unwrap();
        fs::write(
            outside.path().join("small.rs"),
            "pub fn small_linked_symbol() {}\n",
        )
        .unwrap();

        let (d, _env) = isolated_repo();
        fs::write(d.path().join("a.rs"), "pub fn plain_symbol() {}\n").unwrap();
        fs::write(d.path().join("big.rs"), &oversized).unwrap();
        // Exactly at the cap: the rule is `>`, so this one is KEPT. Without
        // it, `>` and `>=` are the same code.
        let head = "pub fn exactly_at_the_cap() {}\n";
        let mut exact = String::from(head);
        exact.push_str(&"/".repeat(super::MAX_INDEX_FILE_BYTES as usize - head.len()));
        assert_eq!(exact.len(), super::MAX_INDEX_FILE_BYTES as usize);
        fs::write(d.path().join("exact.rs"), &exact).unwrap();
        // An extensionless config file. Neither walker indexes one today —
        // `build` admits nine such names and extraction discards every one,
        // because no grammar or LangSpec exists for them. Pinned so that
        // giving them a parser has to touch BOTH walkers.
        fs::write(d.path().join("Makefile"), "build:\n\t@echo hi\n").unwrap();
        fs::create_dir(d.path().join("sub")).unwrap();
        fs::write(d.path().join("sub/s.rs"), "pub fn sub_symbol() {}\n").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("huge.rs"),
            d.path().join("linked_big.rs"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("small.rs"),
            d.path().join("linked_small.rs"),
        )
        .unwrap();

        let files = |idx: &Index| -> Vec<String> {
            let mut v: Vec<String> = idx.mtimes.keys().cloned().collect();
            v.sort();
            v
        };

        let built = Index::build(d.path()).unwrap();
        fs::write(
            d.path().join("a.rs"),
            "pub fn plain_symbol() {}\npub fn added_later() {}\n",
        )
        .unwrap();
        let inc = Index::rebuild_incremental(&built, d.path()).unwrap();

        assert_eq!(
            files(&built),
            files(&inc),
            "the full build and the incremental refresh disagree about which files the project has"
        );

        // And what that agreed set must BE, so "both walkers select nothing"
        // and "both select everything" each fail.
        let f = files(&built);
        for want in ["a.rs", "exact.rs", "linked_small.rs", "sub/s.rs"] {
            assert!(f.iter().any(|x| x == want), "missing {want}: {f:?}");
        }
        for unwanted in ["big.rs", "linked_big.rs", "Makefile"] {
            assert!(
                !f.iter().any(|x| x == unwanted),
                "{unwanted} should not be indexed: {f:?}"
            );
        }
        assert!(
            inc.symbols.iter().any(|s| s.name == "added_later"),
            "the incremental walk never saw the edit, so it proved nothing"
        );
    }

    fn tmp_repo() -> tempdir_lite::Dir {
        let d = tempdir_lite::Dir::new();
        let mut f = fs::File::create(d.path().join("lib.rs")).unwrap();
        writeln!(
            f,
            "pub fn parse_config() {{}}\npub struct User;\nuse crate::util;"
        )
        .unwrap();
        let mut g = fs::File::create(d.path().join("main.rs")).unwrap();
        writeln!(g, "use crate::lib;\nfn main() {{ parse_config(); }}").unwrap();
        d
    }

    /// Holds `ENV_LOCK` and, on drop, takes `TOKENADE_INDEX_DB` back out
    /// BEFORE the lock goes. Without it the variable outlived its test and
    /// pointed the next one (`incremental_rebuild_detects_mtime_moved_backward`
    /// holds the lock too) at a temp dir already deleted: `index db: unable
    /// to open database file`.
    struct IsolatedDb {
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Drop for IsolatedDb {
        fn drop(&mut self) {
            // SAFETY: `_lock` (ENV_LOCK) is still held — fields drop after this.
            unsafe { std::env::remove_var("TOKENADE_INDEX_DB") };
        }
    }

    /// Isolate the global index db for a test, and give back a repo dir.
    fn isolated_repo() -> (tempdir_lite::Dir, IsolatedDb) {
        let g = crate::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let d = tempdir_lite::Dir::new();
        // SAFETY: env vars are process-wide; ENV_LOCK is held by the caller
        // for as long as it holds the returned guard.
        unsafe {
            std::env::set_var("TOKENADE_INDEX_DB", d.path().join("isolated-index.db"));
        }
        (d, IsolatedDb { _lock: g })
    }

    /// A `.vue` / `.html` file is never READ unless the walker admits its
    /// extension, and neither is in `lang_for`'s regex table. The embedded
    /// `<script>` extractor sat behind that gate, unreachable, and measuring
    /// it by calling the extractor directly said 18 745 symbols while the
    /// index said none — the gate before the gate.
    #[test]
    fn a_script_host_reaches_the_index_with_host_line_numbers() {
        let (d, _g) = isolated_repo();
        let r = d.path();
        std::fs::create_dir_all(r.join("src")).unwrap();
        std::fs::write(
            r.join("src/App.vue"),
            "<template><div/></template>\n<script setup>\nfunction mountThing(){}\n</script>\n",
        )
        .unwrap();
        std::fs::write(
            r.join("src/page.html"),
            "<html>\n<script>\nfunction pageBoot(){}\n</script>\n</html>\n",
        )
        .unwrap();
        std::fs::write(r.join("src/plain.ts"), "export function control(){}\n").unwrap();
        let idx = Index::build(r).expect("build");
        let got: Vec<(String, usize)> = idx
            .symbols
            .iter()
            .map(|s| (s.name.clone(), s.line))
            .collect();
        for want in ["mountThing", "pageBoot", "control"] {
            assert!(
                got.iter().any(|(n, _)| n == want),
                "{want} missing: {got:?}"
            );
        }
        // Line 3 of the HOST file for both, not line 1 of the fragment.
        for want in ["mountThing", "pageBoot"] {
            let line = got.iter().find(|(n, _)| n == want).unwrap().1;
            assert_eq!(line, 3, "{want} line is off the host file: {got:?}");
        }
    }

    /// The lesson: there are TWO walkers, and a gate opened in one of them
    /// is a gate still shut in the other.
    #[test]
    fn the_incremental_walk_admits_a_script_host_too() {
        let (d, _g) = isolated_repo();
        let r = d.path();
        std::fs::create_dir_all(r.join("src")).unwrap();
        std::fs::write(r.join("src/plain.ts"), "export function control(){}\n").unwrap();
        let prev = Index::build(r).expect("seed");
        std::fs::write(
            r.join("src/late.html"),
            "<html>\n<script>\nfunction added(){}\n</script>\n</html>\n",
        )
        .unwrap();
        let next = Index::rebuild_incremental(&prev, r).expect("incremental");
        assert!(
            next.symbols.iter().any(|s| s.name == "added"),
            "the incremental walk skipped the host: {:?}",
            next.symbols.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
    }

    /// A minified bundle is indexed EMPTY by both builds: a committed 200 KB
    /// `vendor.min.js` made 3 000 of 3 001 symbols and topped `map`. And a
    /// source file minified in place loses its symbols — not "kept last good".
    #[test]
    fn a_minified_bundle_contributes_no_symbols() {
        let (d, _g) = isolated_repo();
        let r = d.path();
        let bundle: String = (0..400)
            .map(|i| {
                format!("function a{i}(e,t){{return e.ajax(t,{{url:\"/u{i}\",type:\"POST\"}})}}")
            })
            .collect::<Vec<_>>()
            .join(";");
        std::fs::write(r.join("vendor.min.js"), &bundle).unwrap();
        std::fs::write(
            r.join("app.js"),
            "export function submitLogin(u){ return u; }\n",
        )
        .unwrap();
        let names = |i: &Index| i.symbols.iter().map(|s| s.name.clone()).collect::<Vec<_>>();
        let prev = Index::build(r).expect("build");
        assert_eq!(names(&prev), ["submitLogin"]);
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(r.join("app.js"), &bundle).unwrap();
        let next = Index::rebuild_incremental(&prev, r).expect("incremental");
        assert!(names(&next).is_empty(), "{:?}", names(&next));
    }

    fn write_repo(d: &std::path::Path) {
        fs::write(
            d.join("a.rs"),
            "pub fn target_fn() {}\npub fn caller_one() { target_fn(); }\n",
        )
        .unwrap();
        fs::write(d.join("b.rs"), "pub fn caller_two() { target_fn(); }\n").unwrap();
    }

    /// The call graph is 78 % of the bytes and 60 % of the parse, and 84 % of
    /// code-intel invocations never read an edge. It travels in its own
    /// column now — so the main blob must not carry it, and the column must.
    #[test]
    fn the_call_graph_is_stored_apart_from_the_index() {
        let (d, _g) = isolated_repo();
        write_repo(d.path());
        let idx = Index::build(d.path()).unwrap();
        assert!(!idx.calls.is_empty(), "the fixture must produce edges");
        idx.save(d.path()).unwrap();

        let conn = crate::index_db::open().unwrap();
        let blob = crate::index_db::load_blob(&conn, d.path())
            .unwrap()
            .unwrap();
        let text = String::from_utf8_lossy(&blob);
        assert!(
            !text.contains("\"calls\""),
            "the main blob still carries the graph"
        );
        let calls_blob = crate::index_db::load_calls_blob(&conn, d.path())
            .unwrap()
            .expect("the calls column must hold it");
        let edges: Vec<Call> = serde_json::from_slice(&calls_blob).unwrap();
        assert_eq!(edges.len(), idx.calls.len());
    }

    /// `load` leaves the graph behind; `load_calls` fetches it, twice over.
    #[test]
    fn load_leaves_the_graph_behind_and_load_calls_fetches_it() {
        let (d, _g) = isolated_repo();
        write_repo(d.path());
        Index::build(d.path()).unwrap().save(d.path()).unwrap();

        let mut idx = Index::load(d.path()).expect("saved index");
        assert!(idx.calls.is_empty(), "load must not pay for the graph");
        assert!(idx.callers("target_fn", 10).is_empty());

        idx.load_calls(d.path()).unwrap();
        assert_eq!(idx.callers("target_fn", 10).len(), 2);
        let n = idx.calls.len();
        idx.load_calls(d.path()).unwrap();
        assert_eq!(idx.calls.len(), n, "load_calls must be idempotent");

        // And it must not CLOBBER a graph already in hand. The rebuild prunes
        // edges in place and then keeps going; a second fetch that overwrote
        // them would restore every edge it had just dropped.
        idx.calls.retain(|c| c.file != "b.rs");
        let pruned = idx.calls.len();
        assert!(pruned < n);
        idx.load_calls(d.path()).unwrap();
        assert_eq!(
            idx.calls.len(),
            pruned,
            "load_calls overwrote a graph the caller had already edited"
        );
    }

    /// The hazard the split creates, and the one that would be silent:
    /// `semantic` loads an index WITHOUT the graph, then a file changes and
    /// the rebuild persists. Merging new edges into an empty base would write
    /// a graph truncated to whatever moved — the whole history, gone, with
    /// every suite still green.
    #[test]
    fn a_change_seen_without_the_graph_does_not_truncate_it() {
        let (d, _g) = isolated_repo();
        write_repo(d.path());
        let full = Index::build(d.path()).unwrap();
        full.save(d.path()).unwrap();
        let before = full.calls.len();
        assert!(before >= 2);

        // Exactly what a `semantic` run holds: no graph.
        let lean = Index::load(d.path()).expect("saved index");
        assert!(lean.calls.is_empty());

        // A third file appears, and the rebuild runs off that lean index.
        fs::write(
            d.path().join("c.rs"),
            "pub fn caller_three() { target_fn(); }\n",
        )
        .unwrap();
        let (rebuilt, changed) = Index::rebuild_incremental_reporting(&lean, d.path()).unwrap();
        assert!(changed);
        assert_eq!(
            rebuilt.calls.len(),
            before + 1,
            "the pre-existing edges were dropped on the way through"
        );
        assert_eq!(rebuilt.callers("target_fn", 10).len(), 3);

        // Same hazard on the other branch: a DELETION seen by an index that
        // never loaded the graph. Without the load, the prune runs over an
        // empty vector and the save writes a graph with nothing in it.
        let lean = Index::load(d.path()).expect("saved index");
        assert!(lean.calls.is_empty());
        fs::remove_file(d.path().join("b.rs")).unwrap();
        let (pruned, changed) = Index::rebuild_incremental_reporting(&lean, d.path()).unwrap();
        assert!(changed);
        let names: Vec<&str> = pruned
            .callers("target_fn", 10)
            .iter()
            .map(|c| c.caller_name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["caller_one", "caller_three"],
            "a deletion off a lean index must keep the surviving edges"
        );
        assert!(pruned.calls.iter().all(|c| c.file != "b.rs"));
    }

    /// A file's edges leave with it. `index_file` already dropped a RE-PARSED
    /// file's previous edges, so an edit was handled and only the deletion
    /// path was not — every file ever removed left its outbound edges behind,
    /// and a rename is a deletion.
    #[test]
    fn a_deleted_file_takes_its_call_edges_with_it() {
        let (d, _g) = isolated_repo();
        write_repo(d.path());
        let mut idx = Index::build(d.path()).unwrap();
        assert_eq!(idx.callers("target_fn", 10).len(), 2);

        fs::remove_file(d.path().join("b.rs")).unwrap();
        idx.load_calls(d.path()).ok();
        let (after, changed) = Index::rebuild_incremental_reporting(&idx, d.path()).unwrap();
        assert!(changed);
        let names: Vec<&str> = after
            .callers("target_fn", 10)
            .iter()
            .map(|c| c.caller_name.as_str())
            .collect();
        assert_eq!(names, vec!["caller_one"], "b.rs is gone, its edge is not");
        assert!(after.calls.iter().all(|c| c.file != "b.rs"));
    }

    /// An index written before the split carries its edges INSIDE the main
    /// blob. Deserialisation still accepts them, so upgrading does not blank
    /// a graph that is already on disk.
    #[test]
    fn an_index_written_before_the_split_keeps_its_graph() {
        let legacy = serde_json::json!({
            "root": "/x",
            "symbols": [],
            "imports": {},
            "files_indexed": 1,
            "mtimes": {},
            "built_by": env!("CARGO_PKG_VERSION"),
            "calls": [{
                "caller_name": "old_caller",
                "callee_name": "target_fn",
                "file": "legacy.rs",
                "line": 7,
                "qualifier": ""
            }],
        })
        .to_string();
        let idx: Index = serde_json::from_str(&legacy).unwrap();
        assert_eq!(idx.callers("target_fn", 10).len(), 1);
    }

    /// The index is keyed on the PROJECT, but a command runs in a DIRECTORY,
    /// and both sides of the conversation use that directory as their base.
    #[test]
    fn rebase_rewrites_paths_relative_to_where_the_command_runs() {
        let (d, _g) = isolated_repo();
        write_repo(d.path());
        std::fs::create_dir_all(d.path().join("crates").join("core")).unwrap();
        std::fs::write(
            d.path().join("crates").join("core").join("inner.rs"),
            "pub fn inner_fn() { target_fn(); }\n",
        )
        .unwrap();
        let mut idx = Index::build(d.path()).unwrap();
        assert!(idx.symbols.iter().any(|s| s.file == "crates/core/inner.rs"));
        assert!(idx.symbols.iter().any(|s| s.file == "a.rs"));
        assert!(!idx.rebased);

        idx.rebase_to(&d.path().join("crates").join("core"));
        assert!(idx.rebased);
        // A file UNDER the cwd loses the prefix …
        assert!(
            idx.symbols.iter().any(|s| s.file == "inner.rs"),
            "{:?}",
            idx.symbols.iter().map(|s| &s.file).collect::<Vec<_>>()
        );
        // … and one outside it climbs back out, so the path resolves from
        // where the reader is standing. Before this, `query` handed an agent a
        // path its very next `Read` could not open.
        assert!(idx.symbols.iter().any(|s| s.file == "../../a.rs"));
        // The call graph and the mtime map travel with them.
        assert!(idx.calls.iter().all(|c| !c.file.starts_with("crates/")));
        assert!(idx.mtimes.keys().any(|k| k == "inner.rs"));
    }

    /// `root` is the BASE every path in the index is relative to, so re-basing
    /// must move it too. It did not, and `render_with_context` — which opens
    /// `root.join(file)` to show source — answered "(file unreadable)" from a
    /// subdirectory about a file sitting right there. The `rebased` flag
    /// stopped SAVES; nothing stopped READS.
    #[test]
    fn a_rebased_index_still_reads_the_source_it_names() {
        let (d, _g) = isolated_repo();
        let sub = d.path().join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(
            sub.join("here.rs"),
            "pub fn local_fn() {}\npub fn other() {}\n",
        )
        .unwrap();
        std::fs::write(d.path().join("top.rs"), "pub fn top_fn() {}\n").unwrap();

        let mut idx = Index::build(d.path()).unwrap();
        idx.rebase_to(&sub);

        // A file UNDER the cwd, named without a prefix …
        let hit: Vec<&Symbol> = idx
            .symbols
            .iter()
            .filter(|s| s.name == "local_fn")
            .collect();
        assert_eq!(hit.len(), 1);
        let rendered = idx.render_with_context(&hit, 1);
        assert!(
            rendered.contains("pub fn local_fn"),
            "the source was not read back: {rendered}"
        );
        assert!(!rendered.contains("unreadable"), "{rendered}");

        // … and one ABOVE it, reached through `../`.
        let hit: Vec<&Symbol> = idx.symbols.iter().filter(|s| s.name == "top_fn").collect();
        assert_eq!(hit.len(), 1);
        let rendered = idx.render_with_context(&hit, 0);
        assert!(rendered.contains("../top.rs"), "{rendered}");
        let rendered = idx.render_with_context(&hit, 1);
        assert!(rendered.contains("pub fn top_fn"), "{rendered}");
        assert!(!rendered.contains("unreadable"), "{rendered}");
    }

    /// `cwd == root` is the common case and must cost nothing; a cwd OUTSIDE
    /// the project has no sensible base and is left alone.
    #[test]
    fn rebase_is_a_no_op_at_the_root_and_outside_it() {
        let (d, _g) = isolated_repo();
        write_repo(d.path());
        let built = Index::build(d.path()).unwrap();

        let mut same = built.clone();
        same.rebase_to(d.path());
        assert!(!same.rebased);
        assert_eq!(
            same.symbols.iter().map(|s| &s.file).collect::<Vec<_>>(),
            built.symbols.iter().map(|s| &s.file).collect::<Vec<_>>()
        );

        let mut elsewhere = built.clone();
        elsewhere.rebase_to(std::path::Path::new("/somewhere/else"));
        assert!(!elsewhere.rebased);
        assert_eq!(
            elsewhere
                .symbols
                .iter()
                .map(|s| &s.file)
                .collect::<Vec<_>>(),
            built.symbols.iter().map(|s| &s.file).collect::<Vec<_>>()
        );
    }

    /// The store must only ever hold ROOT-relative paths. Persisting a
    /// re-based index would poison it for every other directory, so `save`
    /// refuses — by construction, not by remembering.
    #[test]
    fn a_rebased_index_is_never_persisted() {
        let (d, _g) = isolated_repo();
        write_repo(d.path());
        std::fs::create_dir_all(d.path().join("sub")).unwrap();
        let built = Index::build(d.path()).unwrap();
        built.save(d.path()).unwrap();
        let stored = Index::load(d.path()).expect("saved");
        let before: Vec<String> = stored.symbols.iter().map(|s| s.file.clone()).collect();

        let mut rebased = built.clone();
        rebased.rebase_to(&d.path().join("sub"));
        assert!(rebased.symbols.iter().any(|s| s.file.starts_with("../")));
        rebased.save(d.path()).unwrap(); // silently refused, not an error

        let after: Vec<String> = Index::load(d.path())
            .expect("saved")
            .symbols
            .iter()
            .map(|s| s.file.clone())
            .collect();
        assert_eq!(after, before, "a re-based index reached the store");
    }

    /// `callers` promised "distinct, ordered" in its doc and delivered
    /// neither: it filtered and `take`-d in walk order, so a helper called
    /// eleven times from one function filled the cap with eleven identical
    /// rows. Nothing in production called it, which is exactly why the drift
    /// survived.
    #[test]
    fn callers_are_distinct_ordered_and_counted() {
        let idx = Index {
            root: String::new(),
            symbols: Vec::new(),
            imports: BTreeMap::new(),
            files_indexed: 0,
            mtimes: BTreeMap::new(),
            built_by: String::new(),
            rebased: false,
            calls: vec![
                Call {
                    caller_name: "zeta".into(),
                    callee_name: "target".into(),
                    file: "z.rs".into(),
                    line: 9,
                    qualifier: String::new(),
                },
                Call {
                    caller_name: "alpha".into(),
                    callee_name: "target".into(),
                    file: "a.rs".into(),
                    line: 40,
                    qualifier: String::new(),
                },
                Call {
                    caller_name: "alpha".into(),
                    callee_name: "target".into(),
                    file: "a.rs".into(),
                    line: 41,
                    qualifier: String::new(),
                },
                Call {
                    caller_name: "alpha".into(),
                    callee_name: "target".into(),
                    file: "a.rs".into(),
                    line: 42,
                    qualifier: String::new(),
                },
                Call {
                    caller_name: "mid".into(),
                    callee_name: "target".into(),
                    file: "m.rs".into(),
                    line: 3,
                    qualifier: String::new(),
                },
                Call {
                    caller_name: "other".into(),
                    callee_name: "unrelated".into(),
                    file: "a.rs".into(),
                    line: 1,
                    qualifier: String::new(),
                },
            ],
        };

        let (hits, total) = idx.callers_with_total("target", 10);
        let got: Vec<&str> = hits.iter().map(|c| c.caller_name.as_str()).collect();
        assert_eq!(
            got,
            vec!["alpha", "mid", "zeta"],
            "distinct, ordered by file"
        );
        assert_eq!(total, 3, "three callers, not six call sites");

        // The cap must report what it hid, the way the hit list already does.
        let (hits, total) = idx.callers_with_total("target", 2);
        assert_eq!(hits.len(), 2);
        assert_eq!(total, 3);

        assert!(idx.callers_with_total("target", 0).0.is_empty());
        assert_eq!(idx.callers_with_total("nobody_calls_me", 10).1, 0);
    }

    /// A read-only question on an unchanged repository must not rewrite the
    /// index. Measured before this reported anything: `save` was 73 821 µs of
    /// a 243 024 µs index path on a 3 980-file repository where not one file
    /// had moved, plus 51 MB written to disk per query.
    #[test]
    fn an_unchanged_repository_reports_nothing_changed() {
        let d = tmp_repo();
        let built = Index::build(d.path()).unwrap();
        let (again, changed) = Index::rebuild_incremental_reporting(&built, d.path()).unwrap();
        assert!(!changed, "nothing moved, yet the rebuild reported a change");
        assert_eq!(again.symbols.len(), built.symbols.len());
        assert_eq!(again.files_indexed, built.files_indexed);
    }

    /// The other half, and the one that matters more: a rebuild that DOES see
    /// something must say so, or the index is never written again.
    #[test]
    fn an_edit_a_new_file_and_a_deletion_each_report_changed() {
        // New file.
        let d = tmp_repo();
        let built = Index::build(d.path()).unwrap();
        fs::write(d.path().join("extra.rs"), "pub fn added_symbol() {}\n").unwrap();
        let (with_new, changed) = Index::rebuild_incremental_reporting(&built, d.path()).unwrap();
        assert!(changed, "a new file is a change");
        assert!(with_new.symbols.iter().any(|s| s.name == "added_symbol"));
        // Sortedness is asserted HERE, where a symbol was APPENDED — `find`
        // ranks on top of this order. A deletion-only rebuild keeps the
        // previous order whether it sorts or not, so asserting it there
        // proves nothing: the mutation that never sorts stayed green until
        // this assertion moved to the case that adds.
        assert_sorted_by_name(&with_new);

        // Edit in place.
        fs::write(
            d.path().join("extra.rs"),
            "pub fn renamed_symbol() {}\npub fn second() {}\n",
        )
        .unwrap();
        let (edited, changed) = Index::rebuild_incremental_reporting(&with_new, d.path()).unwrap();
        assert!(changed, "an edit is a change");
        assert!(edited.symbols.iter().any(|s| s.name == "renamed_symbol"));
        assert!(!edited.symbols.iter().any(|s| s.name == "added_symbol"));

        // Deletion.
        fs::remove_file(d.path().join("extra.rs")).unwrap();
        let (pruned, changed) = Index::rebuild_incremental_reporting(&edited, d.path()).unwrap();
        assert!(changed, "a deletion is a change");
        assert!(!pruned.symbols.iter().any(|s| s.name == "renamed_symbol"));

        assert_sorted_by_name(&pruned);
    }

    /// A file whose parse FAILS must not be treated as a file that parsed to
    /// nothing. Measured before the fix on a two-file repository where one
    /// file was re-saved with a Latin-1 byte: `query beta_two` went from
    /// naming it to answering "no symbol matching", `file_count` read 1 for 2
    /// files, `symbol_count` 2 for 4 — and every read-only query afterwards
    /// rewrote the whole index, forever.
    #[test]
    fn a_file_that_stops_parsing_keeps_its_last_good_symbols() {
        let d = tmp_repo();
        fs::write(
            d.path().join("beta.rs"),
            "pub fn beta_two() {}\npub struct BetaT;\n",
        )
        .unwrap();
        let built = Index::build(d.path()).unwrap();
        assert!(built.symbols.iter().any(|s| s.name == "beta_two"));
        let files_before = built.files_indexed;
        let symbols_before = built.symbols.len();

        // Same file, now not valid UTF-8: `read_to_string` refuses it, so
        // `extract_file` answers None. This is what an editor re-saving in a
        // legacy encoding does, and it is PERMANENT — not a transient blip.
        fs::write(
            d.path().join("beta.rs"),
            b"pub fn beta_two() {} // caf\xe9\npub struct BetaT;\n".as_slice(),
        )
        .unwrap();
        let (after, changed) = Index::rebuild_incremental_reporting(&built, d.path()).unwrap();
        assert!(
            after.symbols.iter().any(|s| s.name == "beta_two"),
            "an unreadable file lost the symbols we already had for it"
        );
        assert_eq!(
            after.files_indexed, files_before,
            "the file count shrank on a parse that failed"
        );
        assert_eq!(after.symbols.len(), symbols_before);
        assert!(
            changed,
            "the fingerprint moved, so it must be persisted once"
        );

        // ...and exactly ONCE. The fingerprint is now recorded, so the next
        // read-only command must not re-enter the branch and rewrite the
        // index. This is the half that costs a rewrite per query when absent.
        let (again, changed_again) =
            Index::rebuild_incremental_reporting(&after, d.path()).unwrap();
        assert!(
            !changed_again,
            "a still-unreadable file re-reported a change: every read-only \
             query would rewrite the whole index"
        );
        assert!(again.symbols.iter().any(|s| s.name == "beta_two"));

        // It heals: readable again, with different content, and the stale
        // symbol goes.
        fs::write(
            d.path().join("beta.rs"),
            "pub fn gamma_three() {}\npub struct BetaT;\n",
        )
        .unwrap();
        let (healed, changed) = Index::rebuild_incremental_reporting(&again, d.path()).unwrap();
        assert!(changed);
        assert!(healed.symbols.iter().any(|s| s.name == "gamma_three"));
        assert!(
            !healed.symbols.iter().any(|s| s.name == "beta_two"),
            "the last-good symbols must be replaced once a real parse lands"
        );
        assert_eq!(healed.files_indexed, files_before);
    }

    /// The full build and the incremental rebuild fold a parse into the index
    /// through ONE declarer. They used to hold two copies of that loop; a
    /// difference between them is invisible in any single-path test.
    #[test]
    fn a_full_build_and_an_incremental_rebuild_agree_on_the_same_tree() {
        let d = tmp_repo();
        let seed = Index::build(d.path()).unwrap();
        // Everything the incremental path can do: add, edit, delete.
        fs::write(d.path().join("added.rs"), "pub fn added_one() {}\n").unwrap();
        fs::write(
            d.path().join("main.rs"),
            "use crate::lib;\nfn main() { parse_config(); }\nfn extra_one() {}\n",
        )
        .unwrap();
        fs::remove_file(d.path().join("lib.rs")).unwrap();

        let (mut incremental, changed) =
            Index::rebuild_incremental_reporting(&seed, d.path()).unwrap();
        assert!(changed);
        incremental.load_calls(d.path()).ok();
        let full = Index::build(d.path()).unwrap();

        let names = |i: &Index| -> Vec<String> {
            let mut v: Vec<String> = i.symbols.iter().map(|s| s.name.clone()).collect();
            v.sort();
            v
        };
        assert_eq!(names(&incremental), names(&full), "symbols diverged");
        assert_eq!(
            incremental.files_indexed, full.files_indexed,
            "file counts diverged"
        );
        let edges = |i: &Index| -> Vec<String> {
            let mut v: Vec<String> = i
                .calls
                .iter()
                .map(|c| format!("{}:{}->{}", c.file, c.caller_name, c.callee_name))
                .collect();
            v.sort();
            v
        };
        assert_eq!(edges(&incremental), edges(&full), "call edges diverged");
        // Non-vacuity: the tree really does carry symbols and edges, so the
        // three equalities above are not comparing empty against empty.
        assert!(names(&full).len() >= 3);
        assert!(!edges(&full).is_empty());
        // And an ABSOLUTE anchor on the count. The equality above is relative:
        // both paths now fold a parse through one declarer, so a mistake made
        // INSIDE that declarer is made identically on both sides and the
        // equality still holds. Measured against the tree itself.
        let on_disk = fs::read_dir(d.path())
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "rs"))
            .count();
        assert_eq!(on_disk, 2, "the fixture is two .rs files after the edits");
        assert_eq!(
            full.files_indexed, on_disk,
            "the index counts a different number of files than the tree holds"
        );
    }

    /// A successful store write takes the legacy sidecars with it.
    ///
    /// They are MIGRATION artefacts: `load` consults them only when the store
    /// has no row, and `migrate_local_index` exists to import and delete them.
    /// Left in place they are a stale copy of the truth that an external reader
    /// cannot tell apart — and `tokenade index` wrote the symbol one back on
    /// every run, 22.2 MB inside this repository against the 5.5 MB the store
    /// keeps for the same index. `SemanticIndex::save` has nuked its own pair
    /// since the store landed; this is the sibling that did not.
    #[test]
    fn a_saved_index_takes_its_legacy_sidecars_with_it() {
        let (d, _g) = isolated_repo();
        fs::write(d.path().join("a.rs"), "pub fn alpha_one() {}\n").unwrap();
        for legacy in [".tokenade", ".sts"] {
            fs::create_dir_all(d.path().join(legacy)).unwrap();
            fs::write(d.path().join(legacy).join("index.json"), "{}").unwrap();
        }
        let idx = Index::build(d.path()).unwrap();
        idx.save(d.path()).expect("the store must accept it");
        for legacy in [".tokenade", ".sts"] {
            assert!(
                !d.path().join(legacy).join("index.json").exists(),
                "{legacy}/index.json survived a successful store write"
            );
        }
    }

    /// And a REFUSED store write must leave it alone: it is then the only copy
    /// `load` can still fall back to. Deleting on the failure path would turn a
    /// storage hiccup into a lost index.
    #[test]
    fn a_refused_save_leaves_the_sidecar_alone() {
        let (d, _g) = isolated_repo();
        fs::write(d.path().join("a.rs"), "pub fn alpha_one() {}\n").unwrap();
        fs::create_dir_all(d.path().join(".tokenade")).unwrap();
        let sidecar = d.path().join(".tokenade").join("index.json");
        fs::write(&sidecar, "{}").unwrap();
        // A FILE where the db needs a directory: `open` cannot create the
        // database under it, so `save` fails before it can remove anything.
        let blocker = d.path().join("blocker");
        fs::write(&blocker, "not a directory").unwrap();
        // SAFETY: env vars are process-wide; `isolated_repo` holds ENV_LOCK.
        unsafe {
            std::env::set_var("TOKENADE_INDEX_DB", blocker.join("sub").join("index.db"));
        }
        let idx = Index::build(d.path()).unwrap();
        assert!(
            idx.save(d.path()).is_err(),
            "the fixture must actually make the store refuse"
        );
        assert!(
            sidecar.exists(),
            "a failed store write deleted the last remaining copy"
        );
    }

    fn assert_sorted_by_name(idx: &Index) {
        let names: Vec<&str> = idx.symbols.iter().map(|s| s.name.as_str()).collect();
        let mut want = names.clone();
        want.sort_unstable();
        assert_eq!(names, want, "the index must stay name-sorted");
    }

    /// The saving itself, asserted rather than assumed. Without this, the
    /// mutation that saves unconditionally stays green and the whole point of
    /// the change is lost on the next edit to this function — the correctness
    /// tests pass either way, because saving too often is not incorrect, only
    /// wasteful.
    #[test]
    fn a_query_that_changes_nothing_does_not_rewrite_the_index() {
        let _env = crate::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let d = tmp_repo();
        let idx_db = d.path().join("isolated-index.db");
        // SAFETY: env vars are process-wide; ENV_LOCK is held above.
        unsafe {
            std::env::set_var("TOKENADE_INDEX_DB", &idx_db);
        }
        struct Restore;
        impl Drop for Restore {
            fn drop(&mut self) {
                // SAFETY: still inside the ENV_LOCK guard's lifetime.
                unsafe {
                    std::env::remove_var("TOKENADE_INDEX_DB");
                }
            }
        }
        let _restore = Restore;

        // First call populates the global db.
        Index::ensure_fresh(d.path()).unwrap();
        let read = |what: &str| -> Vec<u8> {
            let conn = crate::index_db::open().unwrap();
            conn.query_row(
                &format!("SELECT {what} FROM project_indexes WHERE project_path=?1"),
                rusqlite::params![d.path().display().to_string()],
                |r| r.get::<_, Vec<u8>>(0),
            )
            .unwrap()
        };
        let stamp = || -> u64 {
            let conn = crate::index_db::open().unwrap();
            conn.query_row(
                "SELECT last_updated FROM project_indexes WHERE project_path=?1",
                rusqlite::params![d.path().display().to_string()],
                |r| r.get::<_, i64>(0),
            )
            .unwrap() as u64
        };
        let blob_before = read("blob");

        // Move the timestamp back, and stamp `size_bytes` with a sentinel no
        // real save would ever write. Byte-comparing the blob cannot tell "not
        // written" from "written identically" — a mutation that saves
        // unconditionally stayed green against exactly that assertion — but
        // `save_blob` always overwrites `size_bytes` with the true length and
        // `touch` never touches it.
        const SENTINEL_SIZE: i64 = 1;
        {
            let conn = crate::index_db::open().unwrap();
            conn.execute(
                "UPDATE project_indexes SET last_updated=?1, size_bytes=?2 WHERE project_path=?3",
                rusqlite::params![
                    (stamp() - 5_000) as i64,
                    SENTINEL_SIZE,
                    d.path().display().to_string()
                ],
            )
            .unwrap();
        }
        let moved = stamp();
        let size_now = || -> i64 {
            let conn = crate::index_db::open().unwrap();
            conn.query_row(
                "SELECT size_bytes FROM project_indexes WHERE project_path=?1",
                rusqlite::params![d.path().display().to_string()],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
        };

        // Nothing changed on disk: the BLOB must not be rewritten. This used
        // to assert that `last_updated` did not move either, which pinned the
        // wrong property — that column is what every eviction pass orders by,
        // so freezing it on a read is what made a repository queried daily age
        // like an abandoned one. The blob is the expensive thing; the
        // timestamp is supposed to move.
        Index::ensure_fresh(d.path()).unwrap();
        assert_eq!(
            read("blob"),
            blob_before,
            "an unchanged index was rewritten"
        );
        assert_eq!(
            size_now(),
            SENTINEL_SIZE,
            "the row was re-saved: `size_bytes` came back from the blob"
        );
        assert!(
            stamp() > moved,
            "reading a project must mark it used, or eviction reads staleness \
             where there is none"
        );

        // Fresh mark, second read: no write at all. The touch is rate-limited
        // because the write is not free — measured on a 51 MB index, touching
        // on every command cost 36 ms of the 147 ms the command takes, to
        // refresh a mark whose consumers reason in DAYS.
        //
        // The sentinel is a DISTINCT value, not "the same second": comparing
        // `stamp()` across two calls in the same second cannot tell a skipped
        // write from one that rewrote the identical number, and the mutation
        // that touches unconditionally stayed green against exactly that.
        let sentinel = stamp() + 1_000;
        {
            let conn = crate::index_db::open().unwrap();
            conn.execute(
                "UPDATE project_indexes SET last_updated=?1 WHERE project_path=?2",
                rusqlite::params![sentinel as i64, d.path().display().to_string()],
            )
            .unwrap();
        }
        Index::ensure_fresh(d.path()).unwrap();
        assert_eq!(
            stamp(),
            sentinel,
            "a mark refreshed within the interval was rewritten again"
        );

        // And a real change must still write the blob.
        fs::write(d.path().join("added.rs"), "pub fn writes_again() {}\n").unwrap();
        Index::ensure_fresh(d.path()).unwrap();
        assert_ne!(
            read("blob"),
            blob_before,
            "a changed repository was not persisted"
        );
        assert_ne!(
            size_now(),
            SENTINEL_SIZE,
            "the save must restore the true size"
        );
    }

    /// The exception the skip must carry: an index read from a legacy
    /// per-project JSON is written through even when nothing changed.
    /// Skipping it would strand the migration to the global db forever on a
    /// repository whose files never change — the cache would be re-read from
    /// JSON on every call and never move.
    #[test]
    fn a_legacy_json_index_is_migrated_even_when_nothing_changed() {
        let _env = crate::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let d = tmp_repo();
        let idx_db = d.path().join("isolated-index.db");
        // SAFETY: env vars are process-wide; ENV_LOCK is held above.
        unsafe {
            std::env::set_var("TOKENADE_INDEX_DB", &idx_db);
        }
        struct Restore;
        impl Drop for Restore {
            fn drop(&mut self) {
                // SAFETY: still inside the ENV_LOCK guard's lifetime.
                unsafe {
                    std::env::remove_var("TOKENADE_INDEX_DB");
                }
            }
        }
        let _restore = Restore;

        // A legacy index on disk, current-build stamped and in step with the
        // files, and NOTHING in the global db.
        let built = Index::build(d.path()).unwrap();
        fs::create_dir_all(d.path().join(".tokenade")).unwrap();
        fs::write(
            d.path().join(".tokenade").join("index.json"),
            serde_json::to_string(&built).unwrap(),
        )
        .unwrap();
        let conn = crate::index_db::open().unwrap();
        assert!(
            crate::index_db::load_blob(&conn, d.path())
                .unwrap()
                .is_none(),
            "the global db must start empty for this to prove anything"
        );
        drop(conn);

        Index::ensure_fresh(d.path()).unwrap();

        let conn = crate::index_db::open().unwrap();
        assert!(
            crate::index_db::load_blob(&conn, d.path())
                .unwrap()
                .is_some(),
            "an unchanged legacy index was never migrated to the global db"
        );
    }

    /// The `by directory:` block is the bulk of a structure map. Unbounded, it
    /// turned the cheap overview into one of the most expensive reads an agent
    /// could make on a large repository — and most of what it spent the budget
    /// on were directories holding a single file.
    #[test]
    fn a_structure_map_stays_bounded_and_says_when_it_trimmed() {
        let d = tempdir_lite::Dir::new();
        for i in 0..(structure::MAP_MAX_DIRS + 20) {
            let sub = d.path().join(format!("pkg{i:03}"));
            fs::create_dir_all(&sub).unwrap();
            // Denser directories must be the ones kept.
            let syms = if i < 10 { 5 } else { 1 };
            let body: String = (0..syms)
                .map(|k| format!("pub fn f{i}_{k}() {{}}\n"))
                .collect();
            fs::write(sub.join("lib.rs"), body).unwrap();
        }
        let idx = Index::build(d.path()).unwrap();
        let map = idx.structure_map();
        let dir_lines = map
            .lines()
            .filter(|l| l.starts_with("  ") && l.contains(" files, "))
            .count();
        assert_eq!(
            dir_lines,
            structure::MAP_MAX_DIRS,
            "the block must be bounded: {dir_lines}"
        );
        assert!(
            map.contains("directories shown"),
            "a trimmed list that does not say so reads as the whole repo: {map}"
        );
        assert!(
            map.contains("pkg000/"),
            "the densest directories are the ones worth keeping: {map}"
        );
        // Stable between processes: same input, same choice.
        assert_eq!(map, Index::build(d.path()).unwrap().structure_map());

        // The ranked file list says how many it left out, too — it was the
        // last silent cap in this command, and "top files:" is exactly the
        // phrase a reader takes for "these ARE the files that matter".
        assert!(
            map.contains("top files (") && map.contains("of "),
            "the ranked file list must disclose its cap: {map}"
        );

        // A small repository is untouched — no trimming, no notice.
        let small = tempdir_lite::Dir::new();
        fs::write(small.path().join("lib.rs"), "pub fn only() {}\n").unwrap();
        let sm = Index::build(small.path()).unwrap().structure_map();
        assert!(
            !sm.contains("directories shown"),
            "no gratuitous notice: {sm}"
        );
        assert!(
            sm.contains("top files:") && !sm.contains("top files ("),
            "a repo under the cap keeps the plain header: {sm}"
        );
    }

    /// `impl Drop for X` declares no type called `Drop`. Indexing the leading
    /// identifier made every std trait look like a repo-local declaration, so
    /// "where is Drop defined?" answered with a list of files that merely
    /// implement it.
    #[test]
    fn an_impl_block_is_indexed_under_the_type_it_extends() {
        let d = tempdir_lite::Dir::new();
        let mut f = fs::File::create(d.path().join("guard.rs")).unwrap();
        writeln!(
            f,
            "pub struct StashGuard;\n\
             impl Drop for StashGuard {{ fn drop(&mut self) {{}} }}\n\
             impl<T> Cache<T> {{ pub fn get(&self) {{}} }}"
        )
        .unwrap();
        let idx = Index::build(d.path()).unwrap();
        let named = |n: &str| -> Vec<String> {
            idx.symbols
                .iter()
                .filter(|s| s.name == n)
                .map(|s| s.kind.clone())
                .collect()
        };
        assert!(
            named("Drop").is_empty(),
            "a trait this file only implements must not read as declared here: {:?}",
            idx.symbols
        );
        assert!(
            named("StashGuard").contains(&"type".to_string())
                && named("StashGuard").contains(&"impl".to_string()),
            "the struct and its impl both belong to StashGuard: {:?}",
            named("StashGuard")
        );
        // Generics are peeled: `impl<T> Cache<T>` is about `Cache`.
        assert_eq!(named("Cache"), vec!["impl".to_string()]);
    }

    /// A cached index is a cache of one particular extractor. Without a stamp,
    /// every extraction fix stayed invisible on files nobody happened to edit.
    #[test]
    fn an_index_built_by_another_version_is_not_reused() {
        // `Index::load` consults the GLOBAL db first (`index_db::open()`), and
        // that path is derived from HOME unless `TOKENADE_INDEX_DB` says
        // otherwise. Without this redirect the test opened — and created — the
        // developer's real `~/.tokenade/index.db` (52 MB on this box), took
        // SQLite locks on it, and shared them with every other test in the run
        // and with any other tokenade process alive at the time. Verified by
        // running the test binary under an isolated HOME: `~/.tokenade/index.db`
        // appeared there. Same defect the `TOKENADE_SEMANTIC_DB` redirect fixed
        // for `semantic.rs`; the override already existed here, nothing used it.
        //
        // ENV_LOCK is held for the whole test, per that lock's contract: the
        // mutation AND every read of the value it guards.
        let _env = crate::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let d = tempdir_lite::Dir::new();
        let idx_db = d.path().join("isolated-index.db");
        // SAFETY: env vars are process-wide; ENV_LOCK is held above.
        unsafe {
            std::env::set_var("TOKENADE_INDEX_DB", &idx_db);
        }
        struct Restore;
        impl Drop for Restore {
            fn drop(&mut self) {
                // SAFETY: still inside the ENV_LOCK guard's lifetime.
                unsafe {
                    std::env::remove_var("TOKENADE_INDEX_DB");
                }
            }
        }
        let _restore = Restore;
        fs::create_dir_all(d.path().join(".tokenade")).unwrap();
        let mut idx = Index::build(d.path()).unwrap();
        assert_eq!(
            idx.built_by,
            env!("CARGO_PKG_VERSION"),
            "a freshly built index must stamp the build that made it"
        );
        idx.built_by = "0.0.0-from-an-older-tokenade".to_string();
        fs::write(
            d.path().join(".tokenade").join("index.json"),
            serde_json::to_string(&idx).unwrap(),
        )
        .unwrap();
        assert!(
            Index::load(d.path()).is_none(),
            "a foreign build's index must be rejected so the caller rebuilds"
        );
    }

    /// A test file is one every test runner would call a test file.
    ///
    /// The attribute and `mod tests` signals are Rust's, and they see NOTHING
    /// in another language. Measured on three indexed repositories: inline
    /// detection found 44% of this workspace's symbols, **0% of a Python
    /// project's where the path rule finds 63%**, and 0% of a TypeScript
    /// one's. Without the path half, `map` reads a repository that is two
    /// thirds test as if all of it were code.
    ///
    /// Directory components and file suffixes, never a bare name: rejecting
    /// `test_connection` because it starts with `test_` would be a known
    /// mistake, and `testing/` is left out because a library can ship test
    /// helpers there for its users.
    #[test]
    fn a_test_file_is_recognised_by_its_path_in_any_language() {
        for p in [
            "tests/test_tokenizer.py",
            "crates/tokenade-core/tests/web_fidelity.rs",
            "src/__tests__/render.tsx",
            "spec/models/user_spec.rb",
            "pkg/store/store_test.go",
            "app/lib/parser.test.ts",
            "app/lib/parser.spec.js",
            "tests/conftest.py",
        ] {
            assert!(file_is_test_path(p), "`{p}` is a test file");
        }
        for p in [
            // A function named for testing is not a test file.
            "src/net/test_connection.rs",
            // Nor is a directory a library ships for its users.
            "src/testing/harness.rs",
            // Nor a source file whose name merely contains the word.
            "src/latest.rs",
            "src/protest.py",
            "src/contest/round.ts",
        ] {
            assert!(!file_is_test_path(p), "`{p}` is NOT a test file");
        }
    }

    /// A test declaration decides nothing about who depends on a file.
    ///
    /// Two ways it used to. A test helper named `out`, `sym` or `scratch` put
    /// that name into the file's declared set, so every file calling anything
    /// so named became a caller the report had to give up on. And a test
    /// declaration in ANOTHER file gave a real name a second "home", which
    /// abandons attribution for all of its callers. Measured on this
    /// repository: `disclose.rs` went from 13 unattributed callers to 4,
    /// `redact.rs` from 6 to 0, and `code_intel.rs` from one dependent to
    /// four.
    #[test]
    fn a_test_declaration_does_not_decide_who_depends_on_a_file() {
        let d = tempdir_lite::Dir::new();
        let mut lib = fs::File::create(d.path().join("lib.rs")).unwrap();
        writeln!(lib, "pub fn fold_it() {{}}").unwrap();
        writeln!(lib, "#[cfg(test)]").unwrap();
        writeln!(lib, "mod tests {{").unwrap();
        writeln!(lib, "fn scratch() {{}}").unwrap();
        writeln!(lib, "}}").unwrap();
        // A real caller of the API name.
        let mut user = fs::File::create(d.path().join("user.rs")).unwrap();
        writeln!(user, "pub fn go() {{ fold_it(); }}").unwrap();
        // A file that calls something named `scratch` — its own helper, which
        // has nothing to do with `lib.rs`.
        let mut other = fs::File::create(d.path().join("other.rs")).unwrap();
        writeln!(other, "fn scratch() {{}}").unwrap();
        writeln!(other, "pub fn run_it() {{ scratch(); }}").unwrap();
        // And a SECOND home for the API name, inside another file's tests.
        let mut rival = fs::File::create(d.path().join("rival.rs")).unwrap();
        writeln!(rival, "#[cfg(test)]").unwrap();
        writeln!(rival, "mod tests {{").unwrap();
        writeln!(rival, "fn fold_it() {{}}").unwrap();
        writeln!(rival, "}}").unwrap();

        let rep = Index::build(d.path()).unwrap().impact_of("lib.rs");
        assert!(
            rep.dependents.iter().any(|f| f.ends_with("user.rs")),
            "a rival home inside another file's tests must not cost the real \
             caller its attribution: {rep:?}"
        );
        assert!(
            !rep.dependents.iter().any(|f| f.ends_with("other.rs")),
            "a file calling its OWN helper is not a dependent: {rep:?}"
        );
        assert_eq!(
            rep.ambiguous_callers, 0,
            "and nothing is reported as unattributable either: {rep:?}"
        );
    }

    /// A test tree pushed out of the map by the code ranking is NAMED.
    ///
    /// Ranking by code sends a repository's test directories to the bottom,
    /// and on this workspace that removed `tests/e2e/` and `tests/` from the
    /// map outright — several hundred declarations, invisible. A suite is
    /// orienting even though nothing depends on it, so the truncation note
    /// says which trees it left out.
    #[test]
    fn a_test_tree_dropped_by_the_ranking_is_named() {
        let d = tempdir_lite::Dir::new();
        for i in 0..(structure::MAP_MAX_DIRS + 1) {
            let dir = d.path().join(format!("m{i:03}"));
            fs::create_dir_all(&dir).unwrap();
            let mut f = fs::File::create(dir.join("lib.rs")).unwrap();
            writeln!(f, "pub fn api_{i}() {{}}").unwrap();
        }
        fs::create_dir_all(d.path().join("tests")).unwrap();
        let mut t = fs::File::create(d.path().join("tests").join("e2e.rs")).unwrap();
        for i in 0..5 {
            writeln!(t, "pub fn case_{i}() {{}}").unwrap();
        }
        let out = Index::build(d.path()).unwrap().structure_map();
        assert!(
            out.contains("directories shown"),
            "the fixture must be big enough to truncate:\n{out}"
        );
        assert!(
            out.contains("Test trees left out: tests/ (5 test)"),
            "a dropped test tree must be named, not silently gone:\n{out}"
        );
    }

    /// `map` ranks directories and files by CODE, not by test volume.
    ///
    /// The block keeps the 80 densest of a repository's directories, and 44%
    /// of this workspace's indexed symbols are test declarations. Measured
    /// before the split: eight `*_dispatcher/` directories holding ONE code
    /// symbol each were kept on the strength of 8 to 12 tests, eleven
    /// directories differed between the two rankings, and three of the ten
    /// "top files" owed their place to a test module. The canonical-path
    /// boost right below already knew this shape — its comment says it exists
    /// so "a single-symbol canonical beats a 30-symbol test helper file" —
    /// but it only rescued framework entry points.
    #[test]
    fn a_map_ranks_by_code_and_counts_tests_apart() {
        let d = tempdir_lite::Dir::new();
        // One file with a small API and a big test module, one with a bigger
        // API and none. Ranked by total the first wins; ranked by code the
        // second does.
        fs::create_dir_all(d.path().join("src")).unwrap();
        let mut a = fs::File::create(d.path().join("src").join("thin.rs")).unwrap();
        writeln!(a, "pub fn only_one() {{}}").unwrap();
        writeln!(a, "#[cfg(test)]").unwrap();
        writeln!(a, "mod tests {{").unwrap();
        for i in 0..8 {
            writeln!(a, "#[test]\nfn t{i}() {{}}").unwrap();
        }
        writeln!(a, "}}").unwrap();
        let mut b = fs::File::create(d.path().join("src").join("thick.rs")).unwrap();
        for i in 0..4 {
            writeln!(b, "pub fn api_{i}() {{}}").unwrap();
        }
        // A bare `mod test;` DECLARATION: named like a test module, encloses
        // nothing here. Read as a body — a one-line span — it would take
        // itself, and whatever else shared that line, out of the code count.
        writeln!(b, "mod test;").unwrap();
        let out = Index::build(d.path()).unwrap().structure_map();
        let thin = out.find("thin.rs").expect("thin.rs listed");
        let thick = out.find("thick.rs").expect("thick.rs listed");
        assert!(
            thick < thin,
            "four API functions must outrank one function with eight tests:\n{out}"
        );
        // Nine, not eight: the `mod tests` module is itself test scaffolding
        // and falls inside its own span, which is the answer we want. Both
        // lines are pinned, because the two renderings are separate code and
        // a guard that accepts either lets one of them go silent.
        assert!(
            out.contains("src/   2 files, 6 symbols (+9 test)"),
            "the directory line must carry the code count and the test count:\n{out}"
        );
        assert!(
            out.contains("src/thin.rs  (1 symbols +9 test)"),
            "and so must the file line:\n{out}"
        );
        assert!(
            out.contains("src/thick.rs  (5 symbols)"),
            "a bare `mod test;` declaration is not a test body:\n{out}"
        );
        // The header still reports everything the index holds — the split
        // changes what RANKS, not what was indexed.
        assert!(
            out.starts_with("2 files, 15 symbols indexed"),
            "the header counts every indexed symbol:\n{out}"
        );
        // Same for a SUBTREE map, which counts from the directory table
        // instead of from the index and is therefore a second place the total
        // can lose its test half.
        let idx = Index::build(d.path()).unwrap();
        let under = idx.structure_map_under(crate::framework::Framework::Unknown, Some("src"));
        assert!(
            under.starts_with("2 files, 15 symbols indexed"),
            "a subtree header counts every symbol under it too:\n{under}"
        );
    }

    /// What a file declares FOR ITS TESTS is counted apart from what it offers.
    ///
    /// `impact` answers "what depends on this file", and nothing outside the
    /// file can depend on a `#[cfg(test)]` declaration. Counting them together
    /// overstated the surface by about half — 9 592 of this workspace's 17 763
    /// indexed functions are test functions, and `systemd.rs` read `fn (20)`
    /// for seven real ones. Source order (the earlier mitigation) only helps
    /// while the API is longer than the twelve-name window; on that file five
    /// of the twelve shown were tests.
    ///
    /// Two signals, because each misses what the other catches: an attribute
    /// (`#[test]`, `#[tokio::test]`) for tests whose module span was never
    /// captured, and the span of a `mod tests` for the helpers that carry no
    /// attribute of their own.
    #[test]
    fn a_files_own_tests_are_counted_apart_from_what_it_offers() {
        let d = tempdir_lite::Dir::new();
        let mut f = fs::File::create(d.path().join("svc.rs")).unwrap();
        writeln!(
            f,
            "pub fn serve_request() {{}}\n\
             mod test;\n\
             #[inline]\n\
             pub fn shutdown() {{}}\n\
             #[test]\n\
             fn an_attributed_test_outside_any_module() {{}}\n\
             #[cfg(test)]\n\
             mod tests {{\n\
             fn a_helper_with_no_attribute() {{}}\n\
             #[test]\n\
             fn an_attributed_test() {{}}\n\
             }}"
        )
        .unwrap();
        // An OUT-OF-LINE test file: no attribute this indexer reads, no
        // `mod tests` — only its path says what it is, which is the only
        // signal there is in Python, Go or TypeScript.
        fs::create_dir_all(d.path().join("tests")).unwrap();
        let mut t = fs::File::create(d.path().join("tests").join("integration.rs")).unwrap();
        writeln!(t, "pub fn drives_the_whole_thing() {{}}").unwrap();
        let out_of_line = Index::build(d.path())
            .unwrap()
            .impact_of("tests/integration.rs");
        assert!(
            out_of_line.symbols.is_empty(),
            "a file under `tests/` offers nothing to depend on: {:?}",
            out_of_line.symbols
        );
        assert_eq!(
            out_of_line.test_symbols.len(),
            1,
            "and its declaration is still counted: {:?}",
            out_of_line.test_symbols
        );
        let rep = Index::build(d.path()).unwrap().impact_of("svc.rs");
        let api: Vec<&str> = rep.symbols.iter().map(|s| s.as_str()).collect();
        assert!(
            api.iter().any(|s| s.ends_with("serve_request"))
                && api.iter().any(|s| s.ends_with("shutdown")),
            "the file's own API must survive the split: {api:?}"
        );
        for hidden in [
            "an_attributed_test_outside_any_module",
            "a_helper_with_no_attribute",
            "an_attributed_test",
        ] {
            assert!(
                !api.iter().any(|s| s.ends_with(hidden)),
                "`{hidden}` is test scaffolding and must not count as declared surface: {api:?}"
            );
            assert!(
                rep.test_symbols.iter().any(|s| s.ends_with(hidden)),
                "`{hidden}` must still be COUNTED, not dropped: {:?}",
                rep.test_symbols
            );
        }
        // A bare `mod test;` declaration has a one-line span, and it is NAMED
        // like a test module. Read as a body it would swallow whatever shares
        // that line, so the span rule demands `end_line > line` — a
        // declaration points at another file and encloses nothing here.
        assert!(
            api.iter().any(|s| s.ends_with("test")),
            "a module DECLARATION is not a module body: {api:?}"
        );
        // And the attribute half must read the attribute's NAME, not merely
        // notice that one is present: `#[inline]`, `#[derive(…)]` and
        // `#[serde(default)]` sit on ordinary API items all over this
        // workspace, and treating any decorated symbol as a test would empty
        // the answer this command exists to give.
        assert!(
            api.iter().any(|s| s.ends_with("shutdown")),
            "`#[inline]` is not a test attribute: {api:?}"
        );
    }

    /// A file's report must read in source order. Consumers show only the
    /// first handful of names per kind, so by-name order handed that window to
    /// whatever sorted first — on a test-heavy file, the inline test module.
    #[test]
    fn a_file_reports_what_it_declares_in_source_order() {
        let d = tempdir_lite::Dir::new();
        let mut f = fs::File::create(d.path().join("svc.rs")).unwrap();
        writeln!(
            f,
            "pub fn serve_request() {{}}\n\
             pub fn shutdown() {{}}\n\
             pub fn aaa_declared_last() {{}}\n\
             #[cfg(test)]\n\
             mod tests {{\n\
             fn aaa_first_alphabetically() {{}}\n\
             fn aab_also_a_test() {{}}\n\
             }}"
        )
        .unwrap();
        let idx = Index::build(d.path()).unwrap();
        let rep = idx.impact_of("svc.rs");
        let fns: Vec<&String> = rep
            .symbols
            .iter()
            .filter(|s| s.starts_with("fn "))
            .collect();
        assert_eq!(
            fns.first().map(|s| s.as_str()),
            Some("fn serve_request"),
            "the API must lead, not the alphabetically-first test: {:?}",
            rep.symbols
        );
        // The order question now lives entirely among API names: an inline
        // test module no longer reaches this list at all (see
        // `a_files_own_tests_are_counted_apart_from_what_it_offers`). So the
        // file declares an API function whose name sorts FIRST and whose line
        // comes LAST, which is the only way left to tell the two orders apart.
        assert!(
            fns.iter().position(|s| s.ends_with("serve_request"))
                < fns.iter().position(|s| s.ends_with("aaa_declared_last")),
            "source order lost: {:?}",
            rep.symbols
        );
        assert!(
            !fns.iter().any(|s| s.ends_with("aaa_first_alphabetically")),
            "an inline test must not be in the declared surface at all: {:?}",
            rep.symbols
        );
        // Stable across rebuilds — the window must not move between processes.
        let again = Index::build(d.path()).unwrap().impact_of("svc.rs");
        assert_eq!(rep.symbols, again.symbols, "report order is not stable");
    }

    #[test]
    fn builds_and_finds_symbols() {
        let d = tmp_repo();
        let idx = Index::build(d.path()).unwrap();
        assert!(idx.files_indexed >= 2);
        let hits = idx.find("parse_config", 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, "fn");
        assert_eq!(hits[0].line, 1);
    }

    /// At equal relevance (same exact-name match tier, same name length)
    /// the real source file must outrank its test twin. `zeta.rs` sorts
    /// AFTER `alpha_test.rs` alphabetically, so only the file-class
    /// tie-break can put the source first.
    #[test]
    fn find_ranks_source_above_test_at_equal_relevance() {
        let d = tempdir_lite::Dir::new();
        let mut src = fs::File::create(d.path().join("zeta.rs")).unwrap();
        writeln!(src, "pub fn shared_helper() {{}}").unwrap();
        let mut tst = fs::File::create(d.path().join("alpha_test.rs")).unwrap();
        writeln!(tst, "pub fn shared_helper() {{}}").unwrap();
        let idx = Index::build(d.path()).unwrap();
        let hits = idx.find("shared_helper", 5);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].file, "zeta.rs", "source must rank before test twin");
        assert_eq!(hits[1].file, "alpha_test.rs");
    }

    /// A caller that never writes an import statement is still a caller.
    /// Measured on this repository before the fix: `logs.rs` reported 3
    /// dependents where 9 files reach into it, because the report was built
    /// from import statements alone and Rust reaches a module by path.
    #[test]
    fn a_caller_without_an_import_statement_is_still_a_dependent() {
        let d = tempdir_lite::Dir::new();
        let mut f = fs::File::create(d.path().join("engine.rs")).unwrap();
        writeln!(f, "pub fn distinctive_engine_entrypoint() {{}}").unwrap();
        // No `use` anywhere: the caller reaches the module by path, which is
        // how most of this codebase calls across modules.
        let mut g = fs::File::create(d.path().join("caller.rs")).unwrap();
        writeln!(
            g,
            "fn go() {{ crate::engine::distinctive_engine_entrypoint(); }}"
        )
        .unwrap();
        // A different language cannot be calling into a Rust module, however
        // its own function names happen to read.
        let mut h = fs::File::create(d.path().join("script.py")).unwrap();
        writeln!(h, "def go():\n    distinctive_engine_entrypoint()").unwrap();

        let idx = Index::build(d.path()).unwrap();
        let r = idx.impact_of("engine.rs");
        assert!(
            r.dependents.iter().any(|f| f.ends_with("caller.rs")),
            "the path-qualified caller was missed: {:?}",
            r.dependents
        );
        assert!(
            !r.dependents.iter().any(|f| f.ends_with(".py")),
            "a python file was reported as calling a Rust module: {:?}",
            r.dependents
        );
    }

    /// `console_logs` is not `logs`. A substring match said it was, and this
    /// repository carries three modules whose name ends that way.
    #[test]
    fn a_longer_module_name_is_not_the_module_you_asked_about() {
        let d = tempdir_lite::Dir::new();
        fs::write(d.path().join("logs.rs"), "pub fn fold_it() {}\n").unwrap();
        fs::write(
            d.path().join("console_logs.rs"),
            "pub fn other_thing() {}\n",
        )
        .unwrap();
        fs::write(
            d.path().join("user.rs"),
            "use crate::console_logs;\nfn go() { console_logs::other_thing(); }\n",
        )
        .unwrap();
        let idx = Index::build(d.path()).unwrap();
        assert!(
            idx.impact_of("logs.rs").dependents.is_empty(),
            "a console_logs importer was charged to logs.rs: {:?}",
            idx.impact_of("logs.rs").dependents
        );
        assert!(
            idx.impact_of("console_logs.rs")
                .dependents
                .iter()
                .any(|f| f.ends_with("user.rs")),
            "the real dependent was lost with the false one"
        );
    }

    #[test]
    fn structure_map_compact() {
        let d = tmp_repo();
        let idx = Index::build(d.path()).unwrap();
        let map = idx.structure_map();
        assert!(map.contains("symbols indexed"));
    }

    /// #937 — empty `file` made `Path::new("").file_stem()` return
    /// None, fallback to "" via unwrap_or, then `deps.contains("")`
    /// matched every file → 4200-entry junk report. Defense-in-depth
    /// lives in `Index::impact_of` itself (the CLI + MCP layers
    /// also reject upfront for better UX).
    #[test]
    fn impact_of_empty_file_returns_empty_report() {
        let d = tmp_repo();
        let idx = Index::build(d.path()).unwrap();
        for case in &["", "  ", "\t\n"] {
            let r = idx.impact_of(case);
            assert!(
                r.dependents.is_empty(),
                "empty/whitespace file {case:?} must not match anything; got {:?}",
                r.dependents
            );
            assert!(
                r.symbols.is_empty(),
                "no symbols either, got {:?}",
                r.symbols
            );
        }
        // Sanity: real file still works.
        let r = idx.impact_of("lib.rs");
        assert!(
            !r.symbols.is_empty(),
            "lib.rs should report its symbols, got empty"
        );
    }

    #[test]
    fn render_with_context_zero_returns_legacy_format() {
        let d = tmp_repo();
        let idx = Index::build(d.path()).unwrap();
        let hits = idx.find("parse_config", 5);
        let out = idx.render_with_context(&hits, 0);
        assert_eq!(out, "fn parse_config — lib.rs:1");
    }

    #[test]
    fn render_with_context_one_emits_grep_format() {
        let d = tempdir_lite::Dir::new();
        // 5-line file so we can verify center + 1-above + 1-below
        // each get the expected `-` / `:` separator.
        let mut f = fs::File::create(d.path().join("lib.rs")).unwrap();
        writeln!(
            f,
            "// line 1\n// line 2\npub fn foo() {{}}\n// line 4\n// line 5"
        )
        .unwrap();
        let idx = Index::build(d.path()).unwrap();
        let hits = idx.find("foo", 5);
        assert_eq!(hits.len(), 1);
        let out = idx.render_with_context(&hits, 1);
        // line 2 is context (- separator), line 3 is the match
        // (: separator), line 4 is context (- separator).
        assert!(out.contains("lib.rs-2-// line 2"), "got:\n{out}");
        assert!(out.contains("lib.rs:3:pub fn foo() {}"), "got:\n{out}");
        assert!(out.contains("lib.rs-4-// line 4"), "got:\n{out}");
        // Must NOT bleed into line 1 or line 5 with N=1.
        assert!(!out.contains("// line 1"), "leaked above:\n{out}");
        assert!(!out.contains("// line 5"), "leaked below:\n{out}");
    }

    #[test]
    fn render_with_context_clamps_at_file_edges() {
        let d = tempdir_lite::Dir::new();
        // Symbol on line 1: above-context must clamp to 0, not underflow.
        let mut f = fs::File::create(d.path().join("lib.rs")).unwrap();
        writeln!(f, "pub fn at_start() {{}}\n// trailing").unwrap();
        let idx = Index::build(d.path()).unwrap();
        let hits = idx.find("at_start", 5);
        let out = idx.render_with_context(&hits, 5);
        // No panic + exactly 2 emitted lines (line 1 match + line 2 ctx).
        let line_count = out.lines().count();
        assert_eq!(line_count, 2, "got:\n{out}");
        assert!(out.contains("lib.rs:1:pub fn at_start() {}"));
    }

    #[test]
    fn render_with_context_separates_multiple_hits() {
        let d = tempdir_lite::Dir::new();
        let mut f = fs::File::create(d.path().join("a.rs")).unwrap();
        writeln!(f, "pub fn shared() {{}}").unwrap();
        let mut g = fs::File::create(d.path().join("b.rs")).unwrap();
        writeln!(g, "pub fn shared() {{}}").unwrap();
        let idx = Index::build(d.path()).unwrap();
        let hits = idx.find("shared", 5);
        assert_eq!(hits.len(), 2);
        let out = idx.render_with_context(&hits, 0);
        // 0-context = legacy `\n`-joined lines, one per hit.
        assert_eq!(out.lines().count(), 2);
        let out_ctx = idx.render_with_context(&hits, 1);
        // With context, hits are separated by a `--\n` divider.
        assert!(out_ctx.contains("\n--\n"), "got:\n{out_ctx}");
    }

    // ---- Adversarial: new language coverage ------------------------------

    fn write_at(dir: &std::path::Path, name: &str, content: &str) {
        let mut f = fs::File::create(dir.join(name)).unwrap();
        writeln!(f, "{content}").unwrap();
    }
    fn write(d: &tempdir_lite::Dir, name: &str, content: &str) {
        write_at(d.path(), name, content);
    }

    #[test]
    fn ruby_methods_and_classes_indexed() {
        let d = tempdir_lite::Dir::new();
        write(
            &d,
            "user.rb",
            "require 'json'\nrequire_relative 'helpers'\n\
             module Auth\n  class User\n    def login(user, pw)\n    end\n    def self.find(id)\n    end\n    def valid?\n    end\n  end\nend",
        );
        let idx = Index::build(d.path()).unwrap();
        let names: Vec<&str> = idx.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"User"), "class missing: {names:?}");
        assert!(names.contains(&"Auth"), "module missing: {names:?}");
        assert!(names.contains(&"login"), "method missing: {names:?}");
        assert!(names.contains(&"find"), "self method missing: {names:?}");
        assert!(
            names.contains(&"valid?"),
            "predicate method missing: {names:?}"
        );
        // Imports captured.
        let imports = idx.imports.get("user.rb").expect("imports for user.rb");
        assert!(imports.contains(&"json".to_string()));
    }

    #[test]
    fn php_classes_and_namespaces_indexed() {
        let d = tempdir_lite::Dir::new();
        write(
            &d,
            "Repo.php",
            "<?php\nnamespace App\\Auth;\nuse PDO;\nuse App\\User;\n\
             class Repository {\n  public function find(int $id): ?User {}\n  private static function cache() {}\n}\ninterface Cacheable {}",
        );
        let idx = Index::build(d.path()).unwrap();
        let names: Vec<&str> = idx.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Repository"));
        assert!(names.contains(&"find"));
        assert!(names.contains(&"cache"));
        assert!(names.contains(&"Cacheable"));
    }

    #[test]
    fn swift_funcs_classes_protocols_indexed() {
        let d = tempdir_lite::Dir::new();
        write(
            &d,
            "Login.swift",
            "import Foundation\nimport SwiftUI\n\
             public protocol AuthProvider {}\n\
             public struct Credentials {\n  let username: String\n  let password: String\n}\n\
             public final class LoginService {\n  func authenticate(_ c: Credentials) async -> Bool { false }\n  static func shared() -> LoginService { LoginService() }\n}",
        );
        let idx = Index::build(d.path()).unwrap();
        let names: Vec<&str> = idx.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"LoginService"));
        assert!(names.contains(&"Credentials"));
        assert!(names.contains(&"AuthProvider"));
        assert!(names.contains(&"authenticate"));
    }

    #[test]
    fn csharp_types_and_methods_indexed() {
        let d = tempdir_lite::Dir::new();
        write(
            &d,
            "UserService.cs",
            "using System;\nusing System.Threading.Tasks;\n\
             namespace App.Services {\n  public class UserService {\n    public async Task<int> FindAsync(string id) { return 0; }\n    private static bool Cache;\n  }\n  public interface IUserRepo {}\n  public record User(string Name);\n}",
        );
        let idx = Index::build(d.path()).unwrap();
        let names: Vec<&str> = idx.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"UserService"));
        assert!(names.contains(&"IUserRepo"));
        assert!(names.contains(&"User"));
        assert!(
            names.contains(&"FindAsync"),
            "async method missing: {names:?}"
        );
    }

    #[test]
    fn scala_objects_traits_defs_indexed() {
        let d = tempdir_lite::Dir::new();
        write(
            &d,
            "Auth.scala",
            "package app.auth\nimport scala.concurrent.Future\nimport play.api.libs.json.{Json, Reads}\n\
             trait AuthRepo {\n  def find(id: String): Future[Option[User]]\n}\n\
             case class User(name: String)\n\
             object UserService extends AuthRepo {\n  override def find(id: String) = ???\n}",
        );
        let idx = Index::build(d.path()).unwrap();
        let names: Vec<&str> = idx.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"AuthRepo"));
        assert!(names.contains(&"User"));
        assert!(names.contains(&"UserService"));
        assert!(names.contains(&"find"));
    }

    #[test]
    fn elixir_modules_and_defs_indexed() {
        let d = tempdir_lite::Dir::new();
        write(
            &d,
            "auth.ex",
            "defmodule App.Auth do\n  alias App.User\n  import Ecto.Query\n\n  defmodule User do\n    defstruct [:id, :email]\n  end\n\n  def login(email, password) do\n    :ok\n  end\n\n  defp hash_password(p), do: p\n\n  defmacro with_user(do: block) do\n    quote do: unquote(block)\n  end\nend",
        );
        let idx = Index::build(d.path()).unwrap();
        let names: Vec<&str> = idx.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"App.Auth"));
        assert!(names.contains(&"login"));
        assert!(names.contains(&"hash_password"));
        assert!(names.contains(&"with_user"));
    }

    #[cfg(unix)]
    #[test]
    fn framework_aware_structure_map_boosts_canonical_paths() {
        // Build a fake Next.js layout where a test file has MORE symbols than
        // the canonical entry — without framework boost it would dominate the
        // top list; with boost the canonical `app/page.tsx` must float up.
        let d = tempdir_lite::Dir::new();
        std::fs::create_dir_all(d.path().join("app")).unwrap();
        std::fs::create_dir_all(d.path().join("__tests__")).unwrap();
        // Canonical entry — 1 symbol.
        write_at(
            &d.path().join("app"),
            "page.tsx",
            "export default function Page() {}",
        );
        // Test file — 10 symbols (would dominate without the boost).
        let mut bigtest = String::new();
        for i in 0..10 {
            let _ = writeln!(bigtest, "function test_helper_{i}() {{}}");
        }
        write_at(&d.path().join("__tests__"), "huge.test.ts", &bigtest);
        write_at(
            d.path(),
            "package.json",
            r#"{"dependencies":{"next":"14"}}"#,
        );

        let idx = Index::build(d.path()).unwrap();
        let fw = crate::framework::Framework::NextJs;
        let map = idx.structure_map_with_framework(fw);
        // Canonical file present.
        assert!(map.contains("app/page.tsx"), "canonical missing:\n{map}");
        // Star marker present.
        assert!(map.contains('★'), "canonical not marked:\n{map}");
        // Framework label present.
        assert!(map.contains("framework: next.js"));
        // Canonical should appear BEFORE the test file in the top list.
        let p_app = map.find("app/page.tsx").unwrap();
        let p_test = map.find("huge.test.ts").unwrap();
        assert!(p_app < p_test, "canonical should rank before test file");
    }

    #[test]
    #[cfg_attr(
        target_os = "windows",
        ignore = "relies on strict mtime ordering after a 1.1s sleep; some Windows filesystems (e.g. VBox shared folders) have coarser mtime granularity, landing the rewrite in the same bucket. Real behavior is exercised on Linux + macOS."
    )]
    fn incremental_rebuild_picks_up_only_changed_file() {
        let d = tempdir_lite::Dir::new();
        write_at(d.path(), "a.rs", "pub fn alpha() {}");
        write_at(d.path(), "b.rs", "pub fn beta() {}");
        let idx1 = Index::build(d.path()).unwrap();
        assert_eq!(idx1.files_indexed, 2);
        assert!(idx1.symbols.iter().any(|s| s.name == "alpha"));

        // Sleep ≥1s so the second file's mtime is strictly greater (most
        // filesystems have 1s mtime granularity for `modified()`).
        std::thread::sleep(std::time::Duration::from_millis(1100));
        write_at(d.path(), "a.rs", "pub fn alpha() {}\npub fn alpha_v2() {}");

        let idx2 = Index::rebuild_incremental(&idx1, d.path()).unwrap();
        assert_eq!(idx2.files_indexed, 2);
        assert!(
            idx2.symbols.iter().any(|s| s.name == "alpha_v2"),
            "new symbol from changed file missing"
        );
        assert!(
            idx2.symbols.iter().any(|s| s.name == "beta"),
            "unchanged file's symbol must survive"
        );
    }

    #[test]
    fn incremental_rebuild_detects_same_second_edit() {
        // `isolated_repo` (ENV_LOCK + its own db) for the reason it exists: since
        // the call graph moved to its own column an incremental rebuild
        // that sees a change opens the index db, so a sibling test pointing
        // `TOKENADE_INDEX_DB` at a temp dir it is about to delete makes this
        // one fail. Measured: 1 run in 3.
        // Regression: with a seconds-granularity mtime compared `<=`, an edit
        // landing in the SAME wall-clock second as the initial build was
        // silently skipped (stale index) — it surfaced as a Windows-only test
        // failure. The change fingerprint folds in file size, so a content edit
        // is detected with NO sleep, deterministically on every OS/filesystem.
        let (d, _env) = isolated_repo();
        write_at(d.path(), "a.rs", "pub fn alpha() {}");
        let idx1 = Index::build(d.path()).unwrap();
        assert!(idx1.symbols.iter().any(|s| s.name == "alpha"));

        // No sleep — the rewrite almost certainly shares the build's mtime bucket.
        write_at(
            d.path(),
            "a.rs",
            "pub fn alpha() {}\npub fn alpha_same_second() {}",
        );
        let idx2 = Index::rebuild_incremental(&idx1, d.path()).unwrap();
        assert!(
            idx2.symbols.iter().any(|s| s.name == "alpha_same_second"),
            "same-second edit missed — incremental fingerprint regressed"
        );
    }

    #[test]
    fn incremental_rebuild_detects_mtime_moved_backward() {
        // `isolated_repo` (ENV_LOCK + its own db) for the reason it exists: since
        // the call graph moved to its own column an incremental rebuild
        // that sees a change opens the index db, so a sibling test pointing
        // `TOKENADE_INDEX_DB` at a temp dir it is about to delete makes this
        // one fail. Measured: 1 run in 3.
        // A `mtime <= prev` test also wrongly skipped a checkout that restores
        // an OLDER revision (mtime moves backward). Inequality on the fingerprint
        // re-indexes it. Simulated by rebuilding after replacing content whose
        // size differs — fingerprint differs regardless of mtime direction.
        let (d, _env) = isolated_repo();
        write_at(d.path(), "a.rs", "pub fn newer_longer_symbol_name() {}");
        let idx1 = Index::build(d.path()).unwrap();
        write_at(d.path(), "a.rs", "pub fn old() {}");
        let idx2 = Index::rebuild_incremental(&idx1, d.path()).unwrap();
        assert!(idx2.symbols.iter().any(|s| s.name == "old"));
        assert!(!idx2
            .symbols
            .iter()
            .any(|s| s.name == "newer_longer_symbol_name"));
    }

    #[test]
    fn incremental_rebuild_drops_deleted_files() {
        // Holds ENV_LOCK for the same reason `isolated_repo` takes it: since
        // the call graph moved to its own column an incremental rebuild
        // that sees a change opens the index db, so a sibling test pointing
        // `TOKENADE_INDEX_DB` at a temp dir it is about to delete makes this
        // one fail. Measured: 1 run in 3.
        let _env = crate::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let d = tempdir_lite::Dir::new();
        write_at(d.path(), "keep.rs", "pub fn kept() {}");
        write_at(d.path(), "gone.rs", "pub fn doomed() {}");
        let idx1 = Index::build(d.path()).unwrap();
        assert!(idx1.symbols.iter().any(|s| s.name == "doomed"));

        std::fs::remove_file(d.path().join("gone.rs")).unwrap();
        let idx2 = Index::rebuild_incremental(&idx1, d.path()).unwrap();
        assert!(!idx2.symbols.iter().any(|s| s.name == "doomed"));
        assert!(idx2.symbols.iter().any(|s| s.name == "kept"));
        assert_eq!(idx2.files_indexed, 1);
    }

    #[test]
    fn incremental_falls_back_to_full_when_prev_empty() {
        let d = tempdir_lite::Dir::new();
        write_at(d.path(), "a.rs", "pub fn a() {}");
        let empty = Index::default();
        let idx = Index::rebuild_incremental(&empty, d.path()).unwrap();
        assert!(idx.symbols.iter().any(|s| s.name == "a"));
    }

    #[test]
    fn parallel_build_produces_complete_symbol_set() {
        // 200 small Rust files with one function each → the parallel build
        // must capture all 200 symbols, no race-condition drops.
        let d = tempdir_lite::Dir::new();
        for i in 0..200 {
            write_at(
                d.path(),
                &format!("f{i}.rs"),
                &format!("pub fn fn_{i}() {{}}"),
            );
        }
        let idx = Index::build(d.path()).unwrap();
        assert_eq!(idx.files_indexed, 200);
        let names: std::collections::HashSet<&str> =
            idx.symbols.iter().map(|s| s.name.as_str()).collect();
        for i in 0..200 {
            assert!(names.contains(format!("fn_{i}").as_str()), "missing fn_{i}");
        }
    }

    #[test]
    fn incremental_serialises_mtimes_to_disk() {
        let d = tempdir_lite::Dir::new();
        write_at(d.path(), "a.rs", "pub fn a() {}");
        let idx = Index::build(d.path()).unwrap();
        let s = serde_json::to_string(&idx).unwrap();
        let back: Index = serde_json::from_str(&s).unwrap();
        assert!(!back.mtimes.is_empty(), "mtimes must persist across save");
    }

    #[test]
    fn structure_map_unchanged_when_framework_unknown() {
        let d = tmp_repo();
        let idx = Index::build(d.path()).unwrap();
        let plain = idx.structure_map();
        let fw_aware = idx.structure_map_with_framework(crate::framework::Framework::Unknown);
        assert_eq!(plain, fw_aware);
    }

    #[test]
    fn a_hard_skip_holds_whatever_the_repository_says() {
        for n in ["node_modules", "__pycache__", "Pods", ".git"] {
            assert!(should_skip_dir(n, true), "{n} entered");
            assert!(should_skip_dir(n, false), "{n} entered");
        }
    }

    #[test]
    fn a_build_name_is_skipped_only_when_nothing_else_can_decide() {
        // With no ignore rule reaching here, the name is all we have.
        for n in [
            "dist",
            "build",
            "out",
            "target",
            "bin",
            "vendor",
            "benchmarks",
        ] {
            assert!(should_skip_dir(n, false), "{n} entered with no ignore info");
            assert!(
                !should_skip_dir(n, true),
                "{n} skipped although the repository declares its own excludes"
            );
        }
    }

    #[test]
    fn a_dot_directory_is_shape_evidence_not_a_verdict() {
        // With nothing else to go on, a dot directory is skipped…
        assert!(should_skip_dir(".github", false));
        assert!(should_skip_dir(".claude", false));
        // …but a repository that declares its excludes has already said what
        // is generated, and `.github` is where its CI source lives.
        assert!(!should_skip_dir(".github", true));
        assert!(!should_skip_dir(".claude", true));
    }

    #[test]
    fn a_credential_store_is_never_entered() {
        // These are HARD so an index rooted at a HOME directory cannot walk
        // them. None of them holds a tracked source file anywhere in the
        // corpus, so the rule costs nothing.
        for n in [".ssh", ".gnupg", ".aws", ".kube", ".npm", ".cache", ".venv"] {
            assert!(should_skip_dir(n, true), "{n} entered");
        }
        // `.config` is not one of them: 5 corpus repositories track source
        // under it.
        assert!(!should_skip_dir(".config", true));
    }

    #[test]
    fn the_name_set_still_covers_both_lists() {
        assert!(is_skip_dir_name("node_modules"));
        assert!(is_skip_dir_name("dist"));
        assert!(!is_skip_dir_name("src"));
    }

    /// End to end: a tracked `bin/` script is indexed when the repository has
    /// a `.gitignore`, and not when it has none. 388 such files across 58 of
    /// the 300 corpus repositories were invisible.
    #[test]
    fn a_bin_script_is_indexed_when_the_repo_declares_its_excludes() {
        let d = tempdir_lite::Dir::new();
        std::fs::create_dir_all(d.path().join("bin")).unwrap();
        write_at(
            d.subdir("bin").path(),
            "tool.py",
            "def tool_entry():\n    pass\n",
        );
        std::fs::write(d.path().join(".gitignore"), "*.log\n").unwrap();
        let idx = Index::build(d.path()).unwrap();
        assert!(
            idx.symbols.iter().any(|s| s.name == "tool_entry"),
            "bin/ still hidden"
        );

        let e = tempdir_lite::Dir::new();
        std::fs::create_dir_all(e.path().join("bin")).unwrap();
        write_at(
            e.subdir("bin").path(),
            "tool.py",
            "def tool_entry():\n    pass\n",
        );
        let idx2 = Index::build(e.path()).unwrap();
        assert!(
            !idx2.symbols.iter().any(|s| s.name == "tool_entry"),
            "with no ignore information the conservative skip must hold"
        );
    }

    /// The INCREMENTAL walk carries the same decision, and it is the one that
    /// runs on every invocation after the first. A guard on `build` alone left
    /// it free to diverge: a repository indexed correctly once would lose
    /// `bin/` again on its next rebuild.
    #[test]
    fn the_incremental_walk_makes_the_same_skip_decision() {
        // `isolated_repo` and not a bare tempdir: the incremental path reads
        // the GLOBAL index db, and under the parallel test runner several
        // tests then share one SQLite file. Written without it, this test
        // failed about one full-suite run in five with `index db: disk I/O
        // error` — a flake introduced with the test, in the test.
        let (d, _env) = isolated_repo();
        std::fs::create_dir_all(d.path().join("bin")).unwrap();
        write_at(
            d.subdir("bin").path(),
            "tool.py",
            "def tool_entry():\n    pass\n",
        );
        std::fs::write(d.path().join(".gitignore"), "*.log\n").unwrap();
        let first = Index::build(d.path()).unwrap();
        assert!(first.symbols.iter().any(|s| s.name == "tool_entry"));

        // A rebuild whose PREVIOUS index knows some other file — so the
        // incremental walk really runs rather than falling back to a full
        // build — must still reach `bin/`.
        write_at(d.path(), "root.py", "def root_entry():\n    pass\n");
        let mut prev = Index::build(d.path()).unwrap();
        prev.symbols.retain(|s| s.name != "tool_entry");
        prev.mtimes.remove("bin/tool.py");
        let again = Index::rebuild_incremental(&prev, d.path()).unwrap();
        assert!(
            again.symbols.iter().any(|s| s.name == "tool_entry"),
            "the incremental walk skipped bin/"
        );
    }

    /// And the repository's own word still excludes: a `.gitignore` that names
    /// `dist/` keeps it out, by the RIGHT rule.
    #[test]
    fn a_declared_build_dir_is_still_excluded() {
        let d = tempdir_lite::Dir::new();
        std::fs::create_dir_all(d.path().join("dist")).unwrap();
        write_at(
            d.subdir("dist").path(),
            "bundle.py",
            "def generated_thing():\n    pass\n",
        );
        std::fs::write(d.path().join(".gitignore"), "dist/\n").unwrap();
        let idx = Index::build(d.path()).unwrap();
        assert!(!idx.symbols.iter().any(|s| s.name == "generated_thing"));
    }

    #[test]
    fn skip_dirs_include_new_language_caches() {
        // bin/obj (C#), _build/deps (Elixir), .bundle (Ruby), .build (Swift)
        // must all be skipped so the index doesn't pick up vendored deps.
        let d = tempdir_lite::Dir::new();
        std::fs::create_dir_all(d.path().join("bin")).unwrap();
        std::fs::create_dir_all(d.path().join("obj")).unwrap();
        std::fs::create_dir_all(d.path().join("_build")).unwrap();
        std::fs::create_dir_all(d.path().join("deps")).unwrap();
        std::fs::create_dir_all(d.path().join(".bundle")).unwrap();
        std::fs::create_dir_all(d.path().join(".build")).unwrap();
        // Plant a fake symbol inside each so we'd detect leakage.
        write_at(d.subdir("bin").path(), "Stale.cs", "public class Stale {}");
        write_at(
            d.subdir("obj").path(),
            "Garbage.cs",
            "public class Garbage {}",
        );
        write_at(
            d.subdir("_build").path(),
            "Compiled.ex",
            "defmodule Compiled do\nend",
        );
        write_at(
            d.subdir("deps").path(),
            "Phoenix.ex",
            "defmodule Phoenix do\nend",
        );
        write_at(
            d.subdir(".bundle").path(),
            "GemStub.rb",
            "class GemStub\nend",
        );
        write_at(d.subdir(".build").path(), "Cached.swift", "class Cached {}");
        let idx = Index::build(d.path()).unwrap();
        let names: Vec<&str> = idx.symbols.iter().map(|s| s.name.as_str()).collect();
        for forbidden in [
            "Stale", "Garbage", "Compiled", "Phoenix", "GemStub", "Cached",
        ] {
            assert!(
                !names.contains(&forbidden),
                "SKIP_DIRS leak: indexed {forbidden}"
            );
        }
    }

    // #755 — lock-in tests for the 9 LangSpecs added in #744-#754
    // (R, Julia, Clojure, Fish, Erlang, Tcl, Vim, Batch, VB).
    // One assertion per spec verifies (a) the extension is dispatched
    // to lang_for, (b) at least one canonical declaration is captured,
    // and (c) the import pattern fires when applicable. Catches
    // regressions if the regex is broken by a future edit.

    #[test]
    fn r_functions_and_imports_indexed() {
        let d = tempdir_lite::Dir::new();
        write(
            &d,
            "stats.R",
            "library(dplyr)\nmean_val <- function(x) {\n  mean(x)\n}",
        );
        let idx = Index::build(d.path()).unwrap();
        let names: Vec<&str> = idx.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"mean_val"), "R fn missing: {names:?}");
        let imports = idx.imports.get("stats.R").expect("imports for stats.R");
        assert!(imports.contains(&"dplyr".to_string()));
    }

    #[test]
    fn julia_functions_and_modules_indexed() {
        let d = tempdir_lite::Dir::new();
        write(
            &d,
            "stats.jl",
            "using Statistics\nmodule MyStats\n  function compute(x)\n    mean(x)\n  end\nend",
        );
        let idx = Index::build(d.path()).unwrap();
        let names: Vec<&str> = idx.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"compute"), "Julia fn missing: {names:?}");
        assert!(
            names.contains(&"MyStats"),
            "Julia module missing: {names:?}"
        );
        let imports = idx.imports.get("stats.jl").expect("imports for stats.jl");
        assert!(imports.contains(&"Statistics".to_string()));
    }

    #[test]
    fn clojure_defn_and_ns_indexed() {
        let d = tempdir_lite::Dir::new();
        write(
            &d,
            "core.clj",
            "(ns myapp.core\n  (:require [clojure.string :as str]))\n\
             (defn add [a b] (+ a b))\n(defmacro when-some [x] x)\n",
        );
        let idx = Index::build(d.path()).unwrap();
        let names: Vec<&str> = idx.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"add"), "Clojure defn missing: {names:?}");
        assert!(
            names.contains(&"when-some"),
            "Clojure defmacro missing: {names:?}"
        );
        assert!(
            names.contains(&"myapp.core"),
            "Clojure ns missing: {names:?}"
        );
        let imports = idx.imports.get("core.clj").expect("imports for core.clj");
        assert!(imports.contains(&"clojure.string".to_string()));
    }

    #[test]
    fn fish_functions_indexed() {
        let d = tempdir_lite::Dir::new();
        write(
            &d,
            "prompt.fish",
            "source ~/.config/fish/aliases.fish\nfunction fish_prompt --description 'Prompt'\n  echo '$ '\nend",
        );
        let idx = Index::build(d.path()).unwrap();
        let names: Vec<&str> = idx.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"fish_prompt"), "Fish fn missing: {names:?}");
    }

    #[test]
    fn erlang_functions_and_module_indexed() {
        let d = tempdir_lite::Dir::new();
        write(
            &d,
            "server.erl",
            "-module(server).\n-import(lists, [map/2]).\nstart() -> ok.\nhandle_call(_, _, S) -> {reply, ok, S}.",
        );
        let idx = Index::build(d.path()).unwrap();
        let names: Vec<&str> = idx.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"start"), "Erlang fn missing: {names:?}");
        assert!(
            names.contains(&"server"),
            "Erlang module missing: {names:?}"
        );
        let imports = idx
            .imports
            .get("server.erl")
            .expect("imports for server.erl");
        assert!(imports.contains(&"lists".to_string()));
    }

    #[test]
    fn tcl_procs_and_namespace_indexed() {
        let d = tempdir_lite::Dir::new();
        write(
            &d,
            "lib.tcl",
            "package require Tk\nnamespace eval mylib {\n}\nproc mylib::do_thing {x} {\n  return $x\n}",
        );
        let idx = Index::build(d.path()).unwrap();
        let names: Vec<&str> = idx.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"mylib::do_thing"),
            "Tcl proc missing: {names:?}"
        );
        assert!(names.contains(&"mylib"), "Tcl namespace missing: {names:?}");
        let imports = idx.imports.get("lib.tcl").expect("imports for lib.tcl");
        assert!(imports.contains(&"Tk".to_string()));
    }

    #[test]
    fn vim_functions_and_commands_indexed() {
        let d = tempdir_lite::Dir::new();
        write(
            &d,
            "plugin.vim",
            "runtime macros/matchit.vim\nfunction! myplug#init(arg)\n  echo a:arg\nendfunction\ncommand! -nargs=0 MyPlugDo call myplug#init('x')",
        );
        let idx = Index::build(d.path()).unwrap();
        let names: Vec<&str> = idx.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"myplug#init"), "Vim fn missing: {names:?}");
        assert!(
            names.contains(&"MyPlugDo"),
            "Vim command missing: {names:?}"
        );
    }

    #[test]
    fn batch_labels_indexed() {
        let d = tempdir_lite::Dir::new();
        write(
            &d,
            "deploy.bat",
            "@echo off\ncall helper.bat\ncall :Setup\ngoto :EOF\n\n:Setup\necho hi\ngoto :EOF\n\n:Cleanup\necho bye\ngoto :EOF",
        );
        let idx = Index::build(d.path()).unwrap();
        let names: Vec<&str> = idx.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Setup"), "Batch label missing: {names:?}");
        assert!(names.contains(&"Cleanup"), "Batch label missing: {names:?}");
        assert!(
            !names.contains(&"EOF"),
            "Reserved :EOF should be excluded: {names:?}"
        );
        let imports = idx
            .imports
            .get("deploy.bat")
            .expect("imports for deploy.bat");
        assert!(imports.iter().any(|s| s.ends_with("helper.bat")));
    }

    #[test]
    fn vb_functions_classes_modules_indexed() {
        let d = tempdir_lite::Dir::new();
        write(
            &d,
            "App.vb",
            "Imports System.IO\nPublic Module MyMod\n  Public Function Compute(x As Integer) As Integer\n    Return x * 2\n  End Function\n  Private Sub Helper()\n  End Sub\nEnd Module\nPublic Class Widget\nEnd Class",
        );
        let idx = Index::build(d.path()).unwrap();
        let names: Vec<&str> = idx.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Compute"), "VB function missing: {names:?}");
        assert!(names.contains(&"Helper"), "VB sub missing: {names:?}");
        assert!(names.contains(&"Widget"), "VB class missing: {names:?}");
        assert!(names.contains(&"MyMod"), "VB module missing: {names:?}");
        let imports = idx.imports.get("App.vb").expect("imports for App.vb");
        assert!(imports.contains(&"System.IO".to_string()));
    }
}
#[cfg(test)]
mod tempdir_lite {
    use std::path::{Path, PathBuf};
    pub struct Dir(PathBuf);
    impl Dir {
        /// A *child* directory under this Dir. The returned wrapper has no
        /// Drop side effects (the parent's Drop cleans up the whole tree),
        /// so it's safe to create many subdirs in a single test.
        pub fn subdir(&self, name: &str) -> ChildDir {
            ChildDir(self.0.join(name))
        }
        pub fn new() -> Self {
            // Uniqueness must NOT rest on the clock alone. `SystemTime` has
            // coarse granularity on some platforms (~15 ms on Windows), so two
            // tests running in parallel can read the same nanos — and
            // `create_dir_all` succeeds on an already-existing path, so the
            // collision is silent: both tests share one directory and the first
            // to drop deletes the other's files mid-test. A process-wide
            // counter makes the name unique by construction, and `create_dir`
            // turns any residual collision into a loud failure, not a flake.
            static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let base = std::env::temp_dir().join(format!(
                "tokenade-test-{}-{}-{seq}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir(&base).expect("test tempdir name must be unique");
            Dir(base)
        }
        pub fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A lightweight child-path handle for tests that needs to write into
    /// nested subdirs. No Drop: the parent Dir owns the cleanup.
    pub struct ChildDir(PathBuf);
    impl ChildDir {
        pub fn path(&self) -> &Path {
            &self.0
        }
    }
}
#[cfg(test)]
mod empty_rebuild_guard {
    use super::*;

    fn scratch(label: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "tokenade-emptyidx-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// A root that has gone away must not empty an index that held files.
    ///
    /// The walk skips an unreadable directory, so a vanished root produces an
    /// empty `current`, every known file is classified DELETED, and the
    /// rebuild succeeds holding nothing. `query` then answers "not found"
    /// about symbols that exist — false absence, reached through the index.
    #[test]
    fn a_vanished_root_does_not_empty_an_index_that_held_files() {
        let root = scratch("vanished");
        fs::write(root.join("a.rs"), "pub fn alpha() {}\npub fn beta() {}\n").unwrap();
        let first = Index::build(&root).expect("first build");
        assert!(first.files_indexed > 0, "the fixture indexed nothing");
        assert!(!first.find("alpha", 10).is_empty());

        // The root goes away between runs.
        fs::remove_dir_all(&root).unwrap();
        let err = Index::rebuild_incremental(&first, &root)
            .expect_err("an empty walk over a non-empty index must refuse");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        assert!(
            err.to_string().contains("refusing to replace an index"),
            "the refusal does not say what it protected: {err}"
        );
    }

    /// …and a root that is genuinely there but now empty is a different fact.
    /// Nothing here should refuse: the previous index simply drains.
    #[test]
    fn a_root_emptied_of_its_files_still_rebuilds_to_nothing() {
        let root = scratch("emptied");
        fs::write(root.join("a.rs"), "pub fn alpha() {}\n").unwrap();
        let first = Index::build(&root).expect("first build");
        assert!(first.files_indexed > 0);

        fs::remove_file(root.join("a.rs")).unwrap();
        let second = Index::rebuild_incremental(&first, &root)
            .expect("an existing but empty root is a legitimate rebuild");
        assert_eq!(second.files_indexed, 0);
        assert!(second.find("alpha", 10).is_empty());
    }

    /// A first build over a missing root is not this class: there is no index
    /// to protect, and the caller wants the honest empty.
    #[test]
    fn a_first_build_over_a_missing_root_is_left_alone() {
        let root = scratch("first");
        let empty = Index::build(&root).expect("empty root builds");
        assert_eq!(empty.files_indexed, 0);
        fs::remove_dir_all(&root).unwrap();
        let again = Index::rebuild_incremental(&empty, &root);
        assert!(again.is_ok(), "nothing to protect, so nothing to refuse");
    }

    // ---- impl blocks carry the trait they implement -----------------------

    fn tmp_rust(body: &str) -> tempdir_lite::Dir {
        let d = tempdir_lite::Dir::new();
        fs::create_dir_all(d.path().join("src")).unwrap();
        fs::write(d.path().join("src").join("lib.rs"), body).unwrap();
        d
    }

    /// `impl <Trait> for <Type>` reads as Rust writes it. Six impls on one
    /// type printed the same six words before; the trait is what tells them
    /// apart.
    #[test]
    fn an_impl_block_names_the_trait_it_implements() {
        let d = tmp_rust(
            "pub struct Foo;\n\
             impl std::fmt::Display for Foo { }\n\
             impl From<u8> for Foo { }\n",
        );
        let idx = Index::build(d.path()).unwrap();
        let labels: Vec<String> = idx
            .symbols
            .iter()
            .filter(|s| s.kind == "impl")
            .map(Symbol::label)
            .collect();
        assert!(
            labels.contains(&"impl std::fmt::Display for Foo".to_string()),
            "got {labels:?}"
        );
        assert!(
            labels.contains(&"impl From<u8> for Foo".to_string()),
            "got {labels:?}"
        );
    }

    /// An inherent `impl Foo { … }` implements no trait and must not grow a
    /// `for` clause out of whatever happens to be first in `bases`.
    #[test]
    fn an_inherent_impl_keeps_the_plain_label() {
        let d = tmp_rust("pub struct Foo;\nimpl Foo { pub fn new() -> Self { Foo } }\n");
        let idx = Index::build(d.path()).unwrap();
        let labels: Vec<String> = idx
            .symbols
            .iter()
            .filter(|s| s.kind == "impl")
            .map(Symbol::label)
            .collect();
        assert_eq!(labels, vec!["impl Foo".to_string()], "got {labels:?}");
    }

    /// The trait goes in `bases`, NEVER in `name`.
    ///
    /// Naming the impl after the trait is the defect the extractor's own
    /// comment records: `impl Drop for StashGuard` indexed as a declaration
    /// of `Drop` answered "where is Drop defined?" with every file that
    /// implements it. `bases` is match tier 2 — below both name tiers — so
    /// the declaration still wins.
    #[test]
    fn a_trait_declaration_outranks_its_implementors() {
        let d = tempdir_lite::Dir::new();
        fs::create_dir_all(d.path().join("src")).unwrap();
        fs::write(
            d.path().join("src").join("api.rs"),
            "pub trait Sink { fn put(&self); }\n",
        )
        .unwrap();
        fs::write(
            d.path().join("src").join("use.rs"),
            "pub struct A;\npub struct B;\n\
             impl Sink for A { fn put(&self) {} }\n\
             impl Sink for B { fn put(&self) {} }\n",
        )
        .unwrap();
        let idx = Index::build(d.path()).unwrap();
        let hits = idx.find("sink", 10);
        assert!(!hits.is_empty(), "no hits");
        assert_eq!(hits[0].name, "Sink", "the declaration must rank first");
        assert_eq!(hits[0].kind, "type", "got {:?}", hits[0]);
        // …and the implementors are reachable at all, which they were not.
        let impls: Vec<&str> = hits
            .iter()
            .filter(|s| s.kind == "impl")
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(impls, vec!["A", "B"], "got {impls:?}");
        // The impl's own NAME is still the type it extends.
        for s in idx.symbols.iter().filter(|s| s.kind == "impl") {
            assert_ne!(s.name, "Sink", "the trait leaked into the name");
        }
    }

    /// A Go method reads with the type it hangs off.
    ///
    /// Measured on a real 316-file Go project: `Name` named 42 methods,
    /// `Description` 41, `Execute` 34, and `query Execute` printed 34 lines
    /// that differed only by path. Same-file duplicates among methods: 85
    /// before, 0 after.
    #[test]
    fn a_go_method_carries_its_receiver() {
        let d = tempdir_lite::Dir::new();
        fs::create_dir_all(d.path().join("src")).unwrap();
        fs::write(
            d.path().join("src").join("t.go"),
            "package t\ntype Task struct{}\n\
             func (t *Task) Run() {}\n\
             func (t Task) Name() string { return \"\" }\n\
             func Run() {}\n",
        )
        .unwrap();
        let idx = Index::build(d.path()).unwrap();
        let labels: Vec<String> = idx.symbols.iter().map(Symbol::label).collect();
        assert!(
            labels.contains(&"method *Task.Run".to_string()),
            "got {labels:?}"
        );
        assert!(
            labels.contains(&"method Task.Name".to_string()),
            "got {labels:?}"
        );
        // A plain function has no receiver and must stay plain.
        assert!(labels.contains(&"fn Run".to_string()), "got {labels:?}");
    }

    /// The receiver's BINDING name is noise: `(t *Task)` is how Go writes the
    /// parameter, `*Task` is the thing that tells two `Run`s apart.
    #[test]
    fn a_go_receiver_drops_the_binding_name() {
        let d = tempdir_lite::Dir::new();
        fs::create_dir_all(d.path().join("src")).unwrap();
        fs::write(
            d.path().join("src").join("t.go"),
            "package t\ntype Task struct{}\nfunc (receiverName *Task) Run() {}\n",
        )
        .unwrap();
        let idx = Index::build(d.path()).unwrap();
        let m = idx
            .symbols
            .iter()
            .find(|s| s.kind == "method")
            .expect("method not indexed");
        assert_eq!(m.owner, "*Task", "got {:?}", m.owner);
        assert!(
            m.bases.is_empty(),
            "a receiver is not a supertype: {:?}",
            m.bases
        );
        assert!(
            !m.label().contains("receiverName"),
            "the binding name leaked: {}",
            m.label()
        );
    }

    /// Java and Kotlin share Go's `method_declaration` node kind and have no
    /// receiver field. The extraction carries no language test — this is what
    /// says that is safe.
    ///
    /// It asserted "no owner at all" for one iteration, which was the
    /// receiver question wearing the owner's clothes: a Java method DOES hang
    /// off its class, and the parent chain finds it. What must not happen is
    /// the receiver branch firing, which would put `(T)` in parentheses as
    /// though Java wrote a receiver.
    #[test]
    fn a_java_method_takes_its_class_and_never_a_receiver() {
        let d = tempdir_lite::Dir::new();
        fs::create_dir_all(d.path().join("src")).unwrap();
        fs::write(
            d.path().join("src").join("T.java"),
            "class T {\n  public void run() {}\n}\n",
        )
        .unwrap();
        let idx = Index::build(d.path()).unwrap();
        let m = idx
            .symbols
            .iter()
            .find(|s| s.name == "run")
            .expect("method not indexed");
        assert_eq!(m.owner, "T", "got {:?}", m.owner);
        assert_eq!(m.label(), "method T.run", "got {:?}", m.label());
    }

    /// The receiver reaches `find`'s tier 2, so asking for a type name brings
    /// back the type AND what hangs off it — with the declaration still first.
    #[test]
    fn a_type_name_reaches_the_methods_that_hang_off_it() {
        let d = tempdir_lite::Dir::new();
        fs::create_dir_all(d.path().join("src")).unwrap();
        fs::write(
            d.path().join("src").join("t.go"),
            "package t\ntype Ledger struct{}\n\
             func (l *Ledger) Append() {}\n\
             func (l *Ledger) Flush() {}\n",
        )
        .unwrap();
        let idx = Index::build(d.path()).unwrap();
        let hits = idx.find("ledger", 10);
        assert_eq!(hits[0].name, "Ledger", "the declaration must rank first");
        // Order among tier-2 hits is `find`'s ranking, not source order —
        // assert what this test is about, which is that both are reachable.
        let mut methods: Vec<&str> = hits
            .iter()
            .filter(|s| s.kind == "method")
            .map(|s| s.name.as_str())
            .collect();
        methods.sort_unstable();
        assert_eq!(methods, vec!["Append", "Flush"], "got {methods:?}");
    }

    /// A field reads with the type that declares it.
    ///
    /// The biggest of the three owner cases. Measured across the 517 indexed
    /// projects on this machine: 38 126 of 90 188 field symbols (42 %) shared
    /// a bare name with a sibling in their own file. On one Go project the
    /// same-file duplicates went 453 → 0.
    #[test]
    fn a_field_carries_the_type_that_declares_it() {
        let d = tempdir_lite::Dir::new();
        fs::create_dir_all(d.path().join("src")).unwrap();
        fs::write(
            d.path().join("src").join("t.go"),
            "package t\n\
             type Server struct { Timeout int }\n\
             type Client struct { Timeout int }\n",
        )
        .unwrap();
        fs::write(
            d.path().join("src").join("t.rs"),
            "pub struct Server { pub timeout: u8 }\n\
             pub struct Client { pub timeout: u8 }\n",
        )
        .unwrap();
        let idx = Index::build(d.path()).unwrap();
        let labels: Vec<String> = idx
            .symbols
            .iter()
            .filter(|s| s.kind == "field")
            .map(Symbol::label)
            .collect();
        for want in [
            "field Server.Timeout",
            "field Client.Timeout",
            "field Server.timeout",
            "field Client.timeout",
        ] {
            assert!(
                labels.contains(&want.to_string()),
                "{want} missing in {labels:?}"
            );
        }
    }

    /// A caller that WROTE the module is attributed even when the bare name
    /// has several homes.
    ///
    /// `impact_of` gives up on any callee declared in more than one file,
    /// because attributing `new` or `compact` would trade a recall gap for a
    /// precision one. The names that get abandoned are the generic ones —
    /// which is also exactly why people qualify them. Measured A/B on this
    /// repository: `debug_log.rs` went from **1 dependent and 69 abandoned**
    /// to **58 and 12**, `platform.rs` from 26/480 to 68/438.
    #[test]
    fn a_qualified_call_is_attributed_where_the_bare_name_is_ambiguous() {
        let d = tempdir_lite::Dir::new();
        fs::create_dir_all(d.path().join("src")).unwrap();
        // `fold` has two homes, so the bare name says nothing…
        fs::write(d.path().join("src").join("logs.rs"), "pub fn fold() {}\n").unwrap();
        fs::write(d.path().join("src").join("text.rs"), "pub fn fold() {}\n").unwrap();
        fs::write(
            d.path().join("src").join("named.rs"),
            "pub fn go() { crate::logs::fold(); }\n",
        )
        .unwrap();
        fs::write(
            d.path().join("src").join("bare.rs"),
            "pub fn go() { fold(); }\n",
        )
        .unwrap();
        let idx = Index::build(d.path()).unwrap();
        let rep = idx.impact_of("src/logs.rs");
        assert!(
            rep.dependents.iter().any(|f| f.ends_with("named.rs")),
            "the caller that named the module must be attributed: {:?}",
            rep.dependents
        );
        // …and the bare caller is still abandoned, not guessed at.
        assert!(
            !rep.dependents.iter().any(|f| f.ends_with("bare.rs")),
            "a bare ambiguous call must NOT be attributed: {:?}",
            rep.dependents
        );
        assert!(
            rep.ambiguous_callers >= 1,
            "the bare caller must be counted"
        );
    }

    /// Two files with the same stem make `logs::` name neither, and guessing
    /// between them is the precision loss the whole branch exists to avoid.
    #[test]
    fn a_qualifier_shared_by_two_files_attributes_nothing() {
        let d = tempdir_lite::Dir::new();
        fs::create_dir_all(d.path().join("src").join("a")).unwrap();
        fs::create_dir_all(d.path().join("src").join("b")).unwrap();
        fs::write(
            d.path().join("src").join("a").join("logs.rs"),
            "pub fn fold() {}\n",
        )
        .unwrap();
        fs::write(
            d.path().join("src").join("b").join("logs.rs"),
            "pub fn fold() {}\n",
        )
        .unwrap();
        fs::write(
            d.path().join("src").join("named.rs"),
            "pub fn go() { crate::logs::fold(); }\n",
        )
        .unwrap();
        let idx = Index::build(d.path()).unwrap();
        let rep = idx.impact_of("src/a/logs.rs");
        assert!(
            !rep.dependents.iter().any(|f| f.ends_with("named.rs")),
            "an ambiguous stem must attribute nothing: {:?}",
            rep.dependents
        );
        assert!(rep.ambiguous_callers >= 1, "and must still be counted");
    }

    /// The qualifier is the segment written before the callee, whatever the
    /// grammar spells it with.
    #[test]
    fn the_qualifier_is_the_segment_before_the_callee() {
        let d = tempdir_lite::Dir::new();
        fs::create_dir_all(d.path().join("src")).unwrap();
        fs::write(
            d.path().join("src").join("t.rs"),
            "pub fn go() { crate::logs::fold(); helper(); obj.method(); }\n",
        )
        .unwrap();
        let idx = Index::build(d.path()).unwrap();
        let q = |name: &str| {
            idx.calls
                .iter()
                .find(|c| c.callee_name == name)
                .map(|c| c.qualifier.clone())
                .unwrap_or_else(|| format!("<{name} not extracted>"))
        };
        assert_eq!(q("fold"), "logs");
        assert_eq!(q("helper"), "", "an unqualified call carries nothing");
        assert_eq!(q("method"), "obj", "a receiver is the segment before it");
    }

    /// `impact_of` reports the SAME string `query` prints.
    ///
    /// Its symbol list is `"<kind> <name>"` that the caller splits at the
    /// first space, so it is easy to build by hand and easy to leave behind:
    /// `impact` printed `impl (6): TokenadeError ×6` and `fn (12): …, from
    /// ×4` for a full iteration after `query` had learned better.
    #[test]
    fn impact_reports_the_qualified_name() {
        let d = tempdir_lite::Dir::new();
        fs::create_dir_all(d.path().join("src")).unwrap();
        fs::write(
            d.path().join("src").join("t.rs"),
            "use std::fmt::Display;\n\
             pub struct Server { pub timeout: u8 }\n\
             pub struct Client { pub timeout: u8 }\n\
             impl Display for Server { fn fmt(&self) {} }\n",
        )
        .unwrap();
        let idx = Index::build(d.path()).unwrap();
        let rep = idx.impact_of("src/t.rs");
        assert!(
            rep.symbols.iter().any(|s| s == "field Server.timeout"),
            "got {:?}",
            rep.symbols
        );
        assert!(
            rep.symbols.iter().any(|s| s == "impl Display for Server"),
            "got {:?}",
            rep.symbols
        );
    }

    /// `label` is `kind` + `qualified_name`, in one place.
    ///
    /// `query` prints the whole thing; `impact` groups by kind already and
    /// prints only the tail. Two surfaces, one string — if they were built
    /// separately, `impact` would keep the bare names it had while `query`
    /// showed the owner.
    #[test]
    fn a_label_is_its_kind_and_its_qualified_name() {
        let d = tempdir_lite::Dir::new();
        fs::create_dir_all(d.path().join("src")).unwrap();
        fs::write(
            d.path().join("src").join("t.rs"),
            "use std::fmt::Display;\npub struct Foo { pub bar: u8 }\n\
             impl Display for Foo { fn fmt(&self) {} }\n",
        )
        .unwrap();
        let idx = Index::build(d.path()).unwrap();
        for s in &idx.symbols {
            assert_eq!(
                s.label(),
                format!("{} {}", s.kind, s.qualified_name()),
                "label and qualified_name disagree for {s:?}"
            );
        }
        let quals: Vec<String> = idx.symbols.iter().map(Symbol::qualified_name).collect();
        assert!(quals.contains(&"Foo.bar".to_string()), "got {quals:?}");
        assert!(
            quals.contains(&"Display for Foo".to_string()),
            "got {quals:?}"
        );
    }

    /// A function declared inside an `impl` block or a class hangs off it.
    ///
    /// The longest-missing of the owner cases, because Rust's
    /// `impl Foo { fn new() }` is a `function_item` like any free function.
    /// `agent_jsonl_usage.rs` declared eight `fn agent` and the index printed
    /// eight identical lines; on this repository the fix took the symbols
    /// still sharing a label inside one file from 321 to 225.
    #[test]
    fn a_method_carries_the_type_that_declares_it() {
        let d = tempdir_lite::Dir::new();
        fs::create_dir_all(d.path().join("src")).unwrap();
        fs::write(
            d.path().join("src").join("t.rs"),
            "pub struct Server;\npub struct Client;\n\
             impl Server { pub fn open() {} }\n\
             impl Client { pub fn open() {} }\n\
             pub fn open() {}\n",
        )
        .unwrap();
        fs::write(
            d.path().join("src").join("m.py"),
            "class Job:\n    def run(self):\n        pass\n",
        )
        .unwrap();
        let idx = Index::build(d.path()).unwrap();
        let labels: Vec<String> = idx.symbols.iter().map(Symbol::label).collect();
        for want in ["fn Server.open", "fn Client.open", "fn Job.run"] {
            assert!(
                labels.contains(&want.to_string()),
                "{want} missing in {labels:?}"
            );
        }
        // A free function belongs to nothing and must stay bare.
        assert!(labels.contains(&"fn open".to_string()), "got {labels:?}");
    }

    /// An `impl` block's first identifier is the TRAIT. A method inside
    /// `impl Display for Foo` hangs off `Foo`, and saying `Display.fmt`
    /// would name the wrong thing — the same trap the extractor records
    /// where it decides what to call the block itself.
    #[test]
    fn an_impl_methods_owner_is_the_type_not_the_trait() {
        let d = tempdir_lite::Dir::new();
        fs::create_dir_all(d.path().join("src")).unwrap();
        // A BARE trait name on purpose. `ident_name` scans direct children
        // for the first identifier-shaped node and `scoped_type_identifier`
        // is not one of its kinds, so `std::fmt::Display` is skipped and the
        // fixture would pass without the impl branch existing at all. Most
        // traits are imported and written bare, which is the case that has
        // to be right.
        fs::write(
            d.path().join("src").join("t.rs"),
            "use std::fmt::Display;\npub struct Foo;\n\
             impl Display for Foo { fn fmt(&self) {} }\n",
        )
        .unwrap();
        let idx = Index::build(d.path()).unwrap();
        let m = idx
            .symbols
            .iter()
            .find(|s| s.name == "fmt")
            .expect("method not indexed");
        assert_eq!(m.owner, "Foo", "got {:?}", m.owner);
        assert_eq!(m.label(), "fn Foo.fmt");
    }

    /// A function nested inside a method is a local, not part of the type's
    /// surface, however deep under the `impl` it sits in the tree.
    #[test]
    fn a_nested_function_does_not_borrow_the_enclosing_type() {
        let d = tempdir_lite::Dir::new();
        fs::create_dir_all(d.path().join("src")).unwrap();
        fs::write(
            d.path().join("src").join("t.rs"),
            "pub struct Server;\n\
             impl Server { pub fn run() { fn helper() {} helper(); } }\n",
        )
        .unwrap();
        let idx = Index::build(d.path()).unwrap();
        let h = idx
            .symbols
            .iter()
            .find(|s| s.name == "helper")
            .expect("nested fn not indexed");
        assert!(h.owner.is_empty(), "borrowed an owner: {:?}", h.owner);
    }

    /// An enum variant belongs to its enum the same way.
    #[test]
    fn an_enum_variant_carries_its_enum() {
        let d = tempdir_lite::Dir::new();
        fs::create_dir_all(d.path().join("src")).unwrap();
        fs::write(
            d.path().join("src").join("t.rs"),
            "pub enum State { Open, Shut }\npub enum Door { Open }\n",
        )
        .unwrap();
        let idx = Index::build(d.path()).unwrap();
        let labels: Vec<String> = idx
            .symbols
            .iter()
            .filter(|s| s.kind == "variant")
            .map(Symbol::label)
            .collect();
        assert!(
            labels.contains(&"variant State.Open".to_string()),
            "got {labels:?}"
        );
        assert!(
            labels.contains(&"variant Door.Open".to_string()),
            "got {labels:?}"
        );
    }

    /// A module-scope constant belongs to nothing and must not grow a dot.
    #[test]
    fn a_module_scope_const_has_no_owner() {
        let d = tempdir_lite::Dir::new();
        fs::create_dir_all(d.path().join("src")).unwrap();
        fs::write(
            d.path().join("src").join("t.rs"),
            "pub const LIMIT: u8 = 4;\n",
        )
        .unwrap();
        let idx = Index::build(d.path()).unwrap();
        let c = idx
            .symbols
            .iter()
            .find(|s| s.name == "LIMIT")
            .expect("const not indexed");
        assert!(c.owner.is_empty(), "got {:?}", c.owner);
        assert_eq!(c.label(), "const LIMIT");
    }

    /// A constant inside a method body sits UNDER a type in the parent chain,
    /// and is not a member of it. Without the function stop in `owner_of` it
    /// would come back as `const Server.RETRIES`, which is a false statement
    /// about the type's surface.
    #[test]
    fn a_function_local_const_does_not_borrow_the_enclosing_type() {
        let d = tempdir_lite::Dir::new();
        fs::create_dir_all(d.path().join("src")).unwrap();
        fs::write(
            d.path().join("src").join("t.rs"),
            "pub struct Server;\n\
             impl Server { pub fn run() { const RETRIES: u8 = 3; let _ = RETRIES; } }\n",
        )
        .unwrap();
        let idx = Index::build(d.path()).unwrap();
        let c = idx
            .symbols
            .iter()
            .find(|s| s.name == "RETRIES")
            .expect("fn-local const not indexed — see the locals rule");
        assert!(c.owner.is_empty(), "borrowed an owner: {:?}", c.owner);
    }

    /// The owner reaches `find`'s tier 2, so a type name brings back what it
    /// declares — and the declaration still ranks above it.
    #[test]
    fn a_type_name_reaches_the_fields_it_declares() {
        let d = tempdir_lite::Dir::new();
        fs::create_dir_all(d.path().join("src")).unwrap();
        fs::write(
            d.path().join("src").join("t.go"),
            "package t\ntype Ledger struct { Head int; Tail int }\n",
        )
        .unwrap();
        let idx = Index::build(d.path()).unwrap();
        let hits = idx.find("ledger", 10);
        assert_eq!(hits[0].name, "Ledger", "the declaration must rank first");
        let mut fields: Vec<&str> = hits
            .iter()
            .filter(|s| s.kind == "field")
            .map(|s| s.name.as_str())
            .collect();
        fields.sort_unstable();
        assert_eq!(fields, vec!["Head", "Tail"], "got {fields:?}");
    }

    /// `bases` is not an impl-only field. A Python class carries its bases
    /// too, and must keep reading as a type declaration.
    #[test]
    fn a_class_with_bases_is_not_rendered_as_an_impl() {
        let d = tempdir_lite::Dir::new();
        fs::create_dir_all(d.path().join("src")).unwrap();
        fs::write(
            d.path().join("src").join("m.py"),
            "class ApiError(RuntimeError):\n    pass\n",
        )
        .unwrap();
        let idx = Index::build(d.path()).unwrap();
        let s = idx
            .symbols
            .iter()
            .find(|s| s.name == "ApiError")
            .expect("class not indexed");
        assert_eq!(s.bases, vec!["RuntimeError".to_string()]);
        assert_eq!(s.label(), "type ApiError", "got {:?}", s.label());
    }

    /// The `<kind> <name> — <file>:<line>` line has ONE spelling.
    ///
    /// It had four, and one of them carried a comment asserting it matched
    /// another ("Same em-dash location line render_with_context(0) emits").
    /// A comment claiming two places agree is what a declarer replaces, and
    /// a guard on the fixed sites says nothing about the fifth copy.
    #[test]
    fn every_location_line_asks_the_one_declarer() {
        // The sources are DERIVED from `crates/`, not named one by one. The
        // three `include_str!` from before went red the day the calls moved
        // file — the guard announced "only 2 call site(s) reached" as if the
        // product had lost sites, whereas it was simply looking in the wrong
        // place. And its own comment says why: a guard on FIXED sites says
        // nothing about the fifth copy.
        let sources = crate::source_guard::rust_sources_whole(&crate::source_guard::crates_dir());
        assert!(
            sources.len() > 100,
            "the descent only brought back {} file(s): a guard that reads \
             nothing is green",
            sources.len()
        );
        let mut declarers = 0usize;
        let mut callers = 0usize;
        for (path, src) in &sources {
            let name = path.display();
            for (_, line) in &crate::srcsweep::production_lines(src) {
                let t = line.trim_start();
                if t.starts_with("//") {
                    continue;
                }
                assert!(
                    !t.contains("s.kind, s.name, s.file, s.line"),
                    "{name} spells the location line by hand again"
                );
                if t.contains("fn location_line") {
                    declarers += 1;
                } else if t.contains("location_line()") {
                    callers += 1;
                }
            }
        }
        assert_eq!(
            declarers, 1,
            "the location line is declared {declarers} times"
        );
        assert!(callers >= 4, "only {callers} call site(s) reached");
    }
}
#[cfg(test)]
mod a_test_module_may_be_called_anything {
    use super::Index;

    /// `#[cfg(test)]` is what makes a block not part of the crate — its NAME is
    /// a convention, and one module in five here does not follow it.
    ///
    /// MEASURED on this repository: **296 of 1 491 `#[cfg(test)]` modules
    /// (20%) are named something other than `test`/`tests`**, and every helper
    /// inside them was reported as a declaration of its file. `impact` on
    /// `pem.rs` listed `handshake` — a fixture builder — among the five
    /// functions the file declares, and `mod wiring` as a declared module.
    /// Repo-wide the fix moves **230 symbols** out of "declares" and into
    /// "test declarations", 13 204 → 12 974, with the total unchanged.
    fn tree() -> super::tempdir_lite::Dir {
        let d = super::tempdir_lite::Dir::new();
        let mk = |rel: &str, body: &str| {
            let p = d.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        };
        mk(
            "src/subject.rs",
            "pub fn real_api() {}\n\n\
             #[cfg(test)]\nmod wiring {\n\
             \u{20}   fn zqfixture() -> u8 { 1 }\n\
             \u{20}   #[test]\n    fn zqcase() { assert_eq!(zqfixture(), 1); }\n}\n\n\
             mod tests {\n\
             \u{20}   fn zqplainhelper() {}\n\
             \u{20}   #[test]\n    fn zqnamed() {}\n}\n\n\
             #[cfg(unix)]\npub mod platform_bits {\n    pub fn zqunix() {}\n}\n\n\
             pub mod real_helpers {\n    pub fn zqhelper() {}\n}\n",
        );
        d
    }

    /// `symbols` / `test_symbols` hold `"<kind> <name>"`, so a bare-name
    /// comparison silently matches nothing and every assertion built on one
    /// passes for free.
    fn names(v: &[String]) -> Vec<&str> {
        v.iter()
            .map(|s| s.rsplit(' ').next().unwrap_or(s.as_str()))
            .collect()
    }

    #[test]
    fn a_cfg_test_module_is_a_test_module_whatever_it_is_called() {
        let d = tree();
        let idx = Index::build(d.path()).unwrap();
        let rep = idx.impact_of("src/subject.rs");
        for gone in ["zqfixture", "wiring"] {
            assert!(
                !names(&rep.symbols).contains(&gone),
                "{gone} is declared only under cfg(test) and is still listed as API: {:?}",
                rep.symbols
            );
            assert!(
                names(&rep.test_symbols).contains(&gone),
                "{gone} vanished instead of moving to the test list: {:?}",
                rep.test_symbols
            );
        }
        // The NAME path, exercised on a `mod tests` carrying no attribute —
        // which is how every language without one spells it, and the only way
        // this arm is reached now that the attribute answers first.
        assert!(
            !names(&rep.symbols).contains(&"zqnamed"),
            "a `mod tests` member came back as API: {:?}",
            rep.symbols
        );
        // `zqnamed` carries `#[test]` and is caught by its OWN attribute, so it
        // says nothing about the module rule. `zqplainhelper` carries nothing:
        // only "a module called tests is a test module" can exclude it, and
        // dropping that clause survived every other assertion here.
        assert!(
            !names(&rep.symbols).contains(&"zqplainhelper"),
            "a plain helper inside `mod tests` came back as API — the name path \
             is gone: {:?}",
            rep.symbols
        );
    }

    /// Anti-vacuity: "call every module a test module" must not pass. A plain
    /// `mod` carrying no attribute is part of what the file offers.
    #[test]
    fn a_module_without_the_attribute_is_still_declared() {
        let d = tree();
        let idx = Index::build(d.path()).unwrap();
        let rep = idx.impact_of("src/subject.rs");
        // `zqunix` is the one that matters: its module carries an attribute,
        // and a predicate matching `cfg(` rather than `cfg(test)` would take
        // every platform-gated module for a test module.
        for kept in [
            "real_api",
            "real_helpers",
            "zqhelper",
            "platform_bits",
            "zqunix",
        ] {
            assert!(
                names(&rep.symbols).contains(&kept),
                "{kept} is ordinary code and stopped being declared: {:?}",
                rep.symbols
            );
        }
    }

    /// The harm was never only cosmetic: `impact_of` builds its ambiguity map
    /// from these names, so a test helper sharing a real name made that name
    /// look as though it had two homes, and attribution was abandoned for it.
    #[test]
    fn a_test_helper_no_longer_makes_a_real_name_look_ambiguous() {
        let d = super::tempdir_lite::Dir::new();
        let mk = |rel: &str, body: &str| {
            let p = d.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        };
        // The real home of `zqshared`.
        mk("src/home.rs", "pub fn zqshared() {}\n");
        // A test helper in ANOTHER file with the same name, under a module
        // whose name is not `tests`.
        mk(
            "src/other.rs",
            "pub fn unrelated() {}\n\n\
             #[cfg(test)]\nmod scaffolding {\n    fn zqshared() {}\n\
             \u{20}   #[test]\n    fn t() { zqshared(); }\n}\n",
        );
        // A caller that names the module, so attribution has something to bind.
        mk(
            "src/caller.rs",
            "pub fn go() {\n    crate::home::zqshared();\n}\n",
        );
        let idx = Index::build(d.path()).unwrap();
        let rep = idx.impact_of("src/home.rs");
        assert!(
            rep.dependents.iter().any(|f| f == "src/caller.rs"),
            "a cfg(test) helper elsewhere still makes the real name ambiguous: \
             dependents {:?}, ambiguous {}",
            rep.dependents,
            rep.ambiguous_callers
        );
    }
}
#[cfg(test)]
mod being_the_only_candidate_is_not_evidence {
    use super::Index;

    /// A method call on a receiver the index never typed must not bind to the
    /// one symbol in the repository that happens to share its name.
    ///
    /// `cursor.execute(…)` on a `sqlite3.Cursor` is the reported shape: the
    /// receiver's type is not indexed, so the bare name `execute` is matched
    /// against every symbol, and a SINGLE hit was bound wherever it lived. The
    /// ambiguous case was already refused; this is the one left behind, and the
    /// refusal reasoning applies to it identically — being the only candidate
    /// is not evidence that the call site can reach it.
    ///
    /// MEASURED on this repository before the guard: 1 942 such edges, whose
    /// names say what they are — `push` 557, `join` 362, `now` 196, `split`
    /// 71, `replace` 61, `sleep` 51, every one a std method. `impact` on
    /// `model_costs.rs`, whose only broadly-named symbol is a private `fn
    /// push`, answered **382 dependents**; it answers 349 now. On the fixture
    /// below the released 1.1.8 answers two dependents and this answers one.
    fn tree() -> super::tempdir_lite::Dir {
        let d = super::tempdir_lite::Dir::new();
        let mk = |rel: &str, body: &str| {
            let p = d.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        };
        // The only `execute` in the tree, in its own package.
        mk(
            "fixtures/doubles.py",
            "class StubCursor:\n    def __init__(self):\n        self.sql = []\n\n\
                 def execute(self, sql, params=None):\n        self.sql.append(sql)\n",
        );
        // Calls `execute` on a sqlite3 cursor — a receiver nothing here types.
        mk(
            "app/db.py",
            "import sqlite3\n\n\ndef load_rows(path, where):\n\
             \u{20}   conn = sqlite3.connect(path)\n\
             \u{20}   cursor = conn.cursor()\n\
             \u{20}   cursor.execute(\"select 1\")\n\
             \u{20}   return cursor.fetchall()\n",
        );
        // Imports the stub and calls it: a real edge that must survive.
        mk(
            "app/real_user.py",
            "from fixtures.doubles import StubCursor\n\n\ndef make():\n\
             \u{20}   c = StubCursor()\n\
             \u{20}   c.execute(\"select 1\")\n\
             \u{20}   return c\n",
        );
        d
    }

    #[test]
    fn an_untyped_receiver_does_not_bind_and_a_real_importer_still_does() {
        let d = tree();
        let idx = Index::build(d.path()).unwrap();
        let rep = idx.impact_of("fixtures/doubles.py");
        assert!(
            rep.dependents.iter().any(|f| f == "app/real_user.py"),
            "the file that IMPORTS the stub stopped being a dependent: {:?}",
            rep.dependents
        );
        assert!(
            !rep.dependents.iter().any(|f| f == "app/db.py"),
            "a sqlite3 cursor was booked as calling into the stub: {:?}",
            rep.dependents
        );
    }

    /// Anti-vacuity in three directions: without them "report nothing" and
    /// "the fixture has no such call" would both pass the test above.
    #[test]
    fn the_fixture_really_exercises_the_guard() {
        let d = tree();
        let idx = Index::build(d.path()).unwrap();
        assert_eq!(
            idx.symbols
                .iter()
                .filter(|s| s.name == "execute" && s.kind != "impl")
                .count(),
            1,
            "the fixture must hold EXACTLY ONE `execute`, or the ambiguity \
             refusal decides this instead of the guard"
        );
        assert!(
            idx.calls
                .iter()
                .any(|c| c.callee_name == "execute" && c.file == "app/db.py"),
            "the untyped call was never extracted, so nothing was refused"
        );
        assert!(
            idx.calls
                .iter()
                .any(|c| c.callee_name == "execute" && c.file == "app/real_user.py"),
            "the real call was never extracted, so nothing was kept"
        );
    }

    /// Each of the four ways a call may bind, one arm apiece.
    ///
    /// Written after a mutation run left four survivors — refuse an empty
    /// qualifier, drop the import evidence, drop the same-directory evidence,
    /// drop symbol names from the vouched set — every one of them a hole in the
    /// FIXTURE rather than a property nothing could observe. A guard with four
    /// ways through needs four call sites, or three of them are decoration.
    #[test]
    fn every_evidence_path_carries_a_call_that_needs_it() {
        let d = super::tempdir_lite::Dir::new();
        let mk = |rel: &str, body: &str| {
            let p = d.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        };
        mk(
            "core/widget.py",
            "class Widget:\n    def spin_it(self):\n        return 1\n\n\ndef bare_helper():\n    return 2\n",
        );
        // 1. UNQUALIFIED call, unrelated package, no import resolved.
        mk("far/plain.py", "def go():\n    return bare_helper()\n");
        // 2. Receiver-qualified, different package, IMPORTS the declaring file.
        // The receiver names must appear NOWHERE as symbols, or the vouched
        // set answers before the evidence below is consulted — `thing` was a
        // parameter in the sibling file and quietly vouched for itself, which
        // let "drop the import evidence" survive.
        mk(
            "other/importer.py",
            "from core.widget import Widget\n\n\ndef go():\n\
             \u{20}   zqimp = Widget()\n\
             \u{20}   return zqimp.spin_it()\n",
        );
        // 3. Receiver-qualified, SAME directory, no import.
        mk(
            "core/sibling.py",
            "def go(zqsib):\n    return zqsib.spin_it()\n",
        );
        // 4. Qualified by the TYPE, different package, no import.
        mk(
            "far/bytype.py",
            "def go():\n    return Widget.spin_it(None)\n",
        );
        let idx = Index::build(d.path()).unwrap();
        let rep = idx.impact_of("core/widget.py");
        for (f, why) in [
            ("far/plain.py", "an unqualified call"),
            (
                "other/importer.py",
                "a call whose caller imports the declaration",
            ),
            ("core/sibling.py", "a call from the same directory"),
            ("far/bytype.py", "a call qualified by the declared type"),
        ] {
            assert!(
                rep.dependents.iter().any(|d| d == f),
                "{why} stopped binding ({f}): {:?}",
                rep.dependents
            );
        }
    }

    /// BOTH consumers, on every evidence path, in one assertion.
    ///
    /// This test exists because of a mistake worth not repeating. The guard was
    /// written for `impact_of` alone; two of its clauses could not be reached
    /// from there — `impact_of` answers "same file?" and "does the caller
    /// import it?" before it ever asks — so two mutations survived, and the
    /// clauses were deleted as dead code. They were not dead. They were
    /// unreachable from ONE consumer, and `callers_of_declaration` needed both
    /// the moment it existed: without them `query` dropped the genuine caller
    /// that imports the declaration, so the two commands went on disagreeing in
    /// the opposite direction.
    ///
    /// A predicate shared by two callers is exercised through BOTH, or
    /// "unreachable" is a claim about the caller that happened to be tested.
    #[test]
    fn both_consumers_exercise_every_evidence_path() {
        let d = super::tempdir_lite::Dir::new();
        let mk = |rel: &str, body: &str| {
            let p = d.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        };
        // The same-file caller uses a receiver named nowhere else: `self` is an
        // indexed parameter, so it is VOUCHED, and a same-file call written on
        // it never reaches the same-file clause at all. That is what let
        // "delete the same-file clause" survive a first mutation run.
        mk(
            "core/widget.py",
            "import queue\n\n\nclass Widget:\n    def spin_it(self):\n        return 1\n\n\
             \u{20}   def spin_it_inner(self):\n        return 2\n\n\ndef local_helper():\n\
             \u{20}   zqlocal = queue.Queue()\n\
             \u{20}   return zqlocal.spin_it_inner()\n",
        );
        // Imports the declaration and calls it on a local: the ONLY evidence is
        // the import, and only `callers_of_declaration` can observe it.
        mk(
            "other/importer.py",
            "from core.widget import Widget\n\n\ndef go():\n\
             \u{20}   zqi = Widget()\n\
             \u{20}   return zqi.spin_it()\n",
        );
        // Same directory, no import.
        mk(
            "core/sibling.py",
            "def go(zqs):\n    return zqs.spin_it()\n",
        );
        // Neither: a receiver nothing here types. Must not bind.
        mk(
            "far/stranger.py",
            "import queue\n\n\ndef go():\n\
             \u{20}   zqx = queue.Queue()\n\
             \u{20}   return zqx.spin_it()\n",
        );
        let idx = Index::build(d.path()).unwrap();

        let (callers, _) = idx.callers_of_declaration("spin_it", "core/widget.py", 50);
        let files: Vec<&str> = callers.iter().map(|c| c.file.as_str()).collect();
        assert!(
            files.contains(&"other/importer.py"),
            "the importing caller was dropped — the import clause is not live: {files:?}"
        );
        assert!(
            files.contains(&"core/sibling.py"),
            "the same-directory caller was dropped: {files:?}"
        );
        assert!(
            !files.contains(&"far/stranger.py"),
            "a queue.Queue was booked as calling spin_it: {files:?}"
        );

        // A call inside the declaring file, on a receiver named nowhere else.
        // Only this consumer sees it — `impact_of` skips a call whose file is
        // the file under report — and it must survive.
        let (inner, _) = idx.callers_of_declaration("spin_it_inner", "core/widget.py", 50);
        assert!(
            inner.iter().any(|c| c.file == "core/widget.py"),
            "a call inside the declaring file itself was dropped: {:?}",
            inner.iter().map(|c| &c.file).collect::<Vec<_>>()
        );

        // The two commands must not contradict each other. Every file `query`
        // names as a caller has to be a file `impact` names as a dependent.
        let rep = idx.impact_of("core/widget.py");
        for f in &files {
            if *f == "core/widget.py" {
                continue; // its own file is never its own dependent
            }
            assert!(
                rep.dependents.iter().any(|d| d == f),
                "`query` names {f} as a caller while `impact` does not list it: {:?}",
                rep.dependents
            );
        }
        assert!(
            !rep.dependents.iter().any(|d| d == "far/stranger.py"),
            "impact kept the phantom the query path refused: {:?}",
            rep.dependents
        );
    }

    /// A qualifier the index vouches for still binds: `logs::fold(…)` names the
    /// home the caller meant, and this guard must not touch it.
    #[test]
    fn a_qualifier_naming_a_real_module_still_binds() {
        let d = super::tempdir_lite::Dir::new();
        let mk = |rel: &str, body: &str| {
            let p = d.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        };
        mk("src/logs.rs", "pub fn fold_them() {}\n");
        mk(
            "app/run.rs",
            "pub fn go() {\n    crate::logs::fold_them();\n}\n",
        );
        let idx = Index::build(d.path()).unwrap();
        let rep = idx.impact_of("src/logs.rs");
        assert!(
            rep.dependents.iter().any(|f| f == "app/run.rs"),
            "a call qualified by its own module stopped resolving: {:?}",
            rep.dependents
        );
    }
}
#[cfg(test)]
mod narrowing_the_structure_map {
    use super::Index;
    use crate::framework::Framework;

    fn tree() -> super::tempdir_lite::Dir {
        let d = super::tempdir_lite::Dir::new();
        let mk = |rel: &str, body: &str| {
            let p = d.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        };
        mk("src/net.rs", "pub fn dial() {}\npub fn hangup() {}\n");
        mk("src/network/dns.rs", "pub fn resolve() {}\n");
        mk(
            "src/hooks/pre.rs",
            "pub fn before() {}\npub fn after() {}\n",
        );
        mk("docs/notes.rs", "pub fn note() {}\n");
        d
    }

    /// `src/net` names a file and a directory that both start with those
    /// bytes. A plain `starts_with` answers the question with `src/network`
    /// folded in, which is a wrong answer that looks like a right one.
    #[test]
    fn a_prefix_stops_at_a_path_separator() {
        assert!(Index::path_is_under("src/net.rs", "src/net.rs"));
        assert!(Index::path_is_under("src/hooks/pre.rs", "src/hooks"));
        assert!(Index::path_is_under("src/hooks/pre.rs", "./src/hooks/"));
        assert!(!Index::path_is_under("src/network/dns.rs", "src/net"));
        assert!(Index::path_is_under("anything.rs", ""));
    }

    /// The header counts belong to what is SHOWN. A narrowed map that keeps
    /// the workspace totals reports a repository under a listing of one
    /// directory, and the header is the figure a reader takes away.
    #[test]
    fn the_header_counts_what_the_narrowing_kept() {
        let d = tree();
        let idx = Index::build(d.path()).expect("index");
        let whole = idx.structure_map_under(Framework::Unknown, None);
        let part = idx.structure_map_under(Framework::Unknown, Some("src/hooks"));

        assert!(
            whole.starts_with("4 files, 6 symbols indexed\n"),
            "whole map header: {whole}"
        );
        assert!(
            part.starts_with("1 files, 2 symbols indexed under src/hooks\n"),
            "narrowed header must be the narrowed count: {part}"
        );
        assert!(!part.contains("docs/notes.rs"), "leaked outside: {part}");
        assert!(!part.contains("src/net.rs"), "leaked outside: {part}");
        assert!(part.len() < whole.len(), "narrowing must reduce");
    }

    /// A path that matches nothing is a question, not an empty subtree.
    /// `0 files, 0 symbols indexed` under a `by directory:` heading reads as
    /// "this directory holds nothing" — the one reading the caller cannot
    /// check, and the likeliest cause is a path spelled another way.
    #[test]
    fn a_prefix_that_matches_nothing_says_so() {
        let d = tree();
        let idx = Index::build(d.path()).expect("index");
        let out = idx.structure_map_under(Framework::Unknown, Some("src/absent"));
        assert!(
            out.contains("no indexed file under 'src/absent'"),
            "must name the prefix: {out}"
        );
        assert!(
            !out.contains("by directory:"),
            "an empty map must not be printed at all: {out}"
        );
        assert!(
            out.contains("4 files indexed"),
            "must give the scale: {out}"
        );
    }

    /// `structure_map` was a second copy of this renderer with the framework
    /// boost removed — which is exactly what `Unknown` does. They were
    /// byte-identical when merged; this keeps them so.
    #[test]
    fn the_plain_map_is_the_unknown_framework_map() {
        let d = tree();
        let idx = Index::build(d.path()).expect("index");
        assert_eq!(
            idx.structure_map(),
            idx.structure_map_under(Framework::Unknown, None),
            "the plain map must stay one renderer"
        );
    }
}
