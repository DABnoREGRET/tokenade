//! `docker_service_ls` — docker service ls.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("REPLICAS") || (text.contains("MODE") && text.contains("IMAGE"))
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "docker-service", "service(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "ID             NAME         MODE         REPLICAS   IMAGE          PORTS";

    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("{:013x}   svc{i}   replicated   3/3   nginx:latest   *:80->80/tcp\n", i))
            .collect();
        format!("{HEADER}\n{rows}")
    }

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("   \n"));
    }

    #[test]
    fn detector_accepts_service_header() {
        assert!(detect(HEADER));
    }

    #[test]
    fn short_table_passes_through() {
        assert!(try_compact(&make_table(5)).is_none());
    }

    #[test]
    fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:docker-service] 30 service(s)"));
        assert!(out.contains("(+10 more)"));
        assert!(out.contains("REPLICAS"));
    }

    #[test]
    fn mode_and_replicas_fields_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("MODE"));
        assert!(out.contains("REPLICAS"));
        assert!(out.contains("svc0"));
    }
}
