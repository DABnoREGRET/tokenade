//! `docker_plugin_ls` — docker plugin ls.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("ENABLED") && text.contains("NAME")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "docker-plugin", "plugin(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "ID             NAME              DESCRIPTION         ENABLED";

    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("{:013x}   plugin{i}   A plugin {i}   true\n", i))
            .collect();
        format!("{HEADER}\n{rows}")
    }

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("   \n"));
    }

    #[test]
    fn detector_accepts_plugin_header() {
        assert!(detect(HEADER));
    }

    #[test]
    fn short_table_passes_through() {
        assert!(try_compact(&make_table(5)).is_none());
    }

    #[test]
    fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:docker-plugin] 30 plugin(s)"));
        assert!(out.contains("(+10 more)"));
        assert!(out.contains("ENABLED"));
    }

    #[test]
    fn name_and_enabled_fields_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("NAME"));
        assert!(out.contains("ENABLED"));
    }
}
