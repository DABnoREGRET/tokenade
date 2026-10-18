//! `npm run <script>` — and the whole family that funnels here:
//! `npm run-script`, `npm run build|start|lint|format`, `npm start`,
//! `pnpm run`, `yarn run`, `bun run`, plus the dispatcher's catch-all
//! fallback (`crate::filter::npm_run`).
//!
//! `npm run` executes an **arbitrary user script**, so the body is whatever the
//! underlying tool prints (webpack, vite, rollup, tsc, esbuild, eslint, a dev
//! server, a shell one-liner, …). There is no command-specific schema to parse
//! — but the *noise* is universal and **structural**, independent of both the
//! tool and the locale:
//!   * ANSI colour / cursor escape sequences — every modern build tool colours
//!     its output;
//!   * carriage-return progress redraws (`10%\r20%\r…\r100%` from webpack's
//!     ProgressPlugin, vite, rollup, ora/listr spinners) — only the final state
//!     of each redrawn line carries information;
//!   * long runs of identical consecutive lines (a warning repeated once per
//!     module, per-file "compiled" chatter, repeated dev-server pings).
//!
//! Before this was a real compactor the live path was a bare `passthrough`, so
//! a multi-thousand-line coloured build log reached the agent verbatim.
//!
//! The real compactor:
//!   * strips ANSI escapes and collapses each `\r`-redrawn line to its final
//!     state (via the shared `strip_ansi`), announcing the redraw count;
//!   * folds runs of identical consecutive lines into `<line>  (×N)` — never a
//!     silent drop, the multiplier IS the announcement, and every *distinct*
//!     line (the npm `> pkg@ver script` header, asset names, file paths,
//!     errors/warnings) survives verbatim;
//!   * leads with a banner carrying the **real** original line count plus the
//!     duplicate- and progress-fold counts.
//!
//! Detection and folding are PURELY structural — escape bytes, `\r`, and
//! byte-equality of whole lines, with **no keyword matching whatsoever**. A
//! localized build log (German webpack, French eslint) therefore compacts
//! identically and keeps every localized line verbatim. The compactor declines
//! (→ passthrough) when stripping + folding does not actually shrink the
//! output, so small/clean script output is returned unchanged.

pub fn detect(text: &str) -> bool {
    crate::archetypes::any_output(text)
}

/// A `run` script is the ONLY way anyone reaches the underlying build tool:
/// `pnpm build` runs `next build`, and the argv tokenade sees says `pnpm`. So
/// look at the OUTPUT for a shape whose own compactor could never be reached
/// by name. Content-detected, never name-detected — a script whose output is
/// not a route table falls straight through.
fn by_content(text: &str) -> Option<String> {
    // A Vite build's asset table is the other shape a `run build` prints
    // whole: `npm run build` → `slidev build` delivered its 60 asset rows
    // untouched (2026-10-05). The shorter of the readers that recognise it.
    // A test script over `node --test` / `tsx --test` prints TAP: an 81-test
    // `pnpm test:affiliate` (2026-10-05) went to the stash as head + tail.
    let tap = super::test_runner_dispatcher::tap::detect_by_content(text)
        .then(|| super::test_runner_dispatcher::tap::compact(text))
        .flatten();
    [super::next_route_table::try_compact(text), super::vite_assets::try_compact(text), tap]
        .into_iter()
        .flatten()
        .min_by_key(String::len)
}

/// Whichever of the two costs the model less.
///
/// The content-detected compactor COMPETES with the name-detected one; it does
/// not preempt it. Preempting looked right — "the more specific reader wins" —
/// and measured badly: on a real `pnpm build` log the route-table reader fires,
/// folds the route table it recognises, leaves the other 90% of the log alone
/// and returns 11.9% where `npm`'s structural fold returns 30%+
/// (`tests/build_log_corpus.rs`). "It shrinks the text" is not enough; it has
/// to shrink it more than what it displaces.
fn cheaper(a: String, b: String) -> String {
    if crate::tokenizer::estimate(&a) <= crate::tokenizer::estimate(&b) {
        a
    } else {
        b
    }
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    match (by_content(text), super::npm::try_compact(text)) {
        (Some(c), Some(n)) => Some(cheaper(c, n)),
        (c, n) => c.or(n),
    }
}

