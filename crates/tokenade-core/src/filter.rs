//! Output filtering & compression engine.
//!
//! Models the proven rtk approach: detect the command that produced an output
//! and apply a command-specific compactor that keeps the *signal* (errors,
//! changed files, test failures, the tail) and drops the *noise* (progress
//! bars, ANSI, repeated lines, multi-megabyte success logs).
//!
//! Every filter is a pure `fn(&str) -> String` so they are trivially testable
//! and composable. A generic fallback handles unknown commands safely.

mod remote_argv;
// `note_folded` is no longer called by this module's production code — the
// counter and its two writers moved to `fold_count`/`elision` — but two of
// its tests still plant a known ceiling before asserting.
use crate::argv::{basename, find_subcommand, has_flag};
use crate::compress::{dedup_lines, generic, truncate_rescuing};
// `truncate_smart` is now only called by this module's tests: the production
// code that used it left with `generic` for `compress`.
use crate::fold_count::named_if_it_folded;
// This module's tests write with write!/writeln!; production code imports
// the trait at the point of use.

use crate::ansi::strip_ansi;
use crate::{auto, tokenizer, Savings};
// `post_passes` is no longer a sub-module of the dispatcher: its own header
// already said so — "none of them picks a route; all of them rework the
// TEXT". It moved one notch up the tree and one floor down the layers, to
// the FORMATS, next to the modules it names (`ssh_keys`, `pem`, `logs`).
// That is what lets `auto` fold a cursor redraw without climbing back up to
// the command dispatcher.
pub use crate::post_passes::*;
use remote_argv::*;

// Eight topics. The dispatcher itself stays here with its state; what decides
// BEFORE it (the intent), what catches AFTER it (the nets, the fallback) and
// what is not dispatching at all (`cat`, the wrappers) has moved out.
#[rustfmt::skip]
mod cat;
mod coverage;
mod dispatch;
mod fallback;
mod intent;
mod result;
mod safety;
mod wrapped;
pub use cat::*;
pub use coverage::*;
use dispatch::*;
pub use fallback::*;
pub use intent::*;
pub(crate) use remote_argv::is_silent_prefix_prog;
pub use result::*;
pub use safety::keeps_every_word_of;
use safety::*;
use wrapped::*;

