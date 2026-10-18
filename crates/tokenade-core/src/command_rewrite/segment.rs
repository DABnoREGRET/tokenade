//! **What IS a segment, and which program does it run?**
//!
//! A segment is not always a command: `set -e` is decoration, `echo
//! '=== build ==='` is a label, `FOO=1 BAR=2 cmd` hides its program
//! behind two assignments, and `cd build && …` carries a prefix that
//! produces nothing.
//!
//! Each of these shapes, taken for a command, would announce a
//! saving on an empty output — or worse, would wrap something
//! that must not be wrapped. Hence `segment_must_not_be_wrapped`, which is
//! an explicit refusal and not a missing rule.

use super::*;

/// Bash control-flow OPENERS. Only these are safe to look for when deciding
/// whether a chain can be split — see [`has_bash_control_flow`].
pub const SHELL_OPENERS: &[&str] = &["if", "for", "while", "until", "case", "select", "function"];

/// The rest of the control-flow vocabulary. Read together with
/// [`SHELL_OPENERS`] when the question is "is this token a program name",
/// which it never is; kept apart because the split-safety question must NOT
/// look at them (`echo done` is an ordinary command).
pub const SHELL_TERMINATORS: &[&str] = &[
    "then", "else", "elif", "fi", "do", "done", "esac", "in", "time", "coproc",
];

/// Could this token be the name of a program?
///
/// A route name is written to disk as a FILENAME and quoted back to the agent,
/// so a token that cannot be a program must not become one. Measured
/// 2026-08-28 over 25 515 real agent commands: of 219 distinct names the
/// missing-compactor telemetry produced, **128 were not programs** —
/// `done;`, `{`, `clean(){`, `-t`, `$B`, `n=$(grep`, `X11;` — covering 839
/// of 3 431 occurrences.
pub fn is_plausible_program_name(tok: &str) -> bool {
    let t = tok.trim_matches(|c| c == '\'' || c == '"');
    let Some(first) = t.chars().next() else {
        return false;
    };
    if !(first.is_ascii_alphanumeric() || first == '_' || first == '/') {
        // A leading `.` is a program only in `./script` form; `.kiro` is a
        // dotted directory that reached the program position from a chain we
        // could not split.
        if !(first == '.' && t.contains('/')) {
            return false;
        }
    }
    if !t
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "_./+-@:".contains(c))
    {
        return false;
    }
    !SHELL_OPENERS.contains(&t) && !SHELL_TERMINATORS.contains(&t)
}

/// The first segment of a (possibly compound) command that is a real,
/// output-producing program — skipping leading state modifiers (`cd`,
/// `export`, `set`, …) and `tokenade` wrappers. Used to route + LABEL a
/// chained command by the command whose output the agent actually reads:
/// `cd /tmp && grep x f` → `grep x f` (not `cd`), `mkdir d && cd d && cmake ..`
/// → `cmake ..`. Falls back to the whole command when it can't split safely
/// (heredoc / subshell / background-`&` / bash control-flow), so the behaviour
/// degrades to "route on the first token" rather than guessing wrong.
pub fn meaningful_segment(cmd: &str) -> &str {
    let clause = meaningful_clause(cmd);
    // A shell launcher is not the command either: `bash -c 'cargo test'` and
    // `sh scripts/build.sh` print what the command and the script print. Named
    // `bash`/`sh`, they were routed to no compactor and booked under a program
    // that does no work — 1.7 M tokens on the fleet over 30 days (2026-10-05).
    match through_shell_launcher(clause, false) {
        Some(inner) if inner.len() < clause.len() => meaningful_segment(inner),
        _ => clause,
    }
}

