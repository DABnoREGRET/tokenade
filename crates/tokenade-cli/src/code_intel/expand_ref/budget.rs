//! **Refuse to return everything, and SAY what to do instead.**
//!
//! A blob returned whole can exceed the window that has just folded it away:
//! the agent would lose its context at the very moment of recovering it. So
//! refusing is the right answer — but a bare refusal is worse than nothing,
//! because it leaves the agent with no path.
//!
//! So every refusal carries its menu: the `--grep`, the bounded slice, the raw
//! path. A menu that leaves out an option hides it, and `--prompt` was missing
//! from the short banners while 48 % of real calls piped the output into
//! `head`.

use super::*;

/// What `--raw` returns, said in ONE sentence, for every place that offers it.
///
/// It was described FIVE times — "for every byte" in the compactors' banner,
/// "for all bytes" in the refusal of an unanswered `--prompt` and twice in a
/// page's fold, "the whole stored blob" in the two pieces of advice of
/// `too_large_advice` — and **all five promised the entirety**, whereas
/// `--raw` stops at a ceiling ever since a 7 MB blob came back whole (1.8 M
/// tokens in one call). Only the help told the truth, and the help is a
/// separate call the agent almost never makes: what it reads is the banner.
///
/// Measured 2026-09-09 on this machine's transcripts: **56 real calls to
/// `expand-ref`, of which 27 (48 %) piped into `head`, 11 (19 %) into
/// `sed -n`, 5 into `tail`** — and one session paging by hand with
/// `--raw | sed -n '1,200p'` then `'200,400p'`, which `--from-line` already
/// does. An agent that believes `--raw` is unlimited defends itself in the
/// shell, and its defence cuts in the wrong place.
pub(crate) use tokenade_core::disclose::RAW_ADVICE;

/// What a bounded `--raw` prints before it stops, in estimated tokens.
///
/// `--raw` is a deliberate escape hatch and stays one — but an agent reaching
/// for it cannot know what it is about to pay: the size line only appears on
/// the BARE path it just skipped. Measured 2026-08-25 on a 7.0 MB blob, `--raw`
/// returned all of it, roughly 1.8 M tokens in a single call, from the command
/// whose whole purpose is to make recovery cheap. Bounded by default, resumable
/// by an exact command, and `--budget 0` still hands over everything.
///
/// The ceiling is generous on purpose — twelve times the disclose threshold —
/// because reaching this path means the agent has already refused the cheap
/// answers and asked for bytes.
pub(crate) fn raw_budget_tokens() -> usize {
    if let Ok(v) = std::env::var("TOKENADE_RAW_BUDGET") {
        if let Ok(n) = v.parse::<usize>() {
            return n;
        }
    }
    tokenade_core::disclose::DEFAULT_THRESHOLD_TOKENS * 12
}

pub(crate) fn print_or_gate(hash: &str, red: &str, args: &ExpandArgs) {
    let raw = args.raw;
    // "the whole stored BLOB", not "all bytes" — the two are
    // not the same thing and the old wording promised the
    // second. `--raw` returns what was stored under this hash.
    // For a fold-store hash that IS the source, verified byte
    // for byte (359/359 lines on a `git log`). For a disclose
    // STASH hash it is whatever that path chose to stash, which
    // is often a folded summary: measured 2026-08-04, a Read of
    // `redact.rs` stashed 1879 lines of a 3387-line file, so
    // `--raw` returned 55 % of it under a flag that said "all
    // bytes anyway". The stash banner is honest about this
    // ("folded summary (omits middle lines)"); this sentence
    // was the one contradicting it. Telling the two hash kinds
    // apart at expand time needs stored provenance.
    //
    // Bare = raw-bytes escape, size-gated. Above the RAG
    // threshold a full dump just re-inflates context, so REFUSE
    // and point at the question path — unless `--raw` explicitly
    // forces the bytes. Small blobs dump as before (cheap, and
    // often exactly what's wanted). See `cmd_expand_ref` for what
    // the field data says, and for the fact that the figure this
    // gate was built on has since MOVED because of the gate.
    let toks = tokenade_core::tokenizer::estimate(red);
    if toks > tokenade_core::disclose::threshold_tokens() && !raw {
        // Which recovery to name FIRST depends on what the blob
        // is. Over a record stream — grep output, a log, a path
        // list — a natural-language question retrieves badly and
        // it is not a ranking bug: a block of adjacent grep lines
        // has no topic, so neighbouring lines are unrelated.
        // MEASURED on a 14 000-line grep stash: asking "which
        // function decides if a value is code" returned 5 561
        // characters that did not contain the answer, while
        // `--grep value_is_code` returned the one defining line.
        // Over prose the order is the other way round, which is
        // why this reads the blob instead of picking a default.
        let banner = format!("[tokenade: {}]", too_large_advice(toks, hash, red));
        // NO CREDIT HERE. This branch retrieves nothing: it refuses the
        // dump and prints a pointer. The tokens it "saves" are the ones the
        // fold already saved, and the fold already booked them
        // (`disclose:stash`), so crediting them again claims the same bytes
        // twice. Measured 2026-09-02 on a live ledger: the single largest
        // recovery row on the machine — 393 740 tokens — was this branch
        // printing a sixty-token banner.
        outln!("{banner}");
    } else if raw {
        print_raw_bounded(hash, red, args);
    } else {
        out!("{red}");
    }
}

