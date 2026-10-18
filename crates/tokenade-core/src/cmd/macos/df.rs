//! macOS BSD `df -h`.
//!
//! Mostly compatible with Linux `df` but BSD `df` adds `iused` /
//! `ifree` / `%iused` columns (`df -h -i`). The Linux disk module
//! handles the common case; we add a tighter macOS detector for
//! the routing layer.


pub fn detect(text: &str) -> bool {
    text.contains("Filesystem") || text.contains("Mounted")
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    // Reuse Linux compactor (same column layout when not in -i mode).
    crate::disk::compact(text)
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_bsd_df_with_inodes() {
        let s = "Filesystem     Size  Used Avail Capacity iused ifree %iused Mounted on\n/dev/disk1s1   500G  300G  200G    60%   1000 9999    10%   /\n";
        assert!(detect(s));
    }
}
