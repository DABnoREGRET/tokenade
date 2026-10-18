//! Data-driven built-in command compactors — Phase A of collapsing the
//! per-command stub modules (`cmd/<os>/<name>.rs`) into a single hardcoded
//! table.
//!
//! Each former stub whose body was a pure archetype ladder (the vast majority)
//! becomes one row in [`crate::cmd::spec_table::BUILTIN_SPECS`]. [`apply`]
//! reproduces that ladder by calling the SAME archetype functions the stub
//! called, with the SAME arguments — so its output is byte-identical to the
//! original module. That equivalence is proven exhaustively in
//! `tests::every_row_with_a_live_stub_reproduces_it` (every row whose stub file
//! still exists, vs that stub via `dispatch_auto`). That is 54 of the 3292 rows:
//! the Phase-C deletion took the other 3238 stubs, and with them any way to
//! check those rows. This doc used to name `builtin_spec_parity_with_stubs` and
//! call the equivalence "proven exhaustively" — that test never existed. Stubs with a bespoke parser (aws_json, …) keep their own
//! module and are never represented here.
//!
//! Anti-reverse: the label/unit/strip strings live in normal `&'static str`s;
//! the binary's existing litcrypt protection already covers sensitive literals,
//! so no separate encoding is needed (decision: Paul 2026-06-30).

use crate::archetypes;
use crate::table_fold as docker_table;
use crate::tabular;
use crate::tokenizer;
use std::collections::HashMap;
use std::sync::OnceLock;

/// The archetype ladder a built-in compactor follows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    /// `looks_like_table` → fold; else if > `head` non-empty lines → head/tail;
    /// else `None`. (Detector gates on table-or-long.)
    TableHeadTail,
    /// table → fold (fall through on `None`); kv → `kv_records` (fall through);
    /// else `head_tail`. (Detector is just non-empty.)
    TableKvHeadTail,
    /// `strip_noise(prefixes, noun)` — drop lines starting with any prefix.
    Strip,
    /// A JavaScript stack (`node app.js` that crashed): runs of `at …` frames
    /// keep the program's, count the runtime's. A frame's location is at the
    /// END of its line (`at Module._compile (node:internal/…)`), which no
    /// prefix list can see — the `node` row stripped `    at node:internal`
    /// and matched none of the seven frames of a real crash.
    JsStack,
    /// `fold_table` alone: fold a table, and fall through (`None`) on anything
    /// else. No head/tail rescue — the stubs this replaces had none, and
    /// adding one would change their output.
    TableFold,
    /// `fold_paths` alone: a path listing folded into a directory census.
    PathFold,
    /// `PathFold`, except that a list which fits under the delivery threshold
    /// is given whole (grouped): for `grep -l` the paths ARE the answer.
    PathList,
    /// `fold_table_priority` alone: fold a table keeping the rows that stand
    /// out, and fall through on anything that is not one.
    TablePriorityFold,
    /// `log_tail::tail(text, 5, 40, label)` — a streaming log: a little head
    /// context, then the tail, where the outcome is. The two counts are the
    /// same in every stub this replaces, so they stay literal here rather than
    /// widening `Spec` with two fields nothing else would use.
    LogTail,
    /// `git_listing::fold` — a flat listing folded to `head` entries.
    ListFold,
}

