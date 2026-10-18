
const HEAD: usize = 40;

pub fn detect(text: &str) -> bool {
    text.lines().any(|l| {
        let t = l.trim();
        t.starts_with(['A', 'C', 'D'])
            && t.len() > 2
            && t.as_bytes()[1] == b' '
            && t.as_bytes()[2] == b'/'
    })
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    crate::archetypes::form_aware(text, HEAD, 5, "docker-diff", "diff entry(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("   \n"));
    }

    #[test]
    fn short_output_passes_through() {
        let line = "A /etc/somefile\\nC /usr/bin/thing\\nD /tmp/old\\n";
        let s = format!("{line}\n{line}\n{line}\n");
        assert!(try_compact(&s).is_none());
    }

    #[test]
    fn long_output_folds_non_decapitating() {
        let line = "A /etc/somefile\\nC /usr/bin/thing\\nD /tmp/old\\n";
        let s: String = (0..(HEAD + 20)).map(|_| format!("{line}\n")).collect();
        let out = try_compact(&s).expect("long structured output should fold");
        assert!(out.contains("[tokenade:docker-diff"));
    }
}
