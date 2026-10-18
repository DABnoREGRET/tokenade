//! `docker_volume` — docker volume ls.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("DRIVER") && text.contains("VOLUME NAME")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "docker-volume", "volume(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "DRIVER    VOLUME NAME";

    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("local   myvolume{i}\n"))
            .collect();
        format!("{HEADER}\n{rows}")
    }

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("   \n"));
    }

    #[test]
    fn detector_accepts_volume_header() {
        assert!(detect(HEADER));
    }

    #[test]
    fn short_table_passes_through() {
        assert!(try_compact(&make_table(5)).is_none());
    }

    #[test]
    fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:docker-volume] 30 volume(s)"));
        assert!(out.contains("(+10 more)"));
        assert!(out.contains("VOLUME NAME"));
    }

    #[test]
    fn driver_and_name_fields_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("DRIVER"));
        assert!(out.contains("VOLUME NAME"));
        assert!(out.contains("myvolume0"));
    }
}
