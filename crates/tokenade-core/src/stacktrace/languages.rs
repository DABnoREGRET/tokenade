//! **One compactor per language.**
//!
//! Python, Rust, JavaScript, Java, Go do not write their traces the same way
//! — neither the direction (Python goes up, Java goes down), nor the frame
//! marker, nor the position of the message. The generic fallback exists for
//! what none of them recognises.
//!
//! `python_repeat_note` is the proof that the shape carries meaning: Python
//! itself says "[Previous line repeated N times]", and dropping that line
//! would drop the most useful fact in the trace.

use super::*;

// ---- per-language compactors ---------------------------------------------

/// Build a compact frame list given parallel arrays of `(is_user, repr)`.
/// Keeps every user frame + `BORDER` framework frames on each side. Runs of
/// purely-framework frames longer than `2*BORDER` get folded.
pub(crate) fn fold(frames: Vec<(bool, String)>) -> Vec<String> {
    let n = frames.len();
    let keep: Vec<bool> = (0..n)
        .map(|i| {
            if frames[i].0 {
                return true; // user frame
            }
            // border around any nearby user frame
            let left = i.saturating_sub(BORDER);
            let right = (i + BORDER + 1).min(n);
            (left..right).any(|j| frames[j].0)
        })
        .collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < n {
        if keep[i] {
            out.push(frames[i].1.clone());
            i += 1;
        } else {
            // Fold this run of framework frames — when it is LONGER than
            // `2*BORDER`, which is what this function always said and never
            // checked. A short run stays: three frames are worth less than the
            // marker that replaces them, and on a trace with no user frame they
            // are the whole trace. MEASURED in a transcript: a 4-line
            // `drizzle-kit` trace came back as one frame and `… 3 framework
            // frame(s)`, the failing `prepareAndMigratePg` folded away, and the
            // agent's `--grep` on the stash found nothing else to read.
            let start = i;
            while i < n && !keep[i] {
                i += 1;
            }
            let run = i - start;
            if run <= 2 * BORDER {
                out.extend(frames[start..i].iter().map(|f| f.1.clone()));
                continue;
            }
            // Hint: include a representative source so the agent knows *what*
            // framework chain we elided.
            let hint = frames[start]
                .1
                .split_whitespace()
                .find(|w| w.contains('/') || w.contains('.'))
                .unwrap_or("framework")
                .trim_end_matches(['(', ')', ':']);
            out.push(format!("… {run} framework frame(s) (e.g. {hint}) …"));
        }
    }
    out
}

/// Python's own "these frames repeat" line, if `line` is one.
///
/// CPython writes `  [Previous line repeated N more times]` when a traceback
/// would otherwise print the same frame over and over. It is the one line that
/// states the RECURSION DEPTH, which is exactly the fact a recursion bug turns
/// on, and it is the interpreter's own honest count — not something we can
/// recompute once the frames are folded.
pub(crate) fn python_repeat_note(line: &str) -> Option<String> {
    let t = line.trim();
    (t.starts_with("[Previous line repeated") && t.ends_with(']')).then(|| t.to_string())
}

/// **One traceback at a time.** The condenser below reads ONE traceback:
/// header, frames, then the exception. Given two — a service log, a test run —
/// it pooled every frame into one list and kept the first `Traceback` line
/// only, so two different failures came back as a single four-frame trace that
/// never happened, and an indented exception line (a journal's messages) was
/// dropped (2026-10-03). Each `Traceback (most recent call last):` opens its own
/// block; what precedes the first is kept as it was.
pub(crate) fn compact_python(text: &str) -> String {
    let opens = |l: &str| {
        l.trim_start()
            .starts_with("Traceback (most recent call last)")
    };
    let lines: Vec<&str> = text.lines().collect();
    let starts: Vec<usize> = (0..lines.len()).filter(|&i| opens(lines[i])).collect();
    if starts.len() < 2 {
        return compact_python_one(text);
    }
    let mut out: Vec<String> = Vec::new();
    if starts[0] > 0 {
        out.push(lines[..starts[0]].join("\n"));
    }
    for (k, &s) in starts.iter().enumerate() {
        let end = starts.get(k + 1).copied().unwrap_or(lines.len());
        out.push(compact_python_one(&lines[s..end].join("\n")));
    }
    out.join("\n")
}

