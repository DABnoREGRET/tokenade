//! `docker_compose` — docker compose ps.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("SERVICE") && (text.contains("STATUS") || text.contains("COMMAND"))
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "docker-compose-ps", "service(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "NAME       IMAGE     COMMAND     SERVICE   CREATED      STATUS      PORTS";

    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("app-svc{i}   nginx   /bin/sh   web   5m ago   Up 5m   80/tcp\n"))
            .collect();
        format!("{HEADER}\n{rows}")
    }

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("   \n"));
    }

    #[test]
    fn detector_accepts_compose_ps_header() {
        assert!(detect(HEADER));
    }

    #[test]
    fn short_table_passes_through() {
        assert!(try_compact(&make_table(5)).is_none());
    }

    #[test]
    fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:docker-compose-ps] 30 service(s)"));
        assert!(out.contains("(+10 more)"));
        assert!(out.contains("SERVICE"));
    }

    #[test]
    fn service_and_status_fields_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("SERVICE"));
        assert!(out.contains("STATUS"));
        assert!(out.contains("app-svc0"));
    }
}
