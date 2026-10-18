//! Windows `tasklist`.
//!
//! ```text
//! Image Name                     PID Session Name        Session#    Mem Usage
//! ========================= ======== ================ =========== ============
//! System Idle Process              0 Services                   0          8 K
//! System                           4 Services                   0      2,300 K
//! Registry                        80 Services                   0     34,656 K
//! ...
//! ```
//!
//! Compaction: rows sorted by memory. While the table fits the delivery
//! threshold every process is kept, grouped under its session; past it, the
//! top 30 by memory + an elision line.

use crate::tabular::{ruler_cell, ruler_spans};
use std::fmt::Write;

pub fn detect(text: &str) -> bool {
    text.contains("Image Name") || text.contains(".exe") || text.contains("PID")
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    let mut headers: Vec<&str> = Vec::new();
    let mut body: Vec<&str> = Vec::new();
    let mut seen_sep = false;
    for line in text.lines() {
        if !seen_sep {
            headers.push(line);
            if line.starts_with("===") {
                seen_sep = true;
            }
        } else if !line.trim().is_empty() {
            body.push(line);
        }
    }
    let spans = headers.last().map(|sep| ruler_spans(sep)).unwrap_or_default();
    // A process is its row PLUS the lines that continue it: `/SVC` and `/M`
    // wrap a long list onto lines whose name and PID cells are blank.
    // Sorted or cut one line at a time, `Power, SystemEventsBroker` landed
    // under another process, or alone.
    let mut records: Vec<Vec<&str>> = Vec::new();
    for line in body {
        let continues = spans.len() >= 2 && {
            let chars: Vec<char> = line.chars().collect();
            ruler_cell(&chars, &spans, 0).is_empty() && ruler_cell(&chars, &spans, 1).is_empty()
        };
        match records.last_mut() {
            Some(record) if continues => record.push(line),
            _ => records.push(vec![line]),
        }
    }
    if records.len() <= 30 {
        return text.to_string();
    }
    // Sort by mem usage (stable: a layout without a memory column keeps its
    // order).
    records.sort_by_key(|r| std::cmp::Reverse(mem_of(r[0])));
    // **Every process, while the table fits.** The top-30 cut lost the
    // process an agent asks `tasklist` about — measured 2026-10-02 on a real
    // Windows 10 (fr-FR): 80 processes, 6 KB, under the delivery threshold,
    // and 50 of them (with their PIDs) gone; `/svc`, with no memory column to
    // sort on, kept the first thirty. What a row costs is its padding — and,
    // in the default layout, the session pair repeated on every line (74 of
    // 80 said `Services 0`), grouped under its session instead.
    if spans.len() >= 2 {
        let title: Vec<char> = headers.get(headers.len().saturating_sub(2)).copied().unwrap_or("").chars().collect();
        let whole = if spans.len() == 5 {
            grouped_by_session(&title, &spans, &records)
        } else {
            respaced(&title, &spans, &records)
        };
        if let Some(whole) = whole.filter(|w| tk_base::delivery::fits(w)) {
            return whole;
        }
    }
    let mut out = String::with_capacity(text.len() / 2);
    for h in &headers {
        out.push_str(h);
        out.push('\n');
    }
    for line in records.iter().take(30).flatten() {
        out.push_str(line);
        out.push('\n');
    }
    let _ = writeln!(
        out,
        "  [... {} more process(es) elided — re-run with /FI for filtering]",
        records.len() - 30
    );
    out.trim_end().to_string()
}

/// A record's first two cells, which every layout opens with: image name
/// and a numeric PID. `None` when the row is not such a row.
fn name_and_pid(chars: &[char], spans: &[(usize, usize)]) -> Option<(String, String)> {
    let (name, pid) = (ruler_cell(chars, spans, 0), ruler_cell(chars, spans, 1));
    (!name.is_empty() && !pid.is_empty() && pid.chars().all(|c| c.is_ascii_digit())).then_some((name, pid))
}

