//! Multi-container k8s log output compaction.
//!
//! `kubectl logs --all-containers --prefix=true deployment/foo` (and the
//! variant `kubectl logs -l <selector> --prefix`) dumps one line per log
//! event with a `[pod/<name>/<container>]` or `[<namespace>/<pod>/<container>]`
//! prefix:
//!
//! ```text
//! [pod/web-1/app]     2024-01-15T10:30:00 INFO server started
//! [pod/web-1/sidecar] 2024-01-15T10:30:00 INFO sidecar started
//! [pod/web-2/app]     2024-01-15T10:30:00 INFO server started
//! [pod/web-2/sidecar] 2024-01-15T10:30:01 INFO sidecar started
//! [pod/web-1/app]     2024-01-15T10:30:05 ERROR connection refused
//! …100 000 more lines across these 4 streams…
//! ```
//!
//! A real production session is millions of lines. What the agent wants:
//!
//!   - per-**stream** count + first/last timestamp (which containers
//!     produced how much)
//!   - every **ERROR/WARN** line across all streams (diagnostic gold)
//!   - **last few** lines per stream as recent-context sample
//!
//! Drops: the bulk of routine INFO output (90%+ of the bytes).
//!
//! Detection: ≥5 lines starting with `[pod/`, `[ns/`, or `[<word>/<word>/<word>]`
//! prefix in the head. Distinguishes from generic logs by the
//! square-bracketed-stream-name shape.

use std::collections::BTreeMap;
use std::fmt::Write;

pub const STREAMS_CAP: usize = 50;
pub const ERRORS_CAP: usize = 200;
pub const WARNINGS_CAP: usize = 100;
pub const TAIL_PER_STREAM: usize = 3;

/// How many prefix-less lines are shown verbatim before the rest are only
/// counted. Errors and warnings are never subject to this — they go through the
/// error/warning path, which has its own caps.
const UNPREFIXED_CAP: usize = 20;

pub fn detect(text: &str) -> bool {
    let head: &str = text.get(..text.len().min(16_384)).unwrap_or(text);
    let mut prefix_lines = 0usize;
    let mut total_nonblank = 0usize;
    for line in head.lines().take(60) {
        if line.trim().is_empty() {
            continue;
        }
        total_nonblank += 1;
        if has_container_prefix(line) {
            prefix_lines += 1;
        }
    }
    prefix_lines >= 5 && total_nonblank > 0 && (prefix_lines * 100) / total_nonblank >= 60
}

/// A line has a container prefix iff it starts with `[`, contains `]`
/// before the first space-or-tab, and the bracketed contents have ≥2
/// slashes (matching `pod/name/container` or `ns/pod/container` shape).
fn has_container_prefix(line: &str) -> bool {
    let trimmed = line.trim_start();
    let Some(rest) = trimmed.strip_prefix('[') else {
        return false;
    };
    let Some(close) = rest.find(']') else {
        return false;
    };
    let inside = &rest[..close];
    if inside.is_empty() {
        return false;
    }
    inside.chars().filter(|c| *c == '/').nth(1).is_some()
}

/// Extract the stream identifier (everything between `[` and `]`) and the
/// remainder of the line.
fn parse_prefix(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix('[')?;
    let close = rest.find(']')?;
    let stream = &rest[..close];
    let tail = rest[close + 1..].trim_start();
    if stream.is_empty() || stream.matches('/').count() < 2 {
        return None;
    }
    Some((stream, tail))
}

fn is_error_line(s: &str) -> bool {
    let upper = s.to_ascii_uppercase();
    upper.contains(" ERROR ")
        || upper.contains("ERROR:")
        || upper.contains(" ERR ")
        || upper.contains(" FATAL")
        || upper.contains("FATAL:")
        || upper.contains(" CRITICAL")
        || upper.contains("CRITICAL:")
        || upper.contains(" CRIT ")
        || upper.contains(" PANIC")
}

fn is_warn_line(s: &str) -> bool {
    let upper = s.to_ascii_uppercase();
    upper.contains(" WARN ") || upper.contains("WARN:") || upper.contains("WARNING:")
}

#[derive(Default)]
struct Stream {
    lines_count: usize,
    errors: usize,
    first_seen: Option<String>,
    last_seen: Option<String>,
    tail: Vec<String>, // Last TAIL_PER_STREAM lines.
}

impl Stream {
    fn observe(&mut self, raw_line: &str) {
        self.lines_count += 1;
        if self.first_seen.is_none() {
            // Best-effort: keep the first non-blank line as a sample seen.
            self.first_seen = Some(raw_line.to_string());
        }
        self.last_seen = Some(raw_line.to_string());
        // Maintain a ring of the last TAIL_PER_STREAM lines.
        if self.tail.len() >= TAIL_PER_STREAM {
            self.tail.remove(0);
        }
        self.tail.push(raw_line.to_string());
    }
}

