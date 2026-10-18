//! macOS `top -l1` — one-shot process snapshot.
//!
//! macOS top format (locale-invariant column names):
//! ```text
//! Processes: 428 total, 3 running, 425 sleeping, 1259 threads
//! 2026/03/07 04:51:42
//! Load Avg: 1.67, 1.35, 1.23
//! CPU usage: 36.0% user, 64.0% sys, 0.0% idle
//! SharedLibs: 227M resident, 33M data, 30M linkedit.
//! MemRegions: 59121 total, 879M resident, 65M private, 197M shared.
//! PhysMem: 4891M used (958M wired), 1036M unused.
//! VM: 14T vsize, 3090M framework vsize, 34312(0) swapins, 109906(0) swapouts.
//! Networks: packets: 603704/493M in, 30938613/19G out.
//! Disks: 5482038/191G read, 9296806/389G written.
//!
//! PID    COMMAND          %CPU TIME     #TH #WQ #PORTS MEM   PURG CMPRS PGRP  PPID  STATE ...
//! 68500  top              0.0  00:00.26 1/1 0   15     1448K 0B   0B    68500 68499 running ...
//! ```
//!
//! Compaction:
//! 1. Keep summary header (up to the blank line before the PID table).
//! 2. Process table: extract 6 essential columns (PID COMMAND %CPU TIME MEM STATE),
//!    dropping ~25 diagnostic columns.
//! 3. Whole when it fits; otherwise the TOP_N = 25 rows ranked by %CPU then MEM.
//!
//! Column indices after `split_whitespace` on macOS top rows (stable across macOS versions):
//!   0=PID  1=COMMAND  2=%CPU  3=TIME  4=#TH  5=#WQ  6=#PORTS  7=MEM  8=PURG
//!   9=CMPRS  10=PGRP  11=PPID  12=STATE  ...
//!
//! Quality contract (anti-regression):
//! - "Processes: N total, M running" line preserved
//! - Load Avg preserved
//! - PhysMem preserved
//! - PID value preserved for each shown row
//! - COMMAND name preserved
//! - %CPU value preserved
//! - MEM value preserved
//! - STATE value preserved
//! - When rows are elided, elision count is noted

use std::fmt::Write;
const TOP_N: usize = 25;

pub fn detect(text: &str) -> bool {
    // macOS `top -l1` always starts with "Processes: N total"
    // and contains "Load Avg:" and "PhysMem:" in the header.
    let t = text.trim_start();
    t.starts_with("Processes:") && text.contains("Load Avg:") && text.contains("PhysMem:")
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    // Every column while the snapshot fits: PPID, USER, threads and ports are
    // what the six-column projection drops, and it is only paid for when the
    // full width does not fit.
    if tk_base::delivery::fits(text) {
        return text.to_string();
    }
    let mut header_lines: Vec<&str> = Vec::new();
    let mut proc_rows: Vec<&str> = Vec::new();
    let mut found_pid_header = false;

    for line in text.lines() {
        let trimmed = line.trim_start();
        if !found_pid_header && trimmed.starts_with("PID") && trimmed.contains("COMMAND") {
            // Skip the wide original PID header — we emit our own compact one
            found_pid_header = true;
            continue;
        }
        if found_pid_header {
            if !line.trim().is_empty() {
                proc_rows.push(line);
            }
        } else {
            header_lines.push(line);
        }
    }

    let mut out = String::with_capacity(text.len() / 4);

    // Keep the summary header verbatim
    for h in &header_lines {
        out.push_str(h);
        out.push('\n');
    }

    if proc_rows.is_empty() {
        return out.trim_end().to_string();
    }

    // Compact process table
    out.push_str("\nPID       COMMAND           %CPU    TIME        MEM       STATE\n");

    // Each row as (cpu, mem bytes, rendered line).
    let mut rows: Vec<(f64, f64, String)> = Vec::with_capacity(proc_rows.len());
    for row in &proc_rows {
        let fields: Vec<&str> = row.split_whitespace().collect();
        // Need at least 13 fields: indices 0..12 (STATE at 12)
        if fields.len() < 13 {
            rows.push((0.0, 0.0, row.to_string()));
            continue;
        }
        let pid = fields[0];
        // COMMAND can be multi-word ("Google Chrome Helper") — fixed
        // left-indices then read %CPU/TIME/MEM/STATE from the WRONG columns
        // (STATE showed a PID). %CPU is the first decimal-shaped token after
        // the command words; anchor on it and shift the offsets.
        let cpu_idx = (2..fields.len())
            .find(|&i| fields[i].contains('.') && fields[i].parse::<f64>().is_ok())
            .unwrap_or(2);
        let extra = cpu_idx - 2;
        let cmd_owned = fields[1..=1 + extra].join(" ");
        let cmd = cmd_owned.as_str();
        let cpu = fields[cpu_idx];
        let time = fields[3 + extra];
        let mem = fields[7 + extra];
        let state = fields.get(12 + extra).copied().unwrap_or("");
        rows.push((
            cpu.parse().unwrap_or(0.0),
            mem_bytes(mem),
            format!("{pid:<10}{cmd:<18}{cpu:<8}{time:<12}{mem:<10}{state}"),
        ));
    }
    let whole: String = rows.iter().map(|r| format!("{}\n", r.2)).collect();
    if tk_base::delivery::fits(&format!("{out}{whole}")) {
        out.push_str(&whole);
        return out.trim_end().to_string();
    }
    // **`top -l1` is sorted by PID, newest first, and its one sample reads
    // 0.0 %CPU on every row** (macOS needs two samples to measure CPU). The
    // head kept the 25 most recently started processes: measured 2026-10-03
    // on a real 252-process Mac, WindowServer (85 MB), syspolicyd and
    // mds_stores — three of the six largest — were cut. Ranked by %CPU, then
    // by memory, before the cut.
    rows.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.total_cmp(&a.1)));
    for r in rows.iter().take(TOP_N) {
        out.push_str(&r.2);
        out.push('\n');
    }
    if rows.len() > TOP_N {
        let _ = writeln!(
            out,
            "  [... {} more process(es) elided — ranked by %CPU then MEM; run `top -l1` for the full list]",
            rows.len() - TOP_N
        );
    }

    out.trim_end().to_string()
}