pub fn compact(text: &str) -> String {
    // `pnpm run test` reaches here, not `npm_test`: a green vitest run keeps
    // its tally rather than the app logs the suite printed.
    if let Some(green) = super::vitest_pass::try_compact(text) {
        return green;
    }
    let named = super::npm::compact(text);
    match by_content(text) {
        Some(c) => cheaper(c, named),
        None => named,
    }
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    /// A realistic coloured webpack-style build log: an npm header, an ANSI
    /// colour wash on every line, a long `\r`-redrawn ProgressPlugin line, a
    /// warning repeated once per module, and the asset/summary tail.
    fn heavy_build_log(modules: usize) -> String {
        let mut s = String::new();
        // npm preamble.
        s.push_str("\n> myapp@1.4.2 build\n> webpack --mode production\n\n");
        // One giant `\r`-redrawn progress line (webpack ProgressPlugin).
        s.push_str("\x1b[2K");
        for pct in (0..=100).step_by(2) {
            let _ = write!(s, "\r{pct}% building {pct}/100 modules");
        }
        s.push('\n');
        // The same deprecation warning emitted once per compiled module
        // (coloured yellow) — a long run of identical consecutive lines.
        for _ in 0..modules {
            s.push_str("\x1b[33mWARNING: 'sass' loader option is deprecated\x1b[0m\n");
        }
        // Distinct, load-bearing tail — asset list + summary.
        s.push_str("\x1b[32masset main.js 1.2 MiB [emitted] [minimized] (name: main)\x1b[0m\n");
        s.push_str("\x1b[32masset vendor.js 845 KiB [emitted] [minimized]\x1b[0m\n");
        s.push_str("webpack 5.88.0 compiled with 1 warning in 12345 ms\n");
        s
    }

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("   \n  \n"));
    }

    // 1. GAIN — the coloured / progress-redrawn / duplicate-heavy build log
    // folds to a fraction of its size.
    #[test]
    fn savings_on_heavy_build_log() {
        let (out, sav) = compact_with_savings(&heavy_build_log(120));
        assert!(
            sav.ratio() >= 0.50,
            "expected >=50% savings, got {:.1}% ({out})",
            sav.percent()
        );
    }

    // 2. QUALITY — banner carries the REAL line count, named entities survive
    // verbatim, the duplicate run is folded WITH a counter (never silent), the
    // progress redraw is announced, and no distinct line is dropped.
    #[test]
    fn banner_real_count_entities_verbatim_and_folds_announced() {
        let src = heavy_build_log(120);
        let orig_lines = src.lines().count();
        let out = try_compact(&src).expect("should compact");

        // Real original line count in the banner.
        assert!(
            out.contains(&format!("[tokenade:npm-run] {orig_lines} line(s)")),
            "real line count missing/wrong: {out}"
        );
        // npm header + the underlying command preserved verbatim (named ents).
        assert!(out.contains("> myapp@1.4.2 build"), "npm header dropped: {out}");
        assert!(out.contains("> webpack --mode production"), "script cmd dropped: {out}");
        // Asset names + final summary preserved verbatim (ANSI stripped).
        assert!(out.contains("asset main.js 1.2 MiB [emitted] [minimized] (name: main)"));
        assert!(out.contains("asset vendor.js 845 KiB [emitted] [minimized]"));
        assert!(out.contains("webpack 5.88.0 compiled with 1 warning in 12345 ms"));
        // The repeated warning is kept ONCE, with a real ×count — not silent.
        assert!(
            out.contains("WARNING: 'sass' loader option is deprecated  (×120)"),
            "duplicate run not folded with counter: {out}"
        );
        // The fold counts are announced in the banner.
        assert!(out.contains("119 duplicate line(s) folded"), "dup count wrong: {out}");
        assert!(out.contains("progress-redraw state(s) folded"), "progress fold missing: {out}");
        // ANSI escapes are gone.
        assert!(!out.contains('\u{1b}'), "raw ANSI escape leaked: {out}");
    }

    // 2b. QUALITY — a unique line buried between two duplicate runs is never
    // lost. Each run is long enough that folding genuinely wins.
    #[test]
    fn unique_lines_between_runs_survive() {
        let mut body = String::new();
        for _ in 0..30 {
            body.push_str("compiling module\n");
        }
        body.push_str("ERROR in ./src/app.ts:42:7 - TS2345: Argument mismatch\n");
        for _ in 0..30 {
            body.push_str("compiling module\n");
        }
        let out = try_compact(&body).expect("should compact");
        assert!(
            out.contains("ERROR in ./src/app.ts:42:7 - TS2345: Argument mismatch"),
            "unique error line dropped: {out}"
        );
        // Both distinct `compiling module` runs are represented (the error line
        // breaks the run, so there are two folded groups, not one).
        assert!(
            out.matches("compiling module  (×30)").count() == 2,
            "expected two separate folded runs: {out}"
        );
    }

    // 3a. MULTILANGUE — folding is byte-equality + structural escape handling,
    // never a keyword list, so a LOCALIZED build log keeps every localized line
    // verbatim while the duplicate/progress noise still folds.
    #[test]
    fn localized_build_log_preserved_verbatim() {
        let mut body = String::from("\n> app@2.0.0 build\n> vite build\n\n");
        // German vite/webpack progress redraw.
        body.push_str("\x1b[2K");
        for pct in (0..=100).step_by(5) {
            let _ = write!(body, "\rWird erstellt … {pct}%");
        }
        body.push('\n');
        // A French deprecation warning repeated per module (coloured).
        for _ in 0..40 {
            body.push_str("\x1b[33mAVERTISSEMENT : l'option « sass » est obsolète\x1b[0m\n");
        }
        // Localized summary line with a comma decimal + accented words.
        body.push_str("Erstellt in 3,45 s — 234 Module transformiert\n");

        let out = try_compact(&body).expect("should compact");
        // Localized lines survive verbatim (no English-keyword dependency).
        assert!(
            out.contains("AVERTISSEMENT : l'option « sass » est obsolète  (×40)"),
            "localized warning not folded with counter: {out}"
        );
        assert!(
            out.contains("Erstellt in 3,45 s — 234 Module transformiert"),
            "localized summary dropped: {out}"
        );
        assert!(out.contains("> vite build"), "script cmd dropped: {out}");
        assert!(!out.contains('\u{1b}'), "ANSI leaked: {out}");
    }

    // 3b. Decline — small clean output is returned unchanged (no banner noise).
    #[test]
    fn small_clean_output_declines() {
        // No ANSI, no `\r`, no duplicates: banner would only enlarge it.
        assert!(try_compact("Done in 0.4s\n").is_none());
        // compact() therefore passes it through verbatim.
        assert_eq!(compact("Done in 0.4s\n"), "Done in 0.4s");
    }
    /// The competition must be able to go BOTH ways.
    ///
    /// `build_log_corpus` pins one side (a real build log keeps `npm`'s fold,
    /// 35.6% vs the route reader's 11.9%). This pins the other: on output that
    /// IS a route table, the content-detected reader must still win — that
    /// reachability is the entire reason `by_content` exists, and a rule that
    /// silently always picked `npm` would leave it dead code nobody noticed.
    #[test]
    fn a_route_table_still_reaches_its_own_reader() {
        let raw = super::super::next_route_table::real_table(400);
        let out = compact(&raw);
        let named = super::super::npm::compact(&raw);
        assert!(
            out.contains("[tokenade:next]"),
            "the route reader must win on its own shape:\n{}",
            &out[..out.len().min(300)]
        );
        assert!(
            crate::tokenizer::estimate(&out) < crate::tokenizer::estimate(&named),
            "and it must win because it is CHEAPER, not by precedence"
        );
    }

}

#[cfg(test)]
mod tap_scripts {
    /// `pnpm test:affiliate` over `tsx --test`: the script's TAP is read as TAP.
    #[test]
    fn a_test_script_printing_tap_is_read_as_tap() {
        let text = include_str!("../../../tests/fixtures/linux/pnpm_tsx_tap.txt");
        let out = super::try_compact(text).expect("a run-script banner");
        assert!(out.contains("[tokenade:tap] 81 passing"), "{}", &out[..out.len().min(300)]);
        assert!(out.contains("# pass 81") && out.contains("# fail 0"));
    }
}
