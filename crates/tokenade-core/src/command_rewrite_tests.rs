// The tests of `command_rewrite`.
//
// Spliced by `include!` from `command_rewrite.rs`: the same module, so the same
// private access and the same `super::` paths, without a single visibility
// line changed. It is the shape this repo already uses for 33 other modules.
//
// A `//!` is impossible here: a file spliced by `include!` is not a
// module, and an INNER doc comment has nothing to qualify.

/// A bare newline separates commands exactly as `;` does.
#[cfg(test)]
mod tests {

    /// A variable-assignment prefix is an assignment whatever its case.
    /// `is_env_assignment_token` demanded UPPERCASE, so a lowercase prefix was
    /// left in and the program was read out of it.
    #[test]
    fn a_lowercase_assignment_prefix_is_still_a_prefix() {
        // The value's basename was being reported as the program.
        assert_eq!(first_program_token("f=/tmp/x cat notes.txt"), "cat");
        assert_eq!(first_program_token("p=/tmp/x vim notes.txt"), "vim");
        assert_eq!(first_program_token("n=3 seq 1 5"), "seq");
        // Uppercase kept working the whole time; it must still.
        assert_eq!(first_program_token("N=3 seq 1 5"), "seq");
        assert_eq!(first_program_token("LANG=C sort -u f"), "sort");
    }

    /// And the consequence that made it more than a naming slip: everything
    /// keyed on the program name answered about a program the command never
    /// named. `vim` behind a lowercase prefix stopped being recognised as
    /// interactive, and wrapping it hangs the agent on a TTY it cannot get.
    #[test]
    fn an_interactive_program_behind_a_lowercase_prefix_is_still_interactive() {
        for c in [
            "p=/tmp/x vim notes.txt",
            "d=/srv ssh -T root@h ls",
            "log=/tmp/a less /var/log/syslog",
        ] {
            let prog = first_program_token(c);
            assert!(
                matches!(prog, "vim" | "ssh" | "less"),
                "{c} reported the program {prog:?}"
            );
        }
    }

    /// What is NOT an assignment stays a program. A shell variable name is a
    /// letter or `_` then letters, digits and `_`; anything else keeps its
    /// token.
    #[test]
    fn a_token_that_is_not_a_variable_name_is_not_an_assignment() {
        // A flag with a value is not an assignment prefix.
        assert_eq!(first_program_token("--flag=v prog a"), "--flag=v");
        // Nor is a dotted name.
        assert_eq!(first_program_token("a.b=c prog"), "a.b=c");
        // A digit cannot open a variable name.
        assert_eq!(first_program_token("1x=2 prog"), "1x=2");
    }

    /// Reported from the field and reproduced 2026-08-03: through the wrapper,
    /// `pgrep -af "sleep 987"` lists tokenade's own process, because
    /// `tokenade wrap '<cmd>'` carries `<cmd>` in its own command line. For
    /// `pkill -f` that is not a wrong listing, it is a signal sent to a process
    /// the user never named.
    #[cfg(test)]
    mod process_matching {
        use super::super::segment_must_not_be_wrapped;

        #[test]
        fn full_command_line_matching_is_never_wrapped() {
            for cmd in [
                "pgrep -f sleep",
                "pgrep -af \"git clone\"",
                "pgrep -fa foo",
                "pkill -f scratchpad",
                "pkill --full scratchpad",
                "pgrep --full x",
            ] {
                assert!(
                    segment_must_not_be_wrapped(cmd),
                    "{cmd} matches by command line and must not be wrapped"
                );
            }
        }

        /// Matching by process NAME cannot see an argument list, so those keep
        /// their normal treatment — narrowing the exclusion to what is actually
        /// broken rather than to everything that looks like it.
        #[test]
        fn name_matching_is_left_alone() {
            for cmd in [
                "pgrep sleep",
                "pgrep -x sleep",
                "pkill -9 node",
                "killall node",
            ] {
                assert!(
                    !segment_must_not_be_wrapped(cmd),
                    "{cmd} matches by name; wrapping it changes nothing"
                );
            }
        }

        /// After `--` everything is a pattern. A pattern that merely contains
        /// an `f` must not be read as the flag.
        #[test]
        fn a_pattern_after_the_separator_is_not_a_flag() {
            assert!(!segment_must_not_be_wrapped("pgrep -- -foo"));
        }
    }
    use super::*;

    #[test]
    fn identity_when_no_rule() {
        let r = rewrite("echo hello");
        assert_eq!(r.command, "echo hello");
        assert!(r.rule.is_none());
        assert!(!r.split_chain);
    }

    #[test]
    fn git_status_rewrites_to_porcelain() {
        let r = rewrite("git status");
        assert!(r.command.contains("--porcelain=v2"));
        assert_eq!(r.rule, Some("git-status-porcelain"));
    }

    #[test]
    fn git_status_keeps_user_porcelain_flag() {
        let r = rewrite("git status --porcelain=v1");
        assert_eq!(r.command, "git status --porcelain=v1");
        assert!(r.rule.is_none(), "must not override user's explicit flag");
    }

    #[test]
    fn git_log_oneline_default() {
        let r = rewrite("git log");
        assert_eq!(r.rule, Some("git-log-oneline"));
        assert!(r.command.contains("--oneline"));
    }

    #[test]
    fn git_log_with_pretty_is_left_alone() {
        let r = rewrite("git log --pretty=full");
        assert!(r.rule.is_none());
    }

    #[test]
    fn pytest_terse() {
        let r = rewrite("pytest tests/test_x.py");
        assert!(r.command.contains("-q"));
        assert!(r.command.contains("--tb=line"));
        assert_eq!(r.rule, Some("pytest-quiet"));
    }

    #[test]
    fn pytest_verbose_is_left_alone() {
        let r = rewrite("pytest -v tests/");
        assert!(r.rule.is_none());
    }

    #[test]
    fn cargo_build_gets_quiet() {
        let r = rewrite("cargo build --release");
        assert!(r.command.contains("--quiet"));
        assert_eq!(r.rule, Some("cargo-quiet"));
        // Original args preserved.
        assert!(r.command.contains("--release"));
    }

    #[test]
    fn cargo_run_is_not_rewritten() {
        // `cargo run` produces program output, not build noise — leave alone.
        let r = rewrite("cargo run -- --port 8080");
        assert!(r.rule.is_none());
    }

    #[test]
    fn kubectl_get_pods_to_name_only() {
        let r = rewrite("kubectl get pods");
        assert!(r.command.ends_with("-o name"));
        assert_eq!(r.rule, Some("kubectl-name-only"));
    }

    #[test]
    fn kubectl_get_with_user_output_left_alone() {
        let r = rewrite("kubectl get pods -o yaml");
        assert!(r.rule.is_none());
    }