/// The default five-column table (`Image Name`, `PID`, `Session Name`,
/// `Session#`, `Mem Usage`), sorted rows regrouped under one
/// `Session Name Session#` line per session, each row `name pid mem`.
fn grouped_by_session(title: &[char], spans: &[(usize, usize)], records: &[Vec<&str>]) -> Option<String> {
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for record in records {
        let chars: Vec<char> = record[0].chars().collect();
        let (name, pid) = name_and_pid(&chars, spans)?;
        let [session, number, mem] = [2, 3, 4].map(|k| ruler_cell(&chars, spans, k));
        let key = format!("{session} {number}");
        let entry = format!("{name} {pid} {mem}");
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => v.push(entry),
            None => groups.push((key, vec![entry])),
        }
    }
    let mut out = format!("{} {} {}\n", ruler_cell(title, spans, 0), ruler_cell(title, spans, 1), ruler_cell(title, spans, 4));
    for (key, entries) in &groups {
        let _ = writeln!(out, "[{} {key} — {}]", ruler_cell(title, spans, 2), entries.len());
        for e in entries {
            out.push_str(e);
            out.push('\n');
        }
    }
    Some(out.trim_end().to_string())
}

/// Any other layout (`/SVC`, `/M`, `/V`): one line per process, its cells
/// separated by two spaces (a cell may hold one: `AUTORITE NT\Système`), its
/// continuation lines joined onto its last cell.
fn respaced(title: &[char], spans: &[(usize, usize)], records: &[Vec<&str>]) -> Option<String> {
    let last = spans.len() - 1;
    let mut out = (0..spans.len()).map(|k| ruler_cell(title, spans, k)).collect::<Vec<_>>().join("  ");
    out.push('\n');
    for record in records {
        let chars: Vec<char> = record[0].chars().collect();
        name_and_pid(&chars, spans)?;
        let mut cells: Vec<String> = (0..spans.len()).map(|k| ruler_cell(&chars, spans, k)).collect();
        for more in &record[1..] {
            let more: Vec<char> = more.chars().collect();
            let tail = ruler_cell(&more, spans, last);
            cells[last] = format!("{} {tail}", cells[last]).trim().to_string();
        }
        out.push_str(&cells.join("  "));
        out.push('\n');
    }
    Some(out.trim_end().to_string())
}

