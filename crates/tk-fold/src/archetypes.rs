//! Reusable compaction archetypes.
//!
//! Most shell commands fall into a handful of output shapes. Rather
//! than duplicate the same fold/cap logic in every per-command file,
//! we centralise it here and have each per-command file:
//!
//! 1. Own its detector (so dispatch routing remains precise).
//! 2. Delegate the actual compaction work to one archetype helper.
//!
//! This keeps the "one file per command" constraint while avoiding
//! 569 copy-pasted fold loops.

use crate::tokenizer;
use std::cell::Cell;
use std::fmt::Write;

thread_local! {
    /// Set whenever a *command-agnostic* fold helper in this module
    /// (`line_cap`, `head_tail`, `strip_noise`, `collapse_padding`,
    /// `kv_records`) actually reduces the current command's output.
    ///
    /// This is the runtime signal that distinguishes a compactor that
    /// truly parses its command's format (df's disk-usage table, ls's
    /// permission columns, git status's porcelain) from a *stub* whose
    /// `detect` fires on arbitrary text and just blind head/tail-folds
    /// it. Both produce `filter == prog.to_string()` so the filter name
    /// alone can't tell them apart; this marker can. `filter.rs` reads
    /// it (via [`generic_fold_used`]) right after a per-command module
    /// returns, to compute `FilterResult.specific`.
    ///
    /// Conservative by design: a structural compactor that *also* uses a
    /// generic helper to bound its already-parsed output is reported as
    /// non-specific (under-counts coverage, never over-counts) — exactly
    /// the bias we want when answering "which commands still need a real
    /// compactor".
    static GENERIC_FOLD: Cell<bool> = const { Cell::new(false) };
}

mod columns;
mod padding;
mod records;
mod shapes;
pub use columns::*;
pub use padding::*;
pub use records::*;
pub use shapes::*;

/// Clear the generic-fold marker. `filter.rs` calls this once at the top
/// of `filter_command`, before routing the output through any module.
pub fn reset_generic_fold() {
    GENERIC_FOLD.with(|c| c.set(false));
}

/// Note that compactor `name`, written for this program, refused its
/// output and that we fall back to the generic path.
///
/// **Why this refusal deserves a name.** `try_compact` returns `Option<String>`, and a `None` means TWO
/// opposite things that nothing told apart: "this is not my shape, I
/// rightly let it pass" and "this really is MY
/// program's output and I did not recognise it". The second one produced
/// a series of defects — and each time it was
/// only found because someone had captured the right fixture,
/// in the right locale, on the right machine.
///
/// When argv NAMES a program whose module we own, a refusal
/// is suspect by construction: the output almost surely comes from that
/// program. Noting it changes nothing in what is delivered; it gives a
/// name to a silence.
///
/// **Written to the LOG, no longer to a thread cell.** The original
/// version put the name in a thread-local `Cell` that a twin
/// reader — `declined_named_compactor()` — could read back. That reader
/// never had a caller: the refusal got a name, and the name stayed
/// as silent as the refusal. The diagnostic log is the only
/// place anyone will look for it, and it costs nothing as long as
/// `TOKENADE_DEBUG_LOG` is not set.
pub fn mark_named_compactor_declined(name: &'static str) {
    crate::debug_log::log(
        "filter",
        "named_compactor_declined",
        serde_json::json!({ "compactor": name }),
    );
}

/// Record that a command-agnostic fold helper produced the compaction.
fn mark_generic_fold() {
    GENERIC_FOLD.with(|c| c.set(true));
}

/// True if a command-agnostic fold helper reduced the output since the
/// last [`reset_generic_fold`].
pub fn generic_fold_used() -> bool {
    GENERIC_FOLD.with(|c| c.get())
}

/// Pass through unchanged, trim trailing whitespace. Good for short
/// fixed outputs (`whoami`, `id`, `date`, `pwd`).
///
/// Neutral w.r.t. the generic-fold marker: a passthrough does not
/// compact, so it never claims (or denies) command-specific coverage.
/// Render one capped section of a report: a header, the kept lines, and an
/// honest count of what was left out.
///
/// Eleven copies of these fifteen lines existed — eight in `pkginstall`, three
/// in `ghactions` — differing only in the label, the noun and which vector was
/// being rendered. Verified identical by normalised-shape hash before folding.
///
/// The elided count is NOT cosmetic: it is the difference between "here are the
/// errors" and "here are SOME of the errors", and every copy carried it, which
/// is exactly why one place should own it.
pub fn capped_section(out: &mut String, label: &str, items: &[String], total: usize, noun: &str) {
    capped_section_with_header(out, &format!("{label} ({total})"), items, total, noun);
}

