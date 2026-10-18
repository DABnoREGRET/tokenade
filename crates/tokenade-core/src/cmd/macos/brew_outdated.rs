//! `brew outdated` — the outdated packages ARE the answer.
//!
//! Observed (macOS 15.7, 2026-10-03): when Homebrew auto-updates first, the
//! list comes AFTER ~90 lines of update report — hints, a download with its
//! progress bar, and `==> New Formulae` with one description per formula.
//! The old table fold kept the first 60 lines, i.e. the report, and elided
//! the 28 outdated packages the command was run for. Its detector also
//! required a `(`, which only the tty `--verbose` form prints: the plain
//! piped form (one name per line) was never recognised.
//!
//! The report is reduced to what it says (`Updated 1 tap`, each section's
//! formula NAMES); the list after it is delivered whole while it fits.
use crate::compress::whole_or_truncate_middle;
use std::fmt::Write;

const AUTO_UPDATE: &str = "==> Auto-updat";

pub fn detect(text: &str) -> bool {
    !text.trim().is_empty()
}

/// Splits `text` into the auto-update report and the list after it: the
/// report ends at its last blank line, which Homebrew prints before the
/// command's own output.
fn split_report(text: &str) -> Option<(&str, &str)> {
    if !text.trim_start().starts_with(AUTO_UPDATE) {
        return None;
    }
    match text.rfind("\n\n") {
        Some(i) => Some((&text[..i], &text[i + 2..])),
        None => Some((text, "")),
    }
}

/// `Updated 1 tap`, and every `==> <section>` with its formula names.
fn report_digest(report: &str) -> String {
    let mut out = String::new();
    let mut section: Option<(&str, Vec<&str>)> = None;
    let flush = |out: &mut String, s: &mut Option<(&str, Vec<&str>)>| {
        if let Some((head, names)) = s.take() {
            let _ = writeln!(out, "{head} ({}): {}", names.len(), names.join(" · "));
        }
    };
    for line in report.lines() {
        let t = line.trim();
        if let Some(h) = t.strip_prefix("==> ") {
            flush(&mut out, &mut section);
            let is_list = !h.starts_with("Auto-upd")
                && !h.starts_with("Downloading")
                && !h.starts_with("Pouring");
            if is_list {
                section = Some((t, Vec::new()));
            }
        } else if let Some((_, names)) = section.as_mut() {
            if !t.is_empty() {
                names.push(t.split(':').next().unwrap_or(t));
            }
        } else if t.starts_with("Updated ") || t.starts_with("You have ") {
            let _ = writeln!(out, "{t}");
        }
    }
    flush(&mut out, &mut section);
    out
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    let Some((report, list)) = split_report(text) else {
        let out = whole_or_truncate_middle(text.trim_end(), 60, 10);
        return (out.len() < text.trim_end().len()).then_some(out);
    };
    let list = list.trim_end();
    let n = list.lines().filter(|l| !l.trim().is_empty()).count();
    let mut out = format!(
        "[tokenade:brew-outdated] {n} outdated package(s) — auto-update report reduced to its names\n"
    );
    out.push_str(&report_digest(report));
    out.push('\n');
    out.push_str(&whole_or_truncate_middle(list, 60, 10));
    Some(out)
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    const AUTO_UPDATED: &str = "==> Auto-updating Homebrew...\n\
        Adjust how often this is run with `$HOMEBREW_AUTO_UPDATE_SECS` or disable with\n\
        `$HOMEBREW_NO_AUTO_UPDATE=1`. Hide these hints with `$HOMEBREW_NO_ENV_HINTS=1` (see `man brew`).\n\
        ==> Downloading https://ghcr.io/v2/homebrew/core/portable-ruby/blobs/sha256:e0088dff\n\
        ######################################################################## 100.0%\n\
        ==> Pouring portable-ruby-4.0.7.arm64_big_sur.bottle.tar.gz\n\
        ==> Auto-updated Homebrew!\n\
        Updated 1 tap (homebrew/core).\n\
        ==> New Formulae\n\
        agent-manager: Run Claude Code, Codex, OpenCode and other AI coding agents in tmux\n\
        amy: Nostr client from the Amethyst project\n\
        \n\
        ca-certificates\n\
        cairo\n\
        openssl@3\n\
        python@3.12\n\
        google-chrome\n";

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
    }

    /// The real shape: the outdated list follows the update report, and the
    /// report used to fill the whole head while the list was elided.
    #[test]
    fn the_outdated_list_survives_the_auto_update_report() {
        let out = try_compact(AUTO_UPDATED).unwrap();
        assert!(out.starts_with("[tokenade:brew-outdated] 5 outdated package(s)"), "{out}");
        assert!(
            out.ends_with("ca-certificates\ncairo\nopenssl@3\npython@3.12\ngoogle-chrome"),
            "{out}"
        );
        assert!(out.contains("Updated 1 tap (homebrew/core)."), "{out}");
        assert!(out.contains("==> New Formulae (2): agent-manager · amy"), "{out}");
        assert!(!out.contains("Nostr client"), "descriptions go:\n{out}");
        assert!(!out.contains("100.0%"), "the progress bar goes:\n{out}");
    }

    #[test]
    fn a_plain_list_that_fits_passes_through() {
        assert!(try_compact("cairo\nopenssl@3\n").is_none());
    }
}