/// The stub's `detect` gate — the predicate that decided whether to compact at
/// all (returning `None`/pass-through when false). Decoupled from [`Shape`] (the
/// compaction ladder) so a generic ladder with a content-specific gate can be
/// data instead of code.
#[derive(Clone, Copy)]
pub enum Detect {
    /// Any non-blank input.
    NonEmpty,
    /// `looks_like_table` OR more than `head` non-empty lines.
    TableOrLong,
    /// `text` contains at least one of these substrings.
    ContainsAny(&'static [&'static str]),
    /// `text`, left-trimmed, starts with one of these prefixes.
    StartsWithAny(&'static [&'static str]),
    /// Some line, trimmed, starts with one of these prefixes.
    LineStartsWithAny(&'static [&'static str]),
    /// Some line, trimmed, is longer than N characters. The gate a stub used
    /// to mean "there is real content here", as opposed to blank padding.
    LineLongerThan(usize),
    /// Disjunction: any of these sub-detects fires (mirrors a `a || b || c`
    /// detect mixing the atoms above).
    Any(&'static [Detect]),
    /// Conjunction: every sub-detect fires (mirrors `a && b`).
    All(&'static [Detect]),
}

/// Whether a single detect atom fires on `text` (`head` only used by
/// [`Detect::TableOrLong`]).
fn detect_fires(d: &Detect, text: &str, head: usize) -> bool {
    match d {
        Detect::NonEmpty => true,
        Detect::TableOrLong => {
            archetypes::looks_like_table(text)
                || text.lines().filter(|l| !l.trim().is_empty()).count() > head
        }
        Detect::ContainsAny(xs) => xs.iter().any(|x| text.contains(x)),
        Detect::StartsWithAny(ps) => {
            let t = text.trim_start();
            ps.iter().any(|p| t.starts_with(p))
        }
        Detect::LineLongerThan(n) => text.lines().any(|l| l.trim().len() > *n),
        Detect::LineStartsWithAny(ps) => text.lines().any(|l| {
            let t = l.trim();
            ps.iter().any(|p| t.starts_with(p))
        }),
        Detect::Any(ds) => ds.iter().any(|d| detect_fires(d, text, head)),
        Detect::All(ds) => ds.iter().all(|d| detect_fires(d, text, head)),
    }
}

/// One built-in command compactor as data.
pub struct Spec {
    /// Dispatch key = suffix-stripped filename stem (`build.rs::dispatch_key_of`).
    pub key: &'static str,
    pub detect: Detect,
    pub shape: Shape,
    pub head: usize,
    pub label: &'static str,
    pub unit: &'static str,
    /// Noise-line prefixes for [`Shape::Strip`]; empty otherwise.
    pub strip: &'static [&'static str],
}

/// Whether the stub's `detect` gate would have fired for this output.
fn passes(spec: &Spec, text: &str) -> bool {
    !text.trim().is_empty() && detect_fires(&spec.detect, text, spec.head)
}

/// Reproduce the stub's `detect` gate + ladder. Byte-identical to the original
/// module by construction (same archetype functions, same arguments).
pub fn apply(spec: &Spec, text: &str) -> Option<String> {
    if !passes(spec, text) {
        return None;
    }
    // **Listings whose rows ARE the answer**: a column table that fits under
    // the delivery threshold is given whole, its padding collapsed, whatever
    // the shape would have cut. DECLARED per command, on its own measurement
    // (real Windows 10, 2026-10-02): `driverquery` kept 50 drivers of 371,
    // `wmic process list brief` 45 processes of 80 with no pointer to the
    // rest — both fit whole.
    const ROWS_ARE_THE_ANSWER: &[&str] = &["driverquery", "wmic"];
    if ROWS_ARE_THE_ANSWER.contains(&spec.key) {
        if let Some(whole) = tabular::whole_column_table(text) {
            return Some(whole);
        }
    }
    match spec.shape {
        Shape::TableHeadTail => {
            if archetypes::looks_like_table(text) {
                docker_table::fold_table_priority(text, spec.head, spec.label, spec.unit)
            } else {
                archetypes::head_tail(text, spec.head, 5, spec.label, spec.unit)
            }
        }
        Shape::TableKvHeadTail => table_kv_head_tail(spec, text),
        Shape::Strip => {
            let stripped = archetypes::strip_noise(text, spec.strip, spec.unit);
            // A strip is a NOISE FILTER, not a size compactor: it drops the
            // prefixes named in the spec and returns whatever is left. When
            // what is left is still enormous, claiming the output is worse
            // than declining, because claiming BLOCKS the generic fold —
            // which is the only thing that would have compacted it.
            //
            // Measured 2026-09-02 on `readelf -a /usr/bin/git`: the spec
            // strips two section-header prefixes, 161 038 tokens become
            // 132 053 (18%), and the generic fold on the same bytes returns
            // **5 483** (96.6%). The safety net did not catch it because it
            // compares against doing NOTHING, never against the fallback.
            // Across a thirty-command sweep of real output this is the only
            // route where the fold wins by more than 2.2x.
            //
            // The bar is the disclosure threshold rather than a number
            // invented here: above it the product already considers an
            // output too big to hand over whole.
            if tokenizer::estimate(&stripped) > crate::disclose::threshold_tokens() {
                return None;
            }
            Some(stripped)
        }
        Shape::JsStack => {
            let (folded, dropped) = crate::stacktrace::fold_js_frame_runs(text);
            (dropped > 0).then_some(folded)
        }
        Shape::TableFold => docker_table::fold_table(text, spec.head, spec.label, spec.unit),
        Shape::TablePriorityFold => {
            docker_table::fold_table_priority(text, spec.head, spec.label, spec.unit)
        }
        Shape::LogTail => super::linux::log_tail::tail(text, 5, 40, spec.label),
        Shape::ListFold => super::linux::git_listing::fold(text, spec.head, spec.label, spec.unit),
        Shape::PathFold => {
            super::linux::git_listing::fold_paths(text, spec.head, spec.label, spec.unit)
        }
        Shape::PathList => {
            super::linux::git_listing::list_or_fold_paths(text, spec.head, spec.label, spec.unit)
        }
    }
}

