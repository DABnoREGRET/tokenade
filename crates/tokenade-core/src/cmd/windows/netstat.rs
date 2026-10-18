//! Windows `netstat -ano` / `netstat -an`.
//!
//! ```text
//! Active Connections
//!
//!   Proto  Local Address          Foreign Address        State           PID
//!   TCP    0.0.0.0:135            0.0.0.0:0              LISTENING       968
//!   TCP    0.0.0.0:445            0.0.0.0:0              LISTENING       4
//!   TCP    127.0.0.1:50432        127.0.0.1:50431        ESTABLISHED     1234
//!   ...
//!   UDP    0.0.0.0:5353           *:*                                    2048
//!   ...
//! ```
//!
//! Compaction: collapse TIME_WAIT/CLOSE_WAIT/FIN_WAIT runs, drop
//! the localised banner ("Active Connections" / "Connexions
//! actives" / "Aktive Verbindungen" / "Conexiones activas").

use std::fmt::Write;

pub fn detect(text: &str) -> bool {
    text.contains("Proto") || text.contains("LISTENING") || text.contains("ESTABLISHED")
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut time_wait = 0usize;
    let mut close_wait = 0usize;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            out.push('\n');
            continue;
        }
        if trimmed.contains("TIME_WAIT") {
            time_wait += 1;
            continue;
        }
        if trimmed.contains("CLOSE_WAIT") || trimmed.contains("FIN_WAIT") {
            close_wait += 1;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if time_wait + close_wait > 0 {
        let _ = writeln!(
            out,
            "  [elided: {time_wait} TIME_WAIT, {close_wait} CLOSE_WAIT/FIN_WAIT]"
        );
    }
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_classic_english() {
        let s = "Active Connections\n\n  Proto  Local Address          Foreign Address        State           PID\n  TCP    0.0.0.0:135            0.0.0.0:0              LISTENING       968\n";
        assert!(detect(s));
    }

    #[test]
    fn detects_french() {
        let s = "Connexions actives\n\n  Proto  Adresse locale          Adresse distante       Etat           PID\n";
        assert!(detect(s));
    }

    #[test]
    fn elides_time_wait_swarm() {
        let mut s = String::from("Active Connections\n\n  Proto  Local Address          Foreign Address        State           PID\n");
        s.push_str("  TCP    0.0.0.0:135            0.0.0.0:0              LISTENING       968\n");
        for i in 0..50 {
            let _ = writeln!(
                s,
                "  TCP    127.0.0.1:50{i:>03}        127.0.0.1:443       TIME_WAIT       0"
            );
        }
        let out = compact(&s);
        assert!(out.contains("LISTENING"));
        assert!(out.contains("50 TIME_WAIT"));
        assert!(!out.contains("TIME_WAIT       0"));
    }
}
