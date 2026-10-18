//! `npm_install` — npm install summary.
//!
//! Archetype: strip

pub fn detect(text: &str) -> bool {
    text.contains("added ") || text.contains("removed ") || text.contains("audited ") || text.contains("found ") || text.contains("vulnerabilities")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    Some(compact(text))
}

pub fn compact(text: &str) -> String {
    crate::archetypes::strip_noise(&without_update_notice(text), &["npm warn", "npm WARN"], "npm warnings")
}

/// **npm's own update nag is not the install's output.** Five `npm notice`
/// lines — `New major version of npm available! 10.8.2 -> 11.6.1`, its
/// changelog URL, `To update run: npm install -g npm@…`, framed by two empty
/// notices — closed every install on a machine with an older npm, at any
/// size, and said nothing about what was installed (headroom #2550 silences
/// the same banner at its source with `npm_config_update_notifier`; changing
/// the user's environment is not ours to do, dropping the lines is). ONLY
/// that block: other notices (`npm notice` lines of a publish, a tarball's
/// contents) are content.
fn without_update_notice(text: &str) -> String {
    if !text.contains("version of npm available") {
        return text.to_string();
    }
    let nag = |l: &str| {
        let t = l.trim();
        let Some(rest) = t.strip_prefix("npm notice") else { return false };
        let rest = rest.trim();
        rest.is_empty()
            || (rest.starts_with("New ") && rest.contains("version of npm available"))
            || rest.starts_with("Changelog: https://github.com/npm/cli/releases")
            || rest.starts_with("To update run: npm install -g npm@")
    };
    let kept: Vec<&str> = text.lines().filter(|l| !nag(l)).collect();
    let mut out = kept.join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    out
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("   \n"));
    }

    #[test]
    fn the_update_nag_goes_and_other_notices_stay() {
        let out = compact("added 214 packages in 6s\n\nfound 0 vulnerabilities\nnpm notice\nnpm notice New major version of npm available! 10.8.2 -> 11.6.1\nnpm notice Changelog: https://github.com/npm/cli/releases/tag/v11.6.1\nnpm notice To update run: npm install -g npm@11.6.1\nnpm notice\n");
        assert_eq!(out.trim_end(), "added 214 packages in 6s\n\nfound 0 vulnerabilities");
        // Without the nag, an empty notice or a publish's notices are untouched.
        let publish = "npm notice\nnpm notice package: demo@1.0.0\nnpm notice 1.2kB index.js\n";
        assert_eq!(compact(publish).trim_end(), publish.trim_end());
    }

    #[test]
    fn try_compact_some_or_none() {
        let s = "sample output line\nsecond line\n";
        let _ = try_compact(s);
        let _ = compact(s);
        let _ = compact_with_savings(s);
    }
}
