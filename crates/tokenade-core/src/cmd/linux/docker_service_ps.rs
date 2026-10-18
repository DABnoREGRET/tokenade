//! `docker_service_ps` — docker service ps.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("DESIRED STATE") || text.contains("CURRENT STATE")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "docker-service-ps", "task(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str =
        "ID             NAME      IMAGE          NODE      DESIRED STATE   CURRENT STATE";

    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("{:013x}   task{i}   nginx:latest   node1   Running   Running 5m\n", i))
            .collect();
        format!("{HEADER}\n{rows}")
    }

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("   \n"));
    }

    #[test]
    fn detector_accepts_service_ps_header() {
        assert!(detect(HEADER));
    }

    #[test]
    fn short_table_passes_through() {
        assert!(try_compact(&make_table(5)).is_none());
    }

    #[test]
    fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:docker-service-ps] 30 task(s)"));
        assert!(out.contains("(+10 more)"));
        assert!(out.contains("DESIRED STATE"));
    }

    #[test]
    fn state_fields_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("DESIRED STATE"));
        assert!(out.contains("CURRENT STATE"));
    }
}
