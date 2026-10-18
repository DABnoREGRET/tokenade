//! `docker_top_proc` — docker top.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("UID") && text.contains("PID") && text.contains("CMD")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "docker-top", "process(es)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "UID   PID   PPID   C   STIME   TTY   TIME     CMD";

    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("root   {i}   1   0   00:00   ?   00:00:00   /usr/bin/proc{i}\n"))
            .collect();
        format!("{HEADER}\n{rows}")
    }

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("   \n"));
    }

    #[test]
    fn detector_accepts_top_header() {
        assert!(detect(HEADER));
    }

    #[test]
    fn short_table_passes_through() {
        assert!(try_compact(&make_table(5)).is_none());
    }

    #[test]
    fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:docker-top] 30 process(es)"));
        assert!(out.contains("(+10 more)"));
        assert!(out.contains("PID"));
    }

    #[test]
    fn pid_and_cmd_fields_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("UID"));
        assert!(out.contains("PID"));
        assert!(out.contains("CMD"));
    }
}