/// The program a command line runs, by name — what the ledger, the dashboard
/// and the missing-compactor nudge call it. One resolver for every caller:
/// `wrap`, the hook and the bench each spelled `meaningful_segment` +
/// `program_basename` themselves, so a fix to one was a fix to one.
///
/// A program held in a variable (`B=./target/release/acme-cli; $B draft list`)
/// is named by the value assigned to it earlier in the same line: 898 real
/// commands in this machine's transcripts ran their program as `$B`, `$PY`,
/// `$SRV`… (2026-10-05), and the ledger booked them under the variable.
pub fn program_name(cmd: &str) -> String {
    // A command that opens on a line continuation (`\⏎sed -i …`): the `\` is
    // not a word. 44 real commands were named `""` for it (2026-10-05).
    let cmd = cmd
        .trim_start()
        .strip_prefix('\\')
        .map_or(cmd, str::trim_start);
    // `p(){ echo "== $1"; sed -n "$2p" "$1"; }; p a.rs 1,20; p b.rs 5,9`: the
    // definition is not a command, and `p` is named by what its body runs.
    // 70 real commands were named `p(){`, `az()`, `}`… (2026-10-05).
    if let Some((rest, defs)) = without_function_definitions(cmd) {
        let name = program_name(&rest);
        return match defs.iter().find(|(n, _)| *n == name) {
            Some((_, body)) => match program_name(body) {
                b if b.is_empty() => name,
                b => variable_program(cmd, &b).unwrap_or(b),
            },
            None => name,
        };
    }
    let seg = meaningful_segment(cmd);
    let argv: Vec<String> = seg.split_whitespace().map(String::from).collect();
    let name = crate::filter::program_basename(&argv);
    // `bash -c 'make' | tail -5` stays whole for ROUTING — the compactor of
    // `make` must not read what `tail` cut — but it is `make` that ran: 110
    // real commands of this machine's transcripts (`bash -c '. env.sh && win
    // …' 2>&1 | head`) were booked under `bash` (2026-10-05).
    if let Some(inner) = through_shell_launcher(seg, true).filter(|i| i.len() < seg.len()) {
        let inner_name = program_name(inner);
        if !inner_name.is_empty() {
            return variable_program(cmd, &inner_name).unwrap_or(inner_name);
        }
    }
    // `time cargo build` is ROUTED to `time`'s own spec (it prints the
    // timing), but it is `cargo` that ran: 56 real commands were booked under
    // `time` (2026-10-05).
    if name == "time" {
        let rest = seg
            .trim_start()
            .strip_prefix("time")
            .unwrap_or("")
            .trim_start();
        let rest = rest.strip_prefix("-p").map_or(rest, str::trim_start);
        let inner_name = program_name(rest);
        if !inner_name.is_empty() {
            // `B=./target/release/x; time $B run`: the variable is set
            // outside what `time` runs.
            return variable_program(cmd, &inner_name).unwrap_or(inner_name);
        }
    }
    variable_program(cmd, &name).unwrap_or(name)
}

/// `cmd` without its shell function definitions (`name() { … }`, `function
/// name { … }`), and each definition as (name, body). `None` when it has none.
fn without_function_definitions(cmd: &str) -> Option<(String, Vec<(String, String)>)> {
    let head = regex::Regex::new(
        r"(?:^|[\s;&|])(?:function\s+([A-Za-z_][\w-]*)\s*(?:\(\))?|([A-Za-z_][\w-]*)\s*\(\))\s*\{",
    )
    .ok()?;
    let mut rest = String::new();
    let mut defs = Vec::new();
    let mut at = 0;
    while let Some(c) = head.captures(&cmd[at..]) {
        let m = c.get(0)?;
        // Code inside quotes or a heredoc (`node -e "function f() {…}"`) is
        // not the shell's: stop at the first such match.
        if !at_top_level(&cmd[..at + m.start()]) {
            break;
        }
        let name = c.get(1).or_else(|| c.get(2))?.as_str().to_string();
        let open = at + m.end();
        let close = open + matching_brace(&cmd[open..])?;
        let start = at
            + m.start()
            + (m.as_str().len()
                - m.as_str()
                    .trim_start_matches([' ', '\t', '\n', ';', '&', '|'])
                    .len());
        rest.push_str(&cmd[at..start]);
        defs.push((
            name,
            cmd[open..close]
                .trim()
                .trim_end_matches(';')
                .trim()
                .to_string(),
        ));
        at = close + 1;
        // The separator after `}` went with the definition.
        let after = cmd[at..].trim_start_matches([' ', '\t']);
        at = cmd.len() - after.trim_start_matches(';').trim_start_matches("&&").len();
    }
    if defs.is_empty() {
        return None;
    }
    rest.push_str(&cmd[at..]);
    Some((rest, defs))
}

