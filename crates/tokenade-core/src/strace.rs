//! `strace` / `ltrace` output compaction.
//!
//! A real Linux `strace -f cargo build` or `strace -p $(pidof app)` can emit
//! 10⁵ – 10⁷ lines, one per syscall, on a single line each:
//!
//! ```text
//! [pid 1234] 12:34:56.789123 openat(AT_FDCWD, "/etc/passwd", O_RDONLY) = 3
//! [pid 1234] 12:34:56.789200 read(3, "root:x:0:0:..."..., 4096) = 1234
//! [pid 1234] 12:34:56.789250 close(3) = 0
//! …100 000 more lines…
//! [pid 1234] 12:34:57.123 +++ exited with 0 +++
//! ```
//!
//! The agent reading this almost never needs the per-syscall body — it wants:
//!   * the *distribution* of syscalls (which ones happened, how often)
//!   * every *error* (`= -1 ENAME …`) verbatim — those are the diagnostic gold
//!   * `+++ exited` / `--- SIG…` markers
//!
//! [`compact`] returns exactly that. Real-world ratio: ≥99 % on multi-MB
//! traces.
//!
//! Detection is pattern-based: a line is a strace line iff, after stripping
//! the optional `[pid N]` prefix and the optional `HH:MM:SS[.…]` timestamp,
//! it matches `identifier(…) = retval`. We require ≥5 such lines (or the
//! unambiguous `+++ exited with` / `--- SIG` / `strace: ` markers) to avoid
//! false-positives on code listings.

use std::collections::HashMap;
use std::fmt::Write;

/// Cap on individual error lines preserved verbatim.
pub const ERRORS_CAP: usize = 200;
/// Cap on per-syscall rows shown in the count table.
pub const SYSCALLS_CAP: usize = 100;
/// Cap on signal / exit lines preserved.
pub const SIGNALS_CAP: usize = 50;

pub fn detect(text: &str) -> bool {
    let head: &str = text.get(..text.len().min(8192)).unwrap_or(text);

    // Strong, unambiguous markers.
    if head.contains("+++ exited with")
        || head.contains("+++ killed by")
        || head.contains("\n--- SIG")
        || head.starts_with("--- SIG")
        || head.contains("\nstrace: ")
        || head.starts_with("strace: ")
        || head.contains("\nltrace: ")
        || head.starts_with("ltrace: ")
    {
        return true;
    }

    // Pattern match: at least 5 lines that look like syscall traces.
    head.lines()
        .take(80)
        .filter(|l| looks_like_strace_line(l))
        .nth(4)
        .is_some()
}

fn strip_optional_pid(s: &str) -> &str {
    let s = s.trim_start();
    if let Some(rest) = s.strip_prefix('[') {
        // [pid 12345]  or  [pid 12345, 67890]
        if let Some(close) = rest.find(']') {
            let after = &rest[close + 1..];
            return after.trim_start();
        }
    }
    s
}

fn strip_optional_time(s: &str) -> &str {
    // Match either HH:MM:SS[.NNNN]  or  epoch.NNNN.
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b':' || bytes[i] == b'.') {
        i += 1;
    }
    if i > 0 && i < bytes.len() && bytes[i] == b' ' {
        // Require at least one ':' or '.' to look time-like (vs e.g. a syscall arg `123`).
        let head = &s[..i];
        if head.contains(':') || head.contains('.') {
            return s[i + 1..].trim_start();
        }
    }
    s
}

fn looks_like_strace_line(line: &str) -> bool {
    let s = strip_optional_pid(line);
    let s = strip_optional_time(s);
    // Must start with an identifier (syscall name).
    let mut chars = s.chars();
    let first = chars.next();
    if !matches!(first, Some(c) if c.is_ascii_alphabetic() || c == '_') {
        return false;
    }
    let paren_idx = match s.find('(') {
        Some(i) => i,
        None => return false,
    };
    let id = &s[..paren_idx];
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return false;
    }
    // The closing-paren-then-equals fingerprint is unique to strace lines
    // (Rust/C/Python rarely emit ` ) = `).
    s.contains(") = ") || s.contains(") =?") // ENOSYS shows `=?`
}

