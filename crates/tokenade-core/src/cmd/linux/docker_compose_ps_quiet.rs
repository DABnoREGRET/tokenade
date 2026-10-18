
const HEAD: usize = 30;

pub fn detect(text: &str) -> bool {
    text.lines().any(|l| {
        let t = l.trim();
        t.len() == 12 && t.chars().all(|c| c.is_ascii_hexdigit())
    })
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    crate::archetypes::form_aware(text, HEAD, 5, "docker-compose-ps-quiet", "container ID(s)")
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
        let s = "abc123def456\n000000000001\nfeedfacecafe\n";
        assert!(try_compact(s).is_none());
    }

    #[test]
    fn long_output_folds_non_decapitating() {
        let s: String = (0..(HEAD + 20)).map(|i| format!("{:012x}\n", i)).collect();
        let out = try_compact(&s).expect("long structured output should fold");
        assert!(out.contains("[tokenade:docker-compose-ps-quiet"));
    }
}
