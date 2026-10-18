
const HEAD: usize = 80;

pub fn detect(text: &str) -> bool {
    text.lines().any(|l| {
        let t = l.trim();
        t.starts_with("PRE ") ||
        (t.len() > 20 && t.as_bytes().iter().take(4).all(|b| b.is_ascii_digit()))
    })
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    crate::archetypes::form_aware(text, HEAD, 5, "aws-s3-ls", "S3 entry(s)")
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
        let line = "                           PRE backups/\\n";
        let s = format!("{line}\n{line}\n{line}\n");
        assert!(try_compact(&s).is_none());
    }

    #[test]
    fn long_output_folds_non_decapitating() {
        let line = "                           PRE backups/\\n";
        let s: String = (0..(HEAD + 20)).map(|_| format!("{line}\n")).collect();
        let out = try_compact(&s).expect("long structured output should fold");
        assert!(out.contains("[tokenade:aws-s3-ls"));
    }
}
