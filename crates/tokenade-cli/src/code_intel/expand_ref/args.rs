//! **What the agent typed** — and what it meant.
//!
//! `expand-ref` is called by an agent that has just read a banner, so often
//! sloppily: the flag after the hash, two hashes glued together, a `--grep`
//! with no pattern. Refusing to the letter would cost the agent the only
//! recourse it has at the moment it needs it.
//!
//! Hence `hoist_misplaced_flags`: misplaced flags are hoisted rather than
//! failing. The limit is that no reinterpretation may change WHAT is asked
//! for — only the order in which it was written.

use super::*;

/// Flags `expand-ref` accepts, parsed in ONE place.
pub(crate) struct ExpandArgs {
    pub(crate) grep_pats: Vec<String>,
    pub(crate) prompt: Option<String>,
    pub(crate) raw: bool,
    /// Ceiling in estimated tokens on what `--raw` prints. `Some(0)` = none.
    pub(crate) budget: Option<usize>,
    /// 1-based line to resume a bounded `--raw` from.
    pub(crate) from_line: usize,
    /// Fold case on both sides of a `--grep`. Off by default: a literal grep
    /// is what an agent copying a token from the output wants.
    pub(crate) ignore_case: bool,
    /// Flag-shaped arguments this command does not accept. Refused by name
    /// rather than dropped: see `parse_expand_args`.
    pub(crate) unknown: Vec<String>,
}

/// Every flag `expand-ref` accepts, and whether it consumes the next argument.
///
/// ONE table, read by both passes over the arguments: the parser asks what to
/// do with a flag, and [`collect_hashes`] asks what is NOT a hash. They used to
/// keep a list each, and the second one's own comment said what that costs —
/// "every flag that TAKES a value must be listed, or its value is read as a
/// hash". A flag missing from either list produced a different wrong answer.
pub(crate) const EXPAND_FLAGS: &[(&str, bool)] = &[
    ("--grep", true),
    ("--prompt", true),
    ("--question", true),
    ("-q", true),
    ("--budget", true),
    ("--from-line", true),
    ("--raw", false),
    ("--full", false),
    ("--ignore-case", false),
    ("-i", false),
];

/// Does `tok` consume the argument after it? `None` when it is not a flag we
/// know at all.
pub(crate) fn expand_flag_takes_value(tok: &str) -> Option<bool> {
    EXPAND_FLAGS
        .iter()
        .find(|(name, _)| *name == tok)
        .map(|(_, takes)| *takes)
}

/// A token the caller meant as a flag: it opens with `-` and is not the bare
/// `-`, which stays a positional.
pub(crate) fn looks_like_a_flag(tok: &str) -> bool {
    tok.starts_with('-') && tok != "-"
}

/// `--grep -i "poll"` is a MISPLACED flag, not a pattern.
///
/// A value-taking flag consumes whatever follows it, so `-i` became the
/// pattern and `"poll"` fell through to [`collect_hashes`] as a second hash.
/// The answer was `0 matching line(s)` plus "nothing matches \"-i\" … ignoring
/// case either" on a stash where EVERY line contained "poll", followed by
/// `'poll' is not a ref hash`. Reproduced 2026-09-05 from a transcript on this
/// machine: `tokenade expand-ref 4e17ec351e3c --grep -i "poll"` — the caller
/// spelled its intent perfectly and was told, twice, that its own text was not
/// there.
///
/// The repair is to read it as it was meant: the valueless flags that follow a
/// value-taking one are hoisted in front of it, so the value is the next thing
/// that is not a flag of this command. A pattern that is genuinely `-i` keeps
/// an unambiguous spelling — `--grep -- -i` — because `--` ends the flags.
pub(crate) fn hoist_misplaced_flags(rest: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(rest.len());
    let mut i = 0;
    while i < rest.len() {
        if rest[i] == "--" {
            out.extend_from_slice(&rest[i..]);
            return out;
        }
        if expand_flag_takes_value(&rest[i]) == Some(true) {
            // `--grep -- -i` is the escape hatch for a pattern that IS a flag
            // spelling: the `--` is consumed here and the next token is the
            // value, verbatim. A bare `--` anywhere else still ends the flags.
            if rest.get(i + 1).map(String::as_str) == Some("--") {
                out.push(rest[i].clone());
                if let Some(v) = rest.get(i + 2) {
                    out.push(v.clone());
                }
                i += 3;
                continue;
            }
            // The run of valueless flags wedged between this flag and its
            // value. `--grep -i --raw poll` is as meant as `--grep -i poll`.
            let mut j = i + 1;
            while j < rest.len() && expand_flag_takes_value(&rest[j]) == Some(false) {
                out.push(rest[j].clone());
                j += 1;
            }
            out.push(rest[i].clone());
            if j < rest.len() {
                out.push(rest[j].clone());
            }
            i = j + 1;
            continue;
        }
        out.push(rest[i].clone());
        i += 1;
    }
    out
}

pub(crate) fn parse_expand_args(rest: &[String]) -> ExpandArgs {
    let mut grep_pats: Vec<String> = Vec::new();
    let mut prompt: Option<String> = None;
    let mut raw = false;
    let mut ignore_case = false;
    let mut budget: Option<usize> = None;
    let mut from_line: usize = 1;
    let mut unknown: Vec<String> = Vec::new();
    let mut flags_over = false;
    let hoisted = hoist_misplaced_flags(rest);
    let mut it = hoisted.iter();
    while let Some(a) = it.next() {
        // `--` ends the flags, as it does everywhere else. It was already
        // accepted here by being ignored; refusing it now would break the one
        // spelling that lets a hash-shaped argument start with a dash.
        if !flags_over && a == "--" {
            flags_over = true;
            continue;
        }
        if flags_over {
            continue;
        }
        match a.as_str() {
            // Repeatable: `--grep a --grep b` greps both, results grouped by
            // pattern. Repeatable (not comma-split) because a pattern may
            // itself contain a comma.
            "--grep" => {
                if let Some(v) = it.next() {
                    grep_pats.push(v.to_string());
                }
            }
            "--prompt" | "--question" | "-q" => prompt = it.next().map(|s| s.to_string()),
            "--raw" | "--full" => raw = true,
            "--ignore-case" | "-i" => ignore_case = true,
            "--budget" => budget = it.next().and_then(|v| v.parse::<usize>().ok()),
            "--from-line" => {
                from_line = it
                    .next()
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(1)
                    .max(1)
            }
            // A flag we do not know was silently dropped here, and this is the
            // command an agent reaches for AFTER it has already lost bytes.
            // Measured: `--raww <hash>` returned the 313-byte "too large"
            // banner where `--raw` returns 187 550 bytes, exit 0 either way,
            // with nothing said — the caller asked for everything and was told
            // the blob was too big to show.
            other if looks_like_a_flag(other) => unknown.push(other.to_string()),
            _ => {}
        }
    }
    ExpandArgs {
        grep_pats,
        prompt: prompt.filter(|p| !p.trim().is_empty()),
        raw,
        budget,
        from_line,
        ignore_case,
        unknown,
    }
}
