//! Tests for the `bash_pretool` hook.

use super::*;

#[test]
fn is_continuous_stream_matches_real_streamers() {
    // Genuine follow modes — bash hook must skip these.
    assert!(is_continuous_stream("tail -f /var/log/syslog"));
    assert!(is_continuous_stream("tail -F /var/log/app.log"));
    assert!(is_continuous_stream("tail --follow /var/log/x"));
    assert!(is_continuous_stream("journalctl -u nginx -f"));
    assert!(is_continuous_stream("journalctl --follow"));
    assert!(is_continuous_stream("docker logs -f mycontainer"));
    assert!(is_continuous_stream("kubectl logs -f pod/x"));
    assert!(is_continuous_stream("podman logs --follow box"));
    assert!(is_continuous_stream("oc logs -f deploy/y"));
}

#[test]
fn is_continuous_stream_rejects_unrelated_dash_f() {
    // `-f` / `-F` exist on many tools with totally different meanings.
    // Skipping these from intercept would lose major compactor wins.
    assert!(!is_continuous_stream("cargo build -F some_feature"));
    assert!(!is_continuous_stream("cargo build --features foo"));
    assert!(!is_continuous_stream("grep -F 'literal string' file"));
    assert!(!is_continuous_stream("grep -rF foo src/"));
    assert!(!is_continuous_stream("pkill -f some-process"));
    assert!(!is_continuous_stream("pgrep -f some-pattern"));
    assert!(!is_continuous_stream("sshpass -f /tmp/pw ssh host"));
    // Plain tail / journalctl WITHOUT follow → not streaming.
    assert!(!is_continuous_stream("tail -n 50 /var/log/x"));
    assert!(!is_continuous_stream("journalctl -u nginx -n 100"));
    // docker / kubectl WITHOUT `logs` subcommand → not streaming.
    assert!(!is_continuous_stream("docker ps -f status=running"));
    assert!(!is_continuous_stream("kubectl get pods -f manifest.yaml"));
}

// #690 — is_bash_intercept_excluded gates whether a Bash command
// gets wrapped by the PreToolUse hook. Wrong answer in either
// direction is bad: false-positive (excluding a batch command)
// loses the compaction win; false-negative (failing to exclude
// an interactive TUI) hangs the agent waiting for a TTY. Zero
// prior coverage. These tests pin the contract for every
// category in the match arm.
#[test]
fn bash_exclude_list_covers_interactive_tuis() {
    for prog in [
        // editors
        "vim", "vi", "nano", "emacs", "ed", // pagers
        "less", "more", "most", "man", // process viewers
        "top", "htop", "btop", "atop", "iotop", "iftop", "nvtop",
        // periodic refresh / multiplexers
        "watch", "tmux", "screen", "byobu", // file managers
        "ranger", "nnn", "lf", "broot",
        // privilege escalation (interactive password prompt)
        "sudo", "su", "doas", // remote shells (interactive)
        "ssh", "mosh", "telnet", "ftp", "sftp", "rlogin", // raw sockets
        "nc", "ncat", "socat", // infinite loop generator
        "yes",   // #855 — binary-transfer + secret-bearing tools
        "scp", "rsync", "sshpass",
    ] {
        assert!(
            super::is_bash_intercept_excluded(prog),
            "{prog} must be excluded — wrapping it hangs or loses interactivity"
        );
    }
}

// #855 — Lock-in for the deliberate non-additions. The
// compression/archive family was considered for #855 but rejected
// because they have legitimate textual modes (list, decompress to
// stdout) that the agent still wants compacted. If anyone ever
// adds these to is_bash_intercept_excluded, this test catches it
// before the regression ships.
/// A launcher prefix must not hide an interactive program.
///
/// The gate read the first meaningful word, so `timeout 60 vim f` resolved to
/// `timeout`, missed the list, and was wrapped. Verified before the fix:
/// `tokenade wrap 'timeout 60 vim f'` had to be killed by an outer timeout.
///
/// The spellings are written out rather than generated from the wrapper list
/// the resolver consults — a test that asks a predicate about the very list it
/// is built from stays green when that list is emptied.
#[test]
fn a_launcher_prefix_does_not_hide_an_interactive_program() {
    for cmd in [
        "timeout 60 vim /etc/hosts",
        "timeout -k 5 120 vim /etc/hosts",
        "nohup less big.log",
        "env EDITOR=x vim notes.txt",
        "command less big.log",
        "setsid top",
        "nice -n 5 htop",
        "sudo -u root vim /etc/hosts",
        "timeout 25 ssh host ls",
        "timeout 30 sshpass -p secret ssh host uptime",
        // The typed spelling is load-bearing, not belt-and-braces:
        // `program_basename` reads `#` as the program on a comment-led
        // command, so the peel alone would wrap these and hang.
        "# open the file\nvim notes.txt",
        "#!/bin/sh\nless big.log",
    ] {
        assert!(
            super::bash_command_is_excluded_by_list(cmd),
            "{cmd} must be excluded — wrapping it hangs the agent's shell"
        );
    }
}

