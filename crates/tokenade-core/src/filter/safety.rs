//! **The safety nets: what to do when the named compactor got it wrong.**
//!
//! A specific compactor that folds NOTHING is worse than the generic fallback,
//! and a compactor that folds too much is worse than both. These nets apply
//! AFTER dispatching, on what it returned, and they are the reason a faulty
//! arm degrades the output instead of destroying it.

use super::*;

/// The four independent safety-net guards + final `FilterResult`
/// assembly, in the SAME order as before: (1) empty-output fallback,
/// (2) inflation→generic fallback, (3) hard inflation cap passthrough,
/// (4) `HARD_CAP_TOKENS` truncation. Pure relocation of the tail of
/// `filter_command_inner`. `name`/`compacted` are the dispatch result;
/// `argv` is the post-prefix argv the original logged.
pub(super) fn apply_safety_nets(
    compacted: String,
    name: &str,
    argv: &[String],
    output: &str,
    before: usize,
) -> FilterResult {
    let after = tokenizer::estimate(&compacted);
    // SAFETY NET 1: a non-empty input that compacts to whitespace-only
    // (observed 2026-05-29 with `git_log` on `--oneline` — the parser
    // expected `commit <hash>` headers that don't exist in that
    // format) almost always means the compactor's pattern didn't
    // match. Returning empty would silently destroy the user's
    // output. Fall back to generic instead.
    //
    // We log the trigger to `~/.tokenade/debug.log` so the team can
    // grep for `event:"safety_net_empty"` and find every compactor
    // that needs a fallback path of its own.
    if compacted.trim().is_empty() && !output.trim().is_empty() {
        let cleaned = generic(output);
        let after2 = tokenizer::estimate(&cleaned);
        crate::debug_log::log(
            "filter",
            "safety_net_empty",
            serde_json::json!({
                "compactor": name,
                "argv_first": argv.first().cloned().unwrap_or_default(),
                "argv_sub": argv.get(1).cloned().unwrap_or_default(),
                "input_bytes": output.len(),
                "input_lines": output.lines().count(),
                "input_head": output
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(120)
                    .collect::<String>(),
                "fallback_after": after2,
            }),
        );
        return FilterResult {
            filter: format!("{name}-fallback-generic"),
            output: cleaned,
            savings: Savings::new(before, after2),
        };
    }
    // SAFETY NET 2: filter made things worse (rare — usually a banner
    // overhead on a tiny input). Fall back to cleaned original. Also
    // logged so we can see which compactor's overhead is hurting on
    // which input shape.
    if after >= before && name != "generic" {
        let cleaned = generic(output);
        let after2 = tokenizer::estimate(&cleaned);
        if after2 < before {
            crate::debug_log::log(
                "filter",
                "safety_net_inflated",
                serde_json::json!({
                    "compactor": name,
                    "argv_first": argv.first().cloned().unwrap_or_default(),
                    "argv_sub": argv.get(1).cloned().unwrap_or_default(),
                    "before": before,
                    "after_compactor": after,
                    "fallback_after": after2,
                }),
            );
            return FilterResult {
                filter: "generic".into(),
                output: cleaned,
                savings: Savings::new(before, after2),
            };
        }
    }
    // SAFETY NET 3 (L35, tightened post-L122-audit). When the picked
    // compactor came back >= before, it's a net loss. Return the
    // ORIGINAL bytes with savings=0 so we never ship a regression.
    // Tokenade should never inflate input — at worst, leave it
    // unchanged.
    //
    // Earlier version had a `before >= 100` skip with the rationale
    // "compactors do synthesis on tiny inputs". The L122 audit found
    // 87 inflation events on `cargo-build … | tail -3` (small input,
    // banner 7-8 toks inflation). The supposed synthesis was never
    // worth the inflation. Hard cap: if after > before, passthrough,
    // period. Compactors that want to add a summary line have to
    // compress something to offset.
    if after >= before {
        // Split the event by reality: strict inflation (after > before)
        // vs no-change (after == before). The old single `inflated_both`
        // label conflated the two — a future analyst filtering on
        // "inflated" would have caught no-op compactors (e.g. `free`
        // through sysinfo: 243→243) alongside genuine inflations.
        let event = if after > before {
            "passthrough_inflated"
        } else {
            "passthrough_no_change"
        };
        crate::debug_log::log(
            "filter",
            event,
            serde_json::json!({
                "compactor": name,
                "argv_first": argv.first().cloned().unwrap_or_default(),
                "before": before,
                "after_compactor": after,
            }),
        );
        return FilterResult {
            filter: "passthrough".into(),
            output: output.to_string(),
            savings: Savings::new(before, before),
        };
    }
    // SAFETY NET 4 (L46 — Bash-path hard cap, mirrors L45 on Read).
    // Even after a format-specific compactor, the output can still
    // be massive (10 MB log file produces 1 M tokens; a pathological
    // `find /` listing produces 500 k tokens). Layer head/tail
    // truncation on top whenever the compacted output exceeds
    // HARD_CAP_TOKENS. The agent already has a §ref§ marker path
    // via `tokenade wrap`'s dedup, and the truncate_middle banner
    // is explicit about the elision.
    if let Some((truncated, trunc_after)) = apply_hard_cap(&compacted, after, name) {
        return FilterResult {
            filter: format!("{name}+cap"),
            output: truncated,
            savings: Savings::new(before, trunc_after),
        };
    }
    FilterResult {
        filter: name.into(),
        output: compacted,
        savings: Savings::new(before, after),
    }
}

