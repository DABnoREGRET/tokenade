//! `ss` — modern replacement for `netstat` (socket statistics).
//!
//! ```text
//! Netid State  Recv-Q Send-Q Local Address:Port  Peer Address:Port  Process
//! tcp   LISTEN 0      128    0.0.0.0:22         0.0.0.0:*          users:(("sshd",pid=1234,fd=3))
//! tcp   ESTAB  0      0      192.168.1.42:50432 1.2.3.4:443        users:(("firefox",pid=5678,fd=42))
//! ...
//! ```
//!
//! Compaction: collapse the `users:(("foo",pid=X,fd=Y))` blob to
//! just the process name (most agents don't need the fd). Group
//! TIME_WAIT entries.

use std::fmt::Write;

pub fn detect(text: &str) -> bool {
    let first = text.lines().next().unwrap_or("");
    let lower = first.to_ascii_lowercase();
    lower.contains("netid") && lower.contains("state") && lower.contains("address")
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut time_wait_count = 0usize;
    let mut last_was_time_wait = false;
    for line in text.lines() {
        if line.contains(" TIME_WAIT") || line.contains("\tTIME_WAIT") {
            time_wait_count += 1;
            last_was_time_wait = true;
            continue;
        }
        if last_was_time_wait && time_wait_count > 0 {
            let _ = writeln!(
                out,
                "  [elided: {time_wait_count} TIME_WAIT socket(s)]"
            );
            time_wait_count = 0;
            last_was_time_wait = false;
        }
        // Collapse `users:(("foo",pid=X,fd=Y),...)` to `proc:foo[,bar]`.
        let collapsed = collapse_users(line);
        out.push_str(&collapsed);
        out.push('\n');
    }
    if time_wait_count > 0 {
        let _ = writeln!(
            out,
            "  [elided: {time_wait_count} TIME_WAIT socket(s)]"
        );
    }
    out.trim_end().to_string()
}

fn collapse_users(line: &str) -> String {
    let Some(idx) = line.find("users:((") else {
        return line.to_string();
    };
    let prefix = &line[..idx];
    let rest = &line[idx + "users:((".len()..];
    // Extract every `"name"` token.
    let mut names: Vec<String> = Vec::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        if c == '"' {
            let mut name = String::new();
            for nc in chars.by_ref() {
                if nc == '"' {
                    break;
                }
                name.push(nc);
            }
            if !name.is_empty() && !names.contains(&name) {
                names.push(name);
            }
        }
    }
    format!("{prefix}proc:{}", names.join(","))
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_ss_output() {
        let s = "Netid State  Recv-Q Send-Q Local Address:Port  Peer Address:Port  Process\n";
        assert!(detect(s));
    }

    #[test]
    fn collapses_users_blob() {
        let s = "Netid State Recv-Q Send-Q Local Address:Port Peer Address:Port Process\ntcp LISTEN 0 128 0.0.0.0:22 0.0.0.0:* users:((\"sshd\",pid=1234,fd=3))\n";
        let out = compact(s);
        assert!(out.contains("proc:sshd"));
        assert!(!out.contains("pid=1234"));
    }

    #[test]
    fn elides_time_wait_runs() {
        let mut s = String::from("Netid State Recv-Q Send-Q Local Address:Port Peer Address:Port Process\n");
        for i in 0..20 {
            let _ = writeln!(
                s,
                "tcp TIME_WAIT 0 0 10.0.0.{i}:443 1.2.3.4:50000"
            );
        }
        s.push_str("tcp LISTEN 0 128 0.0.0.0:22 0.0.0.0:* users:((\"sshd\",pid=1,fd=3))\n");
        let out = compact(&s);
        assert!(out.contains("20 TIME_WAIT"));
        assert!(out.contains("sshd"));
    }
}
