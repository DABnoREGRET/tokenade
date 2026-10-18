//! **`cat` and its twins: displaying a file is not running a command.**
//!
//! The content is that of a FILE, so the cut must be byte-exact, the
//! truncation must keep the banner saying it happened, and a source file
//! folds by its comments rather than by its end.

use super::*;













// ---- command-specific compactors -----------------------------------------

// ─────────────────────────────────────────────────────────────
// The dispatchers that read the PROGRAM.
//
// The eighteen that only read `argv` are gone: each was one line,
// `fn x_dispatch(argv, output) { cmd::linux::x_dispatcher::dispatch(
// argv, output) }`, and the arm of the big `match` called them. One more name
// between the arm and the compactor says nothing the path does not say; the
// arm now names the module that answers.
//
// The three below stay because they DECIDE: they also take `prog` and pick
// the compactor from it. That is the criterion — a dispatcher that reads
// something stays, a redirection that renames goes.
// ─────────────────────────────────────────────────────────────

















// ─── small inline helpers ───

/// #502 — `cat <source-file>` / `bat <source-file>` code_strip pass.
///
/// The Read pretool hook already auto-applies `code_strip(Comments)`
/// when an agent calls Claude Code's native `Read` on a >= 5 KB source
/// file (see `handle_read_pretool` in tokenade-cli). The Bash hook
/// fell through to the generic dedup pass on `cat src/foo.rs`, missing
/// the same win — agents that prefer `cat` over `Read` (or that pipe
/// `cat foo.rs | head -50` to grab a leading slice) got noisy output.
///
/// Returns `Some(stripped)` only when:
///   1. The argv carries a file whose extension maps to a `code_strip`
///      language (rs, py, ts, js, go, rb, c, cpp, …), AND
///   2. `code_strip` actually shrinks the input (Comments mode tried
///      first; `Lite` mode as fallback).
///
/// Otherwise returns `None` so the caller can hand off to
/// `auto_or_generic`, which gives JSON/YAML/diff/etc. their format-
/// specific compactors. (Regression caught by `e2e_proxy_compresses_
/// noisy_command` when an earlier draft of this fix routed `cat
/// blob.json` away from `json_compact`.)
/// #590 — true when any positional argv arg has a source-code
/// extension recognized by `auto::is_source_code_extension` (40+
/// languages: rust/go/c/c++/python/js/ts/jvm/.net/ruby/php/swift/
/// lua/shell/erlang/haskell/clojure/dart/zig/...). Used to guard
/// the cat/bat dispatch from routing source files into the auto
/// detector when code_strip can't help.
/// Whether a `head`/`tail` invocation asked for an exact BYTE count.
///
/// `-c N`, `-cN`, `--bytes N`, `--bytes=N`, and any short-option cluster
/// carrying `c` (`-qc 40`). GNU `head`/`tail` take `-c -n -q -v -z` and only
/// `c` is bytes, so a cluster containing it is unambiguously a byte read.
///
/// A byte-precise read is a question about EXACT bytes at EXACT offsets, and
/// every transformation makes the answer wrong for the question asked. The
/// ranged-read arm below strips comments from it: measured 2026-08-26,
/// `head -c 6000 crates/tokenade-core/src/filter.rs` returned **4 976 bytes**,
/// identical to the file for 3 251 of them and then silently divergent, with
/// NO banner on stdout and NOTHING on stderr. An agent checking a header,
/// computing an offset, or comparing a prefix got a wrong answer that looked
/// right.
///
/// Passing these through costs nothing: measured over 80 286 recorded Bash
/// invocations, 908 use a byte count and the median is **300 bytes** — one
/// single call in 896 asks for more than 20 000. `-c` is a sampling idiom,
/// not a bulk read.
pub fn head_tail_byte_precise(argv: &[String]) -> bool {
    // The LAST stage of the pipeline, which is the one that shaped the bytes
    // we were handed. With no pipe that is the whole argv, so the unpiped
    // answer is unchanged.
    //
    // It read `argv[0]` only, and while any pipe meant passthrough that was
    // harmless: `grep x . | head -c 4096` never reached a compactor anyway.
    // The moment a pipeline is routed by content, this is the one thing that
    // must still not be — a byte-precise read is a question about EXACT bytes
    // at EXACT offsets, and every transformation makes the answer wrong for
    // the question asked. Caught by its own guard, not by review.
    let last = argv
        .iter()
        .rposition(|t| t == "|")
        .map_or(argv, |i| &argv[i + 1..]);
    let prog = last.first().map(|a| basename(a));
    if !matches!(prog.as_deref(), Some("head") | Some("tail")) {
        return false;
    }
    last.iter().skip(1).any(|a| {
        a == "--bytes"
            || a.starts_with("--bytes=")
            || (a.starts_with('-') && !a.starts_with("--") && a[1..].contains('c'))
    })
}

