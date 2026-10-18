//! **The two guarantees every input owes its caller.**
//!
//! 1. **Anti-inflation.** A compactor whose rendering costs MORE than the raw
//!    input must never win — found by fuzzing: Prometheus grouping
//!    on diverse labels grew the output by 44 %.
//! 2. **Error preservation.** An error line that a format compactor
//!    threw away is reattached. It is invariant 4: the agent must see
//!    the failure, and it is exactly the line a summary finds uninteresting.
//!
//! Added to that is the question that decides whether a rendering is honest: did it
//! even READ its input? The comment on the content-shape fallback ladder
//! keeps the measurement that answers no — and why it is not
//! wired.

use super::*;

/// Error-class lines the input contained that a per-format compactor
/// silently dropped (fuzzer invariant-4 catch: Tsc/Dns/Csv/… keep only
/// their expected shape and jettison foreign error lines in MIXED output).
/// Errors are the decisive class — elision disclosure is not enough, the
/// agent needs the location verbatim. Returns a bounded appendix.
/// Does this line carry an ERROR the reader must not lose?
///
/// Shared, because more than one place has to answer it and they were not
/// answering it the same way. `auto::guarded` re-attaches errors a per-format
/// compactor dropped; the head/tail fold in `disclose` did not ask at all, so an
/// error sitting in the middle of a large log was folded away and the agent read
/// a clean tail and concluded the run had passed. MEASURED: five error shapes
/// (a Python traceback frame, a JUnit `[ERROR]`, a Docker build failure, a Go
/// test `--- FAIL`, an `npm ERR!`) all vanished from a 2 500-line log of unique
/// lines.
///
/// Tight shapes only — prose containing the word "failed" must NOT match, or
/// the appendix fills with narration and pushes out the real thing.
pub(crate) fn line_looks_like_an_error(l: &str) -> bool {
    let t = l.trim_start();
    t.starts_with("error[")
            || t.starts_with("error:")
            || t.starts_with("ERROR")
            || t.starts_with("fatal:")
            || t.starts_with("FATAL")
            || t.starts_with("panic")
            || t.starts_with("FAILED ")
            || t.starts_with("Exception")
            // C11 (decapitation class): summary/tally lines are as decisive
            // as errors — jest/vitest/pytest print them at the END, where
            // head-caps and foreign-format compactors drop them.
            || t.starts_with("Tests:")
            || t.starts_with("Test Suites:")
            || {
                let mut words = t.split_whitespace();
                matches!(
                    (words.next(), words.next()),
                    (Some(n), Some("passed" | "failed" | "passing" | "failing" | "errors" | "warnings"))
                    if n.chars().all(|c| c.is_ascii_digit())
                )
            }
            || (t.split(':').next().map(|w| {
                w.len() > 4
                    && w.ends_with("Error")
                    && w.chars().next().map(char::is_uppercase).unwrap_or(false)
                    && w.chars().all(char::is_alphanumeric)
            }) == Some(true))
}

/// How many full substring scans of `output` this guard may pay before it
/// stops asking and simply re-attaches.
///
/// One `output.contains(line)` per candidate is quadratic, and the `>= 10`
/// break below does NOT bound it: the counter only advances on lines it fails
/// to find, so every candidate the compactor DID keep pays a full pass.
/// MEASURED on `cat` of 1,28 Mo of Rust source (`panic…`, `ERROR…`,
/// `SomeError:` all match [`line_looks_like_an_error`]): 6 123 ms in this one
/// function, ~n², extrapolating to ~10 min at the 12,7 Mo the gain ledger's
/// largest real payload actually carries.
///
/// The line index below answers the common case exactly and for free, so this
/// budget only limits the residue — lines a compactor reshaped rather than
/// kept. Running out is the SAFE direction for a guard whose whole job is not
/// losing errors: an unscanned candidate is re-attached, never dropped. The
/// cost of being wrong is one duplicated line in a ≤10-line appendix.
pub(crate) const CONTAINS_SCAN_BUDGET: usize = 64;

