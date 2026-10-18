//! `tokenade raw` — running a command WITHOUT compaction.
//!
//! A last resort, so it is counted: the bypass counter is what tells a user
//! their agent has been paying full price for a habit. Known-safe classes are
//! refused outright, with the recovery path named.

use super::*;

/// `tokenade raw <command…>` — escape hatch when a compactor mangles output
/// or you need the verbatim bytes for one call. Symmetric counterpart of
/// `tokenade proxy`: runs the command **without** compaction, sets
/// `TOKENADE_HOOK_DISABLED=1` for the child so any nested hook invocations
/// (e.g. an agent that spawns `cargo` which itself spawns `rustc`) also
/// bypass, and logs the event to `gain.jsonl` with filter label `raw` so
/// the dashboard shows you when bypasses happen.
///
/// Aliases: `bypass`, `noproxy` — pick whichever reads best in your shell
/// history.
/// How `tokenade raw <…>` reads its arguments: `(shell_line, program)`.
///
/// ONE argument holding a whole command line — `tokenade raw 'grep -r x .'` —
/// is run through the shell. Every other tokenade command that takes a command
/// takes it quoted (`tokenade wrap '<cmd>'`), and the rules we scaffold write it
/// that way, so that is what an agent actually types. Read as argv it used to
/// fail with `failed to run 'grep -r x .': No such file or directory` — an error
/// naming the command as though it did not exist — and it slipped past the
/// safe-class refusal, which reads argv[0] and saw the whole line.
///
/// A path that genuinely is a program with a space in its name still wins: the
/// `exists()` check runs first.
///
/// `program` is what the safe-class refusal must judge: the command, never the
/// line.
pub(crate) fn raw_invocation(rest: &[String]) -> (Option<String>, String) {
    let shell_line = (rest.len() == 1
        && rest[0].contains(char::is_whitespace)
        && !std::path::Path::new(&rest[0]).exists())
    .then(|| rest[0].clone());
    let program = match &shell_line {
        Some(line) => line.split_whitespace().next().unwrap_or("").to_string(),
        None => rest[0].clone(),
    };
    (shell_line, program)
}

/// Run a command WITHOUT compaction because TOKENADE ITSELF asked for it.
///
/// Reached when `tokenade wrap` finds [`INTERNAL_SUBPROCESS_ENV`] set, which
/// only [`internal_command`] sets: some layer between tokenade and the program
/// it wanted — a PATH shim left by a version before 0.9.5, a wrapper script, a
/// shell function — has turned our own plumbing into a wrapped command.
///
/// Two things must not happen then, and both did. MEASURED 2026-09-04 with such
/// a `git` first on PATH: one `tokenade pack` booked `wrap / git-ls-files
/// 8059 -> 249` plus a `recovery:offered` row — a saving on 2,699 file paths no
/// agent was ever going to receive — and pack's own answer came back different,
/// because it had parsed the FOLD instead of the file list.
///
/// Not [`cmd_raw`]: that is the USER's escape hatch and owes them its
/// safe-class refusal, its bypass counter and its nudge. This is tokenade
/// getting out of its own way, so it counts nothing and says nothing.
///
/// [`INTERNAL_SUBPROCESS_ENV`]: tokenade_core::process_util::INTERNAL_SUBPROCESS_ENV
/// [`internal_command`]: tokenade_core::process_util::internal_command
pub(crate) fn run_uncompacted_for_tokenade(rest: &[String]) -> i32 {
    let (shell_line, program) = raw_invocation(rest);
    let mut cmd = match &shell_line {
        Some(line) => {
            let (sh, flag) = tokenade_core::platform::shell_command();
            let mut c = Command::new(sh);
            c.arg(flag).arg(line);
            c
        }
        None => {
            let mut c = Command::new(&rest[0]);
            c.args(&rest[1..]);
            c
        }
    };
    cmd.stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());
    tokenade_core::process_util::configure_no_window(&mut cmd);
    tokenade_core::process_util::configure_own_group(&mut cmd);
    tokenade_core::process_util::configure_die_with_parent(&mut cmd);
    match cmd.status() {
        Ok(st) => st.code().unwrap_or(1),
        Err(e) => {
            eprintln!("tokenade: failed to run '{program}': {e}");
            127
        }
    }
}

