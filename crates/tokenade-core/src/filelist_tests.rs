// The tests of `filelist`.
//
// Spliced by `include!` from `filelist.rs`: the same module, so the same
// private access and the same `super::` paths, without a single visibility
// line changed. This is the form this repo already uses for 33 other modules.
//
// A `//!` is impossible here: a file spliced by `include!` is not a module,
// and an INNER doc comment has nothing to qualify.

#[cfg(test)]
mod tests {
    use super::*;

    const FIND_SAMPLE: &str = "./Cargo.toml
./Cargo.lock
./README.md
./crates/tokenade-core/Cargo.toml
./crates/tokenade-core/src/lib.rs
./crates/tokenade-core/src/auto.rs
./crates/tokenade-core/src/openapi.rs
./crates/tokenade-core/src/graphql.rs
./crates/tokenade-core/src/sourcemap.rs
./crates/tokenade-cli/Cargo.toml
./crates/tokenade-cli/src/main.rs
./crates/tokenade-mcp/Cargo.toml
./crates/tokenade-mcp/src/lib.rs
./tests/integration_test.rs
./tests/unit_test.rs
./docs/architecture.md
./docs/api.md
./scripts/build.sh
./scripts/test.sh
./benchmarks/results.json
./benchmarks/results.csv
";

    const TREE_SAMPLE: &str = ".
├── Cargo.toml
├── Cargo.lock
├── README.md
├── crates
│   ├── tokenade-core
│   │   ├── Cargo.toml
│   │   └── src
│   │       ├── lib.rs
│   │       └── auto.rs
│   └── tokenade-cli
│       ├── Cargo.toml
│       └── src
│           └── main.rs
├── tests
│   └── integration_test.rs
└── docs
    └── api.md

7 directories, 9 files
";

    const LSLAR_SAMPLE: &str = ".:
total 32
drwxr-xr-x 5 user user 4096 Jan 15 10:00 .
drwxr-xr-x 3 user user 4096 Jan 15 10:00 ..
-rw-r--r-- 1 user user  234 Jan 15 10:00 Cargo.toml
-rw-r--r-- 1 user user 5678 Jan 15 10:00 Cargo.lock
-rw-r--r-- 1 user user  123 Jan 15 10:00 README.md
drwxr-xr-x 2 user user 4096 Jan 15 10:00 src
drwxr-xr-x 2 user user 4096 Jan 15 10:00 tests

./src:
total 16
drwxr-xr-x 2 user user 4096 Jan 15 10:00 .
drwxr-xr-x 5 user user 4096 Jan 15 10:00 ..
-rw-r--r-- 1 user user  234 Jan 15 10:00 main.rs
-rw-r--r-- 1 user user  567 Jan 15 10:00 lib.rs

./tests:
total 8
drwxr-xr-x 2 user user 4096 Jan 15 10:00 .
drwxr-xr-x 5 user user 4096 Jan 15 10:00 ..
-rw-r--r-- 1 user user  890 Jan 15 10:00 integration.rs
";

    /// `ls -l` given files by name prints no `total` line; it is still a
    /// listing. A GNU tar listing (`owner/group` where `ls` has a link count)
    /// is not.
    #[test]
    fn a_long_listing_of_named_files_without_a_total_is_a_listing() {
        let ls: String = (0..400)
            .map(|i| format!("-rw-r--r--  1 user  group  {}  2026-08-18 12:0{}  src/module_{i}/generated_{i}.rs\n", 1000 + i, i % 10))
            .collect();
        assert!(detect(&ls), "{ls}");
        assert_eq!(detect_shape(&ls), Shape::LsLaR);
        assert!(compact(&ls).starts_with("ls -laR, 400 file(s)"), "detected but not rolled up:\n{}", compact(&ls));
        let tar: String = (0..30)
            .map(|i| format!("-rw-r--r-- user/group {} 2026-08-18 12:0{} src/module_{i}/generated_{i}.rs\n", 1000 + i, i % 10))
            .collect();
        assert!(!detect(&tar), "{tar}");
    }

    #[test]
    fn detects_find_output() {
        assert!(detect(FIND_SAMPLE));
        assert_eq!(detect_shape(FIND_SAMPLE), Shape::Find);
    }

    #[test]
    fn detects_tree_output() {
        assert!(detect(TREE_SAMPLE));
        assert_eq!(detect_shape(TREE_SAMPLE), Shape::Tree);
    }

    #[test]
    fn detects_lslar_output() {
        assert!(detect(LSLAR_SAMPLE));
        assert_eq!(detect_shape(LSLAR_SAMPLE), Shape::LsLaR);
    }