fn compact_python_one(text: &str) -> String {
    let mut frames: Vec<(bool, String)> = Vec::new();
    let mut header = String::new();
    let mut tail = String::new();
    let pyf = re_py_frame();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some(cap) = pyf.captures(line) {
            let path = cap.get(1).unwrap().as_str();
            let lineno = cap.get(2).unwrap().as_str();
            let func = cap.get(3).unwrap().as_str();
            // Lookahead one line: the source-code line under the File line.
            let src = lines.get(i + 1).copied().unwrap_or("").trim();
            let is_user = !is_python_framework(path);
            let repr = format!("{path}:{lineno} in {func}   {src}");
            frames.push((is_user, repr));
            i += if !src.is_empty() && !src.starts_with("File ") {
                2
            } else {
                1
            };
            continue;
        }
        if frames.is_empty() {
            header.push_str(line);
            header.push('\n');
        } else if let Some(rep) = python_repeat_note(line) {
            // Python's own count of the frames IT collapsed. Indented, so it
            // matched none of the branches below and was dropped in silence:
            // measured 2026-09-06, a 40-deep recursion came back showing three
            // frames with nothing saying 37 more had been folded — and for a
            // runaway recursion the DEPTH is the diagnosis. Kept as a frame so
            // it stays in position, between the frames it stands for.
            frames.push((true, rep));
        } else if line.starts_with("Traceback ") {
            // ignore nested-frame marker repeats
        } else if !line.trim().is_empty()
            && (!line.starts_with(' ') || !frames.is_empty() && is_exception_line(line.trim()))
        {
            tail.push_str(line);
            tail.push('\n');
        }
        i += 1;
    }
    let folded = fold(frames);
    let mut out = header.trim_end().to_string();
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(&folded.join("\n"));
    if !tail.trim().is_empty() {
        out.push('\n');
        out.push_str(tail.trim_end());
    }
    out
}

/// `AttributeError: …`, `pkg.CustomError`, `KeyboardInterrupt` excepted —
/// the line that names what was raised.
fn is_exception_line(t: &str) -> bool {
    let name = t.split([':', ' ']).next().unwrap_or("");
    let last = name.rsplit('.').next().unwrap_or("");
    (last.ends_with("Error") || last.ends_with("Exception") || last.ends_with("Warning"))
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '.' || c == '_')
}

/// Every line after the first frame that was not a frame was dropped: a
/// second thread's `panicked` header, and the `at file:line` under each frame
/// (2026-10-03). Locations ride on their frame; other lines stay in position.
pub(crate) fn compact_rust(text: &str) -> String {
    // Rust panics: header `thread '...' panicked at file:line: message`, then
    // optionally a numbered backtrace (with `RUST_BACKTRACE=1`).
    let frame_re = re(r"(?m)^\s*\d+:\s+0x[0-9a-f]+\s+-\s+(.+)$");
    let mut frames: Vec<(bool, String)> = Vec::new();
    let mut header = String::new();
    let mut started = false;
    for line in text.lines() {
        if let Some(cap) = frame_re.captures(line) {
            started = true;
            let func = cap.get(1).unwrap().as_str().trim();
            let is_user = !is_rust_framework(func);
            frames.push((is_user, func.to_string()));
            continue;
        }
        if !started {
            header.push_str(line);
            header.push('\n');
        } else if let (Some(at), Some(last)) = (line.trim().strip_prefix("at "), frames.last_mut())
        {
            // `at ./src/main.rs:5:9` under a frame is WHERE it is: it was
            // dropped with every other line after the first frame
            // (2026-10-03). It rides on its frame, folded or kept with it.
            last.1.push_str(&format!("  at {at}"));
        } else if !line.trim().is_empty() {
            frames.push((true, line.trim().to_string()));
        }
    }
    if frames.is_empty() {
        return generic_compact(text);
    }
    let mut out = header.trim_end().to_string();
    out.push('\n');
    out.push_str(&fold(frames).join("\n"));
    out
}