/// Whether the text after `before` is shell, not inside a quote or a heredoc.
fn at_top_level(before: &str) -> bool {
    if before.contains("<<") {
        return false;
    }
    let (mut sq, mut dq, mut esc) = (false, false, false);
    for c in before.chars() {
        match c {
            _ if esc => esc = false,
            '\\' if !sq => esc = true,
            '\'' if !dq => sq = !sq,
            '"' if !sq => dq = !dq,
            _ => {}
        }
    }
    !sq && !dq
}

/// Offset of the `}` that closes a block whose `{` was just consumed; quotes
/// and `${…}` are stepped over by depth alone.
fn matching_brace(s: &str) -> Option<usize> {
    let (mut depth, mut sq, mut dq) = (1i32, false, false);
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\\' if !sq => i += 1,
            b'\'' if !dq => sq = !sq,
            b'"' if !sq => dq = !dq,
            b'{' if !sq && !dq => depth += 1,
            b'}' if !sq && !dq => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn variable_program(cmd: &str, name: &str) -> Option<String> {
    let var = name
        .strip_prefix('$')?
        .trim_start_matches('{')
        .trim_end_matches('}');
    if var.is_empty() || !var.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let used = cmd
        .find(&format!("${var}"))
        .or_else(|| cmd.find(&format!("${{{var}}}")))?;
    let re = regex::Regex::new(&format!(
        r#"(?:^|[\s;&|(]){var}=("[^"]*"|'[^']*'|[^\s;&|)]+)"#
    ))
    .ok()?;
    let value = re
        .captures_iter(&cmd[..used])
        .last()?
        .get(1)?
        .as_str()
        .trim_matches(['"', '\'']);
    // Only the program word must be literal: `S="ssh -i $HOME/k host"` runs
    // `ssh` (19 real commands named `$S`, 2026-10-05).
    let word = value.split_whitespace().next()?;
    if word.contains('$') || word.contains('`') {
        return None;
    }
    let base = word.rsplit('/').next()?;
    is_plausible_program_name(base).then(|| base.to_string())
}

/// `bash -c '<cmd>'` → `<cmd>` ; `sh -lc "<cmd>" 2>&1` → `<cmd>` ;
/// `bash scripts/x.sh --all` → `scripts/x.sh --all`. `None` when the launcher
/// reads its program from stdin (`bash <<'EOF'`, `bash -s`), when the quoted
/// command cannot be sliced as typed (`'\''`, escapes), or when something
/// other than a redirection follows it (`bash -c 'x' | tail` stays whole).
fn through_shell_launcher(clause: &str, naming: bool) -> Option<&str> {
    let t = past_wrappers(clause.trim());
    let (head, mut rest) = t.split_once(char::is_whitespace)?;
    let host = head
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(head)
        .to_ascii_lowercase();
    if matches!(
        host.as_str(),
        "powershell" | "powershell.exe" | "pwsh" | "pwsh.exe"
    ) {
        return through_powershell(rest, naming);
    }
    // `cmd /c dir C:\\x`: cmd.exe runs what follows `/c` (or `/k`).
    if matches!(host.as_str(), "cmd" | "cmd.exe") {
        let (flag, after) = rest.trim_start().split_once(char::is_whitespace)?;
        return matches!(flag.to_ascii_lowercase().as_str(), "/c" | "/k" | "/s/c")
            .then(|| after.trim_start())
            .filter(|a| !a.is_empty());
    }
    if !matches!(host.as_str(), "bash" | "sh" | "zsh" | "dash") {
        return None;
    }
    loop {
        rest = rest.trim_start();
        let (tok, after) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        if let Some(flags) = tok
            .strip_prefix('-')
            .filter(|f| !f.is_empty() && !f.starts_with('-'))
        {
            if flags.contains('c') {
                return quoted_command(after.trim_start(), naming);
            }
            if flags.contains('s') {
                return None;
            }
            // `-o pipefail` carries a value.
            rest = if flags.ends_with('o') {
                after.trim_start().split_once(char::is_whitespace)?.1
            } else {
                after
            };
            continue;
        }
        if tok.is_empty() || tok.starts_with('-') || tok.starts_with('<') || tok.starts_with('+') {
            return None;
        }
        return Some(rest);
    }
}

/// `powershell -NoProfile -Command "Get-ChildItem C:\\x"` → the cmdlet line.
/// Measured 2026-10-05 on five real captures from a Windows machine, the host
/// named and routed the command (`powershell`, fleet 30 d: 416 k tokens at 0 %
/// coverage) where the cmdlet's own reader does far better: `Get-ChildItem`
/// 10 005 → 2 953 bytes, `Get-Service` 10 924 → 3 905. PowerShell's flags are
/// whole words, case-insensitive; `-File x.ps1` runs a script.
fn through_powershell(mut rest: &str, naming: bool) -> Option<&str> {
    loop {
        rest = rest.trim_start();
        let (tok, after) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        let flag = tok.strip_prefix('-')?.to_ascii_lowercase();
        match flag.as_str() {
            "command" | "c" => {
                let after = after.trim_start();
                return if after.starts_with(['"', '\'']) {
                    quoted_command(after, naming)
                } else {
                    Some(after).filter(|a| !a.is_empty())
                };
            }
            "file" | "f" => return Some(after.trim_start()).filter(|a| !a.is_empty()),
            "noprofile" | "nop" | "nologo" | "noninteractive" | "noni" | "noexit" | "mta"
            | "sta" => rest = after,
            "executionpolicy" | "ep" | "ex" | "windowstyle" | "w" | "inputformat"
            | "outputformat" | "version" => {
                rest = after.trim_start().split_once(char::is_whitespace)?.1;
            }
            _ => return None,
        }
    }
}

/// `NAME=value`, with nothing quoted or substituted in the name.
fn is_env_assignment_word(w: &str) -> bool {
    w.split_once('=').is_some_and(|(n, _)| {
        !n.is_empty()
            && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !n.starts_with(|c: char| c.is_ascii_digit())
    })
}

/// `timeout 600 nohup env X=1 bash -c '…'` → `bash -c '…'`: the launcher is
/// often behind a wrapper (943 real commands in this machine's transcripts
/// carried one, 2026-10-05), and the wrapper prints nothing of its own.
fn past_wrappers(mut t: &str) -> &str {
    loop {
        let Some((head, rest)) = t.split_once(char::is_whitespace) else {
            return t;
        };
        // `AUTO_GTM_BIN=x bash run.sh`: an assignment prefixes the launcher.
        if is_env_assignment_word(head) {
            t = rest.trim_start();
            continue;
        }
        let name = head.rsplit('/').next().unwrap_or(head);
        if !matches!(
            name,
            "timeout" | "nohup" | "setsid" | "nice" | "ionice" | "env" | "sudo" | "stdbuf"
        ) {
            return t;
        }
        let mut rest = rest.trim_start();
        // The wrapper's own flags, a flag's value, `env`'s assignments, and
        // `timeout`'s duration.
        loop {
            let (tok, after) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            let takes_value = matches!(
                tok,
                "-n" | "-k"
                    | "-s"
                    | "-c"
                    | "-u"
                    | "-g"
                    | "-i"
                    | "-o"
                    | "-e"
                    | "--signal"
                    | "--kill-after"
            );
            let is_duration =
                name == "timeout" && tok.chars().next().is_some_and(|c| c.is_ascii_digit());
            if takes_value {
                rest = after
                    .trim_start()
                    .split_once(char::is_whitespace)
                    .map_or("", |(_, r)| r)
                    .trim_start();
            } else if tok.starts_with('-') || is_duration || (name == "env" && tok.contains('=')) {
                rest = after.trim_start();
                if is_duration {
                    break;
                }
            } else {
                break;
            }
        }
        if rest.is_empty() {
            return t;
        }
        t = rest;
    }
}

/// The command inside `'…'` or `"…"`, if nothing but redirections follow.
fn quoted_command(s: &str, naming: bool) -> Option<&str> {
    let q = s.chars().next().filter(|c| *c == '\'' || *c == '"')?;
    let body = &s[1..];
    // `'\''` is how a single-quoted script writes a quote: it does not end the
    // script. Stopping there left 314 real multi-line `bash -c '…'` scripts in
    // this machine's transcripts named `bash` (2026-10-05).
    let mut close = body.find(q)?;
    while q == '\'' && body[close..].starts_with("'\\''") {
        close += 4 + body[close + 4..].find(q)?;
    }
    let inner = &body[..close];
    // An escape or an expansion inside double quotes makes the sliced text
    // differ from what runs — a ROUTING concern; the program's first word is
    // still the program (`bash -c "cd $D && make"` ran `make`).
    // Only the escapes bash itself processes inside double quotes: a Windows
    // path (`C:\\Windows\\System32`) keeps its backslashes as typed.
    let escaped = ["\\\\", "\\\"", "\\$", "\\`"]
        .iter()
        .any(|e| inner.contains(e));
    if !naming && q == '"' && (escaped || inner.contains('$')) {
        return None;
    }
    let tail = &body[close + 1..];
    if tail.starts_with(|c: char| !c.is_whitespace()) {
        return None;
    }
    // Only redirections may follow: `2>&1`, `>out.log`, or `> /dev/null` in
    // two words.
    let mut words = tail.split_whitespace();
    while let Some(w) = words.next() {
        // Sent to the background: nothing follows that reads its output.
        if w == "&" {
            continue;
        }
        // What the pipe does to the output is the routing's business; the
        // NAME is still the script's.
        if naming && w.starts_with('|') {
            break;
        }
        if !w.contains('>') {
            return None;
        }
        if w.ends_with('>') && words.next().is_none() {
            return None;
        }
    }
    Some(inner.trim())
}

fn meaningful_clause(cmd: &str) -> &str {
    let Some(segs) = split_chain_segments(cmd) else {
        // The scanner refuses scripts (control flow, heredocs, comments); their
        // top-level commands are still found line by line, for naming and
        // routing only — see `lines`.
        let groups = super::lines::top_level_groups(cmd);
        if groups.len() > 1 {
            if let Some(g) = groups.iter().find(|g| {
                !segment_must_not_be_wrapped(g)
                    && !segment_is_a_label(g)
                    && !super::lines::defines_a_function(g)
            }) {
                return meaningful_segment(g);
            }
            // `cd dir⏎tokenade exec … <<'PYEOF'`: the whole text fell through
            // and its heredoc body named it (`for`, 2026-10-05).
            return self_call_or(&groups, cmd);
        }
        return cmd;
    };
    let cmds: Vec<&str> = segs.into_iter().map(|s| s.cmd).collect();
    if let Some(seg) = cmds
        .iter()
        .find(|seg| !segment_must_not_be_wrapped(seg) && !segment_is_a_label(seg))
    {
        return seg;
    }
    self_call_or(&cmds, cmd)
}

/// Every segment was skipped: the self-call among them, else the whole text.
/// `cd … ; tokenade wrap '<cmd>'` is the shape that matters: the caller's note
/// said it "resolves to the tokenade token and is caught by the already-wrapped
/// guard", and it did not — falling back to the whole command made `argv[0]`
/// the leading `cd`, so the chain was compacted again and BOOKED under a
/// command that produces no output. Two such rows are in this machine's
/// ledger. Handing back the self-call lets the guard the note describes see it.
fn self_call_or<'a>(cmds: &[&'a str], cmd: &'a str) -> &'a str {
    cmds.iter()
        .copied()
        .find(|seg| first_program_token(seg.trim()) == "tokenade")
        .unwrap_or(cmd)
}

