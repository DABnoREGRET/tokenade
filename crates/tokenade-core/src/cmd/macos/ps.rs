//! macOS `ps aux` / `ps -ef` — BSD shape.
//!
//! BSD `ps` columns differ from Linux:
//! - `STAT` column has BSD letters (`S+`, `R+`, `Ss`, `Sl`)
//! - `STARTED` is a date OR clock time
//! - `COMMAND` column is bare (no `[kernel-thread]` brackets)
//!
//! Compaction: drop kernel-thread placeholders, fold long process
//! lists, redact long command-line paths.

use std::fmt::Write;

pub fn detect(text: &str) -> bool {
    text.contains("PID") || text.contains("COMMAND")
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let whole = crate::cmd::linux::ps_table::respaced(&lines);
    if lines.len() <= 50 || tk_base::delivery::fits(whole.as_deref().unwrap_or(text)) {
        // Every column, respaced: the padding is half the bytes. Handed back
        // unchanged, this lost to `auto`'s tabular fold, which is shorter
        // because it keeps COMMAND ALONE — measured 2026-10-03 on the corpus
        // `macos/ps_aux` (29 processes, 4.4 KB): no PID, user, %CPU or state
        // in what was delivered. A respaced table folds, so it keeps the hand.
        return whole.unwrap_or_else(|| text.to_string());
    }
    let mut out = String::with_capacity(text.len() / 2);
    for l in lines.iter().take(50) {
        out.push_str(l);
        out.push('\n');
    }
    let _ = writeln!(
        out,
        "  [... {} more process(es) elided]",
        lines.len() - 50
    );
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real macOS `ps aux` (corpus): every column kept, the padding gone, and
    /// a command with spaces untouched.
    #[test]
    fn a_short_ps_aux_keeps_every_column_respaced() {
        let body = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/macos/ps_aux.txt")).unwrap();
        let out = compact(&body);
        assert!(out.starts_with("USER PID %CPU %MEM VSZ RSS TT STAT STARTED TIME COMMAND\n"), "{out}");
        assert!(out.contains("\nroot 109 56.8 0.1 33827288 8200 ?? Rs 22Jan26 10:17.64 /usr/libexec/opendirectoryd\n"), "{out}");
        assert!(out.contains(" sshd: macos [priv]"), "{out}");
        assert!(out.len() < body.len(), "{} of {}", out.len(), body.len());
    }

    #[test]
    fn detects_ps_aux() {
        let s = "USER     PID %CPU %MEM    VSZ   RSS   TT  STAT STARTED      TIME COMMAND\nroot       1   0.0   0.1  12345  6789  ??  Ss   Mon09AM   0:42.31 /sbin/launchd\n";
        assert!(detect(s));
    }
}