    #[test]
    fn rejects_prose() {
        assert!(!detect(
            "This is a paragraph about file paths like /etc/passwd and ./some/file.txt.\n"
        ));
    }

    #[test]
    fn rejects_code() {
        assert!(!detect(
            "fn main() {\n    let path = \"./src/foo.rs\";\n    println!(\"{path}\");\n}\n"
        ));
    }

    #[test]
    fn rejects_log_with_few_paths() {
        let s = "INFO server started\nINFO loading config\n./path/that/looks/like.path\nINFO listening on :8080\nINFO request handled\n";
        // Only 1 path-like line out of 5 — well below the 75% threshold.
        assert!(!detect(s));
    }

    #[test]
    fn rejects_short_ls_la() {
        // Regression: `ls -la` on a small dir (3 entries) used to false-
        // trigger this compactor (the compactor then dropped most info as
        // "0 file(s)…"). Real `ls -laR` runs always produce ≥20 perm
        // lines or multiple `total ` lines, which we now require.
        let s = "total 27264\n\
                 drwxr-xr-x 2 me me     4096 May 28 20:21 .\n\
                 drwxr-xr-x 3 me me     4096 May 28 19:59 ..\n\
                 -rwxr-xr-x 1 me me 27883960 May 28 20:21 tokenade\n";
        assert!(
            !detect(s),
            "short `ls -la` (3 entries) should NOT match filelist compactor"
        );
    }

    #[test]
    fn accepts_real_ls_la_recursive_with_many_entries() {
        // Sanity: a real `ls -laR` style output with two dirs + many
        // perm lines SHOULD still match.
        let mut s = String::from("total 4\n");
        for i in 0..25 {
            let _ = writeln!(s, "-rw-r--r-- 1 me me 4096 May 28 20:00 file{i}.txt");
        }
        s.push_str("\ntotal 2\n");
        for i in 0..5 {
            let _ = writeln!(s, "-rw-r--r-- 1 me me 4096 May 28 20:00 sub{i}.txt");
        }
        assert!(
            detect(&s),
            "real ls -laR (50+ perm lines, 2 totals) MUST match"
        );
    }

    #[test]
    fn find_extracts_dirs_and_extensions() {
        // FIND_SAMPLE is 21 entries (intentionally short) — below
        // MIN_FILES_TO_FOLD so we'd pass through. Build a >= threshold
        // input that mirrors the same shape to verify the rollup.
        let mut s = String::new();
        for dir in &["crates", "tests", "docs", "scripts", "benchmarks"] {
            for i in 0..20 {
                let ext = ["rs", "md", "toml", "json", "sh"][i % 5];
                let _ = writeln!(s, "./{dir}/sub{}/file_{i}.{ext}", i % 6);
            }
        }
        let out = compact(&s);
        /* L122: banner removed; assertion deleted */
        assert!(out.contains("crates"));
        assert!(out.contains(".rs"));
        assert!(out.contains(".md"));
        assert!(out.contains(".toml"));
        assert!(out.contains(".json"));
        assert!(out.contains(".sh"));
    }

    #[test]
    fn small_find_sample_passes_through_unchanged() {
        // FIND_SAMPLE (21 files, < 1 KB) is exactly the kind of
        // listing the L102 fix protects from over-compaction.
        let out = compact(FIND_SAMPLE);
        assert!(!out.contains("[tokenade:filelist]"));
        // Paths preserved.
        assert!(out.contains("./Cargo.toml"));
        assert!(out.contains("./crates/tokenade-core/src/lib.rs"));
    }

    /// A drawn tree of PACKAGES is not a file listing, and the rollup must
    /// not claim it.
    ///
    /// What it printed was not a worse fold but a false one: on this
    /// repository's own `cargo tree` the answer opened with "1034 file(s)
    /// across 2 directories" and an extension histogram led by `.1` at 13.7%,
    /// which is the middle field of the version numbers.
    #[test]
    fn a_dependency_tree_is_not_a_file_tree() {
        let mut dep = String::from("tokenade-bench v1.1.8 (crates/tokenade-bench)\n");
        for (i, (name, ver)) in [
            ("regex", "1.12.3"),
            ("aho-corasick", "1.1.4"),
            ("memchr", "2.8.1"),
            ("regex-automata", "0.4.14"),
            ("regex-syntax", "0.8.10"),
            ("serde", "1.0.228"),
            ("serde_core", "1.0.228"),
            ("itoa", "1.0.17"),
        ]
        .iter()
        .enumerate()
        {
            let pre = if i % 2 == 0 { "├──" } else { "└──" };
            let _ = writeln!(dep, "│   {pre} {name} v{ver}");
        }
        assert!(
            !detect(&dep),
            "a cargo/pnpm/poetry dependency tree must not be read as files:\n{dep}"
        );
    }

