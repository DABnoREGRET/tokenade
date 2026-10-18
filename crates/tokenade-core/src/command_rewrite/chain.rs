//! **Where one command stops and the next begins.**
//!
//! `cd foo && pytest && npm test` is three commands, and only the last
//! carries the output that will be folded. Counting the saving on the whole chain
//! would dilute the number with one-line prefixes.
//!
//! Splitting is done on the operators with quoted regions MASKED: a `&&`
//! between quotes is not an operator, and a parser that ignores this
//! cuts in the middle of an argument. Where the shell would re-parse — subshells,
//! backticks, heredocs, `eval` — we refuse: being wrong costs more
//! than doing nothing.

use super::*;

/// Returns `true` for inputs we refuse to touch (subshells, backticks,
/// heredocs, `eval`, redirects with quoted special chars).
pub(crate) fn has_shell_unsafe(cmd: &str) -> bool {
    cmd.contains("<<")
        || cmd.contains("$(")
        || cmd.contains('`')
        || cmd.split_whitespace().any(|t| t == "eval")
        || cmd.contains("<(")
        || cmd.contains(">(")
}

/// Split on top-level `&&`, `;`, or `|`, return (last_segment, did_split).
///
/// We deliberately ignore `||` (would change semantics — the right side
/// runs only on failure). For `|`, we focus on the *rightmost* command
/// because that's whose output the agent ultimately reads.
pub(crate) fn focus_segment(cmd: &str) -> (Cow<'_, str>, bool) {
    let mut last_break = None;
    let bytes = cmd.as_bytes();
    let mut in_squote = false;
    let mut in_dquote = false;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'\\' if i + 1 < bytes.len() => i += 2,
            b'\'' if !in_dquote => {
                in_squote = !in_squote;
                i += 1;
            }
            b'"' if !in_squote => {
                in_dquote = !in_dquote;
                i += 1;
            }
            b'&' if !in_squote && !in_dquote && i + 1 < bytes.len() && bytes[i + 1] == b'&' => {
                last_break = Some(i + 2);
                i += 2;
            }
            b'|' if !in_squote && !in_dquote => {
                // Skip ||.
                if i + 1 < bytes.len() && bytes[i + 1] == b'|' {
                    i += 2;
                    continue;
                }
                last_break = Some(i + 1);
                i += 1;
            }
            b';' if !in_squote && !in_dquote => {
                last_break = Some(i + 1);
                i += 1;
            }
            _ => i += 1,
        }
    }

    match last_break {
        Some(at) => (Cow::Borrowed(cmd[at..].trim_start()), true),
        None => (Cow::Borrowed(cmd), false),
    }
}

/// One element in a chain-split. `cmd` is the segment text (trimmed),
/// `separator_after` is what came after it in the original input —
/// `"&&"`, `"||"`, `";"`, or `None` for the last segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainSegment<'a> {
    pub cmd: &'a str,
    pub separator_after: Option<&'static str>,
}

/// What a command line holds, as the SCANNER sees it.
///
/// #296 — the question "is this a chain?" was spelled by hand in three
/// predicates, and one of them knew three separators where the scanner knows
/// four. The newline was taught to the scanner first and the gate that decides
/// whether the scanner is even consulted kept its own answer, so the same pair
/// bit twice. There is no separator list outside the scanner any more:
/// callers ask, and say which answer they want.
///
/// The three cases are not interchangeable, which is why they are named rather
/// than collapsed to a bool. `Unparsed` is the shape the last-chance peel in
/// `bash_pretool` exists for — a payload whose quotes or `$()` defeat the
/// scanner but which wraps fine as ONE unit, worth 0.74M characters of
/// compaction — so a caller that refuses `Many` must still accept `Unparsed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chain {
    /// Plainly one command.
    One,
    /// Two or more, and the scanner can separate them.
    Many,
    /// The scanner refused: control flow, a heredoc, a line continuation, a
    /// comment. It may be one command or several.
    Unparsed,
}

/// Ask the scanner what this command line holds.
pub fn chain_shape(cmd: &str) -> Chain {
    match split_chain_segments(cmd) {
        Some(segments) if segments.len() > 1 => Chain::Many,
        Some(_) => Chain::One,
        None => Chain::Unparsed,
    }
}