/// `TableKvHeadTail`: table → fold (fall through on `None`); kv → `kv_records`
/// (fall through); else `head_tail`.
fn table_kv_head_tail(spec: &Spec, text: &str) -> Option<String> {
    if archetypes::looks_like_table(text) {
        if let Some(o) = docker_table::fold_table_priority(text, spec.head, spec.label, spec.unit) {
            return Some(o);
        }
    }
    if archetypes::looks_like_kv(text) {
        if let Some(o) = archetypes::kv_records(text, spec.head, spec.label, spec.unit) {
            return Some(o);
        }
    }
    archetypes::head_tail(text, spec.head, 5, spec.label, spec.unit)
}

fn spec_map() -> &'static HashMap<&'static str, &'static Spec> {
    static M: OnceLock<HashMap<&'static str, &'static Spec>> = OnceLock::new();
    M.get_or_init(|| {
        crate::cmd::spec_table::BUILTIN_SPECS
            .iter()
            .chain(crate::cmd::spec_table_folds::FOLD_SPECS.iter())
            .map(|s| (s.key, s))
            .collect()
    })
}

/// Look up a built-in spec by dispatch key, then apply it. `None` when no spec
/// owns this key (caller falls through to the remaining stub modules).
///
/// Mirrors `build.rs`'s hyphen alias: the table is keyed by the underscore
/// filename stem (`apt_get`, `get_childitem`), but commands/cmdlets arrive
/// hyphenated (`apt-get`, lower-cased `get-childitem`), so a direct miss
/// retries with `-` → `_`.
pub fn dispatch(prog: &str, text: &str) -> Option<String> {
    let map = spec_map();
    if let Some(s) = map.get(prog) {
        return apply(s, text);
    }
    if prog.contains('-') {
        if let Some(s) = map.get(prog.replace('-', "_").as_str()) {
            return apply(s, text);
        }
    }
    None
}

/// Whether a built-in spec owns this key (same hyphen aliasing as [`dispatch`]).
/// Mirrors `dispatch_auto_has_prog` for the migrated commands, so
/// `dispatch_compact` still treats them as known programs worth a pass-through.
pub fn has_key(prog: &str) -> bool {
    let map = spec_map();
    map.contains_key(prog)
        || (prog.contains('-') && map.contains_key(prog.replace('-', "_").as_str()))
}
#[cfg(test)]
include!("spec_tests.rs");