    /// What counts as a version, spelling by spelling.
    ///
    /// The three conditions are asserted separately because a file tree is
    /// full of names that satisfy two of them: `report.2024.txt` has a numeric
    /// second field, `1.txt` a numeric first one, and `libLLVM.so.21.1` has
    /// numbers in the middle. None of them is a package version.
    #[test]
    fn a_version_is_two_numeric_fields_in_a_row() {
        for yes in [
            "regex v1.12.3",
            "serde 1.0.228",
            "pkg v0.4.14",
            "x 10.20",
            // npm / pnpm / yarn join the version with `@`, scoped names
            // included, and draw the same tree.
            "express@4.18.2",
            "@lydell/node-pty-linux-x64@1.2.0-beta.10",
            "UNMET OPTIONAL DEPENDENCY @scope/pkg@1.2.0",
        ] {
            assert!(carries_a_version(yes), "{yes} carries a version");
        }
        for no in [
            "libLLVM.so.21.1",
            "libssl.so.3",
            "report.2024.txt",
            "1.txt",
            "main.rs",
            "mail@example.com",
            "draft@2.txt",
            "Cargo.toml",
            "node_modules",
        ] {
            assert!(!carries_a_version(no), "{no} is a file name, not a version");
        }
    }

    /// The share, not the presence. A file tree with one versioned directory
    /// in it — `3.11` under a python lib, say — is still a file tree.
    #[test]
    fn one_versioned_name_does_not_make_a_dependency_tree() {
        let mut t = String::from(".\n");
        for name in [
            "3.11",
            "main.rs",
            "Cargo.toml",
            "README.md",
            "build.sh",
            "notes.txt",
            "logo.png",
            "data.csv",
            "index.html",
            "style.css",
        ] {
            let _ = writeln!(t, "├── {name}");
        }
        assert!(
            !looks_like_dependency_tree(&t),
            "1 of 10 is not a package tree"
        );

        let mut d = String::from(".\n");
        for name in [
            "regex v1.12.3",
            "serde v1.0.228",
            "memchr v2.8.1",
            "itoa v1.0.17",
            "libc v0.2.180",
            "notes.txt",
        ] {
            let _ = writeln!(d, "├── {name}");
        }
        assert!(looks_like_dependency_tree(&d), "5 of 6 is");
    }

    /// npm's tree is a dependency tree too, and it reaches the content route
    /// whenever the command name is gone — an MCP tool result, say.
    #[test]
    fn an_npm_tree_is_not_a_file_tree() {
        let mut t = String::from("@qwen-code/qwen-code@0.19.2 qwen-code\n");
        for name in [
            "@lydell/node-pty-darwin-arm64@1.2.0-beta.10",
            "@lydell/node-pty-linux-x64@1.2.0-beta.10",
            "express@4.18.2",
            "chalk@5.3.0",
            "yargs@17.7.2",
            "zod@3.23.8",
        ] {
            let _ = writeln!(t, "├── {name}");
        }
        assert!(!detect(&t), "npm ls is packages, not files:\n{t}");
    }

    /// A tree drawn without the space after the branch has no labels to read,
    /// and an empty ratio is not evidence of anything.
    #[test]
    fn a_tree_with_no_readable_labels_is_still_a_file_tree() {
        let mut t = String::from(".\n");
        for name in [
            "main.rs",
            "Cargo.toml",
            "README.md",
            "build.sh",
            "notes.txt",
            "logo.png",
            "data.csv",
            "index.html",
            "style.css",
            "LICENSE",
        ] {
            let _ = writeln!(t, "├──{name}");
        }
        assert!(!looks_like_dependency_tree(&t));
        assert!(detect(&t), "the file listing must keep its fold:\n{t}");
    }

    /// The strong signal still fires on a real `tree(1)` listing, including
    /// one whose names carry dotted numbers.
    #[test]
    fn a_file_tree_with_dotted_names_is_still_a_file_tree() {
        let mut files = String::from(".\n");
        for (i, name) in [
            "libLLVM.so.21.1",
            "libssl.so.3",
            "main.rs",
            "Cargo.toml",
            "README.md",
            "build.sh",
            "notes.txt",
            "logo.png",
        ]
        .iter()
        .enumerate()
        {
            let pre = if i % 2 == 0 { "├──" } else { "└──" };
            let _ = writeln!(files, "{pre} {name}");
        }
        assert!(
            detect(&files),
            "a versioned .so name is not a package version:\n{files}"
        );
    }