    #[test]
    fn docker_ps_compact() {
        let r = rewrite("docker ps");
        assert!(r.command.contains("--format"));
        assert_eq!(r.rule, Some("docker-ps-format"));
    }

    #[test]
    fn docker_ps_with_format_left_alone() {
        let r = rewrite("docker ps --format json");
        assert!(r.rule.is_none());
    }

    #[test]
    fn npm_install_gets_silent() {
        let r = rewrite("npm install react");
        assert!(r.command.contains("--silent"));
        assert_eq!(r.rule, Some("npm-silent"));
    }

    #[test]
    fn npm_view_is_not_rewritten() {
        let r = rewrite("npm view react");
        assert!(r.rule.is_none());
    }

    #[test]
    fn tsc_no_pretty() {
        let r = rewrite("tsc -p tsconfig.json");
        assert!(r.command.contains("--pretty false"));
        assert_eq!(r.rule, Some("tsc-no-pretty"));
    }

    #[test]
    fn vue_tsc_no_pretty() {
        // vue-tsc shares tsc's diagnostic format → same de-prettify rewrite.
        let r = rewrite("vue-tsc --noEmit");
        assert!(r.command.contains("--pretty false"));
        assert_eq!(r.rule, Some("tsc-no-pretty"));
    }

    #[test]
    fn bare_ls_one_per_line() {
        let r = rewrite("ls");
        assert!(r.command.contains("-1"));
    }

    #[test]
    fn ls_with_args_is_left_alone() {
        let r = rewrite("ls -la /etc");
        assert!(r.rule.is_none());
    }

    #[test]
    fn compound_chain_focuses_last_segment() {
        let r = rewrite("cd /tmp && pytest tests/");
        assert!(r.split_chain);
        assert!(r.command.contains("-q"));
        assert!(!r.command.contains("cd /tmp"));
    }

    #[test]
    fn pipe_focuses_rightmost() {
        let r = rewrite("cat foo.txt | grep error | head -10");
        // No rule fires for `head`, but we still focus on the rightmost.
        assert!(r.split_chain);
        assert_eq!(r.command, "head -10");
    }

    #[test]
    fn semicolon_chain() {
        let r = rewrite("echo start; cargo test");
        assert!(r.split_chain);
        assert!(r.command.contains("cargo test"));
        assert!(r.command.contains("--quiet"));
    }

    #[test]
    fn double_pipe_is_not_a_split() {
        // `a || b` runs b only on failure — we MUST NOT focus on b alone.
        let r = rewrite("cargo build || echo failed");
        assert!(!r.split_chain);
        // The whole expression stays. No rule fires because of the chain.
    }

    #[test]
    fn env_prefix_preserved() {
        let r = rewrite("RUST_LOG=debug cargo test");
        assert!(r.command.starts_with("RUST_LOG=debug"));
        assert!(r.command.contains("--quiet"));
    }

    #[test]
    fn heredoc_is_bailed_on() {
        let r = rewrite("cat <<EOF\nhello\nEOF");
        assert!(r.rule.is_none());
        assert!(!r.split_chain);
    }

    #[test]
    fn subshell_is_bailed_on() {
        let r = rewrite("echo $(git status)");
        assert!(r.rule.is_none());
    }

    #[test]
    fn backtick_is_bailed_on() {
        let r = rewrite("echo `git log`");
        assert!(r.rule.is_none());
    }

    #[test]
    fn eval_is_bailed_on() {
        let r = rewrite("eval git status");
        assert!(r.rule.is_none());
    }

    #[test]
    fn quotes_protect_special_chars() {
        // The `&&` inside quotes should NOT trigger a split.
        let r = rewrite("echo 'a && b'");
        assert!(!r.split_chain);
        assert_eq!(r.command, "echo 'a && b'");
    }

    // ── adversarial / property-style ─────────────────────────────────────

    #[test]
    fn rewrite_never_panics_on_pathological_input() {
        for s in [
            "",
            " ",
            "\n\n\n",
            "&&&&&&",
            ";;;;;;",
            "|||",
            "\"\"",
            "''",
            "$(",
            "<<EOF",
            "cd && pytest",
            "    ",
            "a\\&\\&b",
        ] {
            let _ = rewrite(s);
        }
    }

    #[test]
    fn original_is_always_preserved() {
        // The Rewrite contract: callers MUST be able to recover what
        // they passed in. We test that field after rewrites that fire,
        // rewrites that no-op, and pathological inputs.
        for s in [
            "git status",
            "echo hi",
            "cd && pytest",
            "eval foo",
            "cat <<EOF",
        ] {
            let r = rewrite(s);
            assert_eq!(r.original, s, "original drifted for: {s}");
        }
    }

    #[test]
    fn yarn_install_gets_silent() {
        let r = rewrite("yarn install");
        assert_eq!(r.rule, Some("yarn-silent"));
        assert!(r.command.contains("--silent"));
    }

    #[test]
    fn yarn_explicit_silent_is_left_alone() {
        let r = rewrite("yarn --silent install");
        assert!(r.rule.is_none() || !r.command.contains("--silent --silent"));
    }

    #[test]
    fn yarn_unknown_subcommand_no_rewrite() {
        // `yarn workspaces` isn't in our verb allowlist — should pass through.
        let r = rewrite("yarn workspaces list");
        assert!(r.rule.is_none());
    }

    #[test]
    fn make_gets_silent_and_no_print() {
        let r = rewrite("make build");
        assert_eq!(r.rule, Some("make-silent"));
        assert!(r.command.contains("-s"));
        assert!(r.command.contains("--no-print-directory"));
    }

    #[test]
    fn make_explicit_silent_is_left_alone() {
        let r = rewrite("make -s test");
        assert!(r.rule.is_none());
    }

    #[test]
    fn mvn_quiet_added() {
        let r = rewrite("mvn install");
        assert_eq!(r.rule, Some("mvn-quiet"));
        assert!(r.command.starts_with("mvn -q"));
    }

    #[test]
    fn mvn_explicit_debug_left_alone() {
        let r = rewrite("mvn -X compile");
        assert!(r.rule.is_none(), "debug flag should suppress mvn rewrite");
    }

    #[test]
    fn gradle_quiet_added() {
        let r = rewrite("gradle build");
        assert_eq!(r.rule, Some("gradle-quiet"));
        assert!(r.command.contains(" -q"));
    }

    #[test]
    fn gradlew_quiet_added() {
        // Project-local wrapper script.
        let r = rewrite("gradlew test");
        assert_eq!(r.rule, Some("gradle-quiet"));
    }

    #[test]
    fn helm_list_short() {
        let r = rewrite("helm list");
        assert_eq!(r.rule, Some("helm-short"));
        assert!(r.command.contains("--short"));
    }

