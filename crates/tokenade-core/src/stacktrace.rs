//! Stack-trace compaction across Python, Rust, JavaScript/Node, Java, Go.
//!
//! Real production stack traces are dominated by **framework frames** the
//! agent doesn't care about: `site-packages/django/handlers/exception.py`,
//! `node_modules/express/lib/router/route.js`, `.cargo/registry/src/.../tokio/`,
//! `java.lang.reflect.Method.invoke`. The signal is the *error message* and
//! the *user-code frames* — usually one to three lines in a 200-line trace.
//!
//! [`compact`] auto-detects the language by frame syntax, classifies every
//! frame as user or framework via path heuristics, then keeps every user
//! frame + a small border of framework context, folding the rest as
//! `… N framework frames (e.g. tokio::runtime) …`.
//!
//! Never panics on unknown formats — falls back to a generic line-by-line
//! head/tail truncate of the input.

mod dedup;
mod families;
mod frames;
pub(crate) use frames::is_js_framework;

/// One JavaScript stack frame, in any of the shapes a runner prints it —
/// `at Fn.name (where:1:2)`, `Fn.name (where:1:2)`, `at async …`, a bare
/// `where:1:2` — belongs to the RUNTIME (Node internals, a dependency).
/// The location is what decides, so it is read from the parentheses first.
pub(crate) fn is_runtime_js_frame(frame: &str) -> bool {
    let t = frame.trim();
    let t = t.strip_prefix("at ").unwrap_or(t);
    let t = t.strip_prefix("async ").unwrap_or(t);
    let t = t.trim_end_matches(" {");
    let location = match (t.rfind('('), t.ends_with(')')) {
        (Some(o), true) => &t[o + 1..t.len() - 1],
        _ => t,
    };
    is_js_framework(location)
}

/// Every run of JavaScript `at …` frames in `text`, with the runtime's frames
/// counted on one line and the program's kept — never all of a run (an error
/// raised inside the runtime keeps its first frame), and never the `{` that
/// Node opens the error's properties with on the LAST frame's line.
///
/// One mechanism for every route that prints a JS stack: a test runner's
/// failure (`pytest.rs` routes Node's spec reporter, jest, mocha) and a plain
/// `node app.js` that crashed. Returns how many frames went.
pub(crate) fn fold_js_frame_runs(text: &str) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut run: Vec<&str> = Vec::new();
    let mut total = 0usize;
    let flush = |out: &mut String, run: &mut Vec<&str>, total: &mut usize| {
        if run.is_empty() {
            return;
        }
        let own: Vec<&str> = run
            .iter()
            .copied()
            .filter(|f| !is_runtime_js_frame(f))
            .collect();
        let kept = if own.is_empty() { vec![run[0]] } else { own };
        for k in &kept {
            out.push_str(k);
            out.push('\n');
        }
        let dropped = run.len() - kept.len();
        if dropped > 0 {
            let pad = &run[0][..run[0].len() - run[0].trim_start().len()];
            let last = run[run.len() - 1];
            let brace = if last.trim_end().ends_with(" {") && !kept.contains(&last) {
                " {"
            } else {
                ""
            };
            out.push_str(&format!(
                "{pad}[{dropped} runtime frame(s) not shown]{brace}\n"
            ));
            *total += dropped;
        }
        run.clear();
    };
    for line in text.lines() {
        if line.trim_start().starts_with("at ") {
            run.push(line);
            continue;
        }
        flush(&mut out, &mut run, &mut total);
        out.push_str(line);
        out.push('\n');
    }
    flush(&mut out, &mut run, &mut total);
    if !text.ends_with('\n') {
        out.pop();
    }
    (out, total)
}
mod languages;
pub use dedup::*;
pub use families::*;
pub use frames::*;
pub(crate) use languages::*;

use regex::Regex;
use std::fmt::Write;
use std::sync::OnceLock;

/// How many framework frames to keep on each side of a user frame "island"
/// (so the agent has *some* context about the call chain).
pub const BORDER: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Python,
    Rust,
    JavaScript,
    Java,
    Go,
    Unknown,
}

pub fn detect_lang(text: &str) -> Lang {
    // Cheap leading-fingerprint detection.
    if text.contains("Traceback (most recent call last)") || re_py_frame().is_match(text) {
        Lang::Python
    } else if text.contains("thread '") && text.contains("panicked at") {
        Lang::Rust
    } else if js_frame_line(text) || re_js_frame().is_match(text) {
        Lang::JavaScript
    } else if re_java_frame().is_match(text) {
        Lang::Java
    } else if text.contains("goroutine ") && text.contains("[running]") {
        Lang::Go
    } else {
        Lang::Unknown
    }
}

/// A V8-style frame with no `:line:col`, ANCHORED at the start of its line.
///
/// This was `text.contains("at /") || text.contains("at file://")` — an
/// unanchored substring of three characters. Any prose that says "stored at
/// /work/" carries it, and a `docker-compose.yml` whose header comment does
/// exactly that was folded as a JavaScript stack trace: 28 709 bytes in,
/// **854 out**, every service, volume and network gone and only the leading
/// comments left. Measured 2026-08-26.
///
/// `re_js_frame` already covers the normal `at name (path:N:M)` form; this
/// keeps the line-number-less variant some runtimes emit, without letting a
/// sentence stand in for a frame.
fn js_frame_line(text: &str) -> bool {
    text.lines().any(|l| {
        let t = l.trim_start();
        t.strip_prefix("at ")
            .is_some_and(|rest| rest.starts_with('/') || rest.starts_with("file://"))
    })
}

/// Compact a stack trace. The result keeps the original error message line(s)
/// plus a compressed frame list.
pub fn compact(text: &str) -> String {
    // Pre-pass: in multi-test failure outputs (pytest, cargo-test, jest, mocha,
    // go-test) the same traceback is often repeated across N tests. Fold
    // identical blocks BEFORE per-language compaction so we don't re-emit
    // the same frames N times after compression.
    let deduped = dedup_blocks(text);
    let target = if deduped.len() < text.len() {
        deduped
    } else {
        text.to_string()
    };
    match detect_lang(&target) {
        Lang::Python => compact_python(&target),
        Lang::Rust => compact_rust(&target),
        Lang::JavaScript => compact_js(&target),
        Lang::Java => compact_java(&target),
        Lang::Go => compact_go(&target),
        Lang::Unknown => generic_compact(&target),
    }
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
include!("stacktrace_tests.rs");