    #[test]
    fn tree_extracts_files() {
        // Build a tree-shaped sample with >= MIN_FILES_TO_FOLD entries.
        let mut s = String::from(".\n");
        for i in 0..(MIN_FILES_TO_FOLD + 5) {
            let ext = ["toml", "rs", "md"][i % 3];
            let pre = if i % 3 == 0 { "├──" } else { "└──" };
            let _ = writeln!(s, "{pre} file_{i}.{ext}");
        }
        let out = compact(&s);
        /* L122: banner removed; assertion deleted */
        assert!(out.contains(".toml"));
        assert!(out.contains(".rs"));
        assert!(out.contains(".md"));
    }

    #[test]
    fn lslar_extracts_files() {
        // Build an `ls -laR` sample > threshold.
        let mut s = String::from("total 1024\n");
        for i in 0..(MIN_FILES_TO_FOLD + 5) {
            let ext = ["rs", "toml", "lock", "md"][i % 4];
            let _ = writeln!(
                s,
                "-rw-r--r-- 1 user user {} Jan 15 10:00 file_{i}.{ext}",
                i * 17 + 100
            );
        }
        let out = compact(&s);
        /* L122: banner removed; assertion deleted */
        assert!(out.contains(".rs"));
        assert!(out.contains(".toml"));
        assert!(out.contains(".lock"));
        assert!(out.contains(".md"));
    }

    #[test]
    fn unknown_passes_through() {
        let t = "this is not a file listing";
        assert_eq!(compact(t), t);
    }

    #[test]
    fn handles_hidden_files() {
        // 12 hidden files (<MIN_FILES_TO_FOLD) — pass-through expected.
        let s =
            "./.gitignore\n./.gitattributes\n./.github/workflows/ci.yml\n./.editorconfig\n./.dockerignore\n./.env.example\n./.npmrc\n./.prettierrc\n./.eslintrc.json\n./.python-version\n./Makefile\n./Dockerfile\n";
        let out = compact(s);
        assert!(!out.contains("[tokenade:filelist]"));
        assert!(out.contains("./.gitignore"));
        assert!(out.contains("./.github/workflows/ci.yml"));
    }

    #[test]
    fn handles_unicode_paths() {
        // 11 paths, well under threshold — pass-through preserves the
        // verbatim path strings (which is exactly what matters for
        // unicode names — a rollup would lose them).
        let s = "./café/Müller.rs\n./中文/文件.py\n./العربية/ملف.js\n./🚀/rocket.go\n./café/déjà.rs\n./café/naïve.rs\n./中文/数据.py\n./中文/测试.py\n./العربية/ بيان.js\n./🚀/launch.go\n./café/Cargo.toml\n";
        // `./العربية/ بيان.js` has a space → looks_like_path rejects it.
        // 10 valid path lines out of 11 = 90%, still >75% threshold,
        // so detect() returns true.
        assert!(detect(s));
        let out = compact(s);
        // Pass-through preserves unicode dir names verbatim.
        assert!(out.contains("café"));
        assert!(out.contains("中文"));
        assert!(out.contains('🚀'));
        assert!(out.contains("Müller.rs"));
    }

    #[test]
    fn savings_huge_repo() {
        // 5 000-file synthetic monorepo.
        let mut s = String::with_capacity(200_000);
        for i in 0..5_000 {
            let dir = match i % 7 {
                0 => "src",
                1 => "tests",
                2 => "benchmarks",
                3 => "docs",
                4 => "examples",
                5 => "scripts",
                _ => "vendor",
            };
            let ext = match i % 5 {
                0 => "rs",
                1 => "toml",
                2 => "md",
                3 => "json",
                _ => "yml",
            };
            let _ = writeln!(s, "./{dir}/sub{}/file_{i}.{ext}", i % 50);
        }
        let (_out, sav) = compact_with_savings(&s);
        /* L122: banner removed; assertion deleted */
        assert!(sav.percent() > 95.0, "got {sav:?}");
    }

