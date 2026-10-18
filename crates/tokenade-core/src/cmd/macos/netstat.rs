//! macOS BSD `netstat -an` / `netstat -tan`.
//!
//! Different shape from Linux: no leading `Proto Recv-Q Send-Q`
//! padding column widths. macOS uses `tcp4` / `tcp6` not `tcp`.

use std::fmt::Write;

pub fn detect(text: &str) -> bool {
    text.contains("Proto") || text.contains("LISTEN") || text.contains("ESTABLISHED")
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut tw = 0usize;
    for line in text.lines() {
        if line.contains("TIME_WAIT") || line.contains("CLOSE_WAIT") {
            tw += 1;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if tw > 0 {
        let _ = writeln!(out, "  [elided: {tw} TIME_WAIT/CLOSE_WAIT socket(s)]");
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
    fn detects_bsd_netstat() {
        let s = "Active Internet connections (including servers)\nProto Recv-Q Send-Q Local Address          Foreign Address        (state)\ntcp4       0      0  *.22                   *.*                    LISTEN\n";
        assert!(detect(s));
    }
}
