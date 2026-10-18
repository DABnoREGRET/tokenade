//! `docker_node_ls` — docker node ls.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("HOSTNAME") && text.contains("AVAILABILITY")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "docker-node", "node(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str =
        "ID                            HOSTNAME        STATUS    AVAILABILITY   MANAGER STATUS";

    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("{:028x}   node{i}   Ready   Active   Leader\n", i))
            .collect();
        format!("{HEADER}\n{rows}")
    }

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("   \n"));
    }

    #[test]
    fn detector_accepts_node_header() {
        assert!(detect(HEADER));
    }

    #[test]
    fn short_table_passes_through() {
        assert!(try_compact(&make_table(5)).is_none());
    }

    #[test]
    fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:docker-node] 30 node(s)"));
        assert!(out.contains("(+10 more)"));
        assert!(out.contains("HOSTNAME"));
    }

    #[test]
    fn status_and_availability_fields_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("STATUS"));
        assert!(out.contains("AVAILABILITY"));
        assert!(out.contains("node0"));
    }
}