pub(crate) fn compact_js(text: &str) -> String {
    per_trace(text, compact_js_one)
}

/// **One trace at a time**, for the condensers that read ONE: JavaScript and
/// Java. Two errors in one output — `TypeError …` then `Error: connect
/// ECONNREFUSED …` — came back with the second one's frames under the first
/// and the second keeping a single frame: a trace that never happened
/// (2026-10-03; Python had the same flaw, see `compact_python`). A new trace
/// opens at an unindented line naming an exception, once frames were seen;
/// Java's `Caused by:` is a cause, not a new trace, and stays inside.
fn per_trace(text: &str, one: fn(&str) -> String) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let frame = |l: &str| l.trim_start().starts_with("at ");
    let mut starts = vec![0usize];
    let mut seen_frame = false;
    for (i, l) in lines.iter().enumerate() {
        if frame(l) {
            seen_frame = true;
        } else if seen_frame && !l.starts_with([' ', '\t']) && is_exception_line(l.trim()) {
            starts.push(i);
            seen_frame = false;
        }
    }
    if starts.len() < 2 {
        return one(text);
    }
    starts.push(lines.len());
    starts
        .windows(2)
        .map(|w| one(&lines[w[0]..w[1]].join("\n")))
        .collect::<Vec<_>>()
        .join("\n")
}

fn compact_js_one(text: &str) -> String {
    let mut frames: Vec<(bool, String)> = Vec::new();
    let mut header = String::new();
    // What follows the last frame is the OUTCOME, and it was being dropped
    // in silence — no count, no banner, no recovery hash. Measured
    // 2026-09-02 on `node -e 'require("nosuch_xyz")'`: 262 tokens became
    // 191 and what left was `code: 'MODULE_NOT_FOUND'`, the require stack
    // and the runtime version, which is the half of a Node error an agent
    // greps for. `compact_python` a few functions up has kept a tail since
    // it was written, for exactly this reason; this branch never did.
    //
    // Unlike Python's, the rule here keeps INDENTED lines too: Node prints
    // its error object nested two spaces under the throwing frame, so the
    // Python rule ("no leading space") would drop the very field this
    // fixes. `compact_rust`, `compact_java` and `compact_go` share the
    // missing-tail shape, and are left alone deliberately: on real output
    // from this machine none of the three reaches this compactor at all
    // (`cargo run` on a real panic and `go run` on a real one both come
    // back byte-identical), so changing them would be a guess.
    let mut tail = String::new();
    let jsf = re_js_frame();
    for line in text.lines() {
        if jsf.is_match(line) {
            let path = jsf
                .captures(line)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str())
                .unwrap_or("");
            let is_user = !is_js_framework(path);
            frames.push((is_user, line.trim().to_string()));
        } else if frames.is_empty() {
            header.push_str(line);
            header.push('\n');
        } else {
            // Blank lines included: Node puts one before `Node.js vX`,
            // and a filter for them was a clause no fixture could tell
            // apart from its absence. `trim_end` below removes the
            // trailing ones, which is the only case that mattered.
            tail.push_str(line);
            tail.push('\n');
        }
    }
    if frames.is_empty() {
        return generic_compact(text);
    }
    let mut out = header.trim_end().to_string();
    out.push('\n');
    out.push_str(&fold(frames).join("\n"));
    if !tail.trim().is_empty() {
        out.push('\n');
        out.push_str(tail.trim_end());
    }
    out
}

pub(crate) fn compact_java(text: &str) -> String {
    per_trace(text, compact_java_one)
}

