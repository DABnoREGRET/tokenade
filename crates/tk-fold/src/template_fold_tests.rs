// The tests of `template_fold`.
//
// Spliced in by `include!` from `template_fold.rs`: the same module, hence the same
// private access and the same `super::` paths, without a single visibility line
// changed. This is the form this repo already uses for 33 other modules.
//
// A `//!` is impossible here: a file spliced in by `include!` is not a
// module, and an INNER documentation comment has nothing to qualify.

#[cfg(test)]
mod constant_slot_tests {
    use super::*;

    /// **A slot whose value NEVER changes is not a
    /// variable.**
    ///
    /// Measured by wiring this fold onto every command:
    /// `2026-09-08 agent a lu le fichier numero 3` came out as
    /// `{}-{}-{} agent a lu le fichier numero {}` with `2026 · 09 · 08 · 3`
    /// repeated under each of the fourteen lines. The reader loses a date, and the
    /// output grows by the weight of a constant repeated fourteen times.
    #[test]
    fn a_slot_that_never_varies_stays_in_the_template() {
        let text: String = (0..6)
            .map(|i| format!("2026-09-08 agent a lu le fichier numero {i} du lot\n"))
            .collect();
        let out = fold_templated_runs(&text);
        assert_ne!(out, text, "the fixture does not reach the fold it tests");
        assert!(
            out.contains("2026-09-08 agent a lu le fichier numero {} du lot  (×6)"),
            "the constant date was taken out of the template:\n{out}"
        );
        // And only the varying value is listed, not the date next to it.
        assert!(out.contains("\n  3\n"), "{out}");
        assert!(!out.contains("2026 · 09"), "{out}");
        // Nothing is lost: the six values are there.
        for i in 0..6 {
            assert!(out.contains(&format!("\n  {i}")), "value {i} lost:\n{out}");
        }
    }
}

#[cfg(test)]
mod tests {

    /// The folded shape explains itself, once.
    ///
    /// The fold rewrites a run into `sentence (×N)` and lists the values
    /// under it. Measured 2026-09-03 through `read --prompt` on a 68 000
    /// token handbook: 121 lines of answer and not one saying what `{}`
    /// stands for, so `(paragraph {})` reads as literal text. Every other
    /// surface that rewrites its input says so on the spot.
    ///
    /// Four fixtures that oppose: a folding document gets exactly one
    /// legend; a document with several prose runs still gets exactly one;
    /// a document that does not fold gets none and comes back unchanged;
    /// and a document whose saving is smaller than the legend is left
    /// alone rather than made bigger by explaining a fold that barely was.
    #[test]
    fn the_folded_shape_explains_itself_once() {
        // The sentence must be at least [`MIN_TEMPLATE_CHARS`] long once
        // its slots are removed, or nothing folds and the fixture proves
        // nothing — the first version of this test used a 21-character
        // sentence and measured the gate rather than the legend.
        let repetitive: String = (0..60)
            .map(|i| format!(
                "warning: could not resolve the dependency `dep{i}` while building the route \"/app/routes/page{i}.tsx\"\n"
            ))
            .collect();
        let out = fold_if_it_pays(&repetitive);
        assert_eq!(
            out.matches("[tokenade:template-fold]").count(),
            1,
            "a folded document carries {} legends, not one:\n{out}",
            out.matches("[tokenade:template-fold]").count()
        );
        assert!(
            out.contains("(×60)"),
            "the fold did not fire, so this proves nothing:\n{out}"
        );

        // Several prose runs separated by fences: still one legend.
        let mixed = format!(
            "{repetitive}```\nlet a = 1;\nlet b = 2;\n```\n{repetitive}```\nlet c = 3;\n```\n{repetitive}"
        );
        let out = fold_if_it_pays_outside_code(&mixed);
        assert_eq!(
            out.matches("[tokenade:template-fold]").count(),
            1,
            "three prose runs produced {} legends",
            out.matches("[tokenade:template-fold]").count()
        );
        assert!(
            out.contains("let a = 1;"),
            "the fenced code was eaten:\n{out}"
        );

        // Prose that merely rhymes does not fold, and gains no legend.
        let prose = "The pipeline runs nightly.\nOperators are paged on the second failure.\n                     Cache warm-up is about ninety seconds.\n";
        assert_eq!(fold_if_it_pays(prose), prose, "unfolded prose was changed");
        assert_eq!(fold_if_it_pays_outside_code(prose), prose);

        // And a fold whose saving is smaller than the legend is refused:
        // explaining a fold must never cost more than the fold returns.
        let barely: String = (0..3)
            .map(|i| format!(
                "warning: could not resolve the dependency `dep{i}` while building the route \"/app/routes/page{i}.tsx\"\n"
            ))
            .collect();
        let out = fold_if_it_pays(&barely);
        assert!(
            out.len() <= barely.len(),
            "explaining the fold made the output bigger than the input: {} -> {}",
            barely.len(),
            out.len()
        );
    }
    use super::*;