pub(crate) fn dropped_error_lines(input: &str, output: &str) -> Option<String> {
    // Index the output's lines ONCE. A compactor that keeps an error keeps it
    // as a LINE — measured on the archive-listing fold, where all six foreign
    // error lines come through verbatim between the folded runs — so this
    // answers "still present?" in O(1) for everything except reshaped lines.
    let kept: std::collections::HashSet<&str> = output.lines().map(str::trim).collect();
    let mut budget = CONTAINS_SCAN_BUDGET;
    let mut lost: Vec<String> = Vec::new();
    for raw in input.lines() {
        // Mutation-resistant: ANSI codes wrap real-world error lines too.
        // Pay the copy only when there IS an escape to strip.
        let l: std::borrow::Cow<'_, str> = if raw.contains('\x1b') {
            std::borrow::Cow::Owned(crate::ansi::strip_ansi(raw))
        } else {
            std::borrow::Cow::Borrowed(raw)
        };
        if !line_looks_like_an_error(&l) {
            continue;
        }
        let needle = l.trim();
        if kept.contains(needle) {
            continue;
        }
        // Not kept as a line of its own — it may still sit inside one the
        // compactor rebuilt. Scan for it while the budget lasts.
        if budget > 0 {
            budget -= 1;
            if output.contains(needle) {
                continue;
            }
        }
        lost.push(l.into_owned());
        if lost.len() >= 10 {
            break;
        }
    }
    if lost.is_empty() {
        return None;
    }
    Some(format!(
        "\n[errors preserved from elided lines]\n{}",
        lost.join("\n")
    ))
}

/// Split a short run of leading diagnostic lines from a document that
/// follows them, or `None` when there is no such split.
///
/// Deliberately narrow. At most [`MAX_DIAGNOSTIC_LINES`] lines, none of
/// which may open a structured value itself, and together they must be a
/// small fraction of the text — a "prefix" that is half the output is not a
/// prefix, it is the other half of a mixed document, and rebuilding one
/// section from its parse while dropping the other is the failure the
/// mixed-content veto in [`detect`] exists to prevent.
pub(crate) fn split_leading_diagnostics(text: &str) -> Option<(&str, &str)> {
    /// A tool warns in one or two lines. Five is already generous.
    const MAX_DIAGNOSTIC_LINES: usize = 5;
    /// Above this share of the bytes the head is content, not a preamble.
    const MAX_PREFIX_SHARE: usize = 20;

    let mut offset = 0usize;
    for _ in 0..MAX_DIAGNOSTIC_LINES {
        let rest = &text[offset..];
        let line_len = rest.find('\n').map(|i| i + 1)?;
        let line = rest[..line_len].trim();
        // A line that opens a structured value is the document, not a
        // preamble in front of it — stop before eating it.
        if line.starts_with('{') || line.starts_with('[') || line.starts_with('"') {
            break;
        }
        offset += line_len;
        if offset == 0 || offset * 100 > text.len() * MAX_PREFIX_SHARE {
            return None;
        }
        let body = &text[offset..];
        if body.trim_start().starts_with('{') || body.trim_start().starts_with('[') {
            return Some((&text[..offset], body));
        }
    }
    None
}

// NOT wired: `cmd::dispatch(text)`, the content-shape ladder over every
// per-command compactor, documents itself as "used when the program name is
// unknown" and has no caller. The hole it would fill is real and measured — on
// the command corpus the content-only path gains **4.1 %** where the
// name-routed path gains **34.3 %**, one `sc query` fixture accounting for
// 7 839 tokens of it.
//
// It is not safe to plug in as it stands, which is the more useful finding.
// The ladder tries compactors in order and the FIRST one that returns `Some`
// wins, with nothing checking that it understood the input — so it picks the
// wrong parser and that parser rebuilds the output from what it managed to
// read. MEASURED against the name-routed path, unique-word retention:
// `top` 6.1 % vs 100 %, `netstat -s` 7.2 % vs 100 %, `ps -e` 5.1 % vs 80.8 %,
// `sc query` 2.3 % vs 13.4 %. Wiring it raises the corpus "gain" from 4.1 % to
// 74.7 %, and that number is destruction, not compaction.
//
// Making it trustworthy means each `try_compact` proving it RECOGNISES its
// input before claiming it — per-module work across the whole `cmd/` tree.
// `a_content_shape_fallback_would_destroy_content` in `tests/idempotence.rs`
// holds the evidence so this is not re-attempted blind.