fn compact_java_one(text: &str) -> String {
    let mut frames: Vec<(bool, String)> = Vec::new();
    let mut header = String::new();
    let jf = re_java_frame();
    for line in text.lines() {
        if let Some(cap) = jf.captures(line) {
            let fqcn = cap.get(1).unwrap().as_str();
            let is_user = !is_java_framework(fqcn);
            frames.push((is_user, line.trim().to_string()));
        } else if frames.is_empty() {
            header.push_str(line);
            header.push('\n');
        } else if !line.trim().is_empty() {
            // `Caused by: java.io.IOException: disk` — the CAUSE, often the
            // actual diagnosis — and `... 23 more` were dropped: every line
            // after the first frame that was not a frame went (2026-10-03).
            // Kept in position, never folded with the framework frames.
            frames.push((true, line.trim().to_string()));
        }
    }
    if frames.is_empty() {
        return generic_compact(text);
    }
    let mut out = header.trim_end().to_string();
    out.push('\n');
    out.push_str(&fold(frames).join("\n"));
    out
}

/// A Go panic prints EVERY goroutine, each under `goroutine N [state]:`.
/// Every line after the first frame that was not a frame was dropped — the
/// later goroutines' headers with it — so all their frames were filed under
/// the first goroutine: the deadlock a dump exists to show became one
/// impossible stack (2026-10-03). Such lines are now kept in position.
pub(crate) fn compact_go(text: &str) -> String {
    // Go: header `goroutine 1 [running]:` then alternating `pkg.func()` /
    // `\t/path/file.go:N +0x12`. We treat each pair as one frame.
    let lines: Vec<&str> = text.lines().collect();
    let mut frames: Vec<(bool, String)> = Vec::new();
    let mut header = String::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let next = lines.get(i + 1).copied().unwrap_or("");
        let looks_like_frame = !line.is_empty()
            && !line.starts_with('\t')
            && !line.starts_with("goroutine ")
            && next.starts_with('\t');
        if looks_like_frame {
            let combined = format!("{} {}", line.trim(), next.trim());
            let is_user = !is_go_framework(&combined);
            frames.push((is_user, combined));
            i += 2;
            continue;
        }
        if frames.is_empty() {
            header.push_str(line);
            header.push('\n');
        } else if !line.trim().is_empty() {
            // `exit status 2` and the like: kept in position, not dropped.
            frames.push((true, line.trim().to_string()));
        }
        i += 1;
    }
    if frames.is_empty() {
        return generic_compact(text);
    }
    let mut out = header.trim_end().to_string();
    out.push('\n');
    out.push_str(&fold(frames).join("\n"));
    out
}

pub(crate) fn generic_compact(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let max = 30;
    if lines.len() <= max {
        return text.trim_end().to_string();
    }
    // Head 15 + tail 10 through the SHARED renderer, which writes the gap
    // marker AND the recovery footer and books the fold. This wrote its own
    // marker and stopped there: measured 2026-10-02 on a real nextest log
    // (`grep -A4 panicked … | head -40`), 15 lines delivered as `… [15 lines
    // elided by tokenade] …` with no line saying where to get them back.
    let kept: Vec<usize> = (0..15).chain(lines.len() - 10..lines.len()).collect();
    crate::elision::render_kept(&lines, &kept)
}

#[cfg(test)]
mod two_traces_are_two {
    /// Two panicking threads stay apart, and each frame keeps its location.
    #[test]
    fn rust_panics_stay_apart_and_keep_locations() {
        let text = "thread 'main' panicked at src/main.rs:5:9:\nboom\nstack backtrace:\n   0: 0x55d1 - app::run\n             at ./src/main.rs:5:9\nthread 'worker' panicked at src/worker.rs:12:3:\ndisk full\nstack backtrace:\n   0: 0x55e2 - app::worker::flush\n             at ./src/worker.rs:12:3\n";
        let out = super::compact_rust(text);
        let w = out.find("thread 'worker' panicked").expect(&out);
        assert!(
            out.find("app::run  at ./src/main.rs:5:9").expect(&out) < w,
            "{out}"
        );
        assert!(
            out.find("app::worker::flush  at ./src/worker.rs:12:3")
                .expect(&out)
                > w,
            "{out}"
        );
    }

