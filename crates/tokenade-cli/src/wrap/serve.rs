//! **Return the output, whole or folded, on the right stream.**
//!
//! `wrap` MERGES stdout and stderr to compact them together; when it cannot,
//! it serves them separately, and the terminator is not the same. Binary
//! output is not folded. And whatever was stashed must stay recoverable,
//! otherwise the fold is not a fold but a loss.

use super::*;

// ── The ways a wrapped command answers WITHOUT going through the
// compactor pipeline. Each prints, and its `Some(rc)` is the process exit
// code — so `cmd_wrap` reads as "did one of these already answer?".

/// L69 — binary short-circuit: when the captured stdout is a
/// recognisable binary blob (image, archive, audio, video, …),
/// skip the entire compactor pipeline (which assumes text) and
/// emit a metadata-only stash directly. The agent receives a
/// path + tool hints rather than the lossy UTF-8 view of the
/// bytes, which is unreadable anyway.
///
/// `None` when the bytes are TEXT — the caller then runs the normal pipeline.
/// That test is the whole point of the function and it was lost when this body
/// was lifted out of `cmd_wrap`: without it every `wrap` short-circuited here,
/// so the compactor never ran on anything. The e2e suite caught it (5 wrap
/// tests), the unit suites did not — nothing below `cmd_wrap` was wrong.
/// Does this command exist to RECOVER bytes that were folded away?
///
/// `tokenade expand-ref <hash>` is the agent saying, in the product's own
/// words, "I do not have these bytes". Folding its answer hands back a pointer
/// to the very hash it was given. Measured 2026-09-03 in a live session:
/// `timeout 60 tokenade expand-ref 3ebeec7cd4e7 --raw` came back as
/// `§ref:3ebeec7cd4e7§ (~1303 tokens / 5212 bytes, identical to a prior call —
/// ask it what you need: `tokenade expand-ref 3ebeec7cd4e7 --prompt "q1, q2"`)`
/// — the command that had just run, naming the hash it had just been asked to
/// expand. The recovery path the shipped guidance calls "the one you actually
/// want 99% of the time" was a closed loop, with no way out but a redirect to
/// a file.
///
/// The BARE spelling never reaches this file: the PreToolUse hook skips a
/// command whose program is tokenade, so it is never wrapped. Any launcher in
/// front of it defeats that test — `timeout 60 tokenade …`, `env FOO=1
/// tokenade …`, `nice tokenade …` all make the program `timeout`/`env`/`nice`
/// and were rewritten into a wrap, measured on all three. The rule belongs
/// here rather than in the hook: this is the place that folds, so every
/// channel into it — the hook, a hand-written `tokenade wrap '…'`, a
/// non-Claude agent following the scaffold — is covered at once, and the hook
/// keeps doing its own job, which is preventing recursion.
///
/// Matches a `tokenade` token followed by a recovery verb, so a launcher
/// prefix or an absolute path makes no difference. A false positive costs one
/// uncompacted output and never a wrong answer, which is the safe direction
/// for this gate.
pub(crate) fn recovers_folded_bytes(cmd: &str) -> bool {
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    tokens.iter().enumerate().any(|(i, t)| {
        let name = t.rsplit(['/', '\\']).next().unwrap_or(t);
        (name == "tokenade" || name == "tokenade.exe")
            && tokens
                .get(i + 1..)
                .unwrap_or_default()
                .iter()
                .find(|a| !a.starts_with('-'))
                .is_some_and(|verb| matches!(*verb, "expand-ref" | "expand_ref" | "xref"))
    })
}

/// Serve a recovery command's output exactly as the command printed it.
///
/// No compaction, no dedup, no poll answer: see [`recovers_folded_bytes`].
/// Nothing is added either — these are the command's own bytes, so the
/// terminator rule says they need no closing newline.
pub(super) fn serve_recovery(combined: &str, out: &std::process::Output) -> i32 {
    print!("{combined}");
    exit_code_of(out.status)
}

pub(super) fn serve_binary(out: &std::process::Output) -> Option<i32> {
    tokenade_core::disclose::detect_binary(&out.stdout)?;
    let display = tokenade_core::disclose::maybe_stash_bytes(
        out.stdout.clone(),
        tokenade_core::disclose::SourceHint::Shell,
    );
    print!("{display}");
    // A command that printed NOTHING must stay silent, and the metadata
    // banner this normally prints IS a view. Gated, `maybe_stash_bytes`
    // hands the bytes back instead, and those are the command's own.
    let passed_through = display == String::from_utf8_lossy(&out.stdout);
    if needs_terminator(&display, passed_through) {
        println!();
    }
    Some(exit_code_of(out.status))
}