    #[test]
    fn helm_explicit_output_left_alone() {
        let r = rewrite("helm list -o json");
        assert!(r.rule.is_none());
    }

    #[test]
    fn ansible_playbook_one_line() {
        let r = rewrite("ansible-playbook deploy.yml");
        assert_eq!(r.rule, Some("ansible-one-line"));
        assert!(r.command.contains("--one-line"));
    }

    #[test]
    fn ansible_verbose_left_alone() {
        let r = rewrite("ansible-playbook -vv deploy.yml");
        assert!(r.rule.is_none());
    }

    // --- split_chain_segments + segment_must_not_be_wrapped tests ---

    fn segs(cmd: &str) -> Vec<(String, Option<&'static str>)> {
        split_chain_segments(cmd)
            .unwrap()
            .into_iter()
            .map(|s| (s.cmd.to_string(), s.separator_after))
            .collect()
    }

    /// bash deletes a backslash-newline before parsing; the splitter kept it in
    /// the segment, where re-quoting turned it into a literal argument.
    #[test]
    fn split_refuses_across_a_line_continuation() {
        assert_eq!(split_chain_segments("echo one \\\n&& echo two"), None);
        // An escape that is NOT a continuation must still split.
        assert_eq!(segs("echo a\\ b && echo c").len(), 2);
        assert_eq!(segs("echo one && echo two").len(), 2);
    }

    /// `#` starts a comment, so everything after it is not a command at all.
    /// Cutting the chain at a separator that sits INSIDE the comment turned a
    /// disabled command back into a running one — the one rewrite outcome that
    /// is never acceptable, whatever it costs in batching.
    #[test]
    fn split_refuses_when_a_comment_could_hide_a_separator() {
        assert_eq!(split_chain_segments("echo one # && echo two"), None);
        assert_eq!(split_chain_segments("echo one ; echo two # disabled"), None);
        assert_eq!(split_chain_segments("# everything here is a comment"), None);
        // A `#` that is not a comment must not cost us the split.
        assert_eq!(segs("echo a#b && echo c").len(), 2);
        assert_eq!(segs("curl http://x/#frag && echo done").len(), 2);
        assert_eq!(segs("echo '# quoted' && echo c").len(), 2);
    }

    #[test]
    fn split_single_segment_no_separator() {
        let r = segs("cargo build");
        assert_eq!(r, vec![("cargo build".to_string(), None)]);
    }

    #[test]
    fn split_on_and_and() {
        let r = segs("cargo build && cargo test");
        assert_eq!(
            r,
            vec![
                ("cargo build".to_string(), Some("&&")),
                ("cargo test".to_string(), None),
            ]
        );
    }

    #[test]
    fn split_on_or_or() {
        let r = segs("cargo build || echo failed");
        assert_eq!(
            r,
            vec![
                ("cargo build".to_string(), Some("||")),
                ("echo failed".to_string(), None),
            ]
        );
    }

    #[test]
    fn split_on_semicolon() {
        let r = segs("cargo build ; cargo test");
        assert_eq!(
            r,
            vec![
                ("cargo build".to_string(), Some(";")),
                ("cargo test".to_string(), None),
            ]
        );
    }

    #[test]
    fn split_multiple_separators_mixed() {
        let r = segs("a && b || c ; d");
        assert_eq!(
            r,
            vec![
                ("a".to_string(), Some("&&")),
                ("b".to_string(), Some("||")),
                ("c".to_string(), Some(";")),
                ("d".to_string(), None),
            ]
        );
    }

    #[test]
    fn split_pipes_are_not_separators() {
        // Pipes stay inside a single segment — passthrough is handled
        // by the user_wants_raw any-pipe check downstream.
        let r = segs("cargo test | tail -5");
        assert_eq!(r, vec![("cargo test | tail -5".to_string(), None)]);
    }

    #[test]
    fn split_pipe_inside_chain() {
        let r = segs("cargo build && cargo test | tail");
        assert_eq!(
            r,
            vec![
                ("cargo build".to_string(), Some("&&")),
                ("cargo test | tail".to_string(), None),
            ]
        );
    }

    #[test]
    fn split_respects_2_to_1_redirect() {
        // `2>&1` has a single `&` (not `&&`), and `&1` is part of a
        // redirect — must NOT trigger a chain split.
        let r = segs("cargo build 2>&1 && cargo test");
        assert_eq!(
            r,
            vec![
                ("cargo build 2>&1".to_string(), Some("&&")),
                ("cargo test".to_string(), None),
            ]
        );
    }

    #[test]
    fn split_respects_single_quotes() {
        // `&&` inside single quotes is literal data, NOT a separator.
        let r = segs("echo 'A && B' && echo C");
        assert_eq!(
            r,
            vec![
                ("echo 'A && B'".to_string(), Some("&&")),
                ("echo C".to_string(), None),
            ]
        );
    }

    #[test]
    fn split_respects_double_quotes() {
        let r = segs("echo \"A || B ; C\" && echo D");
        assert_eq!(
            r,
            vec![
                ("echo \"A || B ; C\"".to_string(), Some("&&")),
                ("echo D".to_string(), None),
            ]
        );
    }

    #[test]
    fn split_respects_escaped_separators() {
        // `\&\&` and `\;` should be passed through literally.
        let r = segs("echo a\\&\\&b && echo c");
        assert_eq!(
            r,
            vec![
                ("echo a\\&\\&b".to_string(), Some("&&")),
                ("echo c".to_string(), None),
            ]
        );
    }

    #[test]
    fn split_bails_on_heredoc() {
        assert!(split_chain_segments("cat <<EOF\nhello\nEOF").is_none());
    }

    #[test]
    fn split_bails_on_command_substitution() {
        assert!(split_chain_segments("echo $(date) && pwd").is_none());
        assert!(split_chain_segments("echo `date` && pwd").is_none());
    }

    #[test]
    fn split_bails_on_eval() {
        assert!(split_chain_segments("eval cmd && other").is_none());
    }

    #[test]
    fn split_bails_on_process_substitution() {
        assert!(split_chain_segments("diff <(cmd a) <(cmd b)").is_none());
    }

    #[test]
    fn split_bails_on_bash_control_flow() {
        assert!(split_chain_segments("for i in a b; do echo $i; done").is_none());
        assert!(split_chain_segments("if true; then x; fi").is_none());
        assert!(split_chain_segments("while true; do x; done").is_none());
        assert!(split_chain_segments("case $x in a) y;; esac").is_none());
    }

    #[test]
    fn split_subshell_groups_keep_chain_at_top_level() {
        // `(cmd_A) && cmd_B` — the `&&` is at top level (paren depth 0
        // after the closing paren) so it IS a separator. The subshell
        // contents stay inside their own segment.
        let r = segs("(cd /tmp && pwd) && echo done");
        assert_eq!(
            r,
            vec![
                ("(cd /tmp && pwd)".to_string(), Some("&&")),
                ("echo done".to_string(), None),
            ]
        );
    }

