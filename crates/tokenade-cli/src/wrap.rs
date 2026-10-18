//! `tokenade wrap|proxy|raw` shell-wrapping engine + bypass accounting
//! (ShellKind, spawn_wrapped, fail-open logging, bypass nudges). Extracted
//! from main.rs (refactor split). Holds several cross-module helpers
//! (hook_disabled_by_env) used by the hook modules.
#![allow(unused_imports)]
mod bypass;
mod dedup;
mod range_read;
pub(crate) use range_read::register_read_slice;
pub(crate) mod nudges;
use crate::*;

// `TOKENADE_SESSION` is READ here and never written. A shared lock stood in
// this spot for one iteration, to make the two mutators of that process-global
// mutually exclusive; the mutators are gone — the session is passed through
// `DedupSite::session` — so there is nothing left to serialise.

pub(crate) use bypass::*;
pub(crate) use dedup::*;
pub(crate) use nudges::*;
mod bypass_gate;
mod route;
pub(crate) mod second_stage;
mod serve;
mod spawn;
pub(crate) use bypass_gate::*;
pub(crate) use route::*;
pub(crate) use second_stage::*;
pub(crate) use serve::*;
pub(crate) use spawn::*;

pub(crate) fn cmd_wrap(rest: &[String]) -> i32 {
    let _p_all = tokenade_core::profile::phase("wrap:TOTAL");
    use tokenade_core::process_util::{
        drain_capped_keeping_tail, truncation_banner, DEFAULT_STDERR_CAP, DEFAULT_STDOUT_CAP,
    };
    // #635 — leading `--help` shouldn't reach `sh -c` (which exits 2
    // with a confusing `option non valable`). When `--help` / `-h`
    // is the FIRST positional, show docs; otherwise treat it as
    // shell-passthrough for the inner command.
    if crate::help_routing::asks_for_help_leading(rest) {
        println!(
            "usage: tokenade wrap [--shell <posix|powershell|cmd>] [--split-streams] <shell-cmd>\n\nRun a shell command, apply the per-command compactor to its combined\nstdout+stderr, and emit the compact result. Exit code matches the inner\ncommand. Stdout cap 16 MB, stderr cap 4 MB.\n\n--shell picks the interpreter for the inner command:\n  posix       (default) `sh -c` — Git Bash on Windows\n  powershell  `powershell.exe -Command` (or pwsh)\n  cmd         `cmd.exe /C`\nWith no --shell, the shell is auto-detected from the environment and\nfalls back to posix. Use --shell when wrapping a PowerShell/cmd command\nby hand so its syntax isn't run under bash.\n\nThe Claude Code Bash hook auto-wraps `Bash(git log)` → `tokenade wrap\n'git log'`; the PowerShell hook adds `--shell powershell`.\n\nFlags:\n  --split-streams  compact stdout and stderr separately and return each to\n                   its own stream, so redirections behave as they do without\n                   the wrapper. Chronological interleaving is lost.\n  --help, -h    show this message and exit"
        );
        return 0;
    }
    if rest.is_empty() {
        eprintln!("usage: tokenade wrap [--shell <posix|powershell|cmd>] <shell-cmd>");
        return 2;
    }
    // Tokenade's own plumbing, reached through some interception layer (a PATH
    // shim from before 0.9.5, a wrapper script). Nobody asked for this output
    // and nobody will read it but us: run it, return its code, count nothing.
    // See `run_uncompacted_for_tokenade` for what it cost when we did not.
    if tokenade_core::process_util::running_as_internal_subprocess() {
        return crate::wrap::bypass::run_uncompacted_for_tokenade(rest);
    }
    // Start compiling the redaction regexes NOW, on another thread, so the
    // ~8 ms they cost overlaps with running the command instead of landing
    // between the child exiting and its output reaching the agent. See
    // `redact::warm`.
    //
    // Since 2026-08-08 `main` starts the same warm-up EARLIER, for every
    // content subcommand at once (see `subcommand_touches_content` — this
    // per-entry-point wiring is exactly what left `hookwrap` and `read`
    // paying 8.7 ms each). Kept as a belt-and-braces no-op: `OnceLock` makes
    // the loser of the race free, and it keeps `cmd_wrap` correct if it is
    // ever reached by a route `main` does not classify as content.
    std::thread::spawn(|| {
        tokenade_core::redact::warm();
    });
    // Parsed before anything else touches the shell: a usage error must not
    // spawn a child, and `--help` must not reach `sh -c`.
    let WrapInvocation {
        shell_cmd,
        shell_kind,
        shell_override,
        split_streams,
        ..
    } = match parse_invocation(rest) {
        Ok(inv) => inv,
        Err(rc) => return rc,
    };
    // Resilient self-heal channel: `tokenade wrap` runs for every agent command
    // via the shell proxy (rc files), INDEPENDENT of settings.json — so it can
    // rebuild Claude Code hooks even when a plugin/upgrade wiped every one of
    // them. Throttled (one stat on the hot path; real work ~once/6h). Best-
    // effort, never blocks the command.
    {
        let _p = tokenade_core::profile::phase("wrap:ensure_hooks");
        let _ = ensure_hooks(tokenade_core::platform::home_dir().as_deref(), true);
    }
    // Reassemble the shell command. Callers pass it as one argv slot
    // (single-quoted by the hook) — but a careless caller might split
    // it across multiple slots, so we re-join with spaces. The end
    // result feeds `sh -c`, which re-tokenises it the way the user
    // intended.

    // L78 — Memory-capped capture. The naïve `Command::output()`
    // reads the entire child stdout/stderr into memory: a `cat
    // /var/log/big.log` (multi-GB) or `journalctl -u svc` (~500MB)
    // would OOM tokenade BEFORE any compactor saw the bytes. Cap
    // stdout at 16 MB / stderr at 4 MB, drained in background
    // threads (single-threaded reads risk pipe-blocking when one
    // stream fills its buffer before the other is consumed).
    //
    // When we hit the cap, append a truncation banner so the
    // downstream compactor and the agent both see explicit "more
    // bytes were dropped here." Output beyond the cap is silently
    // discarded; the agent can re-run with `tokenade raw <cmd>` or
    // a slice (head/tail) to see the missing region.
    // Run the inner command under the shell it was written for (Posix /
    // PowerShell / cmd — see spawn_wrapped). The Posix path tries `sh` then a
    // Git-Bash fallback on Windows; PowerShell/cmd never fall back to a POSIX
    // shell (their syntax differs — running them under bash would mangle the
    // command). If the requested shell is missing we surface the error rather
    // than silently running under the wrong one.
    // Fast-path routing (#wrap-direct): when the command is PROVABLY a plain
    // `word word word` invocation (no shell metacharacter, no env-assignment,
    // no builtin, argv[0] on PATH, no explicit `--shell`), skip the `sh -c`
    // fork and exec the binary directly. The captured output feeds the
    // IDENTICAL downstream pipeline below (compaction / gain / dedup / disclose),
    // so the result is byte-equivalent to the shell path — only faster. At the
    // slightest doubt `wrap_route` returns `Shell` (slower, never wrong).
    let _p_spawn = tokenade_core::profile::phase("wrap:spawn+drain");
    let (out, cap_truncated) = match run_child(&shell_cmd, shell_kind, shell_override) {
        Ok(v) => v,
        Err(rc) => return rc,
    };
    drop(_p_spawn);
    let _p_rest = tokenade_core::profile::phase("wrap:after_child");
    // Restore rules a 1.x quota pause retracted from this project (one `stat`
    // when nothing was retracted). Covers agents without a SessionStart hook.
    if let Ok(cwd) = std::env::current_dir() {
        let _ = tokenade_core::project_scaffold::sync_gate_state(&cwd, false);
    }
    if let Some(rc) = serve_binary(&out) {
        return rc;
    }
    if split_streams {
        return serve_split_streams(&out, &shell_cmd, cap_truncated);
    }
    // `text_encoding::decode` exists for exactly these bytes and had never been
    // wired to command output: its own module doc calls `from_utf8_lossy` "the
    // culprit" and lists the encodings it destroys, while `cmd_wrap` used
    // `from_utf8_lossy` on every byte a child ever printed. A French log, an
    // Excel CSV `cat`ted, a PowerShell redirect — all mojibake, and the agent
    // paid tokens for the wreckage. Decode first; lossy stays the last resort
    // for bytes that are genuinely not text in any encoding.
    let decoded = |b: &[u8]| tokenade_core::process_util::decode_child_output_of(b, &shell_cmd);
    let (so_text, so_faithful) = decoded(&out.stdout);
    let (se_text, se_faithful) = decoded(&out.stderr);
    let decode_was_faithful = so_faithful && se_faithful;
    let mut combined = format!("{so_text}{se_text}");
    if cap_truncated {
        combined.push_str(&truncation_banner(DEFAULT_STDOUT_CAP, DEFAULT_STDERR_CAP));
    }
    // Use the FIRST WORD of the shell command (ignoring shell syntax)
    // to pick the per-command compactor. Strip leading comment lines
    // first so `# header\ngrep foo bar` dispatches as `grep`, not as
    // `#` (which would land in the generic fallback).
    let _p_pre = tokenade_core::profile::phase("wrap:pre_filter");
    // A command run to RECOVER folded bytes is served as it printed them.
    // Placed before the dispatch so no part of the pipeline below — compactor,
    // exec-dedup poll answer, §ref fold — can turn the answer back into the
    // pointer that was handed in.
    if recovers_folded_bytes(&shell_cmd) {
        tokenade_core::debug_log::log(
            "wrap",
            "recovery_served_verbatim",
            serde_json::json!({ "bytes": combined.len() }),
        );
        return serve_recovery(&combined, &out);
    }
    let cmd_for_dispatch = filter::strip_leading_comment_lines(&shell_cmd);
    // Compound commands: route + label on the first MEANINGFUL segment, not a
    // leading `cd`/`export`/`set` state-modifier — `cd /tmp && grep x` reports
    // and routes as `grep`. (Claude's PreToolUse already wraps segments
    // individually, but non-Claude agents call `tokenade wrap '<compound>'`
    // directly per the scaffold, so the whole chain lands here.)
    let cmd_for_dispatch = command_rewrite::meaningful_segment(cmd_for_dispatch);
    let argv: Vec<String> = cmd_for_dispatch
        .split_whitespace()
        .map(String::from)
        .collect();
    tokenade_core::user_filters::set_command_failed(exit_code_of(out.status) != 0);
    drop(_p_pre);
    // From here until `r` exists, the command's output is captured, redacted,
    // and NOT yet delivered — the only window in which a fault costs the agent
    // the command it ran. Arm the salvage path with those exact bytes and the
    // child's own exit code; the guard disarms as soon as compaction returns,
    // so the salvage and the normal write can never both fire.
    let r = {
        let _salvage = tokenade_core::crash_guard::arm_salvage(&combined, exit_code_of(out.status));
        // Debug-only fault injection so the salvage path can be proved
        // end-to-end against the real binary. Compiled out of releases.
        #[cfg(debug_assertions)]
        // A native fault: never unwinds, so only the signal handler can
        // answer it. The panic case is injected one layer down, inside
        // `filter_command_fail_open`, which is where a panic is recoverable.
        if std::env::var("TOKENADE_CRASH_PROBE").as_deref() == Ok("abort") {
            std::process::abort();
        }
        tokenade_core::profile::timed("wrap:filter_command", || {
            filter::filter_command_fail_open(&argv, &combined)
        })
    };
    let _p_post = tokenade_core::profile::phase("wrap:after_filter");
    // Classify BEFORE anything else runs: `is_command_specific` consults a
    // per-call thread-local that the next `filter_command` would reset.
    let cmd_name = command_rewrite::program_name(filter::strip_leading_comment_lines(&shell_cmd));
    let cmd_specific = r.is_command_specific();
    // Count this run for the uncovered-command nudge BEFORE any early return
    // (dedup/poll-answer paths below can short-circuit), so a command that
    // recurs is actually tallied. Emits at most once per command per window.
    {
        let _p = tokenade_core::profile::phase("wrap:nudge");
        emit_uncovered_compactor_nudge(
            filter::uncovered_route_name(&argv).as_deref(),
            cmd_specific,
            r.savings.tokens_before,
            r.savings.tokens_after,
        );
    }
    let safe_label = tokenade_core::profile::timed("wrap:redact_label", || {
        redact::redact(&redact::redact_argv(&argv).join(" "))
    });
    let _p_tail = tokenade_core::profile::phase("wrap:pick_and_account");

    // L81 — anti-inflation guard via shared helper. See
    // `tokenade_core::process_util::pick_compact_or_raw`.
    let inflated = r.savings.tokens_after > r.savings.tokens_before;
    let (compacted_output, savings_for_log) = tokenade_core::fold_count::pick_compact_or_raw(
        combined.clone(),
        r.output.clone(),
        r.savings,
        || tokenade_core::tokenizer::estimate(&combined),
    );
    // What the fold REMOVED, captured before the logging paths consume the
    // savings. This — not the size of what remains — is what decides whether a
    // recovery pointer is worth a disk write. See `attach_drop_recovery`.
    let dropped_tokens = savings_for_log
        .tokens_before
        .saturating_sub(savings_for_log.tokens_after);
    let before_tokens = savings_for_log.tokens_before;
    let filter_label_for_log = if inflated {
        "passthrough_inflated".to_string()
    } else {
        r.filter.clone()
    };
    // Drop noise-tier entries (mirrors #411 log_mcp + #423/#540 hook path
    // threshold). After `pick_compact_or_raw`, savings == 0 covers BOTH:
    //   * passthrough (no compactor attempted — the filter dispatcher
    //     declined the work for tiny/wrong-shape inputs),
    //   * compactor ran but couldn't reduce (rare; usually tightening
    //     a detector elsewhere is the right fix).
    // #540: drop 1-2 token "saved" micro-events too. The hook path at
    // line ~8300 requires >=5% AND >=10 tokens absolute saved before
    // emitting; cmd_wrap was left on a strict-zero gate, letting 113
    // of 119 grep events through with 1-2 token savings each (pre-fix
    // grep dashboard avg: 50.5%; post-fix: 76.0%). Apply the same
    // threshold here so the dashboard isn't dragged by statistical
    // noise. Debug.log paths still capture the diagnostic if needed.
    // One gate for every write site AND the pruner (`gain::worth_recording`):
    // this copy used to lack the code-intel exemption the others had.
    let worth_logging = tokenade_core::gain::worth_recording(
        savings_for_log.tokens_before as u64,
        savings_for_log.tokens_after as u64,
        &filter_label_for_log,
    );
    if worth_logging {
        let _p_gain = tokenade_core::profile::phase("wrap:gain_record");
        let _p_gain = tokenade_core::profile::phase("wrap:gain_record_cmd");
        // The one place on this path where the RAW bytes are still in hand,
        // so the one place `agent_caps` can be applied at all: it needs the
        // text and its line structure, and a ledger row keeps neither. What
        // gets written is what THIS agent would really have delivered for THIS
        // output — exact where the per-agent scalar was only a bound. Agents
        // the table does not cover leave the field absent and keep the bound.
        let savings_for_log = savings_for_log.with_agent_delivery(
            tokenade_core::detect::from_env().as_str(),
            tokenade_core::agent_caps::Action::Command,
            &combined,
        );
        gain_log::record_cmd(
            "wrap",
            &filter_label_for_log,
            &safe_label,
            savings_for_log,
            &cmd_name,
            cmd_specific,
        );
    }

    // #477 — SECURITY: redact secrets BEFORE the dedup pass. Without
    // this, the per-command compactor's output (which never goes
    // through the central redactor unless the compactor was secrets-
    // aware) lands in ~/.tokenade/dedup.db via `register()`. Worse,
    // it would create a consistency hole — `expand_ref` on a future
    // turn would return DIFFERENT bytes than what the agent saw the
    // first time. Redacting here keeps the cache content == the bytes
    // the agent observes, AND closes the persistent-store leak.
    // (Mirror of the #475 fix in `disclose::maybe_stash`.)
    let mut compacted_output = tokenade_core::profile::timed("wrap:redact_output", || {
        tokenade_core::redact::redact_for_model(&compacted_output)
    });

    // ── Dedup pass (L2, 2026-05-29) ───────────────────────────────
    // If this exact compact output was emitted before this project
    // (within the 7-day cache TTL), substitute it with a tiny
    // §ref:HASH§ marker. The agent already has the full content in a
    // prior turn — re-sending it doubles the cost. For the first
    // call of a given hash we register the content + emit it
    // normally so future calls can dedupe.
    //
    // Skip dedup for outputs below MIN_BYTES_TO_DEDUPE: the ref
    // marker itself costs ~80 chars; substituting a 50-byte output
    // would INFLATE not compress.
    drop(_p_tail);
    let _p_dedup = tokenade_core::profile::phase("wrap:dedup_key");
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let dedupable = compacted_output.len() >= tokenade_core::dedup_ref::MIN_BYTES_TO_DEDUPE;
    // ── Exec-dedup pass (flow analysis 2026-06) ───────────────────
    // ~2,900 transcript re-runs were the agent POLLING: the same
    // command re-executed with nothing changed in between. The
    // identity dedup below already collapses byte-identical output
    // to a §ref§ marker, but that marker carries no poll answer —
    // the agent expands it (or re-runs raw) to learn the status it
    // was polling FOR. Track "last output of this exact command in
    // this cwd" and, when the new output is byte-identical, answer
    // the poll directly: unchanged + age + exit code + output tail.
    // Truthful by construction — fires only on byte equality OF THE COMMAND'S
    // OWN OUTPUT. That qualifier is the whole sentence: this compared the two
    // COMPACTED strings for as long as it existed, and a fold that drops a
    // middle makes two different outputs deliver identically. Reproduced
    // 2026-09-05 on 400 lines of prose with one line rewritten inside the
    // elided middle. See `raw_fingerprint` below.
    let cmd_key = tokenade_core::profile::timed("wrap:command_key", || {
        tokenade_core::dedup_ref::command_key(&cwd.to_string_lossy(), &shell_cmd)
    });
    // The command's OWN bytes, before any fold. The poll answer is keyed on
    // this: it claims "byte-identical output to the previous run of this
    // command", and a fold that drops a middle makes two different outputs
    // deliver identically.
    let raw_fingerprint = tokenade_core::dedup_ref::hash_content(&out.stdout);
    drop(_p_dedup);
    let _p_emit = tokenade_core::profile::phase("wrap:exec_dedup");
    // The command ran again and the bytes MOVED: that is a poll, not a loop.
    // The PreToolUse counter only ever compared the command's INPUT, so a
    // watcher was told at its 3rd run that re-running would change nothing
    // while every run answered differently. A prior pointer must exist — on a
    // first run there is nothing to have moved.
    let ran_before = tokenade_core::dedup_ref::lookup_by_path_with_age(&cwd, &cmd_key).is_some();
    let same_bytes = tokenade_core::dedup_ref::lookup_by_path_with_age(
        &cwd,
        &dedup::poll_key(&cmd_key, &raw_fingerprint),
    )
    .is_some();
    crate::hooks::reset_loop_guard_if_output_moved(&shell_cmd, ran_before, same_bytes);
    if dedupable {
        if let Some(rc) = serve_exec_dedup(
            &mut compacted_output,
            &cwd,
            &cmd_key,
            &safe_label,
            exit_code_of(out.status),
            None,
            &raw_fingerprint,
            tokenade_core::disclose::single_file_read_target(&shell_cmd).as_deref(),
        ) {
            return rc;
        }
    }
    // REPLAY seed from the wrap side too: agents whose integration has NO
    // posttool hook (Cursor: preToolUse-only hooks.json) would otherwise
    // never store replay state. Session unknown here → TOKENADE_SESSION env
    // when the caller sets it, else the shared "nosess" bucket (the state
    // fingerprint + 300s window still guard correctness; write-generation
    // is a no-op for that bucket).
    {
        let skey = tokenade_core::session::ambient_session_key().unwrap_or_else(|| "nosess".into());
        tokenade_core::profile::timed("wrap:store_replay", || {
            crate::hooks::replay::store_replay(&skey, &cwd, &shell_cmd, &compacted_output)
        });
    }
    drop(_p_emit);
    let _p_disp = tokenade_core::profile::phase("wrap:display+disclose");
    let display = if dedupable {
        fold_or_register_dedup(
            compacted_output,
            DedupSite {
                channel: "wrap",
                op: "cmd_dedup",
                cwd: &cwd,
                key: &cmd_key,
                shell_cmd: &shell_cmd,
                safe_label: &safe_label,
                filter_label: &filter_label_for_log,
                // `wrap` runs under the shell hook, which sets the variable.
                session: None,
                raw_fingerprint: &raw_fingerprint,
            },
        )
    } else {
        compacted_output
    };

    // L64 — disclose: if the compact output is STILL > 5k tokens,
    // stash it and emit a summary + path. Shell hint → extension
    // sniffed from the content (`.json` for jq-able output, `.log`
    // for log-shaped output, `.txt` otherwise). Curl/wget/python
    // calls land here naturally — the shape detector picks up html
    // or json regardless of the command name.
    //
    // #1051 — when the wrapped command is a plain single-file read
    // (`cat`/`nl`/`head`/`tail`/… <file>), promote the hint to
    // `File(path)` so the disclose footer can redirect EXACT-match
    // lookups to grep/Read the original file directly instead of the
    // head+tail stash preview. Caught by the `log-needle` benchmark: a
    // folded `cat <log>` previously sent the agent into 8× `search_stash`
    // (BM25) + grep-on-stash detours (~14 wasted turns) before it grepped
    // the source. Conservative — any pipe/redirect/multi-file → Shell.
    let SecondStage {
        display,
        pre_stash_tokens,
    } = second_stage(
        display,
        &combined,
        &shell_cmd,
        dropped_tokens,
        before_tokens,
        &filter_label_for_log,
    );

    // The fallback fold used to be credited NOWHERE. `record_cmd` above
    // books what the per-command compactor saved; `maybe_stash` runs ~120 lines
    // later, so everything the catch-all folded landed outside the ledger.
    // Measured 2026-08-08 on a fresh HOME, each command run once: `journalctl
    // --no-pager -n 2000` delivered at 104 902 → ~1 200 tokens with ZERO rows
    // written — while the ESTIMATED ledger got a `recovery:offered` row saying
    // `saved: 0`. The product was not merely silent about a −99 % delivery, it
    // asserted the zero. That is the complaint this product exists to answer.
    //
    // No double credit, by construction rather than by threshold: the row above
    // spans raw → compacted, this one spans compacted → delivered. The two
    // intervals tile and cannot overlap. And `after` is measured on the FINAL
    // string — after the recovery pointer has been appended — so the credit is
    // what the model actually stopped paying, never what the fold removed
    // before we added bytes back.
    if let Some(before) = pre_stash_tokens {
        let after = tokenade_core::tokenizer::estimate(&display);
        if tokenade_core::gain::worth_recording(before as u64, after as u64, "disclose:stash") {
            gain_log::record_cmd(
                "wrap",
                "disclose:stash",
                &safe_label,
                tokenade_core::Savings::new(before, after),
                &cmd_name,
                cmd_specific,
            );
        }
    }

    // A byte-precise read must not be silently reshaped. Output that is not
    // valid UTF-8 but is not recognisably BINARY either — `head -c N` that
    // split a multibyte character, a latin-1 log, one stray byte — never
    // reaches `serve_binary`, so it comes through here as a lossy string with
    // every bad sequence turned into U+FFFD and nothing said about it.
    // Measured 2026-08-26: `head -c 2` on a file starting `h é` printed
    // `68 ef bf bd` where the file holds `68 c3`, with no banner and no
    // pointer — the agent cannot tell OUR replacement character from one the
    // command really produced.
    //
    // When the pipeline folded NOTHING, the lossy view bought nothing and cost
    // fidelity: hand back the bytes the command actually produced. The
    // equality below is also the safety argument — `display` has already been
    // through `redact_for_model`, so `display == combined` proves redaction
    // found nothing to remove in these bytes.
    if !decode_was_faithful && display == combined {
        use std::io::Write;
        let so = std::io::stdout();
        let mut h = so.lock();
        let _ = h.write_all(&out.stdout);
        let _ = h.write_all(&out.stderr);
        let _ = h.flush();
        tokenade_core::profile::timed("wrap:head_slice_advisory", || {
            emit_head_slice_advisory(&shell_cmd, &out.stdout)
        });
        return exit_code_of(out.status);
    }

    print!("{display}");
    // Same rule as the early-return path above: no output in, no output out.
    //
    // And no terminator on a PASSTHROUGH. The newline exists to close a
    // compacted VIEW — a banner or a summary that would otherwise run into
    // whatever prints next. When the pipeline folded nothing, `display` is the
    // command's own bytes and there is nothing to close; adding one changes
    // them. Measured 2026-09-03: `printf 'no-newline'` returned
    // `b"no-newline\n"` through the wrapper against `b"no-newline"` raw, on
    // the path the hook had already logged as `filter=passthrough
    // reason=below-5pct`. A passthrough that alters the bytes is not one, and
    // `cmd > file` wrote a byte the command never printed.
    //
    // `display == combined` is the same test the raw-bytes branch above uses
    // for "the pipeline folded NOTHING", and it carries the same safety
    // argument: `display` has been through `redact_for_model`, so equality
    // proves redaction removed nothing here either.
    if needs_terminator(&display, display == combined) {
        println!();
    }
    // …and when the fold DID run, it ran on the lossy view, so the substituted
    // characters are in what the agent is reading. Name them rather than let
    // them pass as the command's own.
    if !decode_was_faithful {
        if let Some(note) =
            tokenade_core::process_util::lossy_decode_advisory(&out.stdout, &out.stderr)
        {
            eprint!("{note}");
        }
    }

    tokenade_core::profile::timed("wrap:head_slice_advisory", || {
        emit_head_slice_advisory(&shell_cmd, &out.stdout)
    });

    exit_code_of(out.status)
}
