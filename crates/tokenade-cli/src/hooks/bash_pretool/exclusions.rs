//! **What we NEVER intercept.**
//!
//! An interactive command, a continuous stream, an already-safe program:
//! stepping in front of them saves no tokens, it blocks the user. The refusal
//! is SAID when it is redundant, otherwise we push a reminder about a bypass
//! that isn't one.

use super::*;

/// Commands we deliberately do NOT intercept at PreToolUse. Either
/// they're interactive (vim, sudo password prompt), continuous
/// streams (tail -f), or TUI apps (top) that don't fit the
/// run-once-capture-output model.
/// Is this command on the interactive/streaming exclude list — under EITHER
/// spelling of its program?
///
/// `prog` is the first meaningful word, so a launcher prefix hides what
/// actually runs: `timeout 60 vim f`, `nohup less big`, `env EDITOR=x vim f`,
/// `command less f`, `setsid top` and `nice -n 5 htop` all resolved to the
/// launcher, missed [`is_bash_intercept_excluded`], and were WRAPPED — the one
/// thing the scaffolded rules promise never happens ("the hook already passes
/// interactive/TTY commands … through untouched"). Verified by wrapping them:
/// `tokenade wrap 'timeout 60 vim f'` hangs exactly like the bare form the
/// list was written to catch.
///
/// [`tokenade_core::filter::program_basename`] is THE resolver for this — it
/// skips shell scaffolding, unwraps one wrapper level, and eats the wrapper's
/// own flags and its bare numeric operand (`timeout -k 5 120 cargo test` →
/// `cargo`). It already existed; this gate had grown a weaker second answer to
/// the same question.
///
/// Asked ALONGSIDE the typed form, never instead of it: peeling must not make
/// an excluded spelling look includable. And it only ever SUBTRACTS wrapping,
/// so a peel that lands somewhere odd costs one passthrough and never a hang —
/// the safe direction for this gate, the one
/// [`is_interactive_tty_invocation`] already argues for.
///
/// Prevalence of observed HARM is zero: over 94 031 real Bash calls, 81 reach
/// this (67 `timeout N sshpass … ssh`, 14 `timeout N ssh`) and all 81 returned
/// normally, because a non-interactive `ssh host 'cmd'` needs no TTY. Those
/// are no longer compacted — which is what the BARE `ssh host 'cmd'` has
/// always done. The inconsistency was the defect; the demonstrated hang is why
/// closing it is worth 81 passthroughs, not a count of incidents.
///
/// `stdbuf`, `unbuffer` and `script` are NOT peeled, because
/// `filter::is_wrapper_prog` does not know them and this iteration measured
/// them at ZERO invocations across the same corpus. Naming them here on no
/// evidence is how a list starts drifting from what it is for.
/// The caller disabled the hook for a command the hook already ignores.
///
/// MEASURED 2026-09-05 across this machine's transcripts: of 556 runs
/// carrying `TOKENADE_HOOK_DISABLED=1` or `TOKENADE_FORCE_BYPASS=1`, **239
/// (43%) named a program that is already on the pass-through list** — `sudo`
/// 146, `sshpass` 93. The variable did nothing: those commands are never
/// wrapped, never compacted. Worse than nothing, in fact — a bypass is
/// recorded and shown on the dashboard as a LOST saving, so the accounting
/// picks up 239 losses that never happened.
///
/// The shipped guidance already says not to do this pre-emptively, and it was
/// not enough: the habit starts on one command where the caller is right to be
/// careful (a `sudo` askpass flow, an `sshpass` login) and then spreads — 244
/// of these came from ONE session. Saying it at the moment it happens is what
/// the rest of this file does with every other silent no-op.
///
/// Once per session: the fact is worth one sentence, not one per command.
/// Print the notice, in the protocol-safe shape (context, no rewrite).
pub(crate) fn emit_redundant_bypass_notice(parsed: &serde_json::Value, cmd: &str) {
    let Some(msg) = redundant_bypass_notice(parsed, cmd) else {
        return;
    };
    crate::log_emitted_additional_context("hookwrap-pretool-bypassnotice", "PreToolUse", &msg);
    println!(
        "{}",
        serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "additionalContext": msg,
            }
        })
    );
}

