//! **The shell profile: write a block we will know how to remove.**
//!
//! We are modifying someone else's file. The block is therefore BOUNDED by two
//! markers, which makes removal exact — without them, uninstalling would mean
//! guessing which lines were ours.

use super::*;

/// The file the login shell reads at startup.
///
/// The choice follows `$SHELL`, not a preference: it is the file the person
/// will actually see loaded. Without `$SHELL` — a service, a cron —
/// `~/.profile`, read by sh and bash as a login shell.
///
/// On Windows, the PowerShell profile, which is the only file a shell reads
/// there at startup (cmd.exe has none). A `setx` would reach every process,
/// graphical applications included, but it is not a file: it cannot be
/// backed up, compared or restored byte for byte, which is what the
/// constraint requires and what the "Reversible install" column
/// measures.
pub fn shell_profile_path(home: &Path) -> PathBuf {
    if cfg!(windows) {
        return home
            .join("Documents")
            .join("WindowsPowerShell")
            .join("Microsoft.PowerShell_profile.ps1");
    }
    let shell = std::env::var("SHELL").unwrap_or_default();
    match Path::new(&shell).file_name().and_then(|s| s.to_str()) {
        Some("zsh") => home.join(".zshrc"),
        Some("bash") => home.join(".bashrc"),
        Some("fish") => home.join(".config").join("fish").join("config.fish"),
        _ => home.join(".profile"),
    }
}

/// The two lines that bracket what we write. A marked block is removed byte
/// for byte; an isolated line is found again by resemblance, which is not the
/// same thing and which uninstalling cannot afford in a file the person edits
/// themselves.
/// cursor's default endpoint, declared by its own `--endpoint` option
/// (`.env("CURSOR_API_ENDPOINT")`, default `https://api2.cursor.sh`), read in
/// build 2026.08.11. Serves as the upstream when the person had set nothing.
pub(super) const CURSOR_DEFAULT_UPSTREAM: &str = "https://api2.cursor.sh";

pub(super) const PROFILE_OPEN: &str = "# >>> tokenade llm proxy >>>";
pub(super) const PROFILE_CLOSE: &str = "# <<< tokenade llm proxy <<<";

pub(super) fn profile_export(path: &Path, var: &str, value: &str) -> String {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    if name.ends_with(".ps1") {
        format!("$env:{var} = \"{value}\"")
    } else if name == "config.fish" {
        format!("set -gx {var} {value}")
    } else {
        format!("export {var}={value}")
    }
}

/// What the variable is worth BEFORE us, so it can be put back.
///
/// The block we may already have written is ignored: reinstalling on top must
/// not record our own value as "the previous one", otherwise uninstalling
/// leaves the agent pointed at the proxy forever.
pub(super) fn read_shell_profile(text: &str, var: &str) -> Option<String> {
    let stripped = strip_profile_block(text);
    for line in stripped.lines() {
        let l = line.trim();
        for prefix in [
            format!("export {var}="),
            format!("set -gx {var} "),
            format!("$env:{var} = "),
        ] {
            if let Some(rest) = l.strip_prefix(prefix.as_str()) {
                return Some(rest.trim().trim_matches('"').to_string());
            }
        }
    }
    None
}

pub(super) fn strip_profile_block(text: &str) -> String {
    let Some(start) = text.find(PROFILE_OPEN) else {
        return text.to_string();
    };
    let Some(end) = text[start..].find(PROFILE_CLOSE) else {
        return text.to_string();
    };
    let end = start + end + PROFILE_CLOSE.len();
    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..start]);
    // The marker line takes its newline with it, otherwise every
    // install/uninstall cycle leaves one more blank line behind and the file
    // is no longer the one from before.
    let tail = &text[end..];
    out.push_str(tail.strip_prefix('\n').unwrap_or(tail));
    out
}

pub(super) fn write_shell_profile(text: &str, path: &Path, var: &str, value: &str) -> String {
    let base = strip_profile_block(text);
    let mut out = base.clone();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(PROFILE_OPEN);
    out.push('\n');
    out.push_str(&profile_export(path, var, value));
    out.push('\n');
    out.push_str(PROFILE_CLOSE);
    out.push('\n');
    out
}

pub(super) fn revert_shell_profile(text: &str, record: &Record, path: &Path, var: &str) -> String {
    let base = strip_profile_block(text);
    // The person had their own value: it is put back as is, outside any
    // marker, since it is a line of their own.
    match record.previous_selector.as_deref() {
        Some(prev) if !prev.is_empty() && !base.contains(&var.to_string()) => {
            let mut out = base;
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&profile_export(path, var, prev));
            out.push('\n');
            out
        }
        _ => base,
    }
}
