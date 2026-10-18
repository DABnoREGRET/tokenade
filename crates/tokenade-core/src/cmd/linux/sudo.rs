//! `sudo -l` / `sudoers` — current user's sudo capabilities.
//!
//! ```text
//! Matching Defaults entries for me on host:
//!     env_reset, mail_badpass, secure_path=...
//!
//! User me may run the following commands on host:
//!     (root) NOPASSWD: /usr/bin/apt
//!     (root) /usr/bin/systemctl
//! ```


pub fn detect(text: &str) -> bool {
    text.contains("Matching Defaults entries")
        || (text.contains("may run the following commands") && text.contains("(root)"))
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    // sudo -l output is typically short and the entire content is
    // actionable. Passthrough.
    text.trim().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_sudo_l() {
        let s = "Matching Defaults entries for me on host:\n    env_reset, mail_badpass\n\nUser me may run the following commands on host:\n    (root) NOPASSWD: /usr/bin/apt\n";
        assert!(detect(s));
    }
}