pub(super) fn cat_argv_has_source_extension(argv: &[String]) -> bool {
    argv.iter()
        .skip(1) // prog name
        .filter(|a| !a.starts_with('-'))
        .any(|a| crate::auto::is_source_code_extension(a))
}

// Moved down to `tk_base::argv`: `disclose` asks for it too, and `disclose`
// is under `filter` (guard `layers_only_point_downwards`).
pub(crate) use crate::argv::awk_only_selects_lines;

pub(super) fn cat_try_code_strip(argv: &[String], output: &str) -> Option<String> {
    let lang = argv
        .iter()
        .skip(1) // prog name
        .filter(|a| !a.starts_with('-'))
        .filter_map(|a| {
            std::path::Path::new(a.as_str())
                .extension()
                .and_then(|e| e.to_str())
                .and_then(crate::code_strip::language_for_extension)
        })
        .next()?;
    // SAY it. Every other stripper in this crate returns through
    // `code_strip::declare_strip` (toml, hcl, bazel, proto, sql, cmake, ini);
    // the SOURCE path, which is the busy one — `cat`, `sed -n 'A,Bp'`, `head`,
    // `tail` on a `.py`/`.rs`/`.ts` — returned the stripped text bare.
    //
    // A comment line is removed and its newline stays, so the file comes back
    // with the right number of lines and a blank where the explanation was:
    // it does not read as compacted, it reads as a file that has no comments.
    // Field quote from this machine's transcripts, an agent reasoning about
    // its own repo: "OK, the file is correct (the previous sed was showing a
    // version compacted by tokenade)" — it had concluded the file was
    // wrong. Reproduced 2026-09-05: `sed -n '1,600p'` over 200 commented
    // definitions dropped all 200 comment lines with no banner at all.
    //
    // Counted, not guessed: the Comments mode replaces a comment line with an
    // EMPTY line, so the line count does not move and `declare_strip`'s
    // fallback wording ("inline comments trimmed") would understate 200 whole
    // lines as a trim. A line that had text and now has none was removed.
    // Two modes, two different truths, so the note is built per mode rather
    // than from one guessed count. Comments mode replaces a comment line with
    // an EMPTY line: text went, the newline stayed, so anchors hold. Lite mode
    // collapses blank RUNS: whole lines go, comments are preserved, anchors
    // move. One shared count would have called 479 collapsed blank lines "479
    // comment line(s) stripped" — which is what the first version of this did.
    let declare = |out: String, comments_mode: bool| {
        // **A numbered read (`cat -n`, `nl`) aligns by its NUMBERS.** A
        // stripped comment keeps its gutter (`    16`), so it never reads as
        // "a line that had text and now has none", and `nl` without `-ba`
        // prints blank lines bare — collapsing them dropped lines and sent
        // the note to "line numbers have SHIFTED", while the gutter shows
        // every original number. Measured 2026-10-02 on `nl cat.rs`: comments
        // gone, the note said only "28 blank runs collapsed".
        if let (Some(before), Some(after)) = (numbered_bodies(output), numbered_bodies(&out)) {
            let blanked = before
                .iter()
                .filter(|(n, had_text)| **had_text && after.get(n) == Some(&false))
                .count();
            let noted = if blanked > 0 {
                let gone = output.lines().count().saturating_sub(out.lines().count());
                let also = if gone > 0 {
                    format!("; {gone} blank line(s) removed")
                } else {
                    String::new()
                };
                let n = crate::code_strip::declare_strip(output, out, lang.tag(), Some(blanked));
                match n.split_once('\n') {
                    Some((head, rest)) if head.starts_with("[tokenade:") => {
                        format!("{head} (line numbers unchanged){also}\n{rest}")
                    }
                    _ => n,
                }
            } else if output.lines().count() == out.lines().count() {
                crate::code_strip::declare_strip(output, out, lang.tag(), None)
            } else {
                let gone = output.lines().count().saturating_sub(out.lines().count());
                format!(
                    "[tokenade:{}] {gone} blank line(s) removed; the numbers shown are the \
                     file's own\n{out}",
                    lang.tag()
                )
            };
            return (noted.len() < output.len()).then_some(noted);
        }
        let dropped = output.lines().count().saturating_sub(out.lines().count());
        // Position-by-position, so it only means anything while the two texts
        // still line up. The Comments mode collapses blank RUNS as well on
        // some languages, and once a line has gone the zip compares line N of
        // one against line N+k of the other: measured on a comment-FREE file
        // whose blank runs collapsed, that mis-alignment counted 80 "removed
        // comments" that never existed.
        let blanked = if dropped == 0 {
            output
                .lines()
                .zip(out.lines())
                .filter(|(before, after)| !before.trim().is_empty() && after.trim().is_empty())
                .count()
        } else {
            0
        };
        // Routed on what was MEASURED, not on which mode ran: the Comments
        // strip collapses blank runs too on some languages, so "which strip
        // was it" does not answer "what did the reader lose". A line that had
        // text and now has none is a comment removed; lines that vanished
        // outright are a blank run collapsed, and saying "comments stripped"
        // there is a claim about a file that may hold no comment at all.
        let _ = comments_mode;
        let noted = if blanked > 0 {
            let n = crate::code_strip::declare_strip(output, out, lang.tag(), Some(blanked));
            match n.split_once('\n') {
                Some((head, rest)) if head.starts_with("[tokenade:") => {
                    format!("{head} (line numbers unchanged)\n{rest}")
                }
                _ => n,
            }
        } else if dropped > 0 {
            // Lines that VANISHED are not all blank. On Rust the Comments mode
            // removes a `//` line outright, and this branch called 3 stripped
            // comments "3 line(s) of blank runs collapsed" (measured
            // 2026-10-02 on `sed -n 42,420p detect.rs`): a loss reported as
            // no loss. Text lines are counted, not inferred.
            let text = |t: &str| t.lines().filter(|l| !l.trim().is_empty()).count();
            let comments_gone = text(output).saturating_sub(text(&out));
            if comments_gone > 0 {
                format!(
                    "[tokenade:{}] {comments_gone} comment line(s) stripped, {dropped} line(s) \
                     fewer in all; the file on disk is unchanged (line numbers have SHIFTED)\n{out}",
                    lang.tag()
                )
            } else {
                format!(
                    "[tokenade:{}] {dropped} line(s) of blank runs collapsed; the file on disk is \
                     unchanged (line numbers have SHIFTED)\n{out}",
                    lang.tag()
                )
            }
        } else {
            crate::code_strip::declare_strip(output, out, lang.tag(), None)
        };
        (noted.len() < output.len()).then_some(noted)
    };
    // **A source read that fits keeps its comments.** Measured 2026-10-02 over
    // six weeks of transcripts: after `cat`/`sed -n` of a source file came back
    // `comments stripped`, the agent re-read the same file or the stash in
    // 21 % of `.ts` reads, 25 % of `.rs`, 18 % of `.py`, 25 % of `.sh` — two to
    // ten times the rate of `.sql` (2 %), `.ini` (2 %) or `Dockerfile` (4 %),
    // whose strippers stay as they are. A comment in code is where its intent
    // lives (`// wire-compat: do NOT renumber`). Under the delivery threshold
    // only blank runs are collapsed; above it, the declared strip still pays.
    if !tk_base::delivery::fits(output) {
        let comments = crate::code_strip::strip(output, lang, crate::code_strip::StripMode::Comments);
        if comments.len() < output.len() {
            return declare(comments, true);
        }
    }
    let lite = crate::code_strip::strip(output, lang, crate::code_strip::StripMode::Lite);
    if lite.len() < output.len() {
        return declare(lite, false);
    }
    None
}

