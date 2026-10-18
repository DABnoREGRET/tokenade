//! Extracted from `filter.rs` in the L94/L92 refactor.
//! Body preserved verbatim — only the home changed.

use crate::compress::dedup_lines;
use crate::argv::{has_any_flag, flag_value};
use crate::compress::truncate_middle;
use crate::ansi::strip_ansi;
use std::fmt::Write;

pub fn dispatch(argv: &[String], output: &str) -> String {
    if flag_value(argv, "--message-format").is_some_and(|v| v.starts_with("json")) {
        // `--message-format=json` → auto's JSON compactor is the right tool.
        return crate::auto::compact(output).1;
    }
    if has_any_flag(argv, &["--quiet", "-q"]) {
        return compact_quiet(output);
    }
    compact(output)
}

pub fn compact_quiet(output: &str) -> String {
    let s = strip_ansi(output);
    let cleaned = dedup_lines(&s);
    truncate_middle(&cleaned, 100, 30)
}

pub fn compact(output: &str) -> String {
    // Streaming-style filter for monorepo builds (10–50 MB of `Compiling …`
    // chatter): keep the first 10 `Compiling X` lines as a sample, drop the
    // rest, ALWAYS keep error/warning diagnostic blocks **in full** (file/line
    // arrow + source snippet + help text), and end with the `Finished` line.
    //
    // A diagnostic "block" is the header line (`error[…]:` / `warning[…]:`)
    // plus every subsequent indented or blank line until the next non-indented
    // top-level token. This preserves the file:line context an agent needs to
    // fix the build instead of just knowing one error exists.
    let s = strip_ansi(output);
    let lines: Vec<&str> = s.lines().collect();
    let mut compiling_seen: usize = 0;
    const COMPILING_SAMPLE: usize = 10;
    let mut compiling_lines: Vec<String> = Vec::new();
    let mut error_count: usize = 0;
    let mut warning_count: usize = 0;
    // Everything that is neither a `Compiling` line, a `Finished` line, nor
    // part of a diagnostic block used to be dropped without a word. Under the
    // command path that was almost harmless — cargo prints little else. Now
    // this compactor also sees CONTENT, where a build log can carry
    // anything, and the fuzz router caught it: `SILENT LOSS kind=CargoBuild
    // needle gone, no disclosure`. Counted and disclosed now.
    let mut other_dropped: usize = 0;
    let mut blocks: Vec<String> = Vec::new();
    let mut finished = String::new();

    let is_block_header = |t: &str| {
        t.starts_with("error[")
            || t.starts_with("error:")
            || t.starts_with("warning[")
            || t.starts_with("warning:")
    };
    let is_block_continuation = |line: &str| {
        let trim_left = line.trim_start();
        // rustc's source snippet starts each line with `<lineno> |  <code>` —
        // the lineno is at column 0, NOT preceded by whitespace. Catch that
        // shape explicitly so the source-arrow body survives.
        let is_rustc_snippet = {
            let mut chars = line.chars();
            let leading_digit = chars.next().is_some_and(|c| c.is_ascii_digit());
            leading_digit && line.contains(" | ")
        };
        line.starts_with(' ')
            || line.starts_with('\t')
            || line.is_empty()
            || trim_left.starts_with("--> ")
            || trim_left.starts_with('|')
            || trim_left.starts_with("= ")
            || is_rustc_snippet
    };

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim_start();
        if is_block_header(t) {
            if t.starts_with("error") {
                error_count += 1;
            } else {
                warning_count += 1;
            }
            let mut block = String::from(line);
            block.push('\n');
            let mut j = i + 1;
            while j < lines.len() && is_block_continuation(lines[j]) {
                block.push_str(lines[j]);
                block.push('\n');
                j += 1;
            }
            blocks.push(block);
            i = j;
            continue;
        }
        if t.starts_with("Compiling") {
            compiling_seen += 1;
            if compiling_lines.len() < COMPILING_SAMPLE {
                compiling_lines.push(line.to_string());
            }
        } else if t.starts_with("Finished") {
            finished = line.to_string();
        } else if !t.is_empty() {
            other_dropped += 1;
        }
        i += 1;
    }

    let mut out = String::new();
    if !compiling_lines.is_empty() {
        out.push_str(&compiling_lines.join("\n"));
        out.push('\n');
        if compiling_seen > compiling_lines.len() {
            let _ = writeln!(
                out,
                "… {} more Compiling line(s) elided …",
                compiling_seen - compiling_lines.len()
            );
        }
    }
    if !blocks.is_empty() {
        let _ = writeln!(
            out,
            "{error_count} error(s), {warning_count} warning(s):"
        );
        // Keep blocks in full — they are THE actionable signal. Cap each at
        // 25 lines so a runaway diagnostic doesn't explode the output.
        for block in &blocks {
            let bl: Vec<&str> = block.lines().collect();
            if bl.len() > 25 {
                out.push_str(&bl[..20].join("\n"));
                let _ = write!(out, "\n… {} more line(s) elided …\n", bl.len() - 20);
            } else {
                out.push_str(block);
            }
        }
    } else if error_count == 0
        && warning_count == 0
        && compiling_seen > 0
        // #465: only emit the "build ok (N crate(s) compiled)" summary
        // when it actually adds signal — i.e. some Compiling lines were
        // elided (so the agent needs the total count) OR there's no
        // Finished line as a separate success marker. Without this
        // guard, small builds (1–3 crates with a Finished line) get the
        // redundant summary tacked on AFTER all the data is already
        // present, which inflates ~8 tokens per call. Production 30d:
        // 63/182 cargo-build calls inflated, 59 of them with b<100.
        && (compiling_seen > compiling_lines.len() || finished.is_empty())
    {
        let _ = writeln!(out, "build ok ({compiling_seen} crate(s) compiled)");
    }
    // Only ever an ADDITION to a summary that already says something. When no
    // structured signal fired at all, `out` must stay empty so the #548
    // fallback below returns the input verbatim — a `make` or `gradle` run
    // routed here would otherwise be REPLACED by this one line, which is far
    // worse than the silent loss it exists to fix. The unit tests for that
    // fallback are what caught it.
    if other_dropped > 0 && !(out.trim().is_empty() && finished.is_empty()) {
        let _ = writeln!(out, "… {other_dropped} other line(s) elided …");
    }
    if !finished.is_empty() {
        out.push_str(&finished);
        out.push('\n');
    }
    let out = out.trim_end().to_string();
    // #548 — when none of the structured signals fired (no Compiling /
    // error / warning / Finished), `out` is empty even though the
    // input wasn't. Common case: `cargo build` chatter like
    // `Updating crates.io index`, `Blocking waiting for file lock on
    // package cache`, or a non-cargo binary that happened to be named
    // `cargo-foo`. The empty triggers filter.rs's safety_net_empty
    // fallback (12% generic compaction); returning the ANSI-stripped
    // input verbatim is strictly better (same data minus color codes,
    // no empty fallback, label stays `cargo-build`).
    if out.is_empty() && !s.trim().is_empty() {
        return s;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_build_with_finished_does_not_add_redundant_summary() {
        // Regression for #465: when all Compiling lines fit and a
        // Finished line is present, the "build ok (N crate(s) compiled)"
        // summary is redundant and net-inflates the output. Production
        // 30d: 63 of 182 cargo-build calls inflated this way.
        let input = "   Compiling foo v0.1.0 (/path/foo)\n    Finished `release` profile [optimized] target(s) in 1.23s\n";
        let out = compact(input);
        assert!(
            !out.contains("build ok"),
            "small build with Finished must not add redundant summary; got: {out}"
        );
        assert!(out.contains("Compiling foo"), "Compiling line must be kept");
        assert!(out.contains("Finished"), "Finished line must be kept");
        assert!(
            out.len() <= input.len(),
            "small build output must not inflate; input={} out={}",
            input.len(),
            out.len()
        );
    }

    #[test]
    fn large_build_still_gets_summary_when_lines_elided() {
        // The summary is the user-facing signal that says "all crates
        // compiled" when some Compiling lines were dropped. Without
        // this, the agent only sees the truncated head + Finished and
        // has to count Compiling lines manually.
        let mut input = String::new();
        for i in 0..25 {
            let _ = writeln!(input, "   Compiling crate{i} v0.1.0 (/path/{i})");
        }
        input.push_str("    Finished `release` profile target(s) in 5s\n");
        let out = compact(&input);
        assert!(
            out.contains("build ok (25 crate(s) compiled)"),
            "large build must keep summary so elided lines are accounted for; got: {out}"
        );
    }

    #[test]
    fn build_with_no_finished_still_gets_summary() {
        // When the Finished line is absent (interrupted build, custom
        // wrappers), the summary is the only success signal — keep it.
        let input = "   Compiling foo v0.1.0\n   Compiling bar v0.1.0\n";
        let out = compact(input);
        assert!(
            out.contains("build ok"),
            "build with no Finished line still needs the summary signal; got: {out}"
        );
    }

    #[test]
    fn unrecognized_input_falls_back_to_ansi_stripped_original() {
        // #548 — when input contains no cargo-shape lines (no Compiling /
        // error / warning / Finished), the compactor used to return empty
        // — triggering filter.rs's safety_net_empty fallback that ran the
        // generic compactor at ~12% efficiency. Should now return the
        // ANSI-stripped input verbatim.
        let input = "    Updating crates.io index\n    Blocking waiting for file lock on package cache\n";
        let out = compact(input);
        assert!(
            !out.is_empty(),
            "non-empty input must produce non-empty output (was triggering safety_net_empty)"
        );
        assert!(
            out.contains("Updating crates.io index"),
            "fallback must preserve the original lines; got: {out}"
        );
        assert!(
            out.contains("Blocking waiting for file lock"),
            "fallback must preserve all lines; got: {out}"
        );
    }

    #[test]
    fn unrecognized_input_strips_ansi_codes() {
        // Falling back to the ANSI-stripped input still does ONE job —
        // removing color codes that would otherwise waste tokens.
        let input = "\x1b[32m    Updating crates.io index\x1b[0m\n";
        let out = compact(input);
        assert!(
            !out.contains("\x1b["),
            "ANSI escapes must still be stripped in fallback; got: {out:?}"
        );
        assert!(out.contains("Updating crates.io index"));
    }
    /// A line that is neither `Compiling`, nor `Finished`, nor part of a
    /// diagnostic block used to vanish without a word. Harmless while this
    /// compactor only ever saw output tokenade had RUN; a real defect once
    /// it could see content. The fuzz router named it:
    /// `SILENT LOSS kind=CargoBuild needle gone, no disclosure`.
    #[test]
    fn a_line_that_is_none_of_the_known_shapes_is_disclosed() {
        let out = compact(
            "   Compiling foo v1.0.0\n             NEEDLE-0123456789abcdef this line is none of the shapes\n             another one\n                 Finished `dev` profile in 1.00s\n",
        );
        assert!(
            out.contains("2 other line(s) elided"),
            "the dropped lines were not disclosed:\n{out}"
        );
    }

    /// And the disclosure is an ADDITION, never the whole answer: when no
    /// structured signal fires the compactor must still hand back the input
    /// verbatim (#548), or a `make`/`gradle` run routed here would be replaced
    /// by a single elision line.
    #[test]
    fn the_disclosure_never_replaces_unrecognised_input() {
        let raw = "some build tool said this\nand then this\n";
        assert_eq!(compact(raw), raw);
    }

}
