//! **What a filter RETURNS, and the ceiling it does not exceed.**
//!
//! A compactor that panics must not take the user's command down with it: the
//! failure path returns the original output. And above every compactor, a hard
//! ceiling — because an output nobody managed to fold is still an output that
//! must not be poured as is into a context.

use super::*;

/// Result of filtering one command's output.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FilterResult {
    /// Name of the filter that fired (e.g. `"git-status"`, `"generic"`).
    pub filter: String,
    /// The compacted output.
    pub output: String,
    pub savings: Savings,
}

/// True when the filter *name* denotes a compactor written for a specific
/// command's output format, as opposed to a generic fallback. This is the
/// pure, name-only half of the classification (the testable part).
///
/// Non-specific names: the universal fallbacks (`"generic"`,
/// `"passthrough"`), the form-aware auto compactor (`"auto:json"`,
/// `"auto:yaml"`, … — keyed off output *shape*, not the command), and the
/// explicit fallback marker suffix (`"…-fallback-generic"`). Everything
/// else is a per-command filter name (`"git-status"`, `"df"`, `"ls"`, …).
pub fn filter_name_is_command_specific(filter: &str) -> bool {
    !(filter.is_empty()
        || filter == "generic"
        || filter == "passthrough"
        || filter.starts_with("auto:")
        || filter.ends_with("-fallback-generic"))
}

impl FilterResult {
    /// Whether this result came from a compactor that genuinely parses
    /// *this command's* output format — Paul's definition: "did we run
    /// the command, look at the format, and write a compactor that
    /// optimises THIS command's output". Used to tell the central server
    /// which commands still lack a real compactor.
    ///
    /// Two conditions, both required:
    ///   1. the filter name denotes a dedicated compactor
    ///      ([`filter_name_is_command_specific`]), and
    ///   2. no command-agnostic fold helper from `archetypes`
    ///      produced the output (a head/tail/line-cap stub that fires on
    ///      arbitrary text carries a per-command filter name but is NOT a
    ///      real compactor — the runtime marker unmasks it).
    ///
    /// CONTRACT: condition 2 reads a per-call thread-local marker that
    /// `filter_command` resets on entry. Call this immediately after the
    /// `filter_command` that produced `self`, on the same thread, before
    /// any other `filter_command` call. Conservative: a structural
    /// compactor that *also* bounds its parsed output with a generic
    /// helper reads as non-specific (under-counts coverage, never over-).
    pub fn is_command_specific(&self) -> bool {
        filter_name_is_command_specific(&self.filter) && !crate::archetypes::generic_fold_used()
    }
}

/// Filter `output`, choosing a compactor from the `argv` that produced it.
///
/// `argv` is the full command, e.g. `["git", "status"]`. Only the program and
/// first subcommand are inspected, which is enough to route every filter.
///
/// This is a thin wrapper: it runs `filter_command_inner` (the real router)
/// and then applies two generic, lossless post-passes to ANY compacted output
/// (skipped on passthrough — the user explicitly wanted raw bytes there):
///   1. `relativize_cwd_paths` — strips the workspace prefix that tools like
///      `python -m unittest`, `jest`, `go test`, linters and stack traces
///      print on every line (e.g. `/tmp/run/ws/tests/t.py` → `tests/t.py`).
///   2. `fold_separator_rules` — collapses full-width separator rules
///      (`====`, `----`, `____`, …) to 3 chars, keeping the visual boundary
///      while dropping the bulk.
/// Both only ever shrink and are recoverable verbatim via `expand_ref`.
/// Measured on a 14-failure `unittest` block: ~48% of the (already-compacted)
/// bytes were separator rules + absolute-path prefixes.
/// [`filter_command`], but a panic inside any compactor returns the output
/// UNCHANGED instead of taking the process down.
///
/// tokenade's contract with the user is that wrapping a command never costs
/// them the command: "it never breaks exit codes or stderr-on-failure". The
/// compaction path is 3 352 built-in specs plus hundreds of hand-written
/// modules, nearly all of them doing index arithmetic on text that arrives from
/// somewhere else — the single largest surface in the product, and the one
/// place a `&str[a..b]` on a multi-byte boundary ends the run. There was no
/// boundary here: `wrap` called `filter_command` bare, so a panic anywhere in
/// that surface lost the WHOLE output and replaced the command's exit code with
/// 101.
///
/// The same reasoning already applies one layer down — `docs::extract` and the
/// PDF layout pass are wrapped for exactly this, because "real-world PDFs
/// trigger edge cases". Real-world command output is a strictly larger and less
/// structured input than a PDF.
///
/// A 5 888-combination adversarial sweep over 45 programs (multi-byte runs at
/// every slice offset, 5 000-char single lines, NUL bytes, CRLF, empty) found
/// **no** panicking input — see `no_seed_input_panics_any_compactor`. This is
/// therefore insurance, not a bug fix: it costs nothing on the path that works.
///
/// SCOPE, measured the day after this was written: the release profile is
/// `panic = "abort"`, so this `catch_unwind` **cannot fire in a shipped
/// binary** — it only bites in `unwind` builds (dev, tests). The guarantee
/// `wrap` actually relies on in production is
/// [`crate::crash_guard::arm_salvage`], which hands the output back from the
/// fatal-signal handler and covers native faults too. Both are kept: this one
/// degrades gracefully wherever unwinding exists, that one is the backstop that
/// always runs. See decision #40 for the `abort`/`unwind` trade.
pub fn filter_command_fail_open(argv: &[String], output: &str) -> FilterResult {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // Debug-only fault injection, so the boundary can be proved against the
        // real binary. Compiled out of releases.
        #[cfg(debug_assertions)]
        if std::env::var("TOKENADE_CRASH_PROBE").as_deref() == Ok("panic") {
            panic!("probe: a compactor blew up");
        }
        filter_command(argv, output)
    })) {
        Ok(r) => r,
        Err(_) => {
            crate::debug_log::log(
                "filter",
                "compactor_panicked_output_preserved",
                serde_json::json!({
                    "prog": argv.first().map(String::as_str).unwrap_or(""),
                    "bytes": output.len(),
                }),
            );
            FilterResult {
                filter: "passthrough".to_string(),
                output: output.to_string(),
                savings: Savings::new(
                    crate::tokenizer::estimate(output),
                    crate::tokenizer::estimate(output),
                ),
            }
        }
    }
}