pub(crate) fn redundant_bypass_notice(parsed: &serde_json::Value, cmd: &str) -> Option<String> {
    if hook_disabled_by_env() {
        return None;
    }
    // The assignment travels INSIDE the command string (`FOO=1 sudo …`), so
    // the hook's own environment never carries it — this is a text question.
    if !cmd.contains("TOKENADE_HOOK_DISABLED=1") && !cmd.contains("TOKENADE_FORCE_BYPASS=1") {
        return None;
    }
    // `excluded_program_in` asks the SAME question as
    // `bash_command_is_excluded_by_list` and answers with the name, so a
    // separate guard on the boolean changed nothing and a mutation dropping it
    // survived. One question, asked once.
    // Two reasons to speak at this gate, not one. The first: the program is
    // on the pass-through list, so the variable changed nothing. The second,
    // measured: the program is of the SAFE CLASS that `tokenade raw` REFUSES
    // to bypass — `grep`, `sed`, `cat`, `find`, `wc`… The guard only existed
    // at the gate of the 133 `tokenade raw` calls, while the 2 329 real
    // bypasses go through this variable: 303 kB of safe-class output
    // delivered unfolded on this machine's corpus alone. Same information,
    // same moment, at the gate the agent actually uses.
    let reason = excluded_program_in(cmd)
        .map(Reason::PassesThrough)
        .or_else(|| safe_class_in(cmd).map(Reason::SafeClass))?;
    let session = crate::hooks::session_id_of(parsed).unwrap_or("nosess");
    let skey: String = session
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(40)
        .collect();
    let dir = crate::hooks::read_pending_dir();
    let _ = std::fs::create_dir_all(&dir);
    let marker = dir.join(format!("bypass-notice-{skey}"));
    if marker.exists() {
        return None;
    }
    let _ = std::fs::write(&marker, "1");
    let prog = match &reason {
        Reason::PassesThrough(p) | Reason::SafeClass(p) => p.clone(),
    };
    if let Reason::SafeClass(prog) = &reason {
        tokenade_core::gain::record_nudge(
            "safe-class-bypass",
            "fire",
            tokenade_core::gain::NudgeMetric::None,
        );
        return Some(format!(
            "[tokenade] `{prog}` folds well, and its fold is recoverable: \
             `tokenade expand-ref <hash>` (hash from the banner) returns what was stashed, \
             instantly and without re-running. `tokenade raw {prog}` is refused for this \
             reason; the variable is not, so this run is booked as a lost saving. For the \
             EXACT original bytes, read the source the command read."
        ));
    }
    tokenade_core::gain::record_nudge(
        "redundant-bypass",
        "fire",
        tokenade_core::gain::NudgeMetric::None,
    );
    let why = if prog == "tokenade" {
        "a `tokenade …` command is never wrapped by the hook — it would be wrapping itself"
    } else {
        "`{prog}` is on tokenade's pass-through list — it is never wrapped and its output is \
         never compacted (same for vim/less/man/ssh/scp/rsync/tmux/watch and the rest of that \
         family)"
    }
    .replace("{prog}", &prog);
    Some(format!(
        "[tokenade] {why}, so disabling the hook here changed nothing (and books the run as a \
         lost saving). Keep the variable for a command tokenade actually folds."
    ))
}

/// Why this gate speaks. Both cases share the same session marker: one note
/// per session, whatever the reason.
pub(crate) enum Reason {
    PassesThrough(String),
    SafeClass(String),
}

/// The SAFE-CLASS program this command names — the list is `tokenade raw`'s,
/// never a second copy (a rule written twice drifts).
pub(crate) fn safe_class_in(cmd: &str) -> Option<String> {
    let first = extract_first_meaningful_word(cmd);
    crate::wrap::is_safe_class(first).then(|| first.to_string())
}

