//! `tsc` — TypeScript compiler diagnostics.
//!
//! Folds `tsc` / `tsc --noEmit` / `npx tsc` output into a per-error list:
//! one line per diagnostic (file:line:col + `TSxxxx` code + message), the
//! multi-line source frames dropped, and the `Found N errors` summary kept.
//!
//! Both tsc output shapes carry the full diagnostic on a single *header*
//! line — plain `file(line,col): error TS2322: msg` and pretty
//! `file:line:col - error TS2322: msg` — so we keep those lines verbatim and
//! discard the source frames (`10   const x = …` + the `~~~~` underline) and
//! the trailing `Errors  Files` table, which only repeat what the agent can
//! already read from the file at the cited location.
//!
//! Detection is by content (the distinctive `TS<digit>` diagnostic code), so
//! it fires regardless of how tsc was launched — bare `tsc`, `npx tsc`,
//! `node_modules/.bin/tsc`, or an `npm run typecheck` wrapper — because they
//! all converge on the same diagnostic text.

use crate::ansi::strip_ansi;
use std::fmt::Write;

/// Diagnostic header lines kept verbatim before the tail is folded.
const HEAD: usize = 50;

/// True when `s` contains a TypeScript diagnostic code: the keyword
/// `error `/`warning ` immediately followed by `TS` and a digit
/// (e.g. `error TS2322`). The digit guard keeps prose like "an error TStation"
/// from false-matching.
fn has_ts_code(s: &str) -> bool {
    // Ratio-based: real tsc output is DOMINATED by diagnostic/frame/summary
    // lines, while an `npm run build` transcript with a single `error TS1234`
    // buried in webpack/ESLint output is not — the old any-hit rule claimed
    // the whole document and the compactor (diagnostics-only) destroyed the
    // rest. Require TS-diagnostic lines to be ≥30% of non-empty lines.
    let mut nonempty = 0usize;
    let mut diag = 0usize;
    let mut tscish = 0usize;
    for l in s.lines() {
        if l.trim().is_empty() {
            continue;
        }
        nonempty += 1;
        let is_diag = ["error TS", "warning TS"].iter().any(|kw| {
            l.find(kw).is_some_and(|idx| {
                l[idx + kw.len()..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_digit())
            })
        });
        // Pretty-mode frames (`10   const x…`), `~~~` underlines and the
        // `Found N error` summary belong to the same document — count them
        // toward the ratio so a legit single-error pretty run still detects.
        let t = l.trim_start();
        let is_tsc_ish = is_diag
            || is_summary(l)
            || t.chars().all(|c| c == '~' || c == ' ')
            || t.split_whitespace()
                .next()
                .is_some_and(|w| w.chars().all(|c| c.is_ascii_digit()));
        if is_diag {
            diag += 1;
        }
        if is_tsc_ish {
            tscish += 1;
        }
    }
    diag >= 1 && tscish * 10 >= nonempty * 3
}

pub fn detect(text: &str) -> bool {
    has_ts_code(text)
}

/// A line that is the trailing `Found N error(s)…` summary tsc prints once
/// (also in `--watch` mode, where it is prefixed with a `[hh:mm:ss] ` stamp).
fn is_summary(line: &str) -> bool {
    let t = line.trim_start();
    // Strip a leading `[hh:mm:ss AM] ` watch-mode timestamp if present.
    let t = t
        .strip_prefix('[')
        .and_then(|r| r.split_once("] "))
        .map(|(_, rest)| rest)
        .unwrap_or(t);
    t.starts_with("Found ") && t.contains("error")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    let s = strip_ansi(text);
    let mut headers: Vec<&str> = Vec::new();
    let mut summaries: Vec<&str> = Vec::new();
    let mut nonempty = 0usize;
    let mut errors = 0usize;
    let mut warnings = 0usize;
    for raw in s.lines() {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        nonempty += 1;
        if has_ts_code(line) {
            if line.contains("error TS") {
                errors += 1;
            } else {
                warnings += 1;
            }
            headers.push(line);
        } else if is_summary(line) {
            summaries.push(line);
        }
    }
    if headers.is_empty() {
        return None;
    }
    // Lines we drop = noise: source frames, blank lines, the per-file
    // `Errors  Files` table. When there is nothing to drop AND the error
    // count is within the kept budget, the output is already minimal — leave
    // it byte-for-byte unchanged rather than adding a banner (net inflation).
    let kept_total = headers.len() + summaries.len();
    if kept_total >= nonempty && headers.len() <= HEAD {
        return None;
    }
    let head = crate::read_mode::effective_head(HEAD);
    let tag = crate::read_mode::banner_tag();
    let mut out = format!("[tokenade:tsc{tag}] {errors} error(s), {warnings} warning(s)\n");
    let shown = headers.len().min(head);
    for h in &headers[..shown] {
        out.push_str(h);
        out.push('\n');
    }
    if headers.len() > head {
        // The elided diagnostics are file locations, so the elision names the
        // directories they fall in rather than only how many there were: "12
        // more" and "12 more, all under src/generated/" are different facts
        // about whether the build is worth reading. Same disclosure `find`
        // has always printed for a folded path list.
        let rest: Vec<&str> = headers[head..]
            .iter()
            .map(|h| h.split(['(', ':']).next().unwrap_or(h).trim())
            .filter(|p| p.contains('/'))
            .collect();
        if rest.is_empty() {
            let _ = writeln!(out, "  ... (+{} more)", headers.len() - head);
        } else {
            let census = crate::filelist::path_census(&rest);
            let _ = writeln!(out, "  ... (+{} more) — {census}", headers.len() - head);
        }
    }
    for sm in &summaries {
        out.push_str(sm);
        out.push('\n');
    }
    Some(out.trim_end().to_string())
}

