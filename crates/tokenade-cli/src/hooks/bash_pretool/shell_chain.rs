//! **A command line is not ONE command.**
//!
//! A `&&`, a `;`, a `|` make several of it, and a variable set in the first
//! segment is read in the second — wrapping the segments separately would
//! break the chain. Everything that decides whether a line may be REWRITTEN
//! is here, and each rule exists because a rewrite broke something: a peeled
//! `cd` that took the directory away with it, a lost session variable, a
//! script lifted out of its shell.

use super::*;

/// True when a bare-assignment segment (`VAR=…` with no command word)
/// defines a variable that a LATER segment references (`$VAR` / `${VAR`).
/// Such chains must not be split-wrapped: the assignment is unexported and
/// invisible to the wrapped segments' child shells.
pub(crate) fn chain_has_cross_segment_var_use(
    segments: &[command_rewrite::ChainSegment<'_>],
) -> bool {
    for (i, seg) in segments.iter().enumerate() {
        let text = seg.cmd.trim();
        // Shell FUNCTION definitions have the same cross-segment problem as
        // unexported vars: `f() { …; }; f` — the wrapped later segment runs
        // in a child shell where `f` doesn't exist (hit live during probe
        // scripting). Any definition-looking segment bails the whole chain.
        if text.contains("() {") || text.contains("(){") {
            return true;
        }
        // Collect names from bare-assignment LINES anywhere in the segment.
        // Two shapes matter:
        //   * a pure-assignment SEGMENT (`A=1 B=2`) — the historical case;
        //   * an assignment LINE inside a multi-line segment
        //     (`PH=/x\nB=/y\necho hi`) — hit live: the `;`-split put the
        //     assignments in segment 1 (mixed with a command, so the old
        //     all-tokens check skipped them) while `cat $PH/…` landed in
        //     a separately-wrapped segment 2 where PH didn't exist.
        fn is_assign_tok(tok: &str) -> Option<&str> {
            match tok.split_once('=') {
                Some((name, _))
                    if !name.is_empty()
                        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                        && !name.chars().next().unwrap().is_ascii_digit() =>
                {
                    Some(name)
                }
                _ => None,
            }
        }
        let mut names: Vec<&str> = Vec::new();
        for line in text.lines() {
            let mut toks = line.split_whitespace().peekable();
            // Leading NAME=… run on this line (stop at the first non-assign
            // token: `VAR=1 cmd` scopes to cmd — but a LATER segment can't
            // see it either way only when the line ends there; conservative:
            // count leading assignments regardless, a false bail is safe).
            while let Some(tok) = toks.peek() {
                match is_assign_tok(tok) {
                    Some(n) => {
                        names.push(n);
                        toks.next();
                    }
                    None => break,
                }
            }
        }
        if names.is_empty() {
            continue;
        }
        for later in &segments[i + 1..] {
            // Arithmetic expansion `$((S+1))` references vars WITHOUT `$`;
            // parsing it is not worth the risk — any `$((` after a bare
            // assignment bails conservatively.
            if later.cmd.contains("$((") {
                return true;
            }
            for name in &names {
                if later.cmd.contains(&format!("${name}"))
                    || later.cmd.contains(&format!("${{{name}"))
                {
                    return true;
                }
            }
        }
    }
    false
}

/// True when any segment reads a shell parameter whose value comes from the
/// OUTER shell's live state: `$?`, `$!`, `$$`. Split-wrapping evaluates each
/// segment inside `tokenade wrap`'s FRESH child shell, where `$?` is always 0
/// and `$$` is the wrapper's pid — the chain reports success no matter what
/// the previous segment did.
///
/// Field incident 2026-07-29: `timeout 15 curl … ; echo "exit=$?"` printed
/// `exit=0` while `timeout` had killed curl (124) and swallowed its `-w`
/// line. The agent read "exit=0 but no output" and spent half an hour
/// hunting a phantom compaction bug — the compactor had passed the output
/// through untouched (`passthrough below-5pct 112 → 112`); the rewrite had
/// lied about the status.
///
/// Wrapping the chain as ONE unit keeps these correct (a single child shell
/// evaluates every segment), so the caller falls back to the whole-command
/// wrap instead of skipping compaction entirely.
pub(crate) fn chain_reads_outer_shell_status(
    segments: &[command_rewrite::ChainSegment<'_>],
) -> bool {
    segments.iter().any(|s| {
        let t = s.cmd;
        t.contains("$?") || t.contains("${?") || t.contains("$!") || t.contains("$$")
    })
}

// See call site: `cat a.py b.py` over EDITABLE source files walks the agent
// into the harness's Read-before-Edit refusal. One additive warning per
// session, only when ≥2 source files are catted (single-file cats are often
// display-only; the multi-file "batch read" shape is the trap).

// Auto-lift a BROKEN inline exec script. The agent writes
// `tokenade exec --lang py --script 'x = d['k']'` — sh would end the arg at
// the first inner apostrophe and hand exec a mutilated argv (observed:
// a 230-line script re-emitted whole after the failure, +55% on that run).
// The hook sees the RAW string pre-shell, so the intended script is fully
// recoverable when `--script '` starts a span whose LAST `'` ends the
// command: lift it to a temp file and rewrite to `--script-file`.
// Conservative: fires only when the span CONTAINS an inner apostrophe
// (i.e. sh would actually break) and nothing but whitespace follows the
// closing quote. Anything ambiguous passes through untouched.

/// `TOKENADE_SESSION=<skey> ` prefix for rewritten commands — the wrap
/// child is a separate process with no hook payload; without this its
/// gain rows are sessionless and invisible to the session ticker (the
/// owner's ticker sat at 0 because his gains flowed through wrap).
/// skey is alnum-only, safe unquoted.
///
/// It carries the session's context PRESSURE too, for the same reason: every
/// rewritten `wrap` is that child. Only the one-unit arm used to add it, so a
/// chain split into segments — the most common shape: ≈5 600 chains holding a
/// `sed -n` alone in two days of transcripts, measured 2026-10-02 — and a
/// `cd`-peeled payload
/// were judged at the full thresholds of a fresh session.
pub(crate) fn session_env_prefix(parsed: &serde_json::Value) -> String {
    let pf = crate::hooks::session_lifecycle::export_pressure(parsed);
    let pressure = if pf < 1.0 {
        format!("TOKENADE_PRESSURE={pf:.2} ")
    } else {
        String::new()
    };
    let session = crate::hooks::session_id_of(parsed)
        .map(|sid| {
            let skey: String = sid
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(40)
                .collect();
            if skey.is_empty() {
                String::new()
            } else {
                format!("TOKENADE_SESSION={skey} ")
            }
        })
        .unwrap_or_default();
    format!("{pressure}{session}")
}

pub(crate) fn exec_script_lift(cmd: &str) -> Option<String> {
    if !cmd.trim_start().starts_with("tokenade ") || !cmd.contains(" exec ") {
        return None;
    }
    let flag = "--script '";
    let i = cmd.find(flag)?;
    let span_start = i + flag.len();
    let rest = &cmd[span_start..];
    let last_q = rest.rfind('\'')?;
    if !rest[last_q + 1..].trim().is_empty() {
        return None; // args after the closing quote — ambiguous, hands off
    }
    let script = &rest[..last_q];
    if !script.contains('\'') {
        return None; // sh parses this fine — don't churn
    }
    // DELIBERATE sh quoting idioms: `'"'"'` and `'\''` are how a competent
    // author embeds an apostrophe in a single-quoted arg — sh handles those
    // CORRECTLY, and lifting the raw span would corrupt a working command.
    if script.contains("'\"'\"'") || script.contains("'\\''") {
        return None;
    }
    // Substitution risk: with quoting broken, `$(…)`/backtick fragments may
    // sit in an UNQUOTED gap where sh would expand them; whether the agent
    // wanted expansion is unknowable — hands off.
    if script.contains("$(") || script.contains('`') {
        return None;
    }
    // The head must be EXACTLY the exec invocation — a chain in front
    // (`cd x && tokenade exec …`) or env prefix means our span math no
    // longer owns the command shape.
    let head = &cmd[..i];
    if head.contains("&&") || head.contains("||") || head.contains(';') || head.contains('|') {
        return None;
    }
    let dir = tokenade_core::platform::home_dir()?
        .join(".tokenade")
        .join("exec-scripts");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!(
        "{}.script",
        tokenade_core::dedup_ref::hash_content(script.as_bytes())
    ));
    std::fs::write(&path, script).ok()?;
    // Windows: the rewritten command runs under git-bash where raw
    // backslashes are escapes — normalize to forward slashes and quote.
    let path_str = path.display().to_string().replace('\\', "/");
    let fixed = format!("{}--script-file \"{path_str}\"", &cmd[..i]);
    tokenade_core::debug_log::log(
        "hookwrap-pretool",
        "exec_script_lifted",
        serde_json::json!({"bytes": script.len(), "file": path.display().to_string()}),
    );
    Some(fixed)
}

pub(crate) fn cat_edit_precondition_nudge(parsed: &serde_json::Value, cmd: &str) -> Option<String> {
    const SRC_EXT: &[&str] = &[
        ".py", ".rs", ".js", ".ts", ".tsx", ".jsx", ".go", ".java", ".rb", ".c", ".h", ".cpp",
        ".cs", ".php", ".swift", ".kt",
    ];
    // Read the CLAUSE that does the work, not the first token of the line.
    // `cd repo && cat a.ts b.ts` is the same command as `cat a.ts b.ts`, and
    // asking the first token made it invisible — the `cd`-as-program class
    // this file has now paid for three times (routing, the compactor-gap
    // nudge, here). Measured 2026-09-05 across this machine's transcripts:
    // 118 multi-source `cat` clauses, 91 seen and **27 missed, 20 of them
    // behind a leading `cd`**. Scoping the token count to the clause is the
    // other half: counting over the whole line let a later `grep b.ts` push a
    // single-file `cat` over the two-file bar.
    let segment = tokenade_core::command_rewrite::meaningful_segment(cmd);
    let mut toks = segment.split_whitespace();
    let head = toks.next()?;
    if head.rsplit('/').next().unwrap_or(head) != "cat" {
        return None;
    }
    let n_src = toks
        .filter(|t| !t.starts_with('-'))
        .filter(|t| {
            let low = t.to_ascii_lowercase();
            SRC_EXT.iter().any(|e| low.ends_with(e))
        })
        .count();
    if n_src < 2 {
        return None;
    }
    let session = crate::hooks::session_id_of(parsed).unwrap_or("nosess");
    let skey: String = session
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(40)
        .collect();
    let marker = super::read_staging::read_pending_dir().join(format!("catedit-{skey}"));
    if marker.exists() {
        return None;
    }
    tokenade_core::platform::ensure_parent_dir(&marker);
    let _ = std::fs::write(&marker, b"1");
    Some(
        "[tokenade] Heads-up: `cat` output does NOT satisfy the Edit tool's \
         Read-first precondition — if you plan to Edit any of these files, \
         Edit will be refused and you'll have to Read each one anyway (paying \
         for the content twice). Read the files you intend to modify; keep \
         `cat` for reference-only files."
            .to_string(),
    )
}

/// Build the command Claude Code should run INSTEAD of what the agent typed.
///
/// Either every chain segment wrapped on its own, or the whole command
/// wrapped once — the choice is what the rationale below explains.
///
/// EACH wrap-eligible segment with its own `tokenade wrap` so each
/// gets its proper per-command compactor instead of the old
/// single-wrap-on-argv[0] behaviour that misdispatched everything
/// after the first segment.
///
///   cargo build && cargo test  →
///   tokenade wrap 'cargo build' && tokenade wrap 'cargo test'
///
/// Shell semantics preserved (`&&` short-circuits on failure;
/// `||` runs on failure; `;` always runs). Falls back to the
/// whole-command wrap when:
///   - split returns None (heredocs, $(), backticks, eval, process
///     substitution, control-flow keywords like `for`/`if`/`case`,
///     unbalanced quotes/parens) — too complex to safely re-split.
///   - only one segment after splitting — nothing to split-wrap.
///   - all segments are unwrap-eligible (shell-state modifiers like
///     `cd /tmp && cd /home`) — wrap nothing, leave the cmd alone.
///   - a segment reads `$?` / `$!` / `$$` — wrap's child shell resets
///     them, so the split would report a status the chain never had.
///     Falls through to the whole-command wrap, which keeps them exact.
pub(crate) fn extract_first_meaningful_word(cmd: &str) -> &str {
    // Strip comment lines from the front. A bash comment is a
    // line whose first non-whitespace char is `#` (and is not
    // inside quotes — but we don't try to track quoting here;
    // false-positives on a literal `#` inside a quote are
    // benign because the same line still gets parsed downstream).
    let mut rest = cmd;
    loop {
        let trimmed = rest.trim_start();
        if trimmed.starts_with('#') {
            // Skip to the end of this line.
            match trimmed.find('\n') {
                Some(nl) => rest = &trimmed[nl + 1..],
                None => return "",
            }
        } else {
            rest = trimmed;
            break;
        }
    }
    // Quote/paren-aware: respects `$(...)`, `${...}`, backticks,
    // single/double quotes so `CUTOFF=$(($(date +%s)-1)); tail …`
    // yields `tail`, not the `+%s)` fragment a naive whitespace
    // split would produce. Same helper backs segment_must_not_be_wrapped.
    command_rewrite::first_program_token(rest)
}

/// A chain whose later segment reads a variable an earlier one set cannot be
/// wrapped SEGMENT BY SEGMENT: the bare assignment stays in the outer shell,
/// unexported, and the wrapped segment's child shell never sees it — a file
/// once landed at `/` because `$S` expanded to nothing (field incident
/// 2026-07).
///
/// That is a constraint on SPLITTING, and the code answered it by excluding the
/// command from compaction entirely. Wrapping the whole chain as ONE unit keeps
/// the variable, because assignment and use then live in the same child shell.
/// Measured 2026-08-26 on this machine's own log: 394 commands excluded for
/// this reason, **276 of which do not begin with `cd`**.
///
/// The one thing one-unit wrapping cannot do is let a segment affect the OUTER
/// shell — a `cd` whose new directory must persist, an `export`, a `source`.
/// `segment_must_not_be_wrapped` is the existing list of exactly those, so this
/// asks it rather than keeping a second one.
///
/// Says nothing about how to wrap: the `_` arm below already IS the one-unit
/// path (replay, pressure, `cd`-peel and the deferred gates included), so this
/// only decides that the chain belongs there.
pub(crate) fn one_unit_keeps_the_variable(segments: &[command_rewrite::ChainSegment<'_>]) -> bool {
    chain_has_cross_segment_var_use(segments)
        && segments
            .iter()
            .all(|s| !command_rewrite::segment_must_not_be_wrapped(s.cmd))
}

/// Last-chance peel: `cd DIR <sep> <payload>` where the payload's
/// quotes/`$()`/pipes made `split_chain_segments` bail, or where the split path
/// refused it outright. Field data: 0.74M chars of missed compaction, and every
/// `bypass_lost` entry shared this exact shape. The `cd` stays in the OUTER
/// shell (cwd persistence intact); the payload is wrapped as ONE unit, no
/// further splitting.
///
/// Extracted 2026-08-27 so the cross-segment-variable refusal can reach it too
/// rather than carry a second copy of thirty lines of emission.
/// Returns true when it emitted a decision.
pub(crate) fn try_cd_peel(
    cmd: &str,
    exe: &str,
    parsed: &serde_json::Value,
    started: std::time::Instant,
    payload_kind: PeelPayload,
) -> bool {
    let Some((cd_prefix, payload)) = peel_leading_cd_for(cmd, payload_kind) else {
        return false;
    };
    if command_rewrite::segment_must_not_be_wrapped(payload)
        || bash_segment_excluded_from_wrap(payload)
        || is_interactive_tty_invocation(payload)
        || is_continuous_stream(payload)
    {
        return false;
    }
    crate::hooks::note_wrapped_segment(
        crate::hooks::session_id_of(parsed).unwrap_or("nosess"),
        cmd,
        payload,
    );
    let rewritten = format!(
        "{cd_prefix} {}{exe} wrap {}",
        session_env_prefix(parsed),
        posix_quote(payload)
    );
    let mut hso = serde_json::json!({
        "hookEventName": "PreToolUse",
        "permissionDecision": "allow",
        "permissionDecisionReason": "tokenade auto-wrap for compaction",
        "updatedInput": super::updated_input(parsed, &rewritten)
    });
    if let Some(nudge) = loop_guard_nudge(parsed, cmd) {
        // The plain-rewrite route books this and THIS one did not, so the same
        // nudge was overhead on one path and free on the other. Measured
        // 2026-09-06: 59.1% of bash commands (39 484 of 66 827) are
        // `cd`-prefixed, so the unbooked route was the majority one.
        crate::log_emitted_additional_context("hookwrap-pretool-cdpeel", "PreToolUse", &nudge);
        hso["additionalContext"] = serde_json::Value::String(nudge);
    }
    println!("{}", serde_json::json!({ "hookSpecificOutput": hso }));
    tokenade_core::debug_log::log(
        "hookwrap-pretool",
        "bash_rewritten",
        serde_json::json!({
            "prog": "cd-peel",
            "original_len": cmd.len(),
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
    true
}

pub(crate) fn rewrite_for_wrap(
    cmd: &str,
    exe: &str,
    parsed: &serde_json::Value,
    prog: &str,
    started: std::time::Instant,
    gates_deferred: bool,
) -> Result<String, i32> {
    let read_chain = tokenade_core::disclose::reads_only_source_files(cmd);
    let rewritten = match command_rewrite::split_chain_segments(cmd) {
        Some(segments)
            if segments.len() > 1
                && !chain_reads_outer_shell_status(&segments)
                && !one_unit_keeps_the_variable(&segments)
                // A chain of reads (`cat a.ts; echo ===; sed -n 40,90p b.ts`)
                // is ONE read, judged against the read threshold as a whole —
                // `filter` serves it as written below it. Split here, each
                // statement was judged alone and a `cat` of 3 000 tokens lost
                // its middle to `cat-large`: measured 2026-10-02, the 1.1.22
                // promise held through `tokenade wrap '<chain>'` and never
                // through this hook, which is the path agents take.
                && !read_chain =>
        {
            // Cross-segment shell variables: `S=/path; git diff > $S/f` —
            // the bare assignment stays UNWRAPPED (runs in the outer shell,
            // unexported) while the later segment runs inside `tokenade
            // wrap`'s CHILD shell, which never sees $S → silently empty
            // expansion (field incident 2026-07: a file landed at "/"). If
            // any bare-assignment segment's variable is referenced by a
            // later segment, skip the rewrite entirely.
            if chain_has_cross_segment_var_use(&segments) {
                // Reached only when `one_unit_keeps_the_variable` said no —
                // i.e. some segment needs the OUTER shell. When that segment
                // is a LEADING `cd`, the peel below still applies: the `cd`
                // stays outside, the rest becomes one unit, and the variable
                // never crosses a shell boundary. 38 of the 61 commands this
                // branch refused on a 2 500-call replay were exactly that.
                if try_cd_peel(cmd, exe, parsed, started, PeelPayload::Chain) {
                    return Err(0);
                }
                tokenade_core::debug_log::log(
                    "hookwrap-pretool",
                    "bash_excluded",
                    serde_json::json!({
                        "prog": prog,
                        "reason": "cross-segment-shell-var",
                        "elapsed_ms": started.elapsed().as_millis() as u64,
                    }),
                );
                return Err(0);
            }
            let any_wrappable = segments.iter().any(|s| {
                !command_rewrite::segment_must_not_be_wrapped(s.cmd)
                    && !bash_segment_excluded_from_wrap(s.cmd)
            });
            if !any_wrappable {
                // All segments are shell-state modifiers — wrapping
                // would change semantics (subshell loses cd/export
                // effect). Leave the original cmd unchanged and skip
                // the hook entirely.
                tokenade_core::debug_log::log(
                    "hookwrap-pretool",
                    "bash_chain_all_unwrappable",
                    serde_json::json!({
                        "prog": prog,
                        "segments": segments.len(),
                        "elapsed_ms": started.elapsed().as_millis() as u64,
                    }),
                );
                return Err(0);
            }
            let mut out = String::with_capacity(cmd.len() + 64 * segments.len());
            for seg in &segments {
                let seg_text = seg.cmd.trim();
                if command_rewrite::segment_must_not_be_wrapped(seg_text)
                || bash_segment_excluded_from_wrap(seg_text)
                // Deny-audit (live repro): TOKENADE_HOOK_DISABLED=1 in a
                // NON-FIRST segment was wrapped anyway — the env only
                // reached the child command, not the wrap process, so
                // the promised one-shot escape silently compacted.
                || crate::wrap::cmdline_requests_hook_bypass(seg_text)
                {
                    out.push_str(seg_text);
                } else {
                    // Audit C (critical): chained segments — the MOST common
                    // shape — were rewritten without the session env, making
                    // the session ticker blind to them.
                    crate::hooks::note_wrapped_segment(
                        crate::hooks::session_id_of(parsed).unwrap_or("nosess"),
                        cmd,
                        seg_text,
                    );
                    out.push_str(&session_env_prefix(parsed));
                    out.push_str(exe);
                    out.push_str(" wrap ");
                    out.push_str(&posix_quote(seg_text));
                }
                if let Some(sep) = seg.separator_after {
                    out.push(' ');
                    out.push_str(sep);
                    out.push(' ');
                }
            }
            out
        }
        _ => {
            // A chain of reads is one unit, and its leading `cd` still stays in
            // the agent's shell, where its directory must persist.
            let payload = if read_chain {
                PeelPayload::Chain
            } else {
                PeelPayload::Single
            };
            if try_cd_peel(cmd, exe, parsed, started, payload) {
                return Err(0);
            }
            if gates_deferred {
                // The early-exit was deferred expecting per-segment rescue;
                // the split didn't happen, so honor the gate now instead of
                // whole-wrapping an excluded/streaming/interactive command.
                tokenade_core::debug_log::log(
                    "hookwrap-pretool",
                    "bash_excluded",
                    serde_json::json!({
                        "prog": prog,
                        "reason": "deferred-gate-no-split",
                        "elapsed_ms": started.elapsed().as_millis() as u64,
                    }),
                );
                return Err(0);
            }
            // REPLAY: a whitelisted read-only command whose result is
            // provably unchanged (write-gen + state fingerprint + fresh,
            // same-session, no compaction since) skips re-execution
            // entirely — see hooks/replay.rs for the guard stack.
            {
                let session = crate::hooks::session_id_of(parsed).unwrap_or("nosess");
                let skey: String = session
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric())
                    .take(40)
                    .collect();
                let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                let replay_ctx = crate::hooks::ctx_replay_key(parsed, &skey);
                if let Some(rw) =
                    crate::hooks::replay::try_replay_rewrite(&replay_ctx, &cwd, cmd, exe)
                {
                    rw
                } else {
                    // Adaptive thresholds ride in the env prefix, as on every
                    // other arm.
                    format!(
                        "{}{exe} wrap {}",
                        session_env_prefix(parsed),
                        posix_quote(cmd)
                    )
                }
            }
        }
    };
    Ok(rewritten)
}

#[cfg(test)]
mod lift_tests {
    use super::exec_script_lift;

    #[test]
    fn adversarial_lift_gates() {
        // LIFTS: the report-pdf shape — bare apostrophes inside, span ends the command.
        let broken = "tokenade exec --lang python --script 'd = {'North America': 5}\nprint(d['North America'])'";
        assert!(
            exec_script_lift(broken).is_some(),
            "accidental inner apostrophes must lift"
        );
        // NO LIFT: no inner apostrophe — sh handles it.
        assert!(exec_script_lift("tokenade exec --lang python --script 'print(1)'").is_none());
        // NO LIFT: valid sh escape idiom '\"'\"' — command already works.
        let idiom = "tokenade exec --lang python --script 'print('\"'\"'hi'\"'\"')'";
        assert!(
            exec_script_lift(idiom).is_none(),
            "sh idiom must pass through"
        );
        // NO LIFT: trailing chain after the closing quote.
        for tail in ["&& echo ok", "|| echo ko", "; ls", "| head -2"] {
            let c = format!("tokenade exec --lang python --script 'd['k']' {tail}");
            assert!(exec_script_lift(&c).is_none(), "trailing {tail} must bail");
        }
        // NO LIFT: command substitution / backtick inside the span.
        assert!(exec_script_lift("tokenade exec --lang sh --script 'x = '$(id)' y'").is_none());
        assert!(exec_script_lift("tokenade exec --lang sh --script 'x = '`id`' y'").is_none());
        // NO LIFT: chained head before the exec invocation.
        assert!(
            exec_script_lift("cd /x && tokenade exec --lang python --script 'd['k']'").is_none()
        );
        // LIFT with pipes/semicolons INSIDE the span (script text, our sandbox
        // runs it as python not sh): allowed, bytes preserved verbatim.
        let piped = "tokenade exec --lang python --script 'print('a | b; c && d')'";
        let fixed = exec_script_lift(piped).expect("inner pipes are script text");
        assert!(fixed.contains("--script-file"), "{fixed}");
        // $VAR inside span (no parens/backtick): literal in the sandbox — lift ok.
        assert!(exec_script_lift("tokenade exec --lang python --script 'p = '$HOME'/x'").is_some());
    }
}