pub fn compact(text: &str) -> String {
    if !detect(text) {
        return text.to_string();
    }

    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut error_counts: HashMap<String, usize> = HashMap::new();
    let mut errors_kept: Vec<String> = Vec::new();
    let mut signal_lines: Vec<String> = Vec::new();
    let mut total_syscalls = 0usize;
    let mut total_errors = 0usize;
    let mut unfinished = 0usize;
    let mut unmatched: Vec<String> = Vec::new();

    for raw_line in text.lines() {
        let line = raw_line.trim_end();
        if line.is_empty() {
            continue;
        }

        // Special markers — keep verbatim (capped).
        if line.contains("+++ exited")
            || line.contains("+++ killed by")
            || line.contains("--- SIG")
            || line.trim_start().starts_with("strace: ")
            || line.trim_start().starts_with("ltrace: ")
        {
            if signal_lines.len() < SIGNALS_CAP {
                signal_lines.push(line.to_string());
            }
            continue;
        }

        // strace unfinished/resumed continuations — count but skip detail.
        if line.contains("<unfinished ...>") || line.contains("<... ") {
            unfinished += 1;
            continue;
        }

        let s = strip_optional_pid(line);
        let s = strip_optional_time(s);

        // Lines that are NOT syscall records are the traced program's OWN
        // output (`strace ./app 2>&1`) — they used to be silently destroyed.
        let syscall_shape = s.split_once('(').is_some_and(|(id, _)| {
            !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        }) && (s.contains(") = ") || s.contains(") =?"));
        if !syscall_shape {
            unmatched.push(line.to_string());
            continue;
        }
        let Some((id, _)) = s.split_once('(') else {
            continue;
        };

        total_syscalls += 1;
        *counts.entry(id.to_string()).or_insert(0) += 1;

        // Errors: ` = -1 ENAME (…)`.
        if let Some(eq_idx) = s.find(") = -1 ") {
            total_errors += 1;
            // Capture the error name (ENOENT, EACCES, ...).
            let after = &s[eq_idx + ") = -1 ".len()..];
            let ename: String = after
                .chars()
                .take_while(|c| c.is_ascii_uppercase() || *c == '_' || c.is_ascii_digit())
                .collect();
            if !ename.is_empty() {
                let key = format!("{id} → {ename}");
                *error_counts.entry(key).or_insert(0) += 1;
            }
            if errors_kept.len() < ERRORS_CAP {
                errors_kept.push(line.to_string());
            }
        }
    }

    let unique = counts.len();

    let mut out = String::with_capacity(4096);
    let _ = writeln!(out, "{total_syscalls} syscalls ({unique} unique)");
    if unfinished > 0 {
        let _ = writeln!(
            out,
            "  ({unfinished} unfinished/resumed continuations skipped)"
        );
    }

    // Syscall count table — most frequent first.
    let mut sorted: Vec<_> = counts.iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    if !sorted.is_empty() {
        out.push_str("\nsyscalls by count:\n");
        for (name, count) in sorted.iter().take(SYSCALLS_CAP) {
            let _ = writeln!(out, "  {name:<24}{count:>10}");
        }
        if sorted.len() > SYSCALLS_CAP {
            let _ = writeln!(
                out,
                "  … {} more syscall type(s) elided",
                sorted.len() - SYSCALLS_CAP
            );
        }
    }

    // The syscall LINES themselves — every path, every fd, every argument.
    //
    // Three blocks in this function end with `… N more … elided`: the syscall
    // type table, the error-kind table and the verbatim error section. All
    // three disclose the cap on a list they DO render. Nothing discloses the
    // one list that is never rendered at all, which is the successful calls —
    // and an agent reads `strace` to learn WHICH files a process opened.
    //
    // Measured 2026-08-27 by substituting one field's value on a middle
    // line across the 29 fuzz generators: this compactor answers
    // `40 syscalls (1 unique)` plus a tally, 703 tokens to 31, and no part of
    // that output says a line went away. One token above the flat 30-token
    // ceiling of `reduction_is_pathological`, so that floor does not catch
    // it either — this is the case the "NEAR MISS above flat ceiling"
    // telemetry in `filter.rs` was written to hand over, and it says as much:
    // "one of these two things is true: it holds and this line never fires, or
    // it does not and this line hands over the case that proves it".
    //
    // The fix is disclosure, not restoration: the drop is the design (a
    // hundred thousand syscall lines is what strace produces and why this
    // compactor exists), and the drop being invisible is not.
    let verbatim = errors_kept.len();
    if total_syscalls > verbatim {
        let _ = writeln!(
            out,
            "\n… {} syscall line(s) elided; counted by type above, \
             arguments not shown",
            total_syscalls - verbatim
        );
    }

    // Error summary by (syscall, ENAME).
    if !error_counts.is_empty() {
        let mut by_kind: Vec<_> = error_counts.iter().collect();
        by_kind.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        out.push_str("\nerrors by (syscall → errno):\n");
        for (k, n) in by_kind.iter().take(SYSCALLS_CAP) {
            let _ = writeln!(out, "  {k:<40}{n:>8}");
        }
        if by_kind.len() > SYSCALLS_CAP {
            let _ = writeln!(
                out,
                "  … {} more error kind(s) elided",
                by_kind.len() - SYSCALLS_CAP
            );
        }
    }

    // First ERRORS_CAP errors verbatim.
    crate::archetypes::capped_section_with_header(
        &mut out,
        &format!("errors ({} shown verbatim", errors_kept.len()),
        &errors_kept,
        total_errors,
        "error line(s)",
    );

    if !signal_lines.is_empty() {
        out.push_str("\nsignals & exit:\n");
        for s in &signal_lines {
            out.push_str("  ");
            out.push_str(s);
            out.push('\n');
        }
    }

    if !unmatched.is_empty() {
        // The traced program's own stdout — kept, one more compaction pass.
        out.push('\n');
        out.push_str(&crate::auto::compact_remainder(&unmatched.join("\n")));
    }
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"execve("/usr/bin/cat", ["cat", "/etc/passwd"], 0x7ffd) = 0
brk(NULL)                               = 0x55a000
openat(AT_FDCWD, "/etc/ld.so.cache", O_RDONLY|O_CLOEXEC) = 3
fstat(3, {st_mode=S_IFREG|0644, st_size=12345, ...}) = 0
close(3)                                = 0
openat(AT_FDCWD, "/lib/libc.so.6", O_RDONLY|O_CLOEXEC) = 3
read(3, "\177ELF\2\1\1\0..."..., 832)   = 832
close(3)                                = 0
openat(AT_FDCWD, "/etc/passwd", O_RDONLY) = 3
read(3, "root:x:0:0:root:/root:/bin/bash\n"..., 4096) = 1234
openat(AT_FDCWD, "/missing/file", O_RDONLY) = -1 ENOENT (No such file or directory)
openat(AT_FDCWD, "/etc/shadow", O_RDONLY) = -1 EACCES (Permission denied)
write(1, "root:x:0:0:..."..., 1234)     = 1234
close(3)                                = 0
exit_group(0)                           = ?
+++ exited with 0 +++
"#;

    #[test]
    fn detects_real_trace() {
        assert!(detect(SAMPLE));
    }

    #[test]
    fn detects_with_pid_prefix() {
        let s = "[pid 1234] read(3, \"x\", 1) = 1\n[pid 1234] +++ exited with 0 +++\n";
        assert!(detect(s));
    }

    #[test]
    fn rejects_rust_function_call() {
        assert!(!detect("let v = vec![1, 2, 3];\nlet n = v.len();\n"));
    }

    #[test]
    fn rejects_plain_prose() {
        assert!(!detect(
            "When you run strace you get a lot of output. It's verbose.\n"
        ));
    }

    #[test]
    fn extracts_counts_and_errors() {
        let out = compact(SAMPLE);
        assert!(out.contains("syscalls"), "summary missing: {out}");
        // openat appears 4 times (success + success + 2 errors).
        assert!(out.contains("openat"), "no openat: {out}");
        // Error summary should mention ENOENT + EACCES.
        assert!(out.contains("openat → ENOENT"), "no ENOENT: {out}");
        assert!(out.contains("openat → EACCES"), "no EACCES: {out}");
        // Verbatim errors preserved.
        assert!(out.contains("/missing/file"));
        assert!(out.contains("/etc/shadow"));
        // Exit marker preserved.
        assert!(out.contains("+++ exited with 0 +++"));
    }

    #[test]
    fn unknown_passes_through() {
        let t = "this is not a trace";
        assert_eq!(compact(t), t);
    }

    #[test]
    fn savings_large_trace() {
        // 50 000-line synthetic trace.
        let mut s = String::new();
        for i in 0..50_000 {
            let _ = writeln!(
                s,
                "[pid 1234] 12:34:56.789{i:03} read(3, \"xxxxxxxx\"..., 4096) = 4096"
            );
        }
        s.push_str("[pid 1234] +++ exited with 0 +++\n");
        let (out, sav) = compact_with_savings(&s);
        assert!(out.contains("50000 syscalls"));
        assert!(sav.percent() > 99.0, "got {sav:?}");
    }

    #[test]
    fn handles_equals_inside_string_arg() {
        // The `=` inside the quoted string MUST NOT confuse our `) = ` detector.
        let s = "read(3, \"key=value&foo=bar\", 17) = 17\n+++ exited with 0 +++\n";
        let out = compact(s);
        assert!(out.contains("read"), "should detect read syscall: {out}");
    }

    #[test]
    fn handles_unfinished_continuations() {
        let s = "[pid 1] nanosleep({tv_sec=10, tv_nsec=0},  <unfinished ...>\n[pid 1] <... nanosleep resumed> NULL) = 0\nread(3, \"x\", 1) = 1\n+++ exited with 0 +++\n";
        let out = compact(s);
        assert!(out.contains("unfinished/resumed"));
        assert!(out.contains("read"));
    }
}
