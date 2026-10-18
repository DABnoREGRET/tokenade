//! `npm view <pkg> versions` — every version ever published, oldest first.
//!
//! The list runs to thousands (`typescript`: 3 841 entries, 90 KB as JSON),
//! and npm prints it in ascending semver order. The generic caps kept the
//! FIRST twenty — `0.8.0` … `1.6.0-beta` — and counted the rest: the end of
//! the list, which is what the command is run to read ("what is the latest
//! 5.x?", "is there a newer beta?"), was the part thrown away.
//!
//! What is kept instead: how many versions and how many are stable, the
//! oldest one, the latest stable releases and the latest pre-releases, in
//! the order npm printed them.

/// Stable releases shown, newest last.
const STABLE_SHOWN: usize = 15;
/// Pre-releases shown, newest last.
const PRE_SHOWN: usize = 8;
/// Below this many versions the list is short enough to read as it is.
const MIN_VERSIONS: usize = 40;

/// Every quoted `MAJOR.MINOR.PATCH[-…]` in the output, in order — the JSON
/// form (`"5.6.3"`) and the JavaScript one npm prints without `--json`
/// (`'5.6.3'`) alike.
fn versions(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for piece in text.split(['"', '\'']).skip(1).step_by(2) {
        let core = piece.split(['-', '+']).next().unwrap_or("");
        let parts: Vec<&str> = core.split('.').collect();
        if parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())) {
            out.push(piece);
        }
    }
    out
}

/// A long list of published versions.
pub fn detect(text: &str) -> bool {
    versions(text).len() >= MIN_VERSIONS
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    render(text).unwrap_or_else(|| text.to_string())
}

fn render(text: &str) -> Option<String> {
    let all = versions(text);
    if all.len() < MIN_VERSIONS {
        return None;
    }
    let stable: Vec<&str> = all.iter().copied().filter(|v| !v.contains('-')).collect();
    let pre: Vec<&str> = all.iter().copied().filter(|v| v.contains('-')).collect();
    let tail = |v: &[&str], n: usize| v[v.len().saturating_sub(n)..].join(", ");
    let mut out = format!(
        "[tokenade:npm-view-versions] {} version(s), {} stable, {} pre-release; oldest {}\n",
        all.len(),
        stable.len(),
        pre.len(),
        all[0]
    );
    if !stable.is_empty() {
        out.push_str(&format!("latest stable ({}): {}\n", stable.len().min(STABLE_SHOWN), tail(&stable, STABLE_SHOWN)));
    }
    if !pre.is_empty() {
        out.push_str(&format!("latest pre-release ({}): {}\n", pre.len().min(PRE_SHOWN), tail(&pre, PRE_SHOWN)));
    }
    Some(out.trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn published() -> Vec<String> {
        let mut v = Vec::new();
        for major in 0..6 {
            for minor in 0..9 {
                v.push(format!("{major}.{minor}.0-beta"));
                v.push(format!("{major}.{minor}.0"));
                v.push(format!("{major}.{minor}.{}", minor % 4 + 1));
            }
        }
        v.push("6.0.0-dev.20261001.1".into());
        v.push("6.0.0-dev.20261002.1".into());
        v
    }

    /// The newest end is what the command is run for: the generic cap kept
    /// the oldest twenty and counted the rest.
    #[test]
    fn the_newest_versions_are_the_ones_kept() {
        let v = published();
        let json = format!("[{}]", v.iter().map(|s| format!("\"{s}\"")).collect::<Vec<_>>().join(","));
        let out = render(&json).expect("a long version list");
        assert!(out.starts_with("[tokenade:npm-view-versions] 164 version(s), 108 stable, 56 pre-release; oldest 0.0.0-beta"), "{out}");
        assert!(out.contains("latest stable (15): "), "{out}");
        assert!(out.contains("5.8.0, 5.8.1"), "{out}");
        assert!(out.contains("6.0.0-dev.20261002.1"), "{out}");
        assert!(!out.contains("0.1.2,"), "an old version leaked into the latest lists: {out}");
        // The JavaScript form npm prints without --json reads the same.
        let js = format!("[\n  {}\n]", v.iter().map(|s| format!("'{s}'")).collect::<Vec<_>>().join(",\n  "));
        assert_eq!(render(&js), Some(out));
        assert_eq!(render("[\"1.0.0\",\"1.0.1\"]"), None, "a short list is read as it is");
    }
}