pub fn compact(text: &str) -> String {
    try_compact(text).unwrap_or_else(|| crate::archetypes::passthrough(text))
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_rejects_empty_and_prose() {
        assert!(!detect(""));
        assert!(!detect("just some prose mentioning a TypeScript error\n"));
        // `TS` not followed by a digit must not match.
        assert!(!detect("error TStation closed\n"));
    }

    #[test]
    fn detect_accepts_diagnostic_code() {
        assert!(detect("src/a.ts(10,7): error TS2322: Type mismatch.\n"));
        assert!(detect("src/a.ts:10:7 - warning TS6133: unused.\n"));
    }

    #[test]
    fn plain_minimal_passes_through() {
        // Plain one-line-per-error with a summary is already minimal — folding
        // would only add a banner, so try_compact declines.
        let s = "src/a.ts(10,7): error TS2322: Type 'number' not assignable to 'string'.\n\
                 src/b.ts(3,1): error TS1005: ';' expected.\n\
                 Found 2 errors in 2 files.\n";
        assert!(try_compact(s).is_none());
    }

    #[test]
    fn pretty_frames_are_dropped() {
        // Pretty output: the header carries the whole diagnostic; the source
        // frame + underline are redundant noise we drop.
        let s =
            "src/index.ts:10:7 - error TS2322: Type 'number' is not assignable to type 'string'.\n\
                 \n\
                 10   const x: string = 5;\n\
                 \u{20}\u{20}\u{20}\u{20}\u{20}~\n\
                 \n\
                 Found 1 error in src/index.ts:10\n";
        let out = try_compact(s).expect("pretty output should fold");
        assert!(out.contains("[tokenade:tsc]"));
        assert!(out.contains("1 error(s), 0 warning(s)"));
        assert!(out.contains("error TS2322"));
        // Frame body + underline gone.
        assert!(!out.contains("const x: string = 5;"));
        assert!(!out.contains('~'));
        // Summary kept.
        assert!(out.contains("Found 1 error"));
    }

    #[test]
    fn errors_files_table_is_dropped() {
        // Newer plain tsc appends an `Errors  Files` table that just repeats
        // the file:line already on each header — pure noise.
        let s = "src/a.ts(10,7): error TS2322: msg one.\n\
                 src/b.ts(3,1): error TS1005: msg two.\n\
                 Found 2 errors in 2 files.\n\
                 \n\
                 Errors  Files\n\
                 \u{20}\u{20}\u{20}\u{20} 1  src/a.ts:10\n\
                 \u{20}\u{20}\u{20}\u{20} 1  src/b.ts:3\n";
        let out = try_compact(s).expect("table noise should trigger fold");
        assert!(out.contains("2 error(s), 0 warning(s)"));
        assert!(!out.contains("Errors  Files"));
        assert!(out.contains("Found 2 errors"));
    }

    #[test]
    fn many_errors_elide_tail() {
        let mut s = String::new();
        for i in 0..65 {
            // Alternate frame lines in so there is noise to strip.
            let _ = writeln!(s, "src/f{i}.ts({i},1): error TS2322: msg {i}.");
            let _ = writeln!(s, "{i}   const x = bad;");
        }
        s.push_str("Found 65 errors.\n");
        let out = try_compact(&s).expect("should fold");
        assert!(out.contains("65 error(s)"));
        assert!(out.contains("(+15 more)"));
        assert!(out.contains("Found 65 errors"));
    }
    /// The elided diagnostics are file locations, so the elision names the
    /// directories. "70 more" and "70 more, all under generated/" are
    /// different facts about whether the build is worth reading.
    #[test]
    fn the_elided_diagnostics_say_where_they_are() {
        let mut s = String::new();
        for i in 0..60 {
            let _ = writeln!(
                s,
                "src/app/mod_{i}.ts({i},3): error TS2345: Argument of type 'a{i}' is not assignable."
            );
        }
        for i in 0..60 {
            let _ = writeln!(
                s,
                "generated/proto/gen_{i}.ts({i},7): error TS2339: Property 'p{i}' does not exist."
            );
        }
        s.push_str("Found 120 errors in 120 files.\n");
        let out = try_compact(&s).expect("should fold");
        let line = out
            .lines()
            .find(|l| l.contains("more)"))
            .unwrap_or_default();
        assert!(!line.is_empty(), "no elision line at all:\n{out}");
        assert!(
            line.contains('/'),
            "the elision does not say where the diagnostics are: {line}"
        );
    }
}
