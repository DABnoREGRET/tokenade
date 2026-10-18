//! `docker_history` — docker history.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("CREATED BY")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "docker-history", "layer(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str =
        "IMAGE          CREATED          CREATED BY                          SIZE      COMMENT";

    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("{:012x}   2 days ago   /bin/sh -c #(nop) CMD {i}   0B   \n", i))
            .collect();
        format!("{HEADER}\n{rows}")
    }

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("   \n"));
    }

    #[test]
    fn detector_accepts_history_header() {
        assert!(detect(HEADER));
    }

    #[test]
    fn short_table_passes_through() {
        assert!(try_compact(&make_table(5)).is_none());
    }

    #[test]
    fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:docker-history] 30 layer(s)"));
        assert!(out.contains("(+10 more)"));
        assert!(out.contains("CREATED BY"));
    }

    #[test]
    fn created_by_and_size_fields_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("CREATED BY"));
        assert!(out.contains("SIZE"));
    }
}
