//! **Records: name what is elided.**
//!
//! Eliding three lines without saying which makes the output unrecoverable. A
//! record has a label; the elision mark carries it, which makes
//! "is what I am looking for there?" decidable without reopening the stash.

use super::*;

/// Whether the fenced-code-block fold guard is enabled. Default ON — it's a
/// correctness guard (never emit a dangling ``` that hides where code/JSON
/// ends), and the token cost is negligible (only widens a window when a fence
/// straddles the head/tail boundary, which is rare in command output). Opt out
/// with `TOKENADE_FENCE_GUARD=0`.
pub(super) fn fence_guard_enabled() -> bool {
    std::env::var("TOKENADE_FENCE_GUARD")
        .map(|v| v != "0")
        .unwrap_or(true)
}

/// A line that opens or closes a Markdown code fence.
pub(super) fn is_fence(l: &str) -> bool {
    l.trim_start().starts_with("```")
}

/// Grow `head` forward and `tail` backward so neither window boundary lands
/// inside an open fence (odd number of fence markers ⇒ unterminated). Caps at
/// `total`; the caller re-checks `head + tail` afterwards.
pub(super) fn fence_adjust(lines: &[&str], head: usize, tail: usize) -> (usize, usize) {
    let total = lines.len();
    let mut h = head;
    while h < total && lines[..h].iter().filter(|l| is_fence(l)).count() % 2 == 1 {
        h += 1;
    }
    let mut t = tail;
    while t < total && lines[total - t..].iter().filter(|l| is_fence(l)).count() % 2 == 1 {
        t += 1;
    }
    (h, t)
}

/// The names of the records an elision drops, when they have names.
///
/// A key/value record identifies itself in its first field — `Nom : 7zip`,
/// `Package: libfoo`, `Name: worker-3`. `pacman -Qi` folded 89 records to
/// three plus `... (+86 more)`, and eighty-six package names is not the same
/// fact as the number eighty-six: naming them is what the elision is FOR.
/// Same rule the folded path list has carried since it was written.
///
/// Returns `""` when the records do not identify themselves, so a stream that
/// is only loosely key/value keeps the bare count rather than a line of
/// garbage. A value is taken only from a first line whose KEY reads like a
/// key: letters, digits, spaces and light punctuation, nothing that would
/// make a timestamp or a URL split here by accident.
pub(super) fn elided_record_names(tail: &[&str], rescued: &[&&str]) -> String {
    const MAX_NAMES: usize = 12;
    const MAX_CHARS: usize = 220;
    let names: Vec<&str> = tail
        .iter()
        .filter(|r| !rescued.iter().any(|k| **k == **r))
        .filter_map(|r| record_label(r))
        .collect();
    if names.is_empty() {
        return String::new();
    }
    let mut shown: Vec<&str> = Vec::new();
    let mut used = 0usize;
    for n in names.iter().take(MAX_NAMES) {
        if used + n.len() > MAX_CHARS {
            break;
        }
        used += n.len() + 2;
        shown.push(n);
    }
    if shown.is_empty() {
        return String::new();
    }
    let rest = names.len() - shown.len();
    if rest > 0 {
        format!(": {}, and {rest} other(s)", shown.join(", "))
    } else {
        format!(": {}", shown.join(", "))
    }
}

/// The identifying value of a key/value record, from its first field.
pub(super) fn record_label(record: &str) -> Option<&str> {
    let first = record.lines().find(|l| !l.trim().is_empty())?;
    let (key, value) = first.split_once(':')?;
    let key = key.trim();
    if key.is_empty()
        || key.len() > 40
        || !key.chars().next()?.is_alphabetic()
        || !key
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '.' | '_' | '-' | '\'' | '\u{2019}'))
    {
        return None;
    }
    let value = value.trim();
    (!value.is_empty() && value.len() <= 60).then_some(value)
}