/// A segment whose output nobody reads: the `echo "=== step 2 ==="` an agent
/// puts in front of the command it actually cares about, a `sleep`, a `:`.
///
/// Separate from [`segment_must_not_be_wrapped`], which answers a different
/// question — wrapping `echo` loses nothing, so putting it there would make
/// that predicate lie. This one only decides which segment a chain is ROUTED
/// and LABELLED by.
///
/// Measured cost of not having it: `tokenade wrap 'echo ===; cat build.log'`
/// folded **4 197 tokens where `cat build.log` alone folded 4 956** — the same
/// bytes, the same stash hash, 15 % worse purely because the chain was routed
/// as an `echo`. Leading-`echo` labelling is among the most common shapes an
/// agent types; `cd` was already skipped here and `echo` never was.
///
/// A chain made only of these falls through to `unwrap_or(cmd)`, so a lone
/// `echo x` is still routed as itself.
pub fn segment_is_a_label(segment: &str) -> bool {
    // `""`: an assignment-only segment (`S=/tmp/y`) — `first_program_token`
    // skips the assignments and finds no program. It is no more read than a
    // label. Measured 2026-10-02: `S=/tmp/y && cp a b && npm test` was routed
    // and labelled by `S=/tmp/y`, i.e. by nothing (`cmd: ""`). Not in
    // `segment_must_not_be_wrapped`: the hook wraps such a chain as ONE unit
    // so the variable survives, and that predicate would forbid it.
    //
    // The programs that print nothing are ONE list, read here and by the
    // word-level resolver (`filter::is_silent_prefix_prog`). There were two,
    // and they had drifted: `disown; tail log` was named `disown` here while
    // the word level skipped it, and `mkdir -p out && cargo build` was routed
    // as `mkdir` (2026-10-05).
    let program = first_program_token(segment.trim());
    program.is_empty() || crate::filter::is_silent_prefix_prog(program)
}