    #[test]
    fn short_find_output_passes_through_unmodified() {
        // Regression for 2026-05-29 user report: `find . -maxdepth 4
        // -type d -name .git -prune | sed | sort` produced 524 B of
        // 21 git-repo paths. tokenade replaced the list with
        // `[tokenade:filelist] find, 21 file(s) across N dirs`,
        // destroying the actual paths.
        let raw = "\
./projects/alpha
./projects/beta
./projects/gamma
./projects/delta
./projects/epsilon
./tools/foo
./tools/bar
./tools/baz
./vendor/x
./vendor/y
./vendor/z
./libs/aa
./libs/bb
./libs/cc
./libs/dd
./libs/ee
./scripts/one
./scripts/two
./scripts/three
./scripts/four
./scripts/five
";
        assert!(
            raw.len() < MIN_BYTES_TO_FOLD,
            "fixture should be under-threshold"
        );
        assert!(detect(raw), "fixture should still match the find shape");
        let out = compact(raw);
        // No rollup summary: paths must survive verbatim.
        assert!(!out.contains("[tokenade:filelist]"), "got: {out}");
        for p in [
            "./projects/alpha",
            "./tools/foo",
            "./libs/aa",
            "./scripts/five",
        ] {
            assert!(out.contains(p), "missing path {p} in:\n{out}");
        }
    }

    #[test]
    fn large_find_output_still_folds() {
        // The pre-flight short-circuit must not break the large-list case.
        let mut s = String::with_capacity(MIN_BYTES_TO_FOLD * 4);
        for i in 0..(MIN_FILES_TO_FOLD + 200) {
            let _ = writeln!(s, "./pkg{}/sub/file_{i}.rs", i % 8);
        }
        assert!(s.len() > MIN_BYTES_TO_FOLD);
        let _out = compact(&s);
        /* L122: banner removed; assertion deleted */
    }
    /// The defect this grouping exists to answer.
    ///
    /// `find /abs/path …` — the form this repo's own instructions ask for —
    /// keyed every entry on the first segment of an ABSOLUTE path, so a
    /// listing spanning many directories reported one, named `home`. The
    /// question "where are these files" got the answer "under /".
    #[test]
    fn an_absolute_listing_groups_below_its_shared_root() {
        let mut s = String::new();
        for i in 0..(MIN_FILES_TO_FOLD + 3_000) {
            let top = ["crates", "docs", "benchmarks"][i % 3];
            let _ = writeln!(s, "/home/u/work/proj/{top}/sub{}/f_{i}.rs", i % 5);
        }
        assert!(s.len() > MIN_BYTES_TO_FOLD);
        let out = compact(&s);
        for top in ["crates", "docs", "benchmarks"] {
            assert!(out.contains(top), "group {top} missing from:\n{out}");
        }
        assert!(
            !out.contains(" 1 directory"),
            "still collapsed to one group:\n{out}"
        );
        assert!(
            out.contains("shared root: /home/u/work/proj/"),
            "the root every entry shares is not named:\n{out}"
        );
    }

    /// The relative form was already right, and must stay byte-identical in
    /// its grouping: `./crates/…` shares no root beyond `.`, which the cap
    /// and the skip both leave alone.
    #[test]
    fn a_relative_listing_keeps_its_first_segment_groups() {
        let mut s = String::new();
        for i in 0..(MIN_FILES_TO_FOLD + 200) {
            let top = ["crates", "docs", "benchmarks"][i % 3];
            let _ = writeln!(s, "./{top}/sub{}/f_{i}.rs", i % 5);
        }
        let out = compact(&s);
        for top in ["crates", "docs", "benchmarks"] {
            assert!(out.contains(top), "group {top} missing from:\n{out}");
        }
        assert!(
            !out.contains("shared root:"),
            "no root is shared here:\n{out}"
        );
    }

    /// The cap, which is the difference between a grouping and a listing of
    /// one group per file. `tree` and `ls -laR` hand this function BASENAMES:
    /// uncapped, a set of one file consumes its own key and every group is
    /// `(root)`.
    #[test]
    fn a_shared_root_never_eats_the_last_segment() {
        // `find /one/deep` lists the root ITSELF as its first line. That
        // line is a DIRECTORY, and a cap taken from the shortest entry reads
        // it as a file: the grouping drops a level and `src` and `docs`
        // collapse into one group called `deep`. Entries the shared root
        // consumes entirely are rendered `(root)` and cost nothing.
        let mut s = String::from("/one/deep\n");
        for i in 0..(MIN_FILES_TO_FOLD + 3_000) {
            let top = ["src", "docs"][i % 2];
            let _ = writeln!(s, "/one/deep/{top}/f_{i}.rs");
        }
        let out = compact(&s);
        assert!(
            out.contains("shared root: /one/deep/"),
            "a directory entry pulled the root up a level:\n{out}"
        );
        for top in ["src", "docs"] {
            assert!(out.contains(top), "group {top} lost:\n{out}");
        }
    }

