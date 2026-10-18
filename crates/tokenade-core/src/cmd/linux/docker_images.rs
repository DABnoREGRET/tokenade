//! `docker_images` — docker images.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("REPOSITORY") && text.contains("TAG") && text.contains("IMAGE ID")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "docker-images", "image(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "REPOSITORY      TAG       IMAGE ID       CREATED        SIZE";

    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("nginx   {i}.0   {:012x}   2 days ago   50MB\n", i))
            .collect();
        format!("{HEADER}\n{rows}")
    }

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("   \n"));
    }

    #[test]
    fn detector_accepts_images_header() {
        assert!(detect(HEADER));
    }

    #[test]
    fn short_table_passes_through() {
        assert!(try_compact(&make_table(5)).is_none());
    }

    #[test]
    fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:docker-images] 30 image(s)"));
        assert!(out.contains("(+10 more)"));
        assert!(out.contains("REPOSITORY"));
    }

    #[test]
    fn repository_and_tag_fields_preserved() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("nginx   0.0"));
        assert!(out.contains("nginx   19.0"));
        assert!(!out.contains("nginx   20.0"));
    }
}
