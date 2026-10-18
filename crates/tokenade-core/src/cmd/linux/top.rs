//! `top -bn1` / `top -bn 1 -n 1` — batch-mode single-shot snapshot.
//!
//! ```text
//! top - 16:42:18 up  3:14, 2 users, load average: 0.50, 0.42, 0.38
//! Tasks: 412 total,   2 running, 410 sleeping,   0 stopped,   0 zombie
//! %Cpu(s):  3.2 us,  1.4 sy,  0.0 ni, 94.9 id,  0.4 wa,  0.0 hi,  0.1 si,  0.0 st
//! MiB Mem :  31984.7 total,   1234.5 free,  21456.3 used,   9293.9 buff/cache
//! MiB Swap:  16384.0 total,  16000.0 free,    384.0 used.   9123.4 avail Mem
//!
//!     PID USER      PR  NI    VIRT    RES    SHR S  %CPU  %MEM     TIME+ COMMAND
//!    1234 root      20   0   12345    678    234 R   5.0   0.0  12:34.56 systemd
//!    ...
//! ```
//!
//! Compaction: keep header (5 lines summary) + top 30 processes by
//! %CPU. Drop the long tail.

use std::fmt::Write;

/// A `top` row whose STATE column is one of the states that cost nothing and
/// mean everything. Shares its rule with the `ps` fold: `Z` zombie, `D`
/// uninterruptible sleep, `T`/`t` stopped or traced.
///
/// The state is the single letter just before `%CPU`, which in `top`'s layout
/// is the 8th whitespace-separated field.
fn row_state_needs_attention(row: &str) -> bool {
    row.split_whitespace()
        .nth(7)
        .and_then(|f| f.chars().next())
        .is_some_and(|c| matches!(c, 'Z' | 'D' | 'T' | 't' | 'X'))
}

/// Bound on how many such rows are rescued from the tail — a box with a
/// thousand zombies must not reprint them all.
const MAX_SICK_ROWS_RESCUED: usize = 10;

/// `%MEM` at which a row past the %CPU head is still shown — the `ps` fold's
/// threshold (`ps_table::HOT_THRESHOLD`).
const MEM_PCT_RESCUED: f64 = 1.0;
/// Bound on the memory rescue, as on the state rescue.
const MAX_MEM_ROWS_RESCUED: usize = 20;

/// The `%MEM` field (10th), read in either decimal mark (`1,8` under `fr_FR`).
fn row_mem_pct(row: &str) -> f64 {
    row.split_whitespace()
        .nth(9)
        .and_then(|f| f.replace(',', ".").parse().ok())
        .unwrap_or(0.0)
}