pub(crate) fn cmd_raw(rest: &[String]) -> i32 {
    // #635 — same fix as cmd_proxy / cmd_wrap. `raw` is the explicit
    // "no compaction" escape hatch — when a user types `--help`
    // alone they want docs, not a failed spawn of `--help`. Also
    // covers `bypass` / `noproxy` aliases (same function).
    if crate::help_routing::asks_for_help_leading(rest) {
        println!(
            "usage: tokenade raw <command> [args...]\n       tokenade raw '<command line>'\n  aliases: bypass, noproxy\n\nBoth forms work: bare argv, or a single quoted string, which is run\nthrough the shell so pipes and redirections behave.\n\nRun a command WITHOUT going through tokenade's compaction layer. Sets\nTOKENADE_HOOK_DISABLED=1 so the PreToolUse / PostToolUse hooks bypass\nthe command too.\n\nFOR VERBATIM BYTES YOU DON'T NEED THIS. If you already ran the command\nthrough tokenade and need the raw bytes (a specific JSON shape, a diff\nwith full context, a single error line lost to dedup), call\n`tokenade expand-ref <hash>` — the hash is printed in every\ncompactor banner. Instant, no re-run.\n\n`tokenade raw` is for the cases expand_ref can't cover: re-executing a\ncommand WITHOUT compaction (capturing stderr a banner dropped, running\ninteractively, debugging the compactor itself).\n\nSafe-class refusal (#1031): for commands whose output tokenade folds\nwell (grep, rg, ag, find, ls, wc, head, tail, awk, sed, cat) tokenade\nrefuses the bypass and points at expand_ref, because re-running spends\nthe original call's tokens a second time. Those folds DO drop lines —\nmeasured on 1 200 lines of grep-shaped output, grep came back as 2 and\ncat/sed/awk as 154 — and expand_ref returns the stash, which is what\nthe fold produced. For the exact original bytes, read the source the\ncommand read, or override with `TOKENADE_FORCE_BYPASS=1 tokenade raw …`.\n\nExit code matches the inner command. Output streams inherited — no\ncapture, no log entry in gain.jsonl (bypass count would always be 0/0).\n\nBypass-nudge (#856): each bypass — `tokenade raw` AND any hook\nshort-circuited via `TOKENADE_HOOK_DISABLED=1` — bumps a 24h\ncount in ~/.tokenade/bypass-counter. Once the count crosses 10,\neach further bypass prints a 1-line stderr reminder pointing at\nexpand_ref. The reminder cannot be silenced.\n\nFlags:\n  --help, -h    show this message and exit"
        );
        return 0;
    }
    if rest.is_empty() {
        eprintln!("usage: tokenade raw <command> [args...]   (or: tokenade raw '<command line>')");
        return 2;
    }
    // #1031 — pre-bypass refusal for safe-class commands. The refusal
    // is kept: re-running a command that already answered burns the
    // original call's tokens a second time, and `expand-ref` is
    // usually enough.
    //
    // What is NOT kept is the reason it used to give. It claimed these
    // compactors were "lossless at line granularity" — grep merely
    // stripping a common path prefix, the rest passthrough. Measured on
    // 1 200 lines / 80 370 bytes of grep-shaped output:
    //   grep            ->     318 bytes,   2 lines
    //   cat / sed / awk ->   3 922 bytes, 154 lines
    // Between 87 % and 99.8 % of the lines are gone. And the pointer it
    // offered instead cannot close the gap: the stash holds what the
    // fold PRODUCED, so on a 25 933-byte input `expand-ref --raw` gave
    // back 16 179 bytes with the dedup collapse already baked in.
    //
    // A refusal is the one message that must not overstate, because it
    // is the door it closes. It now says what the fold does and names
    // the two routes that do return verbatim bytes.
    //
    // Override: `TOKENADE_FORCE_BYPASS=1 tokenade raw grep …`.
    let (shell_line, program) = raw_invocation(rest);
    if let Some(msg) = safe_class_bypass_refusal(&program) {
        eprintln!("{msg}");
        return 2;
    }
    let mut raw_cmd = match &shell_line {
        Some(line) => {
            let (sh, flag) = tokenade_core::platform::shell_command();
            let mut c = Command::new(sh);
            c.arg(flag).arg(line);
            c
        }
        None => {
            let mut c = Command::new(&rest[0]);
            c.args(&rest[1..]);
            c
        }
    };
    raw_cmd
        .env("TOKENADE_HOOK_DISABLED", "1")
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());
    // stdio is inherited: keep the child attached to a REAL terminal (a human
    // wants its interactive output). Only suppress the window when there is no
    // terminal — a GUI/hook context where a flash would be pure noise.
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        tokenade_core::process_util::configure_no_window(&mut raw_cmd);
    }
    // `raw` runs the user's command exactly as `wrap` does, so it owes the same
    // guarantee: cancelling it must cancel the work. Conditional, because this
    // path INHERITS stdio precisely so a human keeps an interactive terminal —
    // and there the terminal already signals the whole foreground group.
    tokenade_core::process_util::configure_own_group(&mut raw_cmd);
    tokenade_core::process_util::configure_die_with_parent(&mut raw_cmd);
    let mut child = match raw_cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("tokenade raw: failed to run '{}': {e}", program);
            return 127;
        }
    };
    let reaper = if !tokenade_core::process_util::terminal_owns_the_group() {
        tokenade_core::process_util::forward_termination_to(child.id());
        tokenade_core::process_util::guard_group_against_our_death(child.id())
    } else {
        None
    };
    let status = child.wait().map(exit_code_of).unwrap_or(1);
    tokenade_core::process_util::stop_forwarding_termination();
    if let Some(r) = reaper {
        r.stand_down();
    }
    // Don't log to gain.jsonl: raw bypass is unmeasurable by construction
    // (stdout was inherited to the terminal, never captured) so every entry
    // would be 0/0 — pure noise that drags dashboard averages down. The
    // bypass count is recoverable from shell history / `tokenade debug`
    // if needed; the ledger is for savings, not invocations.
    //
    // #856 — but DO track a count for the bypass-nudge: when an agent
    // bypasses heavily it's usually defensive habit, not a real need.
    // Emit a stderr footer past the threshold so the agent notices.
    // `tokenade raw` is a TERMINAL command: its stderr is in front of the
    // caller's eyes. The message is therefore really delivered, and the
    // firing is counted — unlike the hook path, where it was not.
    if maybe_emit_bypass_nudge().is_some() {
        book_bypass_delivered(1);
    }
    status
}

