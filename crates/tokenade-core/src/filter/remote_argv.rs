//! Recovering the REAL command from a wrapper.
//!
//! `docker exec`, `kubectl exec`, `docker compose run` and `ssh` all carry
//! another command as their tail; the compactor that matters is the one for
//! THAT command, not for the wrapper.

use crate::argv::basename;

use super::*;

/// Extract the REMOTE command argv from an `ssh …` invocation, or `None` when
/// there is no remote command (bare `ssh host` = interactive). Skips ssh's own
/// options — including the short flags that consume the NEXT token as a value
/// (`-p 2222`, `-i key`, `-o Opt=val`, `-L …`) — then takes the first non-
/// option token as the host and returns everything after it. The returned
/// tokens still carry any shell quoting (`'docker`, `ps'`); the downstream
/// `program_basename`/`basename` already strip that.
/// `docker exec [flags] <container> CMD…` → CMD tokens. `exec_pos` is the
/// index of the `exec` token (1 for `docker exec`). Skips exec flags,
/// including the value-consuming ones (`-u user`, `-e K=V`, `-w dir`,
/// `--env K=V`, …); the first non-flag token is the container, the rest is
/// the remote command.
pub(super) fn container_exec_remote_argv(argv: &[String], exec_pos: usize) -> Option<Vec<String>> {
    let mut i = exec_pos + 1;
    while i < argv.len() {
        let a = argv[i].as_str();
        if let Some(rest) = a.strip_prefix("--") {
            // --flag=value is self-contained; bare --env/--user/--workdir
            // consume the next token.
            let consumes = !rest.contains('=')
                && matches!(
                    rest,
                    "env" | "user" | "workdir" | "detach-keys" | "env-file"
                );
            i += if consumes { 2 } else { 1 };
            continue;
        }
        if let Some(rest) = a.strip_prefix('-') {
            let consumes = rest.len() == 1 && matches!(rest.as_bytes()[0], b'u' | b'e' | b'w');
            i += if consumes { 2 } else { 1 };
            continue;
        }
        break; // container name
    }
    let remote = argv.get(i + 1..)?;
    if remote.is_empty() {
        None
    } else {
        Some(remote.to_vec())
    }
}

/// `docker compose [opts] exec|run [flags] <service> CMD…` → CMD tokens.
pub(super) fn compose_exec_remote_argv(argv: &[String]) -> Option<Vec<String>> {
    let pos = argv.iter().position(|a| a == "exec" || a == "run")?;
    container_exec_remote_argv(argv, pos)
}

pub(super) fn ssh_remote_argv(argv: &[String]) -> Option<Vec<String>> {
    // ssh short options that take a following value.
    const VAL_FLAGS: &[u8] = b"bcDEeFIiJLlmOopQRSWw";
    let mut i = 1; // argv[0] == "ssh"
    while i < argv.len() {
        let a = argv[i].as_str();
        if let Some(rest) = a.strip_prefix('-') {
            // A bare `-p` (length-1 flag body, value in next token) consumes
            // the next token; `-p2222` / multi-char / `-4` do not.
            let consumes_next =
                rest.len() == 1 && rest.bytes().next().is_some_and(|b| VAL_FLAGS.contains(&b));
            i += if consumes_next { 2 } else { 1 };
            continue;
        }
        break; // first non-option token = host
    }
    // i is at the host (if present); the remote command is everything after.
    let remote = argv.get(i + 1..)?;
    if remote.is_empty() {
        None
    } else {
        Some(remote.to_vec())
    }
}

// `basename` moved to `argv`: it is not a file path it cuts, it is a PROGRAM
// NAME it normalises — sub-shell parenthesis, quotes, the separator of both
// platforms, and a case-insensitive `.exe`. `post_passes::artifacts` asks for
// it since it left the dispatcher, and there is no reason for an argv
// primitive to live in a remote-commands module.

/// The keyword that opens the BODY of a bash control-flow construct — the
/// part that runs the real, output-producing command. `case` and `select`
/// are deliberately absent: their bodies are a list of alternatives with no
/// single program to attribute the output to.
/// Tokens that CLOSE or punctuate a shell construct rather than start a
/// command. The connector scan must stop at one and must never resume ONTO
/// one: on `if git diff --quiet; then echo ok; fi`, resuming past the `ok;`
/// lands on `fi`, which produces nothing and is not the command that did.
pub(super) fn is_control_marker(name: &str) -> bool {
    matches!(
        name,
        "then" | "do" | "done" | "fi" | "else" | "elif" | "esac" | "}"
    )
}

