//! `strace` — strace.
//!
//! Unlike a git listing (most-recent-first, so the head carries the signal),
//! strace output is *end-weighted*: the syscall that actually mattered is
//! usually the failure right before the program died — i.e. in the TAIL. A
//! plain head-fold (`keep first N, "(+M more)"`) would silently drop it, so we
//! fold the boring tail but ALWAYS retain failed syscalls (`… = -1 ERRNO`)
//! wherever they sit, with a visible cap marker if there are very many.
use std::fmt::Write;

const HEAD: usize = 60;
/// Cap on failed-syscall lines retained from the tail (with a visible marker
/// when exceeded). Generous: the first failure is what usually matters, but we
/// never want to imply there were fewer errors than there were.
const TAIL_ERR_CAP: usize = 50;

pub fn detect(text: &str) -> bool {
    text.lines().any(|l| l.contains('(') && l.contains(')'))
}

/// A failed syscall line: strace renders errors as `… = -1 ENOENT (No such …)`.
/// We require the `= -1` return *and* a trailing uppercase errno token so we
/// don't match a syscall that legitimately returned `-1` as data without an
/// errno (rare, but be precise).
fn is_syscall_error(line: &str) -> bool {
    let Some(idx) = line.find("= -1") else { return false };
    let rest = line[idx + "= -1".len()..].trim_start();
    let errno = rest.split(|c: char| c.is_whitespace() || c == '(').next().unwrap_or("");
    !errno.is_empty()
        && errno.starts_with('E')
        && errno.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

/// Shared end-weighted fold for every strace *trace* mode (plain, `-f`, `-e`,
/// `-T`). Keeps the first `head` syscalls verbatim, folds the boring tail into
/// a count, but ALWAYS retains failed syscalls (`= -1 ERRNO`) from the tail —
/// the diagnostic that explains the crash. `None` when no fold is needed.
/// (`strace -c` is excluded: it's an aggregated stats table, not a trace.)
pub(crate) fn fold_trace(text: &str, head: usize, label: &str) -> Option<String> {
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim_end())
        .filter(|l| !l.is_empty())
        .collect();
    let total = lines.len();
    if total <= head {
        return None;
    }

    let mut out = format!("[tokenade:{label}] {total} syscall(s)\n");
    for l in &lines[..head] {
        out.push_str(l);
        out.push('\n');
    }

    // Partition the tail: fold the boring lines into a count, but never lose a
    // failed syscall — that's the answer to "why did it break?".
    let mut folded = 0usize;
    let mut tail_errors: Vec<&str> = Vec::new();
    for l in &lines[head..] {
        if is_syscall_error(l) {
            tail_errors.push(l);
        } else {
            folded += 1;
        }
    }
    let _ = write!(out, "  ... (+{folded} more)");

    if !tail_errors.is_empty() {
        let shown = tail_errors.len().min(TAIL_ERR_CAP);
        let _ = write!(out, "\n  [tokenade:{label}] {} failed syscall(s) retained from tail:", tail_errors.len());
        for e in &tail_errors[..shown] {
            let _ = write!(out, "\n{e}");
        }
        if tail_errors.len() > shown {
            let _ = write!(out, "\n  ... (+{} more failure(s))", tail_errors.len() - shown);
        }
    }
    Some(out)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    fold_trace(text, HEAD, "strace")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_valid() { assert!(detect("getpid() = 12345")); }
    #[test] fn short_output_passes_through() {
        let line = "getpid() = 12345";
        let s: String = std::iter::repeat_n(line, 3).map(|l| format!("{l}\n")).collect();
        assert!(try_compact(&s).is_none());
    }
    #[test] fn long_output_folds() {
        let line = "getpid() = 12345";
        let s: String = std::iter::repeat_n(line, 65).map(|l| format!("{l}\n")).collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("[tokenade:strace]"));
        assert!(out.contains("(+5 more)"));
    }

    #[test]
    fn is_syscall_error_recognises_failures() {
        assert!(is_syscall_error("openat(AT_FDCWD, \"/x\", O_RDONLY) = -1 ENOENT (No such file or directory)"));
        assert!(is_syscall_error("connect(3, ...) = -1 ECONNREFUSED (Connection refused)"));
        // success / data -1 without errno must NOT match
        assert!(!is_syscall_error("read(3, \"data\", 100) = 42"));
        assert!(!is_syscall_error("brk(NULL) = 0x55a000"));
    }

    #[test]
    fn tail_failure_is_never_folded_away() {
        // 100 benign syscalls, then the failure that actually crashed the
        // program — buried far past HEAD. It MUST survive verbatim.
        let mut s = String::new();
        for _ in 0..100 {
            s.push_str("getpid() = 12345\n");
        }
        s.push_str("openat(AT_FDCWD, \"/etc/missing.conf\", O_RDONLY) = -1 ENOENT (No such file or directory)\n");
        let out = try_compact(&s).unwrap();
        // The boring tail is folded (100 getpid - 60 head = 40 folded) …
        assert!(out.contains("(+40 more)"), "tail not folded: {out}");
        // … but the failing syscall is retained verbatim, errno and all.
        assert!(out.contains("/etc/missing.conf"), "failing path lost: {out}");
        assert!(out.contains("ENOENT"), "errno lost: {out}");
        assert!(out.contains("failed syscall(s) retained"), "no retention banner: {out}");
    }

    #[test]
    fn many_tail_failures_are_capped_with_marker() {
        let mut s = String::new();
        for _ in 0..HEAD + 5 {
            s.push_str("getpid() = 12345\n");
        }
        for i in 0..(TAIL_ERR_CAP + 7) {
            s.push_str(&format!("openat(AT_FDCWD, \"/x/{i}\", O_RDONLY) = -1 ENOENT (No such file or directory)\n"));
        }
        let out = try_compact(&s).unwrap();
        assert!(out.contains("more failure(s)"), "cap marker missing: {out}");
    }
}