    #[test]
    fn split_bails_on_unbalanced_quotes() {
        assert!(split_chain_segments("echo \"unterminated && cmd").is_none());
        assert!(split_chain_segments("echo 'still open && cmd").is_none());
    }

    #[test]
    fn segment_must_not_wrap_shell_state_modifiers() {
        // These change shell state — wrapping puts them in a subshell
        // and their effect is lost.
        assert!(segment_must_not_be_wrapped("cd /tmp"));
        assert!(segment_must_not_be_wrapped("export FOO=bar"));
        assert!(segment_must_not_be_wrapped("source ~/.bashrc"));
        assert!(segment_must_not_be_wrapped(". config.sh"));
        assert!(segment_must_not_be_wrapped("set -e"));
        assert!(segment_must_not_be_wrapped("unset PATH"));
        assert!(segment_must_not_be_wrapped("pushd /tmp"));
        assert!(segment_must_not_be_wrapped("exit 0"));
    }

    #[test]
    fn segment_must_not_wrap_tokenade_invocations() {
        assert!(segment_must_not_be_wrapped("tokenade wrap 'x'"));
        assert!(segment_must_not_be_wrapped(
            "/usr/local/bin/tokenade raw cmd"
        ));
        assert!(segment_must_not_be_wrapped("tokenade query"));
    }

    #[test]
    fn segment_must_not_wrap_handles_env_prefix() {
        // Env-prefix segments: skip the assignment and check the real prog.
        assert!(segment_must_not_be_wrapped("LANG=C cd /tmp"));
        assert!(!segment_must_not_be_wrapped("LANG=C cargo build"));
    }

    #[test]
    fn first_program_token_handles_command_substitution_in_env() {
        // Naive whitespace split would fragment `CUTOFF=$(($(date +%s)-1))`
        // and return `+%s)` as the program. The quote/paren-aware tokenizer
        // sees one env-assignment token then advances to the real prog.
        assert_eq!(
            first_program_token("CUTOFF=$(($(date +%s) - 3600)); tail -n 50"),
            "tail"
        );
        assert_eq!(first_program_token("LANG=$(locale -a) ls"), "ls");
        assert_eq!(
            first_program_token("SCRATCH=$(mktemp -d) && cd $SCRATCH"),
            "cd"
        );
    }

    /// `cd <dir>;` is scene-setting, and the program after it is the command.
    ///
    /// MEASURED 2026-09-05 over this machine's transcripts: 45 results came
    /// back banded `[tokenade:cd]` at a 3.2 KB median — a label naming nothing
    /// the user ran — and not one was ever asked a question, while the agent
    /// went around them 12 times. The real program never reached its own
    /// compactor.
    #[test]
    fn a_leading_cd_clause_is_not_the_program() {
        for (cmd, want) in [
            ("cd /repo; python3 - <<'PY'", "python3"),
            ("cd /repo && cargo test", "cargo"),
            ("cd /repo\ngrep -rn foo .", "grep"),
            ("  cd '/path with spaces' ; ls -l", "ls"),
            ("cd \"$(git rev-parse --show-toplevel)\" && make", "make"),
            ("cd a && cd b && cd c; rg pat", "rg"),
            ("cd /repo; TOKENADE=1 python3 x.py", "python3"),
            // The interactive-exclude list has to see the real program, or a
            // TTY command behind a `cd` gets wrapped and hangs the session.
            ("cd /repo && vim notes.txt", "vim"),
        ] {
            assert_eq!(first_program_token(cmd), want, "cmd: {cmd:?}");
        }
        // A bare `cd` really IS the command, and stays it.
        assert_eq!(first_program_token("cd /tmp"), "cd");
        assert_eq!(first_program_token("cd"), "cd");
        // Not a clause boundary: a pipe changes the sentence, and `cdrom` is
        // not `cd`.
        assert_eq!(first_program_token("cd /tmp | tee log"), "cd");
        assert_eq!(first_program_token("cdrom --eject"), "cdrom");
        // A `;` inside quotes is not a separator.
        assert_eq!(first_program_token("cd 'a;b' ; wc -l"), "wc");
    }

    #[test]
    fn first_program_token_basics() {
        assert_eq!(first_program_token("cargo build"), "cargo");
        assert_eq!(first_program_token("  git status  "), "git");
        assert_eq!(first_program_token("/usr/bin/python3 -V"), "python3");
        assert_eq!(first_program_token("!cargo"), "cargo");
        assert_eq!(first_program_token(""), "");
        assert_eq!(first_program_token("FOO=bar BAZ=qux"), ""); // pure env, no prog
        assert_eq!(first_program_token("FOO=bar make"), "make");
        // A newline ends the token like any other separator. Unpinned until
        // now: every case above ends the first token on a SPACE, so dropping
        // `\n` from the separator set left the whole crate's 7 519 tests
        // green. It only shows when the first line is a bare program with no
        // arguments — the shape an agent writes as `cd`-then-work — where the
        // token would otherwise run into the next line and name no program
        // that exists.
        assert_eq!(first_program_token("pwd\nls -la"), "pwd");
        assert_eq!(first_program_token("make\ncargo test"), "make");
        assert_eq!(first_program_token("/usr/bin/env\nls"), "env");
    }

    #[test]
    fn first_program_token_respects_quotes() {
        // The `;` inside a single-quoted string isn't a separator.
        assert_eq!(first_program_token("echo 'a; b' && ls"), "echo");
        assert_eq!(first_program_token("echo \"a; b\" && ls"), "echo");
    }

    #[test]
    fn segment_should_wrap_normal_commands() {
        assert!(!segment_must_not_be_wrapped("cargo build"));
        assert!(!segment_must_not_be_wrapped("git status"));
        assert!(!segment_must_not_be_wrapped("npm test"));
        assert!(!segment_must_not_be_wrapped("kubectl get pods"));
    }

    #[test]
    fn split_bails_on_background_single_amp() {
        // `cmd1 & cmd2` — background job; wrapping changes semantics.
        assert!(split_chain_segments("sleep 5 & echo done").is_none());
        assert!(split_chain_segments("cmd_a & cmd_b && cmd_c").is_none());
    }

    #[test]
    fn split_keeps_fd_redirect_amp_as_part_of_segment() {
        // `2>&1`, `>&1`, `1>&2` — the `&` follows `>` and is part of
        // an fd-dup redirect, NOT a background-job operator.
        let r = segs("cargo build 2>&1 && echo ok");
        assert_eq!(
            r,
            vec![
                ("cargo build 2>&1".to_string(), Some("&&")),
                ("echo ok".to_string(), None),
            ]
        );
        let r = segs("cmd >&2 ; other");
        assert_eq!(
            r,
            vec![
                ("cmd >&2".to_string(), Some(";")),
                ("other".to_string(), None),
            ]
        );
    }

