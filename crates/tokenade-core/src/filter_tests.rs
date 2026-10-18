// The tests of `filter`.
//
// Spliced by `include!` from `filter.rs`: the same module, so the same
// private access and the same `super::` paths, without a single visibility
// line changed. This is the form this repo already uses for 33 other modules.
//
// A `//!` is impossible here: a file spliced by `include!` is not a module,
// and an INNER doc comment has nothing to qualify.

/// A byte-precise `head`/`tail` must come back byte-exact.
///
/// See [`head_tail_byte_precise`].
#[cfg(test)]
use crate::compress::truncate_smart;
#[cfg(test)]
use crate::fold_count::note_folded;
#[cfg(test)]
use std::fmt::Write;
#[cfg(test)]
mod a_marker_must_not_cost_more_than_the_hole {

    /// The exact price of the marker for a hole of `gap` lines, newline
    /// included. Computed, never copied: `…` is three bytes in UTF-8 and a
    /// constant hand-written here would be a second spelling of the
    /// marker (`no_module_spells_a_marker_of_its_own`).
    fn price(gap: usize) -> usize {
        crate::elision::marker(gap).len() + 1
    }

    /// The case that motivated the rule, measured on the real `pacman -Q`
    /// output delivered by both stages: `… [1 lines elided by tokenade] …`
    /// replaced ONE package line with a longer sentence.
    #[test]
    fn a_hole_smaller_than_its_marker_is_refused() {
        assert!(!crate::elision::a_gap_pays_for_its_marker(
            b"zziplib 0.13.80-1\n".len(),
            1
        ));
    }

    /// The bound, on BOTH sides: the smallest accepted hole, and the one just
    /// below. Without the second, the mutation lowering the threshold survives
    /// — and that is where the output GROWS, which the rule exists to prevent.
    #[test]
    fn the_bound_is_tested_on_both_sides() {
        let p = price(1);
        // The shared guard asks for a tenth, as everywhere else in the
        // product: a hole that saves three bytes out of three hundred is not
        // worth fetching the reserve.
        let smallest_paying = (0..)
            .find(|&h| crate::elision::a_gap_pays_for_its_marker(h, 1))
            .unwrap();
        assert!(
            smallest_paying > p,
            "a hole must save MORE than the marker's price"
        );
        assert!(crate::elision::a_gap_pays_for_its_marker(
            smallest_paying,
            1
        ));
        assert!(!crate::elision::a_gap_pays_for_its_marker(
            smallest_paying - 1,
            1
        ));
    }

    /// A thousand-line hole always pays: the rule must not disarm the fold,
    /// only the holes that save nothing.
    #[test]
    fn a_real_hole_still_pays() {
        assert!(crate::elision::a_gap_pays_for_its_marker(1000 * 20, 1000));
        assert!(crate::elision::a_gap_pays_for_its_marker(20 * 20, 20));
    }

    /// And the refusal KEEPS the lines: not eliding does not mean losing
    /// them. That is what makes the view both truer AND shorter.
    #[test]
    fn a_refused_hole_keeps_its_lines_verbatim() {
        // Two kept lines, a short one in the middle the marker would not pay
        // for. `indices` skips line 1: that is the hole.
        let lines = vec!["kept A", "x", "kept B"];
        let out = crate::elision::render_kept(&lines, &[0, 2]);
        assert!(out.contains("kept A") && out.contains("kept B"));
        assert!(out.contains('x'), "the non-paying line disappeared: {out}");
        assert!(
            !out.contains("elided by tokenade"),
            "a marker was placed on a hole that does not pay for it: {out}"
        );
    }
}

#[cfg(test)]
mod a_fold_footer_only_claims_what_was_folded {
    use super::*;

    /// `truncate_smart` is the public way into `render_kept`. Its footer says
    /// what the dropped middle stood for, and it works that number out by
    /// reading `(×N)` back off the lines — which content can carry too. This
    /// is the same defect as `disclose::original_weight`, in the sibling
    /// consumer, which is where this codebase keeps putting it.
    fn view(mid: &str, n: usize) -> String {
        let mut v: Vec<String> = (0..n)
            .map(|i| format!("line {i} of real content"))
            .collect();
        v[n / 2] = mid.to_string();
        v.join("\n")
    }

    #[test]
    fn a_run_count_taken_from_content_does_not_reach_the_footer() {
        let _floor = crate::fold_count::isolate_folded_ceiling();
        let out = truncate_smart(&view("cache hit for shard 4  (×900000)", 400), 40);
        let footer = out
            .lines()
            .find(|l| l.starts_with("[tokenade folded"))
            .expect("nothing was folded, so this proves nothing");
        // Nothing folded BEFORE this call, so no marker in the input can be
        // ours and there is nothing to stand for. Stated this way the guard
        // also catches the cap reading the ceiling AFTER the call, which lets
        // the call vouch for itself. Asserting on the forged digits does not
        // work: the claim is a SUM, so 900000 comes out as 901824.
        assert!(
            !footer.contains("standing for"),
            "a count from content backed a claim nothing folded: {footer}"
        );
    }

    #[test]
    fn a_run_count_near_usize_max_does_not_abort_the_fold() {
        let out = truncate_smart(&view(&format!("cache hit  (×{})", usize::MAX), 400), 40);
        assert!(out.lines().any(|l| l.starts_with("[tokenade folded")));
    }

    /// Anti-vacuity: a run this process really did collapse still gets said.
    #[test]
    fn a_run_we_really_folded_is_still_claimed() {
        let _floor = crate::fold_count::isolate_folded_ceiling();
        note_folded(900_000);
        let out = truncate_smart(&view("cache hit for shard 4  (×900000)", 400), 40);
        let footer = out
            .lines()
            .find(|l| l.starts_with("[tokenade folded"))
            .expect("nothing was folded, so this proves nothing");
        assert!(
            footer.contains("standing for"),
            "a real fold lost its claim: {footer}"
        );
    }
}

#[cfg(test)]
mod hard_cap {
    use super::*;

    fn long_output(lines: usize) -> String {
        let mut s = String::new();
        for i in 0..lines {
            s.push_str(&format!("line {i} value-{i} unique-token-{i}\n"));
        }
        s.push_str("FINAL SUMMARY: 3 failures, see above\n");
        s
    }

    /// Under the ceiling the net must not fire at all: a compactor that has
    /// already folded well must not be undone by the guard beneath it.
    #[test]
    fn the_cap_does_not_bite_under_the_ceiling() {
        let small = long_output(20);
        let after = tokenizer::estimate(&small);
        assert!(
            after <= HARD_CAP_TOKENS,
            "fixture is over the ceiling: {after}"
        );
        assert!(apply_hard_cap(&small, after, "probe").is_none());
    }

    /// Both ends survive. The tail of a tool output is where the failure
    /// summary lives, and dropping it is the whole reason head-only
    /// truncation is the wrong shape.
    #[test]
    fn the_cap_keeps_the_head_the_tail_and_says_what_it_dropped() {
        let big = long_output(120_000);
        let after = tokenizer::estimate(&big);
        assert!(
            after > HARD_CAP_TOKENS,
            "fixture is under the ceiling: {after}"
        );
        let (out, trunc) = apply_hard_cap(&big, after, "probe").expect("the cap must bite");
        assert!(trunc < after, "{trunc} !< {after}");
        assert!(
            out.starts_with("line 0 "),
            "the head was dropped: {:?}",
            &out[..40.min(out.len())]
        );
        assert!(out.contains("FINAL SUMMARY"), "the tail was dropped");
        assert!(out.contains("elided"), "the elision is not stated");
    }

    /// The ceiling itself, pinned. Without a fixture sitting BETWEEN the
    /// real value and an inflated one, raising 50 000 to 500 000 leaves every
    /// other guard green — an unpinned threshold is a missing guard, not a
    /// dead constant.
    #[test]
    fn the_ceiling_is_where_it_says_it_is() {
        let mid = long_output(5_000);
        let after = tokenizer::estimate(&mid);
        assert!(
            after > HARD_CAP_TOKENS && after < HARD_CAP_TOKENS * 4,
            "the fixture must straddle the ceiling, not tower over it: {after} vs {HARD_CAP_TOKENS}"
        );
        assert!(
            apply_hard_cap(&mid, after, "probe").is_some(),
            "the cap did not bite just above its own ceiling"
        );
    }

