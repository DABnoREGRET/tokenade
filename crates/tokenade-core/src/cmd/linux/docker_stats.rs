//! `docker_stats` — docker stats.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("CPU %") || text.contains("MEM USAGE")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "docker-stats", "container stat(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str =
        "CONTAINER ID   NAME   CPU %   MEM USAGE / LIMIT   MEM %   NET I/O   BLOCK I/O   PIDS";

    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("{:012x}   web{i}   0.5%   50MiB / 512MiB   9.7%   1kB / 1kB   0B / 0B   2\n", i))
            .collect();
        format!("{HEADER}\n{rows}")
    }

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("   \n"));
    }

    #[test]
    fn detector_accepts_stats_header() {
        assert!(detect(HEADER));
    }

    #[test]
    fn short_table_passes_through() {
        assert!(try_compact(&make_table(5)).is_none());
    }

    #[test]
    fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:docker-stats] 30 container stat(s)"));
        assert!(out.contains("(+10 more)"));
        assert!(out.contains("CPU %"));
    }

    #[test]
    fn cpu_and_mem_fields_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("CPU %"));
        assert!(out.contains("MEM USAGE"));
    }
}