    #[test]
    fn a_code_fence_is_never_folded() {
        let mut doc = String::from("# Release journal\n\n");
        for t in ["ABC-1", "ABC-2", "ABC-3", "ABC-4", "ABC-5"] {
            doc.push_str(&format!(
                "A rollback was scheduled for \"{t}\" after the upstream provider returned an error.\n"
            ));
        }
        doc.push_str("\n```rust\nlet a = compute(1);\nlet b = compute(2);\nlet c = compute(3);\nlet d = compute(4);\n```\n");
        let out = fold_if_it_pays_outside_code(&doc);
        for line in [
            "let a = compute(1);",
            "let b = compute(2);",
            "let c = compute(3);",
            "let d = compute(4);",
        ] {
            assert!(out.contains(line), "code rewritten: {out}");
        }
        assert!(
            out.contains("(×5)"),
            "the journal entries did not fold:\n{out}"
        );
        for t in ["ABC-1", "ABC-5"] {
            assert!(out.contains(t), "lost the value {t}:\n{out}");
        }
    }

    /// CommonMark indented code is the third fence form. Same hole as
    /// `~~~`: two adjacent indented lines share a shape and
    /// would be rewritten into a template if treated as prose.
    #[test]
    fn an_indented_code_block_is_never_folded() {
        let mut doc = String::from("# Release journal\n\n");
        for t in ["ABC-1", "ABC-2", "ABC-3", "ABC-4", "ABC-5"] {
            doc.push_str(&format!(
                "A rollback was scheduled for \"{t}\" after the upstream provider returned an error.\n"
            ));
        }
        doc.push('\n');
        for n in 1..=4 {
            doc.push_str(&format!("    let v{n} = compute({n});\n"));
        }
        let out = fold_if_it_pays_outside_code(&doc);
        for line in [
            "    let v1 = compute(1);",
            "    let v2 = compute(2);",
            "    let v3 = compute(3);",
            "    let v4 = compute(4);",
        ] {
            assert!(out.contains(line), "indented code rewritten: {out}");
        }
        assert!(
            out.contains("(×5)"),
            "the journal entries did not fold:\n{out}"
        );
        for t in ["ABC-1", "ABC-5"] {
            assert!(out.contains(t), "lost the value {t}:\n{out}");
        }
    }

    #[test]
    fn a_document_with_nothing_repetitive_comes_back_byte_identical() {
        let doc = "# Title\n\nSome prose here.\n\n```sh\nls -la\n```\n\nMore prose.\n";
        assert_eq!(fold_if_it_pays_outside_code(doc), doc.to_string());
    }

    #[test]
    fn one_sentence_per_route_is_said_once() {
        let mut log = String::new();
        for r in ["alpha", "beta", "gamma", "delta"] {
            log.push_str(&format!(
                " ⚠ Next.js can't recognize the exported `dynamic` field in \"/api/{r}/route\", it may be re-exported\n"
            ));
        }
        let out = fold_templated_runs(&log);
        assert!(out.contains("(×4)"), "not folded:\n{out}");
        assert_eq!(
            out.matches("can't recognize").count(),
            1,
            "sentence repeated:\n{out}"
        );
        for r in ["alpha", "beta", "gamma", "delta"] {
            assert!(out.contains(r), "lost the value {r}:\n{out}");
        }
        assert!(out.len() < log.len(), "folding must actually shrink");
    }

    #[test]
    fn two_facts_that_share_a_template_both_survive() {
        // The reason this is narrow: same shape, opposite meaning.
        let log = "backups are retained for 400 days in this account tier\n\
                   backups are retained for 30 days in this account tier\n\
                   backups are retained for 7 days in this account tier\n";
        let out = fold_templated_runs(log);
        for n in ["400", "30", "7"] {
            assert!(out.contains(n), "lost the value {n}:\n{out}");
        }
    }

    #[test]
    fn a_pairing_is_not_shuffled() {
        let log = "resolved module `a` from package \"one\" successfully today\n\
                   resolved module `b` from package \"two\" successfully today\n\
                   resolved module `c` from package \"three\" successfully today\n";
        let out = fold_templated_runs(log);
        assert!(out.contains("`a` · \"one\""), "pairing lost:\n{out}");
        assert!(out.contains("`c` · \"three\""), "pairing lost:\n{out}");
    }

    #[test]
    fn unrepeated_output_is_returned_untouched() {
        let log = "one line about something\nanother line about something else\n";
        assert_eq!(fold_templated_runs(log), log.to_string());
    }