/// The keyword that ENDS a construct, with or without the `;` that a
/// whitespace split glues to it (`done;`).
pub(super) fn is_closing_marker(name: &str) -> bool {
    matches!(name.trim_end_matches(';'), "done" | "fi" | "esac" | "}")
}

/// Index of the command after the closer at `i` (`done; tail` / `done && tail`),
/// or `None` when no command follows it — `if …; then echo ok; fi` keeps its
/// `echo`, it does not become `fi`.
pub(super) fn after_closer(argv: &[String], i: usize) -> Option<usize> {
    let tok = argv.get(i)?;
    if !is_closing_marker(&basename(tok)) {
        return None;
    }
    let next = if tok.ends_with(';') {
        i + 1
    } else if argv.get(i + 1).is_some_and(|t| is_connector(t)) {
        i + 2
    } else {
        return None;
    };
    (next < argv.len()).then_some(next)
}

/// Index just past the assignment at `i`, whose VALUE may span several
/// whitespace tokens: `PG=$(grep -m1 x .env | cut -d= -f2-) && python3 run.py`.
/// Taking one token left `-m1` in program position — measured 2026-10-02 by
/// `attribution`: `-m1`, `-d`, `-c`, `-f`, `-s` and four `name=$(prog` named
/// as commands. Quotes and parentheses are tracked across tokens; an
/// unterminated value consumes the rest, which then holds no program.
pub(super) fn assignment_end(argv: &[String], i: usize) -> usize {
    let (mut depth, mut sq, mut dq, mut bt) = (0i32, false, false, false);
    let mut j = i;
    while let Some(tok) = argv.get(j) {
        let mut escaped = false;
        for c in tok.chars() {
            if escaped {
                escaped = false;
                continue;
            }
            match c {
                '\\' if !sq => escaped = true,
                '\'' if !dq && !bt => sq = !sq,
                '"' if !sq && !bt => dq = !dq,
                '`' if !sq => bt = !bt,
                '(' if !sq && !dq => depth += 1,
                ')' if !sq && !dq => depth -= 1,
                _ => {}
            }
        }
        j += 1;
        if depth <= 0 && !sq && !dq && !bt {
            return j;
        }
    }
    argv.len()
}

/// A chain separator as its own token.
pub(super) fn is_connector(tok: &str) -> bool {
    matches!(tok, "&&" | "||" | ";")
}

pub(super) fn loop_body_keyword(name: &str) -> Option<&'static str> {
    match name {
        "for" | "while" | "until" => Some("do"),
        "if" => Some("then"),
        _ => None,
    }
}

