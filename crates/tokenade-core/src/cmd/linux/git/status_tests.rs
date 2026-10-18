// The tests of `status`.
//
// Spliced by `include!` from `status.rs`: the same module, so the same
// private access and the same `super::` paths, without a single visibility
// line changed. It is the shape this repo already uses for 33 other modules.
//
// A `//!` is impossible here: a file spliced by `include!` is not a
// module, and an INNER doc comment has nothing to qualify.

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(slice: &[&str]) -> Vec<String> {
        slice.iter().map(|s| s.to_string()).collect()
    }

    /// `git status` is the command an agent runs more than any other, and off
    /// English it was folded by nothing at all. git localises its
    /// human-readable output, so `nothing to commit` never matched and every
    /// French line looked "substantive"; the text went to the generic path,
    /// which returned it unchanged. Measured 2026-09-01: 155 -> 19 bytes under
    /// `LC_ALL=C` against 185 -> 185 under `fr_FR.UTF-8`, same repository, same
    /// second.
    ///
    /// The structural answer needs no language: staged, unstaged, untracked
    /// and conflicted entries are ALL tab-indented, and git prints no empty
    /// section — so zero tab-indented lines IS "nothing to commit".
    #[test]
    fn a_clean_tree_folds_in_a_language_we_cannot_read() {
        let fr = "Sur la branche main\n\
                  Votre branche est en avance sur 'origin/main' de 2 commits.\n\
                  \u{20}\u{20}(utilisez \"git push\" pour publier vos commits locaux)\n\
                  \n\
                  rien à valider, la copie de travail est propre\n";
        let out = dispatch(&argv(&["git", "status"]), fr);
        assert!(
            out.len() < fr.len(),
            "a clean French status must fold, got {} bytes from {}: {out}",
            out.len(),
            fr.len()
        );
        assert!(
            out.contains("Sur la branche main"),
            "the branch line is actionable and must survive: {out}"
        );
        assert!(
            out.contains("clean working tree"),
            "the fold must say so in the vocabulary the English path uses: {out}"
        );
        assert!(
            !out.contains("rien à valider"),
            "git's translated closing summary is the boilerplate being replaced: {out}"
        );
        assert!(
            out.contains("en avance sur 'origin/main' de 2 commits"),
            "ahead/behind is actionable and is kept — the English fold drops it: {out}"
        );
        assert!(
            !out.contains("utilisez"),
            "the hint line is noise for an agent that knows git: {out}"
        );

        // The English path still answers the shorter way, and both agree that
        // the tree is clean.
        let en = "On branch main\n\
                  Your branch is up to date with 'origin/main'.\n\
                  \n\
                  nothing to commit, working tree clean\n";
        assert_eq!(dispatch(&argv(&["git", "status"]), en), "clean working tree");
    }

    /// The other half: a status that DOES carry files must not be read as
    /// clean, in any language, and no file may be dropped.
    #[test]
    fn a_dirty_tree_off_english_still_lists_every_file() {
        let fr = "Sur la branche main\n\
                  Modifications qui ne seront pas validées :\n\
                  \u{a0} (utilisez \"git add\" ...)\n\
                  \tmodifié :         src/a.rs\n\
                  \tmodifié :         src/b.rs\n\
                  \n\
                  Fichiers non suivis:\n\
                  \tsrc/c.rs\n";
        let out = dispatch(&argv(&["git", "status"]), fr);
        // Entries of one section that share a directory are grouped
        // (`src/: a.rs b.rs`), so assert on the parts rather than on a
        // rendering — but assert that EVERY file is still named.
        for f in ["a.rs", "b.rs", "c.rs", "src/"] {
            assert!(out.contains(f), "{f} must survive the fold: {out}");
        }
        assert!(
            out.contains("3 file(s)"),
            "the count must match what was folded: {out}"
        );
        assert!(
            !out.contains("clean"),
            "three modified files is not a clean tree: {out}"
        );
    }

    /// A fold that GROWS the payload is not a fold. The replacement phrase is
    /// eighteen bytes, so a status whose branch name and closing summary are
    /// shorter than that comes out longer than it went in — and then the honest
    /// answer is to leave the text alone.
    #[test]
    fn a_fold_that_would_grow_the_output_is_refused() {
        let tiny = "Sur la branche m\n\nok\n";
        let out = dispatch(&argv(&["git", "status"]), tiny);
        assert!(
            out.len() <= tiny.len(),
            "folding must never cost bytes: {} -> {} ({out})",
            tiny.len(),
            out.len()
        );
    }

    /// Text that is not a status layout at all keeps going to the generic
    /// path — the marker must not be stamped on something we did not fold.
    #[test]
    fn a_non_status_body_is_left_to_the_generic_path() {
        let noise = "some tool wrote this\nand this\nand this too\n";
        let out = dispatch(&argv(&["git", "status"]), noise);
        assert!(
            !out.contains("clean working tree"),
            "nothing here was recognised as a status: {out}"
        );
    }

    #[test]
    fn porcelain_summarises_with_counts() {
        let s = " M foo.rs\nA  bar.rs\n?? new.txt\n";
        let out = dispatch(&argv(&["git", "status", "--porcelain"]), s);
        assert!(out.contains("1 staged, 1 unstaged, 1 untracked"));
        assert!(out.contains("foo.rs"));
    }

    fn many(n: usize) -> String {
        let mut s = String::new();
        for i in 0..n {
            s.push_str(&format!(" M crates/tokenade-core/src/mod_a{i}.rs\n"));
            s.push_str(&format!("?? crates/tokenade-core/src/mod_b{i}.rs\n"));
        }
        s
    }

    /// The fold is LOSSLESS on names: every file the agent might edit is still
    /// spelled out. That is what separates it from the file-list compactors,
    /// which answer "how many, and where".
    #[test]
    fn porcelain_factors_the_directory_and_keeps_every_name() {
        let src = many(20);
        let out = dispatch(&argv(&["git", "status", "--porcelain"]), &src);
        for i in 0..20 {
            assert!(out.contains(&format!("mod_a{i}.rs")), "a modified file is missing: {i}");
            assert!(out.contains(&format!("mod_b{i}.rs")), "an untracked file is missing: {i}");
        }
        assert!(
            out.len() * 2 < src.len(),
            "barely folded: {} of {}",
            out.len(),
            src.len()
        );
    }

    /// A modified file and an untracked one in the same directory are
    /// different answers and never share a line.
    #[test]
    fn porcelain_groups_on_the_status_code_too() {
        let out = dispatch(&argv(&["git", "status", "--porcelain"]), &many(20));
        let m = out
            .lines()
            .find(|l| l.trim_start().starts_with("M "))
            .expect("a modified group");
        assert!(m.contains("mod_a0.rs"), "the modified group is empty: {m}");
        assert!(
            !m.contains("mod_b0.rs"),
            "an untracked file landed in the modified group: {m}"
        );
    }

    /// The flush happens AT the header and not at the end: a file emitted
    /// after the header of the next section is filed under the wrong one,
    /// and the group keys alone do not catch it because they differ anyway.
    #[test]
    fn every_entry_stays_under_its_own_header() {
        let fr = "Sur la branche main\n\
            Modifications qui ne seront pas validées :\n\
            \tmodifié :         src/a.rs\n\
            \tmodifié :         src/b.rs\n\
            \n\
            Fichiers non suivis :\n\
            \tsrc/c.rs\n";
        let out = dispatch(&argv(&["git", "status"]), fr);
        let head = out.find("Fichiers non suivis").expect("header kept");
        let a = out.find("a.rs").expect("a.rs kept");
        let c = out.rfind("c.rs").expect("c.rs kept");
        assert!(a < head, "a modified file fell under the untracked header: {out}");
        assert!(c > head, "an untracked file fell above its header: {out}");
    }

    /// A colon is a legal byte in a filename. The marker is the FIRST `": "`
    /// of the line, never the last. Reading the last one puts half the path
    /// INSIDE the marker, and the re-joined line looks right — it is the
    /// grouping key that gives it away, since a marker carrying a directory
    /// can never group with the neighbour that shares that directory.
    #[test]
    fn a_colon_inside_a_path_is_not_read_as_the_marker() {
        let fr = "Sur la branche main\n\
            Modifications qui ne seront pas validées :\n\
            \tmodifié :         notes/a: b.md\n\
            \tmodifié :         notes/c.md\n";
        let out = dispatch(&argv(&["git", "status"]), fr);
        assert!(!out.contains("modifié : notes/a:"), "path leaked into the marker: {out}");
        assert!(out.contains("b.md") && out.contains("c.md"), "got: {out}");
    }

    /// A group of one is not a group. `src/: a.rs` is two bytes longer than
    /// `src/a.rs`, so the lone member is rendered flat — and a file at the
    /// repository root keeps its bare name rather than gaining a `./`.
    #[test]
    fn a_lone_file_is_not_given_a_group_of_its_own() {
        let fr = "Sur la branche main\n\
            Modifications qui ne seront pas validées :\n\
            \tmodifié :         src/one.rs\n\
            \tmodifié :         doc/a.md\n\
            \tmodifié :         doc/b.md\n\
            \tmodifié :         racine.rs\n";
        let out = dispatch(&argv(&["git", "status"]), fr);
        assert!(out.contains("modifié : src/one.rs"), "lone file grouped: {out}");
        assert!(out.contains("modifié : racine.rs"), "root file gained a ./: {out}");
        assert!(!out.contains("./"), "a phantom ./ survived: {out}");
        assert!(out.contains("modifié : doc/: a.md b.md"), "real group lost: {out}");
    }

    /// A rename carries two paths; folding half of it would leave the reader
    /// guessing which half moved.
    #[test]
    fn porcelain_leaves_a_rename_whole() {
        let mut src = many(20);
        src.push_str("R  docs/a.md -> docs/b.md\n");
        let out = dispatch(&argv(&["git", "status", "--porcelain"]), &src);
        assert!(
            out.contains("docs/a.md -> docs/b.md"),
            "the rename was folded: {out}"
        );
    }

    /// On a handful of entries the grouped form costs more than it saves (one
    /// line per file either way, plus a `dir/:` on each), so the flat form
    /// stays. Whichever is shorter wins.
    #[test]
    fn porcelain_keeps_the_flat_form_when_grouping_costs_more() {
        let src = " M a/b/c/one.rs
?? d/e/f/two.rs
 M g/h/i/three.rs
";
        let out = dispatch(&argv(&["git", "status", "--porcelain"]), src);
        assert!(
            out.contains("a/b/c/one.rs"),
            "the path was split though grouping saves nothing: {out}"
        );
    }

    #[test]
    fn porcelain_with_branch_header() {
        let s = "## main...origin/main\n M foo.rs\n";
        let out = dispatch(&argv(&["git", "status", "-sb"]), s);
        assert!(out.starts_with("[main...origin/main]"));
    }

    #[test]
    fn porcelain_v2_extracts_branch() {
        let s = "# branch.oid abc123\n# branch.head feature/x\n# branch.upstream origin/feature/x\n1 .M N... 100644 100644 100644 abc def foo.rs\n";
        let out = dispatch(&argv(&["git", "status", "--porcelain=v2"]), s);
        assert!(out.contains("[feature/x]"));
        assert!(out.contains("(↑origin/feature/x)"));
        assert!(out.contains("foo.rs"));
    }

    #[test]
    fn porcelain_z_decoded_back_to_newlines() {
        let s = " M foo.rs\0A  bar.rs\0";
        let out = dispatch(&argv(&["git", "status", "--porcelain", "-z"]), s);
        assert!(out.contains("foo.rs"));
        assert!(out.contains("bar.rs"));
    }

    #[test]
    fn detached_head_surfaced() {
        let s = "HEAD detached at v1.2.3\nnothing to commit, working tree clean\n";
        let out = dispatch(&argv(&["git", "status"]), s);
        assert!(out.contains("HEAD detached at v1.2.3"));
    }

    #[test]
    fn rebase_in_progress_marker() {
        // The rebase marker surfaces when there ARE staged/unstaged
        // changes — that's the realistic case during a rebase
        // session where conflicts need resolving. With a fully
        // clean working tree mid-rebase the canonical message
        // wins (existing behaviour from pre-refactor filter.rs).
        let s = "interactive rebasing in progress; onto abc123\n\n\
                 Changes to be committed:\n\
                   modified:  foo.rs\n";
        let out = dispatch(&argv(&["git", "status"]), s);
        assert!(out.contains("rebase-in-progress"), "got: {out}");
    }

    #[test]
    fn non_english_locale_folds_structurally() {
        // Real `git status` output under LANG=fr_FR — the English
        // keyword parser matches nothing, so we must fold by layout
        // instead of dumping to the generic compactor.
        let fr = "Sur la branche main\n\
            Votre branche est en avance sur 'origin/main' de 11 commits.\n\
            \x20\x20(utilisez \"git push\" pour publier vos commits locaux)\n\
            \n\
            Modifications qui ne seront pas validées :\n\
            \x20\x20(utilisez \"git add/rm <fichier>...\" pour mettre à jour)\n\
            \x20\x20(utilisez \"git restore <fichier>...\" pour annuler)\n\
            \tmodifié :         .cargo/config.toml\n\
            \tmodifié :         CLAUDE.md\n\
            \tmodifié :         Cargo.toml\n\
            \n\
            Fichiers non suivis :\n\
            \x20\x20(utilisez \"git add <fichier>...\" pour inclure)\n\
            \tnouveau.txt\n";
        let out = dispatch(&argv(&["git", "status"]), fr);
        // Branch + section headers kept (in the user's language).
        assert!(out.contains("Sur la branche main"), "got: {out}");
        assert!(out.contains("Modifications qui ne seront pas validées"));
        // Every file entry kept — nothing actionable dropped. Grouping factors the
        // directory out of a section, and this one holds a single file there,
        // so the path stays whole: a group of one is not a group.
        assert!(out.contains(".cargo/config.toml"), "got: {out}");
        assert!(out.contains("CLAUDE.md"));
        assert!(out.contains("nouveau.txt"));
        assert!(out.contains("4 file(s)"));
        // Hint noise elided.
        assert!(!out.contains("utilisez"), "hints should be elided: {out}");
        // And it must actually be shorter than the raw input.
        assert!(out.len() < fr.len());
    }

    /// The directory is factored per SECTION. A modified file and an
    /// untracked one in the SAME directory are different answers, and merging
    /// them would say the untracked one is modified.
    #[test]
    fn the_sections_never_merge_their_directories() {
        let fr = "Sur la branche main\n\
            Modifications qui ne seront pas validées :\n\
            \tmodifié :         src/a.rs\n\
            \tmodifié :         src/b.rs\n\
            \n\
            Fichiers non suivis :\n\
            \tsrc/c.rs\n";
        let out = dispatch(&argv(&["git", "status"]), fr);
        assert!(out.contains("modifié : src/: a.rs b.rs"), "got: {out}");
        // `c.rs` is untracked and must not join the modified group.
        assert!(!out.contains("a.rs b.rs c.rs"), "sections merged: {out}");
        assert!(out.contains("c.rs"));
    }

    /// A rename carries two paths and is left whole — half-folding it would
    /// leave the reader guessing which half moved.
    #[test]
    fn a_rename_is_left_whole_in_the_human_form() {
        let fr = "Sur la branche main\n\
            Modifications qui ne seront pas validées :\n\
            \trenommé :         src/old.rs -> src/new.rs\n\
            \tmodifié :         src/a.rs\n";
        let out = dispatch(&argv(&["git", "status"]), fr);
        assert!(out.contains("src/old.rs -> src/new.rs"), "got: {out}");
    }

    /// Grouping is offered, not imposed: one file per directory makes the
    /// grouped form LONGER, and then the flat one is delivered.
    #[test]
    fn the_flat_form_wins_when_grouping_costs() {
        let fr = "Sur la branche main\n\
            Modifications qui ne seront pas validées :\n\
            \tmodifié :         a/one.rs\n\
            \tmodifié :         b/two.rs\n";
        let out = dispatch(&argv(&["git", "status"]), fr);
        assert!(out.contains("modifié : a/one.rs"), "grouping was imposed: {out}");
    }

    /// An untracked entry carries NO marker, and must not be rendered with the
    /// space a marker would have left.
    #[test]
    fn an_unmarked_entry_keeps_its_indent() {
        let fr = "Sur la branche main\n\
            Fichiers non suivis :\n\
            \tsrc/x.rs\n\
            \tsrc/y.rs\n";
        let out = dispatch(&argv(&["git", "status"]), fr);
        assert!(out.contains("  src/: x.rs y.rs"), "got: {out:?}");
        assert!(!out.contains("   src/"), "a phantom marker space: {out:?}");
    }

    /// The most-run command an agent has, given inputs no repository would
    /// produce. Three findings in this series were a parsing path that
    /// crashed (a SIGSEGV on a generated union), starved a machine
    /// (a 400 KB archive taking 3.5 GB) or went quadratic (a
    /// lowercase copy per block), so the class is not hypothetical.
    ///
    /// The bound is loose on purpose: it asserts that nothing here is
    /// quadratic or unbounded, not a particular speed.
    #[test]
    fn adversarial_status_output_stays_bounded() {
        let base = "Sur la branche main\nModifications qui ne seront pas validées :\n";
        let cases: Vec<(&str, String)> = vec![
            ("empty", String::new()),
            ("header only", base.to_string()),
            (
                "100k entries",
                format!("{base}{}", (0..100_000)
                    .map(|i| format!("\tmodifié :         src/d{}/f{i}.rs\n", i % 50))
                    .collect::<String>()),
            ),
            (
                "one huge path",
                format!("{base}\tmodifié :         {}\n", "a/".repeat(50_000)),
            ),
            (
                "unicode paths",
                format!("{base}{}", (0..5_000)
                    .map(|i| format!("\tmodifié :         src/日本語/файл{i}.rs\n"))
                    .collect::<String>()),
            ),
            (
                "no marker at all",
                format!("{base}{}", (0..20_000).map(|i| format!("\tsrc/x{i}.rs\n")).collect::<String>()),
            ),
            (
                "colons inside paths",
                format!("{base}{}", (0..10_000)
                    .map(|i| format!("\tmodifié :         a: b/c{i}: d.rs\n"))
                    .collect::<String>()),
            ),
            (
                "renames",
                format!("{base}{}", (0..20_000)
                    .map(|i| format!("\trenommé :         old{i}.rs -> new{i}.rs\n"))
                    .collect::<String>()),
            ),
            ("bare tabs", "\t\n".repeat(20_000)),
        ];
        for (name, input) in cases {
            let t0 = std::time::Instant::now();
            let out = dispatch(&argv(&["git", "status"]), &input);
            let ms = t0.elapsed().as_millis();
            assert!(ms < 15_000, "{name} took {ms}ms");
            // Nothing may GROW: the anti-inflation floor is what serves a
            // one-entry status verbatim rather than wrapping it in markers.
            assert!(
                out.len() <= input.len() + 200,
                "{name} grew from {} to {}",
                input.len(),
                out.len()
            );
        }
    }

    #[test]
    fn clean_tree_default() {
        let s = "On branch main\nYour branch is up to date with 'origin/main'.\n\nnothing to commit, working tree clean\n";
        let out = dispatch(&argv(&["git", "status"]), s);
        assert!(out.contains("clean working tree"));
    }
}