    /// A goroutine dump: each goroutine keeps its own header and frames.
    #[test]
    fn go_goroutines_stay_apart() {
        let text = "panic: send on closed channel\n\ngoroutine 1 [running]:\nmain.produce()\n\t/srv/app/main.go:21 +0x45\nmain.main()\n\t/srv/app/main.go:9 +0x25\n\ngoroutine 18 [chan receive]:\nmain.consume()\n\t/srv/app/worker.go:33 +0x10\ncreated by main.main\n\t/srv/app/main.go:8 +0x1d\nexit status 2\n";
        let out = super::compact_go(text);
        let g18 = out.find("goroutine 18 [chan receive]:").expect(&out);
        assert!(
            out.find("main.go:21").unwrap() < g18 && out.find("worker.go:33").unwrap() > g18,
            "goroutines merged:\n{out}"
        );
        assert!(out.contains("exit status 2"), "{out}");
    }

    /// Two JavaScript errors: each keeps its own frames, in order.
    #[test]
    fn js_errors_keep_their_own_frames() {
        let text = "TypeError: x is undefined\n    at applyDiscount (/srv/app/pricing.js:142:31)\n    at checkout (/srv/app/views.js:88:17)\nError: connect ECONNREFUSED\n    at Pool.connect (/srv/app/node_modules/pg-pool/index.js:45:11)\n    at charge (/srv/app/pay.js:61:9)\n";
        let out = super::compact_js(text);
        let e2 = out.find("Error: connect ECONNREFUSED").expect(&out);
        assert!(
            out.find("pricing.js").unwrap() < e2 && out.find("pay.js").unwrap() > e2,
            "frames pooled:\n{out}"
        );
    }

    /// Two Java traces split; a `Caused by:` stays with the trace it explains.
    #[test]
    fn java_traces_split_but_a_cause_stays() {
        let text = "java.lang.IllegalStateException: boom\n\tat com.shop.A.run(A.java:10)\nCaused by: java.io.IOException: disk\n\tat com.shop.B.read(B.java:20)\njava.net.SocketTimeoutException: slow\n\tat com.shop.C.call(C.java:30)\n";
        let out = super::compact_java(text);
        let cause = out.find("Caused by: java.io.IOException").expect(&out);
        let second = out.find("java.net.SocketTimeoutException").expect(&out);
        assert!(
            out.find("A.java:10").unwrap() < cause && cause < out.find("B.java:20").unwrap(),
            "{out}"
        );
        assert!(
            out.find("B.java:20").unwrap() < second && second < out.find("C.java:30").unwrap(),
            "{out}"
        );
    }
}

#[cfg(test)]
mod two_tracebacks_are_two {
    /// Two different failures stay two tracebacks, each with its own frames
    /// and its own exception — even indented, as a journal prints them.
    #[test]
    fn each_traceback_keeps_its_frames_and_exception() {
        let tb = |file: &str, line: u32, code: &str, exc: &str| {
            format!("  Traceback (most recent call last):\n    File \"/srv/{file}\", line {line}, in f\n      {code}\n  {exc}\n")
        };
        let text = format!(
            "{}{}",
            tb("pay.py", 61, "charge()", "TimeoutError: slow"),
            tb("pricing.py", 142, "rate()", "AttributeError: none")
        );
        let out = super::compact_python(&text);
        let a = out.find("pay.py:61").expect(&out);
        let ea = out.find("TimeoutError: slow").expect(&out);
        let b = out.find("pricing.py:142").expect(&out);
        let eb = out.find("AttributeError: none").expect(&out);
        assert!(
            a < ea && ea < b && b < eb,
            "frames pooled across tracebacks:\n{out}"
        );
        assert_eq!(
            out.matches("Traceback (most recent call last)").count(),
            2,
            "{out}"
        );
    }
}
