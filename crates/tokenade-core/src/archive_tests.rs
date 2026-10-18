// The tests of `archive`.
//
// Spliced by `include!` from `archive.rs`: the same module, so the same
// private access and the same `super::` paths, without a single visibility
// line changed. It is the shape this repo already uses for 33 other modules.
//
// A `//!` is impossible here: a file spliced by `include!` is not a
// module, and an INNER doc comment has nothing to qualify.

#[cfg(test)]
mod grouping_tests {
    use super::*;

    /// Nothing to group is not "grouped". `detect_format`'s own `pathish >= 5`
    /// makes this unreachable today, which is exactly why it is asserted here
    /// rather than left to a caller that does not exist yet: without it,
    /// `roots.len() * 2 <= 0` reads as TRUE and an empty input becomes a
    /// listing.
    #[test]
    fn nothing_does_not_group() {
        assert!(!entries_group_by_directory(""));
        assert!(!entries_group_by_directory("\n   \nno-slash-here\n"));
    }

    /// Both halves, because a discriminator that rejects everything passes any
    /// test that only shows it something wrong.
    #[test]
    fn base64_is_not_a_listing_and_a_listing_still_is() {
        // Base64 uses `/` as one of its 64 symbols, so every exclusion the
        // shape predicate has accumulated lets these through. 3 509 such lines
        // were delivered as "plain-listing — 3509 entries … grouped by
        // directory".
        let alphabet: Vec<char> = ('A'..='Z')
            .chain('a'..='z')
            .chain('0'..='9')
            .chain(['+', '/'])
            .collect();
        let mut b64 = String::new();
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        for _ in 0..1200 {
            for _ in 0..76 {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                b64.push(alphabet[(seed % 64) as usize]);
            }
            b64.push('\n');
        }
        // Past MAX_ENTRIES on the slash-carrying lines alone: below it the
        // renderer prints entries verbatim and claims nothing, so a smaller
        // fixture would pass whatever the discriminator did.
        assert!(
            b64.lines().filter(|l| l.contains('/')).count() > MAX_ENTRIES,
            "fixture proves nothing unless it reaches the grouped rendering"
        );
        assert_eq!(
            detect_format(&b64),
            Format::Unknown,
            "base64 read as a listing"
        );

        // The CONTENT route too: `auto::detectors` calls `detect_format` on a
        // truncated head, so a bar that scales with the caller's window is a
        // bar that caller can switch off by passing less. Both routes, one
        // rule.
        assert_ne!(
            crate::auto::detect(&b64),
            crate::auto::Kind::ArchiveListing,
            "base64 read as a listing on the content route"
        );

        // A real listing: many entries, few roots. This is what the renderer
        // means by "grouped by directory".
        let mut listing = String::new();
        for i in 0..400 {
            listing.push_str(&format!("crates/tokenade-core/src/module_{i}.rs\n"));
        }
        assert_eq!(
            detect_format(&listing),
            Format::Plain,
            "a real listing was rejected"
        );

        // And one with several roots, still far below the bar.
        let mut mixed = String::new();
        for i in 0..300 {
            let root = ["crates", "reports", "docs"][i % 3];
            mixed.push_str(&format!("{root}/sub{i}/file{i}.txt\n"));
        }
        assert_eq!(
            detect_format(&mixed),
            Format::Plain,
            "a multi-root listing was rejected"
        );

        // A listing whose path is the LAST field, not the first: a size
        // column in front of it, the way `du` and `rsync --stats` print. Read
        // the FIRST field instead and no line carries a slash at all, so every
        // such listing is rejected. The shape predicate above already takes
        // the last field; this half keeps the two agreeing. (A literal tab
        // would not do: the predicate requires every char to be graphic or a
        // space, so a real tab-separated `du` never reaches this branch.)
        let mut sized = String::new();
        for i in 0..300 {
            sized.push_str(&format!("4.0K crates/tokenade-core/src/mod_{i}.rs\n"));
        }
        assert_eq!(
            detect_format(&sized),
            Format::Plain,
            "a listing with a size column was rejected"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line that names a FAILURE is never a listing entry, whatever the
    /// detector decided.
    ///
    /// `failure_survives` measures this on a `composer install` misdetected
    /// as an archive: three hundred `- Installing vendor/pkgN` lines end
    /// with three that say the run could not be resolved. Grouping them away
    /// swallowed the reason the command failed — 23 528 raw chars to 177
    /// kept, marker gone.
    #[test]
    fn a_failure_line_is_never_grouped_away() {
        let mut t = String::new();
        for i in 0..MAX_ENTRIES + 100 {
            t.push_str(&format!(
                "  - Installing vendor/pkg{i} (1.0.{i}): Extracting archive\n"
            ));
        }
        t.push_str("Your requirements could not be resolved to an installable set of packages.\n");
        t.push_str("  Problem 1\n");
        t.push_str("    - Root composer.json requires php ^8.2 ZQMARKERZQ\n");
        let out = compact(&t);
        assert!(
            out.contains("ZQMARKERZQ"),
            "the reason the command failed was grouped away:\n{out}"
        );
    }

    /// The commonest archive there is — a release tarball, a `.crate`, a zip
    /// of one project — puts every entry under one directory. Keying the
    /// breakdown on the first segment then reports ONE group whose count is
    /// the total already printed above. Measured 2026-08-27 on
    /// `llama-cpp-sys-4-0.3.2.crate`: 1 480 entries, 105 422 bytes, folded to
    /// `llama-cpp-sys-4-0.3.2/  (1480 entries)` and nothing else.
    #[test]
    fn a_single_root_archive_names_its_directories() {
        let mut t = String::new();
        for i in 0..MAX_ENTRIES {
            t.push_str(&format!("pkg-1.2.3/src/mod_{i}.rs\n"));
        }
        for i in 0..40 {
            t.push_str(&format!("pkg-1.2.3/docs/page_{i}.md\n"));
        }
        t.push_str("pkg-1.2.3/README.md\n");
        let out = compact(&t);
        assert!(
            out.contains("shared root: pkg-1.2.3/"),
            "root unnamed:\n{out}"
        );
        assert!(out.contains("src/"), "{out}");
        assert!(out.contains("docs/"), "{out}");
        let groups = out.lines().filter(|l| l.contains(" entries)")).count();
        assert!(
            groups > 1,
            "still one group restating the total ({groups}):\n{out}"
        );
    }

    /// `find src -type f; find manual -type f` (749 entries, from a real
    /// project) came back as `src/  (698 entries)` and `manual/  (50
    /// entries)`. The directory holding most of the listing is opened, down
    /// through a single-child chain, and a directory that is not dominant is
    /// left as one line.
    #[test]
    fn the_directory_holding_most_entries_is_opened() {
        let subs = [("accounts", 42), ("platforms", 122), ("tasks", 60), ("wiki", 9)];
        let mut t = String::new();
        for (d, k) in subs {
            for i in 0..k {
                let ext = if i % 5 == 0 { "md" } else { "py" };
                let _ = writeln!(t, "src/linking/{d}/{d}_{i}_handler.{ext}");
            }
        }
        for i in 0..7 {
            let _ = writeln!(t, "src/linking/cli_{i}.py");
        }
        for i in 0..50 {
            let _ = writeln!(t, "manual/step_{i}/notes.txt");
        }
        let out = compact(&t);
        assert!(out.contains("  src/linking/platforms/  (122 entries)"), "{out}");
        assert!(out.contains("  src/linking/wiki/  (9 entries)"), "{out}");
        assert!(out.contains("  src/linking/  (7 files directly in it)"), "{out}");
        assert!(out.contains("  manual/  (50 entries)"), "not dominant, not opened:\n{out}");
        assert!(!out.contains("  src/  ("), "{out}");
        assert!(out.lines().count() < 12, "{out}");
    }

    /// The name is what follows the time or year column, in all three
    /// shapes an entry line comes in. A fixed token skip stopped one byte
    /// into the fifth column: GNU tar gave `2:34 src/a.rs`, and an `ls -l`
    /// line gave its size minus a digit (`05` for 105) plus the date.
    #[test]
    fn an_entry_line_yields_its_name_and_nothing_before_it() {
        let cases = [
            ("-rw-r--r-- user/group 1234 2024-01-01 12:34 src/a.rs", "src/a.rs"),
            ("drwxr-xr-x user/group 0 2024-01-01 12:34:56 docs/", "docs/"),
            ("-rw-r--r--  0 root   wheel   1955 Jan  1  2024 lib/x 2020.txt", "lib/x 2020.txt"),
            ("-rw-r--r-- 1 me me  105  2 oct.  15:30 f_105.txt", "f_105.txt"),
            ("-rw------- 1 me me 2590 11 nov.   2023 id_rsa", "id_rsa"),
            ("lrwxrwxrwx 1 me me    7 15:30 cur -> v2", "cur -> v2"),
        ];
        for (line, name) in cases {
            assert_eq!(path_from_tar_line(line).as_deref(), Some(name), "{line}");
        }
        assert_eq!(path_from_tar_line("-rw-r--r-- user/group 12 no date here"), None);
    }

    /// `ls -la ~/.ssh | grep -v '^total'` has no `total` line and no `.`/`..`
    /// entry left, and was taken for a tar listing: sizes and modes gone, the
    /// two things the listing was run for. A non-zero link count is `ls`.
    #[test]
    fn a_filtered_ls_listing_is_not_a_tar_listing() {
        let ls = "-rw------- 1 me me  573 11 nov.   2023 authorized_keys\n\
                  -rw------- 1 me me 1700  4 juil.  2025 deploy_key\n\
                  -rw-r--r-- 1 me me  405  4 juil.  2025 deploy_key.pub\n";
        assert!(!matches!(detect_format(ls), Format::TarVerbose), "{ls}");
        let bsdtar = "-rw-r--r--  0 root wheel 1955 Jan  1  2024 a/x.txt\n\
                      -rw-r--r--  0 root wheel  105 Jan  1  2024 a/y.txt\n";
        assert!(matches!(detect_format(bsdtar), Format::TarVerbose));
    }

    /// `for p in …; do echo "=== $p"; find $p -type f; done` grouped as one
    /// listing turned its headers into a directory `=== orbit-infra/  (3
    /// entries)` and pooled the sections. Each header now stands verbatim
    /// above its own section's grouping.
    #[test]
    fn echo_headers_split_a_listing_into_sections() {
        let mut t = String::from("=== infra/apps/control-plane/src\n");
        for (d, k) in [("connectors", 90), ("audit", 41), ("auth", 17)] {
            for i in 0..k {
                let ext = if i % 4 == 0 { "spec.ts" } else { "ts" };
                let _ = writeln!(t, "infra/apps/control-plane/src/{d}/{d}-{i}.{ext}");
            }
        }
        t.push_str("=== core/cross-repo\n");
        for i in 0..60 {
            let _ = writeln!(t, "core/cross-repo/parcours/step_{i}.sh");
        }
        t.push_str("## infra/scripts/checks (none)\n");
        let out = compact(&t);
        assert!(!out.contains("=== infra/  ("), "a header was grouped as a path:\n{out}");
        let at = |needle: &str| out.find(needle).unwrap_or_else(|| panic!("{needle} missing:\n{out}"));
        assert!(at("=== infra/apps/control-plane/src") < at("infra/apps/control-plane/src/connectors/  (90 entries)"));
        assert!(at("infra/apps/control-plane/src/auth/  (17 entries)") < at("=== core/cross-repo"));
        assert!(at("=== core/cross-repo") < at("core/cross-repo/parcours/  (60 files directly in it)"));
        assert!(out.ends_with("## infra/scripts/checks (none)"), "{out}");
        assert!(out.contains("— 208 entries"), "headers are not entries:\n{out}");
    }

    /// A listing whose entries share nothing keeps the grouping it had.
    #[test]
    fn a_multi_root_archive_is_unchanged() {
        let mut t = String::new();
        for i in 0..MAX_ENTRIES {
            t.push_str(&format!("alpha-1.0/src/a_{i}.rs\n"));
        }
        for i in 0..MAX_ENTRIES {
            t.push_str(&format!("beta-2.0/src/b_{i}.rs\n"));
        }
        let out = compact(&t);
        assert!(out.contains("alpha-1.0/"), "{out}");
        assert!(out.contains("beta-2.0/"), "{out}");
        assert!(
            !out.contains("shared root:"),
            "nothing is shared here:\n{out}"
        );
    }

    /// A file sitting directly in the shared root is not a directory of its
    /// own — without that test each such file would open a group named after
    /// itself, and the breakdown would be one line per file again.
    #[test]
    fn a_file_in_the_shared_root_is_not_a_directory() {
        let mut t = String::new();
        for i in 0..MAX_ENTRIES {
            t.push_str(&format!("pkg-1.2.3/loose_{i}.txt\n"));
        }
        for i in 0..40 {
            t.push_str(&format!("pkg-1.2.3/src/m_{i}.rs\n"));
        }
        let out = compact(&t);
        assert!(out.contains("(root)/"), "loose files did not group:\n{out}");
        assert!(
            !out.contains("loose_7.txt"),
            "a loose file opened its own group:\n{out}"
        );
        assert!(
            out.contains("src/"),
            "the real directory did not group:\n{out}"
        );
    }

    #[test]
    fn ls_la_is_not_an_archive() {
        // `ls -la` shares tar's line shape but has a `total N` header and
        // `.`/`..` entries (name = LAST token) — must NOT classify.
        let out = "total 0\ndrwxr-sr-x 3 root systemd-journal 4096 2022-12-31 19:31 .\ndrwxr-sr-x 3 root systemd-journal 4096 2022-12-31 19:31 ..\n";
        assert!(matches!(
            detect_format(out),
            Format::Unknown | Format::Plain
        ));
        // Even WITHOUT the `total` header (grep-filtered), dot entries veto.
        let no_total = "drwxr-sr-x 3 root sj 4096 2022-12-31 19:31 .\ndrwxr-sr-x 3 root sj 4096 2022-12-31 19:31 ..\n-rw-r--r-- 1 root sj 12 2022-12-31 19:31 x.log\n";
        assert!(matches!(
            detect_format(no_total),
            Format::Unknown | Format::Plain
        ));
    }

    #[test]
    fn mixed_sections_survive_tar_compaction() {
        let mixed = "=== A ===\n-rw-r--r-- u/g 1 2024-01-01 12:00 f1\n-rw-r--r-- u/g 1 2024-01-01 12:00 f2\n-rw-r--r-- u/g 1 2024-01-01 12:00 f3\n=== B ===\nimportant line\n";
        let out = compact_tar_verbose(mixed);
        assert!(out.contains("=== A ==="), "section A kept: {out}");
        assert!(out.contains("=== B ==="), "section B kept: {out}");
        assert!(out.contains("important line"), "content kept: {out}");
        assert!(out.contains("f2"), "entries kept: {out}");
    }

    #[test]
    fn homogeneous_tar_listing_still_detected() {
        let tar = "-rw-r--r-- user/group 120 2024-01-01 12:00 src/a.rs\n-rw-r--r-- user/group 88 2024-01-01 12:00 src/b.rs\ndrwxr-xr-x user/group 0 2024-01-01 12:00 src/dir/\n";
        assert!(matches!(detect_format(tar), Format::TarVerbose));
    }

    #[test]
    fn detects_tar_verbose() {
        let t = "-rw-r--r-- user/group 1234 2024-01-01 12:34 src/main.rs\n\
                 drwxr-xr-x user/group    0 2024-01-01 12:34 src/\n";
        assert_eq!(detect_format(t), Format::TarVerbose);
    }

    #[test]
    fn sqlite_column_tables_are_not_tar_listings() {
        // Regression: sqlite `-header -column` emits a dash separator line under
        // the header (`----------  ------  -----`). Byte 0 is `-` and bytes 1..10
        // are all `-`, so the old perm-prefix check matched it; two stacked
        // tables gave tarish>=2 and the whole thing folded as a 2-entry tar
        // listing, dropping every column. It must read as Unknown (left alone).
        let t = "competitor  status  score  cost\n\
                 ----------  ------  -----  -----\n\
                 caveman     ok      1.0    0.1\n\
                 ponytail    ok      1.0    0.112\n\
                 \n\
                 competitor  n   avg_cost\n\
                 ----------  --  --------\n\
                 control     9   0.096\n\
                 tokenade    9   0.109\n";
        assert_eq!(detect_format(t), Format::Unknown);
    }

    #[test]
    fn detects_unzip_list() {
        let t = "Archive: foo.zip\n  Length      Date    Time    Name\n---------  ---------- -----   ----\n    12345  2024-01-01 12:34   src/main.rs\n---------                     -------\n    12345                     1 file\n";
        assert_eq!(detect_format(t), Format::UnzipList);
    }

    #[test]
    fn tar_paths_extracted() {
        let t = "-rw-r--r-- user/group 1234 2024-01-01 12:34 src/main.rs\n\
                 drwxr-xr-x user/group    0 2024-01-01 12:34 src/\n\
                 -rw-r--r-- user/group  789 2024-01-01 12:34 README.md\n";
        let out = compact(t);
        assert!(out.contains("src/main.rs"));
        assert!(out.contains("README.md"));
        // chrome columns dropped
        assert!(!out.contains("2024-01-01"));
        assert!(!out.contains("user/group"));
    }

    #[test]
    fn unzip_paths_extracted() {
        let t = "Archive: foo.zip\n  Length      Date    Time    Name\n---------  ---------- -----   ----\n    12345  2024-01-01 12:34   src/main.rs\n      99  2024-01-01 12:34   README.md\n---------                     -------\n    12444                     2 files\n";
        let out = compact(t);
        assert!(out.contains("src/main.rs"));
        assert!(out.contains("README.md"));
        assert!(!out.contains("12345"));
    }

    #[test]
    fn long_listing_grouped_by_top_dir() {
        let mut t = String::new();
        for i in 0..MAX_ENTRIES + 100 {
            let dir = if i % 3 == 0 { "src" } else { "tests" };
            let _ = writeln!(
                t,
                "-rw-r--r-- user/group 1234 2024-01-01 12:34 {dir}/file_{i}.rs"
            );
        }
        let out = compact(&t);
        assert!(out.contains("src/"));
        assert!(out.contains("tests/"));
        assert!(out.contains("entries grouped by directory"));
    }

    #[test]
    fn unknown_format_passes_through() {
        let t = "totally not an archive listing\nlooks like a poem instead";
        let out = compact(t);
        assert_eq!(out, t);
    }

    #[test]
    fn every_entry_its_own_root_comes_through_verbatim_and_in_order() {
        // Past MAX_ENTRIES the renderer groups by top-level dir. When each
        // entry IS its own top-level dir every group holds exactly one path,
        // which must print verbatim, in input order. That is also the shape
        // that made the grouping quadratic — one `paths.iter().find` per
        // singleton root, MEASURED at 2 507 ms for 1,28 Mo — so the
        // first-seen map that replaced the scan has to answer identically.
        let mut paths: Vec<String> = (0..=MAX_ENTRIES)
            .map(|i| format!("solo_{i}/file.txt"))
            .collect();
        paths.extend((0..3).map(|i| format!("shared/f{i}.txt")));
        let out = render_paths("zip", &paths);
        let rendered: Vec<&str> = out.lines().skip(2).collect();
        assert_eq!(rendered.len(), MAX_ENTRIES + 2, "one line per root: {out}");
        for (i, line) in rendered.iter().take(MAX_ENTRIES + 1).enumerate() {
            assert_eq!(*line, format!("  solo_{i}/file.txt"), "entry {i} in {out}");
        }
        // A root with several entries still folds to its count.
        assert_eq!(rendered[MAX_ENTRIES + 1], "  shared/  (3 entries)");
    }

    #[test]
    fn savings_on_huge_listing() {
        let mut t = String::new();
        for i in 0..2000 {
            let _ = writeln!(
                t,
                "-rw-r--r-- user/group 12345 2024-01-01 12:34 deeply/nested/path/dir/file_{i}.txt"
            );
        }
        let (out, sav) = compact_with_savings(&t);
        assert!(out.contains("entries grouped"));
        assert!(sav.percent() > 90.0, "got {:?}", sav);
    }

    #[test]
    fn empty_and_garbage_safe() {
        assert_eq!(compact(""), "");
        let _ = compact("\x00random binary in the middle\nrandom\nrandom");
    }
}

#[cfg(test)]
mod a_comment_is_not_a_path {
    //! The plain-listing branch asked for lines containing `/`. A `//`
    //! comment contains one — its own marker. Measured 2026-08-26,
    //! **2 287 source files** in this corpus were claimed as archive
    //! listings on that alone, 42.9 MB of them, and delivered under a
    //! `[tokenade:archive] plain-listing — N entries` banner with their doc
    //! comments regrouped as directory entries.

    const LICENCE_HEADER: &str = "// ------------------------------------------------------------\n// Copyright 2021, the authors.\n// Licensed under either of Apache License, Version 2.0, or MIT license, at your option.\n// Please see the LICENSE-APACHE or LICENSE-MIT files in this distribution for details.\n// ------------------------------------------------------------\n\n//! Defines the structure of a stack graph.\n//!\n//! This module contains all of the types you need.\n//! stack graph.\n\nuse std::fmt;\n\npub struct Graph;\n";

    #[test]
    fn a_rust_licence_header_is_not_a_listing() {
        assert_eq!(
            format!("{:?}", super::detect_format(LICENCE_HEADER)),
            "Unknown",
            "seven `//` lines are not seven paths"
        );
    }

    #[test]
    fn the_file_comes_back_whole() {
        let (kind, out, _) = crate::auto::compact(LICENCE_HEADER);
        assert_ne!(format!("{kind:?}"), "ArchiveListing");
        assert!(out.contains("Copyright 2021"), "the header must survive");
        assert!(out.contains("pub struct Graph"), "so must the code");
    }

    #[test]
    fn a_real_plain_listing_still_detects() {
        let listing = "src/main.rs\nsrc/lib.rs\ntests/e2e/mod.rs\ndocs/guide/intro.md\nCargo.toml\nsrc/util/mod.rs\n";
        assert_eq!(format!("{:?}", super::detect_format(listing)), "Plain");
    }

    /// A line that MENTIONS a path is not a listing entry. `contains('/')`
    /// could not tell the two apart, and every previous repair of this
    /// predicate bolted on another substring exception instead of asking the
    /// question the detector is actually about.
    #[test]
    fn prose_that_mentions_a_path_is_not_a_listing() {
        let mut prose = String::new();
        for i in 0..40 {
            prose.push_str(&format!(
                "processing /some/path/{i}/module-{i}.ts with {i} symbols and {i} warnings\n"
            ));
        }
        assert_eq!(format!("{:?}", super::detect_format(&prose)), "Unknown");
    }

    /// And a real listing still detects — including the metadata-column form
    /// (`<size> <path>`, as `rsync --list-only` and `find -printf` print it),
    /// where the path is the LAST field rather than the only one.
    #[test]
    fn a_listing_still_detects_with_or_without_a_leading_column() {
        let mut bare = String::new();
        let mut with_size = String::new();
        for i in 0..40 {
            bare.push_str(&format!("/var/lib/app/data/dir{}/file-{i}.bin\n", i % 7));
            with_size.push_str(&format!(
                "4096 /var/lib/app/data/dir{}/file-{i}.bin\n",
                i % 7
            ));
        }
        assert_eq!(format!("{:?}", super::detect_format(&bare)), "Plain");
        assert_eq!(format!("{:?}", super::detect_format(&with_size)), "Plain");
    }

    #[test]
    fn a_pip_progress_bar_is_not_a_comment() {
        // A first pass also rejected `--`, and pip's progress bar opens on
        // four dashes: `---------- 123.4/123.4 kB 14.2 MB/s eta 0:00:00`.
        // That took a real `pip install` fold from 90 % down to 51 %. The
        // rule is the slash-comment markers and nothing else.
        let mut pip = String::new();
        for i in 0..8 {
            pip.push_str(&format!(
                "Collecting pkg-{i}\n  Downloading pkg_{i}-1.0.0-py3-none-any.whl (12{i} kB)\n"
            ));
            pip.push_str("     ---------------------------------------- 123.4/123.4 kB 14.2 MB/s eta 0:00:00\n");
        }
        let bar = "---------------------------------------- 123.4/123.4 kB 14.2 MB/s eta 0:00:00";
        assert!(
            !bar.starts_with("//") && !bar.starts_with("/*"),
            "the progress bar must not read as a comment"
        );
        assert!(
            crate::pkginstall::detect(&pip),
            "pip output stays pip output"
        );
    }
}