pub fn filter_command(argv: &[String], output: &str) -> FilterResult {
    // Reserved argv used only by `a_panicking_compactor_costs_the_agent_nothing`
    // to exercise the fail-open boundary end-to-end. Compiled out of releases.
    #[cfg(test)]
    if argv.first().map(String::as_str) == Some("__tokenade_panic_probe__") {
        panic!("probe: a compactor blew up");
    }
    if let Some(r) = requested_json_whole(argv, output) {
        return r;
    }
    let mut r = filter_command_inner(argv, output);
    // **A numbered read keeps its numbers, not its padding.** Every
    // passthrough — the piped `nl -ba f | sed -n '1,260p'` Codex reads code
    // with, a source read that fits, a small ranged read — delivered
    // `     1\t…`, while a numbered read a fold had rewritten already came
    // back as `1\t…` (`line_gutter::renumber`). Not a cut, like the JSON
    // minify above: every line and every number stays. Measured 2026-10-03:
    // 3 512 → 3 215 tokens on a 260-line slice; `nl` was 7 900 fleet events
    // at 4 % coverage. Named `gutter`: a passthrough that changes the bytes
    // is not one.
    if r.filter == "passthrough" {
        if let Some(unpadded) = crate::line_gutter::unpadded(&r.output) {
            let after = tokenizer::estimate(&unpadded);
            return FilterResult {
                filter: "gutter".to_string(),
                output: unpadded,
                savings: Savings::new(r.savings.tokens_before, after),
            };
        }
    }
    // Gate on the user's EXPLICIT raw intent only (a flag / pipe / slice).
    // We deliberately do NOT skip the `"passthrough"` filter label: a no-op
    // compaction (e.g. a small `find` listing that the fold engine left
    // untouched because it's under HEAD) also carries that label, and those
    // are exactly the inputs the post-passes below target. `user_wants_raw`
    // is the one passthrough we must leave byte-for-byte.
    //
    // EXPLICIT intent only, and that is the same split the dispatch guard
    // makes: a verbose flag is a request for these bytes, a pipe is only an
    // argv we cannot route from. Gating the post-passes on the combined
    // predicate meant a pipeline ending in `head` skipped the template fold
    // that the same `head` run alone receives — measured on 500 identical
    // grep hits, 270 bytes unpiped against 13 193 piped.
    if raw_request_honoured(argv, r.savings.tokens_before) || head_tail_byte_precise(argv) {
        return r;
    }
    // **A source read is not traded for a sample.** The ranged-read arm keeps
    // a source file's text under `sed`/`head`/`tail` (and `cat`/`nl` under
    // theirs); when that text has little to strip, the name "folded nothing"
    // and the preference below handed it to `auto_or_generic` — `generic` is
    // smaller BECAUSE it skips lines at a fixed stride. Spotted in the
    // registry (`sed -n 40,420p detect.rs`, op `generic`) and reproduced: one
    // `///` line in two dropped across 66 lines, "in 67 runs of 1", with no
    // banner saying which.
    let source_read = matches!(
        r.filter.as_str(),
        "cat" | "nl" | "sed" | "head" | "tail" | "awk"
    ) && cat::cat_argv_has_source_extension(argv);
    if !source_read {
        r = prefer_the_fallback_when_the_name_folded_nothing(r, output);
    }
    // When a USER filter answered, the post-passes may shrink ITS answer — they
    // may not go back to the raw bytes. `fold_repetitive_runs` folds the RAW
    // input, and a fold of the raw can beat the filter's answer on size while
    // reinstating the very lines the filter was written to drop. MEASURED
    // 2026-09-04 on a filter that strips a runner's download and cache chatter:
    // on a FAILING run the delivered answer was the generic fold of the raw
    // output, 6,852 bytes with every `Downloading …` and `[cache] …` line back
    // in, against the filter's own 17,958 with none of them. The operator
    // installed an instruction, not a candidate.
    let fold_source = if crate::user_filters::match_for_command(&argv.join(" "))
        .is_some_and(|uf| uf.name == r.filter)
    {
        r.output.clone()
    } else {
        output.to_string()
    };
    // A listing whose rows are NAMES keeps them (see
    // `shrink_only_post_passes_keeping_rows`). DECLARED, not inferred: "the
    // compactor kept every row" was tried first and is not intent — `ps`
    // reformats and keeps 400 `--shard N` rows that the fold rightly collapses,
    // `node --test` keeps TAP receipts it never touched. `ls` is the measured
    // case; a listing joins this list on its own measurement.
    const ROWS_ARE_NAMES: &[&str] = &["ls"];
    let rows_kept = ROWS_ARE_NAMES.contains(&r.filter.as_str());
    if let Some(out) = crate::post_passes::shrink_only_post_passes_keeping_rows(
        argv,
        &fold_source,
        &r.output,
        rows_kept,
    ) {
        let after = tokenizer::estimate(&out);
        r.output = out;
        r.savings = Savings::new(r.savings.tokens_before, after);
    }
    r
}

/// **JSON the command was ASKED for is data, delivered whole while it fits.**
///
/// `--json` (or `-o json`, `--format json`) is a request for values to read
/// exactly: the JSON compactor's cuts — strings at 200 characters, arrays at
/// 20 items, audit keys like `updated_at` dropped — are guesses about what an
/// agent will skim, and here it said it would not. Measured 2026-10-03 in the
/// transcripts since 1.1.21: `acme-cli identity show --json` (4.3 KB) was
/// re-read through `expand-ref` after 3 of its 20 runs; replayed, two claims
/// came back as `…[+315 chars]` and `updated_at` was gone. Minified (whitespace
/// outside strings only, key order kept), never otherwise touched.
fn requested_json_whole(argv: &[String], output: &str) -> Option<FilterResult> {
    let asked = argv.iter().enumerate().any(|(i, a)| {
        let next_is_json = argv.get(i + 1).is_some_and(|n| n == "json");
        matches!(
            a.as_str(),
            "--json" | "--format=json" | "--output=json" | "-ojson" | "--json=true"
        ) || (matches!(a.as_str(), "-o" | "--format" | "--output") && next_is_json)
    });
    let body = output.trim();
    if !asked || !(body.starts_with('{') || body.starts_with('[')) {
        return None;
    }
    serde_json::from_str::<serde_json::Value>(body).ok()?;
    let mut min = String::with_capacity(body.len());
    let (mut in_str, mut escaped) = (false, false);
    for c in body.chars() {
        if in_str {
            min.push(c);
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => in_str = false,
                _ => {}
            }
        } else if !c.is_whitespace() {
            in_str = c == '"';
            min.push(c);
        }
    }
    if !tk_base::delivery::fits(&min) {
        return None;
    }
    let before = tokenizer::estimate(output);
    let after = tokenizer::estimate(&min);
    Some(FilterResult {
        filter: "json-requested".into(),
        output: min,
        savings: Savings::new(before, after),
    })
}