/// Fold a key/value record listing — paragraphs of `key: value` (or
/// `key = value`) lines separated by blank lines (apt show, systemctl
/// status, gcloud/aws describe, dmidecode, `*-info` dumps). Keeps the
/// first `head` records intact, folds the rest into a count. Records
/// preserve their internal structure; we never cut mid-record.
/// Returns `None` when records ≤ head.
pub fn kv_records(text: &str, head: usize, label: &str, noun: &str) -> Option<String> {
    // Split on blank-line boundaries into records.
    let records: Vec<&str> = text
        .split("\n\n")
        .map(|r| r.trim_matches('\n'))
        .filter(|r| !r.trim().is_empty())
        .collect();
    let total = records.len();
    if total <= head {
        return None;
    }
    mark_generic_fold();
    let mut out = format!("[tokenade:{label}] {total} {noun}\n");
    for (i, r) in records.iter().take(head).enumerate() {
        out.push_str(r);
        if i + 1 < head {
            out.push_str("\n\n");
        }
    }
    // Third fold of the same family, and the only one where the unit is a
    // RECORD, not a line. Lifting a bare line out of a key/value stream would
    // corrupt the shape it exists to preserve, so whole records are rescued —
    // a record counts as a failure when any of its lines does.
    const MAX_RESCUED: usize = 5;
    let rescued: Vec<&&str> = records[head..]
        .iter()
        .filter(|r| r.lines().any(crate::compress::is_error_line))
        .take(MAX_RESCUED)
        .collect();
    let elided = total - head - rescued.len();
    let named = elided_record_names(&records[head..], &rescued);
    if rescued.is_empty() {
        let _ = write!(out, "\n  ... (+{elided} more{named})");
    } else {
        let _ = write!(
            out,
            "\n  ... (+{elided} more{named}, {} kept below because they look like failures)",
            rescued.len()
        );
        for r in &rescued {
            out.push_str("\n\n");
            out.push_str(r);
        }
    }
    Some(out)
}

/// Returns true when `text` looks like a whitespace-aligned columnar
/// table: a header line followed by data rows that share column
/// structure (≥2 runs of ≥2 spaces, consistently). Cheap heuristic
/// used by per-command detectors to confirm the expected shape before
/// applying a table fold — so a command that unexpectedly errors out
/// (single line, stack trace) is *not* mis-folded as a table.
/// The two entry points every command module must expose for the auto-dispatch
/// table: `compact` (this module's fold, or the fallback when it declines) and
/// `compact_with_savings` (the same, measured).
///
/// 785 modules carried these six lines verbatim — one contract, restated once
/// per command, and impossible to change anywhere. The fallback stays at the
/// call site because it is the one thing that actually varies: `passthrough`
/// for text, `crate::json::compact` for output that is still JSON worth
/// folding generically.
/// The four items the auto-dispatch table requires of every `.rs` under the
/// command directories, for modules that are HELPERS rather than commands:
/// they detect nothing and fold nothing of their own, they exist so other
/// modules can call into them.
///
/// Eight modules spelled the quartet out. It carries no decision — it is the
/// shape of the registration mechanism, and a mechanism should be stated where
/// it is defined, not once per participant.
// `#[macro_export]` on the three macros of this file: a `macro_rules!`
// does not cross a crate boundary otherwise, and the fold layer has
// been one since 2026-09-13. They land at the root of `tk-fold`;
// the `pub use crate::<name>` following each one gives back the path that 867 sites
// already spell — `crate::archetypes::<name>!`.
#[macro_export]
macro_rules! cmd_helper_module {
    () => {
        pub fn detect(_text: &str) -> bool {
            false
        }

        pub fn try_compact(_text: &str) -> Option<String> {
            None
        }

        pub fn compact(text: &str) -> String {
            $crate::archetypes::passthrough(text)
        }

        pub fn compact_with_savings(text: &str) -> (String, $crate::Savings) {
            $crate::archetypes::savings_of(text, compact)
        }
    };
}

pub use crate::cmd_helper_module;