/// Thin orchestrator: resolve pre-dispatch state, route by command, then
/// run the safety-net guards. Behaviour is identical to the pre-refactor
/// inline version — each phase early-returns a fully-built `FilterResult`
/// exactly where it did before. See [`setup_filter_state`],
/// [`dispatch_by_command`] and [`apply_safety_nets`].
/// A named compactor CLAIMED the output. If it folded nothing of it, its
/// claim cost the fallback ladder — a success that saves nothing blocks
/// exactly what a REFUSAL would have let through, whereas the refusal is
/// already handled (see `folded_or_fallthrough`).
///
/// MEASURED on the corpus's 554 148 crossings by
/// `tokenade-bench --bin compactor_review`: twenty real pairs in this case,
/// across the three platforms. `file` returned **43 663 bytes out of 43 664**
/// where the language-less fallback returns 31 770; the four PowerShell
/// tables, `whoami /all`, `diskutil`, `defaults`, `netstat`, `mount` and
/// `lsusb` are in the same case.
///
/// Placed on the FINAL result, and it is the only place that works: in this
/// module `Err` does not mean "failure" but "final answer, skip the nets", so
/// the rule placed on the `Ok` branch of `dispatch_by_command` was never
/// reached by `file` — measured at zero effect on the twenty cases, twice,
/// before the trace said so.
///
/// The fallback is computed ONLY in this case, so the extra pass is only paid
/// where the first one gave nothing, and it is taken only if it does STRICTLY
/// better: a compactor that returns the output unchanged because it was
/// already minimal keeps the hand, the fallback having nothing more to offer.
pub(super) fn prefer_the_fallback_when_the_name_folded_nothing(
    r: FilterResult,
    output: &str,
) -> FilterResult {
    // COST guard, and it cannot be killed by an output test: without it the
    // answer is identical — a result already produced by `auto_or_generic`
    // cannot be beaten by `auto_or_generic`. What it avoids is the second full
    // pass. MEASURED on the corpus: **112 of the 378 command fixtures**, i.e.
    // 30%, would redo it for nothing.
    if !filter_name_is_command_specific(&r.filter) {
        return r;
    }
    if r.output.len() * 100 < output.len() * 98 {
        return r;
    }
    let before = tokenizer::estimate(output);
    let fallback = auto_or_generic(output, before);
    if fallback.output.len() < r.output.len() && keeps_every_word_of(&fallback.output, &r.output) {
        fallback
    } else {
        r
    }
}

/// Shorter is not better when it is shorter by LOSING what the named
/// compactor kept. Measured 2026-10-03, twice: `git show --stat` traded for
/// a `git log` summary without the changed paths, `launchctl list` for a
/// 30-row head without 22 jobs. Over the corpus the six legitimate swaps
/// (`aligned-block` realigning a table) drop no word at all — that is the
/// line: every word of four characters or more that `kept` carries must
/// still be in `fallback`.
pub fn keeps_every_word_of(fallback: &str, kept: &str) -> bool {
    let have = words_of(fallback);
    words_of(kept).iter().all(|w| have.contains(w))
}

fn words_of(t: &str) -> std::collections::HashSet<&str> {
    t.split(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '.' | '/' | '-')))
        .filter(|w| w.chars().count() >= 4)
        .collect()
}

pub(super) fn filter_command_inner(argv: &[String], output: &str) -> FilterResult {
    let state = match setup_filter_state(argv, output) {
        Ok(s) => s,
        Err(result) => return result,
    };
    let (name, compacted) =
        match dispatch_by_command(&state.prog, &state.sub, &state.argv, output, state.before) {
            Ok(pair) => pair,
            Err(result) => return result,
        };
    apply_safety_nets(compacted, name, &state.argv, output, state.before)
}