/// The SEGMENT gate is a second caller, and the chain path is the one that
/// reaches it: `cd /tmp; vim f` was already passed through while
/// `cd /tmp; timeout 60 vim f` had its `vim` segment wrapped. A guard that
/// only went through `bash_command_is_excluded_by_list` would not have seen
/// that, because each site had grown its own resolver.
#[test]
fn the_segment_gate_peels_a_launcher_too() {
    for seg in [
        "timeout 60 vim f",
        "nohup less big.log",
        "env EDITOR=x vim notes.txt",
        "nice -n 5 htop",
    ] {
        assert!(
            super::bash_segment_excluded_from_wrap(seg),
            "segment {seg} must not be wrapped"
        );
    }
    for seg in ["timeout 60 cargo test", "env FOO=1 git log", "cargo build"] {
        assert!(
            !super::bash_segment_excluded_from_wrap(seg),
            "segment {seg} lost its compactor to the peel"
        );
    }
}

/// The other direction, and the one that pays for the change: peeling must not
/// turn a compactable command into a passthrough. Every one of these has a
/// launcher in front of a program with a real compactor behind it.
#[test]
fn peeling_a_launcher_does_not_disable_compaction() {
    for cmd in [
        "timeout 60 cargo test",
        "timeout -k 5 120 cargo test",
        "nohup cargo build",
        "env FOO=1 git log --oneline",
        "nice -n 10 make -j8",
        "cargo test",
        "git status",
        "grep -rn pattern crates",
    ] {
        assert!(
            !super::bash_command_is_excluded_by_list(cmd),
            "{cmd} must stay wrappable — the peel cost it its compactor"
        );
    }
}

#[test]
fn bash_exclude_list_keeps_compression_tools_wrappable() {
    for prog in ["tar", "gzip", "bzip2", "xz", "zstd", "cpio", "dd"] {
        assert!(
            !super::is_bash_intercept_excluded(prog),
            "{prog} must NOT be excluded — `tar -tzf` / `xzcat` etc. are textual + compactable"
        );
    }
}

// P2a — interactive-TTY gate. Container/k8s commands that request a
// TTY (-i AND -t, or attach) must bypass wrapping: piped stdio either
// hangs them (attach) or makes them fail with "input device is not a
// TTY". A false positive here only costs a missed compaction, never
// a hang, so the gate errs toward matching.
#[test]
fn interactive_tty_gate_matches_attach_and_it_forms() {
    for cmd in [
        "docker exec -it api bash",
        "docker exec -ti api sh",
        "docker exec -i -t api bash",
        "docker exec --interactive --tty api bash",
        "docker run -it --rm ubuntu bash",
        "docker start -ai api",
        "docker attach api",
        "podman exec -it api bash",
        "nerdctl run -it img sh",
        "kubectl exec -it pod-0 -- bash",
        "kubectl debug -it node/n1 --image=busybox",
        "kubectl attach pod-0",
        "oc exec -it pod-0 -- sh",
        "k exec -it pod-0 -- bash",
    ] {
        assert!(
            super::is_interactive_tty_invocation(cmd),
            "{cmd} requests a TTY — must pass through raw"
        );
    }
}

#[test]
fn interactive_tty_gate_keeps_batch_invocations_wrappable() {
    for cmd in [
        // no TTY requested → textual output, compactable
        "docker exec api ls -la",
        "docker run --rm img make test",
        "docker ps -a",
        "docker logs api",
        "kubectl exec pod-0 -- ls",
        "kubectl get pods -o wide",
        // -i alone (piped stdin) is fine without -t for exec
        "docker exec -i api sh -c 'cat > /tmp/f'",
        "kubectl exec -i pod-0 -- tee /tmp/f",
        // detached start just prints the name and returns
        "docker start api",
        // flags AFTER the `--` separator belong to the inner command
        "kubectl exec pod-0 -- ls -lt",
        // unrelated programs never match, whatever the flags
        "git log -it",
        "ls -it",
        // NOTE: `docker exec api grep -it x f` DOES match (docker has
        // no `--` separator, inner flags are indistinguishable) —
        // accepted false positive, costs one passthrough, never a hang.
    ] {
        assert!(
            !super::is_interactive_tty_invocation(cmd),
            "{cmd} is non-interactive — must stay wrappable"
        );
    }
}

