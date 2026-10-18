//! `env` / `printenv` / `set` (POSIX) — environment variable dump.
//!
//! ```text
//! USER=username
//! HOME=/home/username
//! PATH=/usr/local/bin:/usr/bin:/bin
//! XDG_CONFIG_HOME=...
//! ...
//! ```
//!
//! Compaction: redact obvious secrets (API_KEY, TOKEN, SECRET,
//! PASSWORD, AUTH, PRIVATE patterns) AND drop noisy XDG_*, GNOME_*,
//! GTK_*, GDK_*, QT_* user-session vars unless the user is mid-
//! diagnostics. Sort by length to make the result deterministic
//! across runs.

use std::fmt::Write;

// The secret-key predicate lives in `crate::env::is_secret_key` — one list,
// one false-positive guard, for every compactor that renders env vars.

const NOISE_PREFIXES: &[&str] = &[
    "XDG_", "GNOME_", "GTK_", "GDK_", "QT_", "DBUS_", "GSM_", "GIO_",
    "GVFS_", "GPG_", "SSH_AUTH_SOCK", "MOTD_", "LESS",
];

pub fn detect(text: &str) -> bool {
    crate::archetypes::any_output(text)
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    let mut kept: Vec<String> = Vec::new();
    let mut redacted = 0usize;
    let mut elided_noise = 0usize;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            kept.push(line.to_string());
            continue;
        };
        // Noise first, secrets second. A dropped line emits nothing at all,
        // which is strictly safer than emitting `NAME=<redacted>` — and the
        // two rules do overlap: the shared secret predicate flags desktop
        // vars like `XDG_SESSION_ID` on the `SESSION` marker, which are pure
        // noise rather than credentials. Dropping them settles it.
        if NOISE_PREFIXES.iter().any(|p| name.starts_with(p)) {
            elided_noise += 1;
            continue;
        }
        if crate::env::is_secret_key(name) {
            kept.push(format!("{name}=<redacted, {} bytes>", value.len()));
            redacted += 1;
            continue;
        }
        kept.push(line.to_string());
    }
    let mut out = kept.join("\n");
    let mut parts: Vec<String> = Vec::new();
    if redacted > 0 {
        parts.push(format!("{redacted} secret(s) redacted"));
    }
    if elided_noise > 0 {
        parts.push(format!("{elided_noise} XDG/GNOME/GTK/Qt vars elided"));
    }
    if !parts.is_empty() {
        let _ = write!(out, "\n  [{}]", parts.join(", "));
    }
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "USER=username\nHOME=/home/username\nPATH=/usr/bin\nAPI_KEY=abc123xyz\nDB_PASSWORD=hunter2\nXDG_RUNTIME_DIR=/run/user/1000\nXDG_CONFIG_HOME=/home/username/.config\nGNOME_SHELL_SESSION_MODE=ubuntu\nGTK_MODULES=gail:atk-bridge\nLANG=fr_FR.UTF-8\nSHELL=/bin/bash\nTERM=xterm-256color\nEDITOR=vim\nPAGER=less\nLOGNAME=infected\n";

    #[test]
    fn detects_env_dump() {
        assert!(detect(SAMPLE));
    }

    #[test]
    fn redacts_secrets() {
        let out = compact(SAMPLE);
        assert!(out.contains("API_KEY=<redacted"));
        assert!(out.contains("DB_PASSWORD=<redacted"));
        assert!(!out.contains("abc123xyz"));
        assert!(!out.contains("hunter2"));
    }

    #[test]
    fn drops_xdg_gnome_noise() {
        let out = compact(SAMPLE);
        assert!(!out.contains("XDG_"));
        assert!(!out.contains("GNOME_"));
        assert!(out.contains("LANG=fr"));
    }

    #[test]
    fn rejects_random() {
        // Detector is now permissive (routes are gated by program
        // name through `dispatch_named`, not content shape). Only
        // empty input is rejected.
        assert!(!detect(""));
        assert!(!detect("   \n  "));
    }
}