    #[test]
    fn split_respects_brace_groups() {
        // `{ a ; b ; }` — bash brace group; the inner `;` MUST NOT
        // split the chain.
        let r = segs("{ a ; b ; } && c");
        assert_eq!(
            r,
            vec![
                ("{ a ; b ; }".to_string(), Some("&&")),
                ("c".to_string(), None),
            ]
        );
    }

    #[test]
    fn split_respects_parameter_expansion_braces() {
        // `${VAR:-default}` contains `:` and could contain other
        // shell-meta chars; brace tracking keeps it as one segment.
        let r = segs("echo ${VAR:-x;y} && echo ok");
        assert_eq!(
            r,
            vec![
                ("echo ${VAR:-x;y}".to_string(), Some("&&")),
                ("echo ok".to_string(), None),
            ]
        );
    }

    #[test]
    fn split_bails_on_unbalanced_brace() {
        assert!(split_chain_segments("{ a ; b").is_none());
        assert!(split_chain_segments("a ; b }").is_none());
    }

    #[test]
    fn time_is_wrappable_now() {
        // `time` should NOT be in the no-wrap list — wrapping
        // `time cargo build` lets the cargo compactor run on the inner
        // output. The old behaviour skipped it entirely.
        assert!(!segment_must_not_be_wrapped("time cargo build"));
    }

    #[test]
    fn let_is_unwrappable() {
        // `let x=1 && cmd` — `let` is a bash arithmetic builtin that
        // assigns in the current shell; wrapping it loses the var.
        assert!(segment_must_not_be_wrapped("let x=1"));
    }

    /// A shell keyword inside a string is a WORD, not a keyword.
    ///
    /// `has_bash_control_flow` split the RAW command on whitespace, so the
    /// `for` in `echo "=== rows iterated for the totals ==="` declared the
    /// whole chain unsplittable. The chain was then routed and labelled by its
    /// leading `cd` — a command that produces no output at all. Measured over
    /// 108 974 real agent commands: 10 515 (9.6 %) were called control flow on
    /// a quoted keyword alone, and 4 363 of those are chains whose head is a
    /// state modifier or a label — 4 281 of them `cd`. This machine's ledger
    /// holds two rows booked under `cd`, and the "write a compactor for `cd`"
    /// nudge fired 92 times.
    #[test]
    fn a_keyword_inside_a_string_does_not_stop_a_chain_being_split() {
        let real = "cd /r; sed -n '1060,1080p' a.rs; \
                    echo \"=== where rows are iterated for the totals ===\"; \
                    sed -n '1290,1345p' a.rs";
        assert_eq!(meaningful_segment(real), "sed -n '1060,1080p' a.rs");
        // Every opener, quoted, on a chain that must still split.
        for kw in SHELL_OPENERS {
            let cmd = format!("cd /r && echo \"a {kw} b\" && grep -rn x a.rs");
            assert_eq!(
                meaningful_segment(&cmd),
                "grep -rn x a.rs",
                "a quoted `{kw}` must not freeze the chain"
            );
        }
        // …and a REAL opener still stops it: `for i in a; do x; done` must not
        // be cut at its `;`.
        let loop_cmd = "for i in a b; do echo $i; done";
        assert_eq!(meaningful_segment(loop_cmd), loop_cmd);
        // A quoted keyword in SINGLE quotes too — the mask must read both,
        // and a mutation that dropped `'` from it passed while only double
        // quotes were tested.
        assert_eq!(
            meaningful_segment("cd /r && echo 'a for b' && grep -rn x a.rs"),
            "grep -rn x a.rs"
        );
        // An unterminated quote is not reasoned about at all. The `;` INSIDE
        // it is not a separator, so a scan that missed the open quote would
        // hand back `grep x a.rs` — a segment the shell never runs on its own.
        let broken = "cd /r; echo \"oops; grep x a.rs";
        assert_eq!(meaningful_segment(broken), broken);
        // Asserted on the predicate itself, not only through
        // `meaningful_segment`: the chain splitter has its own quote scanner
        // and bails on the same input, so going through it cannot tell
        // whether THIS one still refuses. A caller that arrives without that
        // second scanner would get a chain split at a `;` inside a string.
        assert!(
            has_bash_control_flow(broken),
            "an unterminated quote must be refused, not reasoned about"
        );
        assert!(!has_bash_control_flow("echo \"a for b\"; grep x a.rs"));
        assert!(has_bash_control_flow("for i in a b; do echo $i; done"));
    }

    /// `cd … ; tokenade wrap '<cmd>'` hands back the tokenade segment.
    ///
    /// The routing note claimed this shape "resolves to the tokenade token and
    /// is caught by the already-wrapped guard". It did not: every segment was
    /// skipped — `cd` as a state modifier, the self-call as unwrappable — and
    /// the fallback returned the whole command, whose `argv[0]` is `cd`. The
    /// ledger row proving it is dated 2026-09-01.
    #[test]
    fn a_chain_of_only_skipped_segments_hands_back_the_self_call() {
        let cmd = "cd /r ; /home/u/.cargo/bin/tokenade wrap 'sed -n 1,80p a.rs'";
        assert_eq!(
            meaningful_segment(cmd),
            "/home/u/.cargo/bin/tokenade wrap 'sed -n 1,80p a.rs'"
        );
        // With nothing to hand back, the whole command is still the answer —
        // a chain of pure labels routes as itself.
        assert_eq!(meaningful_segment("echo a; echo b"), "echo a; echo b");
    }