    /// (The two tests above are about the CENSUS: 3 000 paths, to go past the
    /// shipping threshold below which the list is rendered.)
    ///
    /// `grep -rIli statistiques ~/projects | head -50`: fifty long paths
    /// (6 KB) rendered as a census, without a single path — and re-read via
    /// `expand-ref` every time. Below the threshold, every path is shipped,
    /// the common root said once.
    #[test]
    fn a_list_that_fits_is_listed_not_rolled_up() {
        let mut s = String::new();
        for i in 0..50 {
            let _ = writeln!(s, "/home/u/projects/web/content/blog/fr/2026/septembre/article-{i:02}-sur-les-statistiques.mdx");
        }
        assert!(s.len() > MIN_BYTES_TO_FOLD);
        let out = compact(&s);
        assert!(!out.contains("no individual path"), "rolled up:\n{out}");
        for i in 0..50 {
            assert!(out.contains(&format!("article-{i:02}-sur-les-statistiques.mdx")), "path {i} lost:\n{out}");
        }
        assert!(out.len() < s.len(), "the shared root is said once");
        assert!(out.contains("[paths from /home/u/projects/"), "the root must stay absolute:\n{out}");

        // The bound is the SHIPPING threshold, not the fold floor: 200
        // paths with no common root (≈ 7 KB rendered) are still listed.
        let mut s = String::new();
        for i in 0..200 {
            let _ = writeln!(s, "/srv/app{i}/conf{i}/settings-{i}.toml");
        }
        let out = compact(&s);
        assert!(out.len() > 4_096 && !out.contains("no individual path"), "rolled up:\n{out}");
    }

    #[test]
    fn common_path_segments_counts_segments_not_bytes() {
        // `crates/foo` and `crates/font` share `crates`, NOT `crates/fo`.
        assert_eq!(common_path_segments(&["crates/foo", "crates/font"]), 1);
        assert_eq!(common_path_segments(&["a/b/c", "a/b/d"]), 2);
        assert_eq!(common_path_segments(&["a/b", "x/y"]), 0);
        assert_eq!(common_path_segments(&[]), 0);
    }

    /// One declarer, and the sweep that keeps it that way.
    ///
    /// Three files computed a common path prefix by hand and the three
    /// spellings had already drifted — one byte-wise then cut back to the
    /// last `/`, one segment-wise, one uncapped. A guard on the fixed site
    /// says nothing about the fourth copy somebody writes next week; this
    /// one reads the source.
    #[test]
    fn every_common_path_prefix_asks_the_one_declarer() {
        // `filelist` has branches (`filelist/detect.rs`): read the FAMILY.
        let filelist = crate::source_guard::library_module_family("filelist");
        let sources: [(&str, &str); 4] = [
            ("filelist.rs", filelist.as_str()),
            ("grep.rs", include_str!("cmd/linux/grep.rs")),
            (
                "next_route_table.rs",
                include_str!("cmd/linux/next_route_table.rs"),
            ),
            ("git_listing.rs", include_str!("cmd/linux/git_listing.rs")),
        ];
        let mut declarers = 0usize;
        let mut callers = 0usize;
        for (name, src) in sources {
            // Production lines only, brace-tracked. Splitting at the first
            // `#[cfg(test)]` reads as the same thing and is not: `git_listing.rs`
            // opens a test module at line 74 and declares `path_census` at 123,
            // so that spelling hid the call site this floor was raised to count.
            let body = crate::srcsweep::production_lines(src);
            for (_, line) in &body {
                let t = line.trim_start();
                if t.starts_with("//") || !t.contains("common_path_segments") {
                    continue;
                }
                if t.contains("fn common_path_segments") {
                    declarers += 1;
                } else {
                    callers += 1;
                }
            }
            // The hand-rolled byte-wise form, in any file that has since been
            // routed through the declarer.
            assert!(
                !src.contains("bytes()\n            .zip("),
                "{name} computes a common prefix byte-wise again"
            );
        }
        assert_eq!(declarers, 1, "the prefix is declared {declarers} times");
        assert!(
            callers >= 4,
            "only {callers} call site(s) reached — the sweep is not seeing the callers"
        );
    }
}

#[cfg(test)]
mod common_prefix_header_tests {
    use super::*;