/// [`capped_section`] with the header spelled by the caller.
///
/// Two header styles coexist and BOTH are load-bearing: `errors (12)` says how
/// many exist, `errors (5 shown)` says how many survived — and `strace` says
/// `shown verbatim` because that is its promise. A boolean flag would have
/// flattened three distinct sentences into one; the header is the parameter.
pub fn capped_section_with_header(
    out: &mut String,
    header: &str,
    items: &[String],
    total: usize,
    noun: &str,
) {
    use std::fmt::Write as _;
    if items.is_empty() {
        return;
    }
    let _ = write!(out, "\n{header}:\n");
    for it in items {
        out.push_str("  ");
        out.push_str(it);
        out.push('\n');
    }
    if total > items.len() {
        let _ = writeln!(out, "  … {} more {noun} elided", total - items.len());
    }
}

/// Collapse a RUN of consecutive lines the predicate accepts into head + an
/// honest count + tail.
///
/// Four compactors carried these twenty-two lines — `chmod`, `pgrep`, `ls`,
/// `install` — differing only in the predicate, the noun and their own
/// thresholds. Verified identical by normalised-shape hash before folding.
///
/// Returns `Some((index past the run, lines hidden))`, or `None` when the run
/// is too short to be worth folding — in which case the caller has printed
/// nothing and continues line by line.
pub fn fold_run(
    out: &mut String,
    lines: &[&str],
    start: usize,
    matches: impl Fn(&str) -> bool,
    threshold: usize,
    keep_head: usize,
    keep_tail: usize,
    noun: &str,
) -> Option<(usize, usize)> {
    use std::fmt::Write as _;
    if !matches(lines.get(start)?) {
        return None;
    }
    let mut end = start;
    while end < lines.len() && matches(lines[end]) {
        end += 1;
    }
    let run = end - start;
    if run <= threshold {
        return None;
    }
    for l in &lines[start..start + keep_head] {
        out.push_str(l);
        out.push('\n');
    }
    let hidden = run - keep_head - keep_tail;
    let _ = writeln!(out, "... ({hidden} {noun}) ...");
    for l in &lines[end - keep_tail..end] {
        out.push_str(l);
        out.push('\n');
    }
    Some((end, hidden))
}

pub fn passthrough(text: &str) -> String {
    text.trim_end().to_string()
}

/// Cap at `max` non-empty lines, with `"[... N more <noun> elided]"` summary.
pub fn line_cap(text: &str, max: usize, noun: &str) -> String {
    line_cap_with(text, max, noun, true)
}

/// [`line_cap`] for SEARCH RESULTS: no line is rescued past the cap for
/// looking like a failure. A `grep` hit that contains `error` is a result
/// like the others — often the very word searched for — not a failure of the
/// run. Measured 2026-10-03, `grep -rn 'fn ' crates/tk-fold/src`: ten test
/// names (`des_erreurs_suivies_…`, `…_overflow_…`) were pulled out of order
/// and announced as "kept below because they look like failures".
pub fn line_cap_results(text: &str, max: usize, noun: &str) -> String {
    line_cap_with(text, max, noun, false)
}