    #[test]
    fn meaningful_segment_skips_state_modifiers() {
        // The dashboard bug: `cd … && realcmd` was labelled+routed as `cd`.
        // The label an agent puts in front of the command it cares
        // about. `cd` was skipped here from the start and `echo` never was,
        // which routed the whole chain as an `echo` and folded 15 % worse.
        assert_eq!(meaningful_segment("echo === ; cargo test"), "cargo test");
        assert_eq!(
            meaningful_segment("echo \"=== build\" && cargo build"),
            "cargo build"
        );
        assert_eq!(
            meaningful_segment("sleep 2; kubectl get pods"),
            "kubectl get pods"
        );
        assert_eq!(meaningful_segment("printf x ; docker ps"), "docker ps");
        assert_eq!(
            meaningful_segment("cd /r && echo go && cargo test"),
            "cargo test"
        );
        // A chain made only of labels still routes as itself: the fallback is
        // the whole command, never nothing.
        assert_eq!(meaningful_segment("echo a; echo b"), "echo a; echo b");
        assert_eq!(meaningful_segment("echo hello"), "echo hello");
        // A longer name that merely STARTS with one of them is not a label.
        assert_eq!(meaningful_segment("echoes x; ls"), "echoes x");
        assert_eq!(meaningful_segment("cd /tmp && grep x f"), "grep x f");
        assert_eq!(meaningful_segment("cd a && cd b && cmake .."), "cmake ..");
        assert_eq!(meaningful_segment("export FOO=1 ; pytest -q"), "pytest -q");
        // A NEWLINE separates two commands exactly as `;` does, and that shape
        // was pinned nowhere — every case above uses `&&` or `;`. It is also
        // the shape agents actually type, and the one that regressed: the
        // savings ledger on this machine carries 122 rows attributed to `cd`,
        // which `tokenade gain` reports as its 9th most frequent operation,
        // crediting navigation for another command's saving. `cd` produces no
        // output to save. The behaviour is right in this tree; nothing held it
        // there, which is how it broke before (see the `cd`-on-its-own-line
        // fix) and how it would break again.
        assert_eq!(
            meaningful_segment("cd /home/u/proj\nls -la crates/core/src"),
            "ls -la crates/core/src"
        );
        assert_eq!(meaningful_segment("cd /home/u/proj\nls -la ."), "ls -la .");
        assert_eq!(
            meaningful_segment("cd /r\ncargo test -p core"),
            "cargo test -p core"
        );
        // Mixed separators, and a label after the newline: both are skipped.
        assert_eq!(
            meaningful_segment("cd /r\necho '=== step 2'\nmake build"),
            "make build"
        );
        // The fallback holds on this separator too: labels only → itself.
        assert_eq!(meaningful_segment("cd /r\ncd /s"), "cd /r\ncd /s");
        // A real command first → keep it (don't jump to a trailing `echo done`).
        assert_eq!(
            meaningful_segment("grep x big.log && echo done"),
            "grep x big.log"
        );
        // Single command → unchanged.
        assert_eq!(meaningful_segment("cargo build"), "cargo build");
        // Pipelines stay whole (one segment); rightmost owns the output.
        assert_eq!(
            meaningful_segment("cat f | grep x | tail"),
            "cat f | grep x | tail"
        );
        // Unsplittable (heredoc / background-& / control-flow) → whole command.
        assert_eq!(meaningful_segment("sleep 1 & echo hi"), "sleep 1 & echo hi");
        assert_eq!(
            meaningful_segment("for i in a b; do echo $i; done"),
            "for i in a b; do echo $i; done"
        );
        // All-state-modifier chain → falls back to the whole command.
        assert_eq!(
            meaningful_segment("cd /tmp && export X=1"),
            "cd /tmp && export X=1"
        );
    }

    #[test]
    fn split_chain_never_panics_on_pathological_input() {
        // Smoke test: a grab-bag of weird inputs must never panic.
        for cmd in [
            "", " && ", "&&", "||", ";", ";;;", "&&&&", "''", "\"\"", "\"'\"'", "echo \\", "(",
            ")", "(()()", "$$", "$1 && $2",
        ] {
            let _ = split_chain_segments(cmd);
        }
    }
}

#[cfg(test)]
mod newline_is_a_separator {
    use super::*;

    #[test]
    fn a_cd_on_its_own_line_routes_to_the_next_command() {
        assert_eq!(meaningful_segment("cd /tmp\nls -la"), "ls -la");
        assert_eq!(meaningful_segment("cd /tmp\ngrep -rn x ."), "grep -rn x .");
        assert_eq!(
            meaningful_segment("cd /a\nexport K=v\ngit status"),
            "git status"
        );
        // Blank lines between are not commands.
        assert_eq!(meaningful_segment("cd /tmp\n\n\nls -la"), "ls -la");
    }

    #[test]
    fn it_agrees_with_the_separators_that_already_worked() {
        for cmd in ["cd /tmp && ls -la", "cd /tmp; ls -la", "cd /tmp\nls -la"] {
            assert_eq!(meaningful_segment(cmd), "ls -la", "on: {cmd:?}");
        }
    }

    /// Every multi-line shape that must NOT be split still refuses, and each
    /// refuses through a DIFFERENT guard — that is what makes the newline arm
    /// safe rather than lucky.
    #[test]
    fn the_multi_line_shapes_that_must_not_split_still_refuse() {
        // heredoc → has_shell_unsafe
        assert!(split_chain_segments("python3 - <<'PY'\nprint(1)\nPY").is_none());
        // command substitution → has_shell_unsafe
        assert!(split_chain_segments("X=$(date)\necho $X").is_none());
        // bash control flow → has_bash_control_flow
        assert!(split_chain_segments("for f in a b\ndo\necho $f\ndone").is_none());
        // backslash line continuation → the dedicated arm
        assert!(split_chain_segments("grep pat \\\n file").is_none());
        // a newline INSIDE quotes is data, not a separator
        let segs = split_chain_segments("echo 'one\ntwo'").expect("splittable");
        assert_eq!(segs.len(), 1, "a quoted newline must not cut: {segs:?}");
    }

    #[test]
    fn the_separator_it_records_is_the_newline() {
        let segs = split_chain_segments("cd /tmp\nls").expect("splittable");
        assert_eq!(segs[0].separator_after, Some("\n"));
        assert_eq!(segs[0].cmd, "cd /tmp");
        assert_eq!(segs[1].cmd, "ls");
    }
}

#[cfg(test)]
mod plausible_program_tests {
    use super::*;

    #[test]
    fn a_program_name_looks_like_one() {
        for ok in [
            "git",
            "7z",
            "2to3",
            "./script.sh",
            "/usr/bin/env",
            "_build",
            "g++",
            "c++filt",
        ] {
            assert!(is_plausible_program_name(ok), "{ok} rejected");
        }
    }

    #[test]
    fn shell_syntax_and_fragments_are_not_program_names() {
        for bad in [
            "{", "}", "[", "done;", "clean(){", "n=$(grep", "$B", "-t", "X11;", "",
        ] {
            assert!(!is_plausible_program_name(bad), "{bad:?} accepted");
        }
    }

    #[test]
    fn a_dotted_directory_is_not_a_program_but_a_dot_slash_path_is() {
        // `.kiro` reached the program position from a chain we could not
        // split; `./run` is a real program.
        assert!(!is_plausible_program_name(".kiro"));
        assert!(is_plausible_program_name("./run"));
    }

    #[test]
    fn the_control_flow_vocabulary_is_never_a_program() {
        for kw in SHELL_OPENERS.iter().chain(SHELL_TERMINATORS.iter()) {
            assert!(!is_plausible_program_name(kw), "{kw} accepted");
        }
        // …but the split-safety question must still look at OPENERS only:
        // `echo done` is an ordinary command.
        assert!(!has_bash_control_flow("echo done"));
        assert!(has_bash_control_flow("for f in a; do echo $f; done"));
    }
}

