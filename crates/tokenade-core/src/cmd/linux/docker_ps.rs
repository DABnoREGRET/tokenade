//! `docker_ps` — docker ps.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("CONTAINER ID") && (text.contains("STATUS") || text.contains("PORTS"))
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "docker-ps", "container(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str =
        "CONTAINER ID   IMAGE          COMMAND   CREATED   STATUS    PORTS     NAMES";

    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("{:012x}   nginx   /bin/sh   5m   Up 5m   80/tcp   web{i}\n", i))
            .collect();
        format!("{HEADER}\n{rows}")
    }

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("   \n"));
    }

    #[test]
    fn detector_accepts_ps_header() {
        assert!(detect(HEADER));
    }

    #[test]
    fn short_table_passes_through() {
        assert!(try_compact(&make_table(5)).is_none());
    }

    #[test]
    fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:docker-ps] 30 container(s)"));
        assert!(out.contains("(+10 more)"));
        assert!(out.contains(HEADER));
        assert!(out.contains("web0"));
        assert!(!out.contains("web20"));
    }
}