/// Threshold in estimated tokens above which `cat` of a source file is
/// truncated with a recovery hint. Source files this large are almost
/// always exploratory reads — the agent wants structure, not raw bytes.
pub(super) const CAT_LARGE_TOKENS: usize = 2_000;

/// Above this many estimated tokens, a `sed -n 'A,Bp'` / `head` / `tail` slice
/// of a source file is treated as a browse and gets the code strip. Below it
/// the slice is a surgical read and comes back byte-exact.
pub(super) const RANGED_READ_MIN_TOKENS: usize = 1_500;
pub(super) const CAT_HEAD_LINES: usize = 150;
pub(super) const CAT_TAIL_LINES: usize = 50;

/// Returns a truncated version of `content` with a recovery hint when
/// `tokens > CAT_LARGE_TOKENS`. The hint tells the agent to use
/// `skeleton` or `Read` with offset/limit to reach the elided lines.
/// Truncate the BODY, keeping a leading tokenade banner out of the arithmetic.
///
/// `cat_try_code_strip` gained a declaration line, and the
/// truncation that runs after it counted that line as part of the file: a
/// 450-line source came back as `451 lines`, and the "N line(s) elided"
/// arithmetic was computed against the inflated total. The banner also has no
/// business inside the elidable middle — it is the one line that must survive.
pub(super) fn cat_truncate_keeping_banner(
    argv: &[String],
    content: String,
    tokens: usize,
) -> String {
    let Some(rest) = content.strip_prefix("[tokenade:") else {
        return cat_apply_large_truncation(argv, &content, tokens).unwrap_or(content);
    };
    let Some(nl) = rest.find('\n') else {
        return content;
    };
    let split = "[tokenade:".len() + nl;
    let (banner, body) = content.split_at(split);
    let body = body.strip_prefix('\n').unwrap_or(body);
    let body_tokens = crate::tokenizer::estimate(body);
    match cat_apply_large_truncation(argv, body, body_tokens) {
        Some(cut) => format!("{banner}\n{cut}"),
        None => content,
    }
}