/// Advance past shell scaffolding that is not the output-producing program,
/// returning the index of the first token that is one (or `argv.len()` when
/// there is none). Handles, repeatedly and in any order:
///
///   - env-assignment tokens (`LANG=C cargo build` → `cargo`);
///   - a `cd … &&` / `;` / `||` prefix (`cd dir && real-cmd` → `real-cmd`);
///   - bash control-flow headers (`for f in *.log; do grep x "$f"; done`
///     → `grep`).
///
/// The control-flow case is the same lever as the `cd` one. A loop is its
/// body's command run N times, so the bytes it emits have the BODY's shape,
/// not a shape of its own — `for` had the highest tokens-per-event of any
/// bulk row in field data while getting only the generic fold.
/// `split_chain_segments` deliberately refuses to split these constructs
/// (their `;` are structural), so the whole loop arrives here as one argv
/// and used to route + report under the bare keyword.
///
/// Shared by the router (`setup_filter_state`) and the telemetry labeller
/// (`program_basename`) so route and label can never diverge — the C4 audit
/// invariant. Wrapper unwrapping (`sudo`/`timeout`/…) is NOT done here: the
/// router handles it with its own recursing match arm.
/// Leading commands that cannot own the bytes of a chain they start.
///
/// `cd dir && <cmd>` was handled here from the start, for a reason that is not
/// specific to `cd`: the output of `A && B` belongs to whichever of the two
/// actually emits it, and `cd` emits nothing. The rest of this list emits
/// nothing, or one line — never enough to out-weigh what follows.
///
/// What this actually costs, measured 2026-08-04: running `echo start && git
/// status` recorded `op=echo`, `label="echo start"` in `gain.jsonl` for output
/// that was entirely `git status`'s. The bytes were still folded — the damage
/// is that route and label name the wrong command. That telemetry is the only
/// input to the coverage-gap counters in `~/.tokenade/cmdstats`, which is why
/// the field showed `echo.count`, `sleep.count`, `seq.count`, `which.count`
/// there: shell primitives recorded as programs whose compactor is missing.
/// Advice built on that data points at the wrong command.
///
/// This is about naming the right route, NOT about a token win: whether the
/// specific compactor or the generic pass folds a given output harder varies
/// by input, and picking the smaller of the two is a different question this
/// does not answer.
///
/// Deliberately excluded: `rm`/`cp`/`mv`/`chmod`. They are usually silent too,
/// but when they are not, the line they print is a diagnostic the user needs,
/// and handing it to the next command's format-specific compactor risks eating
/// it. The gain does not justify that.
pub(crate) fn is_silent_prefix_prog(name: &str) -> bool {
    matches!(
        name,
        "cd" | "pushd"
            | "popd"
            | "echo"
            | "printf"
            | "true"
            | "false"
            | ":"
            | "export"
            | "unset"
            | "set"
            | "umask"
            | "source"
            | "."
            | "sleep"
            | "mkdir"
            | "touch"
            | "alias"
            | "unalias"
            // A guard (`[ -d out ] && ls out`, `test -f x && cat x`) writes
            // nothing: its exit code is the whole answer. Attributed to `[`,
            // the row named the condition instead of the command that ran
            // (2026-10-03: `[` 103 events on the fleet).
            | "["
            | "[["
            | "test"
            // Loop control prints nothing: `[ "$n" = 0 ] && break; sleep 5;
            // done; tail log` was booked under `break;` (129 commands in this
            // machine's transcripts, 2026-10-05).
            | "break"
            | "continue"
            // Job control prints nothing: `cmd > log 2>&1 & disown; sleep 2;
            // curl …`.
            | "disown"
            | "wait"
            // One line about the session, then the command that does the work:
            // `pwd && find crates -name x` and `date; ls -la …` were named `pwd` /
            // `date` (fleet 30 d: `pwd` 787 k tokens at 1 % coverage; this machine:
            // 57 + 148 such chains, 2026-10-05). Alone, each keeps its own name.
            | "pwd"
            | "date"
    )
}