/// True when `text` is a non-empty JSON OBJECT carrying `marker`.
///
/// Twenty-five command modules — the `az` family and its neighbours — spelled
/// this out. The rule is one rule: a CLI that answers in JSON is ours to fold
/// only when the response is an object and carries the key that identifies it.
/// The key stays at the call site; it is the only thing that varies.
pub fn json_object_with(text: &str, marker: &str) -> bool {
    let t = text.trim_start();
    !t.is_empty() && t.starts_with('{') && text.contains(marker)
}

// Flat parameters on purpose: the callers are per-command modules whose
// own signatures are flat, and building a context struct at each site
// would cost more lines than the block this replaces.
#[allow(clippy::too_many_arguments)]
/// Drop every line starting with one of `prefixes` and say how many went.
///
/// `indented` chooses where the prefix is looked for: after the leading
/// whitespace (`getfacl`, `lscpu`) or at column zero (`iostat`, `route`).
/// `unit` is the whole noun phrase, so each caller keeps its own wording —
/// "loop device(s)" is not "loop device line(s)".
///
/// This is deliberately NOT [`strip_noise`], which these four resemble: that
/// one always trims first, always says "… line(s)", and calls
/// `mark_generic_fold()`. Routing them through it would change one message,
/// two match positions and the generic-fold telemetry — three behaviour
/// changes for forty lines. Measured 2026-08-04, re-measured here.
pub fn elide_prefixed(text: &str, prefixes: &[&str], indented: bool, unit: &str) -> String {
    elide_where(
        text,
        |line| {
            let probe = if indented { line.trim_start() } else { line };
            prefixes.iter().any(|p| probe.starts_with(p))
        },
        unit,
    )
}