/// Which pass-through program this command names — the one to put in the
/// notice, so it names what the caller wrote rather than a category.
pub(crate) fn excluded_program_in(cmd: &str) -> Option<String> {
    let first = extract_first_meaningful_word(cmd);
    // A `tokenade …` command is skipped too, a hundred lines below, to avoid
    // wrapping ourselves — so disabling the hook for one is the same no-op.
    // Measured in the same pass: 17 of the 556.
    if first == "tokenade" {
        return Some("tokenade".to_string());
    }
    if is_bash_intercept_excluded(first) {
        return Some(first.to_string());
    }
    let peeled = tokenade_core::filter::program_basename(
        &cmd.split_whitespace().map(String::from).collect::<Vec<_>>(),
    );
    is_bash_intercept_excluded(&peeled).then_some(peeled)
}

pub(crate) fn bash_command_is_excluded_by_list(cmd: &str) -> bool {
    if is_bash_intercept_excluded(extract_first_meaningful_word(cmd)) {
        return true;
    }
    let peeled = tokenade_core::filter::program_basename(
        &cmd.split_whitespace().map(String::from).collect::<Vec<_>>(),
    );
    is_bash_intercept_excluded(&peeled)
}

pub(crate) fn is_bash_intercept_excluded(prog: &str) -> bool {
    matches!(
        prog,
        "vim"
            | "vi"
            | "nano"
            | "emacs"
            | "ed"
            | "less"
            | "more"
            | "most"
            | "man"
            | "top"
            | "htop"
            | "btop"
            | "atop"
            | "iotop"
            | "iftop"
            | "nvtop"
            | "watch"
            | "tmux"
            | "screen"
            | "byobu"
            | "ranger"
            | "nnn"
            | "lf"
            | "broot"
            | "sudo"
            | "su"
            | "doas"
            | "ssh"
            // `mosh` is ssh-with-roaming; same interactive
            // shell semantics and was missing from the exclude list.
            // If the agent ever runs `mosh server` the hook would wrap
            // it into `bash -c 'mosh server'` and hang waiting for a TTY.
            | "mosh"
            | "telnet"
            | "ftp"
            | "sftp"
            | "rlogin"
            | "nc"
            | "ncat"
            | "socat"
            | "yes"
            // Binary-transfer + secret-bearing tools that
            // agents systematically bypass via `tokenade raw`.
            // Telemetry from the release-pipeline session 2026-06-02:
            // every `tokenade raw scp/rsync/sshpass …` was for moving
            // bytes (no compactable stdout — just a status line or
            // percent counter) or feeding a password through argv.
            // Wrapping cannot help here AND, for sshpass, would log
            // the password before redact runs in the worst-case
            // failure modes. Note: `tar`/`gzip`/`xz`/`zstd`/`dd` are
            // deliberately NOT added — they have legitimate textual
            // modes (`tar -tzf`, `xzcat foo.txt.xz`) that the agent
            // still wants compacted.
            | "scp"
            | "rsync"
            | "sshpass"
    )
}

/// Returns true when the command would stream output indefinitely.
/// Gates on the program AND a `--follow`-ish flag — a naive flag-only
/// check produced false-positives across the dataset (production
/// telemetry of `bash_excluded reason=continuous-stream`):
///   * `cargo build -F feature_x`   — `-F` is cargo's feature flag
///   * `grep -F 'literal'`          — `-F` is grep --fixed-strings
///   * `pkill -f pattern`           — `-f` is pkill --full
///   * `pgrep -f pattern`           — same as pkill
///   * `sshpass -f /path/to/pw`     — `-f` is sshpass --password-file
///   * `gcc -f<option>`             — many gcc flags collide
///
/// All of these were skipping the intercept and losing the cargo /
/// grep compactor wins. Real streamers are narrow: tail/journalctl
/// and the container-log subcommand families.
pub(crate) fn is_continuous_stream(cmd: &str) -> bool {
    let prog = command_rewrite::first_program_token(cmd);
    if prog.is_empty() {
        return false;
    }
    // Tokenize once; matches are checked against the post-program argv.
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    let has_short_f = tokens.contains(&"-f");
    let has_short_cap_f = tokens.contains(&"-F");
    let has_long_follow = tokens
        .iter()
        .any(|t| *t == "--follow" || *t == "--follow-name");
    match prog {
        // `tail -f` / `tail -F` (follow-rename) / `tail --follow`.
        "tail" => has_short_f || has_short_cap_f || has_long_follow,
        // journalctl / logread: only `-f` / `--follow` mean follow.
        "journalctl" | "logread" => has_short_f || has_long_follow,
        // Container/cluster log subcommand families: require BOTH the
        // `logs` subcommand AND a follow flag.
        "docker" | "podman" | "kubectl" | "oc" | "k" => {
            tokens.contains(&"logs") && (has_short_f || has_long_follow)
        }
        // `gcloud logging tail` always streams.
        "gcloud" => {
            tokens.contains(&"tail") && tokens.iter().any(|t| *t == "logging" || *t == "log")
        }
        _ => false,
    }
}