/// #1031 — testable classifier. Returns `Some(message)` if the bypass
/// should be refused, `None` to allow. Override env var
/// `TOKENADE_FORCE_BYPASS` (any truthy value) suppresses the refusal.
///
/// `cmd` may be a path (`/usr/bin/grep`) or bare name (`grep`) —
/// matches on basename, case-insensitive. Strips a `.exe` suffix for
/// Windows symmetry.
pub(crate) fn safe_class_bypass_refusal(cmd: &str) -> Option<String> {
    if tokenade_core::env_compat::is_truthy("TOKENADE_FORCE_BYPASS", "STS_FORCE_BYPASS") {
        return None;
    }
    let basename = std::path::Path::new(cmd)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(cmd)
        .trim_end_matches(".exe")
        .to_ascii_lowercase();
    if !SAFE_CLASS_FOR_BYPASS_REFUSAL.contains(&basename.as_str()) {
        return None;
    }
    Some(format!(
        "[tokenade] refusing bypass for `{basename}` — re-running it would spend the \
         original call's tokens again.\n  \
         Most of what you want is already recoverable: `tokenade expand-ref <hash>` \
         (hash from the compactor's banner) returns what was stashed, instantly and \
         without re-executing.\n  \
         That stash is what the FOLD produced, not the raw stream — a banner reading \
         `omits middle lines`, `outline only` or `schema summary` means whole lines are gone from it. For the exact \
         original bytes, read the source the command read, or bypass on purpose: \
         TOKENADE_FORCE_BYPASS=1 tokenade raw {basename} …"
    ))
}

/// #856 — bypass-nudge footer. Tracks bypasses (both `tokenade raw`
/// AND `TOKENADE_HOOK_DISABLED=1`-gated hook early-returns) in
/// `~/.tokenade/bypass-counter` on a 24h rolling window. Returns
/// `Some(count)` when the call crossed the threshold and the caller
/// should emit a stderr nudge; `None` otherwise. The nudge cannot be
/// silenced.
///
/// We only emit the stderr message from explicit `tokenade raw`
/// invocations — env-var bypasses are session-level opt-outs where
/// stderr spam per-hook-call would be noise. The COUNT still includes
/// env-var bypasses, so the next `tokenade raw` nudge reflects total
/// recent bypass volume.
pub(crate) fn bump_bypass_counter() -> Option<u64> {
    let home = tokenade_core::platform::home_dir()?;
    let now = tokenade_core::platform::now_unix_secs();
    bump_bypass_counter_at(&home, now)
}