    /// Whatever comes back is STRICTLY smaller than what went in. The
    /// `trunc_after >= after` clause is what guarantees it, and no honest
    /// fixture can reach that branch: `generic` never inflates today, and
    /// under the ceiling the function returns before ever calling it. The
    /// clause defends a future `generic`, so the invariant is pinned rather
    /// than the branch — same ruling as the archive reader's `take` on a lying archive.
    #[test]
    fn whatever_comes_back_is_strictly_smaller() {
        for lines in [60_000usize, 120_000] {
            let big = long_output(lines);
            let after = tokenizer::estimate(&big);
            if after <= HARD_CAP_TOKENS {
                continue;
            }
            let (out, trunc) = apply_hard_cap(&big, after, "probe").expect("the cap must bite");
            assert!(trunc < after, "{lines} lines: {trunc} !< {after}");
            assert_eq!(
                trunc,
                tokenizer::estimate(&out),
                "the reported count is not the output's"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    /// A program with an arm of its own returned before the generic path's
    /// composite-key retry, so its `<prog>_<sub>` specs were unreachable — the
    /// exact gap that retry was written to close, reappearing one layer up
    /// because the retry sits in the LAST arm of the match.
    ///
    /// Measured 2026-09-01 against the shipped 1.1.7: `systemctl
    /// list-unit-files` folded nothing (10 729 → 10 729 bytes) with a spec in
    /// the table; through the arm's new call it folds to 5 156 (52 %).
    #[test]
    fn a_program_with_its_own_arm_still_reaches_its_subcommand_specs() {
        // `systemctl list-unit-files`: two columns, a blank line, a count.
        let mut body = String::from("UNIT FILE                         STATE     PRESET\n");
        for i in 0..300 {
            body.push_str(&format!(
                "unit-{i}.service                  enabled   enabled\n"
            ));
        }
        body.push_str("\n300 unit files listed.\n");
        let before = tokenizer::estimate(&body);
        let r = filter_command(&argv(&["systemctl", "--user", "list-unit-files"]), &body);
        assert!(
            r.savings.tokens_after < before,
            "the spec exists and must be reached: {} -> {}",
            before,
            r.savings.tokens_after
        );

        // And the subcommands that DO have an arm keep theirs — the new call
        // must not shadow `list-units` or `status`.
        let mut units = String::from("UNIT LOAD ACTIVE SUB DESCRIPTION\n");
        for i in 0..40 {
            units.push_str(&format!("u{i}.service loaded active running desc {i}\n"));
        }
        let r = filter_command(&argv(&["systemctl", "--user", "list-units"]), &units);
        assert_eq!(
            r.filter, "systemctl-list",
            "list-units keeps its own compactor: {}",
            r.filter
        );
    }

    /// Which arms must ask [`subcommand_spec`] before giving up, and which
    /// need not — with the reason, so the next reader can tell "we decided"
    /// from "nobody looked" (the discipline an earlier exclusion list settled on).
    ///
    /// The distinction is what the composite key MEANS for that program. `gh
    /// api`, `systemctl list-unit-files`: the second token is a subcommand, and
    /// the key names a real compactor the arm was hiding. `ssh_add`,
    /// `curl_head`, `cat_a`, `tail_f`: the key is a sibling PROGRAM reached by
    /// the hyphen alias (`ssh-add`), or a FLAG variant whose parent module
    /// already handles the flags. Calling the lookup there would build
    /// `cat_<filename>` and fold nothing, which is worse than not calling it —
    /// it would look like coverage.
    const ARMS_THAT_ASK: &[&str] = &["gh", "systemctl"];
    const ARMS_THAT_NEED_NOT_ASK: &[(&str, &str)] = &[
        (
            "ssh",
            "ssh_add/ssh_agent/ssh_copy_id are sibling PROGRAMS, not subcommands",
        ),
        (
            "curl",
            "curl_head/curl_k_insecure are flag variants; the curl module owns them",
        ),
        ("cat", "cat_a/cat_n/cat_v are flag variants of one program"),
        ("tail", "tail_f/tail_lines are flag variants of one program"),
        (
            "dig",
            "dig_MX/dig_TXT/dig_trace are RECORD TYPES and flags of one program, and the \
             type usually follows the domain, so the composite key would be `dig_<domain>`",
        ),
    ];

    /// A new arm that falls back generically while owning several composite
    /// keys is a known defect reappearing. Make it a build failure with a
    /// decision to record, rather than a silent five-fold loss nobody measures
    /// until somebody times a command.
    #[test]
    fn an_arm_that_owns_subcommand_specs_asks_for_them() {
        // The file is FOUND, not named: the dispatcher moved down from
        // `filter.rs` into `filter/dispatch.rs`, and this test announced "the
        // sweep reached only 0 arms" as if the `match` had disappeared.
        let (_, src) = crate::source_guard::the_one_source_carrying("match (prog.as_str(), sub)");
        let src = src.as_str();
        let table = include_str!("cmd/spec_table.rs");
        let lines: Vec<&str> = src.lines().collect();
        let mut keys: Vec<&str> = Vec::new();
        for part in table.split("key: \"").skip(1) {
            if let Some(end) = part.find('"') {
                keys.push(&part[..end]);
            }
        }
        assert!(
            keys.len() > 1000,
            "the spec table did not parse ({} keys) — the sweep proves nothing",
            keys.len()
        );
        let mut unclassified: Vec<String> = Vec::new();
        let mut checked = 0usize;
        for (i, line) in lines.iter().enumerate() {
            // `("prog", _) =>` — an arm keyed on a program name.
            let Some(open) = line.find("(\"") else {
                continue;
            };
            let rest = &line[open + 2..];
            let Some(close) = rest.find('"') else {
                continue;
            };
            let prog = &rest[..close];
            if !rest[close..].starts_with("\", _)") {
                continue;
            }
            let body_end = (i + 26).min(lines.len());
            let body = lines[i..body_end].join("\n");
            if !body.contains("auto_or_generic") {
                continue;
            }
            let prefix = format!("{}_", prog.replace('-', "_"));
            let owned = keys.iter().filter(|k| k.starts_with(&prefix)).count();
            if owned < 5 {
                continue;
            }
            checked += 1;
            if ARMS_THAT_ASK.contains(&prog) {
                assert!(
                    body.contains("subcommand_spec("),
                    "`{prog}` is listed as an arm that must ask, and does not"
                );
                continue;
            }
            if ARMS_THAT_NEED_NOT_ASK.iter().any(|(p, _)| *p == prog) {
                continue;
            }
            unclassified.push(format!("{prog} ({owned} composite keys)"));
        }
        assert!(
            checked >= 4,
            "the sweep reached only {checked} arms — it is not looking at the match"
        );
        assert!(
            unclassified.is_empty(),
            "arm(s) that fall back generically while owning subcommand specs, and are in \
             neither list — call `subcommand_spec` (and add to ARMS_THAT_ASK), or record why \
             the key is not a subcommand in ARMS_THAT_NEED_NOT_ASK:\n  {}",
            unclassified.join("\n  ")
        );
        for (_, reason) in ARMS_THAT_NEED_NOT_ASK {
            assert!(!reason.is_empty(), "every exemption carries its reason");
        }
    }

    /// The declarant refuses rather than guesses: an unknown subcommand, a
    /// subcommand equal to the program, and a fold that would not shrink the
    /// payload all come back `None` so the caller falls through unchanged.
    #[test]
    fn the_subcommand_lookup_refuses_instead_of_guessing() {
        let body = "nothing structured here at all\n".repeat(50);
        let before = tokenizer::estimate(&body);
        assert!(
            subcommand_spec("systemctl", "no-such-subcommand", &body, before).is_none(),
            "an unknown subcommand owns no spec"
        );
        assert!(
            subcommand_spec("systemctl", "", &body, before).is_none(),
            "no subcommand, no composite key"
        );
        assert!(
            subcommand_spec("systemctl", "systemctl", &body, before).is_none(),
            "the program is not its own subcommand"
        );
        // The inflation guard, on a body the spec DOES fold: with `before`
        // understated the fold no longer shrinks anything, and a fold that does
        // not shrink is not a fold. Asserted on foldable input on purpose — an
        // unfoldable body returns `None` for the wrong reason and would pass
        // whatever the guard did.
        let mut foldable = String::from("UNIT FILE                     STATE     PRESET\n");
        for i in 0..300 {
            foldable.push_str(&format!(
                "u{i}.service                  enabled   enabled\n"
            ));
        }
        let real = tokenizer::estimate(&foldable);
        assert!(
            subcommand_spec("systemctl", "list-unit-files", &foldable, real).is_some(),
            "the fixture must be one the spec folds, or the next assertion proves nothing"
        );
        assert!(
            subcommand_spec("systemctl", "list-unit-files", &foldable, 1).is_none(),
            "a fold that does not shrink the payload is refused"
        );

        // `sub` empty or equal to the program builds `systemctl_` /
        // `systemctl_systemctl`, which own no spec — so those two early returns
        // cannot be told apart from the lookup missing, and no assertion here
        // can distinguish them. They stay because the key they would build is
        // meaningless, not because a test forces them.
    }

    #[test]
    fn near_miss_is_exactly_what_the_flat_ceiling_lets_through() {
        // Every rejection recorded in production sits under the flat ceiling,
        // so none of them is a near miss — the telemetry must not double-count
        // what the guard already catches.
        for (before, after) in [(329, 10), (362, 24), (258, 30), (481, 20), (816, 19)] {
            assert!(
                !near_miss_above_flat_ceiling(before, after),
                "{before}→{after} is already REJECTED by the guard"
            );
        }
        // The blind spot: same 95 % collapse, larger input, so the fixed-size
        // banner lands above 30 and the guard never looks at it.
        assert!(near_miss_above_flat_ceiling(5_000, 50));
        assert!(near_miss_above_flat_ceiling(100_000, 200));
        // Boundary: 31 tokens is the first output the guard cannot see.
        assert!(!near_miss_above_flat_ceiling(1_000, 30));
        assert!(near_miss_above_flat_ceiling(1_000, 31));
        // An ordinary compaction is not a near miss.
        assert!(!near_miss_above_flat_ceiling(1_000, 400)); // only 60 %
        assert!(!near_miss_above_flat_ceiling(80, 5)); // input too small
        assert!(!near_miss_above_flat_ceiling(100, 100)); // nothing removed
    }

    #[test]
    fn program_basename_fleet_fixes() {
        let v = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();
        assert_eq!(program_basename(&v("cd /x && python3 run.py")), "python3");
        assert_eq!(program_basename(&v("cd /x ; make test")), "make");
        assert_eq!(program_basename(&v("cd /somewhere")), "cd");
        assert_eq!(
            program_basename(&["'python3'".into(), "x.py".into()]),
            "python3"
        );
        assert_eq!(program_basename(&v("sudo -n git status")), "git");
    }

    /// Codex reads code as `nl -ba f | sed -n 'a,bp'`: a passthrough (the pipe
    /// is the agent's own shaping), delivered without the gutter's padding.
    #[test]
    fn a_piped_numbered_read_loses_its_padding_and_nothing_else() {
        let v = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();
        let raw: String = (1..=40)
            .map(|n| format!("{n:>6}\tlet x{n} = {n};\n"))
            .collect();
        let r = filter_command(&v("nl -ba src/lib.rs | sed -n 1,40p"), &raw);
        assert_eq!(r.filter, "gutter");
        let want: String = (1..=40)
            .map(|n| format!("{n}\tlet x{n} = {n};\n"))
            .collect();
        assert_eq!(r.output, want);
    }

    /// A guard writes nothing; the command after it does (fleet 2026-10-03:
    /// `[` 103 events). A guard ALONE stays itself, like a bare `cd`.
    #[test]
    fn a_guard_before_a_command_is_not_the_command() {
        let v = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();
        assert_eq!(program_basename(&v("[ -d out ] && ls -la out")), "ls");
        assert_eq!(program_basename(&v("[[ -f x ]] && cat x")), "cat");
        assert_eq!(
            program_basename(&v("test -f Cargo.toml && cargo build")),
            "cargo"
        );
        assert_eq!(program_basename(&v("[ -d out ]")), "[");
    }

    /// Fleet 2026-10-02: `npm.cmd` (54 events, 120.7k tokens), `pnpm.js`
    /// (142) and `npx.cmd` reported "uncovered" — they are npm, pnpm, npx.
    #[test]
    fn a_launcher_routes_as_the_program_it_launches() {
        let v = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();
        for (launched, plain) in [
            ("npm.cmd install", "npm install"),
            (r"C:\nodejs\npx.cmd tsc --noEmit", "npx tsc --noEmit"),
            ("/home/u/.local/share/pnpm/pnpm.js install", "pnpm install"),
            ("php.ps1 artisan migrate", "php artisan migrate"),
            ("node_modules/npm/bin/npm-cli.js ci", "npm ci"),
        ] {
            assert_eq!(
                program_basename(&v(launched)),
                program_basename(&v(plain)),
                "{launched}"
            );
            let out =
                "added 312 packages, and audited 313 packages in 4s\nfound 0 vulnerabilities\n"
                    .repeat(30);
            assert_eq!(
                crate::filter::filter_command(&v(launched), &out).filter,
                crate::filter::filter_command(&v(plain), &out).filter,
                "{launched} routes elsewhere than {plain}"
            );
        }
        assert_eq!(
            program_basename(&v("server.js --port 3000")),
            "server.js",
            "a script is not a program"
        );
    }

    #[test]
    fn corepack_routes_as_the_package_manager_it_names() {
        let v = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();
        let out = "Lockfile is up to date, resolution step is skipped\nProgress: resolved 812, reused 790, downloaded 22, added 812, done\n".repeat(20);
        for launched in [
            "corepack pnpm install",
            "/usr/lib/node_modules/corepack/dist/corepack.js pnpm install",
        ] {
            assert_eq!(
                crate::filter::filter_command(&v(launched), &out).filter,
                crate::filter::filter_command(&v("pnpm install"), &out).filter,
                "{launched}"
            );
        }
        // Its own commands are not a package manager's.
        assert_ne!(
            crate::filter::filter_command(&v("corepack enable pnpm"), &out).filter,
            crate::filter::filter_command(&v("pnpm install"), &out).filter
        );
    }

    #[test]
    fn basename_strips_windows_exe_suffix() {
        let v = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();
        assert_eq!(program_basename(&v("curl.exe -s https://x")), "curl");
        // The SUFFIX match is case-insensitive; the stem's case is preserved
        // (arms that need case-folding, like `Get-Content`, do it in their
        // own guard).
        assert_eq!(program_basename(&v("PYTHON.EXE run.py")), "PYTHON");
        assert_eq!(
            program_basename(&v(r"C:\Program\php.exe artisan migrate")),
            "php"
        );
        // Not a suffix to strip: a name that merely ends in those letters,
        // and the degenerate all-suffix token.
        assert_eq!(program_basename(&v("makeexe build")), "makeexe");
        assert_eq!(program_basename(&v(".exe")), ".exe");
    }

    #[test]
    fn control_flow_attributes_to_loop_body() {
        let v = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();
        assert_eq!(
            program_basename(&v("for f in *.log; do grep x $f; done")),
            "grep"
        );
        assert_eq!(
            program_basename(&v("while read l; do docker ps; done")),
            "docker"
        );
        assert_eq!(
            program_basename(&v("until ok; do cargo build; done")),
            "cargo"
        );
        assert_eq!(
            program_basename(&v("if git diff --quiet; then echo ok; fi")),
            "echo"
        );
        // Composes with the other scaffolding rules, in both orders.
        assert_eq!(
            program_basename(&v("cd /srv && for f in a b; do LANG=C git log; done")),
            "git"
        );
        // No body keyword in argv → stays attributed to the keyword rather
        // than guessing.
        assert_eq!(program_basename(&v("for f in *.log")), "for");
        // `case` is deliberately not unwrapped.
        assert_eq!(program_basename(&v("case $x in a) ls;; esac")), "case");
    }

    #[test]
    fn php_test_runners_resolve_through_the_interpreter() {
        assert_eq!(
            wrapped_test_runner(&argv(&["php", "artisan", "test"])),
            Some("phpunit")
        );
        assert_eq!(
            wrapped_test_runner(&argv(&["php", "vendor/bin/phpunit", "--testdox"])),
            Some("phpunit")
        );
        assert_eq!(
            wrapped_test_runner(&argv(&["php", "vendor/bin/pest"])),
            Some("pest")
        );
        // Not a test run: a plain script, and a non-test artisan subcommand.
        assert_eq!(wrapped_test_runner(&argv(&["php", "script.php"])), None);
        assert_eq!(
            wrapped_test_runner(&argv(&["php", "artisan", "migrate"])),
            None
        );
    }

    #[test]
    fn interpreters_are_content_routed_not_generic() {
        // A JSON-emitting script routes by the CONTENT's shape, not by a
        // (nonexistent) node/php/ruby output shape — same contract the
        // `sed`/`head`/`tee` arms already hold.
        let mut body = String::from("[\n");
        for i in 0..200 {
            let _ = writeln!(
                body,
                "  {{\"id\": {i}, \"name\": \"item\", \"status\": \"active\", \"value\": 42}},"
            );
        }
        body.push_str("  {\"id\": 999, \"name\": \"last\", \"value\": 42}\n]\n");
        for prog in &["node", "deno", "php", "ruby"] {
            let r = filter_command(&argv(&[prog, "s"]), &body);
            assert_eq!(
                r.filter, "auto:json",
                "{prog} of JSON content should hit the JSON compactor, got {}",
                r.filter
            );
        }
        // …but the test subcommands still win over the interpreter arm. Arm
        // ORDER is the one thing that could silently regress here.
        //
        // Probed with a real RUNNER output, no longer with the JSON body
        // above. That body pinned the order by a side effect: the test
        // compactor claimed it without being able to fold anything, and it
        // was that absence of fold that made the name observable. Since
        // `prefer_the_fallback_when_the_name_folded_nothing` exists, a
        // barren claim hands over — here to the JSON compactor, which
        // delivers fewer bytes without losing anything, hence the right choice
        // for the agent. The order of the branches is proven on what a runner
        // really writes.
        let mut tap = String::from("TAP version 13\n");
        for i in 0..200 {
            let _ = writeln!(tap, "ok {} - suite > case {i} passes", i + 1);
        }
        let _ = writeln!(tap, "1..200\n# pass 200\n# fail 0");
        for a in [
            vec!["node", "--test", "t.js"],
            vec!["deno", "test"],
            vec!["php", "artisan", "test"],
            vec!["php", "vendor/bin/phpunit"],
        ] {
            let r = filter_command(&argv(&a), &tap);
            assert_ne!(
                r.filter, "auto:json",
                "{a:?} must reach its test compactor, not the interpreter arm"
            );
            assert!(
                r.output.len() * 2 < tap.len(),
                "{a:?} was routed but folded nothing, so the name \
                 proves nothing: {} → {}",
                tap.len(),
                r.output.len()
            );
        }
    }

    #[test]
    fn python_dash_m_detects_test_runners() {
        assert_eq!(
            python_m_test_runner(&argv(&["python", "-m", "unittest"])),
            Some("unittest")
        );
        assert_eq!(
            python_m_test_runner(&argv(&["python3", "-m", "pytest", "-q"])),
            Some("pytest")
        );
        assert_eq!(
            python_m_test_runner(&argv(&["python", "-m", "unittest", "discover"])),
            Some("unittest")
        );
        assert_eq!(
            python_m_test_runner(&argv(&["python", "-m", "nose2"])),
            Some("nose2")
        );
        assert_eq!(
            python_m_test_runner(&argv(&["/usr/bin/python3", "-munittest"])),
            Some("unittest")
        );
        // Not a test runner / not python.
        assert_eq!(
            python_m_test_runner(&argv(&["python", "-m", "http.server"])),
            None
        );
        assert_eq!(python_m_test_runner(&argv(&["python", "script.py"])), None);
        assert_eq!(
            python_m_test_runner(&argv(&["node", "-m", "unittest"])),
            None
        );
    }

    #[test]
    fn python_dash_m_unittest_routes_to_unittest_compactor() {
        let raw = "\
FE\n\
======================================================================\n\
FAIL: test_a (m.C.test_a)\n\
----------------------------------------------------------------------\n\
Traceback (most recent call last):\n\
  File \"/abs/t.py\", line 6, in test_a\n\
    self.assertEqual(1, 2)\n\
AssertionError: 1 != 2\n\
\n\
----------------------------------------------------------------------\n\
Ran 2 tests in 0.001s\n\
\n\
FAILED (failures=1)\n";
        let r = filter_command(&argv(&["python3", "-m", "unittest"]), raw);
        assert_eq!(
            r.filter, "unittest",
            "routed to wrong compactor: {}",
            r.filter
        );
        // Identity + assertion + tally preserved; decoration gone.
        assert!(r.output.contains("FAIL: test_a"));
        assert!(r.output.contains("AssertionError: 1 != 2"));
        assert!(r.output.contains("FAILED (failures=1)"));
        assert!(!r.output.contains("===================="));
        assert!(r.output.len() < raw.len());
    }

    #[test]
    fn wrapped_runner_detects_every_wrapper() {
        // JS package-runners.
        assert_eq!(wrapped_test_runner(&argv(&["npx", "jest"])), Some("jest"));
        assert_eq!(
            wrapped_test_runner(&argv(&["bunx", "vitest", "run"])),
            Some("vitest")
        );
        assert_eq!(
            wrapped_test_runner(&argv(&["pnpm", "dlx", "playwright", "test"])),
            Some("playwright")
        );
        assert_eq!(
            wrapped_test_runner(&argv(&["pnpm", "exec", "mocha"])),
            Some("mocha")
        );
        assert_eq!(
            wrapped_test_runner(&argv(&["yarn", "dlx", "jest"])),
            Some("jest")
        );
        // npx flag-skipping: `-p pkg` consumes a value, then the runner.
        assert_eq!(
            wrapped_test_runner(&argv(&["npx", "-y", "-p", "foo", "jest"])),
            Some("jest")
        );
        // Versioned / scoped specs.
        assert_eq!(
            wrapped_test_runner(&argv(&["npx", "jest@29"])),
            Some("jest")
        );
        // Ruby / PHP / Python package-runners.
        assert_eq!(
            wrapped_test_runner(&argv(&["bundle", "exec", "rspec"])),
            Some("rspec")
        );
        assert_eq!(
            wrapped_test_runner(&argv(&["bundle", "exec", "rspec", "spec/"])),
            Some("rspec")
        );
        assert_eq!(
            wrapped_test_runner(&argv(&["poetry", "run", "pytest", "-q"])),
            Some("pytest")
        );
        assert_eq!(
            wrapped_test_runner(&argv(&["uv", "run", "pytest"])),
            Some("pytest")
        );
        assert_eq!(
            wrapped_test_runner(&argv(&["pipenv", "run", "pytest"])),
            Some("pytest")
        );
        // python -m is still covered (delegated to python_m_test_runner).
        assert_eq!(
            wrapped_test_runner(&argv(&["python3", "-m", "unittest"])),
            Some("unittest")
        );
        // Non-test subcommands of the same wrappers must NOT match.
        assert_eq!(wrapped_test_runner(&argv(&["pnpm", "install"])), None);
        assert_eq!(wrapped_test_runner(&argv(&["yarn", "add", "jest"])), None);
        assert_eq!(wrapped_test_runner(&argv(&["bundle", "install"])), None);
        assert_eq!(
            wrapped_test_runner(&argv(&["poetry", "add", "pytest"])),
            None
        );
        // Wrapper running a non-runner package.
        assert_eq!(wrapped_test_runner(&argv(&["npx", "tsc"])), None);
        assert_eq!(
            wrapped_test_runner(&argv(&["npx", "prettier", "--write", "."])),
            None
        );
    }

    #[test]
    fn route_wrapped_runner_picks_the_right_compactor() {
        let (f, _) = route_wrapped_runner("pytest", &argv(&["poetry", "run", "pytest"]), "");
        assert_eq!(f, "pytest");
        let (f, _) = route_wrapped_runner("unittest", &argv(&["python3", "-m", "unittest"]), "");
        assert_eq!(f, "unittest");
        let (f, _) = route_wrapped_runner("rspec", &argv(&["bundle", "exec", "rspec"]), "");
        assert_eq!(f, "test-runner");
        let (f, _) = route_wrapped_runner("jest", &argv(&["npx", "jest"]), "");
        assert_eq!(f, "test-runner");
    }

    #[test]
    fn native_go_test_routes_to_go_test_compactor() {
        // `go test`'s prog is `go`, not the `go_test` module — verify the
        // explicit native arm reaches the dedicated compactor. Use many
        // passing tests so the fold actually shrinks (else the safety net
        // relabels a no-op fold as `passthrough`).
        let mut raw = String::new();
        for i in 0..40 {
            raw.push_str(&format!(
                "=== RUN   TestCase{i}\n--- PASS: TestCase{i} (0.00s)\n"
            ));
        }
        raw.push_str(
            "=== RUN   TestDiv\n--- FAIL: TestDiv (0.00s)\n    math_test.go:21: want 2, got 3\n",
        );
        raw.push_str("FAIL\nexit status 1\nFAIL\texample/math\t0.012s\n");
        let r = filter_command(&argv(&["go", "test", "./..."]), &raw);
        assert_eq!(
            r.filter, "go-test",
            "routed to wrong compactor: {}",
            r.filter
        );
        // The failure detail survives.
        assert!(r.output.contains("TestDiv"));
        assert!(r.output.contains("want 2, got 3"));
    }

    #[test]
    fn two_token_fallback_reaches_orphaned_subcommand_compactor() {
        // `next` has NO explicit arm, NO family dispatcher, and NO single-token
        // `next.rs` shadow module — yet a dedicated `next_build` compactor
        // exists (keyed `next_build`). The bare-prog dispatch looks up `next`
        // and misses entirely; only the two-token fallback rebuilds the
        // composite key `next_build` and reaches it. This is the genuine
        // `<prog>_<sub>` gap that hid `go_test`/`cargo_nextest`.
        let mut raw = String::from("Creating an optimized production build\n");
        for i in 0..20 {
            raw.push_str(&format!("info  - chunk {i} emitted\n"));
        }
        raw.push_str("Compiled successfully\n");
        let r = filter_command(&argv(&["next", "build"]), &raw);
        assert_eq!(
            r.filter, "next",
            "not routed via two-token fallback: {}",
            r.filter
        );
        // The outcome survives; the `info  - ` noise is folded.
        assert!(
            r.output.contains("Compiled successfully"),
            "lost outcome: {}",
            r.output
        );
        assert!(
            !r.output.contains("info  - chunk 0 emitted"),
            "noise kept: {}",
            r.output
        );
        assert!(
            r.savings.tokens_after < r.savings.tokens_before,
            "did not shrink"
        );
    }

    #[test]
    fn two_token_fallback_is_content_defensive() {
        // The same `next build` invocation on output that ISN'T a next build
        // must NOT be folded by `next_build::detect` (needs "Compiled
        // successfully" / "Creating an optimized production build"). It falls
        // through to the generic pass with its diagnostic line intact.
        let raw = "error: Cannot find module 'next'\nnpm ERR! missing script: build\n";
        let r = filter_command(&argv(&["next", "build"]), raw);
        assert!(
            r.output.contains("Cannot find module"),
            "lost the error line: {}",
            r.output
        );
    }

    // --- command-specific classification (admin "Commands" view) ---

    #[test]
    fn filter_name_classification() {
        // Dedicated per-command compactors.
        assert!(filter_name_is_command_specific("git-status"));
        assert!(filter_name_is_command_specific("df"));
        assert!(filter_name_is_command_specific("ls"));
        assert!(filter_name_is_command_specific("git-status+cap"));
        // Universal fallbacks and form-aware auto → NOT command-specific.
        assert!(!filter_name_is_command_specific("generic"));
        assert!(!filter_name_is_command_specific("passthrough"));
        assert!(!filter_name_is_command_specific("auto:json"));
        assert!(!filter_name_is_command_specific("auto:yaml+cap"));
        assert!(!filter_name_is_command_specific("docker-fallback-generic"));
        assert!(!filter_name_is_command_specific(""));
    }

    #[test]
    fn generic_fold_marker_unmasks_stub() {
        use crate::archetypes;
        // Dedicated filter name, no generic fold ran → command-specific.
        archetypes::reset_generic_fold();
        let r = FilterResult {
            filter: "df".into(),
            output: String::new(),
            savings: Savings::new(10, 5),
        };
        assert!(r.is_command_specific(), "real compactor → specific");

        // Same shape of name, but a command-agnostic helper produced the
        // fold (a head/tail stub) → unmasked as NON-specific.
        archetypes::reset_generic_fold();
        let many: String = (0..50).map(|i| format!("line {i}\n")).collect();
        let folded = archetypes::head_tail(&many, 5, 5, "stub", "lines");
        assert!(folded.is_some(), "head_tail must fold to set the marker");
        let r = FilterResult {
            filter: "pants".into(),
            output: String::new(),
            savings: Savings::new(10, 5),
        };
        assert!(!r.is_command_specific(), "head/tail stub → not specific");
    }

    #[test]
    fn program_basename_skips_env_and_path() {
        assert_eq!(
            program_basename(&argv(&["LANG=C", "/usr/bin/git", "status"])),
            "git"
        );
        assert_eq!(program_basename(&argv(&["ls", "-la"])), "ls");
        assert_eq!(program_basename(&argv(&[])), "");
    }

    #[test]
    fn program_basename_strips_quotes_and_subshell_paren() {
        // Real dashboard artifacts: quoted command names and subshell launches
        // must resolve to the bare program so they route + report correctly,
        // not as `'python3'` / `(echo` rows.
        assert_eq!(program_basename(&argv(&["'python3'", "x.py"])), "python3");
        assert_eq!(program_basename(&argv(&["\"git\"", "status"])), "git");
        assert_eq!(program_basename(&argv(&["(echo", "hi)"])), "echo");
        assert_eq!(program_basename(&argv(&["'curl'", "-s", "u"])), "curl");
        // A quoted absolute path still basenames correctly.
        assert_eq!(program_basename(&argv(&["'/usr/bin/python3'"])), "python3");
    }

    #[test]
    fn gh_output_is_content_folded() {
        // `gh api` REST JSON → routed (was uncovered). The url-zoo is stripped
        // and the JSON folds; result must shrink and drop the chrome.
        let json = concat!(
            r#"[{"number":1,"title":"x","node_id":"abc","#,
            r#""url":"https://api.github.com/r/1","html_url":"https://github.com/o/r/1","#,
            r#""events_url":"https://api.github.com/e","comments_url":"https://api.github.com/c"}]"#
        );
        let r = filter_command(&argv(&["gh", "api", "repos/o/r/issues"]), json);
        // gh now routes to the content fold (was passthrough) — coverage, like
        // `python3`/`npx`. The REST url-zoo strip drops the chrome and shrinks.
        assert!(!r.output.contains("node_id"));
        assert!(!r.output.contains("events_url"));
        assert!(r.output.len() < json.len());
        assert!(r.savings.tokens_after <= r.savings.tokens_before);
    }

    #[test]
    fn ssh_remote_argv_extracts_command_skipping_options() {
        assert_eq!(
            ssh_remote_argv(&argv(&["ssh", "host", "docker", "ps"])),
            Some(vec!["docker".to_string(), "ps".to_string()])
        );
        // value-taking short flags (-p, -i) consume their next token.
        assert_eq!(
            ssh_remote_argv(&argv(&[
                "ssh",
                "-p",
                "2222",
                "-i",
                "k",
                "user@host",
                "df",
                "-h"
            ])),
            Some(vec!["df".to_string(), "-h".to_string()])
        );
        // inline-value / boolean flags don't consume the next token.
        assert_eq!(
            ssh_remote_argv(&argv(&["ssh", "-4", "-p2222", "host", "uptime"])),
            Some(vec!["uptime".to_string()])
        );
        // bare host = interactive → no remote command.
        assert_eq!(ssh_remote_argv(&argv(&["ssh", "host"])), None);
        assert_eq!(ssh_remote_argv(&argv(&["ssh", "-i", "key", "host"])), None);
    }

    #[test]
    fn ssh_remote_output_routes_to_inner_compactor() {
        // `ssh vps 'docker ps'` → the remote docker output must route to the
        // docker compactor, not pass through. Build a verbose-ish docker ps.
        let docker_out = "CONTAINER ID   IMAGE     COMMAND   STATUS    NAMES\n".to_string()
            + &"abc123def456   nginx     \"/x\"      Up 2h     web\n".repeat(12);
        let r = filter_command(&argv(&["ssh", "vps", "'docker", "ps'"]), &docker_out);
        // Routed (not raw passthrough) and at least not larger.
        assert!(r.savings.tokens_after <= r.savings.tokens_before);
        // A bare interactive ssh with empty output is a harmless no-op.
        let r2 = filter_command(&argv(&["ssh", "vps"]), "");
        assert!(r2.output.is_empty());
    }

    #[test]
    fn a_wrappers_numeric_argument_is_not_the_program() {
        // Found in `~/.tokenade/cmdstats` 2026-08: counters named `300`, `200`,
        // `115` — durations recorded as commands. Dispatch keys on this name,
        // so every one of those runs lost its dedicated compactor.
        let v = |s: &str| -> Vec<String> { s.split_whitespace().map(|t| t.to_string()).collect() };
        assert_eq!(program_basename(&v("timeout 300 cargo test")), "cargo");
        assert_eq!(program_basename(&v("timeout 5m pytest -q")), "pytest");
        assert_eq!(program_basename(&v("timeout -k 5 120 pytest -q")), "pytest");
        assert_eq!(program_basename(&v("timeout 1.5 curl example.com")), "curl");
        assert_eq!(program_basename(&v("nice -n 10 cargo test")), "cargo");
        assert_eq!(
            program_basename(&v("sudo timeout 60 journalctl -u x")),
            "journalctl"
        );
    }

    #[test]
    fn a_timeout_wrapped_command_still_reaches_its_own_compactor() {
        // The half that costs money: routing, not the label. `timeout 300
        // cargo test` used to route on `300` and fall through to `generic`.
        let v = |s: &str| -> Vec<String> { s.split_whitespace().map(|t| t.to_string()).collect() };
        let mut out = String::new();
        for i in 0..40 {
            out.push_str(&format!("   Compiling crate{i} v0.1.0\n"));
        }
        out.push_str("error[E0432]: unresolved import `foo`\n  --> src/a.rs:1:5\n   |\n1 | use foo;\n   |     ^^^ no `foo`\n\nerror: could not compile `x` due to 1 previous error\n");
        let out = out.as_str();
        let bare = filter_command(&v("cargo build"), out);
        let wrapped = filter_command(&v("timeout 300 cargo build"), out);
        assert_eq!(
            wrapped.filter, bare.filter,
            "a wrapped command must route exactly like the bare one"
        );
        assert_ne!(wrapped.filter, "generic");
    }

    /// Build a Rust source slice of `lines` lines, half of it comments.
    fn rust_slice(lines: usize) -> String {
        let mut s = String::new();
        for i in 0..lines {
            s.push_str(&format!(
                "// a comment that carries no meaning for the reader {i}\nlet value_{i} = compute_something(i, {i});\n"
            ));
        }
        s
    }

    #[test]
    fn a_big_sed_slice_of_source_gets_the_code_strip() {
        let out = rust_slice(300);
        let r = filter_command(&argv(&["sed", "-n", "200,800p", "src/foo.rs"]), &out);
        assert_eq!(r.filter, "sed", "got {}", r.filter);
        assert!(
            r.output.len() < out.len() / 2,
            "comments must be stripped: {} -> {}",
            out.len(),
            r.output.len()
        );
        assert!(r.output.contains("let value_7 = "), "code survives");
    }

    #[test]
    fn a_small_sed_slice_comes_back_byte_exact() {
        // A short slice is an edit target. Touching it would make the agent
        // patch bytes that are not the ones on disk.
        let out = rust_slice(6);
        let r = filter_command(&argv(&["sed", "-n", "10,22p", "src/foo.rs"]), &out);
        assert!(
            r.output
                .contains("// a comment that carries no meaning for the reader 3"),
            "small slices keep their comments: {}",
            r.output
        );
    }

    #[test]
    fn head_of_a_non_source_file_still_goes_to_the_auto_detector() {
        let out = rust_slice(300);
        let r = filter_command(&argv(&["head", "-800", "build.log"]), &out);
        assert_ne!(r.filter, "head", "no source extension → unchanged routing");
    }

    /// `git grep` output is `grep -rn` output: same compactor, same census,
    /// and no result rescued as a "failure".
    #[test]
    fn git_grep_reaches_the_grep_compactor() {
        let _g = tk_base::delivery::threshold_on_this_thread(5_000);
        // Real-looking variety: a template of one shape would be folded by the
        // post-pass and prove nothing about routing.
        let word = |mut n: usize| {
            let mut w = String::new();
            for _ in 0..3 {
                w.push((b'a' + (n % 26) as u8) as char);
                n /= 26;
            }
            w
        };
        let mut out = String::new();
        for i in 0..683 {
            let body = if i == 600 {
                "fn des_erreurs() {".to_string()
            } else {
                format!(
                    "fn {}_{}(x: &{}) -> {} {{",
                    word(i),
                    word(i * 7 + 3),
                    word(i + 11),
                    word(i * 3)
                )
            };
            out.push_str(&format!(
                "crates/tk-fold/src/{}.rs:{}:    {body}\n",
                word(i % 50 + 100),
                10 + i * 3
            ));
        }
        let r = filter_command(&argv(&["git", "grep", "-n", "fn "]), &out);
        assert_eq!(r.filter, "git-grep");
        assert!(
            r.output
                .starts_with("[tokenade:grep] 683 match(es) in 50 file(s)"),
            "{}",
            &r.output[..200.min(r.output.len())]
        );
        let names: std::collections::BTreeSet<String> = (0..50).map(|f| word(f + 100)).collect();
        for f in &names {
            assert!(r.output.contains(&format!("{f}.rs\n")), "{f} unnamed");
        }
        assert!(!r.output.contains("look like failures"));
    }

    #[test]
    fn git_listing_subcommands_reach_their_own_compactor() {
        // These compactors existed but had no dispatcher arm, so every one of
        // them fell through to the generic pass.
        let paths: String = (0..40).map(|i| format!("src/mod{i}/file.rs\n")).collect();
        let r = filter_command(
            &argv(&[
                "git",
                "-c",
                "core.quotepath=false",
                "ls-files",
                "--recurse-submodules",
            ]),
            &paths,
        );
        assert_eq!(r.filter, "git-ls-files", "got {}", r.filter);
        // Forty paths fit: every one is listed (the cut is for listings that
        // do not).
        assert!(
            r.output.contains("mod0") && r.output.contains("mod39"),
            "{}",
            r.output
        );

        let r = filter_command(&argv(&["git", "worktree", "list"]), &paths);
        assert_ne!(r.filter, "generic", "worktree list is routed");
    }

    #[test]
    fn a_short_git_listing_is_left_alone() {
        // Below the fold threshold the census would be longer than the list.
        let r = filter_command(&argv(&["git", "ls-files"]), "a.rs\nb.rs\n");
        assert_ne!(r.filter, "git-ls-files");
    }

    #[test]
    fn a_program_whose_name_starts_with_a_digit_still_resolves() {
        // The guard must not eat real executables: `7z` and `2to3` begin with
        // a digit but are not numbers.
        let v = |s: &str| -> Vec<String> { s.split_whitespace().map(|t| t.to_string()).collect() };
        assert_eq!(program_basename(&v("timeout 30 7z l archive.7z")), "7z");
        assert_eq!(program_basename(&v("nice 2to3 -w src")), "2to3");
    }

    #[test]
    fn program_basename_unwraps_wrappers_like_filter_wrapped() {
        // Wrapped commands must attribute to the inner program (matching the
        // compactor that actually ran), not to sudo/timeout/env/command.
        assert_eq!(program_basename(&argv(&["sudo", "pgrep", "foo"])), "pgrep");
        assert_eq!(
            program_basename(&argv(&["/usr/bin/sudo", "git", "log"])),
            "git"
        );
        assert_eq!(
            program_basename(&argv(&["nohup", "cargo", "build"])),
            "cargo"
        );
        // Nested wrappers + env-assignment after the wrapper word.
        assert_eq!(
            program_basename(&argv(&["sudo", "env", "FOO=bar", "ls"])),
            "ls"
        );
        assert_eq!(
            program_basename(&argv(&["command", "kubectl", "get", "pods"])),
            "kubectl"
        );
        // A bare wrapper with nothing after it has no inner command.
        assert_eq!(program_basename(&argv(&["sudo"])), "");
    }

    // --- systemctl list-units: fold healthy, keep failures ---

    fn systemctl_sample(n_healthy: usize, with_failure: bool) -> String {
        let mut s = String::from("  UNIT                 LOAD   ACTIVE SUB     DESCRIPTION\n");
        for i in 0..n_healthy {
            let sub = if i % 2 == 0 { "running" } else { "exited" };
            let _ = writeln!(
                s,
                "  svc{i}.service        loaded active {sub} Healthy service number {i}"
            );
        }
        if with_failure {
            s.push_str("  broken.service       loaded failed failed  The Broken Thing\n");
            s.push_str(
                "  ghost.service        not-found inactive dead Unit ghost.service not found.\n",
            );
        }
        s.push_str("\n57 loaded units listed. Pass --all to see loaded but inactive units, too.\n");
        s
    }

    #[test]
    fn systemctl_list_folds_healthy_keeps_failures() {
        let body = systemctl_sample(40, true);
        let out = systemctl_list_units(&body);
        // Every abnormal unit survives — never decapitated.
        assert!(out.contains("broken.service"), "failed unit kept");
        assert!(out.contains("failed failed"), "failed state kept");
        assert!(out.contains("ghost.service"), "not-found unit kept");
        assert!(
            out.contains("not-found inactive dead"),
            "not-found state kept"
        );
        // Header and trailing legend survive.
        assert!(out.contains("UNIT") && out.contains("LOAD"), "header kept");
        assert!(out.contains("57 loaded units listed"), "footer kept");
        // The healthy run is named on one line per SUB, not repeated per row.
        assert!(
            out.contains(" · svc20.service · "),
            "healthy unit named:\n{out}"
        );
        assert!(
            !out.contains("svc20.service  "),
            "healthy row folded away:\n{out}"
        );
        assert!(out.len() < body.len(), "must shrink");
    }

    /// Beyond the delivery threshold the names no longer fit, and the run
    /// falls back to its count.
    #[test]
    fn systemctl_list_counts_the_healthy_run_when_its_names_do_not_fit() {
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        let out = systemctl_list_units(&systemctl_sample(40, true));
        assert!(out.contains("healthy units folded"), "{out}");
        assert!(!out.contains("svc20.service"), "{out}");
        assert!(out.contains("broken.service"), "{out}");
    }

    /// Real `systemctl list-units --type=service` (2026-10-03, 46 units, all
    /// healthy): every unit named, grouped by SUB; systemd's fixed legend gone.
    #[test]
    fn systemctl_list_names_every_healthy_unit_and_drops_the_legend() {
        let body = "  UNIT                   LOAD   ACTIVE SUB     DESCRIPTION\n".to_string()
            + &(0..12)
                .map(|i| format!("  d{i}.service           loaded active running Daemon {i}\n"))
                .collect::<String>()
            + "  ldconfig.service       loaded active exited  Rebuild Dynamic Linker Cache\n\
               \x20 systemd-tmpfiles-setup.service loaded active exited  Create Volatile Files\n\
               \n\
               Legend: LOAD   → Reflects whether the unit definition was properly loaded.\n\
               \x20       ACTIVE → The high-level unit activation state, i.e. generalization of SUB.\n\
               \x20       SUB    → The low-level unit activation state, values depend on unit type.\n\
               \n\
               14 loaded units listed. Pass --all to see loaded but inactive units, too.\n";
        let out = systemctl_list_units(&body);
        assert!(
            out.contains("\n  [loaded active exited — 2] ldconfig.service · systemd-tmpfiles-setup.service\n"),
            "{out}"
        );
        assert!(
            out.contains("[loaded active running — 12] d0.service · d1.service"),
            "{out}"
        );
        assert!(!out.contains("Legend"), "{out}");
        assert!(
            out.ends_with(
                "\n\n14 loaded units listed. Pass --all to see loaded but inactive units, too."
            ),
            "{out}"
        );
    }

    /// Every SUB state under `loaded active` is healthy, and listing SUB
    /// values instead of asking about ACTIVE left most of a real host out.
    ///
    /// Measured 2026-08-27 on a 623-unit machine: `running`/`exited` matched
    /// ONE line, because the host is mostly `.device` units in `plugged` and
    /// `.mount` units in `mounted`. 121 748 bytes folded to 111 718.
    #[test]
    fn every_healthy_sub_state_folds() {
        for sub in [
            "running",
            "exited",
            "plugged",
            "mounted",
            "active",
            "listening",
            "waiting",
        ] {
            let mut s = String::from("  UNIT LOAD ACTIVE SUB DESCRIPTION\n");
            for i in 0..12 {
                s.push_str(&format!("  u{i}.unit loaded active {sub} Some Thing {i}\n"));
            }
            s.push_str("  bad.service loaded failed failed Broken Thing\n");
            let out = systemctl_list_units(&s);
            assert!(
                out.contains(&format!("[loaded active {sub} — 12] u0.unit · ")),
                "SUB `{sub}` did not fold:\n{out}"
            );
            assert!(
                out.contains("bad.service"),
                "the failure was folded away:\n{out}"
            );
        }
    }

    /// A run of ONE is not a fold: it swaps a unit line for a banner line and
    /// loses the unit's name to say so.
    #[test]
    fn a_run_of_one_keeps_the_unit_line() {
        let mut s = String::from("  UNIT LOAD ACTIVE SUB DESCRIPTION\n");
        for i in 0..12 {
            s.push_str(&format!("  lone{i}.unit loaded active running Thing {i}\n"));
            s.push_str(&format!(
                "  bad{i}.service loaded failed failed Broken {i}\n"
            ));
        }
        let out = systemctl_list_units(&s);
        assert!(
            !out.contains("healthy units folded"),
            "a run of one was banner-folded:\n{out}"
        );
        for i in 0..12 {
            assert!(
                out.contains(&format!("lone{i}.unit")),
                "lost lone{i}:\n{out}"
            );
        }
    }

    /// The third column is still checked: a DESCRIPTION carrying the words
    /// `loaded active` must not fold the unit it describes.
    #[test]
    fn a_description_is_not_a_sub_state() {
        let mut s = String::from("  UNIT LOAD ACTIVE SUB DESCRIPTION\n");
        for i in 0..12 {
            s.push_str(&format!("  ok{i}.unit loaded active running Thing {i}\n"));
        }
        s.push_str("  reload.service not-found inactive dead Reload loaded active configuration\n");
        let out = systemctl_list_units(&s);
        assert!(
            out.contains("reload.service"),
            "prose in DESCRIPTION folded a unit away:\n{out}"
        );
    }

    #[test]
    fn systemctl_list_small_passes_through() {
        // <10 healthy units → nothing worth folding.
        let body = systemctl_sample(3, false);
        let out = systemctl_list_units(&body);
        assert!(out.contains("svc0.service") && out.contains("svc2.service"));
        assert!(!out.contains("healthy units folded"));
    }

    #[test]
    fn systemctl_dispatch_routes_listing_and_status() {
        // Bare `systemctl` and flag-only forms default to list-units;
        // `status` keeps its bespoke compactor; other subcommands sniff.
        let listing = systemctl_sample(30, true);
        for a in [
            vec!["systemctl"],
            vec!["systemctl", "--type=service"],
            vec!["systemctl", "--failed"],
            vec!["systemctl", "list-units"],
        ] {
            let r = filter_command(&argv(&a), &listing);
            assert_eq!(r.filter, "systemctl-list", "{a:?} → list fold");
            assert!(r.output.contains("broken.service"), "{a:?} keeps failure");
        }
    }

    // --- stream filters route to content sniff, not generic dedup ---

    #[test]
    fn stream_filter_json_output_uses_format_compactor() {
        // `sed '…' data.json`, `head data.json`, … emit JSON. Like `cat`
        // (#502) they must reach the JSON compactor by sniffing the
        // content, not the old blind dedup+truncate `text_util` pass.
        let mut body = String::from("[\n");
        for i in 0..200 {
            body.push_str(&format!(
                "  {{\"id\": {i}, \"name\": \"item\", \"status\": \"active\", \"value\": 42}},\n"
            ));
        }
        body.push_str(
            "  {\"id\": 999, \"name\": \"last\", \"status\": \"active\", \"value\": 42}\n]\n",
        );
        for prog in &["sed", "head", "tail", "awk", "cut", "tr"] {
            let r = filter_command(&argv(&[prog, "x", "data.json"]), &body);
            assert_eq!(
                r.filter, "auto:json",
                "{prog} of JSON content should hit the JSON compactor, got {}",
                r.filter
            );
        }
    }

    #[test]
    fn stream_filter_unstructured_falls_back_to_generic() {
        // Arbitrary prose with no detectable format → auto detects
        // nothing → clean fall-through to the generic pass (never
        // panics, never the removed `text_util` label).
        let body: String = (0..150)
            .map(|i| format!("line {i} of plain unstructured output\n"))
            .collect();
        let r = filter_command(&argv(&["awk", "{print}", "notes.txt"]), &body);
        assert!(
            r.filter == "generic" || r.filter == "passthrough",
            "unstructured stream-filter output should fall back cleanly, got {}",
            r.filter
        );
    }

    // --- find_subcommand: regression coverage for #471 ---

    #[test]
    fn find_subcommand_kubectl_skips_token_value_flag() {
        // Regression for #471: --token used to be missing from the
        // kubectl value-flag list, so the auth token VALUE was returned
        // as the subcommand and the output got routed through the wrong
        // compactor (e.g. token-as-subcommand → auto fallback instead
        // of the tabular `get` path).
        let a = argv(&["kubectl", "--token", "abc123xyz", "get", "pods"]);
        assert_eq!(find_subcommand("kubectl", &a), "get");
    }

    #[test]
    fn find_subcommand_kubectl_skips_certificate_value_flags() {
        let a = argv(&[
            "kubectl",
            "--client-certificate",
            "/etc/k8s/cert.pem",
            "--client-key",
            "/etc/k8s/key.pem",
            "describe",
            "pod",
            "foo",
        ]);
        assert_eq!(find_subcommand("kubectl", &a), "describe");
    }

    #[test]
    fn find_subcommand_kubectl_skips_as_user_flag() {
        // --as / --as-group are RBAC impersonation flags.
        let a = argv(&["kubectl", "--as", "system:admin", "get", "secrets"]);
        assert_eq!(find_subcommand("kubectl", &a), "get");
    }

    #[test]
    fn find_subcommand_kubectl_handles_equals_form() {
        // `--token=X` is one token — already correct under the old code
        // but worth pinning so refactors don't regress it.
        let a = argv(&["kubectl", "--token=abcd1234", "get", "pods"]);
        assert_eq!(find_subcommand("kubectl", &a), "get");
    }

    #[test]
    fn find_subcommand_kubectl_namespace_short_form() {
        // `-n NS` was always in the list; pinned for refactor safety.
        let a = argv(&["kubectl", "-n", "kube-system", "get", "deployments"]);
        assert_eq!(find_subcommand("kubectl", &a), "get");
    }

    #[test]
    fn find_subcommand_git_skips_c_flag_value() {
        // `-C path` is git's "run as if in PATH". Sanity check that
        // git's value-flag list also still works.
        let a = argv(&["git", "-C", "/home/u/proj", "status"]);
        assert_eq!(find_subcommand("git", &a), "status");
    }

    #[test]
    fn find_subcommand_unknown_prog_returns_argv1_verbatim() {
        // The default branch (no value-flag table) just returns argv[1]
        // unchanged — even if it's a flag value.
        let a = argv(&["somebin", "subcmd"]);
        assert_eq!(find_subcommand("somebin", &a), "subcmd");
    }

    // --- find_subcommand: #472 cargo + docker coverage ---

    #[test]
    fn find_subcommand_cargo_skips_color_when_value() {
        // `cargo --color always test` used to return "always" — the
        // dispatcher then ran auto::compact instead of cargo_test_dispatch.
        // Common in CI scripts (`--color always` for log readability).
        let a = argv(&["cargo", "--color", "always", "test"]);
        assert_eq!(find_subcommand("cargo", &a), "test");
    }

    #[test]
    fn find_subcommand_cargo_skips_jobs_value_long_and_short() {
        let a = argv(&["cargo", "--jobs", "4", "test"]);
        assert_eq!(find_subcommand("cargo", &a), "test");
        let a = argv(&["cargo", "-j", "8", "build"]);
        assert_eq!(find_subcommand("cargo", &a), "build");
    }

    #[test]
    fn find_subcommand_cargo_skips_target_triple() {
        let a = argv(&["cargo", "--target", "x86_64-unknown-linux-musl", "build"]);
        assert_eq!(find_subcommand("cargo", &a), "build");
    }

    #[test]
    fn find_subcommand_cargo_skips_z_unstable_flag() {
        // `-Z FLAG` is the unstable-feature lever — separate value form.
        let a = argv(&["cargo", "-Z", "unstable-options", "test"]);
        assert_eq!(find_subcommand("cargo", &a), "test");
    }

    #[test]
    fn find_subcommand_cargo_boolean_flags_pass_through() {
        // --frozen / --locked / --offline take NO value, so the next
        // token IS the subcommand. Pin this so a future "fix" doesn't
        // accidentally add them to the value-flag list.
        let a = argv(&["cargo", "--frozen", "build"]);
        assert_eq!(find_subcommand("cargo", &a), "build");
        let a = argv(&["cargo", "--locked", "--offline", "test"]);
        assert_eq!(find_subcommand("cargo", &a), "test");
    }

    #[test]
    fn find_subcommand_cargo_skips_rustup_toolchain_override() {
        // #596 — `cargo +nightly build` returned "+nightly" instead of
        // "build" because the walker treats non-`-` tokens as the
        // subcommand. Toolchain overrides start with `+` and select
        // the toolchain that runs cargo — they're a SINGLE token (no
        // separate value). Skip them like a boolean flag.
        let a = argv(&["cargo", "+nightly", "build"]);
        assert_eq!(find_subcommand("cargo", &a), "build");

        // Common stable/beta/version specifiers.
        for tk in &["+stable", "+beta", "+1.75", "+1.76.0"] {
            let a = argv(&["cargo", tk, "test"]);
            assert_eq!(find_subcommand("cargo", &a), "test", "{tk} must be skipped");
        }

        // Combined with a value-flag: `cargo +nightly --target x test`.
        let a = argv(&["cargo", "+nightly", "--target", "wasm32", "test"]);
        assert_eq!(find_subcommand("cargo", &a), "test");

        // Bare `+` (no toolchain name) is NOT a toolchain override —
        // pass through so it's reported as the subcommand for whatever
        // downstream code wants to see.
        let a = argv(&["cargo", "+", "build"]);
        assert_eq!(find_subcommand("cargo", &a), "+");
    }

    #[test]
    fn find_subcommand_plus_prefix_only_special_for_cargo() {
        // Other progs don't treat `+` as a toolchain override. Make
        // sure the `+toolchain` skip is gated on prog == "cargo".
        let a = argv(&["git", "+foo", "log"]);
        assert_eq!(find_subcommand("git", &a), "+foo");
    }

    #[test]
    fn find_subcommand_docker_skips_log_level() {
        // `docker --log-level debug ps` used to return "debug".
        let a = argv(&["docker", "--log-level", "debug", "ps"]);
        assert_eq!(find_subcommand("docker", &a), "ps");
    }

    #[test]
    fn find_subcommand_docker_skips_tlscacert_value() {
        let a = argv(&["docker", "--tlscacert", "/etc/ca.pem", "images"]);
        assert_eq!(find_subcommand("docker", &a), "images");
    }

    #[test]
    fn user_wants_raw_detects_verbose_flags() {
        // Long forms — pattern-matched, not enumerated.
        assert!(user_wants_raw(&argv(&[
            "cargo",
            "test",
            "--",
            "--show-output"
        ])));
        assert!(user_wants_raw(&argv(&[
            "cargo",
            "test",
            "--",
            "--nocapture"
        ])));
        assert!(user_wants_raw(&argv(&["foo", "--verbose"])));
        assert!(user_wants_raw(&argv(&["foo", "--debug"])));
        assert!(user_wants_raw(&argv(&["foo", "--trace"])));
        assert!(user_wants_raw(&argv(&["foo", "--show-progress"])));
        assert!(user_wants_raw(&argv(&["foo", "--print-stats"])));
        assert!(user_wants_raw(&argv(&["foo", "--verbose=true"])));
        // Short forms: -v, -vv, -vvv, …
        // On a program that HAS a verbosity dial. `ls` used to stand here and
        // does not have one: its `-v` is `--sort=version`.
        assert!(user_wants_raw(&argv(&["chmod", "-v", "644", "f"])));
        assert!(user_wants_raw(&argv(&["foo", "-vv"])));
        assert!(user_wants_raw(&argv(&["foo", "-vvv"])));
        // NOT a verbose flag (just an arg starting with -).
        assert!(!user_wants_raw(&argv(&["ls", "-la"])));
        assert!(!user_wants_raw(&argv(&["cargo", "test"])));
        assert!(!user_wants_raw(&argv(&["foo", "-l"])));
    }

    #[test]
    fn a_powershell_cmdlet_routes_like_the_bare_cmdlet() {
        // The host name is scaffolding. Before this, `Get-Process` compacted
        // and `powershell Get-Process` did not, because `powershell` is a known
        // program name and reached the generic tail with the cmdlet unread.
        let table: String = std::iter::once(
            "Handles  NPM(K)    PM(K)      WS(K)     CPU(s)     Id  SI ProcessName\n".to_string(),
        )
        .chain((0..200).map(|i| {
            format!("   {i:4}      42    12345      54321      1.25  {i:5}   1 proc{i}\n")
        }))
        .collect();
        let bare = filter_command(&argv(&["Get-Process"]), &table);
        assert!(
            bare.savings.tokens_after < bare.savings.tokens_before,
            "the bare cmdlet must compact for this test to mean anything"
        );
        for host in ["powershell", "powershell.exe", "pwsh"] {
            let r = filter_command(&argv(&[host, "Get-Process"]), &table);
            assert_eq!(
                r.savings.tokens_after, bare.savings.tokens_after,
                "{host} Get-Process must deliver what bare Get-Process delivers"
            );
        }
    }

    /// `--json` asks for values to read exactly: no 200-character cut, no
    /// 20-item cap, no audit key dropped — while the document fits.
    #[test]
    fn json_the_command_asked_for_is_delivered_whole() {
        let _g = tk_base::delivery::threshold_on_this_thread(5_000);
        let claim = "a measured claim, ".repeat(20);
        let items: Vec<String> = (0..25)
            .map(|i| format!("{{\"id\": {i}, \"label\": \"link {i}\"}}"))
            .collect();
        let doc = format!(
            "{{\n  \"claim\": \"{claim}\",\n  \"updated_at\": \"2026-09-12T12:14:59Z\",\n  \"links\": [{}]\n}}\n",
            items.join(", ")
        );
        let r = filter_command(&argv(&["acme-cli", "identity", "show", "--json"]), &doc);
        assert!(r.output.contains(claim.trim_end()), "{}", r.output);
        assert!(
            r.output.contains("\"updated_at\":\"2026-09-12T12:14:59Z\""),
            "{}",
            r.output
        );
        assert!(
            r.output.contains("{\"id\":24,\"label\":\"link 24\"}"),
            "{}",
            r.output
        );
        assert!(!r.output.contains('\n'), "minified:\n{}", r.output);
        // Not asked for: the JSON compactor still decides.
        let r = filter_command(&argv(&["acme-cli", "identity", "show"]), &doc);
        assert_ne!(r.filter, "json-requested", "{}", r.output);
    }

    #[test]
    fn a_package_manager_subcommand_reaches_its_own_module() {
        // `brew_outdated` existed as a module and nothing routed to it: the
        // pkgmgr dispatcher only looked at the program name. Asserted through
        // `filter_command` rather than the dispatcher, so a future arm that
        // shadows this one is caught too. The module's own job — reduce the
        // auto-update report, deliver every outdated package — is what
        // proves it ran.
        let new: String = (0..80)
            .map(|i| format!("formula-{i}: A tool that does thing number {i} well\n"))
            .collect();
        let listing = format!(
            "==> Auto-updating Homebrew...\n==> Auto-updated Homebrew!\n\
             Updated 1 tap (homebrew/core).\n==> New Formulae\n{new}\n\
             cairo\nopenssl@3\npython@3.12\n"
        );
        let r = filter_command(&argv(&["brew", "outdated"]), &listing);
        assert!(
            r.output.ends_with("cairo\nopenssl@3\npython@3.12")
                && !r.output.contains("thing number"),
            "brew outdated (filter={}):\n{}",
            r.filter,
            r.output
        );
        // A subcommand with no module of its own must still be harmless.
        let r2 = filter_command(&argv(&["brew", "no-such-subcommand"]), &listing);
        assert!(
            r2.savings.tokens_after <= r2.savings.tokens_before,
            "an unknown subcommand must never inflate"
        );
    }

    #[test]
    fn short_v_is_not_verbose_for_grep_and_iptables() {
        // `-v` on grep is --invert-match and on iptables it adds counter
        // columns. Neither is a request for raw output, and reading the letter
        // as one cost 3 701 grep calls their compaction on this machine alone.
        // `awk -v` assigns a variable and `sort -v` picks version order — two
        // more programs with no verbosity dial at all, where reading the letter
        // as one only ever costs the fold.
        assert!(!user_wants_raw(&argv(&["awk", "-v", "x=1", "{print}"])));
        assert!(!user_wants_raw(&argv(&["sort", "-v"])));
        assert!(!user_wants_raw(&argv(&["gawk", "-v", "n=2", "{print n}"])));
        for prog in ["grep", "egrep", "fgrep", "rg", "iptables", "ip6tables"] {
            assert!(
                !user_wants_raw(&argv(&[prog, "-rn", "-v", "pattern"])),
                "{prog} -v must not read as verbose"
            );
            assert!(
                !user_wants_raw(&argv(&[prog, "-v"])),
                "{prog} -v alone must not read as verbose"
            );
        }
        // The bundled form never had the bug; it must still be unaffected.
        assert!(!user_wants_raw(&argv(&["grep", "-rnv", "pattern"])));

        // The exemption is for the SHORT form only. A long verbose flag still
        // means what it says, on these programs as on any other.
        assert!(user_wants_raw(&argv(&["grep", "--verbose", "x"])));
        assert!(user_wants_raw(&argv(&["rg", "--debug", "x"])));

        // And it must not leak to programs where -v IS verbosity. Named
        // explicitly rather than "some other program", so emptying the list
        // cannot make this test pass by accident.
        assert!(user_wants_raw(&argv(&["cargo", "build", "-v"])));
        assert!(user_wants_raw(&argv(&["tar", "-v", "-x", "-f", "a.tar"])));
        assert!(user_wants_raw(&argv(&["ldd", "-v", "/usr/bin/git"])));

        // Resolved through the same scaffolding-aware path as the router, so a
        // `cd … &&` prefix or an env assignment cannot reintroduce the bug.
        assert!(!user_wants_raw(&argv(&[
            "cd", "/tmp", "&&", "grep", "-rn", "-v", "x"
        ])));
        assert!(!user_wants_raw(&argv(&[
            "LC_ALL=C", "grep", "-rn", "-v", "x"
        ])));
    }

    #[test]
    fn grep_with_a_separate_v_still_compacts_end_to_end() {
        // The predicate above is one defence; this is the delivery. A guard on
        // the predicate alone would pass while `filter_command` still returned
        // the bytes untouched for some other reason.
        let out: String = (0..400)
            .map(|i| format!("crates/tokenade-core/src/filter.rs:{i}:fn helper_{i}() {{\n"))
            .collect();
        let compacted = filter_command(&argv(&["grep", "-rn", "-v", "test", "crates/"]), &out);
        assert!(
            compacted.savings.tokens_after < compacted.savings.tokens_before,
            "grep -rn -v delivered {} tokens for {} in (filter={})",
            compacted.savings.tokens_after,
            compacted.savings.tokens_before,
            compacted.filter
        );
        assert_ne!(
            compacted.filter, "passthrough",
            "grep -rn -v must not passthrough"
        );
    }

    /// The membership rule for [`SHORT_V_IS_NOT_VERBOSE`], asserted program by
    /// program: a short `-v` on a program with no verbosity dial is not a
    /// request for raw bytes.
    ///
    /// Each meaning is written next to its program because that is the only
    /// thing a future reader can check the list against. `ls` is first because
    /// it was the one the old tests named as the counter-example.
    #[test]
    fn a_short_v_that_is_not_a_verbosity_dial_still_folds() {
        for (prog, meaning) in [
            ("ls", "--sort=version"),
            ("od", "--output-duplicates"),
            ("hexdump", "--no-squeezing"),
            ("nl", "--starting-line-number"),
            ("join", "unpairable lines from FILENUM"),
            ("grep", "--invert-match"),
            ("sort", "--version-sort"),
            ("awk", "a variable assignment"),
            ("iptables", "the counter columns"),
            // Surveyed 2026-10-02 against `--help` on this machine.
            ("docker", "--volume (run/create)"),
            ("podman", "--volume (run/create)"),
            ("psql", "--set NAME=VALUE"),
            ("ffmpeg", "the log level, usually lowered"),
            ("node", "--version"),
            ("pnpm", "--version"),
            ("yarn", "--version"),
            ("bun", "--version"),
            ("perl", "the version banner"),
            ("make", "--version"),
            ("diff", "--version"),
            ("patch", "--version"),
            ("file", "--version"),
            ("lsof", "the version info"),
            ("cat", "--show-nonprinting"),
            ("head", "the file-name header"),
            ("tail", "the file-name header"),
            // Second survey, 2026-10-02.
            ("pgrep", "--inverse"),
            ("pkill", "--inverse"),
            ("findmnt", "--nofsroot"),
            ("lsblk", "--virtio"),
            ("free", "--committed"),
            ("uname", "--kernel-version"),
            ("pr", "--show-nonprinting"),
            ("tree", "a version sort"),
            ("cmp", "--version"),
            ("xxd", "the version"),
            ("bc", "--version"),
        ] {
            assert!(
                !user_wants_raw(&argv(&[prog, "-v", "x"])),
                "`{prog} -v` is {meaning}, not a verbosity dial"
            );
        }

        // The separated and the bundled spelling must agree — the whole point
        // of the exemption is that `ls -v -R` and `ls -Rv` are one command.
        assert!(!user_wants_raw(&argv(&["ls", "-v", "-R", "crates"])));
        assert!(!user_wants_raw(&argv(&["ls", "-Rv", "crates"])));

        // Control: the exemption is per program, and a long verbose flag means
        // what it says even on the exempted ones.
        assert!(user_wants_raw(&argv(&["mkdir", "-v", "d"])));
        // Programs whose `-v` IS the verbosity dial stay out of the table.
        for prog in [
            "curl", "tar", "rsync", "pytest", "cargo", "pip", "gzip", "python3", "mount", "lsusb",
            "ldd", "xz", "zstd", "ping",
        ] {
            assert!(
                user_wants_raw(&argv(&[prog, "-v", "x"])),
                "`{prog} -v` is verbose"
            );
        }
        // The volume mount of a real `docker run` line.
        assert!(!user_wants_raw(&argv(&[
            "docker",
            "run",
            "--rm",
            "-v",
            "/src:/src",
            "img",
            "pytest"
        ])));
        assert!(user_wants_raw(&argv(&["ls", "--verbose"])));
    }

    /// A command asking for its own reference text gets it whole.
    ///
    /// The fold's savings there came out of the option table: `tar --help`
    /// folded from 46 short options to 12. And no fold that keeps the table
    /// pays — option lines are 26-99% of a help text.
    #[test]
    fn a_help_request_is_an_explicit_request_for_the_reference() {
        for spelling in [
            vec!["tar", "--help"],
            vec!["less", "--help"],
            vec!["curl", "--help", "all"],
            vec!["ffmpeg", "--usage"],
            vec!["some.exe", "/?"],
            vec!["oldtool", "-?"],
            // The subcommand form, where four of the tools an agent uses put
            // it.
            vec!["git", "help"],
            vec!["go", "help"],
            vec!["docker", "help"],
            vec!["openssl", "help"],
            vec!["git", "help", "log"],
        ] {
            let a = argv(&spelling);
            assert!(user_wants_raw(&a), "{spelling:?} asked for the reference");
        }
    }

    /// `-h` is `--human-readable` far more often than it is help, and `help`
    /// three arguments in is a file name.
    #[test]
    fn a_short_h_and_a_late_help_are_not_a_help_request() {
        for spelling in [
            vec!["ls", "-lh", "/usr"],
            vec!["du", "-h", "crates"],
            vec!["df", "-h"],
            vec!["free", "-h"],
            vec!["sort", "-h", "sizes.txt"],
            // A path that happens to be called help.
            vec!["cp", "notes.txt", "help"],
            vec!["cat", "docs/help"],
        ] {
            let a = argv(&spelling);
            assert!(
                !user_wants_raw(&a),
                "{spelling:?} did not ask for a reference text"
            );
        }
    }

    /// `env` with nothing to launch answers exactly like `printenv`.
    ///
    /// `env` is the case that matters: it is a launcher AND the command that
    /// prints the environment, and the second spelling ended in the plain
    /// generic pass. Measured on a real environment, 822 bytes came back as
    /// 786 that way and 354 through the content detector — while `printenv`,
    /// which never enters the wrapper branch, got the smaller answer all
    /// along.
    /// `set` is the shell's own environment dump: a real one (bash, 108
    /// lines) lost its 53 middle lines — `PATH`, `PWD`, `SHELL` — to a
    /// head/tail cut. It answers like `printenv`, every variable kept.
    /// `driverquery` lists drivers: a real one (371, under the threshold once
    /// respaced) kept 50. Every driver is delivered while the table fits.
    #[test]
    fn driverquery_delivers_every_driver_that_fits() {
        let mut body = String::from(
            "\nModule Name  Display Name           Driver Type   Link Date\n\
             ============ ====================== ============= ======================\n",
        );
        for i in 0..70 {
            let _ = writeln!(
                body,
                "drv{i:<9} Driver number {i:<8} Kernel        19/05/2015 00:28:03"
            );
        }
        let got = filter_command(&argv(&["driverquery"]), &body).output;
        for i in 0..70 {
            assert!(
                got.contains(&format!("drv{i}  Driver number {i}  Kernel")),
                "drv{i}:\n{got}"
            );
        }
    }

    /// `wmic process list brief` (no rule under its header): a real one kept
    /// 45 processes of 80, with no pointer to the rest.
    #[test]
    fn wmic_delivers_every_row_that_fits() {
        let mut body = String::from("HandleCount  Name                    Priority  ProcessId  \n");
        body.push_str("0            System Idle Process     0         0          \n");
        for i in 0..70 {
            let _ = writeln!(
                body,
                "180          svchost.exe             8         {:<5}      ",
                2000 + i
            );
        }
        let got = filter_command(&argv(&["wmic", "process", "list", "brief"]), &body).output;
        assert!(got.contains("0  System Idle Process  0  0\n"), "{got}");
        for i in 0..70 {
            assert!(
                got.contains(&format!("180  svchost.exe  8  {}", 2000 + i)),
                "{}:\n{got}",
                2000 + i
            );
        }
    }

    /// Listings whose every line is an answer are given whole while they fit:
    /// the old reflog entries are what `git reflog` is read for, the middle
    /// tags are versions, every config key is a setting, every `cargo update`
    /// line a version change, every `cargo search` line a candidate crate.
    #[test]
    fn git_listings_that_fit_keep_their_middle() {
        // The default threshold, whatever the session's pressure.
        let _threshold = tk_base::delivery::threshold_on_this_thread(5_000);
        // Real reflog entries carry a full commit subject (~110 chars): a
        // listing this size is past the pipeline's small-output safety net.
        let tags: String = (0..120).map(|i| format!("v1.{i}.0\n")).collect();
        // Names that differ as words do (`serde`, `tokio`…), not one template
        // with a numbered slot — a template is another fold's business.
        let word = |i: usize| -> String {
            let syl = ["ka", "lo", "mi", "nu", "pe", "ri", "so", "tu"];
            format!("{}{}{}", syl[i % 8], syl[(i / 8) % 8], syl[(i / 64) % 8])
        };
        // Subjects in WORDS: since a digit-leading hash is one `<HEX>` slot,
        // `change number {i}` made the reflog one numbered template.
        let reflog: String = (0..50)
            .map(|i| format!("{:08x} HEAD@{{{i}}}: commit: {} {} — a subject as long as the ones this repository writes, about a hundred chars\n", 0x1000_0000u32 + i, word(i as usize), word(i as usize + 11)))
            .collect();
        let update: String = (0..100)
            .map(|i| {
                format!(
                    "    Updating {} v0.{}.{} -> v0.{}.{}\n",
                    word(i),
                    i % 7,
                    i,
                    i % 7,
                    i + 1
                )
            })
            .collect();
        let search: String = (0..50)
            .map(|i| {
                format!(
                    "{} = \"0.{i}.0\"    # {} — a description as long as crates.io shows them\n",
                    word(i),
                    word(i + 7)
                )
            })
            .collect();
        let config: String = (0..110)
            .map(|i| format!("alias.a{i}=log --oneline -{i}\n"))
            .collect();
        let reflog_probe = format!("HEAD@{{42}}: commit: {} {}", word(42), word(53));
        for (cmd, body, probe) in [
            (vec!["git", "reflog"], &reflog, reflog_probe.as_str()),
            (vec!["git", "tag"], &tags, "v1.75.0"),
            (
                vec!["git", "config", "--list"],
                &config,
                "alias.a90=log --oneline -90",
            ),
            (vec!["cargo", "update"], &update, " v0.3.45 -> v0.3.46"),
            (vec!["cargo", "search", "serde"], &search, " = \"0.33.0\""),
        ] {
            let got = filter_command(&argv(&cmd), body).output;
            assert!(got.contains(probe), "{cmd:?} lost {probe}:\n{got}");
            assert!(
                !got.contains("elided"),
                "{cmd:?} cut a listing that fits:\n{got}"
            );
        }
    }

    /// `git show` of one commit keeps its header and its file list, even
    /// when that barely folds and a summary would be shorter.
    #[test]
    fn git_show_of_one_commit_is_not_traded_for_a_summary() {
        let raw = "commit 98c8e93fda0deb6d8c5c4fe952b2f1d5e68882ab\nAuthor: A Person <a@example.org>\nDate:   Sat Oct 3 00:06:47 2026 +0200\n\n    Un identifiant à forme d'IBAN n'est plus masqué\n\n PATCHNOTES.md                           |  1 +\n crates/tk-base/src/redact/pii.rs        | 29 ++++++++++++\n crates/tk-base/src/redact/structured.rs |  2 +-\n 3 files changed, 31 insertions(+), 1 deletion(-)\n";
        let out = filter_command(&argv(&["git", "show", "--stat", "HEAD~3"]), raw).output;
        for needle in [
            "98c8e93fda0deb6d8c5c4fe952b2f1d5e68882ab",
            "Author: A Person",
            "Date:",
            "crates/tk-base/src/redact/structured.rs",
        ] {
            assert!(out.contains(needle), "{needle}:\n{out}");
        }
    }

    /// A named rendering that barely folds is not traded for a shorter
    /// fallback that drops what it kept: every loaded job stays named.
    #[test]
    fn launchctl_list_is_not_traded_for_a_shorter_head() {
        let mut body = String::from("PID\tStatus\tLabel\n22271\t0\tcom.apple.trustd.agent\n385\t0\tcom.apple.cfprefsd.xpc.agent\n");
        let parts = [
            "mdworker",
            "coreimportd",
            "ctkd",
            "secinitd",
            "lsd",
            "pkd",
            "netauth",
            "fskit",
            "keychain",
            "tonelibraryd",
        ];
        for i in 0..50 {
            let _ = writeln!(
                body,
                "-\t0\tcom.apple.{}.{}{}",
                parts[i % 10],
                parts[(i / 10) % 10],
                i
            );
        }
        let out = filter_command(&argv(&["launchctl", "list"]), &body).output;
        for i in [0usize, 25, 49] {
            let label = format!("com.apple.{}.{}{}", parts[i % 10], parts[(i / 10) % 10], i);
            assert!(out.contains(&label), "{label}:\n{out}");
        }
    }

    #[test]
    fn set_answers_like_printenv() {
        let mut body = String::new();
        for i in 0..120 {
            let _ = writeln!(body, "VAR_{i:03}=value-{i:03}");
        }
        body.push_str("XDG_SESSION_TYPE=wayland\n");
        let got = filter_command(&argv(&["set"]), &body).output;
        assert_eq!(got, filter_command(&argv(&["printenv"]), &body).output);
        for i in 0..120 {
            assert!(
                got.contains(&format!("VAR_{i:03}=value-{i:03}\n")),
                "VAR_{i:03}:\n{got}"
            );
        }
    }

    #[test]
    fn env_with_nothing_to_launch_answers_like_printenv() {
        let mut body = String::new();
        for i in 0..40 {
            let _ = writeln!(body, "VAR_{i:02}=value-{i:02}");
        }
        for noisy in [
            "XDG_SESSION_TYPE=wayland",
            "GTK_MODULES=canberra",
            "QT_QPA_PLATFORM=xcb",
        ] {
            let _ = writeln!(body, "{noisy}");
        }
        let reference = filter_command(&argv(&["printenv"]), &body).output;
        for spelling in [vec!["env"], vec!["env", "-0"], vec!["env", "FOO=1"]] {
            let got = filter_command(&argv(&spelling), &body).output;
            assert_eq!(
                got, reference,
                "{spelling:?} must answer like `printenv` on the same bytes"
            );
        }
        assert!(
            reference.len() < body.len(),
            "anti-vacuity: the fixture must actually fold ({} vs {})",
            reference.len(),
            body.len()
        );
        assert!(
            reference.contains("vars elided"),
            "that body is one the env compactor WINS on: {reference}"
        );

        // And one it loses on. Secret-named variables come back longer than
        // they went in, so the route falls through to the content detector —
        // the other half of `printenv`'s path, and the half a fixture where
        // the compactor always wins can never reach.
        let loses = "INTERNAL_API_KEY=hunter2\nDB_PASSWORD=letmein\nCI_SECRET=abc123\n\
                     HOME=/home/u\nPATH=/usr/bin\nLANG=C\nSHELL=/bin/bash\n\
                     TERM=xterm\nEDITOR=vim\n";
        let fallback = filter_command(&argv(&["printenv"]), loses).output;
        assert!(
            !fallback.contains("vars elided"),
            "this fixture must exercise the FALLBACK, not the compactor: {fallback}"
        );
        assert_eq!(
            filter_command(&argv(&["env"]), loses).output,
            fallback,
            "`env` must follow `printenv` on the fallback half too"
        );
    }

    /// And a wrapper that DOES wrap still routes by the wrapped command.
    #[test]
    fn a_wrapper_with_something_wrapped_still_routes_by_it() {
        let mut body = String::from("On branch main\nChanges not staged for commit:\n");
        for i in 0..40 {
            let _ = writeln!(body, "\tmodified:   src/file_{i:02}.rs");
        }
        let direct = filter_command(&argv(&["git", "status"]), &body).output;
        for spelling in [
            vec!["env", "FOO=1", "git", "status"],
            vec!["sudo", "git", "status"],
            vec!["timeout", "300", "git", "status"],
        ] {
            assert_eq!(
                filter_command(&argv(&spelling), &body).output,
                direct,
                "{spelling:?} must route as the wrapped command"
            );
        }
    }

    #[test]
    fn user_wants_raw_passthrough_on_any_pipe() {
        // ANY pipe triggers — not just specific reshape tools.
        assert!(user_wants_raw(&argv(&[
            "cargo", "build", "|", "tail", "-5"
        ])));
        assert!(user_wants_raw(&argv(&["git", "log", "|", "head", "-20"])));
        assert!(user_wants_raw(&argv(&["find", ".", "|", "wc", "-l"])));
        assert!(user_wants_raw(&argv(&[
            "kubectl", "get", "pods", "|", "jq", "."
        ])));
        // Novel tool downstream still triggers (previously enumerate-only).
        assert!(user_wants_raw(&argv(&["cmd", "|", "my_custom_filter"])));
        assert!(user_wants_raw(&argv(&["a", "|", "b", "|", "c"])));
    }

    /// The half that was never guarded.
    ///
    /// `user_wants_raw` had tests; the BEHAVIOUR it produced had none, which
    /// is why changing a pipeline from passthrough to a content route left
    /// 8 000 tests green. A predicate test is not a behaviour test.
    #[test]
    fn a_pipe_is_routed_by_content_not_passed_through() {
        let body: String = (0..400)
            .map(|i| format!("src/mod_{}.rs:{i}:    let v = compute({i});\n", i % 40))
            .collect();
        let before = tokenizer::estimate(&body);
        let piped = argv(&["grep", "-n", "compute", ".", "|", "head", "-400"]);
        let r = filter_command(&piped, &body);
        assert!(
            r.savings.tokens_after < before,
            "a pipeline's output must still be routed by its own shape: \
             {} -> {}",
            before,
            r.savings.tokens_after
        );
        assert_ne!(r.filter, "passthrough", "the pipe branch no longer bails");

        // …and it gives the SAME answer as the same command unpiped. That
        // agreement is the whole argument: a pipeline ending in `head` is the
        // case `head` alone already had, answered two different ways.
        let alone = filter_command(&argv(&["grep", "-n", "compute", "."]), &body);
        assert_eq!(
            r.savings.tokens_after, alone.savings.tokens_after,
            "piped and unpiped disagree: {} vs {}",
            r.savings.tokens_after, alone.savings.tokens_after
        );
    }

    /// The two halves of the old predicate are NOT interchangeable, and this
    /// pins the asymmetry: an explicit request for raw bytes gets them byte
    /// for byte while they fit under the delivery threshold, and a
    /// byte-precise slice gets them at any size.
    ///
    /// Above the threshold a verbose request is compacted by the program's
    /// own rules ([`raw_request_honoured`]): the next stage would only have
    /// cut the passthrough to its head and tail.
    #[test]
    fn explicit_intent_still_passes_through_byte_for_byte() {
        let lines = |n: usize| -> String {
            (0..n)
                .map(|i| format!("src/mod_{}.rs:{i}:    let v = compute({i});\n", i % 40))
                .collect()
        };
        let small = lines(120);
        let large = lines(2_000);
        assert!(tokenizer::estimate(&small) <= crate::disclose::threshold_tokens());
        assert!(tokenizer::estimate(&large) > crate::disclose::threshold_tokens());
        let verbose = [
            argv(&["cargo", "test", "--", "--nocapture"]),
            argv(&["mytool", "--verbose"]),
            argv(&["mytool", "-vvv"]),
        ];
        // A byte-precise slice is a question about EXACT bytes; a pipe in
        // front of it does not make it any less exact.
        let slice = argv(&["grep", "x", ".", "|", "head", "-c", "4096"]);
        for a in verbose.iter().chain([&slice]) {
            let r = filter_command(a, &small);
            assert!(
                r.output == small,
                "must be byte-for-byte for {a:?}: got {} bytes for {} in",
                r.output.len(),
                small.len()
            );
        }
        assert_eq!(
            filter_command(&slice, &large).output,
            large,
            "a slice is exact at any size"
        );
        for a in &verbose {
            let r = filter_command(a, &large);
            assert!(
                r.output.len() < large.len(),
                "over the threshold {a:?} must be compacted"
            );
        }
        // Help is always whole.
        let help = argv(&["mytool", "--help"]);
        assert_eq!(filter_command(&help, &large).output, large);
    }

    #[test]
    fn user_wants_raw_ignores_redirects_and_2_to_1() {
        // 2>&1 + > redirects don't reshape — argv[0]'s compactor remains correct.
        assert!(!user_wants_raw(&argv(&["cargo", "build", "2>&1"])));
        assert!(!user_wants_raw(&argv(&["cmd", ">", "out.log"])));
        assert!(!user_wants_raw(&argv(&["cmd", ">>", "out.log"])));
    }

    /// A trailing bare `| cat` is a pager-defeat idiom, not a
    /// pipeline: nothing downstream consumes the bytes, so compacting them
    /// corrupts nothing. Agents append it constantly and it was silently
    /// costing them compaction on every `git log | cat`.
    #[test]
    fn trailing_bare_cat_is_not_a_real_pipeline() {
        // The case that motivated it.
        assert!(!argv_shape_is_unknowable(&argv(&[
            "git", "log", "|", "cat"
        ])));
        assert!(!argv_shape_is_unknowable(&argv(&[
            "git",
            "log",
            "--oneline",
            "|",
            "cat"
        ])));
        // But `cat` doing anything else IS a real stage.
        assert!(argv_shape_is_unknowable(&argv(&[
            "git", "log", "|", "cat", "-n"
        ])));
        // `cat` not last is a real stage, so the argv stays unroutable. The
        // old comment here said folding "would corrupt grep's input", and
        // that was never the hazard on this path: `filter_command` is handed
        // what the WHOLE pipeline already printed, so grep has run and
        // nothing downstream is waiting. The real hazard is the stated one —
        // argv[0] does not name this shape — and the answer to it is the
        // content router, not a passthrough.
        assert!(argv_shape_is_unknowable(&argv(&[
            "git", "log", "|", "cat", "|", "grep", "x"
        ])));
        // cat NOT last is a real stage.
        assert!(argv_shape_is_unknowable(&argv(&[
            "cat", "f", "|", "wc", "-l"
        ])));
        // Ordinary pipelines are untouched.
        assert!(argv_shape_is_unknowable(&argv(&[
            "git", "log", "|", "head", "-5"
        ])));
    }

    #[test]
    fn user_wants_raw_passes_simple_commands() {
        assert!(!user_wants_raw(&argv(&["cargo", "build"])));
        assert!(!user_wants_raw(&argv(&["git", "status"])));
        assert!(!user_wants_raw(&argv(&["ls", "-la", "src/"])));
        assert!(!user_wants_raw(&argv(&[])));
        // Chain operators no longer trigger user_wants_raw here — the
        // PreToolUse Bash hook handles them via split-wrap (each
        // segment gets its own dispatch). This function focuses on
        // intent flags + pipe.
        assert!(!user_wants_raw(&argv(&["cd", "/tmp", "&&", "cat", "file"])));
        assert!(!user_wants_raw(&argv(&["cmd_a", "||", "cmd_b"])));
    }

    #[test]
    fn search_head_slice_detects_common_shapes() {
        // Bare `-N`, `-n N`, `-nN`, `--lines=N`, default 10.
        assert_eq!(search_head_slice_n("grep foo big.log | head -20"), Some(20));
        assert_eq!(search_head_slice_n("rg needle . | head -n 5"), Some(5));
        assert_eq!(search_head_slice_n("grep -i x f | head -n30"), Some(30));
        assert_eq!(search_head_slice_n("egrep p f | head --lines=15"), Some(15));
        assert_eq!(search_head_slice_n("grep x f | head"), Some(10));
        assert_eq!(search_head_slice_n("grep x f | tail -3"), Some(3));
        // Env-prefixed search stage still recognised.
        assert_eq!(search_head_slice_n("LC_ALL=C grep x f | head -7"), Some(7));
        // Multi-stage: search anywhere before the final head.
        assert_eq!(
            search_head_slice_n("cat f | grep x | sort | head -9"),
            Some(9)
        );
    }

    #[test]
    fn search_head_slice_rejects_non_matching_shapes() {
        assert_eq!(search_head_slice_n("grep foo big.log"), None); // no slice
        assert_eq!(search_head_slice_n("cat f | head -20"), None); // no search
        assert_eq!(search_head_slice_n("grep x f | wc -l"), None); // last != head
        assert_eq!(search_head_slice_n("grep x f | head -5 | grep y"), None); // head not last
        assert_eq!(search_head_slice_n("ls -la"), None);
    }

    #[test]
    fn is_env_assignment_token_matches_shell_env_prefix() {
        assert!(super::is_env_assignment_token("LANG=C"));
        assert!(super::is_env_assignment_token("FOO=bar"));
        assert!(super::is_env_assignment_token("MY_VAR=value"));
        assert!(super::is_env_assignment_token("PATH_2=x"));
        // NOT env assignments:
        assert!(!super::is_env_assignment_token("--message-format=short")); // starts with `-`
                                                                            // A lowercase name IS an assignment in program position — the shell's
                                                                            // rule. This line asserted the opposite for the router's own
                                                                            // copy, which is how that copy kept the bug the other one had lost.
        assert!(super::is_env_assignment_token("file=name"));
        assert!(!super::is_env_assignment_token("1x=name")); // not a variable name
        assert!(!super::is_env_assignment_token("=value")); // empty key
        assert!(!super::is_env_assignment_token("plain")); // no =
    }

    #[test]
    fn filter_command_skips_env_prefix() {
        // After my env-prefix skip, `LANG=C grep …` must dispatch
        // through the grep arm. The exact filter label after safety
        // nets may be "grep" (no overhead) or "generic" (grep
        // banner tied/inflated → SAFETY NET 2 fallback). What MUST
        // NOT happen: filter == one of the auto:* variants, which
        // would mean the dispatch routed to auto_or_generic because
        // the env-prefix confused argv[0].
        let mut big_output = String::new();
        for i in 0..200 {
            let _ = writeln!(
                big_output,
                "src/big.rs:{}:match number {} here in code",
                i, i
            );
        }
        let av = argv(&["LANG=C", "grep", "foo", "bar.txt"]);
        let r = filter_command(&av, &big_output);
        assert!(
            !r.filter.starts_with("auto:"),
            "env-prefix dispatch should not fall to auto: variants, got {}",
            r.filter
        );
        // Multiple env prefixes — same expectation.
        let av2 = argv(&["FOO=1", "BAR=2", "grep", "x", "."]);
        let r2 = filter_command(&av2, &big_output);
        assert!(!r2.filter.starts_with("auto:"), "got {}", r2.filter);
    }

    #[test]
    fn dispatches_bun_to_npm_group() {
        let bulk: String = (0..50)
            .map(|i| format!("line {i} of fake output text\n"))
            .collect();
        let r = filter_command(&argv(&["bun", "install"]), &bulk);
        assert!(
            !r.filter.starts_with("auto:"),
            "bun should hit npm_dispatch, got {}",
            r.filter
        );
    }

    #[test]
    fn dispatches_biome_to_linter_group() {
        let bulk: String = (0..50).map(|i| format!("line {i}\n")).collect();
        let r = filter_command(&argv(&["biome", "check", "."]), &bulk);
        assert!(!r.filter.starts_with("auto:"), "biome got {}", r.filter);
    }

    #[test]
    fn dispatches_oxlint_to_linter_group() {
        let bulk: String = (0..50).map(|i| format!("line {i}\n")).collect();
        let r = filter_command(&argv(&["oxlint", "check", "."]), &bulk);
        assert!(!r.filter.starts_with("auto:"), "oxlint got {}", r.filter);
    }

    #[test]
    fn dispatches_actionlint_to_linter_group() {
        let bulk: String = (0..50).map(|i| format!("line {i}\n")).collect();
        let r = filter_command(&argv(&["actionlint", "."]), &bulk);
        assert!(
            !r.filter.starts_with("auto:"),
            "actionlint got {}",
            r.filter
        );
    }

    #[test]
    fn cargo_test_dispatcher_default_does_not_recurse() {
        // Regression for #407 — same class as #385 (linter).
        // `cargo test` with no flag previously routed through
        // `cargo_test_default` → `dispatch(&[], output)` → default
        // → `cargo_test_default` → infinite recursion. If this test
        // deadlocks / stack overflows, the fix regressed.
        let bulk: String = (0..50).map(|i| format!("test foo_{i} ... ok\n")).collect();
        let r = filter_command(&argv(&["cargo", "test"]), &bulk);
        assert!(!r.filter.is_empty(), "must return a non-empty filter label");
        // Direct dispatcher call with empty argv — the exact path
        // that used to recurse.
        let _ = crate::cmd::linux::cargo::test::dispatch(&[], &bulk);
    }

    #[test]
    fn linter_dispatcher_default_does_not_recurse() {
        // Regression for the pre-existing infinite-recursion bug in
        // cmd/linux/linter_dispatcher/mod.rs default arm. Empty argv
        // hits the default arm — the previous impl called back
        // through `filter::linter` → `dispatch(&[], ...)` forever.
        // If this test deadlocks / stack overflows, the fix regressed.
        let bulk: String = (0..50)
            .map(|i| format!("/path/file{i}.ts:1:1 some-rule message\n"))
            .collect();
        // With a real argv (no flags) → dispatch hits the default arm
        // via filter_command's match.
        let r = filter_command(&argv(&["eslint", "src/"]), &bulk);
        assert!(!r.filter.is_empty(), "must return a non-empty filter label");
        // Direct dispatcher call with empty argv — the exact path
        // that used to recurse.
        let _ = crate::cmd::linux::linter_dispatcher::dispatch(&[], &bulk);
    }

    #[test]
    fn dispatches_cspell_to_linter_group() {
        let bulk: String = (0..50).map(|i| format!("line {i}\n")).collect();
        let r = filter_command(&argv(&["cspell", "."]), &bulk);
        assert!(!r.filter.starts_with("auto:"), "cspell got {}", r.filter);
    }

    #[test]
    fn strip_leading_comment_lines_works() {
        assert_eq!(
            super::strip_leading_comment_lines("grep foo bar"),
            "grep foo bar"
        );
        assert_eq!(
            super::strip_leading_comment_lines("# header\ngrep foo bar"),
            "grep foo bar"
        );
        assert_eq!(
            super::strip_leading_comment_lines("# h1\n# h2\ngrep x"),
            "grep x"
        );
        assert_eq!(
            super::strip_leading_comment_lines("  # indented\ngrep x"),
            "grep x"
        );
        // Empty / all-comment input.
        assert_eq!(super::strip_leading_comment_lines("# only comment"), "");
        assert_eq!(super::strip_leading_comment_lines(""), "");
        // Mid-command # is NOT a comment line — left untouched.
        assert_eq!(
            super::strip_leading_comment_lines("grep '#' file"),
            "grep '#' file"
        );
    }

    #[test]
    fn auto_compactor_pathological_reduction_is_rejected() {
        // Synth: ~500 tokens of plain text that contains only the
        // single word "npm" buried in lots of other content. The
        // pkginstall detector picks it up as Kind::Pkginstall on the
        // keyword, the compactor emits its empty summary (~10-20
        // tokens), and pre-fix that 95% reduction was emitted. The
        // new guard MUST reject it.
        let mut input = String::with_capacity(2000);
        for i in 0..120 {
            let _ = writeln!(
                input,
                "INFO line {i}: some random log line about npm packages"
            );
        }
        // The output should NOT be tiny — either the generic deduper
        // kicks in (reasonable size) or passthrough preserves the
        // input. Either way, > 30 tokens (the guard's MAX_OUTPUT).
        let r = auto_or_generic(&input, tokenizer::estimate(&input));
        assert!(
            r.savings.tokens_after > 30,
            "pathological reduction not caught: filter={} before={} after={}",
            r.filter,
            r.savings.tokens_before,
            r.savings.tokens_after
        );
    }

    #[test]
    fn a_covered_subcommand_is_never_reported_as_a_gap() {
        // Observed in the field: the "write a compactor" advice fired on
        // `git`, one of the most heavily covered programs in the product,
        // because the run had routed generically. The route is what matters,
        // not the label the run happened to end up with.
        assert_eq!(
            uncovered_route_name(&argv(&["git", "status", "--short"])),
            None
        );
        assert_eq!(
            uncovered_route_name(&argv(&["git", "diff", "--stat"])),
            None
        );
        // A genuine gap in a covered FAMILY is named at subcommand
        // granularity — "git" alone would send someone to write a compactor
        // for eighteen routes that already exist.
        assert_eq!(
            uncovered_route_name(&argv(&["git", "bisect", "start"])).as_deref(),
            Some("git bisect")
        );
        // A program with no subcommand grammar is still named as itself.
        // `some_unknown_prog_xyz` was the fixture until a name also had to
        // RESOLVE, so an invented one answers `None` for the new reason
        // and this check would pass without proving anything.
        let real_gap = if cfg!(windows) { "cmd" } else { "sh" };
        assert_eq!(
            uncovered_route_name(&argv(&[real_gap])).as_deref(),
            Some(real_gap)
        );
    }

    /// A shell-state token can never have a compactor, so naming it is
    /// advice for work nobody can do. Observed on this machine: `cd.nudged`
    /// had fired and the agent was told "'cd' has run 26× recently with only
    /// generic compaction. A command-specific compactor would cut its output".
    /// `cd` has no output to cut.
    #[test]
    fn a_shell_state_token_is_never_named_as_a_gap() {
        for t in ["cd", "export", "set", "continue", "true", ":"] {
            assert_eq!(
                uncovered_route_name(&argv(&[t])),
                None,
                "{t} was named as a coverage gap"
            );
        }
    }

    /// A wrapper's flag VALUE is not the wrapped program — and when the
    /// wrapper is `sshpass`, that value is a password. This name keys
    /// compactor dispatch, becomes a ledger field, is written as a FILENAME
    /// under `~/.tokenade/cmdstats/`, and is quoted back to the agent.
    #[test]
    fn a_wrapper_flag_value_is_never_the_program() {
        let a = argv(&["sshpass", "-p", "Xq7pTr2w", "ssh", "-T", "host", "uptime"]);
        assert_eq!(program_basename(&a), "ssh");
        assert_eq!(uncovered_route_name(&a), None);
        // `frobnicate` still proves the ATTRIBUTION; the nudge half needs a
        // program that exists, because the name is also required to resolve.
        let b = argv(&["sudo", "-u", "root", "frobnicate", "--all"]);
        assert_eq!(program_basename(&b), "frobnicate");
        let real_gap = if cfg!(windows) { "cmd" } else { "sh" };
        let c = argv(&["sudo", "-u", "root", real_gap, "-c", "true"]);
        assert_eq!(program_basename(&c), real_gap);
        assert_eq!(uncovered_route_name(&c).as_deref(), Some(real_gap));
    }

    /// The bundled and `=`-joined forms are ONE token, so the flag rule
    /// already swallows them whole; skipping a value after them would eat the
    /// program instead.
    #[test]
    fn a_joined_flag_value_does_not_eat_the_program() {
        assert_eq!(
            program_basename(&argv(&["sshpass", "-pXq7pTr2w", "ssh", "x"])),
            "ssh"
        );
        assert_eq!(
            program_basename(&argv(&["sudo", "--user=root", "frobnicate"])),
            "frobnicate"
        );
    }

    /// The earlier two fixes of this same class, kept: a wrapper's own flag
    /// and its bare numeric argument are still skipped.
    #[test]
    fn a_wrapper_number_and_flag_are_still_skipped() {
        assert_eq!(
            program_basename(&argv(&["timeout", "-k", "5", "120", "cargo", "test"])),
            "cargo"
        );
        assert_eq!(
            program_basename(&argv(&["nice", "-n", "10", "cargo", "test"])),
            "cargo"
        );
    }

    /// Routing and the label read the same resolver, which is why they now
    /// share one. `sshpass -p hunter2 ls -la` must reach the `ls` compactor.
    #[test]
    fn a_wrapped_command_still_routes_to_its_own_compactor() {
        let out = "total 8\ndrwxr-xr-x 2 me me 4096 Jan  1 00:00 .\n-rw-r--r-- 1 me me   12 Jan  1 00:00 a.txt\n";
        let plain = filter_command(&argv(&["ls", "-la"]), out);
        let wrapped = filter_command(&argv(&["sshpass", "-p", "Xq7pTr2w", "ls", "-la"]), out);
        assert_eq!(
            wrapped.filter, plain.filter,
            "the wrapper changed the route"
        );
        assert_eq!(wrapped.output, plain.output);
    }

    /// A token that cannot be a program must never be named as one.
    #[test]
    fn shell_syntax_is_not_a_missing_compactor() {
        for c in [
            vec!["{", "echo", "a;", "}"],
            vec!["done;", "echo", "end"],
            vec!["-t", "4"],
            vec!["$B", "run"],
            vec!["clean(){", "rm", "-rf", "x;"],
            vec!["n=$(grep", "-c", "x", "f)"],
        ] {
            assert_eq!(uncovered_route_name(&argv(&c)), None, "named {c:?}");
        }
    }

    /// The SUBCOMMAND half of the name goes through the same gate: an
    /// unsplittable chain leaves `git stash;` in it, semicolon and all, and
    /// that string becomes a filename.
    #[test]
    fn a_subcommand_carrying_shell_punctuation_is_not_named() {
        let a = argv(&["git", "stash;", "echo", "done"]);
        assert_ne!(uncovered_route_name(&a).as_deref(), Some("git stash;"));
    }

    /// And the route that reaches it. `meaningful_segment` skips a leading
    /// state-modifier only when the chain SPLITS; a heredoc makes
    /// `chain_shape` answer `Unparsed`, so the whole string comes back with
    /// the `cd` still on the front. That is the exact shape of the 26 runs.
    ///
    /// `first_program_token` now steps over that `cd` itself, which is the
    /// half `meaningful_segment` could not reach — MEASURED 2026-09-05, 22 852
    /// commands on this machine had their program read as `cd`, and the real
    /// one (`grep` 3 938, `python3` 3 978, `cargo` 780, `sqlite3` 758) never
    /// got its own compactor. The premise below therefore reads `python3`
    /// where it used to read `cd`; what must not change is the CONCLUSION,
    /// which is what this test is for: a chain like this reports no coverage
    /// gap named after its scene-setting.
    #[test]
    fn an_unparsable_chain_does_not_name_its_leading_cd() {
        let cmd = "cd /repo && python3 - <<'PY'\nprint(1)\nPY";
        let seg = crate::command_rewrite::meaningful_segment(cmd);
        assert_eq!(
            crate::command_rewrite::first_program_token(seg),
            "python3",
            "the leading `cd` is scene-setting and must not be named"
        );
        let a: Vec<String> = seg.split_whitespace().map(String::from).collect();
        assert_ne!(
            uncovered_route_name(&a).as_deref(),
            Some("cd"),
            "the chain was still credited to its `cd`"
        );
        assert_eq!(uncovered_route_name(&a), None);
    }

    /// A silent setup command before `&&` must not take the chain's name.
    ///
    /// `cd dir && <cmd>` was handled from the start; the other primitives were
    /// not, and the field counters proved it — `echo`, `sleep`, `seq`, `which`
    /// sat in `~/.tokenade/cmdstats` as programs with a missing compactor,
    /// which is only possible if they were credited with someone else's bytes.
    #[test]
    fn a_silent_prefix_never_takes_the_name_of_the_chain() {
        for prefix in [
            vec!["cd", "."],
            vec!["echo", "start"],
            vec!["true"],
            vec!["sleep", "0"],
            vec!["mkdir", "-p", "out"],
            vec!["export", "FOO=1"],
        ] {
            let mut cmd = prefix.clone();
            cmd.push("&&");
            cmd.extend_from_slice(&["git", "status"]);
            assert_eq!(
                program_basename(&argv(&cmd)),
                "git",
                "{prefix:?} && git status was credited to the prefix"
            );
            assert_eq!(
                uncovered_route_name(&argv(&cmd)),
                None,
                "{prefix:?} && git status reported as a coverage gap"
            );
        }
        // A primitive run on its own still owns its output — there is no
        // connector, so there is no other command to attribute it to.
        assert_eq!(program_basename(&argv(&["echo", "hello"])), "echo");
        assert_eq!(program_basename(&argv(&["cd", "/tmp"])), "cd");
    }

    /// The same defect one level coarser, and far more common in the field.
    ///
    /// `knows_subcommand` answers `false` for every program that has no
    /// subcommand grammar — `cat`, `ls`, `grep`, `sed`, `python` — and says so
    /// in its own doc, deferring to `knows_program`. Nothing asked
    /// `knows_program`, so all of them were reported as gaps and the nudge
    /// told users to write compactors that ship in the binary. Counters in
    /// `~/.tokenade/cmdstats` on 2026-08-04 showed `cat.nudged`, `ls.nudged`,
    /// `grep.nudged`, `sed.nudged`, `python.nudged` all fired.
    #[test]
    fn a_covered_program_without_subcommands_is_never_reported_as_a_gap() {
        for cmd in [
            vec!["cat", "PATCHNOTES.md"],
            vec!["ls", "-la", "src"],
            vec!["grep", "-rn", "fn", "src"],
            vec!["sed", "-n", "1,50p", "x.rs"],
            vec!["python3", "script.py"],
            vec!["head", "-100", "x.log"],
        ] {
            let prog = program_basename(&argv(&cmd));
            assert!(
                knows_program(&prog),
                "test premise broken: {prog} is no longer a covered program, \
                 so this case proves nothing"
            );
            assert_eq!(
                uncovered_route_name(&argv(&cmd)),
                None,
                "{cmd:?} routes to a compactor that ships in the binary — \
                 naming it a gap sends the user to write one that exists"
            );
        }
        // The nudge must still find real gaps, or the fix has silenced it.
        //
        // The fixture used to be `some_unknown_prog_xyz`, which is now retired:
        // a name is now also required to RESOLVE, so an invented one answers
        // `None` for the new reason and the anti-vacuity check passed by
        // accident of shape. A real program tokenade does not compact is the
        // only thing that proves the nudge still fires.
        let real_gap = if cfg!(windows) { "cmd" } else { "sh" };
        assert!(
            !knows_program(real_gap),
            "test premise broken: {real_gap} is compacted now, pick another gap"
        );
        assert_eq!(
            uncovered_route_name(&argv(&[real_gap, "-v"])).as_deref(),
            Some(real_gap)
        );
    }

    /// The nudge may only name something a compactor could be written
    /// FOR: not a label, and not a name the shell could never run.
    ///
    /// Two guards, two different field failures, both measured in
    /// `~/.tokenade/cmdstats` on this machine 2026-09-05.
    ///
    /// `echo.count` was accumulating: an unparsable chain (`echo "=== x ===";
    /// cat big.txt`, a heredoc, a `for` body) leaves `echo` in the program
    /// position, and `echo` prints back exactly what the caller typed. The
    /// routing side has skipped this set since `segment_is_a_label`; the nudge
    /// asked the OTHER list, `segment_must_not_be_wrapped`, which is about
    /// subshell safety and contains neither `echo` nor `printf` nor `sleep`.
    ///
    /// `SI.count` 5 with `SI.nudged` ALREADY WRITTEN — the user was told to
    /// write a compactor for `SI` — plus `file_searchrs` (the source file
    /// `file_search.rs` in program position). Both clear every shape guard;
    /// neither is a program on this machine.
    #[test]
    fn a_nudge_names_only_something_that_could_have_a_compactor() {
        for label in [
            vec!["echo", "=== section ==="],
            vec!["printf", "%s\\n", "x"],
            vec!["sleep", "2"],
        ] {
            assert_eq!(
                uncovered_route_name(&argv(&label)),
                None,
                "{label:?} was named as a coverage gap — its output is the \
                 caller's own text, so no compactor can cut it"
            );
        }
        for unrunnable in ["SI", "file_search.rs", "some_unknown_prog_xyz"] {
            assert!(
                !crate::availability::is_available(unrunnable),
                "test premise broken: {unrunnable} resolves here, so this case \
                 proves nothing about unrunnable names"
            );
            assert_eq!(
                uncovered_route_name(&argv(&[unrunnable, "-v"])),
                None,
                "{unrunnable} was named as a coverage gap — nothing can run it"
            );
        }
        // A tool that lives OUTSIDE `$PATH` is spelled by its path, and that
        // spelling is the only one it has. Resolving the basename would lose
        // it, so the written token is what gets checked.
        let dir = std::env::temp_dir().join(format!("tokenade-nudge-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tool = dir.join("frobnicate");
        std::fs::write(&tool, "#!/bin/sh\n").unwrap();
        let written = tool.to_string_lossy().to_string();
        assert!(
            !crate::availability::is_available("frobnicate"),
            "test premise broken: frobnicate is on PATH here"
        );
        assert_eq!(
            uncovered_route_name(&argv(&[&written, "--all"])).as_deref(),
            Some("frobnicate"),
            "a tool spelled by its path lost its nudge to the resolver"
        );
        let _ = std::fs::remove_dir_all(&dir);

        // Anti-vacuity, twice over: a real, resolvable program tokenade does
        // not compact must STILL be named, and it must be named on the same
        // code path (`is_plausible_program_name` accepts all four strings
        // above, so the guards under test are the only difference).
        let real_gap = if cfg!(windows) { "cmd" } else { "sh" };
        assert!(crate::availability::is_available(real_gap));
        assert!(crate::command_rewrite::is_plausible_program_name("SI"));
        assert_eq!(
            uncovered_route_name(&argv(&[real_gap, "-c", "true"])).as_deref(),
            Some(real_gap),
            "the guards silenced the nudge entirely"
        );
    }

    #[test]
    fn a_repetitive_payload_beats_its_format_summary_but_only_when_it_earns_it() {
        // Uniform NDJSON: the format summary counts the levels, the template
        // fold shows the shape AND the count for a fraction of the tokens.
        let uniform: String = (0..3_000)
            .map(|i| format!("{{\"lvl\":\"info\",\"id\":{i},\"msg\":\"handled request\"}}\n"))
            .collect();
        let r = auto_or_generic(&uniform, tokenizer::estimate(&uniform));
        assert!(
            r.savings.tokens_after < 60,
            "uniform payload did not fold: filter={} after={}",
            r.filter,
            r.savings.tokens_after
        );

        // Same format, one level scattered through it: now the rows really do
        // differ, the fold is the WORSE answer, and the summary must keep the
        // round. A one-directional rule is the whole safety property here.
        let mixed: String = (0..3_000)
            .map(|i| {
                if i % 50 == 7 {
                    format!("{{\"lvl\":\"error\",\"id\":{i},\"msg\":\"upstream timeout\"}}\n")
                } else {
                    format!("{{\"lvl\":\"info\",\"id\":{i},\"msg\":\"handled request\"}}\n")
                }
            })
            .collect();
        let r = auto_or_generic(&mixed, tokenizer::estimate(&mixed));
        assert!(
            !r.filter.ends_with("+fold"),
            "the fold took a round it loses: filter={} after={}",
            r.filter,
            r.savings.tokens_after
        );
    }

    #[test]
    fn a_log_that_repeats_itself_is_allowed_to_fold_all_the_way() {
        // 2 000 lines of ONE shape. Every line is textually unique (the
        // counter), so no line-level dedup explains the reduction — only the
        // template does, and folding it to one line + a count is the correct
        // answer. Pre-fix the misclassification guard called that 99 %
        // pathological and fell back to a head slice: 3 768 tokens where 30
        // were right, and the head slice does not even say how many lines it
        // dropped.
        let input: String = (0..2_000)
            .map(|i| format!("2026-08-02 INFO worker={i} status=ok path=/var/run/app/{i}\n"))
            .collect();
        let r = auto_or_generic(&input, tokenizer::estimate(&input));
        assert!(
            r.savings.tokens_after <= 60,
            "the fold was rejected: filter={} after={}",
            r.filter,
            r.savings.tokens_after
        );
        assert!(
            r.output.contains("2000"),
            "a fold has to say how much it folded:\n{}",
            r.output
        );
    }

    #[test]
    fn anti_inflation_guard_passthrough_on_growth() {
        // Synthesise a case where every compactor would add a banner
        // longer than the input. The `generic` filter is the second
        // fallback; if both inflate, we MUST passthrough.
        let tiny = "ok\n";
        let argv_ = argv(&["unknown_command_xyz"]);
        let r = filter_command(&argv_, tiny);
        // Either the picked path doesn't inflate (good) or we get
        // passthrough (also good). What must NOT happen: r.output
        // longer than `tiny`.
        assert!(
            r.savings.tokens_after <= r.savings.tokens_before,
            "compactor inflated tiny input: before={} after={}",
            r.savings.tokens_before,
            r.savings.tokens_after
        );
    }

    // --- #502: cat <source.rs> code_strip dispatch ---

    #[test]
    fn cat_dispatch_strips_comments_from_source_file() {
        // Heavily-commented Rust source big enough that the SAFETY NET 3
        // (after >= before) doesn't undo the compactor. ASCII-only to
        // dodge stray UTF-8 mangling in test diagnostic output.
        // Over the delivery threshold: a source read that fits keeps its
        // comments.
        let mut rust_src = String::new();
        for _ in 0..200 {
            rust_src.push_str(
                "// top-level comment that should disappear after strip\n\
                 pub fn add(a: i32, b: i32) -> i32 {\n\
                     // inline comment that should disappear\n\
                     a + b // trailing comment\n\
                 }\n\
                 /// doc-comment PRESERVED (Comments mode keeps triple-slash)\n\
                 pub fn mul(a: i32, b: i32) -> i32 { a * b }\n\n",
            );
        }
        let argv = argv(&["cat", "src/foo.rs"]);
        let r = filter_command(&argv, &rust_src);
        assert!(!r.output.contains("top-level comment"), "// comment leaked");
        assert!(
            !r.output.contains("inline comment"),
            "inline // comment leaked"
        );
        assert!(
            !r.output.contains("trailing comment"),
            "trailing // comment leaked"
        );
        assert!(
            r.output.contains("doc-comment PRESERVED"),
            "/// doc-comment was dropped (Comments mode must preserve ///)"
        );
        assert!(
            r.savings.tokens_after < r.savings.tokens_before,
            "no saving: before={} after={}",
            r.savings.tokens_before,
            r.savings.tokens_after
        );
        assert_eq!(r.filter, "cat", "filter label should be 'cat'");
    }

    #[test]
    fn cat_dispatch_non_source_extension_does_not_panic_or_inflate() {
        // `cat README.md` — no code_strip language. The dispatcher
        // falls back to generic dedup. We don't care about the exact
        // compaction shape on tiny input (SAFETY NET 3 may passthrough)
        // but the call must never inflate or panic.
        let text = "Hello world\n\n\nLine after blanks.\n";
        let argv = argv(&["cat", "README.md"]);
        let r = filter_command(&argv, text);
        assert!(
            r.savings.tokens_after <= r.savings.tokens_before,
            "non-source cat inflated input: before={} after={}",
            r.savings.tokens_before,
            r.savings.tokens_after
        );
    }

    #[test]
    fn cat_dispatch_passthrough_for_source_extension_unknown_to_code_strip() {
        // #590 — `cat install.ps1` (PowerShell) — code_strip doesn't have
        // a .ps1 language so cat_try_code_strip returns None. Pre-fix:
        // fell through to auto_or_generic, where the cmake detector
        // matched PS keywords (if/else/function/return/set) at 33%
        // efficiency. Post-fix: cat_argv_has_source_extension catches
        // the .ps1 and routes to raw passthrough (ANSI strip only).
        // Build a PS fixture big enough to clear safety nets.
        let mut ps = String::new();
        for _ in 0..15 {
            ps.push_str(
                "if ($Target -eq \"windows\") {\n  \
                     Write-Host \"Installing\"\n  \
                     $cache = \"$env:LOCALAPPDATA\\tokenade\"\n\
                 } else {\n  \
                     $cache = \"$env:HOME/.tokenade\"\n\
                 }\n\n\
                 function Install-Binary {\n  \
                     param([string]$Url)\n  \
                     return $true\n\
                 }\n\n",
            );
        }
        let argv = argv(&["cat", "install.ps1"]);
        let r = filter_command(&argv, &ps);
        // Filter label should be "cat" (the source-extension branch),
        // not "auto:cmake" / "auto:ini" / "auto:env" / etc.
        assert!(
            !r.filter.starts_with("auto:"),
            ".ps1 must not route to auto detector; got filter={}",
            r.filter
        );
        // Content survives substantially intact (only ANSI stripped).
        assert!(
            r.output.contains("Install-Binary") || r.output.contains("Write-Host"),
            "PowerShell content must survive; got: {}",
            &r.output[..r.output.len().min(200)]
        );
    }

    #[test]
    fn cat_dispatch_text_data_still_routes_to_auto() {
        // Regression guard: .json / .yaml / .log files (NOT in
        // is_source_code_extension) must still go through
        // auto_or_generic so their format-specific compactors fire.
        // We use a JSON fixture big enough to make auto compact win.
        let json = format!(
            "{{\"items\":[{}]}}\n",
            (0..50)
                .map(|i| format!("{{\"id\":{i}}}"))
                .collect::<Vec<_>>()
                .join(",")
        );
        let argv = argv(&["cat", "data.json"]);
        let r = filter_command(&argv, &json);
        // The JSON compactor elides array tails → filter label is auto:json.
        assert!(
            r.filter.starts_with("auto:") || r.filter == "cat",
            "JSON cat should route through auto detector path; got filter={}",
            r.filter
        );
    }

    #[test]
    fn cat_dispatch_picks_first_source_file_when_argv_has_flags() {
        // `cat -A src/foo.py` — `-A` is a flag, should be skipped.
        // Need enough volume that SAFETY NET 3 doesn't passthrough.
        let mut py_src = String::new();
        for _ in 0..600 {
            py_src.push_str(
                "# leading comment\n\
                 def foo():\n\
                     # body comment\n\
                     return 42\n\n",
            );
        }
        let argv = argv(&["cat", "-A", "src/foo.py"]);
        let r = filter_command(&argv, &py_src);
        assert!(!r.output.contains("leading comment"), "# comment leaked");
        assert!(!r.output.contains("body comment"), "body # comment leaked");
    }

    #[test]
    fn get_content_routes_like_cat_and_select_string_like_grep() {
        // Get-Content on a source file → code-strip route (same as cat),
        // case-insensitively, including the gc/type aliases.
        let mut code = String::new();
        for i in 0..300 {
            code.push_str(&format!(
                "// noise comment {i}\nfn f_{i}() {{ let x = {i}; }}\n"
            ));
        }
        for prog in ["Get-Content", "get-content", "gc", "type"] {
            let av = argv(&[prog, "src\\main.rs"]);
            let r = filter_command(&av, &code);
            assert!(
                r.savings.tokens_after < r.savings.tokens_before,
                "{prog} must compact a source dump"
            );
        }
        // Select-String output folds through the grep dispatcher.
        let mut hits = String::new();
        for i in 0..200 {
            hits.push_str(&format!("src/big.rs:{i}:match number {i} here in code\n"));
        }
        let av = argv(&["Select-String", "match", "src/big.rs"]);
        let r = filter_command(&av, &hits);
        assert!(r.savings.tokens_after < r.savings.tokens_before);
    }

    #[test]
    fn docker_kubectl_exec_route_to_inner_command() {
        // docker exec <ctr> journalctl → the journal compactor, not generic.
        let mut journal = String::new();
        for i in 0..300 {
            journal.push_str(&format!(
                "Jun 30 12:{:02}:01 host systemd[1]: Started Session {i} of user root.\n",
                i % 60
            ));
        }
        for av in [
            argv(&[
                "docker",
                "exec",
                "-u",
                "root",
                "mycontainer",
                "journalctl",
                "-u",
                "nginx",
            ]),
            argv(&[
                "docker",
                "compose",
                "exec",
                "web",
                "journalctl",
                "-u",
                "nginx",
            ]),
            argv(&[
                "kubectl",
                "exec",
                "mypod",
                "--",
                "journalctl",
                "-u",
                "nginx",
            ]),
        ] {
            let r = filter_command(&av, &journal);
            assert!(
                r.savings.tokens_after < r.savings.tokens_before,
                "{:?} must compact inner journalctl output",
                av.first()
            );
        }
        // No inner command → generic path, no panic.
        let r = filter_command(&argv(&["docker", "exec", "mycontainer"]), "ok\n");
        assert!(r.savings.tokens_after <= r.savings.tokens_before);
    }

    #[test]
    fn filter_command_calls_dispatch_named_for_unknown_programs() {
        // Programs with no explicit arm in filter_command's match reach the
        // `_ =>` arm and must route through `dispatch_named` before falling
        // back to `auto_or_generic`. When dispatch_named compacts successfully
        // AND the result is smaller (inflation guard passes), the filter label
        // is set to the program name — not "auto", "generic", or "passthrough".
        //
        // `who` with 50 sessions: the who compactor folds them with a
        // "…more session" summary, producing output smaller than the input
        // and satisfying the inflation guard.
        let mut who_out = String::new();
        for i in 0..50 {
            let _ = writeln!(who_out, "user_{i} pts/{i} 2026-05-29 09:00 (:0)");
        }
        let r = filter_command(&argv(&["who"]), &who_out);
        assert_eq!(
            r.filter, "who",
            "who must route via dispatch_named in the _ => arm; \
             got {:?} — if dispatch_named was removed, the label would be \
             'auto'/'generic'/'passthrough'",
            r.filter,
        );
    }

    #[test]
    fn tee_is_content_transparent_not_formaware() {
        // tee copies its stdin verbatim — its output IS the content flowing
        // through it. A repetitive blob must be compacted by the content's
        // own shape (generic dedup), never by a tee-specific form-aware stub.
        let mut blob = String::new();
        for _ in 0..60 {
            blob.push_str("identical repeated line of noise\n");
        }
        let r = filter_command(&argv(&["tee", "/tmp/tk_out.txt"]), &blob);
        assert_ne!(r.filter, "tee", "the tee form-aware stub must be gone");
        assert!(r.output.len() <= blob.len(), "must not inflate");
    }

    #[test]
    fn command_builtin_routes_to_wrapped_program() {
        // `command ls -l` runs ls bypassing functions/aliases — its output is
        // ls's, so it must route as the wrapped command, not as `command`.
        let mut body = String::new();
        for i in 0..40 {
            let _ = writeln!(body, "-rw-r--r-- 1 u g {i} Jan  1 12:00 file_{i}.rs");
        }
        let r = filter_command(&argv(&["command", "ls", "-l"]), &body);
        assert_ne!(
            r.filter, "command",
            "wrapper must unwrap to the inner command"
        );
    }

    // --- truncate_smart / truncate_middle: headroom-derived folding ---

    #[test]
    fn truncate_smart_short_input_is_identity() {
        let s = "a\nb\nc";
        assert_eq!(truncate_smart(s, 10), s);
    }

    #[test]
    fn truncate_smart_preserves_buried_error() {
        let mut lines: Vec<String> = (0..400).map(|i| format!("step {i} ok")).collect();
        lines[200] = "ERROR: connection refused".to_string();
        let input = lines.join("\n");
        let out = truncate_smart(&input, 50);
        assert!(
            out.contains("ERROR: connection refused"),
            "buried error dropped by truncate_smart"
        );
        assert!(out.contains("elided by tokenade"));
        // Strict budget: kept lines + banners stay bounded.
        assert!(out.lines().count() <= 50 + 50);
    }

    /// A truncated `cat` says how much it dropped and names a command that runs.
    ///
    /// It used to quote two constants ("showing first 150/last 50 lines") of a
    /// total it never gave — so the agent could not tell whether 3 lines or
    /// 3 000 were gone, nor choose the `offset` for the Read it was told to do
    /// — and the head is adaptive, so the constants were not what happened
    /// either. Then it advertised `skeleton path="<file>"`, which the CLI
    /// answers `No such file or directory`: that argument shape is the MCP
    /// tool's, not the command's.
    ///
    /// MEASURED 2026-09-05 over this machine's transcripts: 23 of these
    /// delivered at a 2.9 KB median, and not ONE was ever asked a question,
    /// while the agent pulled the bytes back 6 times and bypassed 3.
    #[test]
    fn a_truncated_cat_states_what_it_dropped_and_names_a_runnable_command() {
        let content: String = (0..600)
            .map(|i| format!("line {i}: aliquam erat volutpat sed do eiusmod tempor\n"))
            .collect();
        let argv: Vec<String> = ["cat", "src/big.rs"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let out = cat_apply_large_truncation(&argv, &content, 3_000).expect("must truncate");
        let banner = out.lines().next().unwrap();

        assert!(banner.contains("600 lines"), "no total: {banner}");
        let hidden = 600 - out.lines().count() + 1; // +1: the banner line itself
        assert!(
            banner.contains(&format!("{hidden} line(s) elided")),
            "the elided count is wrong or missing (expected {hidden}): {banner}"
        );
        // The spelling has to be the one the CLI takes.
        assert!(
            banner.contains("tokenade skeleton src/big.rs"),
            "the runnable spelling is missing: {banner}"
        );
        assert!(
            !banner.contains("path=\""),
            "the MCP argument shape is back, and the CLI refuses it: {banner}"
        );
        // And the cheapest path the shipped guidance names is offered first.
        assert!(
            banner.contains("--prompt"),
            "the banner never suggests asking a question: {banner}"
        );
        let ask = banner.find("--prompt").unwrap();
        let read = banner.find("`Read` with offset").unwrap();
        assert!(
            ask < read,
            "the expensive option is offered first: {banner}"
        );

        // A file below the bar is untouched, banner and all.
        assert!(cat_apply_large_truncation(&argv, &content, 100).is_none());
    }

    #[test]
    fn elision_emits_actionable_recovery_footer() {
        // #1 — any folded output must tell the agent the bytes are
        // recoverable, not just that lines were dropped.
        let lines: Vec<String> = (0..400).map(|i| format!("step {i} ok")).collect();
        let out = truncate_smart(&lines.join("\n"), 50);
        assert!(out.contains("elided by tokenade"), "inline marker missing");
        assert!(
            out.contains("expand-ref") && out.contains("tokenade raw"),
            "recovery footer must name expand-ref and the tokenade raw fallback: {out}"
        );
    }

    #[test]
    fn no_elision_no_footer() {
        // Short input passes through untouched — no spurious footer.
        let s = "a\nb\nc";
        let out = truncate_smart(s, 10);
        assert_eq!(out, s);
        assert!(!out.contains("expand-ref"));
    }

    // --- knows_program / known_program_names: the derived registry the
    // --- shell proxy is built from (cmd modules + named arms + filter arms)

    #[test]
    fn knows_program_covers_all_three_sources() {
        // auto-dispatch (a cmd/ module file per name)
        assert!(knows_program("dmesg"));
        // NAMED_EXTRA_PROGS (explicit dispatch_named arms)
        assert!(knows_program("ping"));
        assert!(knows_program("printenv"));
        // FILTER_ARM_PROGS (match arms scanned out of filter.rs by build.rs)
        assert!(knows_program("git"));
        assert!(knows_program("cargo"));
        assert!(knows_program("kubectl"));
        // and definitely-not-ours names stay unknown
        assert!(!knows_program("frobnicator-9000"));
        assert!(!knows_program(""));
    }

    #[test]
    fn known_program_names_sorted_unique_nonempty() {
        let names = known_program_names();
        assert!(
            names.len() > 100,
            "registry suspiciously small: {} names",
            names.len()
        );
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(names, sorted, "must be sorted + deduped");
        // Every listed name must round-trip through the predicate.
        for n in &names {
            assert!(knows_program(n), "{n} listed but not known");
        }
    }

    #[test]
    fn fold_separator_rules_collapses_full_width_rules() {
        let inp = "FAIL: test_x\n========================================\n\
                   AssertionError: 1 != 2\n----------------------------------------\nok\n";
        let out = fold_separator_rules(inp);
        assert_eq!(out, "FAIL: test_x\n===\nAssertionError: 1 != 2\n---\nok\n");
    }

    #[test]
    fn fold_separator_rules_preserves_short_rules_and_mixed_lines() {
        // At/under threshold (5 or fewer chars) → untouched.
        let short = "=====\n";
        assert_eq!(fold_separator_rules(short), short);
        // 6 chars is the first folded width.
        assert_eq!(fold_separator_rules("======\n"), "===\n");
        // A line with content + dashes (e.g. a dash-table `--- ---`) → untouched.
        let table = "col1 col2\n---- ----\na    b\n";
        assert_eq!(fold_separator_rules(table), table);
        // Code with a long `---` divider inside text is still a pure rule → folded,
        // but a `# heading ----` comment keeps its content (not a pure run).
        let comment = "# section --------------------\n";
        assert_eq!(fold_separator_rules(comment), comment);
    }

    #[test]
    fn fold_separator_rules_is_crlf_safe_and_keeps_indent() {
        let inp = "a\r\n    ====================\r\nb\r\n";
        let out = fold_separator_rules(inp);
        assert_eq!(out, "a\r\n    ===\r\nb\r\n");
        // No trailing newline preserved.
        assert_eq!(fold_separator_rules("===================="), "===");
    }

    #[test]
    fn fold_separator_rules_idempotent() {
        let inp = "x\n********************************\ny\n";
        let once = fold_separator_rules(inp);
        assert_eq!(fold_separator_rules(&once), once);
    }

    /// Reads the process CWD twice — once here, once inside
    /// `relativize_cwd_paths` — and asserts the two agree, so it must hold
    /// `ENV_LOCK` like every other reader of process-global state. It did
    /// not, and the `Layers` helper moves the CWD under that same lock:
    /// one run in roughly eight failed with the two reads straddling the
    /// move. A lock only protects the tests that take it.
    #[test]
    fn relativize_cwd_paths_strips_workspace_prefix() {
        let _g = crate::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let cwd = std::env::current_dir().unwrap();
        let cwd = cwd.to_string_lossy();
        // Only meaningful when cwd is a real dir (it is, in the test runner).
        if cwd.len() >= 8 && cwd.as_ref() != "/" {
            let inp = format!("File \"{cwd}/tests/test_book.py\", line 22, in test_x");
            let out = relativize_cwd_paths(&inp);
            assert_eq!(out, "File \"tests/test_book.py\", line 22, in test_x");
            // Paths NOT under cwd are left absolute.
            let other = "open /etc/hosts failed";
            assert_eq!(relativize_cwd_paths(other), other);
        }
    }

    fn buildkit_redraw() -> String {
        let mut raw = String::new();
        for tick in 0..30 {
            if tick > 0 {
                raw.push_str("\x1b[4A");
            }
            for j in 1..=4 {
                raw.push_str(&format!(
                    "\x1b[2K#{j} [internal] step {j} of 4 ... {}s\n",
                    tick * 3 + j
                ));
            }
        }
        raw.push_str("#4 DONE 12.3s\n");
        raw
    }

    /// A cursor-up redraw is REPLAYED, not folded. The terminal showed
    /// four lines; the other hundred and sixteen were overwritten before anyone
    /// saw them, so dropping them removes no information and needs no count.
    #[test]
    fn a_cursor_up_redraw_is_replayed_to_what_the_terminal_showed() {
        let out = super::replay_cursor_redraws(&buildkit_redraw());
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines.len(),
            5,
            "four redrawn lines and the DONE line: {out:?}"
        );
        assert_eq!(
            lines[0], "#1 [internal] step 1 of 4 ... 88s",
            "the LAST frame survives"
        );
        assert_eq!(lines[4], "#4 DONE 12.3s");
        assert!(
            !out.contains('\x1b'),
            "the redraw grammar is consumed: {out:?}"
        );
    }

    /// Each half of the reading, with something real refused by each.
    #[test]
    fn each_redraw_condition_refuses_something_real() {
        // No escapes at all: byte-for-byte identical, no allocation of meaning.
        let plain = "one\ntwo\nthree\n";
        assert_eq!(super::replay_cursor_redraws(plain), plain);

        // Escapes but no cursor-up: untouched, because erase-line alone is not
        // a redraw and this function is not an ANSI stripper.
        let coloured = "\x1b[31mred\x1b[0m\nplain\n";
        assert_eq!(super::replay_cursor_redraws(coloured), coloured);

        // `ESC[A` with no digits means one.
        let one_up = "a\nb\n\x1b[Ac\n";
        assert_eq!(super::replay_cursor_redraws(one_up), "a\nc\n");

        // More lines up than exist must not panic and must not go negative.
        let over = "a\n\x1b[9Ab\n";
        assert_eq!(super::replay_cursor_redraws(over), "b\n");

        // MID-line is not a redraw: a full-screen application's output is not
        // a transcript, and honouring it there would delete real lines.
        let midline = "keep me \x1b[4A tail\nsecond\n";
        assert_eq!(super::replay_cursor_redraws(midline), midline);

        // Erase-line ALONE is not a redraw. Removing stray control bytes is
        // `strip_ansi`'s job; this function replays a move or does nothing.
        let erase_only = "\x1b[2Kone\n\x1b[2Ktwo\n";
        assert_eq!(super::replay_cursor_redraws(erase_only), erase_only);

        // A PARTIAL erase (`ESC[1J` above the cursor, `ESC[0J` below) is not
        // "the screen is gone" and must not clear the transcript.
        let partial = "kept\n\x1b[1Jafter\n";
        assert_eq!(super::replay_cursor_redraws(partial), partial);

        // An unknown final byte stops the run and leaves the line alone —
        // colour must not be eaten by the redraw reader. The fixture has to
        // carry a REAL redraw as well, or the rebuild never happens and the
        // assertion passes without reaching the reader at all.
        let colour_first = "a\nb\n\x1b[1A\x1b[31mred\x1b[0m\n";
        assert_eq!(
            super::replay_cursor_redraws(colour_first),
            "a\n\x1b[31mred\x1b[0m\n",
            "the redraw is replayed and the colour is left exactly where it was"
        );
    }

    /// Erasing the whole display says everything above it is gone from
    /// the screen — `watch`, `docker stats`, several test runners. Same
    /// argument as cursor-up, and it was not read either: measured on 25 frames
    /// of `watch kubectl get pods`, 7 738 bytes for what the terminal showed
    /// once.
    #[test]
    fn a_cleared_screen_keeps_only_the_last_frame() {
        let mut raw = String::new();
        for tick in 0..25 {
            // The clear is spelled as a RUN of two sequences, which is why the
            // reader consumes a run rather than one.
            raw.push_str("\x1b[2J\x1b[H");
            raw.push_str(&format!("Every 2.0s: kubectl get pods    tick {tick}\n"));
            for j in 0..6 {
                raw.push_str(&format!("pod-{j}-abc{tick}   1/1   Running   {tick}\n"));
            }
        }
        let out = super::replay_cursor_redraws(&raw);
        assert_eq!(
            out.lines().filter(|l| !l.trim().is_empty()).count(),
            7,
            "one header and six pods — the LAST frame:\n{out}"
        );
        assert!(
            out.contains("tick 24"),
            "the surviving frame is the last: {out}"
        );
        assert!(!out.contains("tick 23"), "no earlier frame survives: {out}");
        assert!(
            !out.contains('\x1b'),
            "the redraw grammar is consumed: {out:?}"
        );
    }

    /// A cursor-up redraw is faithful to the SCREEN, and the screen is
    /// allowed to destroy the one line the agent needed. `cargo`, `npm` and
    /// every progress-bar build print a diagnostic and then redraw a status
    /// block over it; replaying that faithfully hands back a clean progress
    /// frame and no error at all. The repo's rule for an error is written on
    /// `compress::is_error_line` and re-stated in the router fuzzer's fourth
    /// invariant: elision disclosure is not enough, the agent needs the
    /// location. So the pop STOPS at an error rather than skipping it, which
    /// also keeps the order and whatever context sits above it.
    #[test]
    fn a_redraw_never_overwrites_an_error() {
        let raw = concat!(
            "Compiling widget v0.1.0\n",
            "error[E0433]: failed to resolve: use of undeclared crate `serd`\n",
            " --> src/mod.rs:41:5\n",
            "\x1b[2A   Building [====>      ] 12/40\n",
            "   Building [=========> ] 39/40\n",
        );
        let out = super::replay_cursor_redraws(raw);
        assert!(
            out.contains("error[E0433]"),
            "the diagnostic the terminal overwrote survives:\n{out}"
        );
        assert!(
            out.contains("src/mod.rs:41:5"),
            "and so does the location under it, because the pop STOPS \
             rather than skipping — order and context are kept:\n{out}"
        );
        assert!(
            out.contains("39/40"),
            "the frame that did the redrawing is still there:\n{out}"
        );

        // The stop is bounded by the error and by nothing else: with no error
        // in the way, the same two-line pop takes both lines.
        let clean = concat!(
            "Compiling widget v0.1.0\n",
            "   Building [=>         ] 1/40\n",
            "   Building [====>      ] 12/40\n",
            "\x1b[2A   Building [=========> ] 39/40\n",
        );
        let out = super::replay_cursor_redraws(clean);
        assert!(
            !out.contains("1/40") && !out.contains("12/40"),
            "both overwritten frames go when neither is an error:\n{out}"
        );
    }

    /// The post-passes are ONE composition and both doors call it. This
    /// pins the three halves that make that safe: the lossless passes apply
    /// with no command at all, the one command-specific pass does NOT, and the
    /// one LOSSY pass does not either.
    #[test]
    fn the_post_passes_apply_without_a_command_except_the_ones_that_cannot() {
        let raw = buildkit_redraw();
        let folded = super::shrink_only_post_passes(&[], &raw, &raw)
            .expect("the lossless passes must fire with no command at all");
        assert!(
            folded.len() * 20 < raw.len(),
            "the content door replays the redraw: {} vs {}",
            folded.len(),
            raw.len()
        );

        // The command-specific half: `__pycache__` noise is dropped under
        // `find` and must survive under no command, because dropping it from
        // an arbitrary document is not a lossless act — it is a guess about
        // what the bytes are.
        let listing = "src/app.py\nsrc/__pycache__/app.cpython-311.pyc\ntests/test_x.py\n";
        let under_find = super::shrink_only_post_passes(&["find".to_string()], listing, listing);
        assert!(
            under_find
                .as_deref()
                .is_some_and(|o| !o.contains("__pycache__")),
            "under `find` the artifact pass applies: {under_find:?}"
        );
        assert!(
            super::shrink_only_post_passes(&[], listing, listing).is_none(),
            "with no command nothing may be dropped from an arbitrary document"
        );

        // The LOSSY half. A `git log --oneline` is a list of distinct facts;
        // the template fold keeps a shape and a COUNT and drops the hashes,
        // which is the answer the reader asked for. That is what
        // `Unclaimed::Keep` withholds from a document, so it runs for a
        // COMMAND and not for a document.
        let git: String = (0..400)
            .map(|i: usize| {
                format!(
                    "{:08x} commit subject number {i} touching a few files\n",
                    i * 2_654_435_761u64 as usize
                )
            })
            .collect();
        assert!(
            super::shrink_only_post_passes(&[], &git, &git).is_none(),
            "a document must not lose its distinct values to a template"
        );
        // `<`, not `× 2` (amended 2026-10-03): a run whose slots vary
        // together now LISTS them column by column, so the hashes are kept —
        // 13 075 of 23 062 bytes. What this pins is that the fold reaches a
        // COMMAND and not a document, not how much it drops.
        assert!(
            super::shrink_only_post_passes(&["git".to_string(), "log".to_string()], &git, &git)
                .is_some_and(|o| o.len() < git.len()),
            "a COMMAND still gets the fold it always had"
        );
    }

    #[test]
    fn drop_build_artifact_lines_strips_noise_from_find() {
        let inp = "src/app.py\nsrc/__pycache__/app.cpython-311.pyc\n\
                   tests/test_x.py\ntests/__pycache__/test_x.cpython-311.pyc\n\
                   pkg.egg-info/PKG-INFO\n.DS_Store\n.git/HEAD\nREADME.md\n";
        let out = drop_build_artifact_lines(&argv(&["find", ".", "-type", "f"]), inp);
        assert_eq!(out, "src/app.py\ntests/test_x.py\nREADME.md\n");
    }

    #[test]
    fn drop_build_artifact_lines_keeps_lookalikes_and_real_sources() {
        // `.gitignore`/`.github` are NOT the `.git` dir; `.py` is a source file.
        let inp = ".gitignore\n.github/workflows/ci.yml\nsrc/mod.py\n";
        let out = drop_build_artifact_lines(&argv(&["find", "."]), inp);
        assert_eq!(out, inp);
    }

    #[test]
    fn drop_build_artifact_lines_passthrough_when_targeted_or_not_listing() {
        let noise = "a/__pycache__/x.pyc\n";
        // Command explicitly hunts the artifact → leave it alone.
        assert_eq!(
            drop_build_artifact_lines(&argv(&["find", ".", "-name", "*.pyc"]), noise),
            noise
        );
        // Not a listing command → a grep/log line mentioning .pyc is untouched.
        assert_eq!(
            drop_build_artifact_lines(&argv(&["grep", "pyc", "log"]), noise),
            noise
        );
    }

    #[test]
    fn drop_build_artifact_lines_handles_ls_long_and_tree() {
        // `ls -l`: path is the last column.
        let ls = "total 8\n-rw-r--r-- 1 u g 12 Jun 1 12:00 app.py\n\
                  drwxr-xr-x 2 u g  4 Jun 1 12:00 __pycache__\n";
        let out = drop_build_artifact_lines(&argv(&["ls", "-l"]), ls);
        assert_eq!(out, "total 8\n-rw-r--r-- 1 u g 12 Jun 1 12:00 app.py\n");
        // tree drawing prefix before the name.
        let tree = "├── app.py\n└── app.cpython-311.pyc\n";
        let out = drop_build_artifact_lines(&argv(&["tree"]), tree);
        assert_eq!(out, "├── app.py\n");
    }
    /// Adversarial sweep over the compactor dispatch: 45 programs x 8
    /// subcommands x 16 hostile texts (multi-byte runs sized to land on every
    /// slice offset the code uses, 5 000-char single lines, NUL bytes, CRLF,
    /// all-whitespace, empty). MEASURED 2026-08-04: 5 888 combinations, zero
    /// panics.
    ///
    /// `#[ignore]`d because it takes ~12 s — run it with
    /// `cargo test -p tokenade-core --lib no_seed_input_panics -- --ignored`
    /// after touching index arithmetic in any compactor.
    #[test]
    #[ignore = "balayage adverse : 45 programmes x 8 sous-commandes x 16 textes hostiles"]
    fn no_seed_input_panics_any_compactor() {
        let progs = [
            "git",
            "cargo",
            "npm",
            "pnpm",
            "kubectl",
            "docker",
            "ls",
            "grep",
            "find",
            "ps",
            "yarn",
            "terraform",
            "pip",
            "go",
            "mvn",
            "gradle",
            "apt",
            "brew",
            "systemctl",
            "journalctl",
            "curl",
            "jq",
            "awk",
            "sed",
            "make",
            "cmake",
            "pytest",
            "jest",
            "eslint",
            "tsc",
            "next",
            "vite",
            "webpack",
            "helm",
            "aws",
            "gcloud",
            "az",
            "ssh",
            "rsync",
            "tar",
            "df",
            "du",
            "top",
            "netstat",
            "ip",
            "dig",
        ];
        let seeds: Vec<String> = vec![
            "é".repeat(50),
            "日".repeat(200),
            "\u{1F600}".repeat(80),
            format!("{}é", "a".repeat(9)),
            format!("{}日", "x".repeat(11)),
            "a\tb\tc\n".repeat(300),
            "|".repeat(5000),
            "\r\n".repeat(400),
            format!("NAME  READY\n{}", "é  1/1\n".repeat(200)),
            format!("{{\"a\":\"{}\"}}", "日".repeat(100)),
            "\u{0}\u{1}\u{2}é\n".repeat(100),
            " ".repeat(10000),
            format!("┌ ○ /{}   279 B   106 kB\n", "é".repeat(20)).repeat(50),
            format!("error: {}\n", "日".repeat(300)).repeat(20),
            "-".repeat(3000),
            format!("{}\n", "é".repeat(4000)),
        ];
        let mut n = 0;
        for p in progs {
            for sub in [
                "", "status", "build", "get", "install", "list", "test", "run",
            ] {
                let argv: Vec<String> = if sub.is_empty() {
                    vec![p.into()]
                } else {
                    vec![p.into(), sub.into()]
                };
                for s in &seeds {
                    let _ = crate::filter::filter_command(&argv, s);
                    n += 1;
                }
            }
        }
        println!("FUZZED {n} combinations with no panic");
    }

    /// The agent's contract is that wrapping a command never costs it the
    /// command. Before the boundary existed, `wrap` called `filter_command`
    /// bare: a panic anywhere in 3 352 specs plus hundreds of modules lost the
    /// WHOLE output and turned the command's exit code into 101.
    #[test]
    fn a_panicking_compactor_costs_the_agent_nothing() {
        let argv = vec!["__tokenade_panic_probe__".to_string()];
        let text = "line one\nline two\nthe decisive error line\n";
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {})); // keep the probe's backtrace out of the log
        let r = filter_command_fail_open(&argv, text);
        std::panic::set_hook(prev);
        assert_eq!(
            r.output, text,
            "the output was not preserved through the panic"
        );
        assert_eq!(r.filter, "passthrough");
        assert_eq!(
            r.savings.tokens_before, r.savings.tokens_after,
            "a run that compacted nothing must not be credited with savings"
        );
    }

    /// What a module's `compact` returns is NOT what the agent receives:
    /// `filter_command` runs lossless post-passes over ANY routed output, and
    /// `fold_repetitive_runs` in particular folds the N-receipts shape that no
    /// per-command compactor removes.
    ///
    /// MEASURED 2026-08-04, and it corrects a conclusion drawn the same day
    /// from the raw module functions:
    ///
    /// | input | raw module | DELIVERED |
    /// |---|---|---|
    /// | `systemctl` 22 621 B | 22 620 | **159** |
    /// | `dmesg` 18 180 B | 13 489 | **107** |
    ///
    /// So "the specific compactor is weaker than the generic one" was an
    /// artefact of calling `systemctl::compact` instead of `filter_command`.
    /// Nothing needs picking between them — the post-passes already beat both.
    /// Compare DELIVERED output, never a module function.
    #[test]
    fn the_post_passes_carry_output_a_module_left_untouched() {
        let mut sysd = String::from("UNIT   LOAD   ACTIVE SUB     DESCRIPTION\n");
        for i in 0..400 {
            sysd.push_str(&format!(
                "svc{i}.service  loaded active running Service number {i}\n"
            ));
        }
        let raw = crate::cmd::linux::systemctl::compact(&sysd);
        let delivered = filter_command(&argv(&["systemctl", "status"]), &sysd).output;
        assert!(
            raw.len() > sysd.len() / 2,
            "fixture no longer exercises the case: the module now folds it on its own"
        );
        assert!(
            delivered.len() * 20 < raw.len(),
            "the post-passes stopped rescuing output the module left alone: {} vs {}",
            delivered.len(),
            raw.len()
        );
    }
}

#[cfg(test)]
mod cd_prefix_attribution {
    use super::*;

    fn v(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    /// `cd … && cmd` was already handled; `cd …; cmd` and `cd …` followed by a
    /// loop were not, and both are idiomatic. Whitespace-splitting glues the
    /// `;` to the path (`/x;`), so the connector is not a token at all, and a
    /// newline separator disappears entirely.
    ///
    /// MEASURED on the real ledger 2026-08-25: 34 rows carrying 108 425 tokens
    /// of `before` were attributed to `cd`, a command that writes nothing. The
    /// dashboard groups savings by this field to show "where the savings come
    /// from", so the answer it gave named the navigation instead of the work.
    #[test]
    fn a_cd_prefix_never_takes_credit_for_the_command_after_it() {
        // Already correct — the separator is its own token.
        assert_eq!(program_basename(&v("cd /x && python3 run.py")), "python3");
        assert_eq!(program_basename(&v("cd /x ; git status")), "git");

        // The two shapes from the ledger.
        assert_eq!(
            program_basename(&v("cd /x; curl -s https://e/x -o /tmp/y")),
            "curl",
            "a `;` glued to the path hid the real command"
        );
        assert_eq!(
            program_basename(&v("cd /x for f in a b; do cat $f; done")),
            "cat",
            "a newline-separated construct after `cd` was credited to `cd`"
        );

        // …and a bare `cd` with nothing after it is still `cd`.
        assert_eq!(program_basename(&v("cd /some/where")), "cd");
        assert_eq!(program_basename(&v("cd")), "cd");
    }

    /// The two ways the first fix went wrong, kept as cases because each is a
    /// rule the scan has to obey and neither is obvious from the happy path.
    #[test]
    fn the_connector_scan_stops_where_the_clause_does() {
        let v = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();

        // (1) It cannot be width-bounded. A silent prefix may take SEVERAL
        //     arguments, so the connector is not always within two tokens.
        assert_eq!(program_basename(&v("mkdir -p out && git status")), "git");
        assert_eq!(
            program_basename(&v("export A=1 B=2 && cargo build")),
            "cargo"
        );

        // (2) It must not run past the clause. Scanning for any token ending
        //     in `;` found the `b;` of a loop HEADER, and resuming past the
        //     `ok;` of a `then` branch landed on `fi` — a terminator that
        //     produces nothing.
        assert_eq!(
            program_basename(&v("if git diff --quiet; then echo ok; fi")),
            "echo"
        );
        assert_eq!(
            program_basename(&v("cd /x for f in a b; do cat $f; done")),
            "cat"
        );
        assert_eq!(
            program_basename(&v("cd /srv && for f in a b; do LANG=C git log; done")),
            "git"
        );
    }
}

#[cfg(test)]
mod byte_precise_reads {
    use super::*;

    fn argv(cmd: &str) -> Vec<String> {
        cmd.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn every_byte_flag_spelling_is_recognised() {
        for cmd in [
            "head -c 300 src/lib.rs",
            "head -c300 src/lib.rs",
            "head --bytes 300 src/lib.rs",
            "head --bytes=300 src/lib.rs",
            "tail -c 300 src/lib.rs",
            "head -qc 300 src/lib.rs",
            "head -vc300 src/lib.rs",
        ] {
            assert!(head_tail_byte_precise(&argv(cmd)), "missed: {cmd}");
        }
    }

    #[test]
    fn a_line_read_is_not_a_byte_read() {
        for cmd in [
            "head -400 src/lib.rs",
            "head -n 400 src/lib.rs",
            "head -n400 src/lib.rs",
            "head --lines=400 src/lib.rs",
            "tail -f app.log",
            "head src/lib.rs",
            "sed -n 200,600p src/lib.rs",
            "cat -c src/lib.rs",
        ] {
            assert!(!head_tail_byte_precise(&argv(cmd)), "false positive: {cmd}");
        }
    }

    /// The damage, stated as bytes. A source file large enough to clear
    /// `RANGED_READ_MIN_TOKENS` was comment-stripped: identical for a while,
    /// then silently divergent, with no banner anywhere.
    #[test]
    fn a_byte_read_of_source_comes_back_byte_exact() {
        let mut src = String::new();
        for i in 0..600 {
            src.push_str(&format!(
                "// explanatory comment number {i} that a stripper would remove\nlet v{i} = {i};\n"
            ));
        }
        assert!(
            tokenizer::estimate(&src) > RANGED_READ_MIN_TOKENS,
            "fixture must clear the ranged-read gate or it proves nothing"
        );
        let out = filter_command(&argv("head -c 4000 src/lib.rs"), &src).output;
        assert_eq!(out, src, "a byte-precise read was rewritten");
    }

    /// …and the line-based ranged read it shares an arm with still folds,
    /// so the guard is a carve-out and not a hole.
    #[test]
    fn a_line_ranged_read_of_source_still_strips() {
        let mut src = String::new();
        for i in 0..600 {
            src.push_str(&format!(
                "// explanatory comment number {i} that a stripper would remove\nlet v{i} = {i};\n"
            ));
        }
        let out = filter_command(&argv("head -400 src/lib.rs"), &src).output;
        assert!(
            out.len() < src.len(),
            "the ranged-read strip must still fire on a LINE read"
        );
    }
}

#[cfg(test)]
mod cat_of_a_bundle {
    use super::*;

    fn bundle(n: usize) -> String {
        let mut one =
            String::from("import{a as Q}from\"@scope/pkg\";const R=require(\"node:fs\");");
        for i in 0..n {
            one.push_str(&format!(
                "function f{i}($,Z){{let Y=$.x??0;if(typeof Z===\"string\")return Y;var W=(q)=>q+{i};return W(Y)}}"
            ));
        }
        one.push_str("export function handleRequest(Q){return f0(Q,\"\")}");
        format!("#!/usr/bin/env node\n{one}\n")
    }

    fn argv(path: &str) -> Vec<String> {
        vec!["cat".to_string(), path.to_string()]
    }

    /// `first 150 / last 50 lines` of a file with eight lines returns the
    /// file. The slice cannot help where there are no lines to slice.
    #[test]
    fn a_bundle_is_summarised_rather_than_sliced() {
        let b = bundle(400);
        let tokens = crate::tokenizer::estimate(&b);
        let out = cat_apply_large_truncation(&argv("dist/index.js"), &b, tokens)
            .expect("above the large-file threshold");
        assert!(
            out.contains("minified"),
            "not routed to the bundle summary: {}",
            &out[..120.min(out.len())]
        );
        assert!(out.contains("@scope/pkg"), "the specifiers are missing");
        assert!(out.contains("handleRequest"), "the export is missing");
        assert!(
            out.len() < b.len() / 10,
            "barely folded: {} of {}",
            out.len(),
            b.len()
        );
    }

    /// `git show <rev>:<path>` prints a FILE. The commit compactor sampled it
    /// into lines out of context; it is read as `cat` reads the same file, and
    /// its recourse names the revision, not the file on disk.
    #[test]
    fn a_blob_shown_by_git_is_read_like_the_file() {
        let src: String = (0..600)
            .map(|i| format!("pub fn f{i}(x: u32) -> u32 {{\n    x + {i}\n}}\n"))
            .collect();
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let blob = filter_command(&v(&["git", "show", "HEAD~2:src/lib.rs"]), &src);
        let cat = filter_command(&v(&["cat", "src/lib.rs"]), &src);
        assert_eq!(blob.filter, "git-show");
        let body = |o: &str| o.lines().skip(1).collect::<Vec<_>>().join("\n");
        assert_eq!(body(&blob.output), body(&cat.output), "same read as cat");
        assert!(blob
            .output
            .contains("`HEAD~2:src/lib.rs`, not the file on disk"));
        assert!(!blob.output.contains("tokenade read src/lib.rs"));
        // A commit is still a commit.
        let commit =
            "commit 0123456789abcdef0123456789abcdef01234567\nAuthor: a <a@b>\n\n    msg\n";
        let shown = filter_command(&v(&["git", "show", "HEAD"]), commit);
        assert!(
            shown.output.starts_with("commit 0123456789abcdef"),
            "{}",
            shown.output
        );
    }

    /// And an ordinary large source file still gets its head/tail slice —
    /// there the lines ARE the structure.
    #[test]
    fn an_ordinary_source_file_still_gets_the_slice() {
        let src: String = (0..600)
            .map(|i| format!("pub fn f{i}(x: u32) -> u32 {{\n    x + {i}\n}}\n"))
            .collect();
        let tokens = crate::tokenizer::estimate(&src);
        let out = cat_apply_large_truncation(&argv("src/lib.rs"), &src, tokens)
            .expect("above the large-file threshold");
        // Asserted on the PROPERTY, not on the sentence: this test exists to
        // say the slice path was taken rather than the minified one, and it
        // used to do that by matching the words "showing first", which pinned a
        // banner that had to change (it quoted two constants of a total it
        // never gave, and the head is adaptive anyway).
        assert!(
            out.contains("elided from the middle"),
            "the slice path was abandoned: {}",
            &out[..160.min(out.len())]
        );
        assert!(!out.contains("minified"));
        assert!(
            out.lines().count() < src.lines().count(),
            "nothing was actually elided"
        );
    }

    /// The elided middle keeps its declaration headers in place, so the agent
    /// sees WHAT is there (`enroll`) and can ask for that body — of 23
    /// `cat-large` deliveries, the middle was pulled back for 3+, and the
    /// marker had only said how many lines were gone.
    #[test]
    fn the_elided_middle_keeps_its_declarations() {
        let names = [
            "enroll",
            "revoke",
            "rotate_key",
            "audit_trail",
            "pending_since",
            "approve",
        ];
        let mut src = String::from("use std::collections::HashMap;\n\n");
        for (k, n) in names.iter().cycle().take(36).enumerate() {
            let _ = writeln!(
                src,
                "pub fn {n}_{k}(store: &mut HashMap<String, u32>, id: &str) -> Option<u32> {{"
            );
            for j in 0..14 {
                let _ = writeln!(
                    src,
                    "    let v{j} = store.get(id).copied().unwrap_or({}) + {j};",
                    k * 3
                );
            }
            src.push_str("    store.insert(id.to_string(), 1)\n}\n\n");
        }
        let tokens = crate::tokenizer::estimate(&src);
        let out =
            cat_apply_large_truncation(&argv("src/workstations.rs"), &src, tokens).expect("large");
        assert!(
            out.contains("its declarations kept in place"),
            "{}",
            &out[..200]
        );
        // A function wholly inside the elided middle is still named.
        assert!(
            out.contains(
                "pub fn rotate_key_20(store: &mut HashMap<String, u32>, id: &str) -> Option<u32> {"
            ),
            "{out}"
        );
        assert!(
            !out.contains("let v7 = store.get(id).copied().unwrap_or(60) + 7;"),
            "its body is not"
        );
        assert!(out.lines().count() < src.lines().count() / 2);
    }

    /// A loop that reads several files (`for f in …; do echo "═══ $f"; cat -n
    /// "src/$f.rs"; done`): each file's header the caller printed survives the
    /// cut, and so does each file's first line — a blank numbered line (`  6`,
    /// its tab trimmed) is NOT a separator.
    #[test]
    fn a_loop_of_reads_keeps_where_each_file_starts() {
        let mut src = String::new();
        for f in ["alpha", "beta", "gamma", "delta"] {
            let _ = writeln!(src, "═══ src/{f}.rs");
            let mut n = 1;
            for k in 0..20 {
                let _ = writeln!(src, "{n:>6}\tpub fn {f}_{k}(x: u32) -> u32 {{");
                n += 1;
                for j in 0..8 {
                    let _ = writeln!(src, "{n:>6}\t    let v{j} = x.wrapping_mul({j}) + {k};");
                    n += 1;
                }
                let _ = writeln!(src, "{n:>6}\t}}\n{:>6}", n + 1);
                n += 2;
            }
        }
        let tokens = crate::tokenizer::estimate(&src);
        let out = cat_apply_large_truncation(&argv("src/$f.rs"), &src, tokens).expect("large");
        for f in ["alpha", "beta", "gamma", "delta"] {
            assert!(
                out.contains(&format!("═══ src/{f}.rs")),
                "{f} header lost: {out}"
            );
        }
        let bare = out
            .lines()
            .filter(|l| !l.trim().is_empty() && l.trim().chars().all(|c| c.is_ascii_digit()))
            .count();
        assert!(
            bare < 30,
            "{bare} blank numbered lines kept from the middle"
        );
    }

    #[test]
    fn a_read_is_routed_on_its_own_clause() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            super::dispatch::read_clause(&a(&["cat", "-n", "\"src/commands/$f.rs\";", "done"])),
            a(&["cat", "-n", "src/commands/$f.rs"])
        );
        assert_eq!(
            super::dispatch::read_clause(&a(&["cat", "a.rs", "|", "head"])),
            a(&["cat", "a.rs"])
        );
        assert_eq!(
            super::dispatch::read_clause(&a(&["cat", "a.rs", "b.rs"])),
            a(&["cat", "a.rs", "b.rs"])
        );
    }
}

#[cfg(test)]
mod a_claimant_that_folds_nothing_hands_over {
    use super::*;

    /// The shape of a 29 018-byte storybook debug log: bracketed timestamps and
    /// level tags, which the `Json` detector claims and its compactor leaves
    /// byte-for-byte alone.
    fn claimed_but_unfolded() -> String {
        // A named kind must CLAIM this and fold nothing — that is the whole
        // property these three tests are about.
        //
        // The original fixture mixed prose that MENTIONS a path with lines
        // ending on one, and the archive branch claimed the mixture because it
        // counted any line containing a slash. Now it counts only lines
        // that END on their path, so every line here is a real listing entry:
        // forty distinct directories group to forty rows plus a banner, and
        // the claimant folds nothing.
        let mut s = String::new();
        for i in 0..40 {
            s.push_str(&format!(
                "[14:25:{:02}.462] [DEBUG] Getting package.json info for /Users/dev/proj/packages/mod_{i}/package.json\n",
                i % 60
            ));
            s.push_str(&format!(
                "[14:25:{:02}.999] [DEBUG] Reading tsconfig from /Users/dev/proj/packages/ui_{i}/tsconfig.json\n",
                i % 60
            ));
        }
        s
    }

    /// Distinct lines nobody claims — the shape where the information IS
    /// the lines and folding them removes the answer.
    fn unclaimed_document() -> String {
        (0..80)
            .map(|i| {
                format!("collected item number {i} from the suite named alpha_{i} with marker slow_{i}\n")
            })
            .collect()
    }

    #[test]
    fn precondition_a_named_kind_claims_it_and_folds_nothing() {
        let t = claimed_but_unfolded();
        let (kind, _out, sav) = auto::compact(&t);
        assert_ne!(kind, auto::Kind::Unknown, "a named kind must claim it");
        assert!(
            sav.tokens_after >= sav.tokens_before,
            "it must fold nothing: {} -> {}",
            sav.tokens_before,
            sav.tokens_after
        );
    }

    #[test]
    fn the_content_path_hands_over_instead_of_serving_it_whole() {
        let t = claimed_but_unfolded();
        let (kind, out, sav) = auto::compact(&t);
        let (label, delivered, s) =
            structured_or_generic(&t, sav.tokens_before, kind, out, sav, Unclaimed::Keep);
        assert!(
            delivered.len() < t.len(),
            "delivered {} bytes of {} as `{label}` — the claimant folded nothing and \
             nothing took over",
            delivered.len(),
            t.len()
        );
        assert!(s.tokens_after < s.tokens_before, "and it must be booked");
    }

    #[test]
    fn a_named_claimant_is_not_subject_to_the_unclaimed_policy() {
        let t = claimed_but_unfolded();
        let (kind, out, sav) = auto::compact(&t);
        let keep = structured_or_generic(
            &t,
            sav.tokens_before,
            kind,
            out.clone(),
            sav,
            Unclaimed::Keep,
        );
        let fold = structured_or_generic(&t, sav.tokens_before, kind, out, sav, Unclaimed::Fold);
        assert_eq!(
            keep.1, fold.1,
            "the policy answers for UNCLAIMED bytes only; both paths owe the same \
             answer once a detector has named the format"
        );
        assert_eq!(keep.0, fold.0, "and the same label");
    }

    /// The two paths genuinely disagree about bytes nobody claimed, and
    /// the disagreement is declared as an argument rather than spelled twice.
    #[test]
    fn an_unclaimed_document_is_served_whole_and_unclaimed_command_output_is_not() {
        let t = unclaimed_document();
        let (kind, out, sav) = auto::compact(&t);
        assert_eq!(kind, auto::Kind::Unknown, "precondition");

        let (label, kept, _) = structured_or_generic(
            &t,
            sav.tokens_before,
            kind,
            out.clone(),
            sav,
            Unclaimed::Keep,
        );
        assert_eq!(kept, t, "a document nobody claimed is served whole");
        assert_eq!(label, "passthrough");

        let (_, folded, _) =
            structured_or_generic(&t, sav.tokens_before, kind, out, sav, Unclaimed::Fold);
        assert!(
            folded.len() < t.len(),
            "and command output nobody claimed is still folded: {} vs {}",
            folded.len(),
            t.len()
        );
    }
}

#[cfg(test)]
mod fold_footer_magnitude_tests {
    use crate::compress::truncate_middle;
    use crate::elision::run_weight;

    /// The footer names what the elided lines STOOD FOR, not just how many
    /// lines of the compacted view went.
    ///
    /// MEASURED 2026-09-04: `lsof -n -P` (370 367 lines) through
    /// `tokenade filter` announced `folded 8147 line(s) total`, every one of
    /// them a collapsed run — the real loss was two orders of magnitude
    /// larger than the number the agent was handed to judge it by.
    #[test]
    fn the_fold_footer_says_what_the_elided_lines_stood_for() {
        let mut view = String::new();
        for i in 0..400 {
            // Every third line is a collapsed run standing for 100 originals.
            if i % 3 == 0 {
                view.push_str(&format!("proc{i} 1234 user mem REG /lib/x{i}.so  (×100)\n"));
            } else {
                view.push_str(&format!("proc{i} 1234 user {i}u REG /path/f{i}.dat\n"));
            }
        }
        // Same reason as next door: the view is FABRICATED, so it declares the
        // fold it claims to carry (134 runs of 100). Without this line, the
        // provenance ceiling brings the claim back to the line count, and the
        // test only passed by inheriting a neighbour's ceiling.
        super::note_folded(134 * 100);
        let out = truncate_middle(&view, 20, 10);
        let footer = out
            .lines()
            .find(|l| l.starts_with("[tokenade folded "))
            .expect("a fold this size must carry a footer");
        assert!(
            footer.contains("standing for"),
            "the footer counts view lines and says nothing of what they stood for: {footer}"
        );
        // The claimed magnitude is bigger than the line count, and no larger
        // than what the runs actually declare.
        let nums: Vec<usize> = footer
            .split(|c: char| !c.is_ascii_digit())
            .filter(|s| !s.is_empty())
            .map(|s| s.parse().unwrap())
            .collect();
        assert!(nums.len() >= 2 && nums[1] > nums[0], "{footer}");

        // A view with no collapsed runs claims nothing extra: the two numbers
        // would be the same, and saying it twice is noise.
        let plain: String = (0..400).map(|i| format!("plain line {i}\n")).collect();
        let out = truncate_middle(&plain, 20, 10);
        let footer = out
            .lines()
            .find(|l| l.starts_with("[tokenade folded "))
            .expect("footer");
        assert!(
            !footer.contains("standing for"),
            "an ordinary fold invented a second magnitude: {footer}"
        );
    }

    #[test]
    fn a_run_marker_is_read_only_where_it_really_is_one() {
        assert_eq!(run_weight("proc 1 user mem REG /lib/x.so  (×47)"), 47);
        assert_eq!(run_weight("proc 1 user mem REG /lib/x.so  (×47) "), 47);
        // Not a run marker: no multiplier, a bare paren, or one that says one.
        assert_eq!(run_weight("proc 1 user mem REG /lib/x.so"), 1);
        assert_eq!(run_weight("a line that ends in (parentheses)"), 1);
        assert_eq!(run_weight("collapsed once  (×1)"), 1);
        assert_eq!(run_weight("truncated marker  (×)"), 1);
    }
}

#[cfg(test)]
mod interaction_handles {
    use super::*;

    #[test]
    fn a_tree_of_refs_is_handles_and_a_sentence_about_one_is_not() {
        assert!(carries_interaction_handles(
            "- link [ref=e1]\n- link [ref=e2]\n- button [ref=e3]\n"
        ));
        assert!(!carries_interaction_handles(
            "the snapshot uses [ref=e1] markers\n"
        ));
        assert!(!carries_interaction_handles("no handles here at all\n"));
    }
}

#[cfg(test)]
mod never_longer_than_its_input {
    use super::*;

    /// A delivered answer is never longer than the bytes it replaced.
    ///
    /// The comparisons in `structured_or_generic` all weigh
    /// `tokenizer::estimate`, and the agent pays on a different tokenizer.
    /// MEASURED 2026-09-04 on 400 lines of `keyN: valueN`: our estimate said
    /// 2,839 tokens in and 2,373 out, tiktoken said 2,399 in and 2,475 out,
    /// and the answer was 242 BYTES longer than the file. Bytes are the one
    /// number in scope that is not an estimate.
    #[test]
    fn an_answer_longer_than_its_input_is_not_delivered() {
        let src: String = (0..400).map(|i| format!("key{i}: value{i}\n")).collect();
        let before = tokenizer::estimate(&src);
        let (label, out, sav) = structured_or_generic(
            &src,
            before,
            auto::Kind::Yaml,
            format!("{src}[a footer that costs more than the fold saved]\n"),
            Savings::new(before, before - 1),
            Unclaimed::Keep,
        );
        assert_eq!(label, "passthrough");
        assert_eq!(out, src, "the input is what comes back");
        assert_eq!(sav.tokens_after, sav.tokens_before, "and nothing is booked");
    }

    /// And a real fold is still delivered, or the rule would be "never fold".
    #[test]
    fn a_shorter_answer_is_delivered_as_before() {
        let src: String = (0..400)
            .map(|i| format!("2026-09-04T10:00:00Z INFO svc handled req {i} ok\n"))
            .collect();
        let before = tokenizer::estimate(&src);
        let (kind, compacted, sav) = auto::compact(&src);
        let (_label, out, _s) =
            structured_or_generic(&src, before, kind, compacted, sav, Unclaimed::Keep);
        assert!(
            out.len() < src.len() / 10,
            "a fold that wins is still delivered: {} bytes of {}",
            out.len(),
            src.len()
        );
    }
}

#[cfg(test)]
mod line_ending_fidelity {
    use super::*;

    #[test]
    fn a_rebuild_that_changed_only_line_endings_is_not_a_compaction() {
        assert!(differs_only_in_line_endings(
            "# T\r\n\r\nprose\r\n",
            "# T\n\nprose\n"
        ));
        // The trailing newline is decided at the print site, so a rebuild that
        // dropped it is still the same text.
        assert!(differs_only_in_line_endings(
            "# T\r\n\r\nprose\r\n",
            "# T\n\nprose"
        ));
        // Identical is not a difference.
        assert!(!differs_only_in_line_endings("a\r\nb\r\n", "a\r\nb\r\n"));
        // A real reshape is not caught.
        assert!(!differs_only_in_line_endings(
            "a\r\nb\r\nc\r\n",
            "a  [x3 lines]\n"
        ));
        // Losing a line is not "only line endings".
        assert!(!differs_only_in_line_endings("a\r\nb\r\n", "a\n"));
    }
}

#[cfg(test)]
mod a_banner_is_not_part_of_the_file {
    use super::*;

    /// The truncation banner counts the FILE, not the note above it.
    ///
    /// `cat_try_code_strip` gained a declaration line, and the truncation
    /// that runs on its output counted that line as one of the file's: a
    /// 450-line source came back as `451 lines`, and the "N line(s) elided"
    /// arithmetic was computed against the inflated total. Found by sweeping
    /// every countable claim a banner makes against the input it replaced —
    /// one impossible claim out of thirteen, and it was mine.
    #[test]
    fn a_strip_banner_is_not_counted_as_a_line_of_the_file() {
        let mut src = String::new();
        // Over the delivery threshold, where the strip still runs.
        for i in 0..600 {
            src.push_str(&format!(
                "# note {i} that a reader wants\ndef f{i}(a):\n    return a+{i}\n"
            ));
        }
        let src = src.trim_end_matches('\n').to_string();
        let real = src.lines().count();
        assert_eq!(real, 1800, "fixture drifted");
        let argv: Vec<String> = ["cat", "mod.py"].iter().map(|s| s.to_string()).collect();
        let stripped = cat_try_code_strip(&argv, &src).expect("a commented source is stripped");
        assert!(
            stripped.starts_with("[tokenade:"),
            "premise: the strip declares itself, which is what puts a line on top"
        );
        let out = cat_truncate_keeping_banner(&argv, stripped, tokenizer::estimate(&src));
        let banner = out
            .lines()
            .find(|l| l.contains("cat-large"))
            .expect("a file this size is truncated");
        assert!(
            banner.contains(&format!("{real} lines")),
            "the banner must count the file, not the note above it:\n{banner}"
        );
        // The declaration must survive the middle-elision — it is the one line
        // that cannot be re-derived from what is left.
        assert!(
            out.starts_with("[tokenade:py]"),
            "the strip declaration was elided with the middle:\n{}",
            out.lines().next().unwrap_or("")
        );
        // Anti-vacuity: something really was elided, or nothing was measured.
        assert!(
            banner.contains("elided from the middle"),
            "the fixture stopped tripping the truncation:\n{banner}"
        );
    }
}

#[cfg(test)]
mod a_stripped_source_says_so {
    use super::*;

    /// A source slice that lost its comments says so, and says how many.
    ///
    /// Every other stripper in this crate returns through
    /// `code_strip::declare_strip` — toml, hcl, bazel, proto, sql, cmake, ini.
    /// The SOURCE path, the busy one (`cat`, `sed -n 'A,Bp'`, `head`, `tail`
    /// on a `.py`/`.rs`/`.ts`), returned the stripped text bare. A comment line
    /// is replaced by an EMPTY line, so the file comes back with the right
    /// number of lines and a blank where the explanation was: it does not read
    /// as compacted, it reads as a file that has no comments. Field quote from
    /// this machine's transcripts, an agent reasoning about its own repo:
    /// "OK, the file is correct (the previous sed was showing a version
    /// compacted by tokenade)" — it had concluded the file was wrong.
    ///
    /// The count is asserted, not just the presence of a banner: because the
    /// blanking keeps the line count, `declare_strip`'s fallback wording would
    /// have called 200 removed lines "inline comments trimmed".
    #[test]
    fn a_source_slice_names_the_comment_lines_it_removed() {
        let mut src = String::new();
        for i in 0..200 {
            src.push_str(&format!(
                "# explanation for helper {i} that a reader wants\n"
            ));
            src.push_str(&format!("def fn_{i}(a, b):\n    return a + b + {i}\n"));
        }
        let argv: Vec<String> = ["sed", "-n", "1,600p", "mod.py"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let out = cat_try_code_strip(&argv, &src).expect("a commented source is stripped");
        assert!(
            out.lines()
                .next()
                .unwrap_or("")
                .contains("(line numbers unchanged)"),
            "a comment strip leaves the anchors alone and must say so:\n{}",
            out.lines().next().unwrap_or("")
        );
        assert!(
            out.starts_with("[tokenade:py] 200 comment line(s) stripped"),
            "the banner must name the language and the exact count:\n{}",
            out.lines().next().unwrap_or("")
        );
        assert!(
            out.contains("the file on disk is unchanged"),
            "and must say the file itself did not change:\n{out}"
        );
        // Anti-vacuity: the body is still the code, and the comments really are
        // gone — a banner over an unstripped file would prove nothing.
        assert!(out.contains("def fn_199(a, b):"), "the code must survive");
        assert!(
            !out.contains("explanation for helper 199"),
            "the comments must actually be gone, or there is nothing to declare"
        );

        // When the banner would cost more than the strip saves, `declare_strip`
        // hands back the ORIGINAL text — and answering `Some(original)` would
        // tell the caller a compaction happened when none did. That case must
        // read as "nothing to do".
        let barely = "# one note\ndef f():\n    return 1\n";
        assert!(
            cat_try_code_strip(&argv, barely).is_none(),
            "a strip smaller than its own banner must not be reported as one"
        );

        // The OTHER shape: a file with no comments whose blank runs
        // collapse. Lines really do go, so the anchors move, and calling that
        // "comments stripped" is a claim about a file that holds none. It used
        // to say exactly that: the first version counted position-by-position
        // over two texts of different lengths and reported 80 removed comments
        // on a file with zero.
        let mut runs = String::new();
        for i in 0..200 {
            runs.push_str(&format!("def g_{i}(a, b):\n    return {i}\n\n\n\n"));
        }
        let out = cat_try_code_strip(&argv, &runs).expect("blank runs are collapsed");
        let head = out.lines().next().unwrap_or("");
        assert!(
            head.contains("blank runs collapsed") && head.contains("line numbers have SHIFTED"),
            "a blank-run collapse must say what it did and that anchors moved:\n{head}"
        );
        assert!(
            !head.contains("comment"),
            "and must not claim comments were stripped from a file with none:\n{head}"
        );

        // A file with nothing to strip is returned untouched, with no banner.
        let plain = "def f():\n    return 1\n".repeat(200);
        assert!(
            cat_try_code_strip(&argv, &plain).is_none_or(|o| !o.starts_with("[tokenade:")),
            "a file with no comments must not be given a banner"
        );
    }
}

#[cfg(test)]
mod named_if_it_folded_tests {
    /// The module that folded keeps the hand; the one that folded nothing loses it.
    #[test]
    fn only_a_module_that_folded_something_keeps_the_hand() {
        let original = "0123456789";
        let fallback = "abcd".to_string();
        // Folded: it wins, even if it is LONGER than the generic fallback —
        // its judgement is what we keep (zombies counted rather than listed).
        assert_eq!(
            super::named_if_it_folded(fallback.clone(), Some("012345".to_string()), original),
            "012345"
        );
        // Folded nothing: the generic one goes through.
        assert_eq!(
            super::named_if_it_folded(fallback.clone(), Some(original.to_string()), original),
            fallback
        );
        // GREW: same, and it is the case that matters — `tasklist /v` returns
        // 7 185 bytes by one route and 2 862 by the other.
        assert_eq!(
            super::named_if_it_folded(
                fallback.clone(),
                Some("0123456789abc".to_string()),
                original
            ),
            fallback
        );
        assert_eq!(
            super::named_if_it_folded(fallback.clone(), None, original),
            fallback
        );
    }
}

#[cfg(test)]
mod the_program_of_a_real_chain {
    /// Shapes seen in real usage (2026-10-02, 30 days) and reproduced in the
    /// local registry: `done;` 530 times, `&&` 249 times counted as COMMANDS.
    /// The name feeds the displayed coverage, the ranking attribution and the
    /// routing to a compactor.
    fn name(s: &str) -> String {
        let seg = crate::command_rewrite::meaningful_segment(s);
        let argv: Vec<String> = seg.split_whitespace().map(String::from).collect();
        crate::filter::program_basename(&argv)
    }
    fn whole(s: &str) -> String {
        let argv: Vec<String> = s.split_whitespace().map(String::from).collect();
        crate::filter::program_basename(&argv)
    }

    #[test]
    fn a_closed_wait_loop_yields_the_command_that_follows() {
        assert_eq!(
            name(
                r#"until grep -qE "x|y" /tmp/a.log 2>/dev/null; do sleep 3; done; tail -20 /tmp/a.log"#
            ),
            "tail"
        );
        assert_eq!(whole("while true; do sleep 5; done && cat out.log"), "cat");
        assert_eq!(
            whole("until x; do sleep 1; done; echo; grep -c y f.log"),
            "grep"
        );
        // A loop's body stays the program when it has one.
        assert_eq!(name("for f in a b; do wc -l $f; done"), "wc");
    }

    #[test]
    fn a_clause_of_assignments_is_not_a_program() {
        assert_eq!(name("S=/tmp/y && cp a b && npm test"), "cp");
        assert_eq!(whole("cd /x && S=/tmp/y && cp a b"), "cp");
        assert!(crate::command_rewrite::segment_is_a_label("S=/tmp/y"));
        // An assignment as a PREFIX keeps its program.
        assert_eq!(whole("LANG=C cargo build"), "cargo");
        // An assignment's VALUE spans several words: its flag is not a
        // program (`attribution`, 2026-10-02: `-m1` ×3 …).
        assert_eq!(
            whole("PG=$(grep -m1 '^DATABASE_URL=' .env | cut -d= -f2-) && python3 - <<PY"),
            "python3"
        );
        assert_eq!(
            whole(r#"P=$(pgrep -f "uvicorn main:api" | head -1); ls -l /proc"#),
            "ls"
        );
        assert_eq!(whole(r#"MSG="a b c" make test"#), "make");
        // A LOWERCASE name is an assignment for the router too.
        assert_eq!(whole("d=$(ls -dt ~/s/*/ | head -1); grep x $d"), "grep");
        assert_eq!(whole("f=/tmp/x cat $f"), "cat");
    }
}

#[cfg(test)]
mod the_declared_wrappers_are_routed {
    /// `is_wrapper_prog` (label) and the dispatcher arm (routing) name the
    /// same wrappers — two lists, because `build.rs` reads the arm's
    /// literals. A name added to only one would label a command by what it
    /// wraps and route it on the wrapper, or the reverse. This test replaces
    /// the "Must stay in sync" comment.
    #[test]
    fn every_declared_wrapper_is_routed_by_what_it_wraps() {
        let out = "On branch main\nnothing to commit, working tree clean\n";
        let bare = crate::filter::filter_command(&["git".into(), "status".into()], out).filter;
        for w in super::WRAPPER_PROGS {
            let argv: Vec<String> = [*w, "git", "status"]
                .iter()
                .map(|s| s.to_string())
                .collect();
            assert_eq!(
                crate::filter::program_basename(&argv),
                "git",
                "label under `{w}`"
            );
            assert_eq!(
                crate::filter::filter_command(&argv, out).filter,
                bare,
                "`{w} git status` is not routed like `git status`: the dispatcher arm does not know it"
            );
        }
    }
}

#[cfg(test)]
mod a_numbered_file_is_a_document {
    /// Indented Rust code, numbered as `nl -ba` renders it: the case measured on
    /// 2026-10-02 (`nl -ba dispatch.rs | sed -n '300,500p'`).
    fn numbered_code(lines: usize) -> String {
        (0..lines)
            .map(|i| {
                let body = match i % 4 {
                    0 => "        // a comment that explains the next arm in prose".to_string(),
                    1 => format!("        (\"prog{i}\", _) => {{"),
                    2 => format!("            return Err(auto_or_generic(output, {i}));"),
                    _ => "        }".to_string(),
                };
                format!("{:>6}\t{body}", 300 + i)
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    #[test]
    fn a_numbered_range_keeps_every_line_and_its_indentation() {
        let raw = numbered_code(201);
        let argv: Vec<String> = "nl -ba x.rs | sed -n '300,500p'"
            .split_whitespace()
            .map(String::from)
            .collect();
        let r = crate::filter::filter_command(&argv, &raw);
        // 1.1.21: flattened (0 indented lines); first fix: one line in two of
        // the middle elided by `generic`.
        assert!(!r.output.contains("elided by tokenade"), "{}", r.filter);
        assert_eq!(
            r.output
                .lines()
                .filter(|l| l.contains("\t        "))
                .count(),
            201,
            "{}",
            r.filter
        );
    }

    #[test]
    fn padding_is_judged_on_what_the_gutter_numbers() {
        assert!(!crate::archetypes::padding_is_decorative(&numbered_code(
            40
        )));
        // An interleaved elision marker does not demote the code to a table.
        let mut mixed = numbered_code(40);
        mixed.insert_str(0, "… [12 lines elided by tokenade] …\n");
        assert!(!crate::archetypes::padding_is_decorative(&mixed));
        // A REAL table, even numbered, stays a table.
        let table: String = (0..12)
            .map(|i| {
                format!(
                    "{:>6}\tnginx-{i}        Running        10.0.0.{i}        3d\n",
                    i + 1
                )
            })
            .collect();
        assert!(crate::archetypes::padding_is_decorative(&table));
    }
}

#[cfg(test)]
mod nl_is_a_cat_n {
    /// A REAL numbered source file, like `nl` (blanks not numbered) or
    /// `cat -n` (everything numbered) — a uniform synthetic fixture goes to
    /// the shape fold, rightly so, and no longer tests reading a file.
    fn numbered(blank_numbered: bool) -> String {
        let src = include_str!("filter/cat.rs");
        let mut n = 0u64;
        let mut out = String::new();
        for line in src.lines() {
            if line.trim().is_empty() && !blank_numbered {
                out.push('\n');
                continue;
            }
            n += 1;
            out.push_str(&format!("{n:>6}\t{line}\n"));
        }
        out
    }
    fn run(cmd: &str, out: &str) -> crate::filter::FilterResult {
        let argv: Vec<String> = cmd.split_whitespace().map(String::from).collect();
        crate::filter::filter_command(&argv, out)
    }

    /// `nl FILE` went through the `nl` spec: table, "problems" first, 50
    /// head lines and 5 tail lines. It is a `cat -n`.
    #[test]
    fn nl_on_a_source_file_reads_like_cat_n() {
        let r = run("nl src/filter/cat.rs", &numbered(false));
        assert_eq!(r.filter, "nl");
        assert!(!r.output.contains("problem(s) first"));
        // Same reading as `cat -n`: equal numbering, equal output.
        let all = numbered(true);
        assert_eq!(
            run("nl -ba src/filter/cat.rs", &all).output,
            run("cat -n src/filter/cat.rs", &all).output
        );
        let head = r.output.lines().next().unwrap();
        assert!(head.contains("comment line(s) stripped"), "{head}");
        assert!(head.contains("line numbers unchanged"), "{head}");
        assert!(head.contains("blank line(s) removed"), "{head}");
        assert!(!head.contains("SHIFTED"), "{head}");
    }

    #[test]
    fn cat_n_says_the_numbers_hold() {
        let head = run("cat -n src/filter/cat.rs", &numbered(true))
            .output
            .lines()
            .next()
            .unwrap()
            .to_string();
        assert!(head.contains("comment line(s) stripped"), "{head}");
        assert!(head.contains("line numbers unchanged"), "{head}");
    }
}

#[cfg(test)]
mod a_normal_diff_is_served_whole {
    /// A NORMAL diff of two versions of a Rust file: `///` lines (which
    /// detection took for markdown) and indented code, over more than 180
    /// lines (beyond that, `generic` samples).
    fn normal_diff() -> String {
        let src = include_str!("filter/cat.rs");
        let mut out = String::from("12a13,212\n");
        for line in src.lines().filter(|l| !l.trim().is_empty()).take(200) {
            out.push_str(&format!("> {line}\n"));
        }
        out.push_str("300,301c500,501\n<     old_a();\n<     old_b();\n---\n>     new_a();\n>     new_b();\n");
        out
    }

    #[test]
    fn every_changed_line_arrives_as_is() {
        let raw = normal_diff();
        let argv: Vec<String> = ["diff", "a.rs", "b.rs"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let r = crate::filter::filter_command(&argv, &raw);
        assert_eq!(r.output, raw, "filter {}", r.filter);
    }

    #[test]
    fn indentation_behind_a_side_marker_is_not_decoration() {
        assert!(!crate::archetypes::padding_is_decorative(&normal_diff()));
    }
}

#[cfg(test)]
mod a_read_truncated_by_head_keeps_its_code {
    /// `cat F.rs | head -330` — the read an agent makes of a file that is too
    /// long. Spotted in this machine's registry (`op: generic`), then
    /// reproduced: 140 code lines out of 330 were dropped, "in 67 runs of 2".
    fn first(n: usize) -> String {
        include_str!("../../tokenade-cli/src/hooks/replay.rs")
            .lines()
            .take(n)
            .map(|l| format!("{l}\n"))
            .collect()
    }

    #[test]
    fn no_code_line_is_dropped() {
        let input = first(330);
        let argv: Vec<String> = "cat crates/tokenade-cli/src/hooks/replay.rs | head -330"
            .split_whitespace()
            .map(String::from)
            .collect();
        let r = crate::filter::filter_command(&argv, &input);
        let kept: std::collections::HashSet<&str> = r.output.lines().map(str::trim).collect();
        let lost: Vec<&str> = input
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with("//"))
            .filter(|l| !kept.contains(l))
            .collect();
        assert!(
            lost.is_empty(),
            "filter `{}` lost {} code line(s), first: {:?}",
            r.filter,
            lost.len(),
            lost.first()
        );
    }
}

#[cfg(test)]
mod a_sorted_list_stays_a_list {
    /// Names the way people write them: spaces, accents, parentheses.
    fn listing() -> String {
        let dirs = [
            "Acme/04_Audit-et-contexte/ateliers",
            "Acme/09_Ma-production",
            "Acme/Use cases/Relance clients",
        ];
        let words = [
            "Atelier Audit IT (Entretiens)",
            "Synthèse de l'atelier",
            "Présentation Audit",
            "Liste des cas d_usage",
            "Reprise technique Lot1",
            "Maintenance et infrastructure",
            "Contrat signé",
            "Compte rendu réunion",
            "Feuille de route",
            "Budget prévisionnel",
            "Note de cadrage",
            "Retour d'expérience",
            "Plan de charge",
            "Glossaire métier",
            "Annexe juridique",
        ];
        let exts = ["md", "pdf", "docx", "xlsx"];
        let mut v = Vec::new();
        for (i, d) in dirs.iter().enumerate() {
            for (j, w) in words.iter().enumerate() {
                v.push(format!(
                    "{d}/{w} {}.{}",
                    2020 + (i * 7 + j * 3) % 7,
                    exts[(i + j) % 4]
                ));
            }
        }
        v.sort();
        v.join("\n") + "\n"
    }

    #[test]
    fn sorted_find_keeps_the_find_route_and_every_name() {
        let out = listing();
        let argv: Vec<String> = "find Acme -type f | sort"
            .split_whitespace()
            .map(String::from)
            .collect();
        let r = crate::filter::filter_command(&argv, &out);
        assert_eq!(r.filter, "find", "{}", r.output);
        for path in out.lines() {
            let name = path.rsplit('/').next().unwrap();
            assert!(
                r.output.contains(name),
                "lost {name:?} under `{}`",
                r.filter
            );
        }
    }

    #[test]
    fn only_stages_that_order_or_cut_keep_the_route() {
        let a = |c: &str| c.split_whitespace().map(String::from).collect::<Vec<_>>();
        let f = crate::filter::lister_stage_when_the_rest_only_orders;
        assert!(f(&a("find . -name x | sort -u | head -20")).is_some());
        assert!(f(&a("fd rs | uniq")).is_some());
        assert!(
            f(&a("find . | uniq -c")).is_none(),
            "a count is a new shape"
        );
        assert!(f(&a("find . | xargs wc -l")).is_none());
        assert!(f(&a("ls | sort")).is_none());
        assert!(f(&a("find .")).is_none(), "no pipe, nothing to decide");
        // PowerShell: the lister, and only the stages that slice or reorder.
        assert!(f(&a(
            "Get-ChildItem C:\\Windows -Filter *.dll | Select-Object -First 400"
        ))
        .is_some());
        assert!(f(&a("gci . | Sort-Object")).is_some());
        assert!(
            f(&a("Get-ChildItem . | Select-Object Name,Length")).is_none(),
            "a projection is a new shape"
        );
    }
}

#[cfg(test)]
mod a_range_read_by_sed_keeps_its_lines {
    /// `sed -n 42,420p detect.rs` — spotted in this machine's registry under
    /// `op: generic`: one line in two of the `///` blocks was dropped, "in
    /// 67 runs of 1", with no banner saying so.
    #[test]
    fn no_line_is_dropped_silently() {
        // The slice as it was read, frozen: `detect.rs` is a module trunk
        // with branches, and a test reading it would follow the file through
        // every split (guard `no_guard_reads_a_trunk_that_has_branches`).
        let input = include_str!("../tests/fixtures/build/detect-rs-lines-42-420.txt").to_string();
        let argv: Vec<String> = "sed -n 42,420p crates/tk-agents/src/detect.rs"
            .split_whitespace()
            .map(String::from)
            .collect();
        let r = crate::filter::filter_command(&argv, &input);
        let kept: std::collections::HashSet<&str> = r.output.lines().map(str::trim).collect();
        let lost: Vec<&str> = input
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !kept.contains(l))
            .collect();
        // The comment strip is DECLARED: its banner counts what it took.
        let declared = r
            .output
            .lines()
            .next()
            .is_some_and(|l| l.contains("comment"));
        let silent: Vec<&&str> = lost
            .iter()
            .filter(|l| !(declared && l.starts_with("//")))
            .collect();
        assert!(
            silent.is_empty(),
            "filter `{}` lost {} line(s) unannounced, first: {:?}\n{}",
            r.filter,
            silent.len(),
            silent.first(),
            r.output.lines().next().unwrap_or("")
        );
    }
}

#[cfg(test)]
mod a_small_source_range_comes_back_identical {
    /// `sed -n 'A,Bp' store.rs` over the SQL schema in a Rust string: the
    /// slice went to content detection, and the `--` of the string literal
    /// were stripped as SQL comments.
    #[test]
    fn the_sql_of_a_rust_string_is_not_stripped() {
        let src = include_str!("../../tk-fold/src/dedup_ref/store.rs");
        let at = src
            .lines()
            .position(|l| l.contains("-- Read-range coverage"))
            .expect("fixture moved");
        let slice: String = src
            .lines()
            .skip(at - 30)
            .take(68)
            .map(|l| format!("{l}\n"))
            .collect();
        assert!(
            crate::tokenizer::estimate(&slice) <= 1_500,
            "the fixture must stay a SMALL slice"
        );
        let argv: Vec<String> = "sed -n 100,167p crates/tk-fold/src/dedup_ref/store.rs"
            .split_whitespace()
            .map(String::from)
            .collect();
        let r = crate::filter::filter_command(&argv, &slice);
        assert_eq!(
            r.output, slice,
            "filter `{}` rewrote a surgical read",
            r.filter
        );
    }
}

#[cfg(test)]
mod an_announced_hole_carries_its_way_back {
    /// Real output (nextest, repeated failures) through `grep -A4 … | head
    /// -40`: delivered with `… [15 lines elided by tokenade] …` under a
    /// `template-fold` banner saying "nothing was dropped", and no line
    /// saying where to find the fifteen lines.
    #[test]
    fn a_hole_in_the_view_comes_with_its_footer() {
        let input = include_str!("../tests/fixtures/build/nextest-failures-repeated.txt");
        let argv: Vec<String> = "grep -A4 panicked e2e.log | head -40"
            .split_whitespace()
            .map(String::from)
            .collect();
        let r = crate::filter::filter_command(&argv, input);
        assert!(
            r.output.contains("lines elided by tokenade"),
            "the fixture no longer folds:\n{}",
            r.output
        );
        assert!(
            r.output.contains("[tokenade folded "),
            "filter `{}` elided lines with no footer:\n{}",
            r.filter,
            r.output
        );
    }
}

#[cfg(test)]
mod an_awk_that_picks_lines_is_a_read {
    /// Python whose `#` comments read like headings: the real case (a
    /// third-party file, not copied here) came out folded as markdown, 73
    /// lines out of 101 gone.
    fn python() -> String {
        let mut s = String::from("\"\"\"Outils de connecteurs.\n\nLe filtre porte sur les permissions accordées.\n\"\"\"\n\n");
        for (i, word) in [
            "jetons", "portées", "serveur", "tenant", "graphe", "courrier",
        ]
        .iter()
        .enumerate()
        {
            s.push_str(&format!(
                "# {word} : ce que le bloc {i} vérifie\n# lu au démarrage, avant le premier appel\n\nclass C{i}:\n    \"\"\"Connecteur {word}.\"\"\"\n\n    def name(self) -> str:\n        return \"{word}-{i}\"\n\n"
            ));
        }
        s
    }
    fn run(cmd: &str, out: &str) -> crate::filter::FilterResult {
        let argv: Vec<String> = cmd.split_whitespace().map(String::from).collect();
        crate::filter::filter_command(&argv, out)
    }

    #[test]
    fn the_slice_comes_back_identical() {
        let src = python();
        let r = run("awk 'NR>=1 && NR<=80' app/toolkit.py", &src);
        assert_eq!(
            r.output, src,
            "filter `{}` rewrote a line selection",
            r.filter
        );
    }

    #[test]
    fn only_a_selection_program_counts() {
        let a = |c: &str| c.split_whitespace().map(String::from).collect::<Vec<_>>();
        use crate::filter::awk_only_selects_lines as sel;
        assert!(sel(&a("awk 'NR>=200 && NR<=300' f.py")));
        assert!(sel(&a("awk 'NR==5' f.py")));
        assert!(sel(&a("awk 'NR>10 && NR<20 {print}' f.py")));
        assert!(
            !sel(&a("awk '{print $1}' f.py")),
            "a transform is not a read"
        );
        assert!(!sel(&a("awk 'NR>1 {print $2}' f.py")));
        assert!(!sel(&a("awk -F: '{print NR}' f.py")));
    }
}

#[cfg(test)]
mod a_source_read_that_fits_keeps_its_comments {
    /// Six weeks of transcripts: after a `cat`/`sed -n` of source rendered
    /// `comments stripped`, the agent re-read the file in 18 to 25% of
    /// cases (`.ts`, `.rs`, `.py`, `.sh`). Below the shipping threshold, the
    /// comment — the intent of the code — stays.
    #[test]
    fn a_small_source_file_comes_back_with_its_comments() {
        let mut src = String::new();
        for i in 0..60 {
            src.push_str(&format!(
                "// wire-compat {i}: do NOT renumber\npub const K{i}: u32 = {i};\n"
            ));
        }
        for cmd in ["cat src/consts.rs", "sed -n 1,200p src/consts.rs"] {
            let argv: Vec<String> = cmd.split_whitespace().map(String::from).collect();
            let r = crate::filter::filter_command(&argv, &src);
            assert!(
                r.output.contains("// wire-compat 37: do NOT renumber"),
                "{cmd}: comment stripped\n{}",
                &r.output[..r.output.len().min(300)]
            );
            assert!(r.output.contains("pub const K59: u32 = 59;"), "{cmd}");
        }
    }
}

#[cfg(test)]
mod several_sources_read_at_once {
    /// `for f in a.rs b.rs c.rs; do echo "=== $f"; sed -n 1,200p $f; done`:
    /// `generic` folded 258 lines "in 86 runs of 3" and the banner of the
    /// middle file disappeared.
    #[test]
    fn a_loop_of_reads_comes_back_as_is() {
        let dir = std::env::temp_dir().join(format!("tkn-loopread-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut out = String::new();
        let mut names = Vec::new();
        for n in ["a.rs", "b.rs", "c.rs"] {
            let p = dir.join(n);
            let body: String = (0..90)
                .map(|i| {
                    format!("    let v{i} = step({i});\n    check(v{i})?;\n    log(\"{n}\");\n")
                })
                .collect();
            std::fs::write(&p, &body).unwrap();
            out.push_str(&format!("=== {}\n{body}", p.display()));
            names.push(p.display().to_string());
        }
        let cmd = format!(
            "for f in {}; do echo \"=== $f\"; sed -n 1,400p $f; done",
            names.join(" ")
        );
        let argv: Vec<String> = cmd.split_whitespace().map(String::from).collect();
        let r = crate::filter::filter_command(&argv, &out);
        assert_eq!(
            r.output, out,
            "filter `{}` rewrote a read of three source files",
            r.filter
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod raw_help_without_its_escapes {
    /// coreutils 9.8: every `--help` option in an OSC 8 link and bold —
    /// `ls --help`, 5 023 bytes of escapes out of 12 316.
    #[test]
    fn the_text_stays_the_escapes_go() {
        let mut help = String::from("Usage: ls [OPTION]... [FILE]...\n\n");
        for (o, d) in [
            ("-a, --all", "do not ignore entries starting with ."),
            ("-A, --almost-all", "do not list implied . and .."),
        ] {
            help.push_str(&format!(
                "  \x1b]8;;https://www.gnu.org/software/coreutils/manual/coreutils.html#ls{o}\x1b\\\x1b[1m{o}\x1b[0m\x1b]8;;\x1b\\\n         {d}\n"
            ));
        }
        let argv: Vec<String> = ["ls", "--help"].iter().map(|s| s.to_string()).collect();
        let r = crate::filter::filter_command(&argv, &help);
        assert!(
            !r.output.contains('\x1b'),
            "escapes delivered:\n{:?}",
            r.output
        );
        assert!(
            r.output
                .contains("  -A, --almost-all\n         do not list implied . and .."),
            "{}",
            r.output
        );
        // A byte-precise slice keeps its bytes.
        let argv: Vec<String> = ["head", "-c", "400"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(crate::filter::filter_command(&argv, &help).output, help);
    }
}

#[cfg(test)]
mod piped_output_without_its_escapes {
    fn argv(s: &str) -> Vec<String> {
        s.split(' ').map(String::from).collect()
    }

    /// `pnpm run ci 2>&1 | tail -60`, real call of 2026-10-02: the content
    /// route folded nothing, the passthrough returned the 11 colour pairs.
    #[test]
    fn a_piped_ci_summary_loses_its_colours() {
        let mut out = String::from("> orbit@0.0.0 ci\n> node scripts/run-checks.mjs\n\n");
        for (i, ok) in [true, false, true, true, false, true, true, true, true, true]
            .iter()
            .enumerate()
        {
            let (c, m) = if *ok { (32, '✔') } else { (31, '✖') };
            out.push_str(&format!(
                "  \x1b[{c}m{m}\x1b[0m {:<22} 0.{i} s\n",
                format!("check-{i}:run")
            ));
            if !ok {
                out.push_str(&format!("\n      ✖ check-{i} found 1 problem in apps/a{i}.ts\n       ELIFECYCLE  Command failed with exit code 1.\n\n"));
            }
        }
        out.push_str("\n8.2 s au total\n\x1b[31m✖ 2 vérification(s) en échec\x1b[0m\n");
        for cmd in ["pnpm run ci 2>&1 | tail -60", "./run-checks.sh"] {
            let r = crate::filter::filter_command(&argv(cmd), &out);
            assert!(
                !r.output.contains('\x1b'),
                "{cmd}: escapes delivered:\n{:?}",
                r.output
            );
            assert!(
                r.output.contains("✖ check-4 found 1 problem in apps/a4.ts"),
                "{cmd}:\n{}",
                r.output
            );
            assert!(
                r.output.contains("✖ 2 vérification(s) en échec"),
                "{cmd}:\n{}",
                r.output
            );
        }
    }

    /// Cursor redraws are replayed BEFORE the cleanup: a redrawn bar leaves
    /// only its last state.
    #[test]
    fn a_redraw_is_replayed_before_the_cleanup() {
        let mut out = String::new();
        for p in 0..=20 {
            out.push_str(&format!("\x1b[1A\x1b[2Kprogress {}%\n", p * 5));
        }
        out.push_str("done\n");
        let r =
            crate::filter::filter_command(&argv("./build.sh | tail -40"), &format!("start\n{out}"));
        assert!(!r.output.contains('\x1b'), "{:?}", r.output);
        assert!(
            !r.output.contains("progress 50%"),
            "the redraws were not replayed:\n{}",
            r.output
        );
        assert!(
            r.output.contains("progress 100%") && r.output.contains("done"),
            "{}",
            r.output
        );
    }
}

#[cfg(test)]
mod a_listing_keeps_its_names {
    /// `ls -la` of a repo: 44 `…-VBoxHeadless-NNN.log` logs merged into two
    /// templates by the raw-output fold — 42 names lost, whereas `ls-long`
    /// kept every line on purpose.
    #[test]
    fn every_file_of_a_long_ls_stays_named() {
        let mut out = String::from("total 12692\n");
        for i in 0..120 {
            out.push_str(&format!(
                "-rw------- 1 user user {} {} juin  {:02}:{:02} 2026-06-{:02}-07-{:02}-11.0{:02}-VBoxHeadless-{}.log\n",
                14_000 + i * 37, 1 + i % 28, 8 + i % 12, (i * 7) % 60, 1 + i % 28, i % 60, i, 2_027_025 + i * 9_311
            ));
        }
        for n in ["Cargo.toml", "README.md", "crates", "docs", "scripts"] {
            out.push_str(&format!("-rw-r--r-- 1 user user 1234 2 oct.  10:00 {n}\n"));
        }
        let argv: Vec<String> = ["ls", "-la"].iter().map(|s| s.to_string()).collect();
        let r = crate::filter::filter_command(&argv, &out);
        for i in 0..120 {
            assert!(
                r.output
                    .contains(&format!("VBoxHeadless-{}.log", 2_027_025 + i * 9_311)),
                "name {i} lost:\n{}",
                r.output
            );
        }
        assert!(!r.output.contains("[×"), "{}", r.output);
    }

    /// `node app.js` crashed: the program's frames stay, the runtime's are
    /// counted — through the arm production runs, not the spec row.
    #[test]
    fn a_crashed_node_script_counts_its_runtime_frames() {
        let argv: Vec<String> = ["node", "app.js"].iter().map(|s| s.to_string()).collect();
        let raw = "processing item 0\nError: ENOENT: no such file or directory, open './b.json'\n    at Object.readFileSync (node:fs:441:20)\n    at loadBudget (app.js:2:47)\n    at Module._compile (node:internal/modules/cjs/loader:1706:14)\n    at wrapModuleLoad (node:internal/modules/cjs/loader:237:24) {\n  code: 'ENOENT'\n}\n\nNode.js v22.21.1\n";
        let r = crate::filter::filter_command(&argv, raw);
        // The content route's StackTrace pass re-renders the frames it keeps
        // flush left; what is asserted is what survives, not the indentation.
        assert!(
            r.output.contains("at loadBudget (app.js:2:47)\n"),
            "{}",
            r.output
        );
        assert!(
            r.output
                .contains("[3 runtime frame(s) not shown] {\n  code: 'ENOENT'"),
            "{}",
            r.output
        );
        assert!(!r.output.contains("node:internal"), "{}", r.output);
    }

    /// `pdftotext -layout`: the table's padding rows go and its columns
    /// survive the content route as pipe rows.
    #[test]
    fn pdftotext_layout_tables_become_pipe_rows() {
        let argv: Vec<String> = ["pdftotext", "-layout", "r.pdf", "-"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut raw = String::from("Rapport\n\n Région           T1        T2\n\n");
        for z in 1..=12 {
            raw.push_str(&format!(
                " Zone {z}           {}        {}\n\n",
                z * 1000,
                z * 2000
            ));
        }
        let r = crate::filter::filter_command(&argv, &raw);
        assert!(r.output.contains("| Région | T1 | T2 |"), "{}", r.output);
        assert!(
            r.output.contains("| Zone 7 | 7000 | 14000 |"),
            "{}",
            r.output
        );
    }
}
