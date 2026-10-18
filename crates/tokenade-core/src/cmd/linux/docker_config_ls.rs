//! `docker_config_ls` — docker config ls.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    // Same header as docker secret ls; dispatch is command-name based in practice.
    text.contains("ID") && text.contains("CREATED") && text.contains("UPDATED")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "docker-config", "config(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "ID                          NAME              CREATED          UPDATED";

    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("{:028x}   config{i}   2 days ago   2 days ago\n", i))
            .collect();
        format!("{HEADER}\n{rows}")
    }

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("   \n"));
    }

    #[test]
    fn detector_accepts_config_header() {
        assert!(detect(HEADER));
    }

    #[test]
    fn short_table_passes_through() {
        assert!(try_compact(&make_table(5)).is_none());
    }

    #[test]
    fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:docker-config] 30 config(s)"));
        assert!(out.contains("(+10 more)"));
        assert!(out.contains("CREATED"));
    }

    #[test]
    fn name_and_timestamps_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("NAME"));
        assert!(out.contains("CREATED"));
        assert!(out.contains("config0"));
    }
}