    #[test]
    fn ordinary_prose_is_not_folded_for_a_crumb() {
        // Three sentences that rhyme are not a generated list. The gate is
        // what keeps folding out of documents it cannot help.
        let prose = "The committee met in \"Brussels\" on 3 March to review the file.\n\
                     Nothing else of note happened that quarter, and the minutes are short.\n\
                     The committee met in \"Geneva\" on 4 April to review the file.\n\
                     A long paragraph of unrelated narrative text sits here to carry weight.\n\
                     The committee met in \"Vienna\" on 5 May to review the file.\n";
        assert_eq!(fold_if_it_pays(prose), prose.to_string());
    }

    #[test]
    fn short_lines_are_left_alone() {
        // Below the template floor: folding would explain more than it saves.
        let log = "ok 1\nok 2\nok 3\nok 4\n";
        assert_eq!(fold_templated_runs(log), log.to_string());
    }

    /// **The REAL capture, not a made-up sentence.**
    ///
    /// `linux/npm_build` carries two series of 19 Next.js warnings
    /// identical up to the route — 6,850 bytes, 24 % of everything this
    /// capture still lets through, and the 1st shape of the residue census.
    /// This test says whether the fold groups them; if it does, it is the ROUTING
    /// that does not call it.
    #[test]
    fn the_real_next_build_warnings_are_folded() {
        let body = std::fs::read_to_string(
            crate::source_guard::fixtures_dir().join("linux/npm_build.txt"),
        )
        .expect("the corpus npm capture");
        let warnings: Vec<&str> = body
            .lines()
            .filter(|l| l.contains("can't recognize the exported"))
            .collect();
        assert_eq!(warnings.len(), 128, "the capture no longer carries these warnings");

        let input = warnings.join("\n");
        let (out, absorbed) = fold_templated_runs_counted(&input);
        assert_eq!(absorbed, 127, "the 128 warnings are ONE sentence:\n{out}");
        assert!(out.len() * 2 < input.len(), "{} contre {}", out.len(), input.len());
        // Every route survives: it is the only information in these lines.
        for w in &warnings {
            let route = w.split('"').nth(1).expect("the quoted route");
            assert!(out.contains(route), "route perdue : {route}\n{out}");
        }
    }
    // `the_npm_compactor_folds_them_too` moved to the `fold` target:
    // it goes through `filter::filter_command`, which the fold layer can
    // no longer name since `tk-fold` was extracted.


    /// A name given by a MACHINE is a value, not prose.
    ///
    /// `ioreg` describes a tree where every node repeats the same
    /// forty-character sentence and only changes name, class and
    /// identifier. Measured 2026-09-09 on the corpus capture:
    /// **42 % repeated text** (1,593 of 3,769 bytes, seven words
    /// present on more than four lines out of five) and **zero bytes
    /// folded**, because neither `RootDomainUserClient` nor `0x10000021a`
    /// were recognised as values.
    ///
    /// The fold loses nothing: the test requires that EVERY identifier in the
    /// capture comes out of the fold.
    #[test]
    fn a_machine_named_node_is_a_value_and_the_tree_folds() {
        let body = std::fs::read_to_string(
            crate::source_guard::fixtures_dir().join("macos/ioreg_short.txt"),
        )
        .expect("the corpus ioreg capture");
        let out = super::fold_if_it_pays(&body);
        assert!(
            out.len() * 4 < body.len() * 3,
            "the ioreg tree does not fold: {} -> {} bytes",
            body.len(),
            out.len()
        );
        for id in super::slot_re()
            .find_iter(&body)
            .map(|m| m.as_str().to_string())
            .filter(|v| v.starts_with("0x"))
        {
            assert!(out.contains(&id), "identifier {id} vanished from the fold");
        }
    }