    #[test]
    fn the_decision_is_made_once_for_everyone() {
        let long = ["/usr/share/doc/a", "/usr/share/doc/b", "/usr/share/doc/c"];
        let (h, n) = common_prefix_header(&long).expect("a root that pays");
        assert_eq!(h, "[paths from /usr/share/doc/]");
        assert_eq!(n, "/usr/share/doc".len() + 1);

        // Every line on the SAME file: no directory to join, so no trailing
        // `/` — an agent gluing them back would get
        // `crates/a/b/hcl.rs/128`, a path that does not open.
        let same = ["crates/a/b/hcl.rs", "crates/a/b/hcl.rs", "crates/a/b/hcl.rs"];
        assert_eq!(
            common_prefix_header(&same).expect("a root").0,
            "[all matches in crates/a/b/hcl.rs]"
        );

        // Too short to be worth its header.
        assert!(common_prefix_header(&["src/a", "src/b", "src/c"]).is_none());
        // No common root.
        assert!(common_prefix_header(&["/a/x/aaaaaaaaaaaa", "/b/y/bbbbbbbbbbbb", "/c/z/cccccccccccc"]).is_none());
        // Fewer than three lines: the header would cost more than it saves.
        assert!(common_prefix_header(&["/usr/share/doc/a", "/usr/share/doc/b"]).is_none());
    }
}

#[cfg(test)]
mod smallest_case_tests {
    use super::*;

    /// **The smallest accepted case must still save bytes.**
    ///
    /// There is no profitability gate in `common_prefix_header` because with
    /// its two bounds it could never refuse. This test holds the reason: if
    /// either bound goes down, the fold can turn negative and this is WHERE
    /// it shows.
    #[test]
    fn the_smallest_case_still_pays() {
        // Exactly the minimum: three paths, a ten-character prefix.
        let paths = ["/abcdefghi/a", "/abcdefghi/b", "/abcdefghi/c"];
        let (header, strip) = common_prefix_header(&paths).expect("the minimum is accepted");
        assert_eq!(strip, "/abcdefghi".len() + 1);
        let before: usize = paths.iter().map(|p| p.len() + 1).sum();
        let after: usize = header.len() + 1 + paths.iter().map(|p| p.len() + 1 - strip).sum::<usize>();
        assert!(
            after < before,
            "the smallest accepted case makes the output GROW: {before} → {after}"
        );

        // And JUST below the bound, the fold would be negative — that is
        // why it is there. Seven characters: three lines save 24 bytes and
        // the header costs 25.
        let short = ["/abcdef/a", "/abcdef/b", "/abcdef/c"];
        assert_eq!(short[0][.."/abcdef".len()].len(), 7);
        assert!(
            common_prefix_header(&short).is_none(),
            "a too-short prefix was accepted: the fold would cost more than it saves"
        );
    }
    /// **A refused listing must not be rescanned from every line.**
    ///
    /// The first version restarted one step further when the group did not
    /// pay, and `take_while` re-walked the same run: quadratic. MEASURED
    /// 2026-09-09 on forty thousand lines of `grep -rn` under a one-character
    /// directory — a perfectly ordinary case, not a malicious input — **the
    /// command still had not returned after two minutes**; it now returns in
    /// a third of a second.
    ///
    /// The bound is deliberately wide (two orders of magnitude above the
    /// measured time): this test says "not quadratic", not "fast".
    #[test]
    fn a_refused_run_is_scanned_once() {
        let lines: Vec<String> = (0..40_000).map(|i| format!("a/f{i}.rs")).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let start = std::time::Instant::now();
        let mut out = String::new();
        super::push_grouped(&mut out, &refs, 0, &|l| Some(l));
        let elapsed = start.elapsed();
        // The fixture REACHES the rule: this one-character directory never
        // pays, so it is indeed the REFUSAL path that is walked.
        assert!(!out.contains("a/\n"), "the group paid, the test measures something else");
        assert_eq!(out.lines().count(), 40_000);
        assert!(
            elapsed < std::time::Duration::from_secs(10),
            "the refusal rescans the run: {elapsed:?} for 40 000 lines"
        );
    }

    /// The root goes once, the names all stay.
    #[test]
    fn a_path_list_states_its_root_once() {
        let v = "/var/lib/docker/overlay2/l/AAA:/var/lib/docker/overlay2/l/BBB:                 /var/lib/docker/overlay2/l/CCC";
        let v = v.replace(' ', "");
        let out = super::collapse_path_list(&v).expect("three paths under one root");
        assert_eq!(out, "/var/lib/docker/overlay2/l/{AAA:BBB:CCC}");
        assert!(out.len() < v.len());
        // Two paths do not pay.
        assert!(super::collapse_path_list("/var/lib/docker/a:/var/lib/docker/b").is_none());
        // What is not a list of absolute paths is left as is.
        assert!(super::collapse_path_list("host:port:8080").is_none());
        // A root that is too short does not pay: `/a/b` is not worth the braces.
        assert!(super::collapse_path_list("/a/b/x:/a/b/y:/a/b/z").is_none());
    }

}