pub(super) fn skip_shell_scaffolding(argv: &[String], mut i: usize) -> usize {
    // Every branch below strictly increases `i`, so this terminates; the
    // bound is a defensive backstop, not load-bearing.
    for _ in 0..=argv.len() {
        let assigned_from = i;
        while i < argv.len() && is_env_assignment_token(&argv[i]) {
            i = assignment_end(argv, i);
        }
        let Some(tok) = argv.get(i) else {
            return argv.len();
        };
        // A command group opens on `{` and runs what follows it: `cmd || {
        // grep x log; tail -2 log; }` was booked under `{` (726 commands in
        // this machine's transcripts, 2026-10-05). Its `}` already closes like
        // `done`.
        // A subshell runs what follows its `(` just the same: `for h in …; do (
        // yt-dlp … ) & done` was named by the empty word before `yt-dlp` (54
        // real commands of this machine's transcripts, 2026-10-05).
        if tok == "{" || tok == "(" {
            i += 1;
            continue;
        }
        // An assignment-only clause (`S=/tmp/x && cp a b`): the connector after
        // it is not a program. Measured 2026-10-02 on the fleet: `&&` reported
        // as a command 249 times.
        if i > assigned_from && is_connector(tok) {
            i += 1;
            continue;
        }
        let name = basename(tok);
        // A construct that CLOSED (`…; do sleep 3; done; tail -20 log`): what
        // follows the closer is the next command, the one whose output the
        // agent reads. Measured 2026-10-02 on the fleet: `done;` reported as a
        // command 530 times — every polling loop the agent ends with a `tail`.
        // A silent prefix with nothing to say and its `;` glued on (`done; echo;
        // grep x log`): the clause is the token itself, the command is next.
        if tok.len() > 1
            && tok.ends_with(';')
            && is_silent_prefix_prog(name.trim_end_matches(';'))
            && i + 1 < argv.len()
        {
            i += 1;
            continue;
        }
        if is_closing_marker(&name) {
            match after_closer(argv, i) {
                Some(next) => {
                    i = next;
                    continue;
                }
                None => return i,
            }
        }
        // Jump past the connector to the command that produced the output.
        // A bare `cd /somewhere` (no connector) stays attributed to `cd`.
        if is_silent_prefix_prog(&name) {
            // The connector is not always its own token. `cd /x && cmd` splits
            // clean, but `cd /x; cmd` — just as idiomatic — glues the `;` to
            // the path (`/x;`), and a newline separator vanishes entirely when
            // argv is whitespace-split. Both left the row attributed to `cd`,
            // a command that writes nothing: MEASURED on the real ledger
            // 2026-08-25, 34 rows carrying 108 425 tokens of `before`. The
            // dashboard groups savings by this field to say "where the savings
            // come from", so it was naming the navigation instead of the work.
            // Scan forward for the connector, and STOP at a shell keyword.
            //
            // The prefix's own arguments come first and there can be several
            // (`mkdir -p out && git status`), so the search cannot be bounded
            // to a fixed width. But it must not run past the prefix's clause
            // either: on `cd /x` followed by `for f in a b; do …` an unbounded
            // search matched the `b;` of the loop HEADER and resumed at `do`.
            // A keyword is exactly the boundary — a prefix never takes one as
            // an argument, so reaching one means the clause already ended, and
            // the keyword IS the next command (a lost newline separated them).
            //
            // Two connector spellings, because the separator is not always its
            // own token: whitespace-splitting glues `;` to the preceding word
            // (`cd /x; cmd` → `/x;`). MEASURED on the real ledger 2026-08-25:
            // 34 rows carrying 108 425 tokens of `before` were attributed to
            // `cd`, a command that writes nothing, and the dashboard groups by
            // this field to say "where the savings come from".
            let mut k = i + 1;
            let mut resumed = false;
            while let Some(t) = argv.get(k) {
                let n = basename(t);
                if loop_body_keyword(&n).is_some() {
                    i = k;
                    resumed = true;
                    break;
                }
                // A marker CLOSES the clause: everything the prefix could own
                // is behind us and nothing ahead is a command.
                if is_control_marker(&n) {
                    break;
                }
                if is_connector(t) || (t.len() > 1 && t.ends_with(';')) {
                    // …and never resume onto a marker either.
                    let next = argv.get(k + 1).map(|t| basename(t)).unwrap_or_default();
                    // A closer is a boundary too — unless a command FOLLOWS it
                    // (`do sleep 3; done; tail log`), in which case that
                    // command is the one to resume on.
                    if next.is_empty()
                        || (is_control_marker(&next) && after_closer(argv, k + 1).is_none())
                    {
                        break;
                    }
                    i = k + 1;
                    resumed = true;
                    break;
                }
                k += 1;
            }
            if resumed {
                continue;
            }
            // Nothing after it at all: a bare `cd /some/where` really is a `cd`.
            return i;
        }
        // Jump to the token after `do` / `then`. The keyword may carry a
        // trailing `;` once argv is whitespace-split (`…; do` splits clean,
        // but `…;do` does not). A construct with no body keyword in argv
        // (truncated / malformed) stays attributed to the keyword.
        if let Some(open) = loop_body_keyword(&name) {
            match argv
                .iter()
                .skip(i + 1)
                .position(|t| t.trim_end_matches(';') == open)
            {
                Some(j) => {
                    i = i + 1 + j + 1;
                    continue;
                }
                None => return i,
            }
        }
        return i;
    }
    i.min(argv.len())
}

/// The bare program name `filter_command` routes on: the basename of the
/// first non-env-assignment token (`["LANG=C", "/usr/bin/git", "status"]`
/// → `"git"`). Arguments and paths are dropped, so the result is an
/// anonymous, secret-free command identifier safe to report. Empty string
/// for an empty argv.
/// Wrapper programs whose output IS that of the command they wrap.
///
/// The dispatcher's wrapper arm in `filter_command` spells the same names as
/// LITERALS, and must: `build.rs` reads those literals into
/// `FILTER_ARM_PROGS`. The two lists used to be kept equal by a comment; the
/// test `every_declared_wrapper_is_routed_by_what_it_wraps` now does it.
pub(super) const WRAPPER_PROGS: &[&str] = &[
    "sudo",
    "sshpass",
    "timeout",
    "nice",
    "ionice",
    "chrt",
    "env",
    "nohup",
    "setsid",
    "eatmydata",
    "command",
];

