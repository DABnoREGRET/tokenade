//! `dpkg -l` / `dpkg --list` — installed package table.
//!
//! ```text
//! Desired=Unknown/Install/Remove/Purge/Hold
//! | Status=Not/Inst/Conf-files/Unpacked/halF-conf/Half-inst/trig-aWait/Trig-pend
//! |/ Err?=(none)/Reinst-required (Status,Err: uppercase=bad)
//! ||/ Name                 Version           Architecture  Description
//! +++-====================-=================-=============-====================
//! ii  acl                  2.3.1-3           amd64         access control list
//! ii  adduser              3.118ubuntu5      all           add users
//! ...
//! ```
//!
//! Compaction: drop the canonical 4-line legend header. Fold long
//! lists.

use std::fmt::Write;

pub fn detect(text: &str) -> bool {
    text.contains("Desired=Unknown/Install/Remove")
        || (text.contains("+++") && text.contains("Name") && text.contains("Version"))
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    let mut elided_legend = 0usize;
    let mut data: Vec<&str> = Vec::new();
    let mut saw_separator = false;
    for line in text.lines() {
        if line.starts_with("+++-") {
            saw_separator = true;
            continue;
        }
        if !saw_separator {
            elided_legend += 1;
            continue;
        }
        if !line.trim().is_empty() {
            data.push(line);
        }
    }
    let mut out = String::new();
    if elided_legend > 0 {
        let _ = writeln!(
            out,
            "  [elided: {elided_legend} line(s) of canonical legend header]"
        );
    }
    if data.len() <= 30 {
        out.push_str(&data.join("\n"));
    } else {
        for d in data.iter().take(30) {
            out.push_str(d);
            out.push('\n');
        }
        let _ = writeln!(
            out,
            "  [... {} more package(s) elided]",
            data.len() - 30
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
    fn detects_dpkg_list() {
        let s = "Desired=Unknown/Install/Remove/Purge/Hold\n| Status=Not/Inst\n||/ Name                 Version\n+++-====================-=================\nii  acl                  2.3.1-3\n";
        assert!(detect(s));
    }

    #[test]
    fn drops_legend() {
        let s = "Desired=Unknown/Install/Remove/Purge/Hold\n| Status=Not/Inst\n||/ Name                 Version\n+++-====================-=================\nii  acl                  2.3.1-3\nii  adduser              3.118\n";
        let out = compact(s);
        assert!(out.contains("acl"));
        assert!(out.contains("legend"));
        assert!(!out.contains("Desired=Unknown"));
    }
}
