//! `strace` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

pub(crate) fn synth_strace(n_lines: usize, error_every: usize) -> String {
    let syscalls = ["read", "write", "openat", "close", "fstat", "mmap", "brk"];
    let mut s = String::with_capacity(n_lines * 80);
    for i in 0..n_lines {
        let sc = syscalls[i % syscalls.len()];
        if error_every > 0 && i % error_every == 0 {
            s.push_str(&format!(
                "[pid 1234] 12:34:56.{i:06} {sc}(AT_FDCWD, \"/missing/file_{i}\", O_RDONLY) = -1 ENOENT (No such file or directory)\n"
            ));
        } else {
            s.push_str(&format!(
                "[pid 1234] 12:34:56.{i:06} {sc}(3, \"data\"..., 4096) = 4096\n"
            ));
        }
    }
    s.push_str("[pid 1234] +++ exited with 0 +++\n");
    s
}

pub(crate) fn task_strace_real(results: &mut Vec<RunResult>) {
    let raw = synth_strace(5_000, 100); // 5k lines, 50 errors
    let (out, sav, ms) = measure(&raw, strace::compact_with_savings);
    let quality = if out.contains("syscalls (7 unique)")
        && out.contains("50 errors")
        && out.contains("openat → ENOENT")
        && out.contains("+++ exited with 0 +++")
        && sav.percent() > 90.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:strace-compact",
        "strace",
        "synth-5k-50err",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #51: 200k strace lines — single syscall body would balloon to
// ~15 MB. Must compact >99 % in under 2 s, all errors counted.
pub(crate) fn adv_strace_huge(results: &mut Vec<RunResult>) {
    let raw = synth_strace(200_000, 500); // 400 errors
    let (out, sav, ms) = measure(&raw, strace::compact_with_savings);
    let quality = if out.contains("200000 syscalls")
        && out.contains("400 errors")
        && sav.percent() > 99.0
        && ms < 2000
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:strace:huge-200k",
        "strace",
        "adversarial",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #52: malformed/poisoned input. Mixes:
//   - real syscall lines
//   - quoted string args containing `=`, `(`, `)` (must not break parsing)
//   - unfinished/resumed continuations (must count, skip detail)
//   - non-trace garbage interspersed (must NOT be counted as syscalls)
//   - a line where the syscall arg is itself a path with `= -1 ` text
//     (must not be falsely classified as an error).
pub(crate) fn adv_strace_hostile(results: &mut Vec<RunResult>) {
    let raw = concat!(
        "execve(\"/usr/bin/cat\", [\"cat\", \"/etc/passwd\"], 0x7ffd) = 0\n",
        "[pid 100] 12:00:00 read(3, \"foo=bar&baz=(1,2,3)\", 18) = 18\n",
        "[pid 100] 12:00:01 nanosleep({tv_sec=1, tv_nsec=0},  <unfinished ...>\n",
        "[pid 100] 12:00:02 <... nanosleep resumed> 0x7ffd) = 0\n",
        "this is plain prose with no `(` `)` `=` so should be skipped\n",
        "let x = 42;  // looks like an assignment, no parens-equals shape\n",
        "openat(AT_FDCWD, \"/path/with = -1 in name\", O_RDONLY) = 5\n",
        "openat(AT_FDCWD, \"/really/missing\", O_RDONLY) = -1 ENOENT (No such file or directory)\n",
        "+++ exited with 0 +++\n",
    );
    let (out, _sav, ms) = measure(raw, strace::compact_with_savings);
    let quality = if out.contains("execve")
        && out.contains("read")
        && out.contains("openat")
        && out.contains("openat → ENOENT")
        && out.contains("unfinished/resumed")
        && out.contains("+++ exited with 0 +++")
        // The path-with-=−1 line returned 5 (success), so total errors == 1, not 2.
        && out.contains("1 errors")
        // Plain prose / Rust assignment must NOT add to syscall counts.
        && !out.contains("\nlet ")
        && !out.contains("plain prose")
        && ms < 100
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:strace:hostile",
        "strace",
        "adversarial",
        raw,
        &out,
        ms,
        quality,
    );
}