/// The two CENTRAL guards every entry point owes its caller, in one place.
///
/// 1. **Anti-inflation.** A compactor whose rendering costs more than the raw
///    input (fuzzer catch: Prometheus re-grouping on diverse label sets grew the
///    output 44 %) must never win.
/// 2. **Error preservation** (invariant 4): re-attach error lines a format
///    compactor dropped.
///
/// Both skip `Kind::Challenge`, which REPLACES a CAPTCHA wall with a short
/// remediation note — an intentional substitution, not a compaction, and the
/// note can legitimately be bigger than the stub it replaces.
///
/// This lived inline in [`compact`] and was described there as covering every
/// path. It did not: [`compact_with_path_hint`] and
/// [`compact_with_path_hint_indexed`] returned straight from `dispatch_kind`
/// whenever the extension pinned the kind — which is the branch the `Read` hook
/// takes on every recognised file. Measured on 1 325 real JSON files: a
/// tokenizer vocabulary came back **25,8 % BIGGER** than it went in, an OpenAPI
/// document 5,3 % bigger.
/// True when a compactor's summary counts several things and reports NONE of
/// each — the shape of a report about input that was never really read.
///
/// Deliberately narrow: a summary, not a fold (`MAX_SUMMARY`), on input big
/// enough that "nothing" is implausible (`MIN_INPUT_TOKENS`), carrying at
/// least `MIN_COUNTS` integers with no non-zero among them.
pub(crate) fn summarises_nothing_read(out: &str, before: usize) -> bool {
    const MAX_SUMMARY: usize = 200;
    const MIN_INPUT_TOKENS: usize = 100;
    const MIN_COUNTS: usize = 2;
    let out = out.trim();
    if out.len() > MAX_SUMMARY || before < MIN_INPUT_TOKENS {
        return false;
    }
    let mut counts = 0usize;
    for run in out.split(|c: char| !c.is_ascii_digit()) {
        if run.is_empty() {
            continue;
        }
        if run.bytes().any(|b| b != b'0') {
            return false;
        }
        counts += 1;
    }
    counts >= MIN_COUNTS
}