#[cfg(test)]
mod one_home_for_the_separator_set {
    use super::{chain_shape, split_chain_segments, Chain};

    /// Every separator the SCANNER knows, asked through the declarer. There is
    /// no list to keep in step: this drives the scanner itself.
    #[test]
    fn each_separator_the_scanner_knows_yields_many() {
        for sep in [";", "&&", "||", "\n"] {
            let cmd = format!("echo one{sep}echo two");
            assert_eq!(chain_shape(&cmd), Chain::Many, "{sep:?} in {cmd:?}");
        }
    }

    /// A bare pipe joins ONE pipeline; it does not end a command.
    #[test]
    fn a_pipe_is_not_a_separator() {
        assert_eq!(chain_shape("grep foo file | wc -l"), Chain::One);
    }

    #[test]
    fn a_lone_command_is_one() {
        assert_eq!(chain_shape("cd /tmp"), Chain::One);
        assert_eq!(chain_shape("grep -rn needle ."), Chain::One);
    }

    /// A scanner refusal is its OWN answer, not `Many` and not `One`. The peel
    /// accepts it; the exclusion gate defers on it. Collapsing the two would
    /// break one caller or the other.
    #[test]
    fn a_refusal_is_named_rather_than_guessed() {
        let control_flow = "cd /tmp; for i in a b; do echo $i; done";
        assert!(split_chain_segments(control_flow).is_none());
        assert_eq!(chain_shape(control_flow), Chain::Unparsed);
        assert_eq!(chain_shape("grep $(cat pat) file"), Chain::Unparsed);
    }
}

/// Shapes taken from this machine's transcripts (2026-10-05): 5 472 of
/// 128 451 real commands were named by a launcher, a keyword or a script's
/// body line. Each case is one of those shapes, reduced.
#[cfg(test)]
mod the_command_that_does_the_work {
    use super::super::command_rewrite::meaningful_segment;

    fn name(cmd: &str) -> String {
        let seg = meaningful_segment(cmd);
        let argv: Vec<String> = seg.split_whitespace().map(String::from).collect();
        crate::filter::program_basename(&argv)
    }

    /// Routing keeps `bash -c 'make' | tail -5` whole — `make`'s compactor
    /// must not read what `tail` cut — but the program that RAN, for the
    /// ledger and the dashboard, is `make`.
    #[test]
    fn a_piped_shell_launcher_is_named_by_its_script() {
        use super::super::command_rewrite::program_name;
        assert_eq!(name("bash -c 'make' | tail -5"), "bash");
        assert_eq!(program_name("bash -c 'make' | tail -5"), "make");
        assert_eq!(program_name("bash -c 'source scripts/env.sh && win hostname' 2>&1 | head -6"), "win");
        assert_eq!(program_name("cd /r && bash -c 'cargo test -q' 2>&1 | tail -3"), "cargo");
        assert_eq!(program_name("bash <<'EOF'\nls\nEOF"), "bash");
    }

    /// A subshell's `(`, alone or glued to its first word, opens a command;
    /// it is not one.
    #[test]
    fn a_subshell_is_named_by_what_it_runs() {
        assert_eq!(name("for h in a b; do\n( yt-dlp --print x \"$h\" > \"$h.txt\" ) &\ndone; wait"), "yt-dlp");
        assert_eq!(name("( make -j4 2>&1 | tail -3 )"), "make");
        assert_eq!(name("(cargo build -q && echo ok)"), "cargo");
    }

    /// Three shapes ROUTED whole on purpose, NAMED by the program that ran:
    /// a double-quoted launcher script, `time`, a leading line continuation.
    #[test]
    fn what_routing_keeps_whole_is_still_named_by_its_program() {
        use super::super::command_rewrite::program_name;
        assert_eq!(name("bash -c \"cd $D && make\""), "bash");
        assert_eq!(program_name("bash -c \"cd $D && make -j4\""), "make");
        assert_eq!(program_name("time cargo build -q 2>&1 | tail -3"), "cargo");
        assert_eq!(program_name("time -p ./run.sh"), "run.sh");
        assert_eq!(program_name("\\\nsed -i 's#a#b#g' x.mdx"), "sed");
        assert_eq!(program_name("\\ls -la"), "ls");
        assert_eq!(program_name("time"), "time");
    }

    /// PowerShell launched from the shell runs the cmdlet: it routes and is
    /// named by it. A Windows path keeps its backslashes; a `$` bash would
    /// expand first keeps the ROUTE whole, not the name.
    #[test]
    fn powershell_hands_over_to_its_cmdlet() {
        use super::super::command_rewrite::program_name;
        assert_eq!(name("powershell -NoProfile -Command \"Get-ChildItem C:\\Windows\\System32 -Filter *.dll | Select-Object -First 400\""), "Get-ChildItem");
        assert_eq!(name("pwsh -NoLogo -c 'Get-Process'"), "Get-Process");
        assert_eq!(name("powershell.exe -ExecutionPolicy Bypass -File .\\build.ps1"), "build");
        assert_eq!(name("powershell -Command Get-Service"), "Get-Service");
        assert_eq!(name("powershell -Command \"Get-Process | Where-Object { $_.CPU -gt 10 }\""), "powershell");
        assert_eq!(program_name("powershell -Command \"Get-Process | Where-Object { $_.CPU -gt 10 }\""), "Get-Process");
        assert_eq!(name("powershell -Unknown x"), "powershell");
        assert_eq!(name("cmd /c dir C:\\Windows\\System32\\*.exe"), "dir");
        assert_eq!(name("cmd.exe /C where git"), "where");
    }

    /// `pwd` and `date` print one line about the session; the command after
    /// them does the work. Alone, they keep their name.
    #[test]
    fn a_session_info_line_is_a_label() {
        use super::super::command_rewrite::program_name;
        assert_eq!(program_name("cd /r && pwd && find crates -name '*.rs' | head"), "find");
        assert_eq!(program_name("date; ls -la ~/.tokenade/ | head -20"), "ls");
        assert_eq!(program_name("pwd"), "pwd");
        assert_eq!(program_name("date +%s"), "date");
    }

    #[test]
    fn a_heredoc_body_is_never_the_program() {
        let cmd = "cd /repo\npython3 - <<'PY'\n# Clean the now-stale comment.\np='a.toml'; s=open(p).read()\nopen(p,\"w\").write(s)\nPY";
        assert_eq!(name(cmd), "python3");
    }