pub fn compact(text: &str) -> String {
    if !detect(text) {
        return text.to_string();
    }
    // L42 — strip ANSI before grouping. `kubectl logs` colour-codes
    // per-stream prefixes (`pod1/container1`) so streams with codes
    // wouldn't otherwise dedup with the same stream sans codes.
    let stripped = crate::ansi::strip_ansi(text);
    let text = stripped.as_str();

    let mut streams: BTreeMap<String, Stream> = BTreeMap::new();
    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut total_errors = 0usize;
    let mut total_warnings = 0usize;
    let mut total_lines = 0usize;
    let mut unprefixed: Vec<String> = Vec::new();
    let mut total_unprefixed = 0usize;
    for raw in text.lines() {
        if raw.trim().is_empty() {
            continue;
        }
        total_lines += 1;
        // The prefix gate used to `continue` here, BEFORE the error/warning
        // classification below — so a line without a container prefix could not
        // even be rescued as an error. `detect` fires at 60 % prefixed lines,
        // meaning up to two lines in five were dropped that way, and the ones
        // without a prefix are exactly the continuation lines of a panic or a
        // stack trace. A crashed container read as a quiet one.
        //
        // Same rule as `disk.rs` and `netstat`: classify and keep first, and let
        // the prefix decide only what it actually knows about — the per-stream
        // bookkeeping.
        let prefixed = parse_prefix(raw);
        if let Some((stream, _tail)) = prefixed {
            let st = streams.entry(stream.to_string()).or_default();
            st.observe(raw);
            st.errors += usize::from(is_error_line(raw));
        } else if !is_error_line(raw) && !is_warn_line(raw) && unprefixed.len() < UNPREFIXED_CAP {
            unprefixed.push(raw.to_string());
        }
        if prefixed.is_none() {
            total_unprefixed += 1;
        }

        if is_error_line(raw) {
            total_errors += 1;
            if errors.len() < ERRORS_CAP {
                errors.push(raw.to_string());
            }
        } else if is_warn_line(raw) {
            total_warnings += 1;
            if warnings.len() < WARNINGS_CAP {
                warnings.push(raw.to_string());
            }
        }
    }

    if streams.is_empty() {
        return text.to_string();
    }

    let n_streams = streams.len();

    let mut out = String::with_capacity(4096);
    let _ = write!(
        out,
        "{total_lines} prefixed line(s) across {n_streams} stream(s)"
    );
    if total_errors > 0 {
        let _ = write!(out, ", {total_errors} error(s)");
    }
    if total_warnings > 0 {
        let _ = write!(out, ", {total_warnings} warning(s)");
    }
    out.push('\n');

    // Streams summary: sorted by line count desc, then name asc.
    let mut streams_sorted: Vec<_> = streams.iter().collect();
    streams_sorted.sort_by(|a, b| b.1.lines_count.cmp(&a.1.lines_count).then(a.0.cmp(b.0)));
    // Sorted by volume, the cap cut the QUIET streams — and a container that
    // crashed after three lines is the quietest one. Every stream with an
    // error stays (summary and tail); the cap applies to the clean ones.
    let mut clean = 0usize;
    streams_sorted.retain(|(_, s)| {
        s.errors > 0 || {
            clean += 1;
            clean <= STREAMS_CAP
        }
    });

    // Prefix-less lines, verbatim and in order. They come first because they are
    // the ones no other section can account for: an error or a warning among
    // them already went to its own section above, so what is left here is
    // content that would otherwise have vanished without trace.
    if !unprefixed.is_empty() {
        let _ = write!(out, "\nlines with no stream prefix ({total_unprefixed}):\n");
        for line in &unprefixed {
            out.push_str("  ");
            out.push_str(line);
            out.push('\n');
        }
        if total_unprefixed > unprefixed.len() {
            let _ = writeln!(out, "  … {} more", total_unprefixed - unprefixed.len());
        }
    }
    out.push_str("\nstreams (by line count):\n");
    for (name, s) in &streams_sorted {
        if s.errors > 0 {
            let _ = writeln!(
                out,
                "  {:<48} {} line(s), {} error(s)",
                name, s.lines_count, s.errors
            );
        } else {
            let _ = writeln!(out, "  {:<48} {} line(s)", name, s.lines_count);
        }
    }
    if n_streams > streams_sorted.len() {
        let _ = writeln!(
            out,
            "  … {} more stream(s) elided, none with an error",
            n_streams - streams_sorted.len()
        );
    }

    // Errors block — diagnostic gold.
    crate::archetypes::capped_section_with_header(
        &mut out,
        &format!("errors ({total_errors} total, {} shown)", errors.len()),
        &errors,
        total_errors,
        "error(s)",
    );

    // Warnings block.
    crate::archetypes::capped_section_with_header(
        &mut out,
        &format!(
            "warnings ({total_warnings} total, {} shown)",
            warnings.len()
        ),
        &warnings,
        total_warnings,
        "warning(s)",
    );

    // Recent tail per stream (last 3 lines each).
    let _ = write!(out, "\nrecent tail (last {TAIL_PER_STREAM} per stream):\n");
    for (name, s) in &streams_sorted {
        if s.tail.is_empty() {
            continue;
        }
        let _ = writeln!(out, "  {name}:");
        for line in &s.tail {
            out.push_str("    ");
            out.push_str(line);
            out.push('\n');
        }
    }

    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {

    /// Found by generalising the `disk.rs` defect 2026-08-03: this detector
    /// tolerates 40 % prefix-less lines and the compactor dropped every one of
    /// them BEFORE classifying it — so the continuation lines of a panic, and an
    /// unprefixed error line itself, could not even be rescued. A crashed
    /// container read as a quiet one.
    #[cfg(test)]
    mod unprefixed_lines_survive {
        use super::super::compact;

        /// Same shape as `SAMPLE`, which is the shape the detector actually
        /// accepts. A fixture the detector REJECTS would pass through untouched
        /// and every `contains` assertion below would hold vacuously — the test
        /// would prove nothing while looking green.
        fn prefixed(n: usize) -> String {
            (0..n)
                .map(|i| {
                    let s = if i % 2 == 0 { "web-1/app" } else { "web-2/app" };
                    format!("[pod/{s}]     2024-01-15T10:30:00 INFO request {i} served\n")
                })
                .collect()
        }

        /// Guard on the guards: if this stops holding, every assertion in this
        /// module is vacuous.
        #[test]
        fn the_fixture_actually_triggers_the_compactor() {
            let input = prefixed(20);
            assert_ne!(
                compact(&input),
                input,
                "the fixture passes through untouched, so the tests below prove nothing"
            );
        }

        #[test]
        fn an_unprefixed_error_is_still_reported() {
            let input = format!(
                "{}thread 'main' panicked at src/lib.rs:42: assertion failed\n{}",
                prefixed(8),
                prefixed(8)
            );
            let out = compact(&input);
            assert!(
                out.contains("panicked"),
                "an unprefixed panic vanished: {out}"
            );
        }

        #[test]
        fn an_unprefixed_plain_line_is_kept_verbatim() {
            let input = format!(
                "{}   at foo::bar (src/x.rs:11)\n{}",
                prefixed(8),
                prefixed(8)
            );
            let out = compact(&input);
            assert!(
                out.contains("at foo::bar"),
                "a stack-trace continuation vanished: {out}"
            );
        }

        /// The compaction still happens on ordinary container output.
        #[test]
        fn ordinary_container_output_is_still_summarised() {
            let out = compact(&prefixed(400));
            assert!(
                out.contains("streams (by line count):"),
                "no summary: {out}"
            );
            assert!(out.len() < prefixed(400).len(), "output grew");
        }
    }
    use super::*;

    const SAMPLE: &str = "[pod/web-1/app]     2024-01-15T10:30:00 INFO server started
[pod/web-1/sidecar] 2024-01-15T10:30:00 INFO sidecar started
[pod/web-2/app]     2024-01-15T10:30:00 INFO server started
[pod/web-2/sidecar] 2024-01-15T10:30:01 INFO sidecar started
[pod/web-1/app]     2024-01-15T10:30:05 INFO handling request
[pod/web-1/sidecar] 2024-01-15T10:30:05 INFO proxy handling
[pod/web-2/app]     2024-01-15T10:30:05 INFO handling request
[pod/web-1/app]     2024-01-15T10:30:10 ERROR connection refused
[pod/web-1/sidecar] 2024-01-15T10:30:10 WARN  retry attempt 1
[pod/web-2/app]     2024-01-15T10:30:11 ERROR database timeout
[pod/web-1/app]     2024-01-15T10:30:15 INFO recovered
[pod/web-2/sidecar] 2024-01-15T10:30:15 INFO proxy handling
[pod/web-1/app]     2024-01-15T10:30:20 INFO heartbeat
[pod/web-2/app]     2024-01-15T10:30:20 INFO heartbeat
";

    #[test]
    fn a_quiet_stream_that_crashed_is_kept_past_the_cap() {
        let mut s = String::new();
        for i in 0..(STREAMS_CAP + 10) {
            for k in 0..10 {
                let _ = writeln!(s, "[pod/svc{i:02}-7f9c/app] serving request {k}");
            }
        }
        s.push_str("[pod/worker-5d2a/app] starting\n[pod/worker-5d2a/app] ERROR: cannot connect to db\n[pod/worker-5d2a/app] exited with code 1\n");
        let out = compact(&s);
        assert!(
            out.contains("pod/worker-5d2a/app") && out.contains("3 line(s), 1 error(s)"),
            "{out}"
        );
        assert!(out.contains("  pod/worker-5d2a/app:"), "{out}");
        assert!(out.contains("exited with code 1"), "{out}");
        assert!(
            out.contains("… 10 more stream(s) elided, none with an error"),
            "{out}"
        );
    }
    #[test]
    fn detects_kubectl_prefix() {
        assert!(detect(SAMPLE));
    }

    #[test]
    fn detects_namespace_pod_container() {
        // `kubectl logs -n ns --prefix` form: `[default/pod/container]`.
        let s = "[default/web-1/app] 2024-01-15T10:00:00 INFO start
[default/web-1/app] 2024-01-15T10:00:01 INFO start
[default/web-1/app] 2024-01-15T10:00:02 INFO start
[default/web-1/app] 2024-01-15T10:00:03 INFO start
[default/web-1/app] 2024-01-15T10:00:04 INFO start
";
        assert!(detect(s));
    }

    #[test]
    fn rejects_single_stream_logs() {
        // Logs without `[stream]` prefix — the generic logs module handles those.
        let s = "2024-01-15T10:30:00 INFO server started
2024-01-15T10:30:01 INFO handling request
2024-01-15T10:30:02 INFO done
2024-01-15T10:30:03 ERROR oh no
2024-01-15T10:30:04 INFO recovered
";
        assert!(!detect(s));
    }

    #[test]
    fn rejects_grep_style_paths() {
        // grep output uses `path:line:` not `[stream]`.
        let s = "src/main.rs:42:    fn foo() {}
src/lib.rs:15:    fn bar() {}
src/baz.rs:1:    use std;
src/qux.rs:2:    use io;
src/rab.rs:3:    use fmt;
";
        assert!(!detect(s));
    }

    #[test]
    fn rejects_prose() {
        assert!(!detect(
            "This is prose mentioning [pods] and [containers] but not in log format.\n"
        ));
    }

    #[test]
    fn extracts_streams_errors_warnings() {
        let out = compact(SAMPLE);
        // 4 streams: web-1/app, web-1/sidecar, web-2/app, web-2/sidecar.
        assert!(out.contains("14 prefixed line(s) across 4 stream(s)"));
        assert!(out.contains("2 error(s)"));
        assert!(out.contains("1 warning(s)"));
        // Per-stream lines counted.
        assert!(out.contains("pod/web-1/app"));
        assert!(out.contains("pod/web-2/sidecar"));
        // Errors preserved verbatim.
        assert!(out.contains("connection refused"));
        assert!(out.contains("database timeout"));
        // Warning preserved.
        assert!(out.contains("retry attempt 1"));
        // Recent-tail block has each stream.
        assert!(out.contains("recent tail"));
        // Last line of stream web-1/app is "heartbeat".
        assert!(out.contains("heartbeat"));
    }

    #[test]
    fn handles_unicode_in_pod_names() {
        let s = "[default/café-app/main] 2024-01-15T10:30:00 INFO 中文 message
[default/café-app/main] 2024-01-15T10:30:01 INFO ok
[default/中文-pod/sidecar] 2024-01-15T10:30:00 INFO start
[default/中文-pod/sidecar] 2024-01-15T10:30:01 INFO ok
[default/🚀-rocket/launcher] 2024-01-15T10:30:00 INFO launch
[default/🚀-rocket/launcher] 2024-01-15T10:30:01 ERROR Müller-failure occurred
";
        assert!(detect(s));
        let out = compact(s);
        assert!(out.contains("café-app"));
        assert!(out.contains("中文-pod"));
        assert!(out.contains("🚀-rocket"));
        assert!(out.contains("Müller-failure"));
    }

    #[test]
    fn unknown_passes_through() {
        let t = "not container logs";
        assert_eq!(compact(t), t);
    }

    #[test]
    fn savings_huge_multi_stream() {
        // 20 streams × 5 000 lines = 100k lines, 50 errors interspersed.
        let mut s = String::with_capacity(8_000_000);
        for i in 0..100_000 {
            let stream_idx = i % 20;
            let level = if i % 2000 == 0 { "ERROR" } else { "INFO" };
            let _ = writeln!(
                s,
                "[pod/web-{stream_idx}/app] 2024-01-15T10:{:02}:{:02}.{:03} {level} event {i}",
                (i / 60) % 60,
                i % 60,
                i % 1000
            );
        }
        let (out, sav) = compact_with_savings(&s);
        assert!(out.contains("100000 prefixed line(s) across 20 stream(s)"));
        assert!(out.contains("50 error(s)"));
        assert!(sav.percent() > 95.0, "got {sav:?}");
    }
}
