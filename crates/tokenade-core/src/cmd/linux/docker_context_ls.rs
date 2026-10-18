//! `docker_context_ls` — docker context ls.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("DOCKER ENDPOINT")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "docker-context", "context(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str =
        "NAME          DESCRIPTION   DOCKER ENDPOINT               KUBERNETES ENDPOINT";

    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("ctx{i}   Context {i}   unix:///var/run/docker.sock   \n"))
            .collect();
        format!("{HEADER}\n{rows}")
    }

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("   \n"));
    }

    #[test]
    fn detector_accepts_context_header() {
        assert!(detect(HEADER));
    }

    #[test]
    fn short_table_passes_through() {
        assert!(try_compact(&make_table(5)).is_none());
    }

    #[test]
    fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:docker-context] 30 context(s)"));
        assert!(out.contains("(+10 more)"));
        assert!(out.contains("DOCKER ENDPOINT"));
    }

    #[test]
    fn endpoint_field_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("DOCKER ENDPOINT"));
        assert!(out.contains("ctx0"));
    }
}