fn line_cap_with(text: &str, max: usize, noun: &str, rescue: bool) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    // Per-line trailing-whitespace strip. Many sources have 2-10
    // trailing spaces per line (fixed-width terminal padding,
    // indented code in grep output). Trim per line then re-join.
    // Applies uniformly to grep / ack / ag / aircrack / apt-cache
    // and 50+ other compactors that use this archetype.
    if lines.len() <= max {
        let mut out = String::with_capacity(text.len());
        for line in &lines {
            out.push_str(line.trim_end());
            out.push('\n');
        }
        return out.trim_end().to_string();
    }
    mark_generic_fold();
    // Same rule as `head_tail`, and this one reaches further: `line_cap` keeps a
    // HEAD and nothing else, so on 50+ compactors (grep, ack, ag, apt-cache …)
    // every failure past the cap vanished — with no tail to catch it either.
    // MEASURED 2026-08-04: 300 lines of `unit N loaded ok` with
    // `ERROR: unit 251 failed to load` at position 251, capped at 60 → the
    // error was gone and the marker said only "240 more elided".
    //
    // Bounded at `MAX_RESCUED` so an all-error listing still folds, and the
    // marker states the rescue so the elided count is never quietly wrong.
    const MAX_RESCUED: usize = 10;
    let rescued: Vec<&str> = lines[max..]
        .iter()
        .copied()
        .filter(|l| rescue && crate::compress::is_error_line(l))
        .take(MAX_RESCUED)
        .collect();
    let mut out = String::with_capacity(text.len() / 2);
    for line in lines.iter().take(max) {
        out.push_str(line.trim_end());
        out.push('\n');
    }
    let elided = lines.len() - max - rescued.len();
    if rescued.is_empty() {
        let _ = writeln!(out, "  [... {elided} more {noun} elided]");
    } else {
        let _ = writeln!(
            out,
            "  [... {elided} more {noun} elided, {} kept below because they look like failures]",
            rescued.len()
        );
        for l in &rescued {
            out.push_str(l.trim_end());
            out.push('\n');
        }
    }
    out.trim_end().to_string()
}

/// Drop blank lines, collapse runs of >2 spaces to 2. Good for
/// fixed-width tables (Windows wmic, sc query verbose).
pub fn collapse_padding(text: &str) -> String {
    let mut out = String::with_capacity(text.len() / 2);
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let mut compacted = String::with_capacity(line.len());
        let mut spaces = 0usize;
        for c in line.chars() {
            if c == ' ' {
                spaces += 1;
            } else {
                if spaces > 0 {
                    compacted.push_str(if spaces == 1 { " " } else { "  " });
                    spaces = 0;
                }
                compacted.push(c);
            }
        }
        out.push_str(compacted.trim_end());
        out.push('\n');
    }
    let out = out.trim_end().to_string();
    if out.len() < text.trim_end().len() {
        mark_generic_fold();
    }
    out
}

