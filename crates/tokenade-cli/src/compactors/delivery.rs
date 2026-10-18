//! **Bound what goes out, and say so.**
//!
//! The primitives shared by several commands: cutting to a line range,
//! bounding a delivery, recognising our own redaction signal. None of them
//! picks a route; all of them decide what goes OUT.

use super::*;

/// The program names a `--cmd` hint can plausibly credit for its output.
///
/// A hint is a whole shell command line, not a program name, and the naive
/// `split_whitespace().next()` this replaced got three common shapes wrong:
///
/// * `cd /repo && cargo build` → `cd`, whose spec-table entry is a generic
///   long-output fold. It won the "smallest wins" competition on cargo output
///   and printed `[tokenade:cd]` — a provenance line naming a program that
///   produced none of those bytes;
/// * `FOO=1 cargo build` → `FOO=1`, no compactor at all;
/// * `/usr/bin/git status` → `/usr/bin/git`, no compactor — the scaffold tells
///   agents an absolute path is treated exactly like the bare name.
///
/// [`first_program_token`](tokenade_core::command_rewrite::first_program_token)
/// already handles env prefixes, path stripping and quoting; it is what the
/// Bash hook routes on. Chains are split the same way the hook splits them, so
/// every segment that actually runs a program gets a candidate, and the caller
/// keeps whichever fold is smallest. Segments the hook refuses to wrap (`cd`,
/// `export`, `set -e` …) are dropped: they emit nothing, so crediting them is
/// always a mis-attribution.
/// The bytes an "already folded, leave it alone" passthrough serves.
///
/// Redacted, then newline-terminated. Extracted so the redaction is testable:
/// it is the only thing standing between an un-scrubbed payload and stdout on
/// that path, and it was missing until 2026-08-08 — a document carrying a
/// `[tokenade:…]` banner served a live `sk-ant-api03-…` key that the same
/// binary redacted through every other path.
/// The stderr prefix that tells a caller "this output has secrets removed".
///
/// A CONTRACT with the five plugin bridges, not an advisory: each of them
/// accepts our stdout only when it is strictly shorter, and a scrubbed payload
/// is routinely longer. They look for this exact string, so it lives here once
/// and `plugin_bridges_look_for_the_redaction_signal` pins that every asset
/// still carries it.
pub(crate) const REDACTION_SIGNAL: &str = "[tokenade] redacted:";

pub(crate) fn passthrough_text(src: &str) -> String {
    let mut safe = tokenade_core::redact::redact_for_model(src);
    if !safe.ends_with('\n') {
        safe.push('\n');
    }
    safe
}

pub(crate) fn hint_programs(hint: &str) -> Vec<&str> {
    use tokenade_core::command_rewrite as cr;
    let segments: Vec<&str> = match cr::split_chain_segments(hint) {
        Some(segs) => segs.iter().map(|s| s.cmd).collect(),
        // Unsplittable (heredoc, `$(…)`, control flow): treat it as one.
        None => vec![hint],
    };
    let mut progs: Vec<&str> = Vec::new();
    for seg in segments {
        if cr::segment_must_not_be_wrapped(seg) {
            continue;
        }
        let prog = cr::first_program_token(seg);
        if !prog.is_empty() && !progs.contains(&prog) {
            progs.push(prog);
        }
    }
    progs
}

/// Bound an already-compacted delivery, and credit the fold that bounding did.
///
/// Three call sites carried these fifteen lines verbatim — `cmd_wrap`,
/// `cmd_auto` and `cmd_snapshot` — each with its own comment explaining
/// that the last one had been missed. A rule with three copies has three
/// futures, and this one had already spent two of them: a per-format compactor
/// is a BET that we recognised the format, and when the bet loses `maybe_stash`
/// is the only thing standing between the model and the whole file.
///
/// The fourth caller is why it finally got a name: Gemini's `read_file`
/// redirect compacted and never bounded, so on any format the compactor did not
/// recognise it declined entirely and the agent read all 104 000 bytes.
///
/// The two rows tile and cannot overlap: the compactor's spans raw → compacted,
/// this one compacted → delivered, and `after` is taken on the FINAL string so
/// the recovery pointer's own bytes are never credited.
pub(crate) fn bound_delivery(
    compacted: String,
    hint: tokenade_core::disclose::SourceHint,
    op: &str,
) -> String {
    // Measured BEFORE the stash, so the fold below can be credited. Same exact
    // byte floor as `cmd_wrap`: a token is at least one byte, so a string
    // shorter than the threshold cannot exceed it in tokens and `maybe_stash`
    // cannot fire.
    let threshold = tokenade_core::disclose::read_threshold_tokens();
    let pre_stash_tokens =
        (compacted.len() >= threshold).then(|| tokenade_core::tokenizer::estimate(&compacted));
    let bounded = tokenade_core::disclose::maybe_stash_with_threshold(compacted, hint, threshold);
    if let Some(before) = pre_stash_tokens {
        let after = tokenade_core::tokenizer::estimate(&bounded);
        if tokenade_core::gain::worth_recording(before as u64, after as u64, "disclose:stash") {
            gain_log::record("cli", op, "disclose:stash", Savings::new(before, after));
        }
    }
    bounded
}

/// `--lines A-B`, `A-`, `-B` or `A`, 1-based and inclusive on both ends.
///
/// A big file is read in pieces or not at all: an agent that has been told
/// WHERE the answer is — by a search, by a stack trace, by a previous read —
/// otherwise has to take the whole document again to see forty lines of it.
pub(super) fn parse_line_spec(spec: &str) -> Result<(usize, usize), String> {
    let spec = spec.trim();
    let bad =
        || format!("expected a line range like `120-260`, `120-`, `-80` or `42`, got {spec:?}");
    let num = |t: &str| t.parse::<usize>().ok().filter(|n| *n > 0);
    let (from, to) = match spec.split_once('-') {
        None => {
            let n = num(spec).ok_or_else(bad)?;
            (n, n)
        }
        Some(("", tail)) => (1, num(tail).ok_or_else(bad)?),
        Some((head, "")) => (num(head).ok_or_else(bad)?, usize::MAX),
        Some((head, tail)) => (num(head).ok_or_else(bad)?, num(tail).ok_or_else(bad)?),
    };
    if from > to {
        return Err(format!("line range {from}-{to} ends before it starts"));
    }
    Ok((from, to))
}

/// The requested lines, and what the file actually held.
///
/// Returns the slice plus the range it really covers, so the caller can say so
/// rather than let a request for lines 9000-9100 of a 400-line file come back
/// as an empty answer that looks like a file with nothing in it.
pub(super) fn slice_lines(
    text: &str,
    from: usize,
    to: usize,
) -> Option<(String, usize, usize, usize)> {
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    // Out of range is NOT emptiness, and neither is a blank line. Clamping the
    // START answered a request for lines 9000-9100 of a 1 154-line file with
    // line 1 154, and testing the SLICE for emptiness refused line 42 because
    // line 42 of that file is blank. Both were mine, both measured before they
    // shipped.
    if from > total || total == 0 {
        return None;
    }
    let last = to.min(total);
    Some((lines[from - 1..last].join("\n"), from, last, total))
}