/// Resolved pre-dispatch state for `filter_command_inner`. Produced by
/// [`setup_filter_state`]: the `before` token count and the
/// program/subcommand/argv triple the dispatch match keys on. `sub` is
/// owned (rather than borrowing `argv`) so the struct is self-contained.
struct DispatchState {
    before: usize,
    prog: String,
    /// argv from the real program token onward (env-assignment prefix
    /// stripped) — what every compactor arm consumes as `argv`.
    argv: Vec<String>,
    /// Resolved subcommand (`find_subcommand` result), owned.
    sub: String,
}

/// Pre-dispatch setup: token estimate, generic-fold marker reset, the
/// user-filter / intent-preserving passthroughs (both early-returning a
/// `FilterResult`), and program/subcommand resolution.
///
/// Pure relocation of the head of `filter_command_inner`. Returns
/// `Err(result)` when an early passthrough fires (user filter matched or
/// `user_wants_raw`), otherwise `Ok(state)` to continue into dispatch.
/// The `reset_generic_fold` side effect happens here, before any
/// compactor runs — exactly as before.
fn setup_filter_state(argv: &[String], output: &str) -> Result<DispatchState, FilterResult> {
    let before = tokenizer::estimate(output);

    // Clear the per-call generic-fold marker before routing this output
    // through any compactor. `FilterResult::is_command_specific` reads it
    // afterwards to tell a real per-command compactor from a head/tail
    // stub. See `archetypes::reset_generic_fold`.
    crate::archetypes::reset_generic_fold();

    // User-defined compactors take precedence: a client's agent wrote this
    // filter FOR this command (via `tokenade add-compactor`), so honour it
    // before any built-in routing. Its `name` is command-specific by
    // construction, so it counts as real coverage on the dashboard. Fail-open:
    // a malformed filter file was already skipped at load time.
    let command_line = argv.join(" ");
    // ...but not when the command FAILED. A filter's whole-output summary is
    // written for the success case and erases the reason for a failure; the
    // built-in path below preserves it. Exit codes were never wrong here — the
    // reason was.
    if let Some(uf) = crate::user_filters::match_for_command(&command_line)
        .filter(|_| !crate::user_filters::command_failed())
    {
        let (out, sav) = uf.apply_with_savings(output);
        return Err(FilterResult {
            filter: uf.name.clone(),
            output: out,
            savings: sav,
        });
    }

    // **Several source files read in one command** (`for f in a.rs b.rs; do
    // echo "=== $f"; sed -n 1,200p $f; done`) are a read, as one file is: under
    // the read threshold they come back as written. Measured 2026-10-02: three
    // source files read in a loop came back from `generic` with 258 lines
    // folded "in 86 runs of 3" and the middle file's `===` banner gone.
    if crate::disclose::reads_only_source_files(&command_line)
        && before <= crate::disclose::read_threshold_tokens()
    {
        return Err(FilterResult {
            filter: "passthrough".to_string(),
            output: output.to_string(),
            savings: Savings::new(before, before),
        });
    }

    // SAFETY NET 0 (intent-preserving): when the user explicitly asked
    // for raw / sliced output via a flag or pipe, applying a compactor
    // can EAT the lines they wanted to see. Detect those cases up
    // front and passthrough.
    //
    // Triggers:
    //   - `--show-output`, `--nocapture` (cargo test wants println!())
    //   - `-v` / `--verbose` (user asked for detail)
    //   - `--message-format=short` (already a deliberate short format)
    //   - pipe into `tail`/`head` (user already sliced)
    //   - pipe into `wc`/`xargs`/`jq`/`awk`/`sed`/`tr` (output is
    //     transformed downstream — argv[0]'s compactor is wrong shape)
    //   - chain via `|`, `&&`, `||`, `;` with mismatched argv[0]
    //
    // The full command string is recovered from argv (joined) so we
    // can scan tokens that aren't argv-split (after a `|`).
    if raw_request_honoured(argv, before) || head_tail_byte_precise(argv) {
        crate::debug_log::log(
            "filter",
            "passthrough_intent_preserved",
            serde_json::json!({
                // The PROGRAM, not `argv[0]` — this logged the env-assignment
                // token (`TOKENADE_SESSION=…`) or a leading `cd` for two
                // thirds of its entries, which is why nobody could see what
                // was passing through here.
                "argv_first": argv.get(skip_shell_scaffolding(argv, 0)).map(|s| basename(s)).unwrap_or_default(),
                "before": before,
            }),
        );
        // Raw means the program's TEXT, not the escapes it sends a terminal.
        // GNU coreutils 9.8 wraps every option of `--help` in an OSC 8
        // hyperlink and bold: measured 2026-10-02, `ls --help` carried 120
        // links, 5 023 of its 12 316 bytes, delivered as is. A byte-precise
        // slice (`head -c`) keeps its bytes.
        let output = if head_tail_byte_precise(argv) {
            output.to_string()
        } else {
            strip_ansi(output)
        };
        let after = tokenizer::estimate(&output);
        return Err(FilterResult {
            filter: "passthrough".into(),
            output,
            savings: Savings::new(before, after),
        });
    }
    // A pipe: argv[0] does not name the output's shape, so ASK THE OUTPUT.
    // See [`argv_shape_is_unknowable`] — this is the same answer the
    // reshaping-command arm below already gives to the same question.
    let argv = intent::lister_stage_when_the_rest_only_orders(argv).unwrap_or(argv);
    if argv_shape_is_unknowable(argv) {
        let document = intent::pipeline_only_slices_a_source_file(argv);
        crate::debug_log::log(
            "filter",
            "piped_routed_by_content",
            serde_json::json!({
                "argv_first": argv.get(skip_shell_scaffolding(argv, 0)).map(|s| basename(s)).unwrap_or_default(),
                "before": before,
            }),
        );
        return Err(auto_or_generic_keeping(output, before, document));
    }

    // Locate the real program token. Skip leading env-assignment
    // tokens (`LANG=C cargo build` → real prog is `cargo`). Comment
    // tokens can't be reliably skipped here because the argv is
    // already line-flattened (`# foo\ngrep x` → ["#", "foo", "grep",
    // "x"] — we can't tell "foo" is comment text vs program name
    // without the newline info). Comment handling lives upstream in
    // cmd_wrap, where the raw cmd string with newlines is available.
    // C4 audit: the ROUTER must resolve `cd dir && real-cmd` (and a loop's
    // body) the same way program_basename does for labels — otherwise route
    // (generic via "cd" / "for") and label ("python") diverge on the public
    // filter_command path. Both now go through the one shared resolver.
    let real_prog_idx = skip_shell_scaffolding(argv, 0);
    let prog = argv
        .get(real_prog_idx)
        .map(|s| basename(s))
        .unwrap_or_default();
    // Find the subcommand. For most programs that's just argv[1],
    // but git/cargo/docker/kubectl accept pre-subcommand flags like
    // `-C <dir>`, `--git-dir=X`, `--no-pager`, `--config=K=V`. Without
    // skipping those we route `git -C path log` to the generic fallback
    // instead of the dedicated git-log compactor.
    let argv_after_prefix: Vec<String> = argv[real_prog_idx..].to_vec();
    let sub = find_subcommand(&prog, &argv_after_prefix).to_string();

    Ok(DispatchState {
        before,
        prog,
        argv: argv_after_prefix,
        sub,
    })
}

// Generated by build.rs from this file's `filter_command` match arms:
// `pub static FILTER_ARM_PROGS: &[&str]`.
include!(concat!(env!("OUT_DIR"), "/filter_arm_progs.rs"));

#[cfg(test)]
include!("filter_tests.rs");