/// Drop lines matching any of the `noise_prefixes` (after trim_start).
/// Append `"[elided: N <noun> line(s)]"` if any were dropped.
pub fn strip_noise(text: &str, noise_prefixes: &[&str], noun: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut elided = 0usize;
    for line in text.lines() {
        let t = line.trim_start();
        // A prefix that BEGINS with whitespace is matched against the raw
        // line, not the trimmed one — otherwise it can never match anything
        // and the rule is dead on arrival. Measured 2026-08-28 across the
        // callers: **twelve such prefixes in seven files**, every one of them
        // inert — `java_runtime`'s four JVM-noise rules (`    at java.base/`,
        // `    at sun.reflect`, …), `python_runtime`'s two stdlib-frame rules
        // (`  File "/usr/lib/python`), `ruby_runtime`'s two, `zip`'s
        // `  adding: ` (which is the BULK of `zip -r` output), and the bare
        // indent guards in hashcat, npm_audit and openssl_cli. Verified on a
        // JVM trace: 90 framework frames, `[elided: …]` never printed.
        //
        // Trimming stays the default, and deliberately: an indented `bzip2:`
        // is still the same noise line as an unindented one.
        if noise_prefixes.iter().any(|p| {
            if p.starts_with(|c: char| c.is_whitespace()) {
                line.starts_with(p)
            } else {
                t.starts_with(p)
            }
        }) {
            elided += 1;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if elided > 0 {
        mark_generic_fold();
        let _ = writeln!(out, "  [elided: {elided} {noun} line(s)]");
    }
    out.trim_end().to_string()
}

/// Generic NON-decapitating fold: keep the first `head` and the last
/// `tail` non-empty lines verbatim, fold only the middle into a count.
///
/// This is the structure-preserving replacement for blind head-slicing.
/// The tail of a command's output is where the *outcome* lives — the
/// error line, the final tally, the "N failed" summary, the exit
/// diagnostic. A head-only fold (`git_listing::fold`) decapitates that;
/// `head_tail` never does. Returns `None` when total ≤ head + tail
/// (nothing to fold without losing content).
pub fn head_tail(text: &str, head: usize, tail: usize, label: &str, noun: &str) -> Option<String> {
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim_end())
        .filter(|l| !l.is_empty())
        .collect();
    let total = lines.len();
    if total <= head + tail {
        return None;
    }
    // Gated: never cut inside a fenced code block. Grow the head/tail windows
    // to whole-fence boundaries so we never emit a dangling ``` (which would
    // corrupt the block and hide where code/JSON ends) — the misfire class a
    // generic line fold otherwise hits on tool output with embedded code.
    let (head, tail) = if fence_guard_enabled() {
        fence_adjust(&lines, head, tail)
    } else {
        (head, tail)
    };
    if total <= head + tail {
        return None; // adjustment swallowed the middle — emit raw, don't fold.
    }
    mark_generic_fold();
    // What stands out survives the middle. `error_survival.rs` fixed exactly
    // this for logs of DISTINCT lines; the shape that still lost its one
    // decisive line is the opposite one — near-uniform output with a single
    // anomaly. MEASURED 2026-08-04: 800 lines of `job-N step i of 800 ok` with
    // `ERROR: … checksum mismatch` at position 401 came back with the first 60
    // and the last 3, and no trace of the error. The bytes stayed in the stash,
    // which is no help: the delivered text gave the agent no reason to look.
    //
    // Bounded, because an all-error log must not defeat the fold: at most
    // `MAX_RESCUED` lines are lifted out of the elided range, in order, and the
    // marker says how many were rescued so the count is never silently wrong.
    const MAX_RESCUED: usize = 10;
    let rescued: Vec<&str> = lines[head..total - tail]
        .iter()
        .copied()
        .filter(|l| crate::compress::is_error_line(l))
        .take(MAX_RESCUED)
        .collect();
    let mut out = format!("[tokenade:{label}] {total} {noun}\n");
    for l in &lines[..head] {
        out.push_str(l);
        out.push('\n');
    }
    let elided = total - head - tail - rescued.len();
    if rescued.is_empty() {
        let _ = writeln!(out, "  ... (+{elided} more) ...");
    } else {
        let _ = writeln!(
            out,
            "  ... (+{elided} more, {} kept below because they look like failures) ...",
            rescued.len()
        );
        for l in &rescued {
            out.push_str(l);
            out.push('\n');
        }
    }
    for l in &lines[total - tail..] {
        out.push_str(l);
        out.push('\n');
    }
    Some(out.trim_end().to_string())
}

/// The `detect` guard of a `try_compact`, written once.
///
/// `cmd_entry_points!` derives `compact` FROM `try_compact`, and 785 modules use
/// it. Seventy-seven modules hold the INVERSE relation — their
/// `compact` is hand-written, so their `try_compact` is derived from
/// `detect` — and each spelled it on its own: **56 copies** of one body and
/// **21** of another, measured by `tokenade-bench --bin duplication`.
///
/// This is not cosmetic: this guard IS the contract that makes a
/// compactor defensive. Every `try_compact` starts with "if this is not my
/// output, I do not answer", and that is what makes a wrong
/// recognition fold nothing instead of destroying. Spelled seventy-seven
/// times, it can be forgotten once; written here, it no longer
/// can be.
///
/// Two shapes, because measurement found two: the output folded by
/// the module's `compact`, or delegated to a family compactor.
///
/// On the DELEGATED shape the guard changes nothing today, and that is measured
/// rather than assumed: the **21 modules** using it all have
/// `detect = any_output`, and `compact_if_json` already refuses empty and
/// blank input — neutralising the guard does not move a byte. It stays, so that
/// both shapes carry the SAME contract: a delegated module that gained a
/// real `detect` would already have it. A recorded equivalent mutant is better than a
/// test written to disguise it.
#[macro_export]
macro_rules! detect_gated_try_compact {
    () => {
        pub fn try_compact(text: &str) -> Option<String> {
            if !detect(text) {
                return None;
            }
            Some(compact(text))
        }
    };
    ($fold:path) => {
        pub fn try_compact(text: &str) -> Option<String> {
            if !detect(text) {
                return None;
            }
            $fold(text)
        }
    };
}
pub use crate::detect_gated_try_compact;

#[macro_export]
macro_rules! cmd_entry_points {
    ($fallback:path) => {
        pub fn compact(text: &str) -> String {
            try_compact(text).unwrap_or_else(|| $fallback(text))
        }

        pub fn compact_with_savings(text: &str) -> (String, $crate::Savings) {
            $crate::archetypes::savings_of(text, compact)
        }
    };
}
pub use crate::cmd_entry_points;

#[cfg(test)]
include!("archetypes_tests.rs");