#[cfg(test)]
mod a_text_that_mentions_a_tree {
    use super::{compact, detect};
    use std::fmt::Write;

    /// `awk 'NR>=469 && NR<=574' filelist.rs | nl -ba`: a hundred lines of
    /// Rust, two of which write `"── "` in a literal, rendered as "tree, 2
    /// file(s) — a rollup".
    #[test]
    fn source_that_spells_the_drawing_is_not_a_tree() {
        let mut s = String::new();
        for i in 0..100 {
            let line = match i {
                40 => "    Shape::Tree => t.contains(\"── \"),".to_string(),
                41 => "    if let Some((_, name)) = trimmed.rsplit_once(\"├── \") {".to_string(),
                _ => format!("    let value_{i} = compute_{i}(input, &options);"),
            };
            let _ = writeln!(s, "{:>6}: {line}", 469 + i);
        }
        assert!(!detect(&s), "a source slice read as a tree listing");
        assert_eq!(compact(&s), s);
    }

    #[test]
    fn a_real_tree_is_still_one() {
        let mut s = String::from(".\n");
        for d in 0..4 {
            let _ = writeln!(s, "├── dir{d}");
            for f in 0..3 {
                let _ = writeln!(s, "│   ├── file{f}.rs");
            }
        }
        s.push_str("└── README.md\n\n5 directories, 13 files\n");
        assert!(detect(&s), "tree(1) output must stay a tree");
    }

    /// `mdfind` / `find` print a path with a space as it is. Read as
    /// "not a path", it was left out of the rollup's count and printed raw
    /// under "no individual path is listed below" — and its mere presence
    /// sent a listing that fits to the rollup instead of the listing.
    fn spaced_listing() -> String {
        let mut t = String::new();
        for i in 0..120 {
            t.push_str(&format!("/System/Library/LaunchDaemons/com.apple.daemon{i}.plist\n"));
        }
        t.push_str("/System/Library/OpenDirectory/Templates/Active Directory.plist\n");
        t.push_str("/System/Library/Address Book Plug-Ins/LDAP.sourcebundle/Contents/Info.plist\n");
        t.push_str("find: '/System/Library/Caches': Permission denied\n");
        t
    }

    #[test]
    fn a_path_with_a_space_is_listed_with_the_others() {
        let _g = tk_base::delivery::threshold_on_this_thread(5_000);
        let out = compact(&spaced_listing());
        assert!(!out.contains("a rollup"), "{out}");
        assert!(out.contains("Active Directory.plist"), "{out}");
        assert!(out.contains("Permission denied"), "{out}");
    }

    #[test]
    fn a_path_with_a_space_is_counted_by_the_rollup() {
        let _g = tk_base::delivery::threshold_on_this_thread(10);
        let out = compact(&spaced_listing());
        assert!(out.contains("find, 122 file(s)"), "{out}");
        assert!(!out.contains("Active Directory.plist"), "counted, not listed raw:\n{out}");
        assert!(out.contains("Permission denied"), "{out}");
    }
}

#[cfg(test)]
mod windows_paths {
    /// A real `Get-ChildItem -Recurse -Filter *.mui | Select-Object -First 300
    /// FullName`: read as a listing — no stride-sampling — and handed back in
    /// Windows separators.
    #[test]
    fn a_windows_path_listing_is_folded_as_a_listing() {
        let text = include_str!("../tests/fixtures/linux/windows_fullname_listing.txt");
        assert!(crate::filelist::detect(text));
        let out = crate::filelist::compact(text);
        eprintln!("{out}");
        assert!(!out.contains("runs of"), "{out}");
        assert!(out.contains("C:\\Windows\\System32"), "{out}");
        assert!(!out.lines().filter(|l| !l.starts_with("[tokenade")).any(|l| l.contains('/')), "{out}");
        assert!(out.len() * 3 < text.len(), "{} vs {}", out.len(), text.len());
    }

    /// The sentences of a rollup go through the same `/` → `\\` hand-back as
    /// the paths: one with a `/` read `` `-name`\glob `` on a real recursive
    /// `Get-ChildItem` of 1 228 files.
    #[test]
    fn a_rollup_of_windows_paths_keeps_its_sentences() {
        let text: String = (0..3000)
            .map(|i| format!("C:\\Data\\d{}\\sub{}\\file{i}.txt\n", i % 40, i % 7))
            .collect();
        let out = crate::filelist::compact(&text);
        assert!(out.contains("a rollup"), "{out}");
        for l in out.lines().filter(|l| l.contains(' ') && !l.contains(":\\")) {
            assert!(!l.contains('\\'), "a sentence was rewritten: {l}");
        }
    }
}
