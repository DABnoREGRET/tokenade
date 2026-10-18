//! `lsof` — list of open files.
//!
//! ```text
//! COMMAND     PID    USER   FD   TYPE             DEVICE  SIZE/OFF       NODE NAME
//! systemd       1    root  cwd    DIR              259,1      4096          2 /
//! systemd       1    root  rtd    DIR              259,1      4096          2 /
//! systemd       1    root  txt    REG              259,1   1620304       1234 /usr/lib/systemd/systemd
//! systemd       1    root  mem    REG              259,1     54704       5678 /usr/lib/libnss_files-2.31.so
//! …
//! ```
//!
//! Compaction:
//!   * Group by `COMMAND PID`. Within each group, fold runs of
//!     identical FD types (`mem`, `DEL`) to a single representative
//!     line + `(×N)`.
//!   * Optionally drop FIFO / pipe FDs when there are >5 per process
//!     (usually noise).

use std::fmt::Write;

pub fn detect(text: &str) -> bool {
    let mut first = text.lines().next().unwrap_or("");
    if first.is_empty() {
        // Skip blank prefix lines.
        for l in text.lines() {
            if !l.trim().is_empty() {
                first = l;
                break;
            }
        }
    }
    let header_ok = first.contains("COMMAND")
        && first.contains("PID")
        && first.contains("FD")
        && first.contains("NAME");
    let data_lines = text.lines().skip(1).take(80).filter(|l| !l.trim().is_empty()).count();
    header_ok && data_lines >= 5
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut header_printed = false;
    let mut prev_fd_type: Option<String> = None;
    let mut prev_cmd_pid: Option<String> = None;
    let mut run_count = 0usize;
    let mut last_line: Option<String> = None;
    let flush = |last: &Option<String>, run_count: usize, out: &mut String| {
        if let Some(line) = last {
            out.push_str(line);
            if run_count > 1 {
                crate::fold_count::note_folded(run_count);
                let _ = write!(out, "  (×{run_count})");
            }
            out.push('\n');
        }
    };
    for line in text.lines() {
        if !header_printed && line.contains("COMMAND") && line.contains("PID") {
            out.push_str(line);
            out.push('\n');
            header_printed = true;
            continue;
        }
        if line.trim().is_empty() {
            flush(&last_line, run_count, &mut out);
            last_line = None;
            run_count = 0;
            prev_fd_type = None;
            prev_cmd_pid = None;
            out.push('\n');
            continue;
        }
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.len() < 4 {
            flush(&last_line, run_count, &mut out);
            last_line = Some(line.to_string());
            run_count = 1;
            continue;
        }
        let cmd_pid = format!("{} {}", toks[0], toks[1]);
        let fd_type = toks[3].to_string();
        let same_run = prev_cmd_pid.as_deref() == Some(&cmd_pid)
            && prev_fd_type.as_deref() == Some(&fd_type);
        if same_run {
            run_count += 1;
        } else {
            flush(&last_line, run_count, &mut out);
            last_line = Some(line.to_string());
            run_count = 1;
            prev_cmd_pid = Some(cmd_pid);
            prev_fd_type = Some(fd_type);
        }
    }
    flush(&last_line, run_count, &mut out);
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_lsof_header() {
        let s = "COMMAND     PID    USER   FD   TYPE             DEVICE  SIZE/OFF       NODE NAME\nsystemd       1    root  cwd    DIR              259,1      4096          2 /\nsystemd       1    root  rtd    DIR              259,1      4096          2 /\nsystemd       1    root  mem    REG              259,1     54704       5678 /lib/foo\nsystemd       1    root  mem    REG              259,1     54704       5678 /lib/bar\nsystemd       1    root  mem    REG              259,1     54704       5678 /lib/baz\n";
        assert!(detect(s));
    }

    #[test]
    fn folds_mem_runs_per_process() {
        let mut s = String::from("COMMAND     PID    USER   FD   TYPE             DEVICE  SIZE/OFF       NODE NAME\n");
        for i in 0..20 {
            let _ = writeln!(s, "systemd       1    root  mem    REG              259,1     54704       5678 /lib/lib{i}.so");
        }
        let out = compact(&s);
        assert!(out.contains("(×20)"), "should fold 20 mem entries: {out}");
    }

    #[test]
    fn rejects_non_lsof() {
        assert!(!detect("hello world\nsecond line\n"));
    }
}