/// True when the command requests an interactive TTY session that would
/// hang under `tokenade wrap` (the wrap side has no TTY to hand to the
/// inner program, which then blocks waiting for one). The ssh/vim
/// families are unconditionally on `is_bash_intercept_excluded`; this
/// covers the FLAG-gated cases — container/cluster `exec`/`run` with
/// `-it`-style flags — where the same program is usually batch
/// (`docker run --rm img make`) and only interactive when a TTY is
/// explicitly requested. `attach` is interactive by design, no flag
/// needed.
///
/// Flag scan stops at a standalone `--` (the k8s in-container command
/// separator) so `kubectl exec pod -- ls -lt` doesn't read `-lt` as
/// TTY flags. For the docker family there is no such separator; a rare
/// false-positive on an inner-command flag costs one passthrough (no
/// compaction), never a hang — the safe direction for this gate.
pub(crate) fn is_interactive_tty_invocation(cmd: &str) -> bool {
    let prog = command_rewrite::first_program_token(cmd);
    if prog.is_empty() {
        return false;
    }
    let scan: Vec<&str> = cmd.split_whitespace().take_while(|t| *t != "--").collect();
    let has_sub = |sub: &str| scan.iter().skip(1).any(|t| *t == sub);
    let (mut has_i, mut has_t, mut has_a) = (false, false, false);
    for t in scan.iter().skip(1) {
        match *t {
            "--interactive" => has_i = true,
            "--tty" => has_t = true,
            "--attach" => has_a = true,
            _ => {
                // Combined short flags: `-it`, `-ti`, also `-dit`.
                // Letters only — `-t30` or `-i=x` style values stay out.
                if let Some(body) = t.strip_prefix('-') {
                    if !body.is_empty()
                        && !body.starts_with('-')
                        && body.chars().all(|c| c.is_ascii_alphabetic())
                    {
                        has_i |= body.contains('i');
                        has_t |= body.contains('t');
                        has_a |= body.contains('a');
                    }
                }
            }
        }
    }
    let wants_tty = has_i && has_t;
    match prog {
        "docker" | "podman" | "nerdctl" => {
            has_sub("attach")
                // `start -a`/`-i` re-attaches to the container's streams
                // and blocks until it exits — interactive without -t.
                || (has_sub("start") && (has_a || has_i))
                || ((has_sub("exec") || has_sub("run")) && wants_tty)
        }
        "kubectl" | "oc" | "k" => {
            has_sub("attach") || ((has_sub("exec") || has_sub("debug")) && wants_tty)
        }
        _ => false,
    }
}

/// Per-segment mirror of the three pretool exclusion gates
/// (exclude-list / continuous-stream / interactive-tty). The
/// chain deferral relies on the split-wrap path to re-apply the
/// safety checks per segment, but `segment_must_not_be_wrapped` only
/// covers shell-state builtins — so `cd /tmp && ssh host` or
/// `cd app && docker exec -it api bash` had their second segment
/// wrapped anyway (TTY hang for the interactive ones). This closes
/// the gap at the mechanism level: every gate that suppresses a
/// whole-command wrap also suppresses a segment wrap.
pub(crate) fn bash_segment_excluded_from_wrap(seg: &str) -> bool {
    // Peeled as well as typed — this is the THIRD site that asked the exclude
    // list the same question with its own weaker resolver, and it is the one
    // the chain path goes through: `cd /tmp; vim f` was passed through while
    // `cd /tmp; timeout 60 vim f` had the `vim` segment wrapped, which hangs.
    bash_command_is_excluded_by_list(seg)
        || is_continuous_stream(seg)
        || is_interactive_tty_invocation(seg)
}
