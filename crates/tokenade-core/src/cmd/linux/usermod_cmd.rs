//! `usermod_cmd` — usermod cmd.
//!
//! Form-aware: detects a columnar table (folds it, surfacing problem
//! rows) vs a plain listing (keeps head AND tail so the final outcome
//! line is never decapitated). Falls back to passthrough otherwise.
const HEAD: usize = 50;

pub fn detect(text: &str) -> bool {
    crate::archetypes::table_or_over_budget(text, HEAD)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    crate::archetypes::table_or_head_tail(text, HEAD, 5, "usermod-cmd", "line(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detector_rejects_empty() { assert!(!detect("")); }

    #[test]
    fn table_folds_and_surfaces_problem_rows() {
        let mut t = String::from("NAME   STATUS   AGE
");
        for i in 0..(HEAD + 20) { t.push_str(&format!("res-{i}   Running   {i}m
")); }
        t.push_str("res-bad   Error   9m
");
        let out = try_compact(&t).unwrap();
        assert!(out.contains("STATUS"));
        assert!(out.contains("res-bad"));
    }

    #[test]
    fn plain_listing_keeps_tail() {
        let mut t = String::new();
        for i in 0..(HEAD + 20) { t.push_str(&format!("item {i}
")); }
        t.push_str("FINAL OUTCOME LINE
");
        let out = try_compact(&t).unwrap();
        assert!(out.contains("FINAL OUTCOME LINE"));
        assert!(out.contains("item 0"));
    }

    #[test]
    fn short_output_passes_through() {
        assert!(try_compact("one line
two line
").is_none());
    }
}