    #[test]
    fn a_script_s_lines_are_read_one_command_at_a_time() {
        assert_eq!(name("cd /repo\necho \"=== notes ===\"\n# count\ngit log --oneline -5"), "git");
        assert_eq!(name("cd /repo\np(){ sed -n \"$1p\" a.rs; }\nif [ -f a.rs ]; then\n  wc -l a.rs\nfi"), "wc");
    }

    #[test]
    fn a_shell_launcher_hands_over_to_its_command() {
        assert_eq!(name("bash -c 'git log --oneline -3'"), "git");
        assert_eq!(name("sh -lc \"cargo test -q\" 2>&1"), "cargo");
        assert_eq!(name("bash -o pipefail -c 'grep -rn x src'"), "grep");
        assert_eq!(name("bash scripts/build.sh --all"), "build.sh");
        // Left whole: stdin programs, an output pipe, a quote that can't be sliced.
        assert_eq!(name("bash <<'EOF'\nls\nEOF"), "bash");
        assert_eq!(name("bash -c 'make' | tail -5"), "bash");
        assert_eq!(name("bash -c \"cd $D && make\""), "bash");
        assert_eq!(name("bash -c 'echo '\\''x'\\'''"), "echo"); // `'\''` inside the script is a quote, not its end.
    }

    #[test]
    fn a_group_and_loop_control_are_not_commands() {
        // `pgrep -f` is never the routed segment and `echo` is a label: the group speaks.
        assert_eq!(name("pgrep -f build >/dev/null && echo busy || { grep -c ok log; tail -2 log; }"), "grep");
        assert_eq!(name("{ grep -rn x src | head; echo; sed -n '1,5p' a.rs; }"), "grep");
        assert_eq!(name("for i in 1 2 3; do [ -f done.txt ] && break; sleep 5; done; tail -2 run.log"), "tail");
        assert_eq!(name("while read l; do [ -z \"$l\" ] && continue; wc -c $l; done < files"), "wc");
    }

    #[test]
    fn a_program_in_a_variable_is_named_by_its_value() {
        use super::super::command_rewrite::program_name;
        assert_eq!(program_name("cd /r; B=target/release/acme-cli; $B draft list | head -5"), "acme-cli");
        assert_eq!(program_name("PY=\"/opt/venv/bin/python3\" && $PY run.py"), "python3");
        assert_eq!(program_name("B=./a; B=./bin/tool; ${B} --help"), "tool");
        // Nothing to read: a substitution, an unassigned variable.
        assert_eq!(program_name("B=$(which tool); $B x"), "$B");
        assert_eq!(program_name("$EDITOR notes.md"), "$EDITOR");
    }

    #[test]
    fn a_launcher_behind_a_wrapper_still_hands_over() {
        assert_eq!(name("timeout 600 bash -c 'cargo test -q'"), "cargo");
        assert_eq!(name("setsid nohup sh -c 'git log --oneline -5' > /dev/null 2>&1"), "git");
        assert_eq!(name("timeout -k 5 120 env RUST_LOG=1 bash scripts/check.sh --all"), "check.sh");
        assert_eq!(name("nice -n 10 bash -lc 'grep -rn x src'"), "grep");
        assert_eq!(name("MY_TOOL_BIN=target/release/x bash tools/run.sh 2>&1"), "run.sh");
        assert_eq!(name("nohup bash -c 'cargo build -j2' > /tmp/b.log 2>&1 &"), "cargo");
        // A wrapper alone is untouched.
        assert_eq!(name("timeout 30 cargo test"), "cargo");
    }

    #[test]
    fn a_quoted_script_s_own_quotes_do_not_end_it() {
        assert_eq!(name("bash -c '\ncd /r\ngrep -E '\\''^x'\\'' a.log\n'"), "grep");
    }

    #[test]
    fn job_control_is_not_the_command() {
        assert_eq!(name("npm run dev > log 2>&1 & disown; sleep 2; curl -s localhost:3000"), "npm");
        assert_eq!(name("disown; tail -5 server.log"), "tail");
        assert_eq!(name("wait; cat out.txt"), "cat");
        // One list for "prints nothing": a `mkdir` before the work is not it.
        assert_eq!(name("mkdir -p out && cargo build --release"), "cargo");
    }

    #[test]
    fn a_test_guards_the_command_it_is_not() {
        assert_eq!(name("[ -f /etc/hosts ] && cat /etc/hosts"), "cat");
        assert_eq!(name("test -d out && ls out"), "ls");
        assert_eq!(name("[ -f x ]"), "[");
    }
}

#[cfg(test)]
mod a_self_call_script {
    use crate::command_rewrite::program_name;

    /// `cd dir⏎tokenade exec … <<'PYEOF'` with a Python body: every top-level
    /// command is skipped (`cd`, the self-call), and the name came out of the
    /// heredoc body — `for`, 9 real commands of one session (2026-10-05).
    #[test]
    fn a_heredoc_body_never_names_the_command() {
        let cmd = "cd /srv/site/stats\ntokenade exec --lang python --script-file /dev/stdin <<'PYEOF'\nf = \"a.mdx\"\nfor old, new in subs:\n    c = c.replace(old, new)\nPYEOF";
        assert_eq!(program_name(cmd), "tokenade");
        assert_eq!(program_name("cd /srv; tokenade wrap 'cargo test'"), "tokenade");
    }
}

#[cfg(test)]
mod a_shell_function {
    use crate::command_rewrite::program_name;

    /// A definition is not a command; a call is named by what the body runs.
    #[test]
    fn a_function_is_named_by_its_body() {
        assert_eq!(program_name("cd src && p(){ echo \"== $1:$2\"; sed -n \"$2p\" $1; }; p a.ts 7; p b.ts 9"), "sed");
        assert_eq!(program_name("cd ~ && az() { docker run --rm -v \"$HOME/.azure:/root/.azure\" img az \"$@\"; }\necho \"=== org ===\"; az rest --method GET"), "docker");
        assert_eq!(program_name("function q { curl -s \"$1\"; }\nq https://x.test/a"), "curl");
        // Not the shell's own: left alone.
        assert_eq!(program_name("node -e \"function f() { return 1 }; f()\""), "node");
        assert_eq!(program_name("echo 'x run() {' && make && echo '}'"), "make");
        assert_eq!(program_name("p(){ sed -n 1p $1; }; cat x"), "cat");
    }

    /// Only the program word of a variable's value must be literal.
    #[test]
    fn a_variable_with_arguments_is_named_by_its_program() {
        assert_eq!(program_name("S=\"ssh -i $HOME/.k/id -o ConnectTimeout=20 u@h\"; $S \"docker ps\""), "ssh");
        assert_eq!(program_name("B=\"$HOME/bin/tool --x\"; $B run"), "$B");
        // Set outside what `time` runs.
        assert_eq!(program_name("cd /r; B=./target/release/acme-cli; time $B draft publish --id 1"), "acme-cli");
    }
}