// P2a — chain-segment gap (#745 follow-up). Whole-command exclusion
// gates must also suppress per-segment wraps in the chain-split
// path, otherwise `cd app && docker exec -it api bash` wraps the
// docker segment that the whole-command gate would have skipped.
#[test]
fn chain_segments_inherit_whole_command_exclusions() {
    for seg in [
        "ssh host uptime",          // exclude-list program
        "docker exec -it api bash", // interactive-tty gate
        "tail -f /var/log/syslog",  // continuous stream
    ] {
        assert!(
            super::bash_segment_excluded_from_wrap(seg),
            "segment `{seg}` must not be wrapped inside a chain"
        );
    }
    assert!(
        !super::bash_segment_excluded_from_wrap("cargo build --release"),
        "plain segments must stay wrappable inside chains"
    );
}

#[test]
fn bash_exclude_list_does_not_swallow_compactable_progs() {
    // Programs whose output the agent NEEDS — wrapping them
    // through `tokenade wrap` is the entire point of the hook.
    // Excluding any of these by accident would silently regress
    // the savings story.
    for prog in [
        "git",
        "cargo",
        "kubectl",
        "docker",
        "terraform",
        "helm",
        "npm",
        "yarn",
        "pnpm",
        "pip",
        "pytest",
        "go",
        "ls",
        "cat",
        "grep",
        "rg",
        "find",
        "make",
        "mvn",
        "gradle",
        "tar",
        "curl",
        "wget",
        "psql",
        "mysql",
        "sqlite3",
        "ansible",
        "fzf", // not a TUI for stdin-piped input — falls back to filter
        "python",
        "node",
        "ruby", // batch invocations dominate
    ] {
        assert!(
            !super::is_bash_intercept_excluded(prog),
            "{prog} must NOT be excluded — output is compactable"
        );
    }
}

#[test]
fn bash_exclude_list_is_exact_match_not_prefix() {
    // The matches! macro does exact equality. A program whose
    // first word merely starts with an excluded name (e.g.
    // `sshd`, `topgrade`, `lessc`) must not be excluded.
    for prog in ["sshd", "topgrade", "lessc", "ncbi", "yes2"] {
        assert!(
            !super::is_bash_intercept_excluded(prog),
            "{prog} is prefix-similar to an excluded name but not the same — must be intercepted"
        );
    }
}