/// The same elision, decided by a PREDICATE rather than by a prefix.
///
/// A prefix is a word, and a word gets translated: `cmd/mod.rs` requires the
/// parsers to handle at least seven languages, and a `starts_with("Flags:")` only
/// handles one. MEASURED 2026-09-07, `lscpu` on an `fr_FR.UTF-8` machine:
/// **4,062 bytes folded to 4,061**, versus 3,921 → 1,442 (−63 %) on the same
/// machine in English — the `Drapeaux :` and `Vulnérabilité …` lines
/// start with neither of the two English prefixes.
///
/// The loop is the same, written once: `elide_prefixed` goes through it with
/// its prefix predicate.
pub fn elide_where(text: &str, mut drop_line: impl FnMut(&str) -> bool, unit: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut elided = 0usize;
    for line in text.lines() {
        if drop_line(line) {
            elided += 1;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if elided > 0 {
        let _ = writeln!(out, "  [elided: {elided} {unit}]");
    }
    out.trim_end().to_string()
}

// Flat parameters on purpose: the callers are per-command modules whose
// own signatures are flat, and building a context struct at each site
// would cost more lines than the block this replaces.
#[allow(clippy::too_many_arguments)]
/// Fold every run of `is_target` lines longer than `threshold`, keeping every
/// other line verbatim.
///
/// Falls back to `passthrough` when no single run was long enough: output that
/// is plentiful but scattered (files among directories, PIDs among headers) is
/// left intact rather than re-emitted identically.
///
/// Four command modules carried this loop verbatim. Their ENTRY GATE — how
/// many target lines make folding worth attempting at all — stays at the call
/// site, because it genuinely differs: `ls` folds from `>=` where the other
/// three fold from `>`. Everything after the gate was identical.
pub fn fold_runs(
    text: &str,
    lines: &[&str],
    is_target: fn(&str) -> bool,
    threshold: usize,
    keep_head: usize,
    keep_tail: usize,
    noun: &str,
) -> String {
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    let mut folded_total = 0usize;
    while i < lines.len() {
        if let Some((next, hidden)) = fold_run(
            &mut out, lines, i, is_target, threshold, keep_head, keep_tail, noun,
        ) {
            folded_total += hidden;
            i = next;
            continue;
        }
        out.push_str(lines[i]);
        out.push('\n');
        i += 1;
    }
    if folded_total == 0 {
        return passthrough(text);
    }
    out.trim_end().to_string()
}

// Flat parameters on purpose: the callers are per-command modules whose
// own signatures are flat, and building a context struct at each site
// would cost more lines than the block this replaces.
#[allow(clippy::too_many_arguments)]
/// Keep the first and last `keep_edge` lines verbatim and fold the middle into
/// a count.
///
/// For output whose ENDS carry the meaning — a sorted listing's first and last
/// keys, a table's header and its totals row. `None` when there is no real
/// interior to fold, or when the fold did not actually shrink the estimate.
///
/// `getent.rs` and `gh.rs` carried these twenty-four lines each and differed
/// only by their label and their noun. Keeping both edges is an archetype, not
/// a property of `getent`, so it belongs here beside `head_tail`.
pub fn fold_middle(text: &str, keep_edge: usize, label: &str, noun: &str) -> Option<String> {
    let trailing_nl = text.ends_with('\n');
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    // Need a real interior to fold (more than the two preserved edges).
    if total <= keep_edge * 2 {
        return None;
    }
    // The middle is not elided while the whole fits: `getent passwd` of 52
    // accounts, 3 KB, named 10 and counted 42 (2026-10-03).
    if tk_base::delivery::fits(text) {
        return None;
    }
    let folded = total - keep_edge * 2;

    let mut out = String::with_capacity(text.len() / 2);
    let _ = writeln!(
        out,
        "[tokenade:{label}] {total} {noun} · kept first/last {keep_edge} · folded {folded} middle {noun}"
    );
    for line in &lines[..keep_edge] {
        out.push_str(line);
        out.push('\n');
    }
    let _ = writeln!(out, "  … [tokenade:{label} folded {folded} {noun}] …");
    for line in &lines[total - keep_edge..] {
        out.push_str(line);
        out.push('\n');
    }
    if !trailing_nl {
        out.pop();
    }

    // Only a win if the token estimate actually shrank.
    if crate::tokenizer::estimate(&out) < crate::tokenizer::estimate(text) {
        Some(out)
    } else {
        None
    }
}

/// Keep the first `cap` non-blank lines and say how many were elided.
///
/// Six command modules carried this loop verbatim. The cap and the wording of
/// the elision line were the only differences, and the wording stays a
/// parameter on purpose: it is what tells the agent how to get the rest, and
/// "refine the pattern" is not advice an `lsmod` fold should give.
///
/// **Whole while it fits.** The six are lists where every line is an answer
/// (`lsmod`, `lspci`, `pip list`, `apropos`, `locate`, `sysctl`), and the cap
/// cut them at ANY size. Measured 2026-10-03 on this machine: `pip list` 100
/// packages in 3 KB delivered 30, `lsmod` 40 of 161 modules in 7.5 KB,
/// `lspci` 40 of 49, `apropos socket` 40 of 226 — "is X installed / loaded?"
/// answered for the head only. `sysctl -a` (100 KB) does not fit and keeps
/// the cut.
pub fn head_lines_with_tally(text: &str, cap: usize, elided: &str) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() <= cap || tk_base::delivery::fits(text) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() / 2);
    for line in lines.iter().take(cap) {
        out.push_str(line);
        out.push('\n');
    }
    let _ = writeln!(out, "  [... {} more {elided}]", lines.len() - cap);
    out.trim_end().to_string()
}

#[cfg(test)]
mod head_lines_with_tally_tests {
    use super::head_lines_with_tally;

    /// `pip list`: 100 packages in 3 KB came back as 30.
    #[test]
    fn a_list_that_fits_is_delivered_whole() {
        let _g = tk_base::delivery::threshold_on_this_thread(5_000);
        let t: String = (0..100)
            .map(|i| format!("package-{i}   1.{i}.0\n"))
            .collect();
        assert_eq!(head_lines_with_tally(&t, 30, "package(s) elided"), t);
    }

    #[test]
    fn a_list_past_the_threshold_keeps_its_head_and_tally() {
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        let t: String = (0..100)
            .map(|i| format!("package-{i}   1.{i}.0\n"))
            .collect();
        let out = head_lines_with_tally(&t, 30, "package(s) elided");
        assert!(out.ends_with("  [... 70 more package(s) elided]"), "{out}");
    }
}