/// True when the first meaningful program of `segment` is a shell-state
/// modifier that MUST NOT be wrapped with `tokenade wrap` — wrapping
/// would put it in a child shell where its effect (changing directory,
/// exporting a variable, sourcing a file) would be lost before the
/// next segment runs.
///
/// Also returns true for empty segments and segments that already
/// invoke `tokenade` (avoid double-wrap).
/// Does this `pgrep`/`pkill` invocation match against the FULL command line?
///
/// True for `-f` in any of the shapes a person actually types: `-f`, bundled
/// (`-af`, `-fa`), or the long form. Anything after a `--` separator is a
/// pattern, not a flag.
pub(crate) fn matches_by_full_command_line(segment: &str) -> bool {
    for tok in segment.split_whitespace().skip(1) {
        if tok == "--" {
            return false;
        }
        if tok == "--full" {
            return true;
        }
        if let Some(flags) = tok.strip_prefix('-') {
            if !flags.starts_with('-') && flags.contains('f') {
                return true;
            }
        }
    }
    false
}

pub fn segment_must_not_be_wrapped(segment: &str) -> bool {
    let t = segment.trim();
    if t.is_empty() {
        return true;
    }
    // Quote/paren-aware to handle `LANG=$(locale -a) ls` — naive
    // whitespace split breaks the env-assignment token and falsely
    // reports `-a)` as the program.
    let bare = first_program_token(t);
    // A command that selects processes BY COMMAND LINE sees the wrapper's own
    // command line, because `tokenade wrap '<cmd>'` contains `<cmd>` verbatim.
    // Reported from the field 2026-08-03 and reproduced here: through the
    // wrapper, `pgrep -af "sleep 987"` lists tokenade's own process as a match.
    // For `pgrep` that is a false positive; for `pkill -f` it is a signal sent
    // to something the user never named — the reporter lost their own waiting
    // loops to `pkill -f "scratchpad/*.py"` (exit 144).
    //
    // Only the `-f`/full-command-line forms are affected: matching by process
    // NAME (`pgrep sleep`, `killall`) never sees an argument list, so those stay
    // wrappable. There is nothing to compact in this output anyway.
    if matches!(bare, "pgrep" | "pkill") && matches_by_full_command_line(t) {
        return true;
    }
    bare == "tokenade"
        || matches!(
            bare,
            // Directory & environment state — wrapping these puts them in
            // a subshell, losing their effect on the parent shell.
            "cd" | "pushd"
            | "popd"
            | "export"
            | "unset"
            | "source"
            | "."
            | "set"
            | "alias"
            | "unalias"
            | "shopt"
            | "declare"
            | "typeset"
            | "local"
            | "readonly"
            | "let"
            // Control-flow that must run in the parent shell.
            | "return"
            | "break"
            | "continue"
            | "wait"
            | "trap"
            | "exit"
            // No-op-ish builtins where wrapping is pure overhead.
            | "true"
            | "false"
            | ":"
        )
    // NOTE: `time` is intentionally NOT here. `time cargo build` is
    // safe to wrap (the inner cmd still runs and time prints stats).
    // It routes to the `time` spec (head + tail, so the `real/user/sys`
    // lines survive), NOT to the cargo compactor: `time` is not in
    // `filter::WRAPPER_PROGS`, deliberately — an inner compactor could
    // fold away the timing the agent asked for. (This note claimed the
    // opposite until 2026-10-02.)
}