/// Testable inner of `bump_bypass_counter` — `home` is the directory
/// where `.tokenade/bypass-counter` will live; `now` is the current
/// unix epoch. Lets tests use a tempdir + a synthetic clock without
/// touching the process environment.
/// #856 / #862 — pure formatter for the `bypass_count` healthcheck
/// entry. Two branches: past-threshold (nudge firing), then plain count.
/// Age renders as `<1h` when the window is fresh (< 1h old) instead of `0h`.
pub(crate) fn format_bypass_count_check(count: u64, age_secs: u64) -> String {
    // When the window itself has expired (age > 24h), the count is
    // stale: the next bump resets window_start AND count to fresh
    // values, so neither "in window of 168h" nor "past threshold;
    // nudge is firing" is true at that moment. Show a dedicated
    // expired message instead — and skip the threshold suffix even
    // when count >= 10, because the nudge will NOT fire on the next
    // bypass (it'll reset to 1).
    let expired = age_secs > BYPASS_NUDGE_WINDOW_SECS;
    // The expired case needs its own SENTENCE, not its own value. Feeding the
    // note into the `in window of {…}` slot printed, verbatim on a real
    // install: "20 bypass(es) (…) in window of expired (24h window passed;
    // next bypass resets)". A health line the reader has to re-parse costs
    // more than the fact it carries.
    if expired {
        return format!(
            "{count} bypass(es) (raw + TOKENADE_HOOK_DISABLED) — 24h window expired, so this \
             count is stale; the next bypass resets it"
        );
    }
    let age_str = if age_secs < 3600 {
        "<1h".to_string()
    } else {
        format!("{}h", age_secs / 3600)
    };
    let suffix = if count >= BYPASS_NUDGE_THRESHOLD {
        " — past threshold; nudge is firing on every bypass past 10"
    } else {
        ""
    };
    format!("{count} bypass(es) (raw + TOKENADE_HOOK_DISABLED) in window of {age_str}{suffix}")
}

/// #856 — `tokenade evict-stale` calls this to drop the
/// `bypass-counter` file when its 24h window has expired. Returns
/// `Ok(true)` when the file was removed, `Ok(false)` when there was
/// nothing to do (no file, unparseable contents, or still inside the
/// window), `Err` on filesystem error. Pure-enough: only the `home`
/// argument touches disk, and only the file at `<home>/.tokenade/
/// bypass-counter` is read/removed.
pub(crate) fn prune_stale_bypass_counter(
    home: &std::path::Path,
    now: u64,
) -> std::io::Result<bool> {
    let path = home.join(".tokenade").join("bypass-counter");
    let Ok(s) = std::fs::read_to_string(&path) else {
        return Ok(false);
    };
    let Some(ws) = s
        .split_whitespace()
        .next()
        .and_then(|t| t.parse::<u64>().ok())
    else {
        return Ok(false);
    };
    if now.saturating_sub(ws) <= BYPASS_NUDGE_WINDOW_SECS {
        return Ok(false);
    }
    std::fs::remove_file(&path)?;
    Ok(true)
}

pub(crate) fn bump_bypass_counter_at(home: &std::path::Path, now: u64) -> Option<u64> {
    let path = home.join(".tokenade").join("bypass-counter");
    let prev = std::fs::read_to_string(&path).ok().and_then(|s| {
        let mut it = s.split_whitespace();
        let ws: u64 = it.next()?.parse().ok()?;
        let c: u64 = it.next()?.parse().ok()?;
        Some((ws, c))
    });
    let (new_state, should_nudge) = bypass_nudge_step(now, prev, BYPASS_NUDGE_THRESHOLD);
    let _ = std::fs::create_dir_all(home.join(".tokenade"));
    // Atomic write: tmp + rename so a Ctrl-C or crash mid-write can't
    // leave a truncated file. The parser tolerates truncation (silent
    // skip → fresh window on next bump), but rotating reliably also
    // means concurrent `tokenade raw` processes can't observe a
    // partial state. Pattern mirrors `gain::rotate_if_oversized`.
    let body = format!("{} {}\n", new_state.0, new_state.1);
    // 0600 from the start (#988 + #990) AND a temp name no other process
    // shares: every wrapped command touches this counter, so two of them
    // running at once is the ordinary case, not the exotic one. The shared
    // declarer owns both halves — see `platform::write_atomic_600`.
    let _ = tokenade_core::platform::write_atomic_600(&path, body.as_bytes());
    if should_nudge {
        Some(new_state.1)
    } else {
        None
    }
}