pub(crate) fn guarded(
    kind: Kind,
    text: &str,
    out: String,
    before: usize,
    after: usize,
) -> (Kind, String, Savings) {
    if after > before && !matches!(kind, Kind::Challenge) {
        return (kind, text.to_string(), Savings::new(before, before));
    }

    // Is there DECORATIVE PADDING left in what we are about to deliver?
    //
    // Phrased this way rather than « did the rung fold something? », and
    // both differences matter. The question is about the OUTPUT, so the
    // named rung's work is kept and only the decoration it left
    // goes. And it is stable by construction: once the padding is
    // gone, running it again changes nothing, whereas a threshold on « did it
    // fold » broke the fixed point — MEASURED, `ulimit -a` and
    // `Get-SmbShare` folded on the second pass what they had refused
    // on the first, because the named rung had nibbled 1 % then 3 %.
    //
    // MEASURED by `compactor_review`: `Kind::DashTable` claimed the
    // PowerShell tables and folded one percent of them, on outputs where
    // **76 %** of the bytes are column padding. The fold does not read a
    // word, so it cannot get the language wrong.
    //
    // Decided on BYTES: this estimator barely bills a run
    // of spaces, so a fold that removes 26 % of the bytes of `ulimit -a` leaves
    // the token count unmoved. The token condition stays as a safeguard —
    // the fold must never COST anything.
    let (out, after) = {
        // **By SEGMENT and on the WHOLE DOCUMENT, and the smaller wins.**
        //
        // Both readings existed, with different strengths, and only one
        // was the universal rung. By segment is stronger on a
        // document carrying several tables of different geometries; on the
        // whole document is stronger when the table is cut by
        // blank lines and no piece alone reaches five lines.
        //
        // MEASURED 2026-09-09: the output of the `netstat` compactor — 4 299 bytes
        // — qualified NO segment, whereas the whole document
        // qualified. It therefore went out as is, and the NEXT pass
        // recognised it as `AlignedBlock` and returned it at 2 021: the fixed
        // point was lost, and the guard `command_compaction_reaches_a_fixed_point`
        // said so without naming the cause. The gain was real and arrived one round
        // too late.
        let collapsed =
            crate::archetypes::collapse_padding_best(&out).unwrap_or_else(|| out.clone());
        let after2 = tokenizer::estimate(&collapsed);
        if collapsed.len() < out.len() && after2 <= after {
            (collapsed, after2)
        } else {
            (out, after)
        }
    };
    // A FLOOR, and it had none. A compactor that returns nothing at all on
    // non-empty input has not compacted, it has failed — and it fails
    // silently, because there is no banner in an empty string to say so.
    //
    // Measured 2026-08-26: `Kind::GraphQl` claims Rust source whose head holds
    // no keyword from its source-code veto (that list knows TypeScript and Go,
    // not `use`, `fn`, `impl`, `mod`), and its SDL rebuild then emits the empty
    // string. `lean-ctx/…/calibrator/selection.rs` came back as **38 063 bytes
    // to 0**, `llmtrim/…/stages/tools.rs` as **56 952 to 0**. Not folded, not
    // elided, not announced — gone.
    //
    // The floor is EMPTY, not "small": a one-line summary of a huge log is a
    // legitimate result and must stay one. Nothing at all never is.
    if out.trim().is_empty() && !text.trim().is_empty() {
        crate::debug_log::log(
            "auto",
            "compactor_returned_nothing",
            serde_json::json!({ "kind": format!("{kind:?}"), "bytes_in": text.len() }),
        );
        return (kind, text.to_string(), Savings::new(before, before));
    }
    // THE SAME FLOOR, one step up. The rule above asks "is the output empty",
    // and a guard that asks only that is structurally blind to the case where
    // the compactor produced a perfectly well-formed summary OF NOTHING —
    // `pnpm: 0 added, 0 warnings, 0 errors` is not empty, so it sails through.
    //
    // Earlier findings fixed exactly this for Ansible and for `cargo bench`,
    // one compactor at a time, which is an exemption list written backwards.
    // Asked HERE it costs nothing per compactor and covers the ones nobody has
    // hit yet — the placement argument `jcodemunch-mcp` makes for its own
    // column guard on 2026-08-26.
    //
    // Measured 2026-08-27 over 49 951 files: **one** hit, and it is a total
    // loss. A 6 865-byte `.gitignore` is claimed by `PkgInstall` and served
    // through `tokenade exec` as **36 bytes** — `pnpm: 0 added, 0 warnings,
    // 0 errors`, the report of an install that never ran, with the file gone.
    // `structured_or_generic` catches it on the two paths that consult it, but
    // a dozen callers take `auto::compact(..).1` straight to the agent: MCP,
    // the Gemini bridge, `llm_proxy`, `expand_ref`, and the sandbox that
    // `exec` runs on.
    //
    // TWO integers at least, and every one of them zero. One is not enough:
    // the same census flagged a 255-byte Python file whose only digit is a `0`
    // in `TypeVar`, and a rule that reads a lone zero as a count would hand
    // back source it should have stripped.
    if summarises_nothing_read(&out, before) {
        crate::debug_log::log(
            "auto",
            "compactor_summarised_nothing",
            serde_json::json!({
                "kind": format!("{kind:?}"),
                "bytes_in": text.len(),
                "summary": out.trim(),
            }),
        );
        return (kind, text.to_string(), Savings::new(before, before));
    }
    // THE THIRD FLOOR, and the one that was asked at only one door.
    //
    // The two above ask "is the output empty" and "is it a summary of
    // nothing". Neither sees the case where the summary is well formed, counts
    // real things, and still cannot be speaking for its input any more.
    // `structured_or_generic` has asked exactly that since the
    // `auto:pkginstall`-on-SSH leak (816 → 19 tokens) — inline, on the path
    // only a COMMAND reaches.
    //
    // Measured 2026-08-27 by planting a marker in a field each format itself
    // defines (a commit subject, a journal message, a request path, a test
    // name, an `openat` filename) rather than on a line of its own — a foreign
    // line changes what the document detects as, so the compactors that demand
    // format purity had never been reached at all:
    //
    //   strace, marker in an openat path   content 944 → 19   command +rejected
    //   Gradle, 8 modules × 4 tasks        content 877 → 11   command +rejected
    //
    // `Gradle BUILD SUCCESSFUL: 33 tasks (0 failed)` keeps no module and no
    // task name, on the shape whose SIBLING test says in as many words that
    // "the module names are what an agent needs from a reactor build". The
    // command door refuses both already, so this removes a divergence rather
    // than adding a cost.
    //
    // The FALLBACK differs by door on purpose. `structured_or_generic` hands
    // off to `generic`, because a command's output is `Unclaimed::Fold` and
    // something should still try. Here the answer is the input, exactly like
    // the two floors above it: this is the door that serves DOCUMENTS whole
    // so "we cannot compact this safely" already has an established
    // answer here and inventing a second one would make the three floors
    // disagree about what a floor returns.
    if crate::speaks_for::reduction_is_pathological(text, &out, before, after) {
        crate::debug_log::log(
            "auto",
            "compactor_reduction_was_pathological",
            serde_json::json!({
                "kind": format!("{kind:?}"),
                "before": before,
                "after": after,
                "summary": out.trim().chars().take(120).collect::<String>(),
            }),
        );
        return (kind, text.to_string(), Savings::new(before, before));
    }
    if !matches!(kind, Kind::Challenge) {
        if let Some(appendix) = dropped_error_lines(text, &out) {
            let out = format!("{out}{appendix}");
            let after2 = tokenizer::estimate(&out);
            if after2 <= before {
                return (kind, out, Savings::new(before, after2));
            }
            return (kind, text.to_string(), Savings::new(before, before));
        }
    }
    (kind, out, Savings::new(before, after))
}
