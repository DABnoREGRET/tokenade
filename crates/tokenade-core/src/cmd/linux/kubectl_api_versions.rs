
const HEAD: usize = 30;

pub fn detect(text: &str) -> bool {
    // Output is one version per line: "v1", "apps/v1", "networking.k8s.io/v1", etc.
    text.lines().any(|l| {
        let t = l.trim();
        !t.is_empty() && (t == "v1" || t.contains("/v"))
    })
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    crate::archetypes::form_aware(text, HEAD, 5, "kubectl-api-versions", "API version(s)")
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
        let line = "v1\\napps/v1\\nnetworking.k8s.io/v1\\n";
        let s = format!("{line}\n{line}\n{line}\n");
        assert!(try_compact(&s).is_none());
    }

    #[test]
    fn long_output_folds_non_decapitating() {
        let line = "v1\\napps/v1\\nnetworking.k8s.io/v1\\n";
        let s: String = (0..(HEAD + 20)).map(|_| format!("{line}\n")).collect();
        let out = try_compact(&s).expect("long structured output should fold");
        assert!(out.contains("[tokenade:kubectl-api-versions"));
    }
}