/// The ceiling every compaction path shares, in tokens.
///
/// Deliberately large: 50 k is roughly 5–8 % of a fresh context window, big
/// enough to convey real structure and small enough to be survivable.
///
/// It was declared FOUR times with the same value — twice here, once in
/// `web_pipeline`, once in the CLI's read interceptor — each followed by the
/// same four lines. Codex centralises exactly this policy in one module
/// (`codex-rs/utils/string/src/truncate.rs`); so does this now.
pub const HARD_CAP_TOKENS: usize = 50_000;

/// Head/tail truncation layered ON TOP of a structured compaction when the
/// result is still huge (a multi-MB log with thousands of distinct
/// templates, a `find /` listing of 500 k tokens). [`generic`] keeps both
/// ends, states the elided count, and names the recovery path.
///
/// `None` when the cap does not bite, or when truncating would not actually
/// shrink the output — a compactor that already folded well must not be
/// undone by the net beneath it.
///
/// Logging lives HERE rather than at a call site: `hard_cap_applied` existed
/// but only one of the four copies emitted it, so a cap applied on the web
/// pipeline or the read interceptor left no trace at all.
pub fn apply_hard_cap(compacted: &str, after: usize, compactor: &str) -> Option<(String, usize)> {
    apply_hard_cap_rescuing(
        compacted,
        after,
        compactor,
        HARD_CAP_LINES,
        &crate::compress::is_error_line,
    )
}

/// The hard cap's window, in LINES. The cap is decided in tokens; this
/// number is the size of the view drawn from it.
pub const HARD_CAP_LINES: usize = 180;

/// [`apply_hard_cap`], naming what deserves to be rescued from the middle.
///
/// **A page has no errors, it has headings.** MEASURED 2026-09-10 on
/// `pages/zhwiki`: fixed-stride sampling returns **6 headings out of 53**, so
/// a fifty-section article arrives without an outline, and it now arrives
/// with a pointer to a reserve that nothing helps to query. The skeleton of
/// that page — the 53 headings plus the 38 leading paragraphs — is worth
/// **≈ 7 100 characters**, 14% of the cap: keeping it costs almost nothing
/// and turns the delivered page into an index of the reserve.
pub fn apply_hard_cap_rescuing(
    compacted: &str,
    after: usize,
    compactor: &str,
    lines_budget: usize,
    worth_rescuing: &dyn Fn(&str) -> bool,
) -> Option<(String, usize)> {
    if after <= HARD_CAP_TOKENS {
        return None;
    }
    let truncated = truncate_rescuing(
        &dedup_lines(&strip_ansi(compacted)),
        lines_budget,
        worth_rescuing,
    );
    let trunc_after = tokenizer::estimate(&truncated);
    if trunc_after >= after {
        return None;
    }
    crate::debug_log::log(
        "filter",
        "hard_cap_applied",
        serde_json::json!({
            "compactor": compactor,
            "after_compactor": after,
            "after_cap": trunc_after,
        }),
    );
    Some((truncated, trunc_after))
}