/// compactor instead of the old single-wrap-on-argv[0] behaviour.
pub fn split_chain_segments(cmd: &str) -> Option<Vec<ChainSegment<'_>>> {
    // Bail on anything we can't safely parse.
    if has_shell_unsafe(cmd) {
        return None;
    }
    // Bail on bash control-flow constructs — splitting on `;` would
    // break `for i in a; do x; done`.
    if has_bash_control_flow(cmd) {
        return None;
    }

    let mut segments = Vec::new();
    let bytes = cmd.as_bytes();
    let mut in_squote = false;
    let mut in_dquote = false;
    let mut depth_paren = 0i32; // ( ... ) groups and (( arithmetic ))
    let mut depth_brace = 0i32; // { ... } brace groups and ${VAR} expansion
    let mut seg_start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            // A backslash-newline is a LINE CONTINUATION: bash removes it before
            // parsing. Scanning past it finds the right separators, but the
            // segment text still carries it, and re-quoting that text makes the
            // backslash a literal argument — a continued `grep pat \ <nl> file`
            // started passing `\` to grep. Removing it correctly means knowing
            // the quote context at every byte, so take the exit this function
            // already has and let bash do its own line joining.
            b'\\' if !in_squote && i + 1 < bytes.len() && bytes[i + 1] == b'\n' => {
                return None;
            }
            b'\\' if i + 1 < bytes.len() => i += 2,
            b'\'' if !in_dquote => {
                in_squote = !in_squote;
                i += 1;
            }
            b'"' if !in_squote => {
                in_dquote = !in_dquote;
                i += 1;
            }
            // An unquoted `#` at a word boundary starts a comment that runs to
            // the end of the line — everything after it is NOT a command. The
            // splitter did not know this, so `echo one # && echo two` was cut
            // at the `&&` and the commented-out half was RUN. A split can never
            // resurrect something the user disabled, so refuse: the caller then
            // wraps the command whole and bash parses the comment itself.
            b'#' if !in_squote && !in_dquote && (i == 0 || bytes[i - 1].is_ascii_whitespace()) => {
                return None;
            }
            b'(' if !in_squote && !in_dquote => {
                depth_paren += 1;
                i += 1;
            }
            b')' if !in_squote && !in_dquote => {
                depth_paren -= 1;
                if depth_paren < 0 {
                    // Unbalanced; refuse to split.
                    return None;
                }
                i += 1;
            }
            b'{' if !in_squote && !in_dquote => {
                depth_brace += 1;
                i += 1;
            }
            b'}' if !in_squote && !in_dquote => {
                depth_brace -= 1;
                if depth_brace < 0 {
                    return None;
                }
                i += 1;
            }
            b'&' if !in_squote
                && !in_dquote
                && depth_paren == 0
                && depth_brace == 0
                && i + 1 < bytes.len()
                && bytes[i + 1] == b'&' =>
            {
                let seg = cmd[seg_start..i].trim();
                segments.push(ChainSegment {
                    cmd: seg,
                    separator_after: Some("&&"),
                });
                i += 2;
                seg_start = i;
            }
            // Single `&` (background job) — too messy to safely wrap.
            // `cmd1 & cmd2` runs cmd1 in the background and cmd2 in
            // the foreground; output streams interleave and wrapping
            // either side changes timing semantics. Refuse to split.
            //
            // Distinguish from `2>&1` (redirect: `&` follows a `>`) —
            // tracked by the previous byte. Also exclude `&&` which
            // matched above.
            b'&' if !in_squote && !in_dquote && depth_paren == 0 && depth_brace == 0 => {
                let prev = if i > 0 { bytes[i - 1] } else { 0 };
                // `2>&1`, `>&1`, `1>&2`: the `&` is part of an fd-dup
                // redirect, not a job-control operator.
                if prev != b'>' {
                    return None;
                }
                i += 1;
            }
            b'|' if !in_squote
                && !in_dquote
                && depth_paren == 0
                && depth_brace == 0
                && i + 1 < bytes.len()
                && bytes[i + 1] == b'|' =>
            {
                let seg = cmd[seg_start..i].trim();
                segments.push(ChainSegment {
                    cmd: seg,
                    separator_after: Some("||"),
                });
                i += 2;
                seg_start = i;
            }
            // A bare NEWLINE separates two commands exactly as `;` does, and
            // agents type it constantly: `cd <dir>` on its own line, then the
            // real work on the next. It was not a separator here, so the whole
            // block routed on `cd` and the real program's compactor never ran.
            // Measured 2026-08-26 on the ledger, 95 rows are keyed `cd` and
            // carry 142 610 tokens of input a `cd` never produced; on
            // `ls -la` the plain form comes back `[tokenade:ls-long]` with the
            // link/owner/group columns elided, the newline form
            // `[tokenade:cd] 119 line(s)` with them kept.
            //
            // Safe for the same reason `;` is: every multi-line shape that
            // must NOT be split has already refused above — heredocs, command
            // and process substitution (`has_shell_unsafe`), `for`/`while`/
            // `if` (`has_bash_control_flow`), and a backslash line
            // continuation, which returns `None` a few arms up.
            b';' | b'\n' if !in_squote && !in_dquote && depth_paren == 0 && depth_brace == 0 => {
                let seg = cmd[seg_start..i].trim();
                segments.push(ChainSegment {
                    cmd: seg,
                    separator_after: Some(if c == b';' { ";" } else { "\n" }),
                });
                i += 1;
                seg_start = i;
            }
            _ => i += 1,
        }
    }
    // Unbalanced quotes or groups → refuse to split.
    if in_squote || in_dquote || depth_paren != 0 || depth_brace != 0 {
        return None;
    }
    let tail = cmd[seg_start..].trim();
    segments.push(ChainSegment {
        cmd: tail,
        separator_after: None,
    });
    Some(segments)
}