/// The lines a reader of several files must still see where one file ends:
/// in a `cat -n` stream, a line with no number was printed by the CALLER (the
/// `echo "═══ $f"` of a loop), and a number starting again at 1 is a new file.
/// Measured 2026-10-05 on a real four-file loop: the cut kept the first header
/// only, so the declarations below it read as one file's.
fn caller_separators(content: &str) -> Vec<usize> {
    let numbered = |l: &str| {
        let t = l.trim_start();
        let digits = t.len() - t.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        digits > 0 && (t[digits..].starts_with('\t') || t[digits..].is_empty())
    };
    let lines: Vec<&str> = content.lines().collect();
    let n = lines.iter().filter(|l| numbered(l)).count();
    if n * 10 < lines.len() * 8 {
        return Vec::new();
    }
    let found: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| !l.starts_with("[tokenade"))
        .filter(|(_, l)| (!l.trim().is_empty() && !numbered(l)) || l.trim_start().starts_with("1\t"))
        .map(|(i, _)| i)
        .collect();
    // One file's own first line is not a boundary.
    if found.len() < 2 {
        return Vec::new();
    }
    found
}

pub(super) fn cat_apply_large_truncation(argv: &[String], content: &str, tokens: usize) -> Option<String> {
    if tokens <= CAT_LARGE_TOKENS {
        return None;
    }
    let filename = argv
        .iter()
        .skip(1)
        .find(|a| !a.starts_with('-'))
        .map(|s| s.as_str())
        .unwrap_or("<file>");
    // A slice of a file that HAS no lines is not a slice. A bundle is eight
    // rows of which one is ninety thousand characters, so `first 150 / last
    // 50` returns the whole thing and the agent receives minified soup it
    // cannot read at any length. Summarise what survives minification —
    // specifiers and exported names — instead.
    if crate::minified::detect(content) {
        let summary = crate::minified::compact(content);
        return Some(format!(
            "[tokenade:cat-large — {filename} ≈{tokens} tokens, minified. \
             `grep` it, or `Read` with offset/limit, for the bytes]\n{summary}"
        ));
    }
    // The middle keeps its DECLARATION HEADERS in place. Measured 2026-10-02
    // over this machine's transcripts: of 23 `cat-large` deliveries, the
    // agent went back for the middle of 3+ — "the exact body of `enroll()`",
    // the session middleware, a grep for `reason|error` — because the marker
    // said how many lines were gone and nothing of what they were. With the
    // signatures kept, each at its own place between counted gaps, the agent
    // can see `enroll()` is there and ask for exactly that body. Same parser
    // as `tokenade skeleton`; no grammar for the extension ⇒ the plain cut.
    let headers: Vec<usize> = std::path::Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .and_then(|ext| crate::treesitter::skeleton_spans(content, ext))
        .map(|spans| spans.iter().flat_map(|&(a, b)| (a.saturating_sub(1))..b).collect())
        .unwrap_or_default();
    // An outline that is most of the middle is not an outline: 600 three-line
    // functions keep 600 headers and the cut saves nothing. The headers stay
    // only while they are at most a quarter of the middle they stand for.
    let total_lines = content.lines().count();
    let middle = CAT_HEAD_LINES..total_lines.saturating_sub(CAT_TAIL_LINES);
    let in_middle = headers.iter().filter(|i| middle.contains(i)).count();
    let mut headers = if in_middle * 4 <= middle.len() { headers } else { Vec::new() };
    let outline = !headers.is_empty();
    headers.extend(caller_separators(content));
    headers.sort_unstable();
    headers.dedup();
    let truncated = crate::compress::truncate_middle_keeping(content, CAT_HEAD_LINES, CAT_TAIL_LINES, &headers);
    // Say what was actually DROPPED, and name a command that runs.
    //
    // The banner used to quote the two constants — "showing first 150/last 50
    // lines" — of a total it never gave, so the agent could not tell whether 3
    // lines or 3 000 were missing, nor pick the `offset` for the Read it was
    // being told to do. And the head is ADAPTIVE (`coverage_knee`), so those
    // constants were not even what happened. Then it advertised
    // `skeleton path="<file>"`, which the CLI answers `No such file or
    // directory` — the argument shape belongs to the MCP tool, not the command.
    //
    // MEASURED 2026-09-05 over this machine's transcripts: 23 of these were
    // delivered, at a 2.9 KB median, and NOT ONE was ever asked a question,
    // while the agent pulled the bytes back 6 times and went around the hook 3
    // times. A fold nobody questions is a fold whose banner never suggested a
    // question — so the cheapest path the shipped guidance names goes first.
    let kept_decl = if outline { ", its declarations kept in place" } else { "" };
    let total = content.lines().count();
    let kept = truncated.lines().count();
    let hidden = total.saturating_sub(kept);
    // `git show <rev>:<path>` arrives as `["git", "<rev>:<path>"]`: the file on
    // disk may be another version, so every path-based recourse would answer
    // for the wrong bytes. The stash holds THIS revision.
    let ask = if argv.first().map(String::as_str) == Some("git") {
        format!(
            "This is `{filename}`, not the file on disk: ask the stash named below \
             (`--prompt \"q1, q2\"`, `--grep <pat>`), or `git show {filename} | sed -n 'A,Bp'` \
             for the bytes"
        )
    } else {
        format!(
            "Ask it what you need: `tokenade read {filename} --prompt \"q1, q2\"` \
             (cheapest), or `tokenade skeleton {filename}` for the signatures, \
             or `Read` with offset/limit for the bytes"
        )
    };
    Some(format!(
        "[tokenade:cat-large — {filename}: {total} lines ≈{tokens} tokens, \
         {hidden} line(s) elided from the middle{kept_decl}. {ask}]\n\
         {truncated}"
    ))
}

/// Line number → "this line's body has text", for a numbered read; `None`
/// unless most lines carry a gutter (same bar as `line_gutter::without_gutter`).
fn numbered_bodies(text: &str) -> Option<std::collections::HashMap<u64, bool>> {
    crate::line_gutter::without_gutter(text)?;
    Some(
        text.lines()
            .filter_map(|l| {
                crate::line_gutter::strip_one(l)
                    .map(|(n, body)| (n, !body.trim().is_empty()))
                    // A stripped comment loses its body AND the tab before
                    // it: `    16` is line 16, now empty.
                    .or_else(|| l.trim().parse::<u64>().ok().map(|n| (n, false)))
            })
            .collect(),
    )
}