pub(super) fn is_wrapper_prog(name: &str) -> bool {
    WRAPPER_PROGS.contains(&name)
}

/// Does this wrapper's own flag take a SEPARATE value?
///
/// A wrapper's leading flags are skipped when resolving the program position,
/// and the flag's VALUE was not: `sshpass -p <password> ssh host uptime`
/// resolved the PASSWORD as the program. That name is not cosmetic — it keys
/// compactor dispatch, it becomes the `cmd` field of a ledger row, it is
/// written as a FILENAME under `~/.tokenade/cmdstats/`, and it is quoted back
/// to the agent by the missing-compactor nudge. Reproduced 2026-08-28 and
/// found in real traffic: 100 calls in one corpus of 25 515 carried a
/// `sshpass -p` password in the program position.
///
/// Only the separated form matters. `-pSECRET` and `--password=SECRET` are one
/// token and are skipped whole by the flag rule itself.
pub(super) fn wrapper_flag_takes_value(wrapper: &str, flag: &str) -> bool {
    let f = flag.trim_start_matches('-');
    if f.is_empty() {
        // `--` ends the options; it carries no value of its own.
        return false;
    }
    match wrapper {
        "sshpass" => matches!(f, "p" | "f" | "d" | "P" | "password" | "file" | "fd"),
        "sudo" => matches!(
            f,
            "u" | "g"
                | "p"
                | "h"
                | "C"
                | "r"
                | "t"
                | "T"
                | "U"
                | "D"
                | "user"
                | "group"
                | "prompt"
                | "host"
                | "close-from"
                | "role"
                | "type"
                | "other-user"
                | "command-timeout"
                | "chdir"
        ),
        "timeout" => matches!(f, "k" | "s" | "kill-after" | "signal"),
        "nice" => matches!(f, "n" | "adjustment"),
        "ionice" => matches!(
            f,
            "c" | "n" | "p" | "P" | "u" | "class" | "classdata" | "pid" | "uid"
        ),
        "chrt" => matches!(f, "p" | "T" | "P" | "pid"),
        "env" => matches!(f, "u" | "S" | "C" | "unset" | "split-string" | "chdir"),
        _ => false,
    }
}

/// Advance past a wrapper's own flags, their values, and its numeric argument.
///
/// THE single declarer of "where does the wrapped program start". It was
/// written twice — once for routing (`filter_wrapped`) and once for the
/// label/telemetry name (`program_index`) — and the comment on the routing
/// copy already said the two "MUST agree". They agreed on the flag and on the
/// bare number because each defect was fixed twice; they would have had to
/// agree a third time on the flag's value.
pub(super) fn skip_wrapper_prefix(wrapper: &str, argv: &[String], mut i: usize) -> usize {
    while i < argv.len() {
        let tok = argv[i].as_str();
        if tok.starts_with('-') {
            i += 1;
            if wrapper_flag_takes_value(wrapper, tok) && i < argv.len() {
                i += 1;
            }
            continue;
        }
        if is_bare_number(tok) {
            i += 1;
            continue;
        }
        break;
    }
    i
}

/// Basename of the program that produced the output, for telemetry/labelling.
///
/// Mirrors `filter_command`'s routing exactly: skip leading `FOO=bar`
/// env-assignment tokens, and unwrap wrapper programs the same way
/// `filter_wrapped` does (`sudo pgrep …` is attributed to `pgrep`, not
/// `sudo`). Without this, wrapped commands would be mis-attributed to the
/// wrapper and the coverage flag — computed from the *inner* command's
/// compactor — would not match the reported command name.
/// A token that is only a number — optionally fractional, optionally with a
/// time-unit suffix. `timeout`'s duration, `nice`'s priority. Never a program.
pub(super) fn is_bare_number(tok: &str) -> bool {
    let t = tok.trim_matches(|c| c == '\'' || c == '"');
    let t = t.strip_suffix(['s', 'm', 'h', 'd']).unwrap_or(t);
    if t.is_empty() {
        return false;
    }
    let mut dots = 0;
    t.chars().all(|c| {
        if c == '.' {
            dots += 1;
            return dots == 1;
        }
        c.is_ascii_digit()
    })
}