pub(crate) fn is_env_assignment_token(tok: &str) -> bool {
    // The shell's own rule for a variable name: a letter or `_`, then letters,
    // digits and `_`. CASE IS IRRELEVANT — `f=/tmp/x cat $f` is an assignment
    // prefix exactly as `F=/tmp/x cat $f` is.
    //
    // This demanded UPPERCASE until 2026-08-27, and the cost was not cosmetic.
    // `first_program_token` skips assignment tokens to find the program, so a
    // lowercase prefix left the assignment IN and the program was read out of
    // it: `f=/tmp/x cat $f` reported the program **`x`** — the basename of the
    // VALUE. Everything keyed on that name then answered about a program the
    // command never named. Reproduced end to end through the pretool hook:
    // `vim notes.txt` is correctly left alone, while
    // `p=/tmp/x vim notes.txt` was REWRITTEN into `tokenade wrap` — vim with
    // no TTY, which is the hang the exclude list exists to prevent (#690 notes
    // that a hang there is the whole session, not one call).
    let Some((key, _)) = tok.split_once('=') else {
        return false;
    };
    let mut cs = key.chars();
    match cs.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Extract the leading program token from `cmd`, respecting `$(...)`,
/// `${...}`, backticks and quotes so command substitutions inside an
/// env-prefix don't fragment the token. Skips:
///   * leading `KEY=…` env-assignment tokens (full value, including
///     embedded `$(date +%s)`),
///   * empty / whitespace-only tokens.
///
/// Returns the program token without a path prefix (`/usr/bin/cargo`
/// → `cargo`). Returns `""` when no program is identifiable (pure
/// env-assignment line, blank input, etc.).
///
/// Background: a naive `split_whitespace().find(|t| !is_env_assignment(t))`
/// breaks on `CUTOFF=$(($(date +%s) - 3600)); tail …` because the spaces
/// inside `$(…)` split the env-prefix into fragments — `+%s)` then looks
/// like the "program" to downstream telemetry and the bash-pretool hook
/// excludes/builtins check. Caught ~10 events/day in production debug.log.
pub fn first_program_token(cmd: &str) -> &str {
    // `cd <dir>;` is scene-setting, not the command whose output matters. It
    // was read as the program, and everything keyed on that name then answered
    // about `cd`: MEASURED 2026-09-05 over this machine's transcripts, 45
    // results came back banded `[tokenade:cd]` — a label naming nothing the
    // user ran — at a 3.2 KB median, and NOT ONE of them was ever asked a
    // question (0 `expand-ref --prompt`), while the agent went around them 12
    // times. The real program (`python3`, `grep`, `tokenade exec`) never
    // reached its own compactor, so a generic archetype folded output that had
    // a purpose-built handler waiting.
    //
    // Same shape as the env-assignment skip below, and it closes the same
    // hazard the comment there names: `cd /repo && vim notes.txt` reported
    // `cd`, so the interactive-exclude list never saw `vim`.
    let mut cmd = cmd;
    for _ in 0..8 {
        match strip_leading_cd_clause(cmd) {
            Some(rest) => cmd = rest,
            None => break,
        }
    }
    let bytes = cmd.as_bytes();
    let mut i = 0usize;
    let mut start = 0usize;
    let mut in_squote = false;
    let mut in_dquote = false;
    let mut in_backtick = false;
    let mut depth_paren = 0i32; // $( and (( and (
    let mut depth_brace = 0i32; // ${ and { groups
    while i <= bytes.len() {
        let at_end = i == bytes.len();
        let c = if at_end { 0u8 } else { bytes[i] };

        // Boundary char only at top-level + outside any quote.
        let is_top_level =
            !in_squote && !in_dquote && !in_backtick && depth_paren == 0 && depth_brace == 0;
        let is_sep_byte = matches!(c, b' ' | b'\t' | b'|' | b'&' | b';' | b'\n');
        let is_sep = at_end || (is_top_level && is_sep_byte);

        if is_sep {
            let tok = &cmd[start..i];
            let trimmed = tok.trim();
            if !trimmed.is_empty() && !is_env_assignment_token(trimmed) {
                // Strip leading `!` (history-expand prefix in some shells)
                // and any path component so `/usr/bin/cargo` → `cargo`.
                let no_bang = trimmed.trim_start_matches('!');
                return no_bang.rsplit(['/', '\\']).next().unwrap_or(no_bang);
            }
            if at_end {
                return "";
            }
            i += 1;
            start = i;
            continue;
        }

        match c {
            b'\\' if i + 1 < bytes.len() => i += 2,
            b'\'' if !in_dquote && !in_backtick => {
                in_squote = !in_squote;
                i += 1;
            }
            b'"' if !in_squote && !in_backtick => {
                in_dquote = !in_dquote;
                i += 1;
            }
            b'`' if !in_squote => {
                in_backtick = !in_backtick;
                i += 1;
            }
            b'$' if !in_squote
                && i + 1 < bytes.len()
                && (bytes[i + 1] == b'(' || bytes[i + 1] == b'{') =>
            {
                if bytes[i + 1] == b'(' {
                    depth_paren += 1;
                } else {
                    depth_brace += 1;
                }
                i += 2;
            }
            b'(' if !in_squote && !in_dquote => {
                depth_paren += 1;
                i += 1;
            }
            b')' if !in_squote && !in_dquote && depth_paren > 0 => {
                depth_paren -= 1;
                i += 1;
            }
            b'{' if !in_squote && !in_dquote => {
                depth_brace += 1;
                i += 1;
            }
            b'}' if !in_squote && !in_dquote && depth_brace > 0 => {
                depth_brace -= 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    ""
}

/// If `cmd` opens with a `cd <arg>` clause followed by a top-level separator,
/// return what comes after it.
///
/// `None` when it does not, INCLUDING a bare `cd /somewhere` with nothing
/// after: that one really is the command, and reporting `""` for it would be
/// a different answer, not a better one.
pub(crate) fn strip_leading_cd_clause(cmd: &str) -> Option<&str> {
    let trimmed = cmd.trim_start();
    let rest = trimmed.strip_prefix("cd")?;
    // `cdrom …` is not `cd`.
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    let bytes = trimmed.as_bytes();
    let (mut i, mut sq, mut dq, mut bt) = (2usize, false, false, false);
    let (mut paren, mut brace) = (0i32, 0i32);
    while i < bytes.len() {
        let c = bytes[i];
        let top = !sq && !dq && !bt && paren == 0 && brace == 0;
        if top && (c == b';' || c == b'\n' || (c == b'&' && bytes.get(i + 1) == Some(&b'&'))) {
            let after = i + if c == b'&' { 2 } else { 1 };
            let tail = trimmed[after..].trim_start();
            // Nothing after the separator is the bare case again.
            return (!tail.is_empty()).then_some(tail);
        }
        // A pipe or a single `&` is NOT a clause boundary we may cross: `cd x |
        // tee` is a different sentence, and backgrounding changes what runs.
        if top && (c == b'|' || c == b'&') {
            return None;
        }
        match c {
            b'\\' if i + 1 < bytes.len() => i += 2,
            b'\'' if !dq && !bt => {
                sq = !sq;
                i += 1;
            }
            b'"' if !sq && !bt => {
                dq = !dq;
                i += 1;
            }
            b'`' if !sq => {
                bt = !bt;
                i += 1;
            }
            b'$' if !sq && matches!(bytes.get(i + 1), Some(b'(') | Some(b'{')) => {
                if bytes[i + 1] == b'(' {
                    paren += 1;
                } else {
                    brace += 1;
                }
                i += 2;
            }
            b'(' if !sq && !dq => {
                paren += 1;
                i += 1;
            }
            // A `$(…)` opened INSIDE double quotes has to close inside them
            // too — `cd "$(git rev-parse --show-toplevel)" && make` otherwise
            // never leaves the substitution and the whole clause is missed.
            b')' if !sq && paren > 0 => {
                paren -= 1;
                i += 1;
            }
            b'{' if !sq && !dq => {
                brace += 1;
                i += 1;
            }
            b'}' if !sq && brace > 0 => {
                brace -= 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}