/// `2465K`, `85M`, `1.2G`, `0B` — top's MEM column, in bytes.
fn mem_bytes(s: &str) -> f64 {
    let (num, unit) = s.split_at(s.len().saturating_sub(1));
    let mult = match unit {
        "B" => 1.0,
        "K" => 1024.0,
        "M" => 1024.0 * 1024.0,
        "G" => 1024.0 * 1024.0 * 1024.0,
        _ => return s.parse().unwrap_or(0.0),
    };
    num.trim_end_matches(['+', '-']).parse::<f64>().unwrap_or(0.0) * mult
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a realistic macOS top -l1 fixture with `n` process rows.
    fn macos_top(n: usize) -> String {
        let mut s = format!(
            "Processes: {n} total, 2 running, {} sleeping, 500 threads \n\
             2026/03/07 04:51:42\n\
             Load Avg: 1.67, 1.35, 1.23 \n\
             CPU usage: 36.0% user, 64.0% sys, 0.0% idle \n\
             SharedLibs: 227M resident, 33M data, 30M linkedit.\n\
             MemRegions: 59121 total, 879M resident, 65M private, 197M shared.\n\
             PhysMem: 4891M used (958M wired), 1036M unused.\n\
             VM: 14T vsize, 3090M framework vsize, 0(0) swapins, 0(0) swapouts.\n\
             Networks: packets: 603704/493M in, 30938613/19G out.\n\
             Disks: 5482038/191G read, 9296806/389G written.\n\
             \n\
             PID    COMMAND          %CPU TIME     #TH #WQ #PORTS MEM   PURG CMPRS PGRP  PPID  STATE    BOOSTS  %CPU_ME %CPU_OTHRS UID FAULTS COW\n",
            n.saturating_sub(2)
        );
        for i in 0..n {
            let _ = writeln!(
                s,
                "{:<6} proc_{i:<12} {:.1}  00:0{}.00 1   0   10     {}K  0B   0B    {}  1     sleeping *0[1]   0.00000 0.00000    0  100  50",
                1000 + i,
                if i == 0 { 5.0_f64 } else { 0.0_f64 },
                i % 10,
                256 * (i + 1),
                1000 + i,
            );
        }
        s
    }

    #[test]
    fn detects_macos_top() {
        assert!(detect(&macos_top(5)));
        assert!(detect(&macos_top(30)));
    }

    #[test]
    fn rejects_linux_top() {
        let linux = "top - 16:42:18 up 3:14, 2 users, load average: 0.50, 0.42, 0.38\n\
                     Tasks: 412 total,   2 running, 410 sleeping\n\
                     %Cpu(s):  3.2 us,  1.4 sy, 94.9 id\n\
                     MiB Mem :  31984.7 total,   1234.5 free\n\n\
                     PID USER      PR  NI    VIRT    RES    SHR S  %CPU  %MEM     TIME+ COMMAND\n\
                     1234 root      20   0   12345    678    234 R   5.0   0.0  12:34.56 proc\n";
        assert!(!detect(linux));
    }

    #[test]
    fn rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("   \n  "));
    }

    #[test]
    fn preserves_summary_lines() {
        let out = compact(&macos_top(5));
        assert!(
            out.contains("Processes: 5 total, 2 running"),
            "processes summary missing"
        );
        assert!(out.contains("Load Avg: 1.67"), "load avg missing");
        assert!(out.contains("PhysMem: 4891M"), "phymem missing");
    }

    #[test]
    fn preserves_pid_and_command_for_each_row() {
        let out = compact(&macos_top(5));
        for i in 0..5 {
            assert!(
                out.contains(&format!("{}", 1000 + i)),
                "PID {} missing in: {out}",
                1000 + i
            );
            assert!(
                out.contains(&format!("proc_{i}")),
                "COMMAND proc_{i} missing"
            );
        }
    }

    #[test]
    fn column_reduction_saves_tokens() {
        // Past the delivery threshold; under it every column is kept.
        let _g = tk_base::delivery::threshold_on_this_thread(200);
        let input = macos_top(10);
        let out = compact(&input);
        // Column reduction should cut >50% even for short lists
        assert!(
            out.len() < input.len() * 2 / 3,
            "expected >33% size reduction, in={} out={}",
            input.len(),
            out.len()
        );
    }

    #[test]
    fn caps_at_top_n() {
        // Past the delivery threshold; under it the table is whole.
        let _g = tk_base::delivery::threshold_on_this_thread(200);
        let out = compact(&macos_top(50));
        assert!(
            out.contains("25 more process(es) elided"),
            "elision note missing in: {out}"
        );
    }

    #[test]
    fn no_elision_for_small_list() {
        let out = compact(&macos_top(10));
        assert!(!out.contains("elided"), "should not elide: {out}");
    }

    #[test]
    fn passthrough_returns_none_for_non_macos() {
        assert_eq!(try_compact(""), None);
        assert_eq!(try_compact("not top output\nno relevant lines"), None);
    }

    /// Real `top -l1`: PID order, newest first, 0.0 %CPU everywhere. The
    /// oldest PID holds the most memory and must survive the cut.
    #[test]
    fn the_largest_memory_survives_a_pid_ordered_snapshot() {
        let _g = tk_base::delivery::threshold_on_this_thread(200);
        let mut t = String::from("Processes: 41 total\nLoad Avg: 1.0, 1.0, 1.0\nPhysMem: 4891M used\n\nPID COMMAND %CPU TIME #TH #WQ #PORTS MEM PURG CMPRS PGRP PPID STATE\n");
        for i in 0..40 {
            t.push_str(&format!("{} helper{i} 0.0 00:00.01 1 0 10 2465K 0B 0B {} 1 sleeping\n", 90000 - i, 90000 - i));
        }
        t.push_str("155 WindowServer 0.0 13:02.11 21 5 2042 85M 0B 0B 155 1 sleeping\n");
        let out = compact(&t);
        assert!(out.contains("155       WindowServer"), "{out}");
        assert!(out.contains("16 more process(es) elided — ranked by %CPU then MEM"), "{out}");
    }

    /// `top` reaches the compactors by NAME, which resolves to the Linux one:
    /// the Mac layout must still land here.
    #[test]
    fn a_mac_top_reached_by_name_is_read_as_a_mac_top() {
        let _g = tk_base::delivery::threshold_on_this_thread(200);
        let mut t = String::from("Processes: 41 total\nLoad Avg: 1.0, 1.0, 1.0\nPhysMem: 4891M used\n\nPID COMMAND %CPU TIME #TH #WQ #PORTS MEM PURG CMPRS PGRP PPID STATE\n");
        for i in 0..40 {
            t.push_str(&format!("{} helper{i} 0.0 00:00.01 1 0 10 2465K 0B 0B {} 1 sleeping\n", 90000 - i, 90000 - i));
        }
        t.push_str("155 WindowServer 0.0 13:02.11 21 5 2042 85M 0B 0B 155 1 sleeping\n");
        let out = crate::cmd::dispatch_named("top", &t).unwrap();
        assert!(out.contains("155       WindowServer"), "{out}");
    }
}