/// `cd DIR && <payload-with-quotes>`: split_chain_segments bails on the
/// quotes, and the whole chain used to pass through raw (0.74M chars of
/// missed compaction in field data). The peel keeps `cd` in the outer
/// shell (cwd persistence) and wraps the payload as one unit.
#[test]
fn peel_leading_cd_rescues_unsplittable_payloads() {
    use crate::hooks::bash_pretool::{peel_leading_cd_for, PeelPayload};
    let peel_leading_cd = |c: &'static str| peel_leading_cd_for(c, PeelPayload::Single);
    let (pfx, payload) = peel_leading_cd(r#"cd /srv/app && python3 -c "print(1)""#).unwrap();
    assert_eq!(pfx, "cd /srv/app &&");
    assert_eq!(payload, r#"python3 -c "print(1)""#);
    // Fancy dirs and multi-segment payloads stay conservative.
    assert!(peel_leading_cd("cd \"my dir\" && ls").is_none());
    assert!(peel_leading_cd("cd $HOME && ls").is_none());
    assert!(peel_leading_cd("cd /tmp && a && b").is_none());
    assert!(peel_leading_cd("ls /tmp").is_none());
}

/// The peel knew ONE separator where the splitter knows four, and
/// refused every multi-segment payload. A `cd DIR; VAR=…; use $VAR` therefore
/// fell between two paths: the split path refuses a cross-segment variable, and
/// the peel refused the separator. Measured on 2 500 real Bash calls replayed
/// through the gate: 61 commands excluded as `cross-segment-shell-var`, 38 of
/// them a leading `cd` and nothing else needing the outer shell. After: 30.
#[test]
fn peel_leading_cd_knows_every_separator_the_splitter_knows() {
    use crate::hooks::bash_pretool::{peel_leading_cd_for, PeelPayload};
    // `;`, and the payload is a chain the split path has already refused.
    let (pfx, payload) =
        peel_leading_cd_for("cd /srv/app; W=/etc; ls $W", PeelPayload::Chain).unwrap();
    assert_eq!(pfx, "cd /srv/app ;");
    assert_eq!(payload, "W=/etc; ls $W");

    // A NEWLINE separator must produce a prefix that still runs. Spliced
    // verbatim it reads `cd /srv/app tokenade wrap …` — `cd` with two
    // arguments — so it is respelled `;`, which is what a newline means here.
    let (pfx, payload) =
        peel_leading_cd_for("cd /srv/app\nW=/etc\nls $W", PeelPayload::Chain).unwrap();
    assert_eq!(pfx, "cd /srv/app ;");
    assert!(
        !pfx.contains('\n'),
        "a newline cannot survive into the rewrite"
    );
    assert_eq!(payload, "W=/etc\nls $W");

    // A payload segment that needs the OUTER shell is still refused: one unit
    // would swallow the second `cd`.
    assert!(peel_leading_cd_for("cd /a; W=/etc; cd $W", PeelPayload::Chain).is_none());
    assert!(peel_leading_cd_for("cd /a; export W=/etc; ls $W", PeelPayload::Chain).is_none());

    // And `Single` — the historical contract — still hands a chain back to the
    // split path, which is why the existing pin above still holds.
    assert!(peel_leading_cd_for("cd /tmp; a; b", PeelPayload::Single).is_none());
}

/// `S=/path; git diff > $S/f`: the bare assignment runs unexported in the
/// outer shell; a split-wrapped later segment runs in wrap's CHILD shell
/// and expands $S to EMPTY (field incident: file written to "/"). Such
/// chains must skip the rewrite.
#[test]
fn chain_split_bails_on_cross_segment_var_use() {
    use crate::hooks::bash_pretool::chain_has_cross_segment_var_use;
    use tokenade_core::command_rewrite::split_chain_segments;
    let segs = split_chain_segments("S=/tmp/x; git diff > $S/out.patch").unwrap();
    assert!(chain_has_cross_segment_var_use(&segs));
    let segs = split_chain_segments("A=1 B=2; echo ${B}").unwrap();
    assert!(chain_has_cross_segment_var_use(&segs));
    // `VAR=1 cmd` scopes to that command — no cross-segment expectation.
    let segs = split_chain_segments("LANG=C sort f.txt; wc -l f.txt").unwrap();
    assert!(!chain_has_cross_segment_var_use(&segs));
    // Unrelated var name → fine.
    let segs = split_chain_segments("S=1; echo $T").unwrap();
    assert!(!chain_has_cross_segment_var_use(&segs));
    // cd chains keep their split (cwd IS inherited by wrap's child).
    let segs = split_chain_segments("cd /tmp && cargo test").unwrap();
    assert!(!chain_has_cross_segment_var_use(&segs));
}

/// `timeout 15 curl … ; echo "exit=$?"`: split-wrapping evaluates `$?` in
/// wrap's fresh child shell, which always reports 0 — the field incident
/// where a killed curl (124) looked like a success with a missing line.
/// Such chains must fall back to the whole-command wrap.
#[test]
fn chain_split_bails_on_outer_shell_status_params() {
    use crate::hooks::bash_pretool::chain_reads_outer_shell_status;
    use tokenade_core::command_rewrite::split_chain_segments;
    let segs = split_chain_segments("timeout 15 curl -s https://x/; echo \"exit=$?\"").unwrap();
    assert!(chain_reads_outer_shell_status(&segs));
    let segs = split_chain_segments("false; echo $?").unwrap();
    assert!(chain_reads_outer_shell_status(&segs));
    // Background pid and shell pid are equally shell-local.
    let segs = split_chain_segments("sleep 1 & echo $!").unwrap_or_default();
    assert!(segs.is_empty() || chain_reads_outer_shell_status(&segs));
    let segs = split_chain_segments("ls /tmp; echo /tmp/scratch.$$").unwrap();
    assert!(chain_reads_outer_shell_status(&segs));
    // No status read → the split-wrap (and its per-segment compactors) stays.
    let segs = split_chain_segments("cargo build && cargo test").unwrap();
    assert!(!chain_reads_outer_shell_status(&segs));
}

/// A command whose OUTPUT keeps changing is a poll, and the loop guard
/// stops calling it a loop.
///
/// The counter only ever compared the command's input. MEASURED 2026-09-05
/// across this machine's transcripts: of 50 firings whose command could be
/// matched to its own runs, **5 fired on a command whose outputs all
/// differed** — `timeout 900 python3 scripts/watch.py … | tail -10`, 7 runs
/// and 6 distinct answers, plus a `curl` health probe. The nudge lands in
/// PreToolUse, BEFORE the command runs, and says "re-running won't change the
/// result": on a watcher that is false, and it argues the agent out of the one
/// thing that was working.
///
/// Both halves are asserted, because a reset that fires on every run would
/// silence the guard entirely.
#[test]
fn a_command_whose_output_moves_is_not_called_a_loop() {
    let _home = tokenade_core::platform::ScratchHome::new("loop-guard-poll");
    let sess = "loopguard674";
    std::env::set_var("TOKENADE_SESSION", sess);
    let parsed = serde_json::json!({ "session_id": sess });

    // Anti-vacuity: a genuinely stuck command still gets the steer at the 3rd.
    let stuck = "cargo test --lib broken_case";
    assert!(crate::hooks::loop_guard_nudge(&parsed, stuck).is_none());
    assert!(crate::hooks::loop_guard_nudge(&parsed, stuck).is_none());
    assert!(
        crate::hooks::loop_guard_nudge(&parsed, stuck).is_some(),
        "the guard must still fire on a command that really is stuck"
    );

    // A poll: every run answered differently, so the counter is cleared each
    // time and the 3rd run is still a first run.
    let poll = "timeout 900 python3 scripts/watch.py 2>&1 | tail -10";
    for run in 1..=5 {
        let fired = crate::hooks::loop_guard_nudge(&parsed, poll).is_some();
        assert!(
            !fired,
            "run {run} of a poll was called a loop — its output moved every time"
        );
        crate::hooks::reset_loop_guard_if_output_moved(poll, true, false);
    }

    // And `false` must not reset, or the guard is dead for everyone.
    // The other three combinations must NOT reset, or the guard is dead for
    // everyone: same bytes (the loop it exists for), and a first run of any
    // kind, where nothing has moved yet.
    for (ran_before, same_bytes) in [(true, true), (false, true), (false, false)] {
        let same = format!("cat frozen-{ran_before}-{same_bytes}.txt");
        for _ in 0..2 {
            assert!(crate::hooks::loop_guard_nudge(&parsed, &same).is_none());
            crate::hooks::reset_loop_guard_if_output_moved(&same, ran_before, same_bytes);
        }
        assert!(
            crate::hooks::loop_guard_nudge(&parsed, &same).is_some(),
            "({ran_before}, {same_bytes}) reset a counter it must have left alone"
        );
    }

    // Counters are per COMMAND. Two commands sharing one would let a fire on
    // the first silence the second, or the reverse.
    let a = "make build-alpha";
    let b = "make build-bravo";
    assert!(crate::hooks::loop_guard_nudge(&parsed, a).is_none());
    assert!(crate::hooks::loop_guard_nudge(&parsed, a).is_none());
    assert!(
        crate::hooks::loop_guard_nudge(&parsed, b).is_none(),
        "a different command must start its own count"
    );
    std::env::remove_var("TOKENADE_SESSION");
}

/// Disabling the hook for a command the hook already ignores is said
/// once, not paid for in silence.
///
/// MEASURED 2026-09-05 across this machine's transcripts: of 556 runs carrying
/// `TOKENADE_HOOK_DISABLED=1` or `TOKENADE_FORCE_BYPASS=1`, **239 (43%) named
/// a program already on the pass-through list** — `sudo` 146, `sshpass` 93.
/// The variable did nothing, and the run is still booked as a lost saving. The
/// habit starts where the caller is right to be careful (a `sudo` askpass flow)
/// and spreads: 244 of those came from one session.
#[test]
fn a_bypass_of_a_command_we_never_touch_is_said_once() {
    let _home = tokenade_core::platform::ScratchHome::new("redundant-bypass");
    let parsed = serde_json::json!({ "session_id": "bypass675" });

    let msg = crate::hooks::bash_pretool::redundant_bypass_notice(
        &parsed,
        "SUDO_ASKPASS=/tmp/ap.sh TOKENADE_HOOK_DISABLED=1 sudo -A systemctl restart web",
    )
    .expect("a bypass of an excluded program must be named");
    assert!(
        msg.contains("`sudo`") && msg.contains("pass-through"),
        "the notice must name the program the caller wrote:\n{msg}"
    );

    // ONCE per session: the fact is worth one sentence, not one per command.
    assert!(
        crate::hooks::bash_pretool::redundant_bypass_notice(
            &parsed,
            "TOKENADE_HOOK_DISABLED=1 sshpass -p x ssh host uptime",
        )
        .is_none(),
        "the notice repeated inside one session"
    );

    // A different session says it again — the habit is per agent, not global.
    let other = serde_json::json!({ "session_id": "bypass675other" });
    assert!(
        crate::hooks::bash_pretool::redundant_bypass_notice(
            &other,
            "TOKENADE_HOOK_DISABLED=1 sshpass -p x ssh host uptime",
        )
        .is_some(),
        "a fresh session must get the fact too"
    );

    // The three ways this must stay quiet, each for its own reason.
    let quiet = serde_json::json!({ "session_id": "bypass675quiet" });
    for (why, cmd) in [
        // A command tokenade really does fold: the bypass is doing something.
        (
            "a real bypass",
            "TOKENADE_HOOK_DISABLED=1 cargo test --workspace",
        ),
        // An excluded program with no bypass at all: nothing to say.
        ("no bypass", "sudo -A systemctl restart web"),
        // Neither.
        ("neither", "cargo test --workspace"),
    ] {
        assert!(
            crate::hooks::bash_pretool::redundant_bypass_notice(&quiet, cmd).is_none(),
            "{why}: the notice fired on {cmd:?}"
        );
    }
    // And that session must still be able to say it when the case is real —
    // or the loop above passed by having burned the once-per-session marker.
    assert!(
        crate::hooks::bash_pretool::redundant_bypass_notice(
            &quiet,
            "TOKENADE_FORCE_BYPASS=1 less /var/log/syslog",
        )
        .is_some(),
        "the quiet cases must not have consumed the session's one notice"
    );
}

/// The SAFE CLASS guard existed on the door nobody uses.
///
/// `tokenade raw grep …` has been REFUSED since #1031: re-running spends the
/// first call's tokens a second time, while `expand-ref` returns the same
/// content without re-running anything. But MEASURED over the 782 transcripts
/// of this machine: **2 329 bypasses go through `TOKENADE_HOOK_DISABLED=1`
/// against 133 through `tokenade raw`**, and the variable's door said nothing —
/// 303 KB of `grep`/`sed`/`cat`/`find`/`wc` output delivered unfolded, decided
/// 98 % of the time WITHOUT any experience of the fold. One list, two doors (a
/// fallback is judged where the agent reads).
#[test]
fn a_safe_class_bypass_is_named_on_the_variable_door() {
    let _home = tokenade_core::platform::ScratchHome::new("safe-class-bypass");
    let parsed = serde_json::json!({ "session_id": "safeclass1015" });

    let msg = crate::hooks::bash_pretool::redundant_bypass_notice(
        &parsed,
        "TOKENADE_HOOK_DISABLED=1 grep -rn motif crates/",
    )
    .expect("a safe-class bypass must be named");
    assert!(
        msg.contains("`grep`") && msg.contains("expand-ref"),
        "the note must name the program AND the cheaper fallback:\n{msg}"
    );

    // The list is the one of `tokenade raw`, not a second copy: what one
    // refuses, the other names.
    let other = serde_json::json!({ "session_id": "safeclass1015b" });
    assert!(
        crate::hooks::bash_pretool::redundant_bypass_notice(
            &other,
            "TOKENADE_HOOK_DISABLED=1 sed -n 1,200p fichier.rs",
        )
        .is_some(),
        "`sed` is in the safe class of `tokenade raw` and must be here too"
    );

    // A program OUTSIDE the safe class stays silent: `git` has a compactor
    // that transforms, and the agent may legitimately want the bytes.
    let third = serde_json::json!({ "session_id": "safeclass1015c" });
    assert!(
        crate::hooks::bash_pretool::redundant_bypass_notice(
            &third,
            "TOKENADE_HOOK_DISABLED=1 git log --stat",
        )
        .is_none(),
        "`git` is not in the safe class: the note must not settle its case"
    );
}

/// The cat-before-Edit heads-up reads the CLAUSE, not the first token.
///
/// `cat` output does not satisfy the Edit tool's Read-first precondition, so
/// `cat a.ts b.ts` followed by an Edit costs the content twice. The check asked
/// whether the FIRST token was `cat`, so `cd repo && cat a.ts b.ts` — the same
/// command — was invisible. MEASURED 2026-09-05 across this machine's
/// transcripts: 118 multi-source `cat` clauses, 91 seen and **27 missed, 20 of
/// them behind a leading `cd`**. Same `cd`-as-program class this file has paid
/// for in the router and in the compactor-gap nudge.
///
/// Scoping to the clause is the other half of the fix: the token count used to
/// run over the whole line, so a later clause could push a single-file `cat`
/// over the two-file bar on its own.
#[test]
fn the_cat_before_edit_heads_up_reads_the_clause() {
    let _home = tokenade_core::platform::ScratchHome::new("catedit-clause");
    let fire = |sess: &str, cmd: &str| {
        crate::hooks::bash_pretool::cat_edit_precondition_nudge(
            &serde_json::json!({ "session_id": sess }),
            cmd,
        )
        .is_some()
    };

    assert!(
        fire("cc-a", "cat src/a.ts src/b.ts"),
        "the plain form must fire"
    );
    assert!(
        fire("cc-b", "cd repo && cat src/a.ts src/b.ts"),
        "a leading `cd` hid the same command"
    );
    assert!(
        fire("cc-c", "cd repo; cat src/a.ts src/b.ts | head -200"),
        "`;` and a trailing pipe are the same command too"
    );

    // One source file is not the pattern this warns about, and a later clause
    // must not push it over the bar.
    assert!(
        !fire("cc-d", "cat src/a.ts"),
        "a single-file cat is not the two-file pattern"
    );
    assert!(
        !fire("cc-e", "cat src/a.ts && grep -n x src/b.ts"),
        "a later clause must not be counted as cat's arguments"
    );
    // Nothing to do with cat at all.
    assert!(!fire("cc-f", "cd repo && grep -rn foo src/a.ts src/b.ts"));

    // Once per session, as before.
    assert!(fire("cc-g", "cat src/a.ts src/b.ts"));
    assert!(
        !fire("cc-g", "cd repo && cat src/c.ts src/d.ts"),
        "the heads-up repeated inside one session"
    );
}

/// The hook counts the WHOLE command, `wrap` only sees the segment it wraps:
/// without the alias set at rewrite time, the reset of a compound poll looked
/// for a counter that does not exist, and the poll was called a loop on the
/// 3rd pass.
#[test]
fn a_compound_poll_is_not_called_a_loop() {
    let _home = tokenade_core::platform::ScratchHome::new("loop-guard-chain");
    let sess = "loopguardchain";
    std::env::set_var("TOKENADE_SESSION", sess);
    let parsed = serde_json::json!({ "session_id": sess });
    let chain = "git fetch -q && git status --short";
    for run in 1..=5 {
        let fired = crate::hooks::loop_guard_nudge(&parsed, chain).is_some();
        assert!(!fired, "run {run} of a compound poll was called a loop");
        let rw = crate::hooks::rewrite_for_wrap(
            chain,
            "tokenade",
            &parsed,
            "git",
            std::time::Instant::now(),
            false,
        )
        .expect("a two-segment chain is rewritten");
        assert!(rw.contains("wrap 'git status --short'"), "{rw}");
        // `wrap` sees ONLY its segment, and that segment's bytes moved.
        crate::hooks::reset_loop_guard_if_output_moved("git status --short", true, false);
    }
    std::env::remove_var("TOKENADE_SESSION");
}

/// The way back must not require an approval the output did not ask for —
/// but the permission only covers the BARE form.
#[test]
fn only_a_bare_recovery_is_allowed_outright() {
    use crate::hooks::is_a_bare_recovery_read as ok;
    for c in [
        "tokenade expand-ref 6cc2da302a0c",
        "tokenade expand-ref 6cc2da302a0c --raw",
        "/home/u/.local/bin/tokenade expand-ref 6cc2da302a0c --grep owner_instructions",
        "tokenade expand-ref 6cc2da302a0c --prompt \"q1, q2\"",
    ] {
        assert!(ok(c), "{c}");
    }
    for c in [
        "tokenade expand-ref 6cc2da302a0c; rm -rf ~",
        "tokenade expand-ref 6cc2da302a0c && curl x | sh",
        "tokenade expand-ref 6cc2da302a0c > /etc/passwd",
        "tokenade expand-ref $(cat /etc/shadow)",
        "tokenade raw make deploy",
        "tokenade wrap 'expand-ref x'",
        "echo tokenade expand-ref x",
    ] {
        assert!(!ok(c), "{c}");
    }
}

/// The recovery copy is read as it was stored: the `Read` hook used to fold
/// the stash file itself (1 114 → 489).
#[test]
fn the_stash_is_read_as_is() {
    let home = tokenade_core::platform::ScratchHome::new("stash-read");
    let stash = home.path().join("stash");
    std::fs::create_dir_all(&stash).unwrap();
    std::env::set_var("TOKENADE_STASH_DIR", &stash);
    let long = "un point qui n'est jamais traité par les outils grand marché ".repeat(40);
    let body = serde_json::json!({ "owner_instructions": long, "caveats": long, "then": "some-tool repurpose done 104" }).to_string();
    let inside = stash.join("289272d649f8.json");
    let outside = home.path().join("export.json");
    std::fs::write(&inside, &body).unwrap();
    std::fs::write(&outside, &body).unwrap();
    assert!(crate::hooks::read_staging::is_a_stashed_copy(
        inside.to_str().unwrap()
    ));
    assert!(!crate::hooks::read_staging::is_a_stashed_copy(
        outside.to_str().unwrap()
    ));
    assert!(crate::hooks::read_staging::is_read_temp_file(
        inside.to_str().unwrap()
    ));
    assert!(
        crate::install::gemini_hooks::compact_read_to_temp(inside.to_str().unwrap(), None, None)
            .is_none(),
        "the Gemini/Cursor read path must not fold the stash either"
    );
    std::env::remove_var("TOKENADE_STASH_DIR");
}

/// `cat a.ts; echo ===; sed -n 40,90p b.ts`: ONE read. Split by the hook, each
/// statement was judged alone and the promise of 1.1.22 ("delivered as is
/// under the read threshold") only held through `tokenade wrap '<chain>'`,
/// never through the hook — measured 2026-10-02.
#[test]
fn a_chain_of_reads_stays_a_single_read() {
    let _home = tokenade_core::platform::ScratchHome::new("read-chain");
    let dir = std::env::temp_dir().join(format!("tkn-read-chain-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (a, b) = (dir.join("a.ts"), dir.join("b.rs"));
    std::fs::write(&a, "export const a = 1;\n").unwrap();
    std::fs::write(&b, "fn b() {}\n").unwrap();
    let (a, b) = (
        a.to_string_lossy().to_string(),
        b.to_string_lossy().to_string(),
    );
    let parsed = serde_json::json!({ "session_id": "readchain" });
    let rewrite = |cmd: &str| {
        crate::hooks::rewrite_for_wrap(
            cmd,
            "tokenade",
            &parsed,
            "cat",
            std::time::Instant::now(),
            false,
        )
        .expect("rewritten")
    };
    for chain in [
        format!("cat {a}; echo ===; sed -n 40,90p {b}"),
        format!("head -60 {a} && echo --- && tail -20 {b}"),
    ] {
        let rw = rewrite(&chain);
        assert_eq!(
            rw.matches(" wrap ").count(),
            1,
            "a chain of reads is one unit: {rw}"
        );
        assert!(
            rw.contains(&format!("sed -n 40,90p {b}")) || rw.contains("tail -20"),
            "{rw}"
        );
    }
    // A leading `cd`: it stays in the agent's shell (the `try_cd_peel` path,
    // which emits by itself and returns `Err(0)`), the reads go together.
    let cd_reads = format!(
        "cd {} && cat a.ts; echo ===; sed -n 1,5p b.rs",
        dir.display()
    );
    let r = crate::hooks::rewrite_for_wrap(
        &cd_reads,
        "tokenade",
        &parsed,
        "cd",
        std::time::Instant::now(),
        false,
    );
    assert_eq!(
        r,
        Err(0),
        "a cd-led chain of reads is peeled as ONE payload: {r:?}"
    );
    let (prefix, payload) =
        crate::hooks::peel_leading_cd_for(&cd_reads, crate::hooks::PeelPayload::Chain).unwrap();
    assert_eq!(
        (prefix.as_str(), payload),
        (
            format!("cd {} &&", dir.display()).as_str(),
            "cat a.ts; echo ===; sed -n 1,5p b.rs"
        )
    );
    // The same `cd` before something other than a read: split as before.
    let rw = rewrite(&format!(
        "cd {} && cat a.ts; git status --short",
        dir.display()
    ));
    assert!(rw.matches(" wrap ").count() >= 2, "{rw}");
    // A statement that is not a read: the chain is split as before.
    let rw = rewrite(&format!("cat {a}; git status --short"));
    assert_eq!(rw.matches(" wrap ").count(), 2, "{rw}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A deep session tightens its thresholds — for ALL rewrite shapes. Only the
/// single-unit arm exported the pressure: a split chain and a payload under
/// `cd` went through at the thresholds of a fresh session (measured
/// 2026-10-02).
#[test]
fn pressure_follows_every_segment_of_a_chain() {
    let _home = tokenade_core::platform::ScratchHome::new("chain-pressure");
    std::env::remove_var("TOKENADE_PRESSURE");
    let sess = "chainpressure";
    let dir = tokenade_core::platform::home_dir()
        .unwrap()
        .join(".tokenade")
        .join("sessions");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{sess}-turn")), "120").unwrap();
    let parsed = serde_json::json!({ "session_id": sess });
    let rewrite = |cmd: &str| {
        crate::hooks::rewrite_for_wrap(
            cmd,
            "tokenade",
            &parsed,
            "git",
            std::time::Instant::now(),
            false,
        )
        .expect("rewritten")
    };
    for cmd in [
        "git status --short && git log --oneline -3",
        "git status --short",
    ] {
        let rw = rewrite(cmd);
        let wraps = rw.matches(" wrap ").count();
        assert!(wraps >= 1, "{rw}");
        assert_eq!(
            rw.matches("TOKENADE_PRESSURE=0.35 ").count(),
            wraps,
            "every wrap carries the pressure: {rw}"
        );
    }
    std::env::remove_var("TOKENADE_PRESSURE");
}