/// True when `cmd` contains a bash control-flow OPENER at the top
/// level. Splitting these on `;` would shred the syntax — `for i in a;
/// do echo $i; done` becomes three nonsense fragments.
///
/// We only check openers (`for`, `while`, `until`, `if`, `case`,
/// `select`, `function`) rather than every keyword — terminators
/// (`done`, `fi`, `esac`) double as common English words (e.g.
/// `echo done`) and would false-positive. An opener implies its
/// terminators are present and the whole structure is unsafe to split.
pub(crate) fn has_bash_control_flow(cmd: &str) -> bool {
    let Some(masked) = mask_quoted(cmd) else {
        // An unterminated quote is a command we cannot reason about. Bailing
        // is the conservative answer: a chain we refuse to split is routed
        // whole, never mis-split.
        return true;
    };
    for tok in masked.split_whitespace() {
        // Strip a trailing `;` so e.g. `if;` is caught (rare but legal).
        if SHELL_OPENERS.contains(&tok.trim_end_matches(';')) {
            return true;
        }
    }
    false
}

/// `cmd` with every quoted run replaced by a filler byte, or `None` when a
/// quote is left open.
///
/// A shell keyword inside a string is a WORD, not a keyword, and reading the
/// raw command let one decide that a whole chain could not be split.
/// Measured: `cd … ; sed -n '1,80p' a.rs ; echo "=== rows iterated for the
/// totals ===" ; sed -n '90,120p' a.rs` bailed on the `for` inside the echo,
/// so the chain was routed and labelled by its leading `cd` — a command that
/// produces no output at all. That is where the ledger's `cd` rows and 92 of
/// the "write a compactor for `cd`" nudges came from.
///
/// A filler rather than a deletion: `"a"for` must stay one token (`xfor`) and
/// not become the opener `for`.
pub(crate) fn mask_quoted(cmd: &str) -> Option<String> {
    let mut out = String::with_capacity(cmd.len());
    let mut chars = cmd.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                out.push('x');
                if let Some(next) = chars.next() {
                    // A backslash-escaped character is never a keyword edge.
                    out.push(if next.is_whitespace() { next } else { 'x' });
                }
            }
            '\'' | '"' => {
                let quote = c;
                out.push('x');
                let mut closed = false;
                for q in chars.by_ref() {
                    if q == '\\' && quote == '"' {
                        out.push('x');
                        continue;
                    }
                    if q == quote {
                        closed = true;
                        out.push('x');
                        break;
                    }
                    // Newlines inside a quote still separate nothing: keep the
                    // run one token.
                    out.push('x');
                }
                if !closed {
                    return None;
                }
            }
            _ => out.push(c),
        }
    }
    Some(out)
}