pub fn detect(text: &str) -> bool {
    crate::archetypes::any_output(text)
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    // `top` is routed here by NAME on every platform, and macOS prints its
    // own layout (`Processes:` / `PhysMem:`, 30 columns, PID order). Read as
    // this one, the Mac's 252 rows were cut to the 30 newest PIDs; the module
    // written for that layout was never reached (measured 2026-10-03).
    if super::super::macos::top::detect(text) {
        return super::super::macos::top::compact(text);
    }
    let mut header: Vec<&str> = Vec::new();
    let mut process_header: Option<&str> = None;
    let mut processes: Vec<&str> = Vec::new();
    let mut seen_proc_header = false;
    for line in text.lines() {
        let t = line.trim_start();
        if !seen_proc_header && t.starts_with("PID") {
            process_header = Some(line);
            seen_proc_header = true;
            continue;
        }
        if seen_proc_header {
            if !line.trim().is_empty() {
                processes.push(line);
            }
        } else {
            header.push(line);
        }
    }
    if processes.len() <= 30 {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() / 2);
    for h in &header {
        out.push_str(h);
        out.push('\n');
    }
    if let Some(ph) = process_header {
        out.push_str(ph);
        out.push('\n');
    }
    // `top` emits rows already sorted by %CPU descending, so keeping the head
    // keeps whatever is burning the machine — and drops, by construction, the
    // states that burn nothing: a zombie sits at the very bottom at 0.0 %, and
    // a process wedged in uninterruptible sleep with it. Those are what someone
    // reading `top` after "the box is unhappy" is looking for, and the header
    // even counts them (`1 zombie`) while the rows that would name them are cut.
    // MEASURED: `stuck <defunct>` never reached the reader.
    let mut kept: Vec<&str> = processes.iter().take(30).copied().collect();
    kept.extend(
        processes
            .iter()
            .skip(30)
            .filter(|p| row_state_needs_attention(p))
            .take(MAX_SICK_ROWS_RESCUED)
            .copied(),
    );
    // The %CPU order is blind to MEMORY: an idle browser holding 1 GB sits at
    // 0.0 % CPU and was cut. Measured 2026-10-03 on a real 615-task snapshot:
    // 8 of the 10 largest by %MEM (Chrome tabs, a terminal, a dashboard)
    // gone, while `ps aux` of the same machine kept them — its fold surfaces
    // every row at 1 % or more of CPU OR memory. Same bar here.
    kept.extend(
        processes
            .iter()
            .skip(30)
            .filter(|p| !row_state_needs_attention(p) && row_mem_pct(p) >= MEM_PCT_RESCUED)
            .take(MAX_MEM_ROWS_RESCUED)
            .copied(),
    );
    for p in &kept {
        out.push_str(p);
        out.push('\n');
    }
    let _ = writeln!(
        out,
        "  [... {} more process(es) elided — sorted by %CPU desc, top 30 kept plus any in a state that needs attention or at 1 %MEM or more]",
        processes.len() - kept.len()
    );
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(n: usize) -> String {
        let mut s = String::from("top - 16:42:18 up 3:14, 2 users, load average: 0.50, 0.42, 0.38\n");
        s.push_str("Tasks: 412 total, 2 running, 410 sleeping, 0 stopped, 0 zombie\n");
        s.push_str("%Cpu(s): 3.2 us, 1.4 sy, 0.0 ni, 94.9 id\n");
        s.push_str("MiB Mem : 31984.7 total, 1234.5 free, 21456.3 used\n");
        s.push('\n');
        s.push_str("    PID USER      PR  NI    VIRT    RES    SHR S  %CPU  %MEM     TIME+ COMMAND\n");
        for i in 0..n {
            let _ = writeln!(
                s,
                "  {pid:>5} root      20   0   12345    678    234 R   {cpu:>4.1}   0.0  12:34.56 proc_{i}",
                pid = 1000 + i,
                cpu = (n - i) as f64 / 10.0,
            );
        }
        s
    }

    #[test]
    fn detects_top_batch() {
        assert!(detect(&sample(5)));
    }

    #[test]
    fn keeps_short_list() {
        let s = sample(20);
        assert_eq!(compact(&s), s);
    }

    #[test]
    fn folds_long_list() {
        let s = sample(100);
        let out = compact(&s);
        assert!(out.contains("70 more"));
    }

    #[test]
    fn rejects_empty() {
        // Detector was widened so the macOS `top -l 1` and Linux
        // batched `top -b -n 1` outputs both pass; only truly empty
        // strings are rejected now. Discrimination against arbitrary
        // prose has moved to dispatch_named (callers must know the
        // program name before reaching this compactor).
        assert!(!detect(""));
        assert!(!detect("   \n  "));
    }

    /// An idle process holding memory sits at 0 % CPU, below the %CPU head:
    /// it is kept like `ps aux` keeps it (≥ 1 %MEM), whatever the decimal mark.
    #[test]
    fn a_memory_hog_at_zero_cpu_survives_the_cpu_head() {
        let mut t = String::from("top - 04:10:05 up 20 days\n\n    PID UTIL.     PR  NI    VIRT    RES    SHR S  %CPU  %MEM     TEMPS+ COM.\n");
        for i in 0..40 {
            t.push_str(&format!("{:>7} root      20   0   2680   1776   1660 S   0,0   0,0   0:00.01 d{i}\n", 1000 + i));
        }
        t.push_str("1600968 infected  20   0 1186.5g   1,1g 300000 S   0,0   1,8   2:10.03 chrome\n");
        let out = compact(&t);
        assert!(out.contains("1600968 infected"), "{out}");
        assert!(!out.contains(" d39\n"), "{out}");
    }
}
