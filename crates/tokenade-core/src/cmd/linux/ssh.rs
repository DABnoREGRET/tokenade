//! `ssh` — shared compactor for SSH-related subcommand output.
//!
//! SSH family commands (ssh-add, ssh-agent, ssh-keygen, ssh-copy-id)
//! typically produce short status messages or key listings. The shared
//! compactor caps at SSH_LINE_CAP lines.
//!
//! All generic `ssh_*` subcommand files delegate to this module.

const SSH_LINE_CAP: usize = 40;

pub fn detect(text: &str) -> bool {
    crate::archetypes::any_output(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    Some(compact(text))
}

pub fn compact(text: &str) -> String {
    crate::archetypes::line_cap(text, SSH_LINE_CAP, "line(s)")
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_short() {
        let s = "Identity added: /home/user/.ssh/id_ed25519\n";
        let out = compact(s);
        assert!(out.contains("Identity"), "short output must pass through");
    }

    #[test]
    fn rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("  \n"));
    }
}