/// Does this rendered stream need a closing newline?
///
/// Only a VIEW does. The newline terminates something the pipeline reshaped —
/// a banner or a summary that would otherwise run into whatever prints next.
/// Bytes handed back unchanged are the command's own and need nothing: adding
/// one there writes a byte the command never produced, which is invisible in a
/// terminal and a corrupted artifact under `> file`.
///
/// One declarant because the two ends got it wrong in opposite directions and
/// neither could see the other: the merged path terminated a passthrough, and
/// `--split-streams` — the mode whose whole purpose is that "redirections
/// behave as they do without the wrapper" — terminated nothing at all, so a
/// folded stream lost the newline the fold strips. Measured 2026-09-03:
/// `wrap --split-streams 'echo hi'` returned `hi` against `hi\n` raw, on 7 of
/// 14 real commands, on stdout and stderr alike.
pub(crate) fn needs_terminator(display: &str, passed_through: bool) -> bool {
    !passed_through && !display.is_empty() && !display.ends_with('\n')
}

/// `--split-streams`: each stream compacted alone and returned to its own
/// fd, so a redirection behaves as it does without the wrapper.
pub(super) fn serve_split_streams(
    out: &std::process::Output,
    shell_cmd: &str,
    cap_truncated: bool,
) -> i32 {
    use tokenade_core::process_util::{truncation_banner, DEFAULT_STDERR_CAP, DEFAULT_STDOUT_CAP};
    // Each stream compacted alone, each returned to its own fd. No stash
    // pointer is emitted here on purpose: a ref written into a redirected
    // data file would be worse than the bytes it replaced.
    let (so, so_faithful) =
        tokenade_core::process_util::decode_child_output_of(&out.stdout, shell_cmd);
    let (se, se_faithful) =
        tokenade_core::process_util::decode_child_output_of(&out.stderr, shell_cmd);
    tokenade_core::user_filters::set_command_failed(exit_code_of(out.status) != 0);
    let argv: Vec<String> =
        command_rewrite::meaningful_segment(filter::strip_leading_comment_lines(shell_cmd))
            .split_whitespace()
            .map(String::from)
            .collect();
    let fold = |text: &str| -> String {
        if text.is_empty() {
            return String::new();
        }
        let r = filter::filter_command_fail_open(&argv, text);
        let (picked, _) = tokenade_core::fold_count::pick_compact_or_raw(
            text.to_string(),
            r.output.clone(),
            r.savings,
            || tokenade_core::tokenizer::estimate(text),
        );
        redact::redact_for_model(&picked)
    };
    // Byte fidelity, per stream: a stream the fold left untouched is written
    // back as the bytes the child produced, not as its lossy decode. Only the
    // streams that WERE folded carry the advisory, since only those hand the
    // agent a substituted character.
    let write_stream =
        |bytes: &[u8], folded: String, lossy: &str, faithful: bool, is_stderr: bool| {
            let unchanged = folded == lossy;
            if !unchanged || faithful {
                let close = needs_terminator(&folded, unchanged);
                if is_stderr {
                    eprint!("{folded}");
                    if close {
                        eprintln!();
                    }
                } else {
                    print!("{folded}");
                    if close {
                        println!();
                    }
                }
                return false;
            }
            use std::io::Write;
            if is_stderr {
                let h = std::io::stderr();
                let _ = h.lock().write_all(bytes);
            } else {
                let h = std::io::stdout();
                let mut h = h.lock();
                let _ = h.write_all(bytes);
                let _ = h.flush();
            }
            true
        };
    let so_raw = write_stream(&out.stdout, fold(&so), &so, so_faithful, false);
    let se_raw = write_stream(&out.stderr, fold(&se), &se, se_faithful, true);
    // A stream carries the advisory only when it was BOTH decoded lossily and
    // shown folded — written raw, the bytes are the command's own; decoded
    // faithfully, nothing was substituted. Either alone would put a line on
    // output that is perfectly fine.
    let a = if so_raw || so_faithful {
        &[][..]
    } else {
        &out.stdout[..]
    };
    let b = if se_raw || se_faithful {
        &[][..]
    } else {
        &out.stderr[..]
    };
    if let Some(note) = tokenade_core::process_util::lossy_decode_advisory(a, b) {
        eprint!("{note}");
    }
    if cap_truncated {
        eprint!(
            "{}",
            truncation_banner(DEFAULT_STDOUT_CAP, DEFAULT_STDERR_CAP)
        );
    }
    exit_code_of(out.status)
}