fn mem_of(line: &str) -> u64 {
    // `tasklist` ends rows with `1,234 K`; `tasklist /V` APPENDS Status/
    // User/CPU-Time/Window-Title after it, so "last token" missed the
    // memory column and every /V process sorted as 0. Scan from the right
    // for the `<number> K` pair instead.
    //
    // The unit and the thousands separator are the LOCALE's: a French Windows
    // writes `26 236 Ko` — `Ko`, and a no-break space inside the number.
    // Measured 2026-10-02 on a real Windows 10 (fr-FR): only `K` was read, so
    // every row weighed 0, the sort did nothing, and "top 30 by memory" kept
    // the FIRST thirty (System, smss, csrss…). Split on ASCII spaces only, so
    // a no-break space stays inside its number.
    let tokens: Vec<&str> = line.split(|c: char| c.is_ascii_whitespace()).filter(|t| !t.is_empty()).collect();
    for i in (1..tokens.len()).rev() {
        let scale = match tokens[i].to_ascii_lowercase().as_str() {
            "k" | "kb" | "ko" | "kib" => 1,
            "m" | "mb" | "mo" | "mib" => 1024,
            _ => continue,
        };
        let digits: String = tokens[i - 1].chars().filter(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            return digits.parse::<u64>().unwrap_or(0) * scale;
        }
    }
    0
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(n: usize) -> String {
        let mut s = String::from("Image Name                     PID Session Name        Session#    Mem Usage\n");
        s.push_str("========================= ======== ================ =========== ============\n");
        for i in 0..n {
            let _ = writeln!(
                s,
                "process_{i:<18}.exe {pid:>8} Services                   0      {mem:>5} K",
                pid = 1000 + i,
                mem = (i * 100) % 999_999
            );
        }
        s
    }

    #[test]
    fn detects_tasklist() {
        assert!(detect(&sample(5)));
    }

    #[test]
    fn folds_long_list() {
        let s = sample(3000);
        let out = compact(&s);
        assert!(out.contains("more process"));
    }

    fn svc_sample(n: usize) -> String {
        let mut s = String::from("\nImage Name                     PID Services\n");
        s.push_str("========================= ======== ============================================\n");
        for i in 0..n {
            let _ = writeln!(s, "svchost.exe               {pid:>8} Svc{i}", pid = 1000 + i);
            if i == 7 {
                s.push_str("                                   Power, SystemEventsBroker\n");
            }
        }
        s
    }

    /// `/svc` has no memory column and wraps a long service list onto a
    /// line of its own: every process is kept, the wrapped list joined back.
    #[test]
    fn svc_keeps_every_process_and_its_wrapped_services() {
        let out = compact(&svc_sample(60));
        assert!(!out.contains("more process"), "{out}");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "Image Name  PID  Services");
        assert_eq!(lines[1], "svchost.exe  1000  Svc0");
        assert!(lines.contains(&"svchost.exe  1007  Svc7 Power, SystemEventsBroker"), "{out}");
        assert_eq!(lines.len(), 61, "{out}");
    }

    /// Past the threshold the cut still keeps a continuation under its row.
    #[test]
    fn a_cut_keeps_a_continuation_with_its_process() {
        let out = compact(&svc_sample(4000));
        assert!(out.contains("more process"));
        assert!(out.contains("1007 Svc7\n                                   Power, SystemEventsBroker\n"), "{}", &out[..900]);
    }

    #[test]
    fn a_table_that_fits_keeps_every_process() {
        let mut s = String::from("Image Name                     PID Session Name        Session#    Mem Usage\n");
        s.push_str("========================= ======== ================ =========== ============\n");
        s.push_str("System Idle Process              0 Services                   0          8 K\n");
        for i in 0..60 {
            let _ = writeln!(s, "svchost.exe               {pid:>8} Services                   0     {mem:>6} K", pid = 1000 + i, mem = format!("1,{:03}", i));
        }
        s.push_str("dwm.exe                        420 Console                    1     48,812 K\n");
        // A memory figure wider than its `====` ruler runs past the line's end.
        s.push_str("sqlservr.exe                  7000 Services                   0 123,456,789 K\n");
        let out = compact(&s);
        assert!(out.contains("\nsqlservr.exe 7000 123,456,789 K\n"), "{out}");
        assert!(!out.contains("more process"), "{out}");
        for i in 0..60 {
            assert!(out.contains(&format!("\nsvchost.exe {} 1,{:03} K\n", 1000 + i, i)), "pid {}: {out}", 1000 + i);
        }
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "Image Name PID Mem Usage");
        // Sessions come in the order of their biggest process.
        assert_eq!(&lines[1..3], ["[Session Name Services 0 — 62]", "sqlservr.exe 7000 123,456,789 K"]);
        assert_eq!(&lines[lines.len() - 3..], ["System Idle Process 0 8 K", "[Session Name Console 1 — 1]", "dwm.exe 420 48,812 K"]);
    }

    #[test]
    fn keeps_short_list_verbatim() {
        let s = sample(10);
        let out = compact(&s);
        assert_eq!(out, s);
    }

    /// Windows 10 in French: `Ko`, and a no-break space inside the number.
    #[test]
    fn memory_is_read_in_the_system_language() {
        assert_eq!(mem_of("Registry                        92 Services                   0    26\u{a0}236 Ko"), 26_236);
        assert_eq!(mem_of("dwm.exe                        420 Console                    1    48\u{202f}772 Ko"), 48_772);
        assert_eq!(mem_of("System                           4 Services                   0      2,300 K"), 2_300);
        assert_eq!(mem_of("big.exe                          9 Console                    1        3 Mo"), 3 * 1024);
        // So the sort keeps the hungriest, not the first to come.
        let mut s = String::from("Nom de l'image                 PID Nom de la sessio Numéro de s Utilisation\n========================= ======== ================ =========== ============\n");
        for i in 0..40 {
            let _ = writeln!(s, "proc{i}.exe {pid:>8} Services 0 {mem} Ko", pid = 100 + i, mem = if i == 39 { "512\u{a0}000".to_string() } else { format!("{}", 100 + i) });
        }
        let out = compact(&s);
        assert!(out.contains("proc39.exe"), "the biggest process was elided:\n{out}");
    }
}