/// Would a BARE `expand-ref` on a blob this size be refused?
///
/// One spelling, so the fold banner and the refusal cannot disagree about which
/// blobs are dumpable.
pub(crate) fn bare_dump_refused(toks: usize) -> bool {
    toks > tokenade_core::disclose::threshold_tokens()
}

/// What to do with a blob too large to dump whole — the sentence, WITHOUT the
/// banner brackets, so the refusal and the fold banner say the same thing.
///
/// Said at the earliest moment it is true. The fold banner used to advertise a
/// bare `tokenade expand-ref <hash>` (and `--raw`) even for blobs this gate
/// refuses: measured 2026-09-06, a fold of 1 225 395 tokens pointed at a bare
/// expand that answered "too large to dump whole" in 333 bytes — a whole turn
/// spent learning something the fold already knew, since the bytes and their
/// size were both in hand when the banner was written.
///
/// Which recovery to name FIRST depends on what the blob is. Over a record
/// stream — grep output, a log, a path list — a natural-language question
/// retrieves badly and it is not a ranking bug: a block of adjacent grep lines
/// has no topic, so neighbouring lines are unrelated. MEASURED on a 14 000-line
/// grep stash: asking "which function decides if a value is code" returned
/// 5 561 characters that did not contain the answer, while
/// `--grep value_is_code` returned the one defining line. Over prose the order
/// is the other way round, which is why this reads the blob instead of picking
/// a default.
pub(crate) fn too_large_advice(toks: usize, hash: &str, blob: &str) -> String {
    format!(
        "this blob is ~{toks} tokens — too large to dump whole. {}",
        recovery_menu(hash, blob)
    )
}

/// **The three recourses, all named, in the order the blob's SHAPE dictates.**
///
/// Written once because the two banners did not say the same thing. The long
/// one — for a blob too big to be served whole — named all three and chose
/// its order by reading the blob. The SHORT one, by far the most frequent,
/// named only TWO: `--grep` and `--raw`. **It left out `--prompt`**, i.e. the
/// natural-language question, which is both the cheapest option and the one
/// the product recommends first on prose. An agent cannot choose an option it
/// is not shown.
///
/// The order is not decorative and it is measured: over a record stream —
/// `grep` output, a log, a path list — a natural-language question retrieves
/// badly, because a block of adjacent lines has no topic. MEASURED on a
/// 14 000-line `grep` stash: "which function decides if a value is code"
/// returned 5 561 characters that did not contain the answer, while
/// `--grep value_is_code` returned the defining line. Over prose, the order is
/// reversed.
pub(crate) use tokenade_core::disclose::recovery_menu;

/// How far a bounded `--raw` reaches from `start`, and what it costs.
///
/// Split out so the guard exercises THIS and not a copy of it: the first
/// version of that test re-implemented the loop in the test body, which proves
/// only that the copy agrees with itself.
pub(crate) fn raw_slice(lines: &[&str], start: usize, budget: usize) -> (usize, usize) {
    let mut used = 0usize;
    let mut end = start;
    while end < lines.len() {
        let cost = tokenade_core::tokenizer::estimate(lines[end]) + 1;
        // Always emit at least one line: a budget smaller than the first line
        // must not print an empty answer and call it a slice.
        if end > start && used + cost > budget {
            break;
        }
        used += cost;
        end += 1;
    }
    (end, used)
}

/// `--raw`, bounded and resumable.
///
/// Verbatim is kept — this prints the stored bytes, unaltered, in order. What
/// changes is that it STOPS, and says exactly how to continue. An agent that
/// asked for bytes gets bytes; an agent that asked for 1.8 M tokens of them
/// gets the first slice plus one command, instead of a context it cannot undo.
pub(crate) fn print_raw_bounded(hash: &str, red: &str, args: &ExpandArgs) {
    let budget = args.budget.unwrap_or_else(raw_budget_tokens);
    let lines: Vec<&str> = red.lines().collect();
    let total = lines.len();
    let start = args.from_line.saturating_sub(1).min(total);

    if budget == 0 {
        for l in &lines[start..] {
            outln!("{l}");
        }
        return;
    }

    let (end, used) = raw_slice(&lines, start, budget);
    for l in &lines[start..end] {
        outln!("{l}");
    }
    if end < total {
        let remaining = total - end;
        outln!(
            "[tokenade: lines {}-{} of {total} (~{used} tokens). {remaining} line(s) left — \
             continue with `tokenade expand-ref {hash} --raw --from-line {}`, narrow with \
             `--grep <pattern>`, or take the rest in one go with `--budget 0`.]",
            start + 1,
            end,
            end + 1
        );
    }
}