    /// The three shapes added to the alphabet are each NECESSARY.
    ///
    /// Each fixture differs from one line to the next only by the
    /// shape under test: if it stops being a value, the lines stay
    /// different templates and nothing folds. A fixture that
    /// carried all three at once would not say which one carries the fold.
    #[test]
    fn each_widened_value_shape_carries_a_fold_on_its_own() {
        let folds = |lines: Vec<String>| {
            let body = lines.join("\n") + "\n";
            let out = super::fold_if_it_pays(&body);
            assert!(
                out.len() < body.len(),
                "nothing folded on\n{body}\n-> {out}"
            );
        };
        let node = |name: &str, id: usize| {
            format!("| +-o {name}  <class {name}, id 0x1000002{id:02x}, !registered, !matched, active, busy 0, retain 5>")
        };
        // Mixed case starting with an internal lowercase: the NAME
        // varies, the identifier is CONSTANT.
        // Each name appears only ONCE: a fixture that repeated
        // four names three times would fold through plain repetition of
        // the line, and say nothing about the pattern. MEASURED: it survived.
        folds(
            [
                "RootDomainUserClient", "AppleKeyStore", "NvmeController",
                "SerialPort", "DisplayPipe", "AudioEngine", "GraphicsDevice",
                "KeyboardHid", "PowerManager", "ThunderboltPort",
                "BluetoothHost", "NetworkStack",
            ]
            .iter()
            .map(|n| node(n, 0x1a))
            .collect(),
        );
        // Mixed case starting with an ACRONYM — the shape the
        // first version of the pattern missed.
        folds(
            [
                "IOPMrootDomain", "IOACPIPlatform", "USBHub", "HIDKeyboard",
                "ACPINode", "PCIBridge", "SMCDevice", "NVMeDrive", "AHCIPort",
                "GPUChannel", "XHCIPort", "UARTPort",
            ]
            .iter()
            .map(|n| node(n, 0x1a))
            .collect(),
        );
        // Hexadecimal literal: here the name is constant and the identifier
        // is the ONLY thing that changes.
        folds((0..12).map(|i| node("RootDomainUserClient", i)).collect());
        // And the word boundary holds: the end of an identifier is not an
        // address. The fold has no final pass that masks digits,
        // so this is WHERE the case can be observed.
        assert!(
            super::slot_re().find("foo0x1f").is_none(),
            "the end of an identifier was taken for a value"
        );
    }

    /// **What all the values share belongs to the template.**
    ///
    /// The template says the sentence once; the common root of the values was
    /// rewritten under every line. Measured on the real `mount` capture:
    /// the two columns of the fifteen `squashfs` mounts share 21 and 20
    /// characters — 615 bytes — and the delivered capture goes from 5,864 to 5,220.
    #[test]
    fn a_shared_root_is_said_once_in_the_template() {
        let names = [
            "bare_5", "dalfox_1313", "gnome-3-28-1804_198", "somiibo_98", "core20_2769",
            "gtk-common-themes_1535", "snapd_27710", "core22_2437", "core18_2999",
            "core20_2866", "wine-platform-5-stable_18", "wine-platform-runtime_409",
            "core22_2411", "dalfox_1438", "snapd_27738",
        ];
        let text: String = names
            .iter()
            .map(|n| {
                let (pkg, ver) = n.rsplit_once('_').expect("nom_version");
                format!(
                    "/var/lib/snapd/snaps/{n}.snap on /var/lib/snapd/snap/{pkg}/{ver} \
                     type squashfs (ro,nodev,relatime)\n"
                )
            })
            .collect();
        let (out, absorbed) = fold_templated_runs_counted(&text);
        assert_eq!(absorbed, 14, "the run was not folded: {out}");
        assert!(
            out.contains("/var/lib/snapd/snaps/{}.snap on /var/lib/snapd/snap/{} type squashfs"),
            "the root and the suffix were not lifted: {out}"
        );
        assert!(out.contains("\n  bare_5 · bare/5\n"), "the value is not shortened: {out}");
        // Nothing disappears: every name recomposes.
        for n in names {
            assert!(out.contains(n), "value {n} lost: {out}");
        }
        assert!(out.len() < text.len(), "the fold must save: {} vs {}", out.len(), text.len());
    }

    /// The three safeguards of `shared_affix`, each on its own case.
    #[test]
    fn a_shared_affix_never_empties_a_value_nor_cuts_a_character() {
        // A value that is the prefix of another, and whose common prefix
        // DOES end on a separator: lifting `aaa/bbb/` would leave an
        // EMPTY cell under the template, which cannot be read back. Without the
        // overlap bound, the separator bound lets this case through — that is
        // why both are needed, and why the fixture must cross one
        // to test the other.
        assert_eq!(shared_affix(&["aaa/bbb/", "aaa/bbb/ccc"]), (0, 0));
        // Too short to be worth the reading effort.
        assert_eq!(shared_affix(&["abxx", "abyy"]), (0, 0));
        // Long enough, AND the cut falls on a separator.
        assert_eq!(shared_affix(&["pre/fix/aa", "pre/fix/bb"]), (8, 0));
        // A common suffix too.
        assert_eq!(shared_affix(&["aa.suffix", "bb.suffix"]), (0, 7));
        // **Never in the middle of a word**: eleven `ioreg` identifiers sharing
        // `0x1000001` must not be cut there, otherwise `0x10000011a`
        // no longer exists anywhere in the output.
        assert_eq!(shared_affix(&["0x10000011a", "0x1000001 21a"]), (0, 0));
        // The cut falls on a character boundary: `é` is two bytes
        // and the values share only its first.
        let (h, _) = shared_affix(&["aaa/aéx", "aaa/aèy"]);
        assert_eq!(h, 4, "the cut went through a character or missed the separator");
        // A single value has nothing to share.
        assert_eq!(shared_affix(&["seule"]), (0, 0));
    }
}
